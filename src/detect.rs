//! Ecosystem detection via marker files, and the convention-based mapping from
//! a bare task name to its native runner (SPEC §3.1 form 3, §9).

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

/// Read `[table…].name` from a TOML manifest via `toml_edit`.
fn toml_name(path: &Path, table_path: &[&str]) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc: toml_edit::DocumentMut = text.parse().ok()?;
    let mut item = doc.as_item();
    for key in table_path {
        item = item.get(key)?;
    }
    item.get("name")?.as_str().map(str::to_string)
}

/// Extract the top-level `"name"` string from `package.json` without pulling in
/// a full JSON parser: find the first `"name"` key at object depth 1.
fn json_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    // Track key strings at depth 1 followed by `:`.
    while i < bytes.len() {
        let c = bytes[i];
        if in_str {
            if esc {
                esc = false;
            } else if c == b'\\' {
                esc = true;
            } else if c == b'"' {
                in_str = false;
                // Is this the key "name" at depth 1?
                if depth == 1 && text[..i].ends_with("\"name") {
                    // Skip whitespace to ':' then read the value string.
                    let mut j = i + 1;
                    while j < bytes.len() && bytes[j].is_ascii_whitespace() {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b':' {
                        return json_string_after(&text, j + 1);
                    }
                }
            }
        } else {
            match c {
                b'"' => in_str = true,
                b'{' | b'[' => depth += 1,
                b'}' | b']' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Read the next JSON string literal starting at/after `from`.
fn json_string_after(text: &str, from: usize) -> Option<String> {
    let bytes = text.as_bytes();
    let mut i = from;
    while i < bytes.len() && bytes[i] != b'"' {
        i += 1;
    }
    if i >= bytes.len() {
        return None;
    }
    i += 1; // opening quote
    let mut out = String::new();
    let mut esc = false;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if esc {
            out.push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == '"' {
            return Some(out);
        } else {
            out.push(c);
        }
        i += 1;
    }
    None
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
