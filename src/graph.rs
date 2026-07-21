//! Dependency-graph validation (SPEC §5). The `deps` edges form a DAG; before
//! execution we verify every referenced task exists and that there are no
//! cycles. Both failures are runner-level errors (exit `64`).
//!
//! Execution order itself is handled by the recursive executor (a task runs its
//! `deps` before itself); this module only guarantees that recursion terminates.

use std::collections::HashSet;

use crate::config::Config;
use crate::error::{Result, TsrError};

/// Validate the dependency subgraph reachable from `root`: every dep must name a
/// defined task, and the graph must be acyclic.
pub fn validate(cfg: &Config, root: &str) -> Result<()> {
    if cfg.task(root).is_none() {
        return Err(TsrError::runtime(format!("unknown task '{root}'")));
    }
    let mut on_stack: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    dfs(cfg, root, &mut on_stack, &mut visited)
}

/// All task keys reachable from `root` through `deps`, including `root` itself.
/// Assumes the subgraph has already been [`validate`]d (defined, acyclic).
pub fn reachable(cfg: &Config, root: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut stack = vec![root.to_string()];
    while let Some(key) = stack.pop() {
        if !seen.insert(key.clone()) {
            continue;
        }
        order.push(key.clone());
        if let Some(task) = cfg.task(&key) {
            for dep in &task.deps {
                stack.push(dep.clone());
            }
        }
    }
    order
}

fn dfs(
    cfg: &Config,
    key: &str,
    on_stack: &mut Vec<String>,
    visited: &mut HashSet<String>,
) -> Result<()> {
    if on_stack.iter().any(|k| k == key) {
        on_stack.push(key.to_string());
        return Err(TsrError::config(format!(
            "dependency cycle: {}",
            on_stack.join(" → ")
        )));
    }
    if visited.contains(key) {
        return Ok(());
    }

    let task = cfg.task(key).ok_or_else(|| {
        TsrError::config(format!("task depends on unknown task '{key}'"))
    })?;

    on_stack.push(key.to_string());
    for dep in &task.deps {
        dfs(cfg, dep, on_stack, visited)?;
    }
    on_stack.pop();
    visited.insert(key.to_string());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn cfg(text: &str) -> Config {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-graph-{}-{id}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join("tasks.toml");
        std::fs::write(&path, text).unwrap();
        Config::load(&path).unwrap()
    }

    #[test]
    fn accepts_acyclic_graph() {
        let c = cfg(
            "[tasks.ci]\ndeps=[\"lint\",\"test\"]\n[tasks.lint]\nrun=\"l\"\n[tasks.test]\nrun=\"t\"\n",
        );
        assert!(validate(&c, "ci").is_ok());
    }

    #[test]
    fn accepts_diamond() {
        let c = cfg(
            "[tasks.top]\ndeps=[\"a\",\"b\"]\n[tasks.a]\ndeps=[\"base\"]\n[tasks.b]\ndeps=[\"base\"]\n[tasks.base]\nrun=\"x\"\n",
        );
        assert!(validate(&c, "top").is_ok());
    }

    #[test]
    fn detects_cycle() {
        let c = cfg("[tasks.a]\ndeps=[\"b\"]\n[tasks.b]\ndeps=[\"a\"]\n");
        let err = validate(&c, "a").unwrap_err();
        assert!(err.to_string().contains("cycle"));
        assert_eq!(err.exit_code(), 64);
    }

    #[test]
    fn detects_self_cycle() {
        let c = cfg("[tasks.a]\ndeps=[\"a\"]\n");
        assert!(validate(&c, "a").unwrap_err().to_string().contains("cycle"));
    }

    #[test]
    fn unknown_root_task_is_error() {
        let c = cfg("[tasks.a]\nrun=\"x\"\n");
        let err = validate(&c, "nope").unwrap_err();
        assert!(err.to_string().contains("unknown task"));
    }

    #[test]
    fn unknown_dep_is_error() {
        let c = cfg("[tasks.a]\ndeps=[\"ghost\"]\n");
        assert!(validate(&c, "a").unwrap_err().to_string().contains("ghost"));
    }
}
