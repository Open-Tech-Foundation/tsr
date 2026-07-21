//! Task execution engine (SPEC §5, §6, §8).
//!
//! Each task runs its `deps` (as a batch, sequential unless `parallel = true`)
//! and then its own command — a single spawn, a `packages` fan-out batch, or, for
//! a deps-only aggregator, nothing. Tasks are de-duplicated so a diamond runs a
//! shared dependency once.
//!
//! Failure handling is fail-fast (SPEC §5.2): the first non-zero child sets a
//! shared abort flag; sequential batches stop launching, parallel siblings are
//! killed (leaf spawns poll the flag), and a summary is printed. The first
//! failing child's exact exit code is propagated (SPEC §10).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Config, Task};
use crate::env;
use crate::error::TsrError;
use crate::resolve::{self, Invocation};
use crate::shell::{self, ExecPlan, RunPlan, Sep};
use crate::workspace;

/// Adaptive poll backoff while waiting on a child (SPEC §5.2). Starting small
/// keeps fast tasks (`true`, `echo`) near their true cost — a fixed interval
/// would add a full tick of latency to every quick command — while the cap keeps
/// fail-fast kill latency bounded for long-running ones.
const POLL_MIN: Duration = Duration::from_micros(100);
const POLL_MAX: Duration = Duration::from_millis(20);

/// Run `root` and its dependency tree, owning all failure reporting. Returns the
/// process exit code to propagate (SPEC §10): `0` on success, the first failing
/// child's exact code, or `64` when the runner itself could not proceed (bad
/// spawn, missing delegate, unmatched `packages`, …). `passthrough` is forwarded
/// to the root task's own command (SPEC §6).
pub fn run(cfg: &Config, root: &str, passthrough: &[String]) -> i32 {
    let ctx = Ctx::new(cfg);
    let _ = ctx.run_task(root, passthrough, true);

    let runner_error = ctx.runner_error.lock().unwrap().clone();
    let first_failure = *ctx.first_failure.lock().unwrap();

    // A genuine child failure yields its exact code; otherwise a runner-level
    // failure is 64; otherwise success.
    let code = match (first_failure, &runner_error) {
        (Some(c), _) => c,
        (None, Some(_)) => crate::error::EXIT_RUNNER_ERROR,
        (None, None) => 0,
    };

    if code != 0 {
        ctx.print_summary(root, code, runner_error.as_deref());
    }
    code
}

/// Control-flow status of a task or job. `Copy` so it can be memoised cheaply;
/// the runner-error detail lives on [`Ctx`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Ok,
    Failed(i32),
    Skipped,
    Runner,
}

impl Status {
    fn is_ok(self) -> bool {
        matches!(self, Status::Ok)
    }
}

/// A recorded leaf result, for the failure summary.
#[derive(Debug, Clone)]
struct JobResult {
    label: String,
    kind: ResultKind,
    dur: Option<Duration>,
}

#[derive(Debug, Clone, Copy)]
enum ResultKind {
    Ok,
    Failed(i32),
    Skipped,
}

/// What a leaf job actually executes.
enum Action {
    /// A single direct command (`execvp`-style).
    Spawn { program: String, args: Vec<String> },
    /// A mini-shell command sequence.
    Shell(ExecPlan),
}

/// A concrete unit of work: one command (or mini-shell sequence) in one
/// directory with one merged environment.
struct Job {
    label: String,
    dir: PathBuf,
    env: HashMap<String, String>,
    action: Action,
}

/// Memoisation slot so each task runs at most once (diamond-safe).
struct TaskSlot {
    state: Mutex<SlotState>,
    done: Condvar,
}

enum SlotState {
    Running,
    Done(Status),
}

/// Shared execution state.
struct Ctx<'a> {
    cfg: &'a Config,
    aborted: AtomicBool,
    /// First failing child's exact exit code (set once; wall-clock-first wins).
    first_failure: Mutex<Option<i32>>,
    /// First runner-level failure message (bad spawn, missing package, …).
    runner_error: Mutex<Option<String>>,
    results: Mutex<Vec<JobResult>>,
    memo: Mutex<HashMap<String, std::sync::Arc<TaskSlot>>>,
}

impl<'a> Ctx<'a> {
    fn new(cfg: &'a Config) -> Ctx<'a> {
        Ctx {
            cfg,
            aborted: AtomicBool::new(false),
            first_failure: Mutex::new(None),
            runner_error: Mutex::new(None),
            results: Mutex::new(Vec::new()),
            memo: Mutex::new(HashMap::new()),
        }
    }

    fn aborted(&self) -> bool {
        self.aborted.load(Ordering::SeqCst)
    }

    fn abort(&self) {
        self.aborted.store(true, Ordering::SeqCst);
    }

    fn note_failure(&self, code: i32) {
        let mut f = self.first_failure.lock().unwrap();
        if f.is_none() {
            *f = Some(code);
        }
        self.abort();
    }

    fn note_runner(&self, msg: String) {
        let mut r = self.runner_error.lock().unwrap();
        if r.is_none() {
            *r = Some(msg);
        }
        self.abort();
    }

    fn record(&self, label: &str, kind: ResultKind, dur: Option<Duration>) {
        self.results.lock().unwrap().push(JobResult {
            label: label.to_string(),
            kind,
            dur,
        });
    }

    // --- task execution ---

    /// Run a task by key, memoising so it executes at most once.
    fn run_task(&self, key: &str, passthrough: &[String], is_root: bool) -> Status {
        use std::sync::Arc;

        // Claim or find the memo slot.
        let (slot, owner) = {
            let mut memo = self.memo.lock().unwrap();
            match memo.get(key) {
                Some(s) => (s.clone(), false),
                None => {
                    let s = Arc::new(TaskSlot {
                        state: Mutex::new(SlotState::Running),
                        done: Condvar::new(),
                    });
                    memo.insert(key.to_string(), s.clone());
                    (s, true)
                }
            }
        };

        if !owner {
            // Another invocation owns this task; wait for its result.
            let mut st = slot.state.lock().unwrap();
            loop {
                match &*st {
                    SlotState::Done(status) => return *status,
                    SlotState::Running => st = slot.done.wait(st).unwrap(),
                }
            }
        }

        let status = self.run_task_inner(key, passthrough, is_root);
        let mut st = slot.state.lock().unwrap();
        *st = SlotState::Done(status);
        slot.done.notify_all();
        status
    }

    fn run_task_inner(&self, key: &str, passthrough: &[String], _is_root: bool) -> Status {
        if self.aborted() {
            return Status::Skipped;
        }
        let task = match self.cfg.task(key) {
            Some(t) => t,
            None => {
                self.note_runner(format!("unknown task '{key}'"));
                return Status::Runner;
            }
        };

        // 1. Dependencies first (SPEC §5). Their batch honours *this* task's
        //    `parallel` flag. A dep failure fails the task (own command skipped).
        if !task.deps.is_empty() {
            let dep_status = self.run_task_batch(&task.deps, task.parallel);
            if !dep_status.is_ok() {
                return dep_status;
            }
        }
        if self.aborted() {
            return Status::Skipped;
        }

        // 2. The task's own command.
        let has_own = task.run.is_some() || task.delegate.is_some() || task.packages.is_some();

        if let Some(patterns) = &task.packages {
            self.run_packages(task, patterns, passthrough)
        } else if !has_own {
            // A deps-only task is a pure aggregator: nothing of its own to run.
            Status::Ok
        } else {
            let dir = self.task_dir(task);
            match self.build_job(task, &dir, key.to_string(), passthrough) {
                Ok(job) => self.run_leaf(job),
                Err(msg) => {
                    self.note_runner(msg);
                    Status::Runner
                }
            }
        }
    }

    /// Fan the task out across matching packages (SPEC §9.1), as a batch that
    /// honours the task's `parallel` flag.
    fn run_packages(&self, task: &Task, patterns: &[String], passthrough: &[String]) -> Status {
        let pkgs = match workspace::match_packages(self.cfg, patterns, &task.key) {
            Ok(p) => p,
            Err(e) => {
                self.note_runner(strip_error(&e));
                return Status::Runner;
            }
        };

        let mut jobs = Vec::with_capacity(pkgs.len());
        for pkg in &pkgs {
            let label = format!("{} ({})", task.key, pkg.rel);
            match self.build_job(task, &pkg.path, label, passthrough) {
                Ok(job) => jobs.push(job),
                Err(msg) => {
                    self.note_runner(msg);
                    return Status::Runner;
                }
            }
        }
        self.run_job_batch(jobs, task.parallel)
    }

    fn task_dir(&self, task: &Task) -> PathBuf {
        match &task.dir {
            Some(d) => self.cfg.root.join(d),
            None => self.cfg.root.clone(),
        }
    }

    /// Resolve a task's form into a runnable [`Job`] (SPEC §3.1, §6, §8).
    fn build_job(
        &self,
        task: &Task,
        dir: &Path,
        label: String,
        passthrough: &[String],
    ) -> std::result::Result<Job, String> {
        let env = env::build(self.cfg, task);
        let extra = |base: Vec<String>| -> Vec<String> {
            // args (SPEC §6) then CLI passthrough, appended to the resolved args.
            let mut v = base;
            v.extend(task.args.iter().cloned());
            v.extend(passthrough.iter().cloned());
            v
        };

        let invocation = resolve::resolve(task, dir).map_err(|e| strip_error(&e))?;
        let action = match invocation {
            Invocation::Direct { program, args } => Action::Spawn {
                program,
                args: extra(args),
            },
            Invocation::Run(s) => match shell::parse(&s).map_err(|e| strip_error(&e))? {
                RunPlan::Direct(argv) => {
                    let mut it = argv.into_iter();
                    let program = it
                        .next()
                        .ok_or_else(|| "'run' string is empty".to_string())?;
                    Action::Spawn {
                        program,
                        args: extra(it.collect()),
                    }
                }
                RunPlan::Shell(program) => {
                    let mut plan = program
                        .expand(&|k| env.get(k).cloned())
                        .map_err(|e| strip_error(&e))?;
                    append_to_last(&mut plan, &extra(Vec::new()));
                    Action::Shell(plan)
                }
            },
        };

        Ok(Job {
            label,
            dir: dir.to_path_buf(),
            env,
            action,
        })
    }

    // --- batching ---

    /// Run a batch of dependency tasks, fail-fast (SPEC §5.1, §5.2).
    fn run_task_batch(&self, keys: &[String], parallel: bool) -> Status {
        if parallel {
            let statuses: Vec<Status> = std::thread::scope(|scope| {
                let handles: Vec<_> = keys
                    .iter()
                    .map(|k| scope.spawn(move || self.run_task(k, &[], false)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            self.combine(&statuses)
        } else {
            self.run_sequential(keys.len(), |i| self.run_task(&keys[i], &[], false))
        }
    }

    /// Run a batch of leaf jobs, fail-fast.
    fn run_job_batch(&self, jobs: Vec<Job>, parallel: bool) -> Status {
        if parallel {
            let statuses: Vec<Status> = std::thread::scope(|scope| {
                let handles: Vec<_> = jobs
                    .into_iter()
                    .map(|job| scope.spawn(move || self.run_leaf(job)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            self.combine(&statuses)
        } else {
            let mut jobs = jobs;
            let n = jobs.len();
            let mut drained: Vec<Option<Job>> = jobs.drain(..).map(Some).collect();
            self.run_sequential(n, |i| self.run_leaf(drained[i].take().unwrap()))
        }
    }

    /// Sequential fail-fast: stop launching on the first failure; remaining
    /// items are recorded as skipped.
    fn run_sequential(&self, n: usize, mut run: impl FnMut(usize) -> Status) -> Status {
        let mut result = Status::Ok;
        for i in 0..n {
            if !result.is_ok() {
                // A prior item failed: don't launch the rest (SPEC §5.2).
                continue;
            }
            let s = run(i);
            if !s.is_ok() {
                result = s;
            }
        }
        result
    }

    /// Combine a parallel batch's statuses into one, preferring failures.
    fn combine(&self, statuses: &[Status]) -> Status {
        if statuses.iter().any(|s| matches!(s, Status::Runner)) {
            Status::Runner
        } else if let Some(code) = statuses.iter().find_map(|s| match s {
            Status::Failed(c) => Some(*c),
            _ => None,
        }) {
            Status::Failed(code)
        } else if statuses.iter().any(|s| matches!(s, Status::Skipped)) {
            Status::Skipped
        } else {
            Status::Ok
        }
    }

    // --- leaf execution ---

    fn run_leaf(&self, job: Job) -> Status {
        if self.aborted() {
            self.record(&job.label, ResultKind::Skipped, None);
            return Status::Skipped;
        }
        let start = Instant::now();
        let wait = self.execute_action(&job);
        let dur = start.elapsed();

        match wait {
            LeafWait::Exited(0) => {
                self.record(&job.label, ResultKind::Ok, Some(dur));
                Status::Ok
            }
            LeafWait::Exited(code) => {
                self.note_failure(code);
                self.record(&job.label, ResultKind::Failed(code), Some(dur));
                Status::Failed(code)
            }
            LeafWait::Killed => {
                self.record(&job.label, ResultKind::Skipped, Some(dur));
                Status::Skipped
            }
            LeafWait::SpawnFailed(msg) => {
                self.note_runner(msg);
                self.record(&job.label, ResultKind::Failed(64), Some(dur));
                Status::Runner
            }
        }
    }

    /// Execute a job's action, returning how it finished.
    fn execute_action(&self, job: &Job) -> LeafWait {
        match &job.action {
            Action::Spawn { program, args } => self.spawn_wait(program, args, job),
            Action::Shell(plan) => self.run_shell(plan, job),
        }
    }

    /// Run a mini-shell sequence with `&&`/`||`/`;` semantics (SPEC §8.1),
    /// checking the abort flag between commands.
    fn run_shell(&self, plan: &ExecPlan, job: &Job) -> LeafWait {
        let mut last = match self.spawn_wait(&plan.first.argv[0], &plan.first.argv[1..], job) {
            LeafWait::Exited(c) => c,
            other => return other,
        };
        for (sep, cmd) in &plan.rest {
            if self.aborted() {
                return LeafWait::Killed;
            }
            let should_run = match sep {
                Sep::And => last == 0,
                Sep::Or => last != 0,
                Sep::Semi => true,
            };
            if should_run {
                match self.spawn_wait(&cmd.argv[0], &cmd.argv[1..], job) {
                    LeafWait::Exited(c) => last = c,
                    other => return other,
                }
            }
        }
        LeafWait::Exited(last)
    }

    /// Spawn one child and wait, polling the abort flag so a fail-fast can kill
    /// it mid-run (SPEC §5.2).
    fn spawn_wait(&self, program: &str, args: &[String], job: &Job) -> LeafWait {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&job.dir)
            .env_clear()
            .envs(&job.env);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                return LeafWait::SpawnFailed(format!("cannot run '{program}': {e}"));
            }
        };

        let mut backoff = POLL_MIN;
        loop {
            if self.aborted() {
                let _ = child.kill();
                let _ = child.wait();
                return LeafWait::Killed;
            }
            match child.try_wait() {
                Ok(Some(status)) => return LeafWait::Exited(exit_code_of(status)),
                Ok(None) => {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(POLL_MAX);
                }
                Err(e) => return LeafWait::SpawnFailed(e.to_string()),
            }
        }
    }

    // --- reporting ---

    fn print_summary(&self, root: &str, code: i32, runner_error: Option<&str>) {
        let results = self.results.lock().unwrap();
        eprintln!();
        eprintln!("✗ {root} failed");
        eprintln!();
        let width = results.iter().map(|r| r.label.len()).max().unwrap_or(0);
        for r in results.iter() {
            let (sym, status) = match r.kind {
                ResultKind::Ok => ("✓", "ok".to_string()),
                ResultKind::Failed(c) => ("✗", format!("exit {c}")),
                ResultKind::Skipped => ("⊘", "skipped".to_string()),
            };
            let dur = r
                .dur
                .map(|d| format!("{:.1}s", d.as_secs_f64()))
                .unwrap_or_default();
            eprintln!(
                "  {sym} {label:width$}  {status:<10} {dur}",
                label = r.label,
            );
        }
        eprintln!();
        if let Some(msg) = runner_error {
            eprintln!("  {msg}");
            eprintln!();
        }
        eprintln!("exit code: {code}");
    }
}

/// How a single leaf command finished.
enum LeafWait {
    Exited(i32),
    Killed,
    SpawnFailed(String),
}

/// Append extra args (task `args` + CLI passthrough) to the final command of a
/// mini-shell sequence — the "resolved command" that passthrough targets.
fn append_to_last(plan: &mut ExecPlan, extra: &[String]) {
    if extra.is_empty() {
        return;
    }
    match plan.rest.last_mut() {
        Some((_, cmd)) => cmd.argv.extend(extra.iter().cloned()),
        None => plan.first.argv.extend(extra.iter().cloned()),
    }
}

/// Strip the `Display` banner so a re-wrapped message reads cleanly.
fn strip_error(e: &TsrError) -> String {
    let s = e.to_string();
    s.strip_prefix("✗ config error: ")
        .or_else(|| s.strip_prefix("✗ error: "))
        .map(str::to_string)
        .unwrap_or(s)
}

/// Extract a child's exit code, mapping signal death to `128 + signal` on unix.
fn exit_code_of(status: std::process::ExitStatus) -> i32 {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(code) = status.code() {
            code
        } else {
            128 + status.signal().unwrap_or(0)
        }
    }
    #[cfg(not(unix))]
    {
        status.code().unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch_root() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-exec-{}-{id}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Write `tasks.toml` at a fresh root and load it. Returns (Config, root).
    fn setup(toml: &str) -> (Config, PathBuf) {
        let root = scratch_root();
        let path = root.join("tasks.toml");
        std::fs::write(&path, toml).unwrap();
        (Config::load(&path).unwrap(), root)
    }

    fn run_task(toml: &str, task: &str) -> i32 {
        let (cfg, _root) = setup(toml);
        graph::validate(&cfg, task).unwrap();
        run(&cfg, task, &[])
    }

    use crate::graph;

    #[test]
    fn single_run_task_succeeds() {
        assert_eq!(run_task("[tasks.ok]\nrun = \"true\"\n", "ok"), 0);
    }

    #[test]
    fn single_run_task_propagates_failure_code() {
        assert_eq!(run_task("[tasks.bad]\nrun = \"false\"\n", "bad"), 1);
    }

    #[test]
    fn propagates_exact_child_exit_code() {
        let toml = "[tasks.x]\ndelegate = { bin = \"sh\", args = [\"-c\", \"exit 3\"] }\n";
        assert_eq!(run_task(toml, "x"), 3);
    }

    #[test]
    fn missing_binary_is_runner_error_64() {
        let toml = "[tasks.x]\nrun = \"definitely-not-a-real-binary-xyz\"\n";
        assert_eq!(run_task(toml, "x"), 64);
    }

    #[test]
    fn deps_run_before_task_and_fail_fast_sequentially() {
        let root = scratch_root();
        let marker = root.join("b-ran");
        let toml = format!(
            "[tasks.ci]\ndeps = [\"a\", \"b\"]\n\
             [tasks.a]\nrun = \"false\"\n\
             [tasks.b]\nrun = \"touch {}\"\n",
            marker.display()
        );
        std::fs::write(root.join("tasks.toml"), &toml).unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        graph::validate(&cfg, "ci").unwrap();
        // a fails → b must be skipped (never launched).
        assert_eq!(run(&cfg, "ci", &[]), 1);
        assert!(!marker.exists(), "sibling 'b' should not have run");
    }

    #[test]
    fn aggregator_runs_only_its_deps() {
        let root = scratch_root();
        let marker = root.join("a-ran");
        let toml = format!(
            "[tasks.top]\ndeps = [\"a\"]\n[tasks.a]\nrun = \"touch {}\"\n",
            marker.display()
        );
        std::fs::write(root.join("tasks.toml"), &toml).unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        graph::validate(&cfg, "top").unwrap();
        assert_eq!(run(&cfg, "top", &[]), 0);
        assert!(marker.exists());
    }

    #[test]
    fn diamond_runs_shared_dep_once() {
        let root = scratch_root();
        let log = root.join("base-log");
        let toml = format!(
            "[tasks.top]\ndeps = [\"a\", \"b\"]\nparallel = true\n\
             [tasks.a]\ndeps = [\"base\"]\n\
             [tasks.b]\ndeps = [\"base\"]\n\
             [tasks.base]\ndelegate = {{ bin = \"sh\", args = [\"-c\", \"echo x >> {}\"] }}\n",
            log.display()
        );
        std::fs::write(root.join("tasks.toml"), &toml).unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        graph::validate(&cfg, "top").unwrap();
        assert_eq!(run(&cfg, "top", &[]), 0);
        let contents = std::fs::read_to_string(&log).unwrap();
        assert_eq!(contents.lines().count(), 1, "base must run exactly once");
    }

    #[test]
    fn parallel_batch_all_succeed() {
        let toml = "[tasks.top]\ndeps = [\"a\", \"b\"]\nparallel = true\n\
                    [tasks.a]\nrun = \"true\"\n[tasks.b]\nrun = \"true\"\n";
        assert_eq!(run_task(toml, "top"), 0);
    }

    #[test]
    fn parallel_fail_fast_kills_slow_sibling() {
        // One dep fails immediately; a slow sibling must be killed, so the whole
        // run finishes well under the sleep duration.
        let toml = "[tasks.top]\ndeps = [\"fast\", \"slow\"]\nparallel = true\n\
                    [tasks.fast]\nrun = \"false\"\n\
                    [tasks.slow]\nrun = \"sleep 5\"\n";
        let (cfg, _r) = setup(toml);
        graph::validate(&cfg, "top").unwrap();
        let start = Instant::now();
        let code = run(&cfg, "top", &[]);
        assert_eq!(code, 1);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "slow sibling not killed"
        );
    }

    #[test]
    fn passthrough_and_args_ordering() {
        // args prepended before CLI passthrough, appended to the resolved command.
        let (cfg, _r) = setup("[tasks.t]\nrun = \"vitest\"\nargs = [\"--color\"]\n");
        let ctx = Ctx::new(&cfg);
        let task = cfg.task("t").unwrap();
        let job = ctx
            .build_job(task, &cfg.root, "t".into(), &["--watch".to_string()])
            .unwrap();
        match job.action {
            Action::Spawn { program, args } => {
                assert_eq!(program, "vitest");
                assert_eq!(args, vec!["--color", "--watch"]);
            }
            _ => panic!("expected direct spawn"),
        }
    }

    #[test]
    fn native_runner_gets_args_and_passthrough() {
        let root = scratch_root();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        std::fs::write(root.join("tasks.toml"), "[tasks.test]\nargs = [\"--ci\"]\n").unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        let ctx = Ctx::new(&cfg);
        let task = cfg.task("test").unwrap();
        let job = ctx
            .build_job(task, &cfg.root, "test".into(), &["--watch".to_string()])
            .unwrap();
        match job.action {
            Action::Spawn { program, args } => {
                assert_eq!(program, "npm");
                assert_eq!(args, vec!["run", "test", "--ci", "--watch"]);
            }
            _ => panic!(),
        }
    }
}
