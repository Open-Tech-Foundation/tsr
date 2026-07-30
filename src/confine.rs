//! The workspace boundary (SPEC §12).
//!
//! `tsr` cannot sandbox the programs it spawns — a task runner that stopped
//! `cargo` from writing outside the repo would stop being a task runner. What it
//! *can* do is confine the file operations it performs **itself**: the in-process
//! builtins (SPEC §8.5), and the directories a config points it at.
//!
//! That distinction matters because builtins have no process boundary of their
//! own. `rm -rf dist` is not a `/bin/rm` that can be audited, denied by a
//! sandbox, or left off `PATH` — it is `tsr`, always preferred over any binary of
//! the same name, doing the deletion. So the one guard that can exist is the one
//! here: an operand that resolves outside the workspace is refused.
//!
//! Resolution is **physical**, not textual. `canonicalize` runs over the longest
//! existing prefix of a path, so a symlink pointing out of the tree is caught by
//! where it leads rather than by how it is spelled; only the non-existent tail
//! (a file about to be created) is joined lexically, where there are no symlinks
//! left to resolve.
//!
//! The escape hatch is `[security] allow_paths`, for the genuine cases — a build
//! that writes to a shared cache outside the repo. It lives in the config, which
//! means it defends against *accidents*, not against a hostile `tasks.toml` that
//! could simply widen it. Guards a config must not be able to relax are the
//! env ones (SPEC §12.2), which only a CLI flag can lift.

use std::ffi::OsString;
use std::path::{Component, Path, PathBuf};

use crate::error::{Result, TsrError};

/// The set of directories a run may touch.
#[derive(Debug, Clone)]
pub struct Bounds {
    /// The workspace root, physically resolved.
    root: PathBuf,
    /// Extra roots from `[security] allow_paths`, physically resolved.
    allowed: Vec<PathBuf>,
}

impl Bounds {
    /// Build the boundary for a workspace rooted at `root`, widened by the
    /// `[security] allow_paths` entries in `allow` (relative ones resolve
    /// against `root`).
    pub fn new(root: &Path, allow: &[String]) -> Bounds {
        Bounds {
            root: resolve(root, "."),
            allowed: allow.iter().map(|p| resolve(root, p)).collect(),
        }
    }

    /// A boundary that permits everything, for the tests that assert on a
    /// builtin's behaviour rather than on its confinement. Deliberately not
    /// available to the running binary: there is no supported way to switch the
    /// guard off wholesale, only to widen it (SPEC §12.1).
    #[cfg(test)]
    pub fn unbounded() -> Bounds {
        Bounds {
            root: PathBuf::new(),
            allowed: vec![PathBuf::new()],
        }
    }

    /// Whether `path`, already resolved by [`resolve`], is inside the boundary.
    fn permits(&self, path: &Path) -> bool {
        std::iter::once(&self.root)
            .chain(&self.allowed)
            // An empty root is the unbounded case: every path is under "".
            .any(|base| base.as_os_str().is_empty() || path.starts_with(base))
    }

    /// Resolve `arg` against `base` and confirm it stays inside the boundary.
    ///
    /// `what` names the thing being checked (`"rm"`, `"task 'build': dir"`) and
    /// leads the error message, which always points at the escape hatch — a
    /// refusal the user cannot act on is just an obstacle.
    pub fn check(&self, what: &str, base: &Path, arg: &str) -> Result<PathBuf> {
        let path = resolve(base, arg);
        if self.permits(&path) {
            return Ok(path);
        }
        Err(TsrError::config(format!(
            "{what}: '{arg}' resolves to '{}', outside the workspace at '{}' — \
             add it to `[security] allow_paths` if that is intended",
            path.display(),
            self.root.display()
        )))
    }

    /// The boundary check for a builtin operand, which reports through an exit
    /// code rather than an error type (SPEC §8.5).
    pub fn permits_operand(&self, base: &Path, arg: &str) -> bool {
        self.permits(&resolve(base, arg))
    }

    /// The workspace root, physically resolved.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Resolve `arg` against `base` to an absolute path, following symlinks as far
/// as the filesystem actually goes.
///
/// [`std::fs::canonicalize`] fails outright on a path that does not exist yet,
/// which is most of what a task writes (`mkdir -p dist/assets`, `touch
/// build/stamp`). So the longest existing prefix is canonicalized — that is the
/// part where symlinks live — and the remaining components are appended
/// lexically, with `..` popping, since a component that does not exist cannot be
/// a symlink to somewhere else.
pub fn resolve(base: &Path, arg: &str) -> PathBuf {
    let joined = if Path::new(arg).is_absolute() {
        PathBuf::from(arg)
    } else {
        base.join(arg)
    };

    let mut tail: Vec<OsString> = Vec::new();
    let mut cur = joined.clone();
    loop {
        if let Ok(real) = cur.canonicalize() {
            return append(real, &tail);
        }
        // Nothing on this path exists (or it cannot be read): resolve what is
        // left textually. Absolute already, since `base` is.
        let Some(name) = cur.file_name().map(OsString::from) else {
            return lexical(&joined);
        };
        tail.push(name);
        match cur.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => cur = parent.to_path_buf(),
            _ => return lexical(&joined),
        }
    }
}

/// Re-attach the components stripped while searching for an existing prefix.
fn append(mut base: PathBuf, tail: &[OsString]) -> PathBuf {
    for name in tail.iter().rev() {
        if name == ".." {
            base.pop();
        } else if name != "." {
            base.push(name);
        }
    }
    base
}

/// Purely textual normalisation, for a path with no existing prefix at all.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-confine-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // The temp dir is itself a symlink on macOS, so resolve the root the
        // same way the checks do.
        dir.canonicalize().unwrap()
    }

    fn bounds(root: &Path) -> Bounds {
        Bounds::new(root, &[])
    }

    #[test]
    fn permits_paths_inside_the_workspace() {
        let root = scratch();
        let b = bounds(&root);
        for arg in ["dist", "dist/assets/app.js", "./a/../b", "."] {
            assert!(
                b.check("rm", &root, arg).is_ok(),
                "'{arg}' should be inside the workspace"
            );
        }
    }

    #[test]
    fn refuses_paths_above_the_workspace() {
        let root = scratch();
        let b = bounds(&root);
        for arg in ["..", "../sibling", "../../etc/passwd", "dist/../.."] {
            let err = b.check("rm", &root, arg).unwrap_err().to_string();
            assert!(err.contains("outside the workspace"), "'{arg}': {err}");
            assert!(err.contains("allow_paths"), "should name the escape hatch");
        }
    }

    #[test]
    fn refuses_an_absolute_path_outside() {
        let root = scratch();
        let other = scratch();
        let b = bounds(&root);
        assert!(b.check("rm", &root, other.to_str().unwrap()).is_err());
        // An absolute path that happens to be inside is still fine.
        let inside = root.join("dist");
        assert!(b.check("rm", &root, inside.to_str().unwrap()).is_ok());
    }

    /// The check is physical: a link is judged by where it lands, not by the
    /// fact that its *name* sits inside the workspace.
    #[test]
    #[cfg(unix)]
    fn follows_a_symlink_out_of_the_workspace() {
        let root = scratch();
        let other = scratch();
        fs::write(other.join("secret.txt"), "s").unwrap();
        std::os::unix::fs::symlink(&other, root.join("escape")).unwrap();

        let b = bounds(&root);
        assert!(b.check("rm", &root, "escape/secret.txt").is_err());
        assert!(b.check("rm", &root, "escape").is_err());
    }

    #[test]
    fn allow_paths_widens_the_boundary() {
        let root = scratch();
        let cache = scratch();
        let b = Bounds::new(&root, &[cache.to_str().unwrap().to_string()]);
        assert!(
            b.check("cp", &root, &format!("{}/out", cache.display()))
                .is_ok()
        );
        // Only the listed path, not its parent or its siblings.
        let elsewhere = scratch();
        assert!(b.check("cp", &root, elsewhere.to_str().unwrap()).is_err());
    }

    #[test]
    fn unbounded_permits_anything() {
        let root = scratch();
        let b = Bounds::unbounded();
        assert!(b.check("rm", &root, "../../../etc").is_ok());
    }

    /// A path whose tail does not exist yet is still resolved — that is the
    /// common case for anything a task is about to create.
    #[test]
    fn resolves_a_path_that_does_not_exist_yet() {
        let root = scratch();
        let b = bounds(&root);
        assert!(b.check("mkdir", &root, "build/deep/nested").is_ok());
        assert!(b.check("mkdir", &root, "build/../../escape").is_err());
    }
}
