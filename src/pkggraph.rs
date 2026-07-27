//! The **package** dependency graph (SPEC §9 v1.1) — distinct from
//! [`crate::graph`], which validates the *task* DAG declared in `tasks.toml`.
//!
//! Every workspace package declares dependencies in its own manifest. An edge
//! exists exactly when a declared dependency name matches another workspace
//! package's manifest name; external registry dependencies simply do not match
//! and are dropped. That single rule holds across all five ecosystems, so
//! `workspace:*`, `path = "../ui"`, `replace` directives and plain version
//! ranges all resolve identically without special-casing any protocol.
//!
//! This is the substrate the rest of v1.1 is built on: `^task` walks
//! [`PackageGraph::deps_of`] upward, affected-detection walks
//! [`PackageGraph::dependents_of`] downward from the changed set.

use std::collections::{HashMap, HashSet};

use crate::config::Config;
use crate::detect;
use crate::error::{Result, TsrError};
use crate::workspace::{self, Package};

/// A workspace's packages plus the edges between them, addressed by index.
///
/// Package graphs are **not** required to be acyclic: npm tolerates circular
/// dependencies and real repos ship them. Construction therefore never fails —
/// only [`PackageGraph::topo_order`], which cannot answer at all in a cycle,
/// reports one. The closure walks are cycle-safe by construction.
#[derive(Debug, Clone, Default)]
pub struct PackageGraph {
    packages: Vec<Package>,
    /// `deps[i]` — packages that `i` depends on (upstream).
    deps: Vec<Vec<usize>>,
    /// `dependents[i]` — packages that depend on `i` (downstream).
    dependents: Vec<Vec<usize>>,
    by_rel: HashMap<String, usize>,
    by_name: HashMap<String, usize>,
}

impl PackageGraph {
    /// Discover the workspace's packages and resolve the edges between them.
    pub fn build(cfg: &Config) -> Self {
        Self::from_packages(workspace::packages(cfg))
    }

    /// Build the graph over an already-discovered package set, re-reading each
    /// package's manifest for its declared dependency names.
    pub fn from_packages(packages: Vec<Package>) -> Self {
        let mut by_rel = HashMap::new();
        let mut by_name = HashMap::new();
        for (i, pkg) in packages.iter().enumerate() {
            by_rel.insert(pkg.rel.clone(), i);
            if let Some(name) = &pkg.name {
                // First declaration wins; `packages()` is ordered by path, so a
                // duplicate name resolves deterministically.
                by_name.entry(name.clone()).or_insert(i);
            }
        }

        let mut deps: Vec<Vec<usize>> = vec![Vec::new(); packages.len()];
        let mut dependents: Vec<Vec<usize>> = vec![Vec::new(); packages.len()];
        for (i, pkg) in packages.iter().enumerate() {
            let mut seen = HashSet::new();
            for declared in detect::manifest_deps(&pkg.path, pkg.eco) {
                let Some(&j) = by_name.get(&declared) else {
                    continue; // external dependency
                };
                // A package listing itself is not an edge, and would otherwise
                // make every walk look cyclic.
                if j != i && seen.insert(j) {
                    deps[i].push(j);
                    dependents[j].push(i);
                }
            }
        }

        PackageGraph {
            packages,
            deps,
            dependents,
            by_rel,
            by_name,
        }
    }

    /// Every package in the workspace, ordered by relative path.
    pub fn packages(&self) -> &[Package] {
        &self.packages
    }

    /// Look a package up the same way `packages` patterns match (SPEC §9.1):
    /// by relative path first, then by manifest name.
    pub fn index_of(&self, key: &str) -> Option<usize> {
        self.by_rel
            .get(key)
            .or_else(|| self.by_name.get(key))
            .copied()
    }

    pub fn get(&self, index: usize) -> &Package {
        &self.packages[index]
    }

    /// Direct upstream dependencies of `index` — what `^task` runs first.
    pub fn deps_of(&self, index: usize) -> &[usize] {
        &self.deps[index]
    }

    /// Direct downstream dependents of `index` — what a change to it affects.
    pub fn dependents_of(&self, index: usize) -> &[usize] {
        &self.dependents[index]
    }

    /// Every package reachable upstream from `roots`, **always excluding** the
    /// roots themselves — even where a cycle leads back to one. `^task` means
    /// "my dependencies", never "me".
    pub fn upstream_closure(&self, roots: &[usize]) -> Vec<usize> {
        self.closure(roots, &self.deps)
    }

    /// Every package reachable downstream from `roots`, **always excluding** the
    /// roots themselves. This is the affected set for a given change; callers
    /// that also want the changed packages add them back explicitly.
    pub fn downstream_closure(&self, roots: &[usize]) -> Vec<usize> {
        self.closure(roots, &self.dependents)
    }

    /// Breadth-first reachability over `edges`. Seeding `seen` with the roots
    /// both excludes them from the result and makes the walk cycle-safe.
    fn closure(&self, roots: &[usize], edges: &[Vec<usize>]) -> Vec<usize> {
        let mut seen: HashSet<usize> = roots.iter().copied().collect();
        let mut queue: Vec<usize> = roots.to_vec();
        let mut out = Vec::new();
        while let Some(i) = queue.pop() {
            for &next in &edges[i] {
                if seen.insert(next) {
                    out.push(next);
                    queue.push(next);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// All packages in dependency order — every package after the ones it
    /// depends on. Ties break by relative path, so the order is stable across
    /// runs and platforms.
    ///
    /// Errors (exit `64`) when the package graph contains a cycle, naming the
    /// packages involved: no build order exists, and silently picking one would
    /// produce a wrong `^task` schedule.
    pub fn topo_order(&self) -> Result<Vec<usize>> {
        let mut remaining: Vec<usize> = (0..self.packages.len()).collect();
        let mut done: HashSet<usize> = HashSet::new();
        let mut order = Vec::with_capacity(self.packages.len());

        while !remaining.is_empty() {
            // Ready = every dependency already emitted. `remaining` starts in
            // index order (i.e. by path) and stays sorted, so ties are stable.
            let (ready, blocked): (Vec<usize>, Vec<usize>) = remaining
                .iter()
                .partition(|&&i| self.deps[i].iter().all(|d| done.contains(d)));
            if ready.is_empty() {
                return Err(TsrError::runtime(format!(
                    "package dependency cycle among: {}",
                    self.rels(&blocked).join(", ")
                )));
            }
            done.extend(ready.iter().copied());
            order.extend(ready);
            remaining = blocked;
        }
        Ok(order)
    }

    /// Relative paths for a set of indices — for diagnostics.
    pub fn rels(&self, indices: &[usize]) -> Vec<&str> {
        indices
            .iter()
            .map(|&i| self.packages[i].rel.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::Ecosystem;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a temp workspace from `(relpath, marker_file, contents)` triples and
    /// return its package graph.
    fn graph(members: &[&str], pkgs: &[(&str, &str, &str)]) -> PackageGraph {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("tsr-pkggraph-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let members_toml = members
            .iter()
            .map(|m| format!("\"{m}\""))
            .collect::<Vec<_>>()
            .join(", ");
        let path = root.join("tasks.toml");
        fs::write(&path, format!("[workspace]\nmembers = [{members_toml}]\n")).unwrap();
        for (rel, marker, contents) in pkgs {
            let dir = root.join(rel);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(marker), contents).unwrap();
        }
        PackageGraph::build(&Config::load(&path).unwrap())
    }

    /// Relative paths of a package set, for readable assertions.
    fn rels(g: &PackageGraph, idx: &[usize]) -> Vec<String> {
        let mut v: Vec<String> = g.rels(idx).iter().map(|s| s.to_string()).collect();
        v.sort();
        v
    }

    fn pkg_json(name: &str, deps: &[&str]) -> String {
        let entries = deps
            .iter()
            .map(|d| format!("\"{d}\": \"workspace:*\""))
            .collect::<Vec<_>>()
            .join(", ");
        format!("{{\"name\": \"{name}\", \"dependencies\": {{{entries}}}}}")
    }

    #[test]
    fn links_workspace_deps_and_drops_external_ones() {
        let g = graph(
            &["apps/*", "packages/*"],
            &[
                (
                    "apps/web",
                    "package.json",
                    &pkg_json("@scope/web", &["@scope/ui", "react"]),
                ),
                ("packages/ui", "package.json", &pkg_json("@scope/ui", &[])),
            ],
        );
        let web = g.index_of("@scope/web").unwrap();
        let ui = g.index_of("packages/ui").unwrap();
        assert_eq!(g.deps_of(web), &[ui]);
        assert_eq!(g.dependents_of(ui), &[web]);
        // `react` matched no workspace package, so it produced no edge.
        assert_eq!(g.deps_of(ui), &[] as &[usize]);
    }

    #[test]
    fn indexes_by_path_and_by_name() {
        let g = graph(
            &["packages/*"],
            &[("packages/ui", "package.json", &pkg_json("@scope/ui", &[]))],
        );
        assert_eq!(g.index_of("packages/ui"), g.index_of("@scope/ui"));
        assert!(g.index_of("nope").is_none());
    }

    #[test]
    fn collects_every_npm_dependency_kind() {
        let g = graph(
            &["packages/*"],
            &[
                (
                    "packages/app",
                    "package.json",
                    r#"{"name": "app",
                        "dependencies": {"a": "*"},
                        "devDependencies": {"b": "*"},
                        "peerDependencies": {"c": "*"},
                        "optionalDependencies": {"d": "*"}}"#,
                ),
                ("packages/a", "package.json", &pkg_json("a", &[])),
                ("packages/b", "package.json", &pkg_json("b", &[])),
                ("packages/c", "package.json", &pkg_json("c", &[])),
                ("packages/d", "package.json", &pkg_json("d", &[])),
            ],
        );
        let app = g.index_of("app").unwrap();
        assert_eq!(
            rels(&g, g.deps_of(app)),
            vec!["packages/a", "packages/b", "packages/c", "packages/d"]
        );
    }

    #[test]
    fn links_cargo_path_and_renamed_deps() {
        let g = graph(
            &["crates/*"],
            &[
                (
                    "crates/app",
                    "Cargo.toml",
                    "[package]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n\
                     aliased = { package = \"helper\", path = \"../helper\" }\nserde = \"1\"\n",
                ),
                ("crates/core", "Cargo.toml", "[package]\nname = \"core\"\n"),
                (
                    "crates/helper",
                    "Cargo.toml",
                    "[package]\nname = \"helper\"\n",
                ),
            ],
        );
        let app = g.index_of("app").unwrap();
        assert_eq!(
            rels(&g, g.deps_of(app)),
            vec!["crates/core", "crates/helper"]
        );
    }

    #[test]
    fn links_go_require_block_and_replace() {
        let g = graph(
            &["services/*", "libs/*"],
            &[
                (
                    "services/api",
                    "go.mod",
                    "module example.com/api\n\ngo 1.22\n\n\
                     require (\n\texample.com/lib v0.1.0 // indirect\n\tgithub.com/ext/x v1.0.0\n)\n\n\
                     replace example.com/other => ../other\n",
                ),
                ("libs/lib", "go.mod", "module example.com/lib\n"),
                ("libs/other", "go.mod", "module example.com/other\n"),
            ],
        );
        let api = g.index_of("services/api").unwrap();
        assert_eq!(rels(&g, g.deps_of(api)), vec!["libs/lib", "libs/other"]);
    }

    #[test]
    fn links_python_pep621_and_poetry_deps() {
        let g = graph(
            &["pkgs/*"],
            &[
                (
                    "pkgs/app",
                    "pyproject.toml",
                    "[project]\nname = \"app\"\ndependencies = [\"core>=1.0\", \"requests\"]\n\
                     [project.optional-dependencies]\ntest = [\"harness[extra] ; python_version > '3'\"]\n",
                ),
                (
                    "pkgs/svc",
                    "pyproject.toml",
                    "[tool.poetry]\nname = \"svc\"\n[tool.poetry.dependencies]\ncore = \"^1.0\"\n",
                ),
                (
                    "pkgs/core",
                    "pyproject.toml",
                    "[project]\nname = \"core\"\n",
                ),
                (
                    "pkgs/harness",
                    "pyproject.toml",
                    "[project]\nname = \"harness\"\n",
                ),
            ],
        );
        let app = g.index_of("app").unwrap();
        assert_eq!(rels(&g, g.deps_of(app)), vec!["pkgs/core", "pkgs/harness"]);
        let svc = g.index_of("svc").unwrap();
        assert_eq!(rels(&g, g.deps_of(svc)), vec!["pkgs/core"]);
    }

    #[test]
    fn closures_are_transitive_in_both_directions() {
        // web → ui → tokens
        let g = graph(
            &["apps/*", "packages/*"],
            &[
                ("apps/web", "package.json", &pkg_json("web", &["ui"])),
                ("packages/ui", "package.json", &pkg_json("ui", &["tokens"])),
                ("packages/tokens", "package.json", &pkg_json("tokens", &[])),
            ],
        );
        let web = g.index_of("web").unwrap();
        let tokens = g.index_of("tokens").unwrap();
        assert_eq!(
            rels(&g, &g.upstream_closure(&[web])),
            vec!["packages/tokens", "packages/ui"]
        );
        assert_eq!(
            rels(&g, &g.downstream_closure(&[tokens])),
            vec!["apps/web", "packages/ui"]
        );
    }

    #[test]
    fn topo_order_puts_dependencies_first() {
        let g = graph(
            &["apps/*", "packages/*"],
            &[
                ("apps/web", "package.json", &pkg_json("web", &["ui"])),
                ("packages/ui", "package.json", &pkg_json("ui", &["tokens"])),
                ("packages/tokens", "package.json", &pkg_json("tokens", &[])),
            ],
        );
        let order = g.topo_order().unwrap();
        let names: Vec<&str> = g.rels(&order);
        assert_eq!(names, vec!["packages/tokens", "packages/ui", "apps/web"]);
    }

    #[test]
    fn cyclic_package_graph_builds_but_cannot_be_ordered() {
        let g = graph(
            &["packages/*"],
            &[
                ("packages/a", "package.json", &pkg_json("a", &["b"])),
                ("packages/b", "package.json", &pkg_json("b", &["a"])),
            ],
        );
        // Construction and traversal still work — only ordering is impossible.
        // The walk terminates, and `a` stays out of its own upstream set even
        // though the cycle leads back to it.
        let a = g.index_of("a").unwrap();
        assert_eq!(rels(&g, &g.upstream_closure(&[a])), vec!["packages/b"]);

        let err = g.topo_order().unwrap_err();
        assert!(err.to_string().contains("cycle"), "{err}");
        assert!(err.to_string().contains("packages/a"), "{err}");
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn self_dependency_is_not_an_edge() {
        let g = graph(
            &["packages/*"],
            &[("packages/a", "package.json", &pkg_json("a", &["a"]))],
        );
        let a = g.index_of("a").unwrap();
        assert_eq!(g.deps_of(a), &[] as &[usize]);
        assert_eq!(g.topo_order().unwrap(), vec![a]);
    }

    #[test]
    fn duplicate_declaration_yields_one_edge() {
        let g = graph(
            &["packages/*"],
            &[
                (
                    "packages/app",
                    "package.json",
                    r#"{"name": "app", "dependencies": {"lib": "*"}, "devDependencies": {"lib": "*"}}"#,
                ),
                ("packages/lib", "package.json", &pkg_json("lib", &[])),
            ],
        );
        assert_eq!(g.deps_of(g.index_of("app").unwrap()).len(), 1);
    }

    #[test]
    fn unnamed_and_malformed_manifests_are_inert() {
        let g = graph(
            &["packages/*"],
            &[
                ("packages/broken", "package.json", "{ not json"),
                ("packages/anon", "package.json", "{}"),
                ("packages/ok", "package.json", &pkg_json("ok", &[])),
            ],
        );
        assert_eq!(g.packages().len(), 3);
        assert!(g.topo_order().is_ok());
        assert_eq!(
            g.index_of("packages/broken").map(|i| g.deps_of(i).len()),
            Some(0)
        );
    }

    #[test]
    fn empty_workspace_is_valid() {
        let g = PackageGraph::from_packages(Vec::new());
        assert!(g.packages().is_empty());
        assert!(g.topo_order().unwrap().is_empty());
        assert!(g.upstream_closure(&[]).is_empty());
    }

    #[test]
    fn mixed_ecosystems_link_within_their_own_naming() {
        let g = graph(
            &["js/*", "rs/*"],
            &[
                ("js/web", "package.json", &pkg_json("web", &["ui"])),
                ("js/ui", "package.json", &pkg_json("ui", &[])),
                (
                    "rs/app",
                    "Cargo.toml",
                    "[package]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n",
                ),
                ("rs/core", "Cargo.toml", "[package]\nname = \"core\"\n"),
            ],
        );
        assert_eq!(g.packages().len(), 4);
        assert_eq!(
            rels(&g, g.deps_of(g.index_of("web").unwrap())),
            vec!["js/ui"]
        );
        assert_eq!(
            rels(&g, g.deps_of(g.index_of("app").unwrap())),
            vec!["rs/core"]
        );
        // Ecosystems do not cross-link: `ui` is not a dependency of `app`.
        assert_eq!(g.get(g.index_of("rs/core").unwrap()).eco, Ecosystem::Cargo);
    }
}
