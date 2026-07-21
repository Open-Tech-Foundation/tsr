//! Workspace package discovery and `packages` matching (SPEC §9.1).
//!
//! Packages are the directories matched by `[workspace] members` globs that
//! carry a recognised ecosystem marker. A task's `packages` entry matches each
//! package by **either** a path glob (`apps/*`) **or** an exact manifest name
//! (`@scope/pkg`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::config::Config;
use crate::detect::{self, Ecosystem};
use crate::error::{Result, TsrError};

/// A discovered workspace package.
#[derive(Debug, Clone)]
pub struct Package {
    /// Absolute path to the package directory.
    pub path: PathBuf,
    /// Path relative to the workspace root, `/`-separated (e.g. `apps/web`).
    pub rel: String,
    /// Declared manifest name, if any (e.g. `@scope/web`).
    pub name: Option<String>,
    /// Detected ecosystem. Retained for diagnostics/`list`; the per-package
    /// command is re-resolved against the package dir at run time.
    #[allow(dead_code)]
    pub eco: Ecosystem,
}

/// Enumerate all workspace packages by expanding `[workspace] members` globs and
/// keeping the directories that carry an ecosystem marker. Results are unique by
/// path and ordered by relative path.
pub fn packages(cfg: &Config) -> Vec<Package> {
    let mut found: BTreeMap<String, Package> = BTreeMap::new();
    for pattern in &cfg.members {
        let abs = cfg.root.join(pattern);
        let Some(pat) = abs.to_str() else { continue };
        let Ok(paths) = glob::glob(pat) else { continue };
        for entry in paths.flatten() {
            if !entry.is_dir() {
                continue;
            }
            let Some(eco) = detect::detect(&entry) else {
                continue;
            };
            let rel = rel_path(&cfg.root, &entry);
            let name = detect::manifest_name(&entry, eco);
            found.entry(rel.clone()).or_insert(Package {
                path: entry,
                rel,
                name,
                eco,
            });
        }
    }
    found.into_values().collect()
}

/// Resolve a task's `packages` patterns to concrete packages (SPEC §9.1),
/// de-duplicated and order-preserving. A pattern matching nothing is a runtime
/// error (exit `64`) — it almost always means a typo.
pub fn match_packages(cfg: &Config, patterns: &[String], task: &str) -> Result<Vec<Package>> {
    let all = packages(cfg);
    let mut selected: Vec<Package> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    for pattern in patterns {
        let mut matched_any = false;
        for pkg in &all {
            if pattern_matches(pattern, pkg) {
                matched_any = true;
                if seen.insert(pkg.rel.clone()) {
                    selected.push(pkg.clone());
                }
            }
        }
        if !matched_any {
            return Err(TsrError::runtime(format!(
                "task '{task}': packages pattern '{pattern}' matched no workspace package"
            )));
        }
    }
    Ok(selected)
}

fn pattern_matches(pattern: &str, pkg: &Package) -> bool {
    if is_glob(pattern) {
        // Path glob against the relative package path.
        glob::Pattern::new(pattern)
            .map(|p| p.matches(&pkg.rel))
            .unwrap_or(false)
    } else {
        // Exact match against the path or the manifest name.
        pkg.rel == pattern || pkg.name.as_deref() == Some(pattern)
    }
}

fn is_glob(pattern: &str) -> bool {
    pattern.contains(['*', '?', '['])
}

fn rel_path(root: &std::path::Path, path: &std::path::Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a temp workspace: root `tasks.toml` plus the given package dirs,
    /// each `(relpath, marker_file, marker_contents)`.
    fn workspace(members: &[&str], pkgs: &[(&str, &str, &str)]) -> Config {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsr-ws-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let members_toml = members
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let toml = format!("[workspace]\nmembers = [{members_toml}]\n");
        let path = root.join("tasks.toml");
        fs::write(&path, toml).unwrap();
        for (rel, marker, contents) in pkgs {
            let dir = root.join(rel);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(marker), contents).unwrap();
        }
        Config::load(&path).unwrap()
    }

    #[test]
    fn enumerates_packages_with_markers() {
        let cfg = workspace(
            &["apps/*", "packages/*"],
            &[
                ("apps/web", "package.json", "{\"name\": \"@scope/web\"}"),
                ("packages/ui", "Cargo.toml", "[package]\nname = \"ui\"\n"),
                ("apps/no-marker", "README.md", ""), // ignored: no marker
            ],
        );
        let pkgs = packages(&cfg);
        let rels: Vec<&str> = pkgs.iter().map(|p| p.rel.as_str()).collect();
        assert_eq!(rels, vec!["apps/web", "packages/ui"]);
        assert_eq!(pkgs[0].name.as_deref(), Some("@scope/web"));
    }

    #[test]
    fn matches_path_glob() {
        let cfg = workspace(
            &["apps/*"],
            &[
                ("apps/web", "package.json", "{}"),
                ("apps/api", "package.json", "{}"),
            ],
        );
        let m = match_packages(&cfg, &["apps/*".into()], "test").unwrap();
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn matches_manifest_name() {
        let cfg = workspace(
            &["packages/*"],
            &[("packages/ui", "package.json", "{\"name\": \"@scope/ui\"}")],
        );
        let m = match_packages(&cfg, &["@scope/ui".into()], "test").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rel, "packages/ui");
    }

    #[test]
    fn dedups_across_patterns() {
        let cfg = workspace(
            &["apps/*"],
            &[("apps/web", "package.json", "{\"name\": \"web\"}")],
        );
        let m = match_packages(&cfg, &["apps/*".into(), "web".into()], "test").unwrap();
        assert_eq!(m.len(), 1);
    }

    #[test]
    fn unmatched_pattern_is_error() {
        let cfg = workspace(&["apps/*"], &[("apps/web", "package.json", "{}")]);
        let err = match_packages(&cfg, &["nope/*".into()], "test").unwrap_err();
        assert_eq!(err.exit_code(), 64);
    }

    // Silence unused import warnings in some configurations.
    #[allow(dead_code)]
    fn _p() -> PathBuf {
        PathBuf::new()
    }
}
