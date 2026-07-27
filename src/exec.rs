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

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::builtins;
use crate::cli::{Reporter, RunOptions};
use crate::config::{Config, Task, upstream_dep};
use crate::env;
use crate::error::TsrError;
use crate::pkggraph::PackageGraph;
use crate::resolve::{self, Invocation};
use crate::shell::{self, Arg, ExecPlan, ExpandedCommand, RunPlan};
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
pub fn run(cfg: &Config, root: &str, passthrough: &[String], sel: Selection<'_>) -> i32 {
    let ctx = Ctx::new(cfg, sel);
    let started = Instant::now();
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

    if ctx.events_enabled() {
        ctx.emit_summary(root, code, runner_error.as_deref(), started);
    }
    // The human summary is still printed on failure unless the terminal itself
    // is carrying the NDJSON stream.
    if ctx.sel.opts.reporter == Reporter::Human && code != 0 {
        ctx.print_summary(root, code, runner_error.as_deref());
    }
    code
}

/// Which packages a run is restricted to, plus the run's options. Both filters
/// are `None` when the corresponding flag was not given; an empty set would mean
/// "nothing selected", which is a different thing entirely.
#[derive(Debug, Clone, Copy)]
pub struct Selection<'a> {
    /// `--since`: the only packages a fan-out may run in (SPEC §9.3).
    pub affected: Option<&'a HashSet<String>>,
    /// `--resume-from`: packages to treat as already done (SPEC §9.4).
    pub skip: Option<&'a HashSet<String>>,
    pub opts: &'a RunOptions,
    /// `--reporter-file`: an opened NDJSON sink. Owned by the caller so that a
    /// failure to create it is reported before any task runs, rather than after
    /// a long build has already happened.
    pub events: Option<&'a Mutex<std::fs::File>>,
}

impl<'a> Selection<'a> {
    /// No package filtering — an ordinary `tsr <task>`. `main` builds its
    /// `Selection` directly because it has the filters to hand; this is the
    /// convenience form the tests use.
    #[cfg(test)]
    pub fn plain(opts: &'a RunOptions) -> Self {
        Selection {
            affected: None,
            skip: None,
            opts,
            events: None,
        }
    }
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

impl ResultKind {
    /// Stable machine-readable name, used by the NDJSON reporter (SPEC §6.2).
    fn as_str(self) -> &'static str {
        match self {
            ResultKind::Ok => "ok",
            ResultKind::Failed(_) => "failed",
            ResultKind::Skipped => "skipped",
        }
    }
}

/// What a leaf job actually executes.
enum Action {
    /// A resolved `delegate` or auto-detected native-runner command. Always an
    /// external binary — builtins (SPEC §8.5) apply to `run` strings only.
    Spawn { program: String, args: Vec<String> },
    /// A `run` string: one command or a `&&`/`||`/`;` sequence, each command
    /// either a builtin or a spawn.
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
    /// Package filters and run options. `--since` narrows which packages a
    /// fan-out runs in (SPEC §9.3); upstream `^task` work is never filtered, as
    /// it is needed whether or not it changed.
    sel: Selection<'a>,
    aborted: AtomicBool,
    /// First failing child's exact exit code (set once; wall-clock-first wins).
    first_failure: Mutex<Option<i32>>,
    /// First runner-level failure message (bad spawn, missing package, …).
    runner_error: Mutex<Option<String>>,
    results: Mutex<Vec<JobResult>>,
    memo: Mutex<HashMap<String, std::sync::Arc<TaskSlot>>>,
}

impl<'a> Ctx<'a> {
    fn new(cfg: &'a Config, sel: Selection<'a>) -> Ctx<'a> {
        Ctx {
            cfg,
            sel,
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

    /// Record a task failure. Under the default fail-fast policy this also sets
    /// the abort flag; with `--no-bail` the code is remembered but siblings keep
    /// running (SPEC §5.2). Runner-level errors abort either way — they mean
    /// `tsr` itself could not proceed, not that a task failed.
    fn note_failure(&self, code: i32) {
        let mut f = self.first_failure.lock().unwrap();
        if f.is_none() {
            *f = Some(code);
        }
        if !self.sel.opts.no_bail {
            self.abort();
        }
    }

    fn note_runner(&self, msg: String) {
        let mut r = self.runner_error.lock().unwrap();
        if r.is_none() {
            *r = Some(msg);
        }
        self.abort();
    }

    fn record(&self, label: &str, kind: ResultKind, dur: Option<Duration>) {
        if self.events_enabled() {
            self.emit(serde_json::json!({
                "type": "task",
                "label": label,
                "status": kind.as_str(),
                "exitCode": match kind { ResultKind::Failed(c) => Some(c), _ => None },
                "durationMs": dur.map(|d| d.as_secs_f64() * 1000.0),
            }));
        }
        self.results.lock().unwrap().push(JobResult {
            label: label.to_string(),
            kind,
            dur,
        });
    }

    /// Whether any NDJSON sink is configured — the stderr reporter, a
    /// `--reporter-file`, or both. Emission is gated on this rather than on the
    /// reporter alone, so `--reporter-file` works on its own: the terminal keeps
    /// the human summary while the file gets the machine-readable stream.
    fn events_enabled(&self) -> bool {
        self.sel.opts.reporter == Reporter::Ndjson || self.sel.events.is_some()
    }

    /// Write one NDJSON event to each configured sink (SPEC §6.2).
    ///
    /// `--reporter ndjson` puts events on stderr, which is fine to read but not
    /// to parse: children inherit stdio, so their output — including any JSON
    /// they log — lands in the same stream. `--reporter-file` is the sink to
    /// script against, because nothing else writes to it.
    fn emit(&self, value: serde_json::Value) {
        let line = value.to_string();
        if self.sel.opts.reporter == Reporter::Ndjson {
            eprintln!("{line}");
        }
        if let Some(file) = self.sel.events {
            use std::io::Write;
            let mut f = file.lock().unwrap();
            if let Err(e) = writeln!(f, "{line}") {
                // Losing results silently is worse than failing: a CI job would
                // read a truncated file and draw the wrong conclusion. (A
                // failure while writing the *summary* cannot change the exit
                // code, which is already computed by then — it still reports.)
                drop(f);
                self.note_runner(format!("cannot write reporter file: {e}"));
            }
        }
    }

    /// Final NDJSON event: the run's verdict and result tallies.
    fn emit_summary(&self, root: &str, code: i32, runner_error: Option<&str>, started: Instant) {
        let results = self.results.lock().unwrap();
        let count = |f: fn(&ResultKind) -> bool| results.iter().filter(|r| f(&r.kind)).count();
        self.emit(serde_json::json!({
            "type": "summary",
            "task": root,
            "status": if code == 0 { "ok" } else { "failed" },
            "exitCode": code,
            "ok": count(|k| matches!(k, ResultKind::Ok)),
            "failed": count(|k| matches!(k, ResultKind::Failed(_))),
            "skipped": count(|k| matches!(k, ResultKind::Skipped)),
            "runnerError": runner_error,
            "durationMs": started.elapsed().as_secs_f64() * 1000.0,
        }));
    }

    // --- task execution ---

    /// Run `f` under `key`, memoising so that unit of work executes at most once
    /// even when several dependents reach it at the same time (diamond-safe).
    ///
    /// Keys are task keys for whole tasks and `"<task> (<pkg>)"` labels for the
    /// per-package nodes of a topological fan-out. The two namespaces cannot
    /// collide: task names admit neither spaces nor parentheses (SPEC §4.1).
    fn memoized(&self, key: String, f: impl FnOnce() -> Status) -> Status {
        use std::sync::Arc;

        // Claim or find the memo slot.
        let (slot, owner) = {
            let mut memo = self.memo.lock().unwrap();
            match memo.get(&key) {
                Some(s) => (s.clone(), false),
                None => {
                    let s = Arc::new(TaskSlot {
                        state: Mutex::new(SlotState::Running),
                        done: Condvar::new(),
                    });
                    memo.insert(key, s.clone());
                    (s, true)
                }
            }
        };

        if !owner {
            // Another invocation owns this work; wait for its result.
            let mut st = slot.state.lock().unwrap();
            loop {
                match &*st {
                    SlotState::Done(status) => return *status,
                    SlotState::Running => st = slot.done.wait(st).unwrap(),
                }
            }
        }

        let status = f();
        let mut st = slot.state.lock().unwrap();
        *st = SlotState::Done(status);
        slot.done.notify_all();
        status
    }

    /// Run a task by key, memoising so it executes at most once.
    fn run_task(&self, key: &str, passthrough: &[String], is_root: bool) -> Status {
        self.memoized(key.to_string(), || {
            self.run_task_inner(key, passthrough, is_root)
        })
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

        // 1. Ordinary dependencies first (SPEC §5). Their batch honours *this*
        //    task's `parallel` flag. A dep failure fails the task (own command
        //    skipped). `^upstream` deps are *not* run here: they are relative to
        //    each package of the fan-out, so they belong to step 2.
        let plain = plain_deps(task);
        if !plain.is_empty() {
            let dep_status = self.run_task_batch(&plain, task.parallel);
            if !dep_status.is_ok() {
                return dep_status;
            }
        }
        if self.aborted() {
            return Status::Skipped;
        }

        // 2. The task's own command, by precedence:
        //    - `packages` → fan out (each package resolves its own form).
        //    - deps present, but no `run`/`delegate` → a pure aggregator (e.g.
        //      `ci`): its dependencies *are* its work, so it runs nothing of its
        //      own and does NOT auto-detect (which would attempt `npm run ci` /
        //      `cargo ci`). SPEC §5.2 shows such a task running only its deps.
        //    - otherwise → form 1 (`delegate`), form 2 (`run`), or form 3: a bare
        //      task with no deps auto-detects the package's native runner
        //      (SPEC §3.1) — a bare `[tasks.test]` becomes `npm run test`, etc.
        if let Some(patterns) = &task.packages {
            self.run_packages(task, patterns, passthrough)
        } else if task.run.is_none() && task.delegate.is_none() && !task.deps.is_empty() {
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
    ///
    /// With no `^upstream` deps the packages are independent and the fan-out is
    /// a flat batch. With them, order matters, and the fan-out becomes a walk of
    /// the package graph instead (SPEC §4.2, §5).
    fn run_packages(&self, task: &Task, patterns: &[String], passthrough: &[String]) -> Status {
        let matched = match workspace::match_packages(self.cfg, patterns, &task.key) {
            Ok(p) => p,
            Err(e) => {
                self.note_runner(strip_error(&e));
                return Status::Runner;
            }
        };

        // `--since` narrows the selection (SPEC §9.3) and `--resume-from` drops
        // everything ordered before the resume point (SPEC §9.4). An unmatched
        // *pattern* is still an error above — a typo — but a pattern whose
        // packages are all filtered out is the whole point of these flags, so it
        // is a clean no-op rather than a failure.
        let pkgs: Vec<workspace::Package> = matched
            .into_iter()
            .filter(|p| self.selected(&p.rel))
            .collect();
        if pkgs.is_empty() {
            if self.sel.opts.reporter == Reporter::Human {
                println!("· {} — no packages selected", task.key);
            }
            return Status::Ok;
        }

        if task.deps.iter().any(|d| upstream_dep(d).is_some()) {
            return self.run_packages_topological(task, &pkgs, passthrough);
        }

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

    /// Fan out in package-dependency order: each selected package becomes a node
    /// that runs its `^`-named tasks in every package it depends on before its
    /// own command (SPEC §4.2).
    ///
    /// Upstream packages are visited whether or not they were selected — that is
    /// the point of `^`: building `apps/web` must build the libraries it uses
    /// even when the pattern only named `apps/*`.
    fn run_packages_topological(
        &self,
        task: &Task,
        selected: &[workspace::Package],
        passthrough: &[String],
    ) -> Status {
        let graph = PackageGraph::build(self.cfg);
        // A cyclic package graph admits no order; refuse rather than pick one.
        if let Err(e) = graph.topo_order() {
            self.note_runner(strip_error(&e));
            return Status::Runner;
        }

        let mut nodes = Vec::with_capacity(selected.len());
        for pkg in selected {
            match graph.index_of(&pkg.rel) {
                Some(i) => nodes.push(i),
                None => {
                    // `match_packages` and the graph both enumerate via
                    // `workspace::packages`, so this is unreachable in practice.
                    self.note_runner(format!(
                        "task '{}': package '{}' is not in the package graph",
                        task.key, pkg.rel
                    ));
                    return Status::Runner;
                }
            }
        }
        self.run_node_batch(task, &graph, &nodes, task.parallel, passthrough)
    }

    /// One package's slice of a topological fan-out: upstream work first, then
    /// this package's own command. Memoised per `(task, package)`, so a library
    /// shared by several dependents is built once.
    fn run_pkg_node(
        &self,
        task: &Task,
        graph: &PackageGraph,
        index: usize,
        passthrough: &[String],
    ) -> Status {
        let pkg = graph.get(index);
        let label = format!("{} ({})", task.key, pkg.rel);
        self.memoized(label.clone(), || {
            if self.aborted() {
                return Status::Skipped;
            }
            // `--resume-from` means "these were built by the previous run", so a
            // skipped package is treated as already satisfied — including when
            // it is reached as another package's upstream dependency. Without
            // this the resume would rebuild the very prefix it is skipping.
            if let Some(skip) = self.sel.skip
                && skip.contains(&pkg.rel)
            {
                self.record(&label, ResultKind::Skipped, None);
                return Status::Ok;
            }

            // 1. This task's ordinary deps. Memoised by task key, so a fan-out
            //    across N packages still runs them exactly once.
            let plain = plain_deps(task);
            if !plain.is_empty() {
                let status = self.run_task_batch(&plain, task.parallel);
                if !status.is_ok() {
                    return status;
                }
            }

            // 2. Each `^name`, in every package this one directly depends on.
            //    Recursion carries the marker up the graph, so a transitive
            //    upstream is reached through its own dependents' nodes.
            let upstream = graph.deps_of(index);
            if !upstream.is_empty() {
                for dep in &task.deps {
                    let Some(name) = upstream_dep(dep) else {
                        continue;
                    };
                    let up_task = match self.cfg.task(name) {
                        Some(t) => t,
                        None => {
                            self.note_runner(format!(
                                "task '{}': upstream dep '{dep}' names no task '{name}'",
                                task.key
                            ));
                            return Status::Runner;
                        }
                    };
                    let status = self.run_node_batch(up_task, graph, upstream, task.parallel, &[]);
                    if !status.is_ok() {
                        return status;
                    }
                }
            }

            if self.aborted() {
                return Status::Skipped;
            }

            // 3. This package's own command.
            match self.build_job(task, &pkg.path, label.clone(), passthrough) {
                Ok(job) => self.run_leaf(job),
                Err(msg) => {
                    self.note_runner(msg);
                    Status::Runner
                }
            }
        })
    }

    /// Run `task` across a set of package nodes, fail-fast, honouring `parallel`.
    fn run_node_batch(
        &self,
        task: &Task,
        graph: &PackageGraph,
        nodes: &[usize],
        parallel: bool,
        passthrough: &[String],
    ) -> Status {
        if parallel {
            let statuses: Vec<Status> = std::thread::scope(|scope| {
                let handles: Vec<_> = nodes
                    .iter()
                    .map(|&i| scope.spawn(move || self.run_pkg_node(task, graph, i, passthrough)))
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });
            self.combine(&statuses)
        } else {
            self.run_sequential(nodes.len(), |k| {
                self.run_pkg_node(task, graph, nodes[k], passthrough)
            })
        }
    }

    /// Whether a package survives the `--since` / `--resume-from` filters.
    /// Applies only to the packages a fan-out *selects*; upstream `^task` work
    /// is reached through [`Self::run_pkg_node`] and is deliberately not
    /// filtered here, since it is needed regardless.
    fn selected(&self, rel: &str) -> bool {
        if let Some(affected) = self.sel.affected
            && !affected.contains(rel)
        {
            return false;
        }
        if let Some(skip) = self.sel.skip
            && skip.contains(rel)
        {
            return false;
        }
        true
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
        let mut env = env::build(self.cfg, task);
        // Resolve locally-installed binaries (`vite`, `eslint`, …) the way
        // npm/bun do, so `run = "vite"` is a real `npm run` replacement (SPEC §9.2).
        env::prepend_node_bin(&mut env, dir, &self.cfg.root);
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
            // Both `run` paths become an `ExecPlan`: the direct one is simply a
            // one-command sequence. Keeping a single shape gives builtin
            // dispatch and passthrough one code path instead of two.
            Invocation::Run(s) => {
                let mut plan = match shell::parse(&s).map_err(|e| strip_error(&e))? {
                    RunPlan::Direct(argv) => ExecPlan {
                        first: ExpandedCommand {
                            args: argv.into_iter().map(Arg::Literal).collect(),
                        },
                        rest: Vec::new(),
                    },
                    RunPlan::Shell(program) => program
                        .expand(&|k| env.get(k).cloned())
                        .map_err(|e| strip_error(&e))?,
                };
                append_to_last(&mut plan, &extra(Vec::new()));
                Action::Shell(plan)
            }
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
            if !result.is_ok() && !self.sel.opts.no_bail {
                // A prior item failed: don't launch the rest (SPEC §5.2).
                // `--no-bail` opts out — the batch runs to completion and the
                // first failing code is still what propagates.
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

    /// Run a `run` string's command sequence with `&&`/`||`/`;` semantics
    /// (SPEC §8.1), checking the abort flag between commands.
    fn run_shell(&self, plan: &ExecPlan, job: &Job) -> LeafWait {
        let mut last = match self.run_command(&plan.first, job) {
            LeafWait::Exited(c) => c,
            other => return other,
        };
        for (sep, cmd) in &plan.rest {
            if self.aborted() {
                return LeafWait::Killed;
            }
            if !sep.proceeds(last) {
                continue;
            }
            match self.run_command(cmd, job) {
                LeafWait::Exited(c) => last = c,
                other => return other,
            }
        }
        LeafWait::Exited(last)
    }

    /// Run one command of a `run` string: a builtin in-process, anything else
    /// as a spawned child (SPEC §8.5). Globs are resolved here, so a pattern
    /// sees whatever an earlier command in the sequence produced.
    fn run_command(&self, cmd: &ExpandedCommand, job: &Job) -> LeafWait {
        let argv = cmd.argv(&job.dir);
        let Some((program, args)) = argv.split_first() else {
            return LeafWait::SpawnFailed("'run' string is empty".into());
        };
        if builtins::is_builtin(program) {
            // Builtins are in-process and always fast, so there is no child to
            // poll; an abort is honoured by the caller's between-command check.
            return LeafWait::Exited(builtins::run(program, args, &job.dir));
        }
        self.spawn_wait(program, args, job)
    }

    /// Spawn one child and wait, polling the abort flag so a fail-fast can kill
    /// it mid-run (SPEC §5.2).
    fn spawn_wait(&self, program: &str, args: &[String], job: &Job) -> LeafWait {
        // On Windows a bare `npm`/`vite` is a `.cmd` shim, which `Command`'s own
        // PATH search never probes for; resolving it here against the job's PATH
        // is what makes those spawn at all (SPEC §9.2).
        let resolved = env::resolve_program(program, &job.env);
        let mut cmd = match &resolved {
            Some(path) => Command::new(path),
            None => Command::new(program),
        };
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
/// A task's ordinary `deps` — everything that is not an `^upstream` marker.
/// Those are resolved per package during the fan-out, not as task-key edges.
fn plain_deps(task: &Task) -> Vec<String> {
    task.deps
        .iter()
        .filter(|d| upstream_dep(d).is_none())
        .cloned()
        .collect()
}

fn append_to_last(plan: &mut ExecPlan, extra: &[String]) {
    if extra.is_empty() {
        return;
    }
    // Passthrough arrives as already-resolved argv, so it is never re-globbed.
    let args = extra.iter().cloned().map(Arg::Literal);
    match plan.rest.last_mut() {
        Some((_, cmd)) => cmd.args.extend(args),
        None => plan.first.args.extend(args),
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

// These tests spawn real Unix coreutils (`true`, `false`, `sh`, `touch`, `sleep`)
// as stand-in workloads, so they run on Unix only. The execution engine itself is
// cross-platform; Windows coverage comes from the pure-logic tests in other
// modules (config, shell, env, graph, …) plus a `cargo build` on the CI matrix.
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Default run options, borrowed by every `Selection` the tests build.
    const OPTS: RunOptions = RunOptions {
        since: None,
        resume_from: None,
        no_bail: false,
        reporter: Reporter::Human,
        reporter_file: None,
    };

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
        run(&cfg, task, &[], Selection::plain(&OPTS))
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
        assert_eq!(run(&cfg, "ci", &[], Selection::plain(&OPTS)), 1);
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
        assert_eq!(run(&cfg, "top", &[], Selection::plain(&OPTS)), 0);
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
        assert_eq!(run(&cfg, "top", &[], Selection::plain(&OPTS)), 0);
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
        let code = run(&cfg, "top", &[], Selection::plain(&OPTS));
        assert_eq!(code, 1);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "slow sibling not killed"
        );
    }

    #[test]
    fn no_bail_does_not_kill_a_parallel_sibling() {
        // The mirror of `parallel_fail_fast_kills_slow_sibling`: with --no-bail
        // the abort flag is never set, so the slow sibling runs to completion
        // and leaves its marker — while the failure still propagates.
        let root = scratch_root();
        let marker = root.join("slow-finished");
        let toml = format!(
            "[tasks.top]\ndeps = [\"fast\", \"slow\"]\nparallel = true\n\
             [tasks.fast]\nrun = \"false\"\n\
             [tasks.slow]\nrun = \"sleep 1 && touch {}\"\n",
            marker.display()
        );
        std::fs::write(root.join("tasks.toml"), &toml).unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        graph::validate(&cfg, "top").unwrap();

        let opts = RunOptions {
            no_bail: true,
            ..RunOptions::default()
        };
        let code = run(&cfg, "top", &[], Selection::plain(&opts));
        assert_eq!(code, 1, "the first failure still propagates");
        assert!(
            marker.exists(),
            "--no-bail must let a parallel sibling finish"
        );
    }

    #[test]
    fn no_bail_runs_every_package_of_a_failing_fan_out() {
        // A fan-out is a batch too: --no-bail must not stop at the first
        // package that fails.
        let root = scratch_root();
        for pkg in ["a", "b"] {
            let dir = root.join("packages").join(pkg);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("package.json"), format!("{{\"name\":\"{pkg}\"}}")).unwrap();
        }
        let toml = "[workspace]\nmembers = [\"packages/*\"]\n\
                    [tasks.build]\npackages = [\"packages/*\"]\nrun = \"false\"\n";
        std::fs::write(root.join("tasks.toml"), toml).unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();

        let opts = RunOptions {
            no_bail: true,
            ..RunOptions::default()
        };
        let ctx = Ctx::new(&cfg, Selection::plain(&opts));
        assert_eq!(ctx.run_task("build", &[], true), Status::Failed(1));
        // Both packages were attempted, not just the first.
        assert_eq!(ctx.results.lock().unwrap().len(), 2);
    }

    #[test]
    fn passthrough_and_args_ordering() {
        // args prepended before CLI passthrough, appended to the resolved command.
        let (cfg, _r) = setup("[tasks.t]\nrun = \"vitest\"\nargs = [\"--color\"]\n");
        let ctx = Ctx::new(&cfg, Selection::plain(&OPTS));
        let task = cfg.task("t").unwrap();
        let job = ctx
            .build_job(task, &cfg.root, "t".into(), &["--watch".to_string()])
            .unwrap();
        match job.action {
            Action::Shell(plan) => {
                assert!(plan.rest.is_empty(), "a plain `run` is a one-command plan");
                assert_eq!(
                    plan.first.argv(&cfg.root),
                    vec!["vitest", "--color", "--watch"]
                );
            }
            _ => panic!("expected a run-string plan"),
        }
    }

    #[test]
    fn passthrough_appends_to_the_last_command_of_a_sequence() {
        let (cfg, _r) = setup("[tasks.t]\nrun = \"build && vitest\"\nargs = [\"--color\"]\n");
        let ctx = Ctx::new(&cfg, Selection::plain(&OPTS));
        let job = ctx
            .build_job(
                cfg.task("t").unwrap(),
                &cfg.root,
                "t".into(),
                &["--watch".to_string()],
            )
            .unwrap();
        match job.action {
            Action::Shell(plan) => {
                assert_eq!(plan.first.argv(&cfg.root), vec!["build"]);
                assert_eq!(
                    plan.rest[0].1.argv(&cfg.root),
                    vec!["vitest", "--color", "--watch"]
                );
            }
            _ => panic!("expected a run-string plan"),
        }
    }

    #[test]
    fn builtins_run_in_process_for_run_strings() {
        let (cfg, root) = setup("[tasks.t]\nrun = \"mkdir -p out/nested && touch out/nested/x\"\n");
        assert_eq!(run(&cfg, "t", &[], Selection::plain(&OPTS)), 0);
        assert!(root.join("out/nested/x").is_file());
    }

    #[test]
    fn builtins_receive_glob_expanded_arguments() {
        let (cfg, root) = setup("[tasks.clean]\nrun = \"rm -rf dist/*\"\n");
        std::fs::create_dir_all(root.join("dist/keep")).unwrap();
        std::fs::write(root.join("dist/a.js"), "").unwrap();
        std::fs::write(root.join("dist/b.js"), "").unwrap();
        assert_eq!(run(&cfg, "clean", &[], Selection::plain(&OPTS)), 0);
        // The glob expanded to the entries, so `dist` itself survives.
        assert!(root.join("dist").is_dir());
        assert!(!root.join("dist/a.js").exists());
        assert!(!root.join("dist/keep").exists());
        // And it stays a success once there is nothing left to match.
        assert_eq!(run(&cfg, "clean", &[], Selection::plain(&OPTS)), 0);
    }

    #[test]
    fn globs_resolve_against_the_task_dir_not_the_process_cwd() {
        let (cfg, root) = setup("[tasks.clean]\ndir = \"pkg\"\nrun = \"rm -f *.log\"\n");
        std::fs::create_dir_all(root.join("pkg")).unwrap();
        std::fs::write(root.join("pkg/a.log"), "").unwrap();
        std::fs::write(root.join("outside.log"), "").unwrap();
        assert_eq!(run(&cfg, "clean", &[], Selection::plain(&OPTS)), 0);
        assert!(!root.join("pkg/a.log").exists());
        assert!(root.join("outside.log").exists(), "must not escape 'dir'");
    }

    #[test]
    fn builtin_failure_propagates_its_exit_code() {
        let (cfg, _r) = setup("[tasks.t]\nrun = \"rm ghost.txt\"\n");
        assert_eq!(run(&cfg, "t", &[], Selection::plain(&OPTS)), 1);
    }

    #[test]
    fn native_runner_gets_args_and_passthrough() {
        let root = scratch_root();
        std::fs::write(root.join("package.json"), "{}").unwrap();
        std::fs::write(root.join("tasks.toml"), "[tasks.test]\nargs = [\"--ci\"]\n").unwrap();
        let cfg = Config::load(&root.join("tasks.toml")).unwrap();
        let ctx = Ctx::new(&cfg, Selection::plain(&OPTS));
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
