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
        Cli::List => {
            let cfg = discover()?;
            cli::list(&cfg);
            Ok(0)
        }
        Cli::Run { task, passthrough } => {
            let cfg = discover()?;
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

fn discover() -> error::Result<Config> {
    let cwd = std::env::current_dir().map_err(|e| TsrError::runtime(e.to_string()))?;
    Config::discover(&cwd)
}
