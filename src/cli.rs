//! Command-line parsing and the `list` output (SPEC §6, §7).
//!
//! Grammar: `tsr <task> [-- <passthrough>…]` runs a task, forwarding everything
//! after `--` to the resolved command. `tsr --list` prints the defined tasks and
//! `tsr --init` scaffolds a starter `tasks.toml`. Builtins are flags, never bare
//! subcommands, so the first positional is always a task name.

use crate::config::{CONFIG_FILE, Config, Delegate, Task};
use crate::error::{Result, TsrError};

pub const USAGE: &str = "\
tsr — a lightweight, polyglot, repo-aware task runner

USAGE:
    tsr <task> [-- <args>...]   run a task; args after -- are forwarded
    tsr --list                  list the tasks defined in tasks.toml
    tsr --config                edit tasks.toml in an interactive TUI
    tsr --init                  create a starter tasks.toml here
    tsr --help | --version

The first argument is always a task name — every builtin is a flag, so a task
named `list` or `init` is never shadowed.

EXAMPLES:
    tsr dev
    tsr test -- --watch
    tsr ci";

/// The starter config written by `tsr --init`. Kept valid and legible for hand
/// editing (SPEC §1.5); it showcases all three task forms plus the graph.
pub const INIT_TEMPLATE: &str = "\
# tasks.toml — the workspace root anchor and config for `tsr`.
#
# Task names: [A-Za-z0-9_:-]+   ·   `#` = pkg#task   ·   quote keys containing `:`.
# Precedence when a task runs: delegate → run → auto-detect the native runner.

# [workspace]
# members = [\"apps/*\", \"packages/*\"]   # uncomment for a monorepo

[env]
# Shared environment inherited by every task (task `env` overrides these).
# NODE_ENV = \"development\"

# Form 2 — spawn a command directly (no `npm run` startup tax).
[tasks.dev]
run = \"echo 'edit tasks.toml to set your dev command'\"

# Form 3 — no `run`/`delegate`: auto-detect the ecosystem and use its runner
# (npm/bun run <task>, cargo <task>, go <task>, uv run <task>).
# [tasks.test]

# Form 1 — delegate (and hand caching) to a specialist backend.
# [tasks.build]
# delegate = \"turbo\"                       # → `turbo run build`

# Dependency graph + opt-in parallelism (sequential by default).
# [tasks.ci]
# deps = [\"lint\", \"test\", \"build\"]
# parallel = true
";

/// A parsed command line.
#[derive(Debug, PartialEq, Eq)]
pub enum Cli {
    Run {
        task: String,
        passthrough: Vec<String>,
    },
    List,
    Init,
    Config,
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

    // Builtins are flags, never bare subcommands: the first positional argument
    // is always a task name, so a task called `list` or `init` is never shadowed.
    match head.first().map(String::as_str) {
        None => Err(TsrError::runtime(format!("no task specified\n\n{USAGE}"))),
        Some("--list") => {
            if head.len() > 1 {
                return Err(TsrError::runtime("'--list' takes no arguments"));
            }
            Ok(Cli::List)
        }
        Some("--init") => {
            if head.len() > 1 {
                return Err(TsrError::runtime("'--init' takes no arguments"));
            }
            Ok(Cli::Init)
        }
        Some("--config") => {
            if head.len() > 1 {
                return Err(TsrError::runtime("'--config' takes no arguments"));
            }
            Ok(Cli::Config)
        }
        Some("-h" | "--help") => Ok(Cli::Help),
        Some("-V" | "--version") => Ok(Cli::Version),
        Some(flag) if flag.starts_with('-') => Err(TsrError::runtime(format!(
            "unknown flag '{flag}'\n\n{USAGE}"
        ))),
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

/// Scaffold a starter `tasks.toml` in `dir`. Refuses to overwrite an existing
/// one (a runner-level error, exit `64`), so `--init` is always safe to run.
pub fn init(dir: &std::path::Path) -> Result<()> {
    let path = dir.join(CONFIG_FILE);
    if path.exists() {
        return Err(TsrError::runtime(format!(
            "'{}' already exists — not overwriting",
            path.display()
        )));
    }
    std::fs::write(&path, INIT_TEMPLATE)
        .map_err(|e| TsrError::runtime(format!("cannot write '{}': {e}", path.display())))?;
    println!("Created {}", path.display());
    println!("Next: edit it, then run `tsr <task>` or `tsr --list`.");
    Ok(())
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

/// A short human descriptor of a task's form, for `tsr --list`.
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
        assert_eq!(parse_ok(&["--list"]), Cli::List);
        assert_eq!(parse_ok(&["--help"]), Cli::Help);
        assert_eq!(parse_ok(&["-V"]), Cli::Version);
    }

    #[test]
    fn parses_init() {
        assert_eq!(parse_ok(&["--init"]), Cli::Init);
        assert!(
            parse_err(&["--init", "x"])
                .to_string()
                .contains("no arguments")
        );
    }

    #[test]
    fn parses_config() {
        assert_eq!(parse_ok(&["--config"]), Cli::Config);
        assert!(
            parse_err(&["--config", "x"])
                .to_string()
                .contains("no arguments")
        );
    }

    #[test]
    fn builtin_names_are_not_reserved_as_tasks() {
        // The first positional is always a task name — builtins are flags only,
        // so `tsr list` / `tsr init` run tasks called `list` / `init`.
        assert_eq!(
            parse_ok(&["list"]),
            Cli::Run {
                task: "list".into(),
                passthrough: vec![],
            }
        );
        assert_eq!(
            parse_ok(&["init", "--", "--flag"]),
            Cli::Run {
                task: "init".into(),
                passthrough: vec!["--flag".into()],
            }
        );
    }

    #[test]
    fn init_writes_template_and_refuses_overwrite() {
        let dir = std::env::temp_dir().join(format!("tsr-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init(&dir).unwrap();
        assert!(dir.join(CONFIG_FILE).exists());
        // Second run must not clobber the existing file.
        let err = init(&dir).unwrap_err();
        assert_eq!(err.exit_code(), 64);
        assert!(err.to_string().contains("already exists"));
    }

    #[test]
    fn init_template_is_a_valid_runnable_config() {
        // The scaffold must load cleanly and expose the uncommented `dev` task,
        // so `tsr --init` immediately followed by `tsr dev` works.
        let dir = std::env::temp_dir().join(format!("tsr-inittmpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), INIT_TEMPLATE).unwrap();
        let cfg = Config::load(&dir.join(CONFIG_FILE)).unwrap();
        assert!(cfg.task("dev").and_then(|t| t.run.as_deref()).is_some());
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
        assert!(
            parse_err(&["--list", "x"])
                .to_string()
                .contains("no arguments")
        );
    }
}
