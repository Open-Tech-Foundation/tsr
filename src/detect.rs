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

    /// The marker file that identifies this ecosystem.
    pub fn marker(self) -> &'static str {
        match self {
            Ecosystem::Npm | Ecosystem::Bun => "package.json",
            Ecosystem::Cargo => "Cargo.toml",
            Ecosystem::Go => "go.mod",
            Ecosystem::Python => "pyproject.toml",
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
