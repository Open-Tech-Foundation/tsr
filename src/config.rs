//! Config model, `tasks.toml` parsing, workspace-root discovery, and the
//! structural validation performed at load time (SPEC §2, §3, §4).
//!
//! Parsing goes through `toml_edit` so comments and unknown keys survive a
//! round-trip (SPEC §1.5): the original [`DocumentMut`] is retained on
//! [`Config`] and never discarded.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Value};

use crate::error::{Result, TsrError};

/// The canonical config-file name; also the workspace-root anchor (SPEC §2).
pub const CONFIG_FILE: &str = "tasks.toml";

/// A fully parsed and structurally validated workspace configuration.
#[derive(Debug)]
pub struct Config {
    /// Absolute path to the workspace root (the directory holding `tasks.toml`).
    pub root: PathBuf,
    /// `[workspace] members` globs (empty for a single-package repo).
    pub members: Vec<String>,
    /// Workspace-wide `[env]`, preserved in declaration order so later keys may
    /// reference earlier ones (SPEC §7.3).
    pub env: Vec<(String, String)>,
    /// Tasks keyed by their full table key (may contain a `#` package prefix).
    pub tasks: BTreeMap<String, Task>,
    /// The parsed document, retained so comments and unknown keys survive a
    /// round-trip when the config is rewritten (SPEC §1.5). Not read on the
    /// execution path; consumed by tooling and the round-trip test.
    #[allow(dead_code)]
    pub doc: DocumentMut,
}

/// A backend hand-off target (SPEC §3, form 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delegate {
    /// String form: `delegate = "turbo"` → `turbo run <task>`.
    Bin(String),
    /// Table form: `delegate = { bin = "make", args = ["bundle"] }`.
    Full { bin: String, args: Vec<String> },
}

/// A single `[tasks.<name>]` table, parsed into a typed model.
#[derive(Debug, Clone, Default)]
pub struct Task {
    /// The full table key as written (e.g. `test` or `web#build`).
    pub key: String,
    pub run: Option<String>,
    pub delegate: Option<Delegate>,
    pub dir: Option<String>,
    pub packages: Option<Vec<String>>,
    pub deps: Vec<String>,
    pub parallel: bool,
    pub args: Vec<String>,
    /// Per-task env, in declaration order (SPEC §7.1).
    pub env: Vec<(String, String)>,
    /// `.env`-style files to load for this task, in listed order (later files
    /// override earlier ones), resolved relative to the task's directory. Layered
    /// above the root `.env` and workspace `[env]`, below the inline `env`
    /// (SPEC §7.2).
    pub env_files: Vec<String>,
}

impl Task {
    /// The task-name portion of the key: everything after a `#` package prefix,
    /// or the whole key when there is none (SPEC §4.2). This is what form-3
    /// auto-detection and `delegate` string form map onto the native runner.
    pub fn task_name(&self) -> &str {
        self.key.rsplit('#').next().unwrap_or(&self.key)
    }
}

impl Config {
    /// Load and validate a specific `tasks.toml` file.
    pub fn load(path: &Path) -> Result<Config> {
        let text = fs::read_to_string(path)
            .map_err(|e| TsrError::config(format!("cannot read '{}': {e}", path.display())))?;
        let root = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let cfg = parse(&text, root)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Look up a task by its full key.
    pub fn task(&self, key: &str) -> Option<&Task> {
        self.tasks.get(key)
    }
}

/// Locate the nearest existing `tasks.toml` at/above `start`, if any. Used by the
/// `--config` TUI to decide whether to open an existing workspace or start a new
/// file in the current directory.
pub fn locate(start: &Path) -> Option<PathBuf> {
    find_config(start)
}

/// Walk up from `start` to the nearest directory carrying a recognised ecosystem
/// marker (`package.json`, `Cargo.toml`, `go.mod`, `pyproject.toml`). Anchors
/// **configless** mode: when there is no `tasks.toml`, this is where `tsr <task>`
/// auto-detects the native runner from.
pub fn nearest_package_root(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        Some(start.to_path_buf())
    } else {
        start.parent().map(Path::to_path_buf)
    };
    while let Some(d) = dir {
        if crate::detect::detect(&d).is_some() {
            return Some(d);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// Build a synthetic single-task config for **configless** mode (SPEC §3.1,
/// form 3). With no `tasks.toml`, `tsr <task>` still works repo-aware: `task` is
/// treated as a bare auto-detect task rooted at `root`, so `tsr dev` in a
/// `package.json` repo runs `npm run dev`, in a Cargo repo `cargo dev`, and so on.
pub fn implicit(root: PathBuf, task: &str) -> Config {
    let mut tasks = BTreeMap::new();
    tasks.insert(
        task.to_string(),
        Task {
            key: task.to_string(),
            ..Task::default()
        },
    );
    Config {
        root,
        members: Vec::new(),
        env: Vec::new(),
        tasks,
        doc: DocumentMut::new(),
    }
}

/// Validate a full config document given as text, without touching the file
/// system — parse then run the structural checks. Used by the `--config` TUI to
/// reject an edit before it is written.
pub(crate) fn validate_str(text: &str) -> Result<()> {
    parse_str(text, PathBuf::from(".")).map(|_| ())
}

/// Parse and validate a config document from text, resolving auto-detect against
/// `root`. Used by the `--config` TUI's graph/dry-run preview, which needs the
/// typed [`Task`] model (not just a yes/no validation).
pub(crate) fn parse_str(text: &str, root: PathBuf) -> Result<Config> {
    let cfg = parse(text, root)?;
    cfg.validate()?;
    Ok(cfg)
}

/// Validate a single task-name/key against the grammar (SPEC §4.1). Exposed for
/// the TUI's per-field feedback.
pub(crate) fn validate_task_name(key: &str) -> Result<()> {
    validate_task_key(key)
}

/// Walk up from `start` looking for the nearest `tasks.toml`.
fn find_config(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        Some(start.to_path_buf())
    } else {
        start.parent().map(Path::to_path_buf)
    };
    while let Some(d) = dir {
        let candidate = d.join(CONFIG_FILE);
        if candidate.is_file() {
            return Some(candidate);
        }
        dir = d.parent().map(Path::to_path_buf);
    }
    None
}

/// Parse TOML text into the typed [`Config`] model (no cross-field validation).
fn parse(text: &str, root: PathBuf) -> Result<Config> {
    let doc: DocumentMut = text
        .parse()
        .map_err(|e| TsrError::config(format!("invalid TOML in '{CONFIG_FILE}': {e}")))?;

    let mut members = Vec::new();
    if let Some(ws) = doc.get("workspace").and_then(Item::as_table_like)
        && let Some(m) = ws.get("members")
    {
        members = parse_string_array(m, "workspace.members")?;
    }

    let env = match doc.get("env") {
        Some(item) => parse_env_table(item, "env")?,
        None => Vec::new(),
    };

    let mut tasks = BTreeMap::new();
    if let Some(tbl) = doc.get("tasks").and_then(Item::as_table_like) {
        for (key, item) in tbl.iter() {
            let task = parse_task(key, item)?;
            tasks.insert(key.to_string(), task);
        }
    }

    Ok(Config {
        root,
        members,
        env,
        tasks,
        doc,
    })
}

fn parse_task(key: &str, item: &Item) -> Result<Task> {
    let tbl = item
        .as_table_like()
        .ok_or_else(|| TsrError::config(format!("task '{key}' must be a table")))?;

    let mut task = Task {
        key: key.to_string(),
        ..Task::default()
    };

    for (field, value) in tbl.iter() {
        match field {
            "run" => task.run = Some(expect_string(value, key, "run")?),
            "dir" => task.dir = Some(expect_string(value, key, "dir")?),
            "delegate" => task.delegate = Some(parse_delegate(value, key)?),
            "packages" => {
                task.packages = Some(parse_string_array(value, &format!("tasks.{key}.packages"))?)
            }
            "deps" => task.deps = parse_string_array(value, &format!("tasks.{key}.deps"))?,
            "args" => task.args = parse_string_array(value, &format!("tasks.{key}.args"))?,
            "parallel" => {
                task.parallel = value.as_bool().ok_or_else(|| {
                    TsrError::config(format!("task '{key}': 'parallel' must be a boolean"))
                })?
            }
            "env" => task.env = parse_env_table(value, &format!("tasks.{key}.env"))?,
            "env_file" => {
                task.env_files = parse_string_or_array(value, &format!("tasks.{key}.env_file"))?
            }
            // Unknown keys are tolerated: they round-trip via `doc` (SPEC §1.5).
            _ => {}
        }
    }
    Ok(task)
}

fn parse_delegate(item: &Item, key: &str) -> Result<Delegate> {
    // String form: `delegate = "turbo"`.
    if let Some(s) = item.as_str() {
        if s.is_empty() {
            return Err(TsrError::config(format!(
                "task '{key}': 'delegate' string must not be empty"
            )));
        }
        return Ok(Delegate::Bin(s.to_string()));
    }
    // Table form: `delegate = { bin = "make", args = ["bundle"] }`.
    if let Some(tbl) = item.as_table_like() {
        let bin = tbl
            .get("bin")
            .and_then(Item::as_str)
            .ok_or_else(|| {
                TsrError::config(format!(
                    "task '{key}': 'delegate' table requires a string 'bin'"
                ))
            })?
            .to_string();
        let args = match tbl.get("args") {
            Some(a) => parse_string_array(a, &format!("tasks.{key}.delegate.args"))?,
            None => Vec::new(),
        };
        return Ok(Delegate::Full { bin, args });
    }
    Err(TsrError::config(format!(
        "task '{key}': 'delegate' must be a string or a {{ bin, args }} table"
    )))
}

fn expect_string(item: &Item, key: &str, field: &str) -> Result<String> {
    item.as_str()
        .map(str::to_string)
        .ok_or_else(|| TsrError::config(format!("task '{key}': '{field}' must be a string")))
}

/// Parse either a single string or an array of strings into a `Vec<String>`, so
/// `env_file = ".env.test"` and `env_file = [".env.local", ".env.test"]` both
/// work. Empty entries are rejected.
fn parse_string_or_array(item: &Item, ctx: &str) -> Result<Vec<String>> {
    let out = if let Some(s) = item.as_str() {
        vec![s.to_string()]
    } else {
        parse_string_array(item, ctx)?
    };
    if out.iter().any(|s| s.trim().is_empty()) {
        return Err(TsrError::config(format!(
            "'{ctx}' must not contain empty paths"
        )));
    }
    Ok(out)
}

/// Parse an array of strings; rejects non-arrays and non-string elements.
fn parse_string_array(item: &Item, ctx: &str) -> Result<Vec<String>> {
    let arr = item
        .as_array()
        .ok_or_else(|| TsrError::config(format!("'{ctx}' must be an array of strings")))?;
    let mut out = Vec::with_capacity(arr.len());
    for v in arr.iter() {
        let s = v
            .as_str()
            .ok_or_else(|| TsrError::config(format!("'{ctx}' must contain only strings")))?;
        out.push(s.to_string());
    }
    Ok(out)
}

/// Parse an inline or standard table of `KEY = "value"` string pairs, preserving
/// declaration order (SPEC §7.3 — later keys may reference earlier ones).
fn parse_env_table(item: &Item, ctx: &str) -> Result<Vec<(String, String)>> {
    let tbl = item
        .as_table_like()
        .ok_or_else(|| TsrError::config(format!("'{ctx}' must be a table")))?;
    let mut out = Vec::new();
    for (k, v) in tbl.iter() {
        let s = value_as_str(v)
            .ok_or_else(|| TsrError::config(format!("'{ctx}.{k}' must be a string")))?;
        out.push((k.to_string(), s));
    }
    Ok(out)
}

fn value_as_str(item: &Item) -> Option<String> {
    match item {
        Item::Value(Value::String(s)) => Some(s.value().to_string()),
        _ => item.as_str().map(str::to_string),
    }
}

impl Config {
    /// Structural validation performed once at load time (SPEC §3.3, §4).
    /// `$VAR` resolution is validated later, against the merged env (SPEC §7.3).
    fn validate(&self) -> Result<()> {
        for task in self.tasks.values() {
            validate_task_key(&task.key)?;

            if task.dir.is_some() && task.packages.is_some() {
                return Err(TsrError::config(format!(
                    "task '{}': 'dir' and 'packages' are mutually exclusive",
                    task.key
                )));
            }

            for dep in &task.deps {
                validate_dep_ref(&task.key, dep)?;
            }

            // Reject unsupported mini-shell metacharacters at load time (SPEC
            // §8.2/§8.4). `$VAR` resolution is checked later, once the env is
            // merged (SPEC §7.3).
            if let Some(run) = &task.run {
                crate::shell::parse(run).map_err(|e| {
                    TsrError::config(format!("task '{}': {}", task.key, strip_prefix(&e)))
                })?;
            }
        }
        Ok(())
    }
}

/// Strip the leading "✗ config error: " that `Display` adds, so a wrapped
/// message doesn't repeat the banner.
fn strip_prefix(e: &TsrError) -> String {
    let s = e.to_string();
    s.strip_prefix("✗ config error: ")
        .map(str::to_string)
        .unwrap_or(s)
}

/// A task table key: an optional `pkg#` prefix, then a task name. Both segments
/// must match the name grammar `[A-Za-z0-9_:-]+` (SPEC §4.1).
fn validate_task_key(key: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('#').collect();
    match parts.as_slice() {
        [name] => validate_name_segment(key, name),
        [pkg, name] => {
            validate_name_segment(key, pkg)?;
            validate_name_segment(key, name)
        }
        _ => Err(TsrError::config(format!(
            "task '{key}': at most one '#' (package↔task separator) is allowed"
        ))),
    }
}

/// A dependency reference: `task`, `pkg#task`. The `^upstream` form is v1.1 and
/// is rejected here with a pointer to the version boundary (SPEC §5, §11).
fn validate_dep_ref(owner: &str, dep: &str) -> Result<()> {
    if dep.starts_with('^') {
        return Err(TsrError::config(format!(
            "task '{owner}': upstream dep '{dep}' (the '^' marker) is a v1.1 feature"
        )));
    }
    validate_task_key(dep)
        .map_err(|_| TsrError::config(format!("task '{owner}': invalid dependency '{dep}'")))
}

fn validate_name_segment(key: &str, seg: &str) -> Result<()> {
    if seg.is_empty() {
        return Err(TsrError::config(format!(
            "task '{key}': empty name segment"
        )));
    }
    for c in seg.chars() {
        if !is_name_char(c) {
            return Err(TsrError::config(format!(
                "task '{key}': illegal character '{c}' — task names allow only [A-Za-z0-9_:-]"
            )));
        }
    }
    Ok(())
}

/// Legal task-name characters: letters, digits, `_`, `-`, `:` (SPEC §4.1).
fn is_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ':'
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Create a unique temp dir under the OS temp root and drop `tasks.toml` in
    /// it, returning the config path. (No external tempdir crate.)
    fn write_config(text: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-test-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        fs::write(&path, text).unwrap();
        path
    }

    fn load(text: &str) -> Result<Config> {
        Config::load(&write_config(text))
    }

    #[test]
    fn parses_all_task_forms() {
        let cfg = load(
            r#"
            [workspace]
            members = ["apps/*", "packages/*"]

            [env]
            NODE_ENV = "development"

            [tasks.dev]
            run = "vite"
            dir = "apps/web"
            args = ["--host"]

            [tasks.test]
            packages = ["apps/*"]

            [tasks.build]
            delegate = "turbo"

            [tasks.bundle]
            delegate = { bin = "make", args = ["bundle"] }

            [tasks.ci]
            deps = ["lint", "test"]
            parallel = true
            env = { CI = "true" }
            "#,
        )
        .unwrap();

        assert_eq!(cfg.members, vec!["apps/*", "packages/*"]);
        assert_eq!(cfg.env, vec![("NODE_ENV".into(), "development".into())]);

        let dev = cfg.task("dev").unwrap();
        assert_eq!(dev.run.as_deref(), Some("vite"));
        assert_eq!(dev.dir.as_deref(), Some("apps/web"));
        assert_eq!(dev.args, vec!["--host"]);

        assert_eq!(
            cfg.task("test").unwrap().packages,
            Some(vec!["apps/*".into()])
        );
        assert_eq!(
            cfg.task("build").unwrap().delegate,
            Some(Delegate::Bin("turbo".into()))
        );
        assert_eq!(
            cfg.task("bundle").unwrap().delegate,
            Some(Delegate::Full {
                bin: "make".into(),
                args: vec!["bundle".into()],
            })
        );

        let ci = cfg.task("ci").unwrap();
        assert!(ci.parallel);
        assert_eq!(ci.deps, vec!["lint", "test"]);
        assert_eq!(ci.env, vec![("CI".into(), "true".into())]);
    }

    #[test]
    fn preserves_comments_and_unknown_keys_on_round_trip() {
        let src =
            "# top comment\n[tasks.dev]\nrun = \"vite\" # trailing\nfuture_key = \"keep me\"\n";
        let cfg = load(src).unwrap();
        // Unknown key is tolerated (not modeled) but survives via the document.
        assert_eq!(cfg.doc.to_string(), src);
    }

    #[test]
    fn rejects_dir_and_packages_together() {
        let err = load("[tasks.x]\nrun = \"a\"\ndir = \"p\"\npackages = [\"q\"]\n").unwrap_err();
        assert!(matches!(err, TsrError::Config(_)));
        assert!(err.to_string().contains("mutually exclusive"));
    }

    #[test]
    fn accepts_colon_in_task_names() {
        let cfg = load("[tasks.\"build:prod\"]\nrun = \"vite build\"\n").unwrap();
        assert!(cfg.task("build:prod").is_some());
    }

    #[test]
    fn accepts_hash_package_task_key() {
        let cfg = load("[tasks.\"web#build:prod\"]\nrun = \"vite build\"\n").unwrap();
        assert!(cfg.task("web#build:prod").is_some());
    }

    #[test]
    fn rejects_illegal_task_name_char() {
        let err = load("[tasks.\"bad name\"]\nrun = \"a\"\n").unwrap_err();
        assert!(err.to_string().contains("illegal character"));
    }

    #[test]
    fn rejects_double_hash_in_key() {
        let err = load("[tasks.\"a#b#c\"]\nrun = \"x\"\n").unwrap_err();
        assert!(err.to_string().contains("at most one '#'"));
    }

    #[test]
    fn rejects_upstream_dep_as_v1_1() {
        let err = load("[tasks.ci]\ndeps = [\"^build\"]\n").unwrap_err();
        assert!(err.to_string().contains("v1.1"));
    }

    #[test]
    fn rejects_invalid_toml() {
        let err = load("[tasks.dev\nrun = ").unwrap_err();
        assert_eq!(err.exit_code(), crate::error::EXIT_RUNNER_ERROR);
    }

    #[test]
    fn rejects_unsupported_run_metachar_at_load() {
        let err = load("[tasks.x]\nrun = \"cat a | grep b\"\n").unwrap_err();
        assert!(matches!(err, TsrError::Config(_)));
        assert!(err.to_string().contains("task 'x'"));
        assert!(err.to_string().contains("pipe"));
    }

    #[test]
    fn accepts_supported_run_metachars_at_load() {
        // `&&` and `$VAR` are supported; they must not be rejected at load.
        assert!(load("[tasks.x]\nrun = \"lint && test\"\n").is_ok());
        assert!(load("[tasks.x]\nrun = \"deploy $TARGET\"\n").is_ok());
    }

    #[test]
    fn locates_root_by_walking_up() {
        let path = write_config("[tasks.dev]\nrun = \"vite\"\n");
        let root = path.parent().unwrap();
        let nested = root.join("a").join("b");
        fs::create_dir_all(&nested).unwrap();
        let found = locate(&nested).expect("should find tasks.toml in a parent");
        assert_eq!(found.parent().unwrap(), root);
        let cfg = Config::load(&found).unwrap();
        assert_eq!(cfg.root, root);
        assert!(cfg.task("dev").is_some());
    }

    #[test]
    fn nearest_package_root_walks_up_to_a_marker() {
        let dir = std::env::temp_dir().join(format!("tsr-nearest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let nested = dir.join("src").join("inner");
        fs::create_dir_all(&nested).unwrap();
        fs::write(dir.join("package.json"), "{}").unwrap();
        assert_eq!(
            nearest_package_root(&nested).as_deref(),
            Some(dir.as_path())
        );

        // No marker anywhere up to a bare temp subtree → None.
        let bare = std::env::temp_dir().join(format!("tsr-bare-{}-xyz", std::process::id()));
        let _ = fs::remove_dir_all(&bare);
        fs::create_dir_all(&bare).unwrap();
        assert_eq!(nearest_package_root(&bare), None);
    }

    #[test]
    fn implicit_config_has_one_bare_autodetect_task() {
        let cfg = implicit(PathBuf::from("/tmp/x"), "dev");
        let task = cfg.task("dev").expect("implicit task present");
        assert!(task.run.is_none() && task.delegate.is_none() && task.packages.is_none());
        assert!(task.deps.is_empty());
    }
}
