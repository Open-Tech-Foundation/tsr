//! Affected-package detection (SPEC §9.3, v1.1) — the `--since <ref>` filter.
//!
//! Two steps: ask git which files changed since a ref, then map those files onto
//! workspace packages and widen the set downstream through
//! [`PackageGraph::downstream_closure`]. Changing a library selects every
//! package that depends on it, because those are exactly the ones whose result
//! could differ.
//!
//! Only the **selection** is narrowed. Upstream dependencies are still built:
//! `^task` needs them regardless of whether they changed, so a filtered run
//! remains correct rather than merely fast.

use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use crate::error::{Result, TsrError};
use crate::pkggraph::PackageGraph;

/// Files changed since `since`, as workspace-root-relative `/`-separated paths.
///
/// Covers committed changes, unstaged edits **and** untracked files: a brand-new
/// package exists only as untracked files, and missing it would silently skip
/// the very thing that changed.
pub fn changed_files(root: &Path, since: &str) -> Result<Vec<String>> {
    let mut files = git(root, &["diff", "--name-only", "--relative", since])?;
    files.extend(git(root, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(files)
}

/// Run git in `root` and split stdout into lines. Any git failure — not a repo,
/// unknown ref, git not installed — is a runner error (exit `64`), because the
/// alternative is silently running the wrong subset.
fn git(root: &Path, args: &[&str]) -> Result<Vec<String>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| {
            TsrError::runtime(format!(
                "'--since' needs git on PATH, but it could not be run: {e}"
            ))
        })?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(TsrError::runtime(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }

    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.replace('\\', "/"))
        .collect())
}

/// The packages affected by `files`: the packages those files live in, plus
/// every package that transitively depends on one of them.
///
/// `None` means **do not narrow at all**. A changed file that belongs to no
/// package — the root `tasks.toml`, a lockfile, a CI workflow, a shared config —
/// could affect any package, so the safe answer is to run everything. Skipping
/// work that should have run is a far worse failure than running too much.
pub fn affected(graph: &PackageGraph, files: &[String]) -> Option<HashSet<String>> {
    let mut roots: Vec<usize> = Vec::new();
    let mut seen: HashSet<usize> = HashSet::new();
    for file in files {
        let owner = owning_package(graph, file)?;
        if seen.insert(owner) {
            roots.push(owner);
        }
    }

    let mut out: HashSet<String> = roots.iter().map(|&i| graph.get(i).rel.clone()).collect();
    for i in graph.downstream_closure(&roots) {
        out.insert(graph.get(i).rel.clone());
    }
    Some(out)
}

/// The package a workspace-relative path lives in, most specific first so a
/// nested package wins over the one containing it.
fn owning_package(graph: &PackageGraph, file: &str) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (rel length, index)
    for (i, pkg) in graph.packages().iter().enumerate() {
        if file.starts_with(&format!("{}/", pkg.rel))
            && best.is_none_or(|(len, _)| pkg.rel.len() > len)
        {
            best = Some((pkg.rel.len(), i));
        }
    }
    best.map(|(_, i)| i)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A workspace with `apps/web` → `packages/ui` → `packages/tokens`, plus an
    /// unrelated `apps/docs`.
    fn graph() -> PackageGraph {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsr-affected-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let pkgs = [
            ("apps/web", r#"{"name":"web","dependencies":{"ui":"*"}}"#),
            ("apps/docs", r#"{"name":"docs"}"#),
            (
                "packages/ui",
                r#"{"name":"ui","dependencies":{"tokens":"*"}}"#,
            ),
            ("packages/tokens", r#"{"name":"tokens"}"#),
        ];
        for (rel, manifest) in pkgs {
            let dir = root.join(rel);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("package.json"), manifest).unwrap();
        }
        let path = root.join("tasks.toml");
        fs::write(
            &path,
            "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n",
        )
        .unwrap();
        PackageGraph::build(&Config::load(&path).unwrap())
    }

    fn sorted(set: HashSet<String>) -> Vec<String> {
        let mut v: Vec<String> = set.into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn a_changed_package_selects_itself() {
        let g = graph();
        let a = affected(&g, &["apps/docs/index.md".into()]).unwrap();
        assert_eq!(sorted(a), vec!["apps/docs"]);
    }

    #[test]
    fn a_changed_library_selects_its_dependents() {
        let g = graph();
        // tokens → ui → web, so touching tokens selects all three.
        let a = affected(&g, &["packages/tokens/src/x.ts".into()]).unwrap();
        assert_eq!(
            sorted(a),
            vec!["apps/web", "packages/tokens", "packages/ui"]
        );
    }

    #[test]
    fn dependents_widen_but_dependencies_do_not() {
        let g = graph();
        // Touching an app selects only that app — its libraries did not change.
        let a = affected(&g, &["apps/web/src/main.ts".into()]).unwrap();
        assert_eq!(sorted(a), vec!["apps/web"]);
    }

    #[test]
    fn a_file_outside_every_package_widens_to_everything() {
        let g = graph();
        assert!(affected(&g, &["tasks.toml".into()]).is_none());
        assert!(affected(&g, &[".github/workflows/ci.yml".into()]).is_none());
        // Even alongside a package-scoped change.
        assert!(affected(&g, &["apps/web/x.ts".into(), "README.md".into()]).is_none());
    }

    #[test]
    fn no_changes_affects_nothing() {
        let g = graph();
        assert!(affected(&g, &[]).unwrap().is_empty());
    }

    #[test]
    fn multiple_changes_union_their_effects() {
        let g = graph();
        let a = affected(&g, &["apps/docs/a.md".into(), "packages/ui/b.ts".into()]).unwrap();
        assert_eq!(sorted(a), vec!["apps/docs", "apps/web", "packages/ui"]);
    }

    #[test]
    fn a_package_directory_prefix_is_not_a_partial_match() {
        let g = graph();
        // `apps/website` is not inside `apps/web`, so this is outside every
        // package and must widen rather than select `apps/web`.
        assert!(affected(&g, &["apps/website/x.ts".into()]).is_none());
    }
}
