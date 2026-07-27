//! Ecosystem detection via marker files, the convention-based mapping from a
//! bare task name to its native runner (SPEC §3.1 form 3, §9), and the manifest
//! reads that back it: the package's declared **name** (v1) and its declared
//! **dependency names** (v1.1, the raw material for [`crate::pkggraph`]).

use std::path::Path;

/// A package ecosystem, identified by its marker manifest file (SPEC §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    /// `package.json` present, no bun lockfile.
    Npm,
    /// `package.json` present alongside a bun lockfile.
    Bun,
    /// `Cargo.toml` present.
    Cargo,
    /// `go.mod` present.
    Go,
    /// `pyproject.toml` present.
    Python,
}

impl Ecosystem {
    /// Map a bare task name to `(program, args)` for this ecosystem's native
    /// runner, convention-based (SPEC §3.1):
    ///
    /// - npm/bun → `npm run <task>` / `bun run <task>`
    /// - cargo   → `cargo <task>`   (`test`/`build`/`run` are native subcommands)
    /// - go      → `go <task>`
    /// - python  → `uv run <task>`
    pub fn native_command(self, task: &str) -> (String, Vec<String>) {
        match self {
            Ecosystem::Npm => ("npm".into(), vec!["run".into(), task.into()]),
            Ecosystem::Bun => ("bun".into(), vec!["run".into(), task.into()]),
            Ecosystem::Cargo => ("cargo".into(), vec![task.into()]),
            Ecosystem::Go => ("go".into(), vec![task.into()]),
            Ecosystem::Python => ("uv".into(), vec!["run".into(), task.into()]),
        }
    }
}

/// Detect the ecosystem of the package rooted at `dir` by probing for marker
/// files. Node is checked first, disambiguating npm vs bun by lockfile.
///
/// Returns `None` when no recognised marker is present.
pub fn detect(dir: &Path) -> Option<Ecosystem> {
    if dir.join("package.json").is_file() {
        if dir.join("bun.lockb").is_file() || dir.join("bun.lock").is_file() {
            return Some(Ecosystem::Bun);
        }
        return Some(Ecosystem::Npm);
    }
    if dir.join("Cargo.toml").is_file() {
        return Some(Ecosystem::Cargo);
    }
    if dir.join("go.mod").is_file() {
        return Some(Ecosystem::Go);
    }
    if dir.join("pyproject.toml").is_file() {
        return Some(Ecosystem::Python);
    }
    None
}

/// Read the package's manifest name, so `packages` can match against declared
/// names (e.g. `@scope/pkg`), not only path globs (SPEC §9.1). Returns `None`
/// when the manifest is unreadable or declares no name.
pub fn manifest_name(dir: &Path, eco: Ecosystem) -> Option<String> {
    match eco {
        Ecosystem::Npm | Ecosystem::Bun => json_name(&dir.join("package.json")),
        Ecosystem::Cargo => toml_name(&dir.join("Cargo.toml"), &["package"]),
        Ecosystem::Python => toml_name(&dir.join("pyproject.toml"), &["project"])
            .or_else(|| toml_name(&dir.join("pyproject.toml"), &["tool", "poetry"])),
        Ecosystem::Go => go_module(&dir.join("go.mod")),
    }
}

/// Read the names of the package's **declared dependencies** (SPEC §9, v1.1).
///
/// Only names are collected — version specifiers, ranges and protocols are all
/// irrelevant here, because a workspace edge exists exactly when a declared name
/// matches another workspace package's manifest name. That keeps one rule across
/// every ecosystem and sidesteps `workspace:*` vs `^1.2.3` vs `path = "…"`.
///
/// Unreadable or malformed manifests yield no dependencies rather than an error:
/// package discovery must stay total, and the native runner gives a far better
/// diagnostic for a broken manifest than we could.
///
/// The result is grouped by dependency kind in the order listed by each reader.
/// Ordering *within* a kind follows the manifest for the TOML ecosystems and is
/// alphabetical for JSON — deterministic either way, which is what the graph
/// needs; reformatting a manifest never reorders a fan-out.
pub fn manifest_deps(dir: &Path, eco: Ecosystem) -> Vec<String> {
    match eco {
        Ecosystem::Npm | Ecosystem::Bun => json_deps(&dir.join("package.json")),
        Ecosystem::Cargo => cargo_deps(&dir.join("Cargo.toml")),
        Ecosystem::Python => python_deps(&dir.join("pyproject.toml")),
        Ecosystem::Go => go_deps(&dir.join("go.mod")),
    }
}

/// Parse a TOML manifest, or `None` when it is unreadable or malformed.
fn toml_doc(path: &Path) -> Option<toml_edit::DocumentMut> {
    std::fs::read_to_string(path).ok()?.parse().ok()
}

/// Read `[table…].name` from a TOML manifest via `toml_edit`.
fn toml_name(path: &Path, table_path: &[&str]) -> Option<String> {
    let doc = toml_doc(path)?;
    let mut item = doc.as_item();
    for key in table_path {
        item = item.get(key)?;
    }
    item.get("name")?.as_str().map(str::to_string)
}

/// Parse a JSON manifest. `None` when it is unreadable or malformed — a broken
/// `package.json` is the native runner's problem to report, not ours.
fn json_doc(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// Extract the top-level `"name"` string from `package.json`.
fn json_name(path: &Path) -> Option<String> {
    json_doc(path)?.get("name")?.as_str().map(str::to_string)
}

/// Collect the keys of every dependency object in `package.json`.
fn json_deps(path: &Path) -> Vec<String> {
    const KINDS: [&str; 4] = [
        "dependencies",
        "devDependencies",
        "peerDependencies",
        "optionalDependencies",
    ];
    let Some(doc) = json_doc(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in KINDS {
        if let Some(obj) = doc.get(kind).and_then(serde_json::Value::as_object) {
            out.extend(obj.keys().cloned());
        }
    }
    out
}

/// Collect dependency names from a `Cargo.toml`'s three dependency tables. A
/// renamed dependency (`ui = { package = "real-name" }`) contributes the *real*
/// crate name, since that is what matches a workspace member's manifest name.
fn cargo_deps(path: &Path) -> Vec<String> {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    let Some(doc) = toml_doc(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for kind in KINDS {
        let Some(table) = doc.get(kind).and_then(|i| i.as_table_like()) else {
            continue;
        };
        for (key, item) in table.iter() {
            let name = item.get("package").and_then(|p| p.as_str()).unwrap_or(key);
            out.push(name.to_string());
        }
    }
    out
}

/// Collect distribution names from a `pyproject.toml`, covering PEP 621
/// (`[project]`), PEP 735 (`[dependency-groups]`) and Poetry's own tables.
fn python_deps(path: &Path) -> Vec<String> {
    let Some(doc) = toml_doc(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();

    if let Some(project) = doc.get("project").and_then(|i| i.as_table_like()) {
        push_pep508(project.get("dependencies"), &mut out);
        if let Some(extras) = project
            .get("optional-dependencies")
            .and_then(|i| i.as_table_like())
        {
            for (_, group) in extras.iter() {
                push_pep508(Some(group), &mut out);
            }
        }
    }

    if let Some(groups) = doc.get("dependency-groups").and_then(|i| i.as_table_like()) {
        for (_, group) in groups.iter() {
            push_pep508(Some(group), &mut out);
        }
    }

    // Poetry declares dependencies as table *keys*, not PEP 508 strings.
    if let Some(poetry) = doc
        .get("tool")
        .and_then(|t| t.get("poetry"))
        .and_then(|i| i.as_table_like())
    {
        push_table_keys(poetry.get("dependencies"), &mut out);
        if let Some(groups) = poetry.get("group").and_then(|i| i.as_table_like()) {
            for (_, g) in groups.iter() {
                push_table_keys(g.get("dependencies"), &mut out);
            }
        }
    }
    out
}

/// Push the distribution names from an array of PEP 508 requirement strings.
fn push_pep508(item: Option<&toml_edit::Item>, out: &mut Vec<String>) {
    let Some(arr) = item.and_then(|i| i.as_array()) else {
        return;
    };
    for value in arr.iter() {
        if let Some(name) = value.as_str().and_then(pep508_name) {
            out.push(name);
        }
    }
}

/// Push the keys of a dependency table.
fn push_table_keys(item: Option<&toml_edit::Item>, out: &mut Vec<String>) {
    if let Some(table) = item.and_then(|i| i.as_table_like()) {
        out.extend(table.iter().map(|(k, _)| k.to_string()));
    }
}

/// The distribution name of a PEP 508 requirement: the leading run before any
/// extras, version specifier, environment marker or URL (`pkg[x]>=1 ; y` → `pkg`).
fn pep508_name(req: &str) -> Option<String> {
    let name: String = req
        .trim()
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Collect module paths from a `go.mod`'s `require` and `replace` directives,
/// in both the single-line and parenthesised-block forms.
fn go_deps(path: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut in_block = false;
    for raw in text.lines() {
        // Module paths never contain `//`, so this is a safe comment strip.
        let line = raw.split("//").next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if in_block {
            if line == ")" {
                in_block = false;
            } else {
                push_first_word(line, &mut out);
            }
            continue;
        }
        for directive in ["require", "replace"] {
            let Some(rest) = directive_body(line, directive) else {
                continue;
            };
            if rest == "(" {
                in_block = true;
            } else {
                push_first_word(rest, &mut out);
            }
            break;
        }
    }
    out
}

/// The remainder of `line` after `directive`, if the line really opens with that
/// directive (guards against `requirements ...` matching `require`).
fn directive_body<'a>(line: &'a str, directive: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(directive)?;
    if rest.starts_with(char::is_whitespace) || rest.starts_with('(') {
        Some(rest.trim())
    } else {
        None
    }
}

fn push_first_word(line: &str, out: &mut Vec<String>) {
    if let Some(word) = line.split_whitespace().next() {
        out.push(word.to_string());
    }
}

/// Read the module path from a `go.mod` `module <path>` directive.
fn go_module(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("module ") {
            return Some(rest.trim().to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-detect-{}-{id}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn detects_npm_and_bun() {
        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Npm));
        fs::write(d.join("bun.lockb"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Bun));
    }

    #[test]
    fn detects_cargo_go_python() {
        let d = scratch();
        fs::write(d.join("Cargo.toml"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Cargo));

        let d = scratch();
        fs::write(d.join("go.mod"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Go));

        let d = scratch();
        fs::write(d.join("pyproject.toml"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Python));
    }

    #[test]
    fn none_when_no_marker() {
        assert_eq!(detect(&scratch()), None);
    }

    #[test]
    fn reads_manifest_names() {
        let d = scratch();
        fs::write(
            d.join("package.json"),
            "{\n  \"name\": \"@scope/web\",\n  \"version\": \"1\"\n}",
        )
        .unwrap();
        assert_eq!(manifest_name(&d, Ecosystem::Npm), Some("@scope/web".into()));

        let d = scratch();
        fs::write(d.join("Cargo.toml"), "[package]\nname = \"my-crate\"\n").unwrap();
        assert_eq!(manifest_name(&d, Ecosystem::Cargo), Some("my-crate".into()));

        let d = scratch();
        fs::write(d.join("go.mod"), "module github.com/me/proj\n\ngo 1.22\n").unwrap();
        assert_eq!(
            manifest_name(&d, Ecosystem::Go),
            Some("github.com/me/proj".into())
        );

        let d = scratch();
        fs::write(d.join("pyproject.toml"), "[project]\nname = \"pkg\"\n").unwrap();
        assert_eq!(manifest_name(&d, Ecosystem::Python), Some("pkg".into()));
    }

    /// `manifest_deps` returns every *declared* name verbatim — filtering to
    /// workspace members is the graph's job, not the reader's.
    #[test]
    fn reads_declared_dependency_names() {
        let d = scratch();
        fs::write(
            d.join("package.json"),
            r#"{"name": "app", "dependencies": {"ui": "workspace:*", "react": "^18"},
                "devDependencies": {"vitest": "*"}}"#,
        )
        .unwrap();
        // Names are grouped by dependency kind, alphabetical within each.
        assert_eq!(
            manifest_deps(&d, Ecosystem::Npm),
            vec!["react", "ui", "vitest"]
        );

        let d = scratch();
        fs::write(
            d.join("Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\nserde = \"1\"\n\
             alias = { package = \"real\", path = \"../real\" }\n[dev-dependencies]\ntempfile = \"3\"\n",
        )
        .unwrap();
        // `alias` contributes the renamed crate's real name, `real`.
        assert_eq!(
            manifest_deps(&d, Ecosystem::Cargo),
            vec!["serde", "real", "tempfile"]
        );

        let d = scratch();
        fs::write(
            d.join("go.mod"),
            "module example.com/app\n\nrequire single.example/one v1.0.0\n\n\
             require (\n\tex.com/a v0.1.0 // indirect\n\tex.com/b v2.0.0\n)\n",
        )
        .unwrap();
        assert_eq!(
            manifest_deps(&d, Ecosystem::Go),
            vec!["single.example/one", "ex.com/a", "ex.com/b"]
        );
    }

    /// PEP 508 requirements carry extras, specifiers and markers; only the
    /// distribution name identifies the package.
    #[test]
    fn strips_pep508_decoration() {
        let d = scratch();
        fs::write(
            d.join("pyproject.toml"),
            "[project]\nname = \"app\"\n\
             dependencies = [\"core>=1.0,<2\", \"web[async]\", \"tool ; python_version >= '3.11'\", \"plain\"]\n",
        )
        .unwrap();
        assert_eq!(
            manifest_deps(&d, Ecosystem::Python),
            vec!["core", "web", "tool", "plain"]
        );
    }

    /// A `go.mod` directive is only a directive when the keyword stands alone.
    #[test]
    fn go_reader_ignores_lookalike_directives() {
        let d = scratch();
        fs::write(
            d.join("go.mod"),
            "module example.com/app\n\nrequirements v1\nreplacement v2\nrequire real.example/x v1.0.0\n",
        )
        .unwrap();
        assert_eq!(manifest_deps(&d, Ecosystem::Go), vec!["real.example/x"]);
    }

    /// Manifest reads must never panic or error out: discovery has to stay total.
    #[test]
    fn malformed_manifests_yield_nothing() {
        let d = scratch();
        fs::write(d.join("package.json"), "{ this is not json").unwrap();
        assert_eq!(manifest_name(&d, Ecosystem::Npm), None);
        assert!(manifest_deps(&d, Ecosystem::Npm).is_empty());

        let d = scratch();
        fs::write(d.join("Cargo.toml"), "[package\nname =").unwrap();
        assert_eq!(manifest_name(&d, Ecosystem::Cargo), None);
        assert!(manifest_deps(&d, Ecosystem::Cargo).is_empty());

        // A manifest that is absent entirely is equally inert.
        let d = scratch();
        assert!(manifest_deps(&d, Ecosystem::Go).is_empty());
    }

    #[test]
    fn native_command_conventions() {
        assert_eq!(
            Ecosystem::Npm.native_command("test"),
            ("npm".into(), vec!["run".into(), "test".into()])
        );
        assert_eq!(
            Ecosystem::Cargo.native_command("test"),
            ("cargo".into(), vec!["test".into()])
        );
        assert_eq!(
            Ecosystem::Go.native_command("build"),
            ("go".into(), vec!["build".into()])
        );
        assert_eq!(
            Ecosystem::Python.native_command("lint"),
            ("uv".into(), vec!["run".into(), "lint".into()])
        );
    }
}
