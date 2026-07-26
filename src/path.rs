//! Turning paths into the text `tsr` shows and passes on.
//!
//! Paths are handled as [`Path`]/[`PathBuf`](std::path::PathBuf) everywhere
//! inside `tsr`, but some of them leave the process — a glob match becomes an
//! argv entry (SPEC §8.1). Those renderings are normalised here so a
//! `tasks.toml` behaves the same on every platform.
//!
//! This is the *only* place that rewrites a path into text with `/`. Diagnostics
//! are not covered: an error message naming a file should show the platform's
//! own spelling, so it stays on `Path::display`.

use std::path::{Component, Path};

/// Render `path` with `/` separators, whatever the platform uses natively.
///
/// The path is rebuilt from its [`Component`]s rather than having its
/// separators rewritten, so only a real separator becomes a `/` — a `\` that is
/// part of a *filename*, which is legal on Unix, survives intact. Redundant and
/// trailing separators are dropped along the way, since components carry no
/// separators of their own.
///
/// A root renders as the `/` it already is (`/a/b`, `C:/proj`); a Windows
/// verbatim prefix (`\\?\C:`) keeps its own backslashes, as it must to stay a
/// valid prefix.
pub fn to_slash(path: &Path) -> String {
    let mut out = String::new();
    for c in path.components() {
        match c {
            // The root is the separator, so it never needs one in front.
            Component::RootDir => out.push('/'),
            _ => {
                if !out.is_empty() && !out.ends_with('/') {
                    out.push('/');
                }
                out.push_str(&c.as_os_str().to_string_lossy());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn relative_paths_join_with_slashes() {
        assert_eq!(to_slash(&PathBuf::from("dist").join("a.js")), "dist/a.js");
        assert_eq!(to_slash(Path::new("one.js")), "one.js");
        assert_eq!(to_slash(Path::new("")), "");
    }

    #[test]
    fn a_root_is_not_doubled() {
        assert_eq!(to_slash(&PathBuf::from("/").join("a").join("b")), "/a/b");
        #[cfg(windows)]
        assert_eq!(to_slash(Path::new(r"C:\proj\a.js")), "C:/proj/a.js");
    }

    /// Only separators are rewritten — component names are copied verbatim.
    #[test]
    #[cfg(unix)]
    fn a_backslash_inside_a_unix_filename_is_kept() {
        assert_eq!(to_slash(Path::new("dir/we\\ird.js")), "dir/we\\ird.js");
    }

    #[test]
    fn redundant_separators_collapse() {
        assert_eq!(to_slash(Path::new("a//b/")), "a/b");
    }
}
