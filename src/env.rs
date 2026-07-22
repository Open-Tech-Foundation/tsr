//! Environment-variable model (SPEC §7).
//!
//! Sources are merged — never replaced — with this precedence (highest wins):
//!
//! ```text
//! task env  >  task env_file(s)  >  workspace [env]  >  root .env file  >  process env
//! ```
//!
//! `env_file` loads one or more `.env`-style files declared on the task, in
//! listed order (later files override earlier ones), resolved relative to the
//! task's directory. Each `[env]`/task/file value is expanded as it is applied,
//! so it may reference the process env and *earlier* keys, but never forward keys
//! (SPEC §7.3). `$VAR` inside a `run` string is expanded later, against this
//! fully-merged map.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{Config, Task};
use crate::error::{Result, TsrError};
use crate::shell;

/// The `.env` file loaded from the workspace root (SPEC §7.2).
pub const DOTENV_FILE: &str = ".env";

/// Prepend `node_modules/.bin` directories to `PATH` so locally-installed
/// binaries (`vite`, `eslint`, `tsc`, …) resolve when a `run` string names them
/// directly — the same lookup npm/bun/yarn/pnpm perform, and what makes tsr a
/// real `npm run` replacement (SPEC §9.2).
///
/// Directories are collected walking up from the task's working directory to the
/// workspace `root` (inclusive), nearest first, so a package's own `.bin` wins
/// over a hoisted root one. Only directories that exist are added.
pub fn prepend_node_bin(env: &mut HashMap<String, String>, dir: &Path, root: &Path) {
    let mut bins: Vec<PathBuf> = Vec::new();
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let bin = d.join("node_modules").join(".bin");
        if bin.is_dir() {
            bins.push(bin);
        }
        if d == root {
            break; // don't climb above the workspace root
        }
        cur = d.parent();
    }
    if bins.is_empty() {
        return;
    }
    // Prepend the discovered bin dirs (nearest first) ahead of the existing PATH.
    let existing = env.get("PATH").cloned().unwrap_or_default();
    let mut parts = bins;
    parts.extend(std::env::split_paths(&existing));
    if let Ok(joined) = std::env::join_paths(parts) {
        env.insert("PATH".to_string(), joined.to_string_lossy().into_owned());
    }
}

/// Build the merged, fully-expanded environment for `task` (SPEC §7.1), reading
/// the real process env, the root `.env`, and the task's `env_file`(s).
pub fn build(cfg: &Config, task: &Task) -> HashMap<String, String> {
    let process: HashMap<String, String> = std::env::vars().collect();
    let dotenv = load_dotenv(&cfg.root);
    let file_env = load_env_files(&task_base_dir(&cfg.root, task), &task.env_files);
    build_from(process, &dotenv, &cfg.env, &file_env, &task.env)
}

/// The directory a task's `env_file` paths resolve against: its `dir` (relative
/// to the workspace root) or the workspace root itself. Kept consistent between
/// execution and the load-time `$VAR` check.
fn task_base_dir(root: &Path, task: &Task) -> PathBuf {
    match &task.dir {
        Some(d) => root.join(d),
        None => root.to_path_buf(),
    }
}

/// Load each of a task's `env_file`s (left → right), relative to `base`. Later
/// files override earlier ones. A missing or unreadable file is skipped (so an
/// optional `.env.local` need not exist), matching the root `.env`.
fn load_env_files(base: &Path, files: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for f in files {
        if let Ok(text) = std::fs::read_to_string(base.join(f)) {
            out.extend(parse_dotenv(&text));
        }
    }
    out
}

/// Core merge, with the process env and `.env` injected explicitly so tests need
/// not mutate global state. Overlays are applied lowest-precedence first:
/// `.env` → workspace `[env]` → task `env_file`(s) → task `env`.
fn build_from(
    process: HashMap<String, String>,
    dotenv: &[(String, String)],
    workspace_env: &[(String, String)],
    file_env: &[(String, String)],
    task_env: &[(String, String)],
) -> HashMap<String, String> {
    let mut map = process;
    // Each value is expanded against everything applied so far (process + earlier
    // keys), lowest precedence first so higher sources overwrite.
    for (k, v) in dotenv
        .iter()
        .chain(workspace_env)
        .chain(file_env)
        .chain(task_env)
    {
        let val = expand_value(v, &map);
        map.insert(k.clone(), val);
    }
    map
}

/// Load and parse the workspace-root `.env` if present (SPEC §7.2). Only the
/// root file is read; per-package `.env` files are ignored by design. A missing
/// or unreadable file yields an empty set.
pub fn load_dotenv(root: &Path) -> Vec<(String, String)> {
    let path = root.join(DOTENV_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    parse_dotenv(&text)
}

/// Parse `.env` content: `KEY=VALUE` lines, `#` comments, blank lines, an
/// optional `export ` prefix, and optional surrounding single/double quotes.
fn parse_dotenv(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").unwrap_or(line).trim_start();
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        out.push((key.to_string(), unquote(value.trim()).to_string()));
    }
    out
}

fn unquote(s: &str) -> &str {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 {
        let first = bytes[0];
        let last = bytes[bytes.len() - 1];
        if (first == b'"' && last == b'"') || (first == b'\'' && last == b'\'') {
            return &s[1..s.len() - 1];
        }
    }
    s
}

/// Expand `$VAR` / `${VAR}` in an env *value* against `map`. Following shell
/// convention for env blocks, an undefined reference expands to empty (the
/// strict undefined-variable error applies to `run` strings, SPEC §7.3).
fn expand_value(input: &str, map: &HashMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '$'
            && let Some((name, next)) = read_var(&chars, i + 1)
        {
            out.push_str(map.get(&name).map(String::as_str).unwrap_or(""));
            i = next;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Read a `${NAME}` or `$NAME` reference starting just after the `$`. Returns the
/// name and the index following it, or `None` for a literal `$`.
fn read_var(chars: &[char], start: usize) -> Option<(String, usize)> {
    match chars.get(start) {
        Some('{') => {
            let mut j = start + 1;
            let mut name = String::new();
            while let Some(&c) = chars.get(j) {
                if c == '}' {
                    return if name.is_empty() {
                        None
                    } else {
                        Some((name, j + 1))
                    };
                }
                name.push(c);
                j += 1;
            }
            None // unterminated ${...}: treat '$' literally
        }
        Some(&c) if c == '_' || c.is_ascii_alphabetic() => {
            let mut j = start;
            let mut name = String::new();
            while let Some(&c) = chars.get(j) {
                if c == '_' || c.is_ascii_alphanumeric() {
                    name.push(c);
                    j += 1;
                } else {
                    break;
                }
            }
            Some((name, j))
        }
        _ => None,
    }
}

/// Validate, at load time, that every `$VAR` referenced by a `run` string in the
/// given tasks is defined in that task's merged env (SPEC §7.3). Undefined →
/// exit `64`. Only the tasks that will actually run are checked, so an unrelated
/// broken task does not block the invoked one.
pub fn validate_run_vars(cfg: &Config, keys: &[String]) -> Result<()> {
    let process: HashMap<String, String> = std::env::vars().collect();
    let dotenv = load_dotenv(&cfg.root);
    validate_run_vars_from(cfg, keys, &process, &dotenv)
}

fn validate_run_vars_from(
    cfg: &Config,
    keys: &[String],
    process: &HashMap<String, String>,
    dotenv: &[(String, String)],
) -> Result<()> {
    for key in keys {
        let Some(task) = cfg.task(key) else { continue };
        let Some(run) = &task.run else { continue };
        let plan = shell::parse(run)?;
        let vars = plan.referenced_vars();
        if vars.is_empty() {
            continue;
        }
        let file_env = load_env_files(&task_base_dir(&cfg.root, task), &task.env_files);
        let map = build_from(process.clone(), dotenv, &cfg.env, &file_env, &task.env);
        for var in vars {
            if !map.contains_key(&var) {
                return Err(TsrError::config(format!(
                    "task '{}': '${}' is not defined in task env, env_file, workspace [env], or .env\n  run = \"{}\"",
                    task.key, var, run
                )));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn proc(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn owned(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn owned_paths(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn prepends_node_bin_dirs_nearest_first() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsr-nbin-{}-{id}", std::process::id()));
        let pkg = root.join("apps/web");
        std::fs::create_dir_all(pkg.join("node_modules/.bin")).unwrap();
        std::fs::create_dir_all(root.join("node_modules/.bin")).unwrap();

        let mut env = proc(&[("PATH", "/usr/bin")]);
        prepend_node_bin(&mut env, &pkg, &root);

        let sep = if cfg!(windows) { ';' } else { ':' };
        let parts: Vec<&str> = env["PATH"].split(sep).collect();
        // Package .bin first, then hoisted root .bin, then the original PATH.
        // Build expected paths the same way as prepend_node_bin so separators
        // match on Windows (join("node_modules/.bin") would keep a forward slash).
        assert_eq!(
            parts[0],
            pkg.join("node_modules").join(".bin").to_str().unwrap()
        );
        assert_eq!(
            parts[1],
            root.join("node_modules").join(".bin").to_str().unwrap()
        );
        assert_eq!(*parts.last().unwrap(), "/usr/bin");
    }

    #[test]
    fn node_bin_noop_when_absent() {
        let dir = std::env::temp_dir();
        let mut env = proc(&[("PATH", "/usr/bin")]);
        prepend_node_bin(&mut env, &dir, &dir);
        assert_eq!(env["PATH"], "/usr/bin");
    }

    #[test]
    fn precedence_task_beats_file_beats_workspace_beats_dotenv_beats_process() {
        let map = build_from(
            proc(&[("K", "process"), ("P", "keepme")]),
            &owned(&[("K", "dotenv")]),
            &owned(&[("K", "workspace")]),
            &owned(&[("K", "file")]),
            &owned(&[("K", "task")]),
        );
        assert_eq!(map["K"], "task");
        // Lower sources are merged, never wiped.
        assert_eq!(map["P"], "keepme");
    }

    #[test]
    fn env_file_overrides_dotenv_and_workspace_but_not_task_env() {
        let map = build_from(
            proc(&[]),
            &owned(&[("K", "dotenv"), ("A", "base")]),
            &owned(&[("K", "workspace")]),
            &owned(&[("K", "file"), ("A", "fromfile")]),
            &[],
        );
        // env_file beats .env and [env]…
        assert_eq!(map["K"], "file");
        assert_eq!(map["A"], "fromfile");

        // …but an inline task env still wins over env_file.
        let map2 = build_from(
            proc(&[]),
            &[],
            &[],
            &owned(&[("K", "file")]),
            &owned(&[("K", "task")]),
        );
        assert_eq!(map2["K"], "task");
    }

    #[test]
    fn merge_never_wipes_lower_sources() {
        let map = build_from(
            proc(&[("PATH", "/bin")]),
            &[],
            &owned(&[("X", "1")]),
            &[],
            &[],
        );
        assert_eq!(map["PATH"], "/bin");
        assert_eq!(map["X"], "1");
    }

    #[test]
    fn value_references_process_and_earlier_keys() {
        let map = build_from(
            proc(&[("HOME", "/h")]),
            &[],
            &owned(&[("A", "$HOME/a"), ("B", "${A}/b")]),
            &[],
            &[],
        );
        assert_eq!(map["A"], "/h/a");
        assert_eq!(map["B"], "/h/a/b");
    }

    #[test]
    fn undefined_reference_in_value_is_empty() {
        let map = build_from(
            HashMap::new(),
            &[],
            &owned(&[("A", "x${MISSING}y")]),
            &[],
            &[],
        );
        assert_eq!(map["A"], "xy");
    }

    #[test]
    fn load_env_files_layers_later_over_earlier_and_skips_missing() {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let base = std::env::temp_dir().join(format!("tsr-envfile-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join(".env.local"), "FOO=local\nONLY_LOCAL=1\n").unwrap();
        std::fs::write(base.join(".env.test"), "FOO=test\n").unwrap();

        let files = load_env_files(
            &base,
            &owned_paths(&[".env.local", ".env.test", ".env.missing"]),
        );
        // Collapse to a map to check the effective (last-wins) values.
        let map = build_from(HashMap::new(), &[], &[], &files, &[]);
        assert_eq!(map["FOO"], "test"); // later file wins
        assert_eq!(map["ONLY_LOCAL"], "1"); // earlier-only key preserved
    }

    #[test]
    fn parses_dotenv_forms() {
        let env = parse_dotenv(
            "# comment\n\nexport FOO=bar\nQUOTED=\"hello world\"\nSQ='literal'\nEMPTY=\n",
        );
        assert_eq!(
            env,
            vec![
                ("FOO".into(), "bar".into()),
                ("QUOTED".into(), "hello world".into()),
                ("SQ".into(), "literal".into()),
                ("EMPTY".into(), "".into()),
            ]
        );
    }

    // --- load-time $VAR validation ---

    static N: AtomicUsize = AtomicUsize::new(0);
    fn write_config(text: &str) -> PathBuf {
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-env-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("tasks.toml");
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn undefined_run_var_is_load_error() {
        let cfg = Config::load(&write_config(
            "[tasks.deploy]\nrun = \"deploy --target $TARGET\"\n",
        ))
        .unwrap();
        let keys = vec!["deploy".to_string()];
        let err = validate_run_vars_from(&cfg, &keys, &HashMap::new(), &[]).unwrap_err();
        assert!(err.to_string().contains("$TARGET"));
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn run_var_defined_in_task_env_passes() {
        let cfg = Config::load(&write_config(
            "[tasks.deploy]\nrun = \"deploy $TARGET\"\nenv = { TARGET = \"prod\" }\n",
        ))
        .unwrap();
        let keys = vec!["deploy".to_string()];
        assert!(validate_run_vars_from(&cfg, &keys, &HashMap::new(), &[]).is_ok());
    }

    #[test]
    fn run_var_defined_in_process_env_passes() {
        let cfg = Config::load(&write_config("[tasks.x]\nrun = \"echo $HOME\"\n")).unwrap();
        let keys = vec!["x".to_string()];
        assert!(validate_run_vars_from(&cfg, &keys, &proc(&[("HOME", "/h")]), &[]).is_ok());
    }

    #[test]
    fn run_var_defined_in_dotenv_passes() {
        let cfg = Config::load(&write_config("[tasks.x]\nrun = \"echo $TOKEN\"\n")).unwrap();
        let keys = vec!["x".to_string()];
        assert!(
            validate_run_vars_from(&cfg, &keys, &HashMap::new(), &owned(&[("TOKEN", "abc")]))
                .is_ok()
        );
    }

    #[test]
    fn unreachable_broken_task_is_not_checked() {
        // Only the requested keys are validated; an unrelated undefined-var task
        // must not block them.
        let cfg = Config::load(&write_config(
            "[tasks.ok]\nrun = \"echo hi\"\n[tasks.broken]\nrun = \"deploy $NOPE\"\n",
        ))
        .unwrap();
        let keys = vec!["ok".to_string()];
        assert!(validate_run_vars_from(&cfg, &keys, &HashMap::new(), &[]).is_ok());
    }
}
