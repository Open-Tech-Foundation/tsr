//! `tsr` — a lightweight, polyglot, repo-aware task runner (SPEC v1).

mod config;
mod detect;
mod env;
mod error;
mod resolve;
mod shell;

use std::process::ExitCode;

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
    // Stage 1 scaffolding: discover + validate the config so failures surface
    // with exit code 64. Task execution is wired up in later stages.
    let cwd = std::env::current_dir().map_err(|e| TsrError::runtime(e.to_string()))?;
    let cfg = Config::discover(&cwd)?;
    let _ = cfg;
    Ok(0)
}
