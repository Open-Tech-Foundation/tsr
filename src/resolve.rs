//! Task-form resolution (SPEC §3.1). Given a task and the directory it will run
//! in, decide *what* to execute, by precedence:
//!
//! 1. `delegate` present → `<bin> run <task>` (string) or `bin`+`args` (table).
//! 2. `run` present      → the raw command string (handed to the shell layer).
//! 3. neither            → auto-detect the ecosystem and map to its native runner.

use std::path::Path;

use crate::config::{Delegate, Task};
use crate::detect;
use crate::error::{Result, TsrError};

/// What a resolved task will execute, before env merge and CLI passthrough.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Invocation {
    /// A concrete `program` + `args` invocation: `delegate` (either form) or an
    /// auto-detected native-runner command. Never touches the mini-shell.
    Direct { program: String, args: Vec<String> },
    /// A raw `run` string (form 2). The shell layer (SPEC §8) decides between a
    /// direct spawn and the mini-shell.
    Run(String),
}

/// Resolve `task`'s form into an [`Invocation`], detecting the ecosystem in
/// `dir` when the task has neither `delegate` nor `run` (form 3). `root` is the
/// workspace root, which bounds the search for a Node lockfile.
pub fn resolve(task: &Task, dir: &Path, root: &Path) -> Result<Invocation> {
    // Precedence 1: delegate.
    if let Some(delegate) = &task.delegate {
        return Ok(match delegate {
            Delegate::Bin(bin) => Invocation::Direct {
                program: bin.clone(),
                args: vec!["run".into(), task.task_name().into()],
            },
            Delegate::Full { bin, args } => Invocation::Direct {
                program: bin.clone(),
                args: args.clone(),
            },
        });
    }

    // Precedence 2: run.
    if let Some(run) = &task.run {
        return Ok(Invocation::Run(run.clone()));
    }

    // Precedence 3: auto-detect ecosystem → native runner.
    let eco = detect::detect_within(dir, root).ok_or_else(|| {
        TsrError::runtime(format!(
            "task '{}': no recognised ecosystem in '{}' \
             (expected one of package.json, deno.json, Cargo.toml, go.mod, pyproject.toml, Makefile) \
             and the task defines neither 'run' nor 'delegate'",
            task.key,
            dir.display()
        ))
    })?;
    if eco == detect::Ecosystem::Python && !detect::python_target(dir, task.task_name()) {
        return Err(TsrError::runtime(format!(
            "task '{}': a Python project has no '{name}' script for `uv run {name}` to find \
             — give the task a command, e.g. `run = \"uv run pytest\"` in tasks.toml \
             (only a `.py` file or a name declared in [project.scripts] runs bare)",
            task.key,
            name = task.task_name(),
        )));
    }
    let (program, args) = eco.native_command(task.task_name());
    Ok(Invocation::Direct { program, args })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Delegate;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-resolve-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn task(key: &str) -> Task {
        Task {
            key: key.into(),
            ..Task::default()
        }
    }

    #[test]
    fn delegate_string_becomes_bin_run_task() {
        let mut t = task("build");
        t.delegate = Some(Delegate::Bin("turbo".into()));
        assert_eq!(
            resolve(&t, &scratch(), &std::env::temp_dir()).unwrap(),
            Invocation::Direct {
                program: "turbo".into(),
                args: vec!["run".into(), "build".into()],
            }
        );
    }

    #[test]
    fn delegate_string_uses_task_name_not_full_key() {
        let mut t = task("web#build");
        t.delegate = Some(Delegate::Bin("turbo".into()));
        let Invocation::Direct { args, .. } =
            resolve(&t, &scratch(), &std::env::temp_dir()).unwrap()
        else {
            panic!();
        };
        assert_eq!(args, vec!["run", "build"]);
    }

    #[test]
    fn delegate_table_is_verbatim() {
        let mut t = task("bundle");
        t.delegate = Some(Delegate::Full {
            bin: "make".into(),
            args: vec!["bundle".into()],
        });
        assert_eq!(
            resolve(&t, &scratch(), &std::env::temp_dir()).unwrap(),
            Invocation::Direct {
                program: "make".into(),
                args: vec!["bundle".into()],
            }
        );
    }

    #[test]
    fn run_takes_precedence_over_autodetect() {
        let d = scratch();
        fs::write(d.join("Cargo.toml"), "").unwrap();
        let mut t = task("dev");
        t.run = Some("vite".into());
        assert_eq!(resolve(&t, &d, &d).unwrap(), Invocation::Run("vite".into()));
    }

    #[test]
    fn delegate_takes_precedence_over_run() {
        let mut t = task("build");
        t.run = Some("should-not-run".into());
        t.delegate = Some(Delegate::Bin("turbo".into()));
        let Invocation::Direct { program, .. } =
            resolve(&t, &scratch(), &std::env::temp_dir()).unwrap()
        else {
            panic!();
        };
        assert_eq!(program, "turbo");
    }

    #[test]
    fn autodetect_maps_to_native_runner() {
        let d = scratch();
        fs::write(d.join("Cargo.toml"), "").unwrap();
        assert_eq!(
            resolve(&task("test"), &d, &d).unwrap(),
            Invocation::Direct {
                program: "cargo".into(),
                args: vec!["test".into()],
            }
        );
    }

    #[test]
    fn autodetect_uses_the_workspace_root_lockfile() {
        // A pnpm monorepo keeps one lockfile at the root; the member runs pnpm.
        let root = scratch();
        fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        let web = root.join("apps/web");
        fs::create_dir_all(&web).unwrap();
        fs::write(web.join("package.json"), "{}").unwrap();
        assert_eq!(
            resolve(&task("dev"), &web, &root).unwrap(),
            Invocation::Direct {
                program: "pnpm".into(),
                args: vec!["run".into(), "dev".into()],
            }
        );
    }

    #[test]
    fn python_bare_name_without_a_script_is_an_error() {
        let d = scratch();
        fs::write(d.join("pyproject.toml"), "[project]\nname = \"p\"\n").unwrap();
        let err = resolve(&task("test"), &d, &d).unwrap_err();
        assert_eq!(err.exit_code(), 64);
        let msg = err.to_string();
        assert!(msg.contains("run = \"uv run pytest\""), "{msg}");
    }

    #[test]
    fn python_declared_script_maps_to_uv_run() {
        let d = scratch();
        fs::write(
            d.join("pyproject.toml"),
            "[project]\nname = \"p\"\n[project.scripts]\nserve = \"p:main\"\n",
        )
        .unwrap();
        assert_eq!(
            resolve(&task("serve"), &d, &d).unwrap(),
            Invocation::Direct {
                program: "uv".into(),
                args: vec!["run".into(), "serve".into()],
            }
        );
    }

    #[test]
    fn autodetect_failure_is_runtime_error() {
        let err = resolve(&task("test"), &scratch(), &std::env::temp_dir()).unwrap_err();
        assert!(matches!(err, TsrError::Runtime(_)));
        assert_eq!(err.exit_code(), 64);
    }
}
