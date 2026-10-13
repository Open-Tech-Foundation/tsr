//! Readers for the ecosystems beyond the core five (SPEC §3.1, §9): the JVM
//! (Gradle, Maven, sbt), Ruby, PHP, .NET, Dart/Flutter, Elixir, Swift, Zig,
//! Haskell, Terraform and Docker Compose — plus the build systems and
//! languages that are recognised but have no task-name convention to map onto.
//!
//! [`crate::detect`] owns the [`Ecosystem`](crate::detect::Ecosystem) enum and
//! the dispatch; this module holds the per-ecosystem file reading, kept apart
//! so that file stays about the mapping rather than about fifteen formats.
//!
//! Every reader is total: an unreadable or unusual manifest yields `None` or
//! nothing, never an error. The native tool reports a broken manifest far better
//! than a best-effort reader here could.

use std::path::{Path, PathBuf};

/// Gradle's build and settings scripts, Groovy and Kotlin.
pub const GRADLE_FILES: [&str; 4] = [
    "build.gradle",
    "build.gradle.kts",
    "settings.gradle",
    "settings.gradle.kts",
];

/// The names `rake` looks for.
pub const RAKEFILES: [&str; 3] = ["Rakefile", "rakefile", "Rakefile.rb"];

/// Docker Compose's file names, in its own lookup order.
pub const COMPOSE_FILES: [&str; 4] = [
    "compose.yaml",
    "compose.yml",
    "docker-compose.yaml",
    "docker-compose.yml",
];

/// .NET solution and project file extensions.
pub const DOTNET_EXTS: [&str; 4] = ["sln", "csproj", "fsproj", "vbproj"];

/// Whether `dir` holds any of `names`.
pub fn has_any(dir: &Path, names: &[&str]) -> bool {
    names.iter().any(|f| dir.join(f).is_file())
}

/// The first file in `dir` (by name, for determinism) with one of `exts`.
pub fn first_with_ext(dir: &Path, exts: &[&str]) -> Option<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter(|p| {
            p.extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e))
        })
        .collect();
    found.sort();
    found.into_iter().next()
}

/// A project-local build wrapper (`gradlew`, `mvnw`): the nearest one walking
/// up from `dir` to `root` inclusive, never past it (SPEC §12.3). A
/// multi-project Gradle build keeps one wrapper at its root, which is the one
/// every subproject is meant to use — the same reasoning as a Node lockfile.
///
/// On Windows the wrapper is the `.cmd` / `.bat` script beside it.
pub fn wrapper(dir: &Path, root: &Path, name: &str) -> Option<PathBuf> {
    let candidates: Vec<String> = if cfg!(windows) {
        vec![format!("{name}.bat"), format!("{name}.cmd")]
    } else {
        vec![name.to_string()]
    };
    let bound = if dir.starts_with(root) { root } else { dir };
    for d in dir.ancestors() {
        for c in &candidates {
            let p = d.join(c);
            if p.is_file() {
                return Some(p);
            }
        }
        if d == bound {
            break;
        }
    }
    None
}

/// Read a text file, or `None`.
fn read(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// The text of the first `"…"` string literal after `key` on its line, e.g.
/// `name: "Foo"` → `Foo`. Single quotes count too.
fn quoted_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = &line[line.find(key)? + key.len()..];
    let start = rest.find(['"', '\''])?;
    let quote = rest[start..].chars().next()?;
    let body = &rest[start + 1..];
    body.find(quote).map(|end| &body[..end])
}

/// Maven: the project's own `<artifactId>` — the first one outside the
/// `<parent>` block, which names the parent, not this module.
pub fn maven_name(dir: &Path) -> Option<String> {
    let mut text = read(&dir.join("pom.xml"))?;
    if let (Some(start), Some(end)) = (text.find("<parent>"), text.find("</parent>"))
        && start < end
    {
        text.replace_range(start..end + "</parent>".len(), "");
    }
    let start = text.find("<artifactId>")? + "<artifactId>".len();
    let end = start + text[start..].find("</artifactId>")?;
    Some(text[start..end].trim().to_string()).filter(|s| !s.is_empty())
}

/// sbt: `name := "…"` in `build.sbt`.
pub fn sbt_name(dir: &Path) -> Option<String> {
    read(&dir.join("build.sbt"))?
        .lines()
        .find(|l| l.trim_start().starts_with("name") && l.contains(":="))
        .and_then(|l| quoted_after(l, ":=").map(str::to_string))
}

/// Elixir: the `app: :name` atom in `mix.exs`.
pub fn mix_app(dir: &Path) -> Option<String> {
    let text = read(&dir.join("mix.exs"))?;
    let rest = &text[text.find("app:")? + "app:".len()..];
    let atom = rest.trim_start().strip_prefix(':')?;
    let name: String = atom
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    Some(name).filter(|s| !s.is_empty())
}

/// Swift: the package's `name: "…"` in `Package.swift`.
pub fn swift_name(dir: &Path) -> Option<String> {
    read(&dir.join("Package.swift"))?
        .lines()
        .find(|l| l.contains("name:"))
        .and_then(|l| quoted_after(l, "name:").map(str::to_string))
}

/// Dart/Flutter: the top-level `name:` in `pubspec.yaml`.
pub fn pubspec_name(dir: &Path) -> Option<String> {
    read(&dir.join("pubspec.yaml"))?
        .lines()
        .find_map(|l| l.strip_prefix("name:"))
        .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
        .filter(|s| !s.is_empty())
}

/// Whether a `pubspec.yaml` is a Flutter app or package: it depends on the
/// Flutter SDK (`sdk: flutter`), which plain Dart packages never do.
pub fn is_flutter(dir: &Path) -> bool {
    read(&dir.join("pubspec.yaml")).is_some_and(|t| t.lines().any(|l| l.trim() == "sdk: flutter"))
}

/// Haskell: the `name:` field of the package's `.cabal` file, or the file's
/// stem when it declares none.
pub fn cabal_name(dir: &Path) -> Option<String> {
    let file = first_with_ext(dir, &["cabal"])?;
    let declared = read(&file).and_then(|t| {
        t.lines()
            .find_map(|l| l.strip_prefix("name:"))
            .map(|v| v.trim().to_string())
    });
    declared
        .filter(|s| !s.is_empty())
        .or_else(|| file.file_stem()?.to_str().map(str::to_string))
}

/// .NET: the stem of the project (or solution) file — what `dotnet` itself
/// calls the project.
pub fn dotnet_name(dir: &Path) -> Option<String> {
    let file = first_with_ext(dir, &["csproj", "fsproj", "vbproj", "sln"])?;
    file.file_stem()?.to_str().map(str::to_string)
}

/// Composer: `scripts` from `composer.json`, alphabetical. A script is a
/// command string or a list of them, shown joined with `&&`.
pub fn composer_scripts(doc: &serde_json::Value) -> Vec<(String, String)> {
    let Some(scripts) = doc.get("scripts").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    scripts
        .iter()
        .map(|(k, v)| {
            let text = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Array(items) => items
                    .iter()
                    .filter_map(|i| i.as_str())
                    .collect::<Vec<_>>()
                    .join(" && "),
                _ => String::new(),
            };
            (k.clone(), text)
        })
        .collect()
}

/// Rake: the tasks a `Rakefile` defines at top level — `task :name`,
/// `task name: …`, `task "name"` — each with the `desc "…"` above it.
/// Namespaced tasks are skipped: `ns:task` is not a name `tsr` reads as one
/// task (SPEC §4.1 — it is, but `rake` would need the namespace too).
pub fn rake_tasks(dir: &Path) -> Vec<(String, String)> {
    let Some(text) = RAKEFILES.iter().find_map(|f| read(&dir.join(f))) else {
        return Vec::new();
    };
    let mut out: Vec<(String, String)> = Vec::new();
    let mut desc = String::new();
    let mut depth = 0usize;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("namespace") && t.ends_with("do") {
            depth += 1;
            continue;
        }
        if depth > 0 {
            if t == "end" {
                depth -= 1;
            }
            continue;
        }
        if let Some(rest) = t.strip_prefix("desc ") {
            desc = rest.trim().trim_matches(['"', '\'']).to_string();
            continue;
        }
        let Some(rest) = t.strip_prefix("task ").or_else(|| t.strip_prefix("task(")) else {
            continue;
        };
        let rest = rest.trim_start();
        let name: String = if let Some(sym) = rest.strip_prefix(':') {
            sym.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect()
        } else if let Some(s) = rest.strip_prefix(['"', '\'']) {
            s.chars()
                .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
                .collect()
        } else {
            // `task name: [:deps]` — the hash-key form.
            let n: String = rest
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            if rest[n.len()..].starts_with(':') {
                n
            } else {
                String::new()
            }
        };
        if !name.is_empty() && !out.iter().any(|(k, _)| *k == name) {
            out.push((name, std::mem::take(&mut desc)));
        }
    }
    out
}

/// A project `tsr` recognises but cannot auto-detect a task for: a build
/// system or language with no `<tool> <task-name>` convention. Configless mode
/// and auto-detect name it in their error, with a `run` line to start from,
/// instead of reporting that nothing was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unmapped {
    /// The marker file that identified it.
    pub marker: &'static str,
    /// The tool or language, as people call it.
    pub tool: &'static str,
    /// A `run` string that is a reasonable start for a `build`/`test` task.
    pub example: &'static str,
}

const UNMAPPED: [Unmapped; 10] = [
    Unmapped {
        marker: "CMakeLists.txt",
        tool: "CMake",
        example: "cmake -B build && cmake --build build",
    },
    Unmapped {
        marker: "meson.build",
        tool: "Meson",
        example: "meson setup build && meson compile -C build",
    },
    Unmapped {
        marker: "MODULE.bazel",
        tool: "Bazel",
        example: "bazel build //...",
    },
    Unmapped {
        marker: "WORKSPACE.bazel",
        tool: "Bazel",
        example: "bazel build //...",
    },
    Unmapped {
        marker: "WORKSPACE",
        tool: "Bazel",
        example: "bazel build //...",
    },
    Unmapped {
        marker: "Dockerfile",
        tool: "Docker",
        example: "docker build -t app .",
    },
    Unmapped {
        marker: "DESCRIPTION",
        tool: "R",
        example: "Rscript -e 'devtools::test()'",
    },
    Unmapped {
        marker: "Project.toml",
        tool: "Julia",
        example: "julia --project -e 'using Pkg; Pkg.test()'",
    },
    Unmapped {
        marker: "cpanfile",
        tool: "Perl",
        example: "prove -l t",
    },
    Unmapped {
        marker: "Makefile.PL",
        tool: "Perl",
        example: "perl Makefile.PL && make test",
    },
];

/// The recognised-but-unmapped project at `dir`, if any.
pub fn unmapped(dir: &Path) -> Option<Unmapped> {
    UNMAPPED
        .iter()
        .find(|u| dir.join(u.marker).is_file())
        .copied()
}

impl Unmapped {
    /// The error text for `tsr <task>` against this project, found in `dir`.
    pub fn explain(&self, dir: &Path, task: &str) -> String {
        format!(
            "found {} in '{}' — {} has no task-name convention for `tsr {task}` to map \
             onto, so give it a task in tasks.toml (`tsr --init` creates one), e.g.\n\n  \
             [tasks.{task}]\n  run = \"{}\"",
            self.marker,
            dir.display(),
            self.tool,
            self.example.replace('"', "\\\"")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-langs-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn maven_name_skips_the_parent_block() {
        let d = scratch();
        fs::write(
            d.join("pom.xml"),
            "<project>\n  <parent>\n    <artifactId>acme-parent</artifactId>\n  </parent>\n  \
             <artifactId>acme-api</artifactId>\n</project>\n",
        )
        .unwrap();
        assert_eq!(maven_name(&d).as_deref(), Some("acme-api"));
    }

    #[test]
    fn reads_names_from_each_manifest() {
        let d = scratch();
        fs::write(
            d.join("build.sbt"),
            "ThisBuild / scalaVersion := \"3.3.1\"\nname := \"core\"\n",
        )
        .unwrap();
        assert_eq!(sbt_name(&d).as_deref(), Some("core"));

        let d = scratch();
        fs::write(
            d.join("mix.exs"),
            "def project do\n  [app: :my_app, version: \"0.1.0\"]\n",
        )
        .unwrap();
        assert_eq!(mix_app(&d).as_deref(), Some("my_app"));

        let d = scratch();
        fs::write(
            d.join("Package.swift"),
            "let package = Package(\n    name: \"Kit\",\n",
        )
        .unwrap();
        assert_eq!(swift_name(&d).as_deref(), Some("Kit"));

        let d = scratch();
        fs::write(
            d.join("pubspec.yaml"),
            "name: my_app\nenvironment:\n  sdk: '>=3.0.0'\n",
        )
        .unwrap();
        assert_eq!(pubspec_name(&d).as_deref(), Some("my_app"));
        assert!(!is_flutter(&d));
        fs::write(
            d.join("pubspec.yaml"),
            "name: my_app\ndependencies:\n  flutter:\n    sdk: flutter\n",
        )
        .unwrap();
        assert!(is_flutter(&d));

        let d = scratch();
        fs::write(d.join("lib.cabal"), "cabal-version: 3.0\nname:    parser\n").unwrap();
        assert_eq!(cabal_name(&d).as_deref(), Some("parser"));

        let d = scratch();
        fs::write(d.join("Api.csproj"), "<Project/>").unwrap();
        assert_eq!(dotnet_name(&d).as_deref(), Some("Api"));
    }

    #[test]
    fn wrapper_is_found_up_to_the_root_only() {
        let outer = scratch();
        let root = outer.join("ws");
        let sub = root.join("app");
        fs::create_dir_all(&sub).unwrap();
        let name = if cfg!(windows) {
            "gradlew.bat"
        } else {
            "gradlew"
        };
        fs::write(outer.join(name), "").unwrap();
        // Above the root: ignored.
        assert_eq!(wrapper(&sub, &root, "gradlew"), None);
        fs::write(root.join(name), "").unwrap();
        assert_eq!(wrapper(&sub, &root, "gradlew"), Some(root.join(name)));
    }

    #[test]
    fn reads_composer_scripts() {
        let doc: serde_json::Value =
            serde_json::from_str(r#"{"scripts":{"test":"phpunit","check":["@lint","@test"]}}"#)
                .unwrap();
        assert_eq!(
            composer_scripts(&doc),
            [
                ("check".to_string(), "@lint && @test".to_string()),
                ("test".to_string(), "phpunit".to_string())
            ]
        );
    }

    #[test]
    fn reads_top_level_rake_tasks() {
        let d = scratch();
        fs::write(
            d.join("Rakefile"),
            "desc \"Run the specs\"\ntask :spec do\nend\n\
             task default: [:spec]\n\
             task \"lint\" do\nend\n\
             namespace :db do\n  task :migrate do\n  end\nend\n\
             desc 'Build gem'\ntask(:build) { }\n",
        )
        .unwrap();
        assert_eq!(
            rake_tasks(&d),
            [
                ("spec".to_string(), "Run the specs".to_string()),
                ("default".to_string(), String::new()),
                ("lint".to_string(), String::new()),
                ("build".to_string(), "Build gem".to_string()),
            ]
        );
    }

    #[test]
    fn recognises_unmapped_projects() {
        let d = scratch();
        assert_eq!(unmapped(&d), None);
        fs::write(d.join("CMakeLists.txt"), "").unwrap();
        let u = unmapped(&d).unwrap();
        assert_eq!(u.tool, "CMake");
        let msg = u.explain(&d, "build");
        assert!(msg.contains("found CMakeLists.txt"), "{msg}");
        assert!(
            msg.contains("[tasks.build]\n  run = \"cmake -B build"),
            "{msg}"
        );
    }
}
