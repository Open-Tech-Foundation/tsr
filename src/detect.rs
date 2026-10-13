//! Ecosystem detection via marker files, the convention-based mapping from a
//! bare task name to its native runner (SPEC §3.1 form 3, §9), and the manifest
//! reads that back it: the package's declared **name** (v1) and its declared
//! **dependency names** (v1.1, the raw material for [`crate::pkggraph`]).

use std::path::Path;

/// A package ecosystem, identified by its marker manifest file (SPEC §9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ecosystem {
    /// `package.json` present, no other package manager's lockfile.
    Npm,
    /// `package.json` governed by a bun lockfile (`bun.lock` / `bun.lockb`).
    Bun,
    /// `package.json` governed by `pnpm-lock.yaml`.
    Pnpm,
    /// `package.json` governed by `yarn.lock`.
    Yarn,
    /// `Cargo.toml` present.
    Cargo,
    /// `go.mod` present.
    Go,
    /// `pyproject.toml` present.
    Python,
    /// `deno.json` / `deno.jsonc` present. Checked before `package.json`: Deno
    /// reads `package.json` too, and `deno task` falls back to its scripts.
    Deno,
    /// Only a `Makefile` (`makefile`, `GNUmakefile`). Checked last, so a Cargo
    /// or Go repo that also keeps a Makefile stays Cargo or Go.
    Make,
}

/// The file names `make` itself looks for, in its own order.
const MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// Deno's two config names.
const DENO_CONFIGS: [&str; 2] = ["deno.json", "deno.jsonc"];

impl Ecosystem {
    /// Map a bare task name to `(program, args)` for this ecosystem's native
    /// runner, convention-based (SPEC §3.1):
    ///
    /// - npm/bun/pnpm/yarn → `<runner> run <task>`
    /// - cargo   → `cargo <task>`   (`test`/`build`/`run` are native subcommands)
    /// - go      → `go <task>`, except `run` → `go run .` (a bare `go run`
    ///   names no package and always fails; `build`/`test` default to `.`)
    /// - deno    → `deno task <task>`
    /// - make    → `make <task>`
    /// - python  → `uv run <task>`, valid only for a [`python_target`]; the
    ///   caller rejects any other name before getting here
    pub fn native_command(self, task: &str) -> (String, Vec<String>) {
        match self {
            Ecosystem::Npm => ("npm".into(), vec!["run".into(), task.into()]),
            Ecosystem::Bun => ("bun".into(), vec!["run".into(), task.into()]),
            Ecosystem::Pnpm => ("pnpm".into(), vec!["run".into(), task.into()]),
            Ecosystem::Yarn => ("yarn".into(), vec!["run".into(), task.into()]),
            Ecosystem::Cargo => ("cargo".into(), vec![task.into()]),
            Ecosystem::Go if task == "run" => ("go".into(), vec!["run".into(), ".".into()]),
            Ecosystem::Go => ("go".into(), vec![task.into()]),
            Ecosystem::Python => ("uv".into(), vec!["run".into(), task.into()]),
            Ecosystem::Deno => ("deno".into(), vec!["task".into(), task.into()]),
            Ecosystem::Make => ("make".into(), vec![task.into()]),
        }
    }
}

/// Detect the ecosystem of the package rooted at `dir` by probing for marker
/// files, looking for a Node lockfile in `dir` alone. Use [`detect_within`]
/// when the package sits inside a workspace whose lockfile is at the root.
///
/// Returns `None` when no recognised marker is present.
pub fn detect(dir: &Path) -> Option<Ecosystem> {
    detect_within(dir, dir)
}

/// Detect the ecosystem of the package rooted at `dir`, a directory at or
/// below the workspace `root`. Node is checked first; which Node runner owns it
/// is decided by the nearest lockfile between `dir` and `root` inclusive, since
/// a pnpm/yarn/bun monorepo keeps one lockfile at the root, not one per member.
///
/// Returns `None` when no recognised marker is present.
pub fn detect_within(dir: &Path, root: &Path) -> Option<Ecosystem> {
    if DENO_CONFIGS.iter().any(|f| dir.join(f).is_file()) {
        return Some(Ecosystem::Deno);
    }
    if dir.join("package.json").is_file() {
        return Some(node_runner(dir, root));
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
    if MAKEFILES.iter().any(|f| dir.join(f).is_file()) {
        return Some(Ecosystem::Make);
    }
    None
}

/// The Node runner for a package at `dir`: the owner of the nearest lockfile
/// walking up to `root`, or npm when there is none.
///
/// The walk never leaves `root` (SPEC §12.3) — a stray `yarn.lock` in a home
/// directory must not switch the runner for every project beneath it. A `dir`
/// outside `root` is probed on its own. A `package-lock.json` is a lockfile
/// too: a member that keeps its own stays npm even in a pnpm workspace.
fn node_runner(dir: &Path, root: &Path) -> Ecosystem {
    const LOCKFILES: [(&str, Ecosystem); 5] = [
        ("bun.lock", Ecosystem::Bun),
        ("bun.lockb", Ecosystem::Bun),
        ("pnpm-lock.yaml", Ecosystem::Pnpm),
        ("yarn.lock", Ecosystem::Yarn),
        ("package-lock.json", Ecosystem::Npm),
    ];
    let bound = if dir.starts_with(root) { root } else { dir };
    for d in dir.ancestors() {
        for (file, eco) in LOCKFILES {
            if d.join(file).is_file() {
                return eco;
            }
        }
        if d == bound {
            break;
        }
    }
    Ecosystem::Npm
}

/// Whether `uv run <task>` can mean anything for the Python project at `dir`.
///
/// Python has no `<runner> run <script-name>` convention the way npm does, so a
/// bare `test` becomes `uv run test`, which looks for an executable called
/// `test` and fails without saying why. Only two kinds of name are real
/// targets: a `.py` file, and a console script the project itself declares in
/// `[project.scripts]` (or Poetry's `[tool.poetry.scripts]`), the nearest thing
/// Python has to a `package.json` script.
pub fn python_target(dir: &Path, task: &str) -> bool {
    if task.ends_with(".py") {
        return true;
    }
    let Some(doc) = toml_doc(&dir.join("pyproject.toml")) else {
        return false;
    };
    let declares = |table: Option<&toml_edit::Item>| {
        table
            .and_then(|i| i.get("scripts"))
            .and_then(|i| i.as_table_like())
            .is_some_and(|t| t.contains_key(task))
    };
    declares(doc.get("project")) || declares(doc.get("tool").and_then(|t| t.get("poetry")))
}

/// The named scripts a package declares, as `(name, what it runs)` — the
/// tasks configless mode can run by name. `None` for an ecosystem with no
/// script concept (Cargo, Go), where task names are the tool's own
/// subcommands instead.
///
/// Node: `package.json` `scripts`, alphabetical. Python: `[project.scripts]`
/// then `[tool.poetry.scripts]`, each mapped to its entry point. An unreadable
/// manifest declares nothing.
pub fn scripts(dir: &Path, eco: Ecosystem) -> Option<Vec<(String, String)>> {
    match eco {
        Ecosystem::Npm | Ecosystem::Bun | Ecosystem::Pnpm | Ecosystem::Yarn => {
            let doc = json_doc(&dir.join("package.json"));
            let scripts = doc
                .as_ref()
                .and_then(|d| d.get("scripts"))
                .and_then(serde_json::Value::as_object);
            Some(
                scripts
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_string()))
                    .collect(),
            )
        }
        Ecosystem::Python => {
            let mut out = Vec::new();
            if let Some(doc) = toml_doc(&dir.join("pyproject.toml")) {
                let tables = [
                    doc.get("project"),
                    doc.get("tool").and_then(|t| t.get("poetry")),
                ];
                for table in tables.into_iter().flatten() {
                    let Some(scripts) = table.get("scripts").and_then(|i| i.as_table_like()) else {
                        continue;
                    };
                    for (k, v) in scripts.iter() {
                        out.push((k.to_string(), v.as_str().unwrap_or_default().to_string()));
                    }
                }
            }
            Some(out)
        }
        Ecosystem::Deno => {
            // A task is a command string, or `{ command, description, … }`.
            let doc = deno_doc(dir);
            let tasks = doc
                .as_ref()
                .and_then(|d| d.get("tasks"))
                .and_then(serde_json::Value::as_object);
            Some(
                tasks
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        let text = v
                            .as_str()
                            .or_else(|| v.get("description").and_then(|d| d.as_str()))
                            .or_else(|| v.get("command").and_then(|d| d.as_str()))
                            .unwrap_or_default();
                        (k.clone(), text.to_string())
                    })
                    .collect(),
            )
        }
        Ecosystem::Make => Some(make_targets(dir)),
        Ecosystem::Cargo | Ecosystem::Go => None,
    }
}

/// Parse the package's `deno.json`, or its `deno.jsonc` with comments and
/// trailing commas stripped. `None` when neither reads as JSON.
fn deno_doc(dir: &Path) -> Option<serde_json::Value> {
    if let Some(doc) = json_doc(&dir.join("deno.json")) {
        return Some(doc);
    }
    let text = std::fs::read_to_string(dir.join("deno.jsonc")).ok()?;
    serde_json::from_str(&strip_jsonc(&text)).ok()
}

/// Reduce JSONC to JSON: drop `//` and `/* */` comments and trailing commas,
/// leaving string contents alone.
fn strip_jsonc(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let mut in_string = false;
    while i < chars.len() {
        let c = chars[i];
        if in_string {
            out.push(c);
            if c == '\\' && i + 1 < chars.len() {
                out.push(chars[i + 1]);
                i += 2;
                continue;
            }
            if c == '"' {
                in_string = false;
            }
            i += 1;
            continue;
        }
        match (c, chars.get(i + 1)) {
            ('"', _) => {
                in_string = true;
                out.push(c);
                i += 1;
            }
            ('/', Some('/')) => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
            }
            (',', _) => {
                // A comma followed only by whitespace/comments and a closer.
                let mut j = i + 1;
                while j < chars.len() && chars[j].is_whitespace() {
                    j += 1;
                }
                if !matches!(chars.get(j), Some('}' | ']')) {
                    out.push(c);
                }
                i += 1;
            }
            _ => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

/// The targets a Makefile declares that `tsr` can name: explicit rules whose
/// target is a legal task name (SPEC §4.1). Special targets (`.PHONY`),
/// pattern rules (`%.o`), variable assignments (`X := y`) and recipe lines are
/// skipped. A `## text` comment after the prerequisites — the common `make
/// help` convention — becomes the target's description.
fn make_targets(dir: &Path) -> Vec<(String, String)> {
    let Some(text) = MAKEFILES
        .iter()
        .find_map(|f| std::fs::read_to_string(dir.join(f)).ok())
    else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with(['\t', ' ', '#', '.']) {
            continue;
        }
        let Some((head, rest)) = line.split_once(':') else {
            continue;
        };
        // `X := y`, `X ::= y` are assignments, not rules.
        if rest.starts_with('=') || rest.starts_with(":=") || head.contains('=') {
            continue;
        }
        let desc = rest
            .split_once("##")
            .map(|(_, d)| d.trim().to_string())
            .unwrap_or_default();
        for target in head.split_whitespace() {
            let legal = target
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'));
            if legal && !out.iter().any(|(t, _)| t == target) {
                out.push((target.to_string(), desc.clone()));
            }
        }
    }
    out
}

/// Read the package's manifest name, so `packages` can match against declared
/// names (e.g. `@scope/pkg`), not only path globs (SPEC §9.1). Returns `None`
/// when the manifest is unreadable or declares no name.
pub fn manifest_name(dir: &Path, eco: Ecosystem) -> Option<String> {
    match eco {
        Ecosystem::Npm | Ecosystem::Bun | Ecosystem::Pnpm | Ecosystem::Yarn => {
            json_name(&dir.join("package.json"))
        }
        Ecosystem::Cargo => toml_name(&dir.join("Cargo.toml"), &["package"]),
        Ecosystem::Python => toml_name(&dir.join("pyproject.toml"), &["project"])
            .or_else(|| toml_name(&dir.join("pyproject.toml"), &["tool", "poetry"])),
        Ecosystem::Go => go_module(&dir.join("go.mod")),
        Ecosystem::Deno => deno_doc(dir)?.get("name")?.as_str().map(str::to_string),
        // A Makefile declares no package name.
        Ecosystem::Make => None,
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
        Ecosystem::Npm | Ecosystem::Bun | Ecosystem::Pnpm | Ecosystem::Yarn => {
            json_deps(&dir.join("package.json"))
        }
        Ecosystem::Cargo => cargo_deps(&dir.join("Cargo.toml")),
        Ecosystem::Python => python_deps(&dir.join("pyproject.toml")),
        Ecosystem::Go => go_deps(&dir.join("go.mod")),
        // Deno names dependencies by specifier (`jsr:@scope/pkg`), and a
        // Makefile names none; neither yields workspace edges yet.
        Ecosystem::Deno | Ecosystem::Make => Vec::new(),
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
    let Some(doc) = toml_doc(path) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut inherited = Vec::new();
    push_cargo_dep_tables(doc.as_item(), &mut out, &mut inherited);

    // `[target.'cfg(windows)'.dependencies]` and friends. A sibling crate
    // declared only for some platforms is still a workspace edge; missing it
    // would silently produce the wrong build order rather than an error.
    if let Some(targets) = doc.get("target").and_then(|i| i.as_table_like()) {
        for (_, cfg) in targets.iter() {
            push_cargo_dep_tables(cfg, &mut out, &mut inherited);
        }
    }

    // `dep = { workspace = true }` inherits from the workspace root, which is
    // also where a `package = "…"` rename would live — so the member's key can
    // differ from the real crate name. Resolve those against the root, falling
    // back to the key when there is nothing to resolve against.
    if !inherited.is_empty() {
        let renames = cargo_workspace_dep_names(path);
        for key in inherited {
            out.push(renames.get(&key).cloned().unwrap_or(key));
        }
    }
    out
}

/// Push the dependency names from a container's three dependency tables. The
/// container is either the manifest root or one `[target.<cfg>]` table.
/// Entries deferring to the workspace are collected into `inherited` instead,
/// since resolving them needs the workspace root manifest.
fn push_cargo_dep_tables(
    container: &toml_edit::Item,
    out: &mut Vec<String>,
    inherited: &mut Vec<String>,
) {
    const KINDS: [&str; 3] = ["dependencies", "dev-dependencies", "build-dependencies"];
    for kind in KINDS {
        let Some(table) = container.get(kind).and_then(|i| i.as_table_like()) else {
            continue;
        };
        for (key, item) in table.iter() {
            match item.get("package").and_then(|p| p.as_str()) {
                // An explicit rename on the member always wins.
                Some(name) => out.push(name.to_string()),
                None if item.get("workspace").and_then(|w| w.as_bool()) == Some(true) => {
                    inherited.push(key.to_string());
                }
                None => out.push(key.to_string()),
            }
        }
    }
}

/// Map each `[workspace.dependencies]` key to the crate name it really refers
/// to, found by walking up from a member manifest to the nearest `Cargo.toml`
/// carrying a `[workspace]` table — the same search Cargo itself performs.
///
/// Only keys that are *renamed* need to appear; everything else resolves to
/// itself, so the common case costs one file read and no allocations beyond it.
fn cargo_workspace_dep_names(member: &Path) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let mut dir = member.parent();
    while let Some(d) = dir {
        let manifest = d.join("Cargo.toml");
        if let Some(doc) = toml_doc(&manifest)
            && let Some(ws) = doc.get("workspace").and_then(|i| i.as_table_like())
        {
            if let Some(deps) = ws.get("dependencies").and_then(|i| i.as_table_like()) {
                for (key, item) in deps.iter() {
                    if let Some(real) = item.get("package").and_then(|p| p.as_str()) {
                        names.insert(key.to_string(), real.to_string());
                    }
                }
            }
            break; // the nearest workspace root is the one that applies
        }
        dir = d.parent();
    }
    names
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
    fn detects_pnpm_and_yarn_by_lockfile() {
        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        fs::write(d.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Pnpm));

        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        fs::write(d.join("yarn.lock"), "").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Yarn));

        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        fs::write(d.join("package-lock.json"), "{}").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Npm));
    }

    #[test]
    fn member_inherits_the_workspace_root_lockfile() {
        let root = scratch();
        fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        let web = root.join("apps/web");
        fs::create_dir_all(&web).unwrap();
        fs::write(web.join("package.json"), "{}").unwrap();
        assert_eq!(detect_within(&web, &root), Some(Ecosystem::Pnpm));
        // Probed alone, the member has no lockfile of its own.
        assert_eq!(detect(&web), Some(Ecosystem::Npm));

        // The nearest lockfile wins: a member's own package-lock keeps it npm.
        fs::write(web.join("package-lock.json"), "{}").unwrap();
        assert_eq!(detect_within(&web, &root), Some(Ecosystem::Npm));
    }

    #[test]
    fn lockfile_walk_stops_at_the_root() {
        // A lockfile *above* the workspace root is ignored.
        let outer = scratch();
        fs::write(outer.join("yarn.lock"), "").unwrap();
        let root = outer.join("ws");
        let pkg = root.join("pkg");
        fs::create_dir_all(&pkg).unwrap();
        fs::write(pkg.join("package.json"), "{}").unwrap();
        assert_eq!(detect_within(&pkg, &root), Some(Ecosystem::Npm));
        // A dir outside the root is probed on its own, never walked.
        let elsewhere = outer.join("other");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::write(elsewhere.join("package.json"), "{}").unwrap();
        assert_eq!(detect_within(&elsewhere, &root), Some(Ecosystem::Npm));
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

    /// A sibling crate declared only for some platform is still a workspace
    /// edge; missing it would silently produce the wrong build order.
    #[test]
    fn reads_cargo_target_specific_dependencies() {
        let d = scratch();
        fs::write(
            d.join("Cargo.toml"),
            "[package]\nname = \"app\"\n\
             [dependencies]\ncore = { path = \"../core\" }\n\
             [target.'cfg(windows)'.dependencies]\nwinlib = { path = \"../winlib\" }\n\
             [target.'cfg(unix)'.dev-dependencies]\nunixlib = { path = \"../unixlib\" }\n\
             [target.'cfg(unix)'.dependencies]\naliased = { package = \"real\", path = \"../real\" }\n",
        )
        .unwrap();
        let deps = manifest_deps(&d, Ecosystem::Cargo);
        for expected in ["core", "winlib", "unixlib", "real"] {
            assert!(
                deps.contains(&expected.to_string()),
                "{expected} missing: {deps:?}"
            );
        }
    }

    /// `dep = { workspace = true }` inherits from the workspace root — including
    /// a `package = "…"` rename declared there, which makes the member's key
    /// differ from the real crate name.
    #[test]
    fn resolves_cargo_workspace_inherited_dependencies() {
        let root = scratch();
        fs::write(
            root.join("Cargo.toml"),
            "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\n\
             mylib = { path = \"crates/mylib\" }\n\
             alias = { package = \"real\", path = \"crates/real\" }\n",
        )
        .unwrap();
        let member = root.join("crates/app");
        fs::create_dir_all(&member).unwrap();
        fs::write(
            member.join("Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\n\
             mylib = { workspace = true }\nalias = { workspace = true }\nserde = \"1\"\n",
        )
        .unwrap();

        let deps = manifest_deps(&member, Ecosystem::Cargo);
        // Un-renamed inheritance keeps its key…
        assert!(deps.contains(&"mylib".to_string()), "{deps:?}");
        // …and a renamed one resolves to the real crate name, not the alias.
        assert!(deps.contains(&"real".to_string()), "{deps:?}");
        assert!(!deps.contains(&"alias".to_string()), "{deps:?}");
    }

    /// With no workspace root to inherit from, the key is the best answer
    /// available — never a panic and never a dropped dependency.
    #[test]
    fn inherited_dependency_without_a_workspace_root_falls_back_to_its_key() {
        let d = scratch();
        fs::write(
            d.join("Cargo.toml"),
            "[package]\nname = \"app\"\n[dependencies]\nmylib = { workspace = true }\n",
        )
        .unwrap();
        assert_eq!(manifest_deps(&d, Ecosystem::Cargo), vec!["mylib"]);
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
    fn python_targets_are_py_files_and_declared_scripts() {
        let d = scratch();
        fs::write(
            d.join("pyproject.toml"),
            "[project]\nname = \"p\"\n[project.scripts]\nserve = \"p.app:main\"\n\
             [tool.poetry.scripts]\nmigrate = \"p.db:migrate\"\n",
        )
        .unwrap();
        assert!(python_target(&d, "hello.py"));
        assert!(python_target(&d, "scripts/hello.py"));
        assert!(python_target(&d, "serve"));
        assert!(python_target(&d, "migrate"));
        // `test` is not declared anywhere, so `uv run test` would mean nothing.
        assert!(!python_target(&d, "test"));
        // No pyproject (or a malformed one) declares nothing.
        assert!(!python_target(&scratch(), "serve"));
    }

    #[test]
    fn lists_declared_scripts() {
        let d = scratch();
        fs::write(
            d.join("package.json"),
            r#"{"scripts":{"dev":"vite","build":"vite build"}}"#,
        )
        .unwrap();
        assert_eq!(
            scripts(&d, Ecosystem::Npm).unwrap(),
            [
                ("build".to_string(), "vite build".to_string()),
                ("dev".to_string(), "vite".to_string())
            ]
        );

        let d = scratch();
        fs::write(
            d.join("pyproject.toml"),
            "[project.scripts]\nserve = \"p:main\"\n[tool.poetry.scripts]\nmigrate = \"p.db:run\"\n",
        )
        .unwrap();
        let names: Vec<String> = scripts(&d, Ecosystem::Python)
            .unwrap()
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        assert_eq!(names, ["serve", "migrate"]);

        // No scripts, or no manifest at all, is an empty list; Cargo/Go have
        // no script concept.
        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        assert_eq!(scripts(&d, Ecosystem::Npm), Some(Vec::new()));
        assert_eq!(scripts(&d, Ecosystem::Cargo), None);
    }

    #[test]
    fn detects_deno_before_node_and_make_last() {
        let d = scratch();
        fs::write(d.join("package.json"), "{}").unwrap();
        fs::write(d.join("deno.jsonc"), "{}").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Deno));

        // A Makefile alone is Make; beside another marker it defers to it.
        let d = scratch();
        fs::write(d.join("Makefile"), "all:\n").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Make));
        fs::write(d.join("go.mod"), "module m\n").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Go));
        let d = scratch();
        fs::write(d.join("GNUmakefile"), "all:\n").unwrap();
        assert_eq!(detect(&d), Some(Ecosystem::Make));
    }

    #[test]
    fn reads_deno_tasks_and_name_from_jsonc() {
        let d = scratch();
        fs::write(
            d.join("deno.jsonc"),
            r#"{
              // the package
              "name": "@acme/app", /* inline */
              "tasks": {
                "dev": "deno run -A main.ts",
                "test": { "command": "deno test", "description": "Run the tests" },
                "url": "echo http://x//y",
              },
            }"#,
        )
        .unwrap();
        assert_eq!(
            manifest_name(&d, Ecosystem::Deno).as_deref(),
            Some("@acme/app")
        );
        assert_eq!(
            scripts(&d, Ecosystem::Deno).unwrap(),
            [
                ("dev".to_string(), "deno run -A main.ts".to_string()),
                ("test".to_string(), "Run the tests".to_string()),
                // `//` inside a string is not a comment.
                ("url".to_string(), "echo http://x//y".to_string()),
            ]
        );
    }

    #[test]
    fn reads_make_targets_skipping_specials_and_assignments() {
        let d = scratch();
        fs::write(
            d.join("Makefile"),
            "CC := gcc\nVERSION = 1\nexport X ?= y\n.PHONY: build test\n\
             build: deps ## Build everything\n\tgo build ./...\n\
             test lint:\n\t@echo hi\n%.o: %.c\n\t$(CC) -c $<\n\
             out/app.bin: build\n# comment: not a rule\nbuild:\n",
        )
        .unwrap();
        assert_eq!(
            scripts(&d, Ecosystem::Make).unwrap(),
            [
                ("build".to_string(), "Build everything".to_string()),
                ("test".to_string(), String::new()),
                ("lint".to_string(), String::new()),
            ]
        );
        assert_eq!(manifest_name(&d, Ecosystem::Make), None);
    }

    #[test]
    fn native_command_conventions() {
        assert_eq!(
            Ecosystem::Npm.native_command("test"),
            ("npm".into(), vec!["run".into(), "test".into()])
        );
        assert_eq!(
            Ecosystem::Pnpm.native_command("dev"),
            ("pnpm".into(), vec!["run".into(), "dev".into()])
        );
        assert_eq!(
            Ecosystem::Yarn.native_command("dev"),
            ("yarn".into(), vec!["run".into(), "dev".into()])
        );
        assert_eq!(
            Ecosystem::Deno.native_command("dev"),
            ("deno".into(), vec!["task".into(), "dev".into()])
        );
        assert_eq!(
            Ecosystem::Make.native_command("build"),
            ("make".into(), vec!["build".into()])
        );
        assert_eq!(
            Ecosystem::Cargo.native_command("test"),
            ("cargo".into(), vec!["test".into()])
        );
        assert_eq!(
            Ecosystem::Go.native_command("build"),
            ("go".into(), vec!["build".into()])
        );
        // `go run` needs a package argument; `build`/`test` do not.
        assert_eq!(
            Ecosystem::Go.native_command("run"),
            ("go".into(), vec!["run".into(), ".".into()])
        );
        assert_eq!(
            Ecosystem::Python.native_command("lint"),
            ("uv".into(), vec!["run".into(), "lint".into()])
        );
    }
}
