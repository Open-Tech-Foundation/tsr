//! Command-line parsing and the `list` output (SPEC §6, §7).
//!
//! Grammar: `tsr <task> [-- <passthrough>…]` runs a task, forwarding everything
//! after `--` to the resolved command. `tsr list` prints the defined tasks.

use crate::config::{Config, Delegate, Task};
use crate::error::{Result, TsrError};

pub const USAGE: &str = "\
tsr — a lightweight, polyglot, repo-aware task runner

USAGE:
    tsr <task> [-- <args>...]   run a task; args after -- are forwarded
    tsr list                    list the tasks defined in tasks.toml
    tsr --help | --version

EXAMPLES:
    tsr dev
    tsr test -- --watch
    tsr ci";

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    Run {
        task: String,
        passthrough: Vec<String>,
    },
    List,
    Help,
    Version,
}

/// Parse process arguments (excluding argv[0]) into a [`Cli`]. Misuse is a
/// runner-level error (exit `64`).
pub fn parse(args: &[String]) -> Result<Cli> {
    // Everything after the first `--` is passthrough (SPEC §6).
    let (head, tail): (&[String], &[String]) = match args.iter().position(|a| a == "--") {
        Some(i) => (&args[..i], &args[i + 1..]),
        None => (args, &[]),
    };

    match head.first().map(String::as_str) {
        None => Err(TsrError::runtime(format!("no task specified\n\n{USAGE}"))),
        Some("list") => {
            if head.len() > 1 {
                return Err(TsrError::runtime("'list' takes no arguments"));
            }
            Ok(Cli::List)
        }
        Some("-h" | "--help") => Ok(Cli::Help),
        Some("-V" | "--version") => Ok(Cli::Version),
        Some(flag) if flag.starts_with('-') => {
            Err(TsrError::runtime(format!("unknown flag '{flag}'\n\n{USAGE}")))
        }
        Some(task) => {
            if head.len() > 1 {
                return Err(TsrError::runtime(format!(
                    "unexpected argument '{}' — forward args after '--' (e.g. `tsr {task} -- {}`)",
                    head[1], head[1],
                )));
            }
            Ok(Cli::Run {
                task: task.to_string(),
                passthrough: tail.to_vec(),
            })
        }
    }
}

/// Print the tasks defined in the config, with a one-line form descriptor.
pub fn list(cfg: &Config) {
    if cfg.tasks.is_empty() {
        println!("No tasks defined in tasks.toml.");
        return;
    }
    let width = cfg.tasks.keys().map(String::len).max().unwrap_or(0);
    println!("Available tasks:");
    for (key, task) in &cfg.tasks {
        println!("  {key:width$}  {}", describe(task));
    }
}

/// A short human descriptor of a task's form, for `tsr list`.
fn describe(task: &Task) -> String {
    let mut parts: Vec<String> = Vec::new();
    match &task.delegate {
        Some(Delegate::Bin(bin)) => parts.push(format!("delegate: {bin}")),
        Some(Delegate::Full { bin, .. }) => parts.push(format!("delegate: {bin} (custom)")),
        None => {}
    }
    if let Some(run) = &task.run {
        parts.push(format!("run: {run}"));
    }
    if let Some(pkgs) = &task.packages {
        parts.push(format!("packages: {}", pkgs.join(", ")));
    }
    if let Some(dir) = &task.dir {
        parts.push(format!("dir: {dir}"));
    }
    if !task.deps.is_empty() {
        parts.push(format!("deps: {}", task.deps.join(", ")));
    }
    if task.parallel {
        parts.push("parallel".to_string());
    }
    if parts.is_empty() {
        // No form fields → auto-detected native runner (SPEC §3.1 form 3).
        parts.push("auto".to_string());
    }
    parts.join("  ·  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_ok(args: &[&str]) -> Cli {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap()
    }

    fn parse_err(args: &[&str]) -> TsrError {
        parse(&args.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap_err()
    }

    #[test]
    fn parses_bare_task() {
        assert_eq!(
            parse_ok(&["dev"]),
            Cli::Run {
                task: "dev".into(),
                passthrough: vec![]
            }
        );
    }

    #[test]
    fn parses_passthrough_after_double_dash() {
        assert_eq!(
            parse_ok(&["test", "--", "--watch", "-x"]),
            Cli::Run {
                task: "test".into(),
                passthrough: vec!["--watch".into(), "-x".into()],
            }
        );
    }

    #[test]
    fn empty_passthrough_is_allowed() {
        assert_eq!(
            parse_ok(&["test", "--"]),
            Cli::Run {
                task: "test".into(),
                passthrough: vec![]
            }
        );
    }

    #[test]
    fn passthrough_keeps_list_and_flags_literal() {
        // A `--help` after `--` belongs to the task, not tsr.
        assert_eq!(
            parse_ok(&["run", "--", "list", "--help"]),
            Cli::Run {
                task: "run".into(),
                passthrough: vec!["list".into(), "--help".into()],
            }
        );
    }

    #[test]
    fn parses_list_help_version() {
        assert_eq!(parse_ok(&["list"]), Cli::List);
        assert_eq!(parse_ok(&["--help"]), Cli::Help);
        assert_eq!(parse_ok(&["-V"]), Cli::Version);
    }

    #[test]
    fn no_task_is_error() {
        assert_eq!(parse_err(&[]).exit_code(), 64);
        assert_eq!(parse_err(&["--"]).exit_code(), 64);
    }

    #[test]
    fn extra_token_before_dashes_is_error() {
        let err = parse_err(&["test", "extra"]);
        assert!(err.to_string().contains("unexpected argument"));
    }

    #[test]
    fn unknown_flag_is_error() {
        assert!(parse_err(&["--nope"]).to_string().contains("unknown flag"));
    }

    #[test]
    fn list_rejects_arguments() {
        assert!(parse_err(&["list", "x"]).to_string().contains("no arguments"));
    }
}
