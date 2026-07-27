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
    tsr <task> --since <ref>    run only in packages affected since a git ref
    tsr --list                  list the tasks defined in tasks.toml
    tsr --config                edit tasks.toml in an interactive TUI
    tsr --init                  create a starter tasks.toml here
    tsr --help | --version

The first argument is always a task name — every builtin is a flag, so a task
named `list` or `init` is never shadowed.

tasks.toml is optional: with no config, `tsr <task>` runs the package's native
script (e.g. `tsr dev` → `npm run dev` / `cargo dev`) by auto-detecting the
ecosystem in the current directory or a parent.

EXAMPLES:
    tsr dev
    tsr test -- --watch
    tsr ci
    tsr build --since main";

/// The starter config written by `tsr --init`: reference comments only, no live
/// tasks. Defining nothing keeps the scaffold from shadowing what the repo
/// already does — a present `tasks.toml` takes full precedence over auto-detection
/// (SPEC §2.1), so a placeholder task would hide the real `npm run dev`.
pub const INIT_TEMPLATE: &str = "\
# tasks.toml — the workspace root anchor and config for `tsr`.
# Docs: https://tsr.opentechf.org/docs

# [workspace]
# members = [\"apps/*\", \"packages/*\"]

# [env]
# NODE_ENV = \"development\"

# [tasks.dev]
# run = \"vite\"

# [tasks.build]
# delegate = \"turbo\"

# [tasks.test]

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
        /// `--since <ref>`: restrict `packages` fan-outs to the packages
        /// affected by changes since this git ref (SPEC §9.3).
        since: Option<String>,
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
            let since = parse_since(task, &head[1..])?;
            Ok(Cli::Run {
                task: task.to_string(),
                passthrough: tail.to_vec(),
                since,
            })
        }
    }
}

/// Parse the options that may follow a task name. Only `--since <ref>` (or
/// `--since=<ref>`) is accepted; anything else is still the "did you mean `--`?"
/// error, since the bare-word namespace belongs to task names (SPEC §6.1).
fn parse_since(task: &str, rest: &[String]) -> Result<Option<String>> {
    let mut since: Option<String> = None;
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        let value = if let Some(v) = arg.strip_prefix("--since=") {
            i += 1;
            v.to_string()
        } else if arg == "--since" {
            let v = rest.get(i + 1).ok_or_else(|| {
                TsrError::runtime("'--since' needs a git ref (e.g. `--since main`)")
            })?;
            i += 2;
            v.clone()
        } else {
            return Err(TsrError::runtime(format!(
                "unexpected argument '{arg}' — forward args after '--' (e.g. `tsr {task} -- {arg}`)"
            )));
        };
        if value.is_empty() {
            return Err(TsrError::runtime(
                "'--since' needs a git ref (e.g. `--since main`)",
            ));
        }
        since = Some(value);
    }
    Ok(since)
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

/// `tsr --list` when there is no `tasks.toml`: nothing is declared, but tasks
/// still run repo-aware via auto-detection — point the user at how that works.
pub fn list_configless(cwd: &std::path::Path) {
    match crate::config::nearest_package_root(cwd) {
        Some(root) => {
            let runner = crate::detect::detect(&root)
                .map(ecosystem_label)
                .unwrap_or("a native runner");
            println!("No tasks.toml — tsr runs your package scripts directly.");
            println!("Detected {runner} at {}.", root.display());
            println!("Run one with:  tsr <script>   (e.g. tsr dev, tsr build)");
        }
        None => {
            println!(
                "No tasks.toml, and no package.json / Cargo.toml / go.mod / pyproject.toml here."
            );
            println!("Run `tsr --init` to create a config, or cd into a package.");
        }
    }
}

/// Human label for a detected ecosystem, for the configless `--list` hint.
fn ecosystem_label(eco: crate::detect::Ecosystem) -> &'static str {
    use crate::detect::Ecosystem::*;
    match eco {
        Npm => "an npm package (package.json)",
        Bun => "a bun package (package.json + bun lockfile)",
        Cargo => "a Cargo crate (Cargo.toml)",
        Go => "a Go module (go.mod)",
        Python => "a Python project (pyproject.toml)",
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
                passthrough: vec![],
                since: None,
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
                since: None,
            }
        );
    }

    #[test]
    fn empty_passthrough_is_allowed() {
        assert_eq!(
            parse_ok(&["test", "--"]),
            Cli::Run {
                task: "test".into(),
                passthrough: vec![],
                since: None,
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
                since: None,
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
                since: None,
            }
        );
        assert_eq!(
            parse_ok(&["init", "--", "--flag"]),
            Cli::Run {
                task: "init".into(),
                passthrough: vec!["--flag".into()],
                since: None,
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
    fn init_template_is_valid_and_defines_no_tasks() {
        // The scaffold must load cleanly, and must define nothing: a live task
        // would shadow what the repo already runs via auto-detection.
        let dir = std::env::temp_dir().join(format!("tsr-inittmpl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(CONFIG_FILE), INIT_TEMPLATE).unwrap();
        let cfg = Config::load(&dir.join(CONFIG_FILE)).unwrap();
        assert!(cfg.tasks.is_empty(), "scaffold must not define tasks");
    }

    #[test]
    fn init_template_points_at_the_docs() {
        // The scaffold is the main discovery surface for the docs site.
        assert!(INIT_TEMPLATE.contains("https://tsr.opentechf.org/docs"));
    }

    /// `key ` before an `=` — i.e. this line is a TOML assignment, not prose.
    fn is_bare_key(s: &str) -> bool {
        let k = s.trim();
        !k.is_empty()
            && k.len() < s.len() // there was an `=` after it
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    }

    #[test]
    fn init_template_examples_uncomment_into_a_valid_config() {
        // Every commented example must be real, working TOML — uncommenting the
        // task blocks (not the prose header) has to produce a config that loads.
        let body = INIT_TEMPLATE
            .lines()
            .filter_map(|l| l.strip_prefix("# "))
            // Keep only the commented TOML — a table header or a `key = value`
            // — and drop the surrounding prose.
            .filter(|l| l.starts_with('[') || l.split('=').next().is_some_and(is_bare_key))
            .collect::<Vec<_>>()
            .join("\n");
        crate::config::validate_str(&body)
            .unwrap_or_else(|e| panic!("uncommented scaffold is invalid: {e}\n---\n{body}"));
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

    #[test]
    fn parses_since_in_both_spellings() {
        for args in [
            ["build", "--since", "main"].as_slice(),
            ["build", "--since=main"].as_slice(),
        ] {
            assert_eq!(
                parse_ok(args),
                Cli::Run {
                    task: "build".into(),
                    passthrough: vec![],
                    since: Some("main".into()),
                }
            );
        }
    }

    #[test]
    fn since_combines_with_passthrough() {
        assert_eq!(
            parse_ok(&["test", "--since", "HEAD~1", "--", "--watch"]),
            Cli::Run {
                task: "test".into(),
                passthrough: vec!["--watch".into()],
                since: Some("HEAD~1".into()),
            }
        );
    }

    #[test]
    fn since_without_a_ref_is_an_error() {
        assert!(
            parse_err(&["build", "--since"])
                .to_string()
                .contains("git ref")
        );
        assert!(
            parse_err(&["build", "--since="])
                .to_string()
                .contains("git ref")
        );
    }

    #[test]
    fn a_ref_that_looks_like_a_flag_is_still_taken_as_the_value() {
        // `--since` consumes the next token whatever it is; git will reject a
        // bogus ref far more informatively than we could.
        assert_eq!(
            parse_ok(&["build", "--since", "--weird"]),
            Cli::Run {
                task: "build".into(),
                passthrough: vec![],
                since: Some("--weird".into()),
            }
        );
    }

    #[test]
    fn other_arguments_after_a_task_still_error() {
        // The bare-word namespace belongs to task names (SPEC §6.1), so this
        // must keep pointing at `--` rather than silently accepting options.
        let err = parse_err(&["build", "extra"]).to_string();
        assert!(err.contains("unexpected argument"), "{err}");
        assert!(err.contains("--"), "{err}");
    }
}
