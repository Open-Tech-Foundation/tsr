//! `tsr` — a lightweight, polyglot, repo-aware task runner (SPEC v1).

mod cli;
mod config;
mod detect;
mod env;
mod error;
mod exec;
mod graph;
mod resolve;
mod shell;
mod tui;
mod workspace;

use std::process::ExitCode;

use crate::cli::Cli;
use crate::config::Config;
use crate::error::TsrError;

fn main() -> ExitCode {
    match run() {
        Ok(code) => ExitCode::from(code as u8),
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(e.exit_code() as u8)
        }
    }
}

fn run() -> error::Result<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match cli::parse(&args)? {
        Cli::Help => {
            println!("{}", cli::USAGE);
            Ok(0)
        }
        Cli::Version => {
            println!("tsr {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        Cli::Init => {
            let cwd = std::env::current_dir().map_err(|e| TsrError::runtime(e.to_string()))?;
            cli::init(&cwd)?;
            Ok(0)
        }
        Cli::Config => {
            // Open an existing tasks.toml if present, otherwise author a new one
            // in the current directory (SPEC §1.5 — TUI-primary editing).
            let cwd = std::env::current_dir().map_err(|e| TsrError::runtime(e.to_string()))?;
            let path = config::locate(&cwd).unwrap_or_else(|| cwd.join(config::CONFIG_FILE));
            tui::run(&path)?;
            Ok(0)
        }
        Cli::List => {
            let cwd = cwd()?;
            match config::locate(&cwd) {
                Some(path) => cli::list(&Config::load(&path)?),
                None => cli::list_configless(&cwd),
            }
            Ok(0)
        }
        Cli::Run { task, passthrough } => {
            // A `tasks.toml` is optional: with one, load it; without one, run the
            // task repo-aware via auto-detection (configless mode) so `tsr dev`
            // maps to `npm run dev` / `cargo dev` / … (SPEC §3.1 form 3).
            let cwd = cwd()?;
            let cfg = match config::locate(&cwd) {
                Some(path) => Config::load(&path)?,
                None => implicit_config(&cwd, &task)?,
            };
            // Unknown-task and dependency-cycle checks (SPEC §5) → exit 64.
            graph::validate(&cfg, &task)?;
            // Load-time undefined-$VAR check over the tasks that will run,
            // i.e. the invoked task and its dependency closure (SPEC §7.3).
            let reachable = graph::reachable(&cfg, &task);
            env::validate_run_vars(&cfg, &reachable)?;
            // exec::run owns its own failure reporting and returns the exit code.
            Ok(exec::run(&cfg, &task, &passthrough))
        }
    }
}

fn cwd() -> error::Result<std::path::PathBuf> {
    std::env::current_dir().map_err(|e| TsrError::runtime(e.to_string()))
}

/// Synthesize a configless single-task config anchored at the nearest package
/// marker. Errors (exit `64`) when neither a `tasks.toml` nor an ecosystem marker
/// exists to run against.
fn implicit_config(cwd: &std::path::Path, task: &str) -> error::Result<Config> {
    if task.contains('#') {
        return Err(TsrError::config(format!(
            "package-qualified task '{task}' needs a tasks.toml with a [workspace]; \
             none was found in '{}' or any parent",
            cwd.display()
        )));
    }
    let root = config::nearest_package_root(cwd).ok_or_else(|| {
        TsrError::config(format!(
            "no '{}' found in '{}' or any parent, and no package.json / Cargo.toml / \
             go.mod / pyproject.toml to detect a runner from — run `tsr --init` to \
             create a config, or cd into a package",
            config::CONFIG_FILE,
            cwd.display()
        ))
    })?;
    Ok(config::implicit(root, task))
}
