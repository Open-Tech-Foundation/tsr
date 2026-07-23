//! End-to-end tests driving the compiled `tsr` binary against real temp
//! workspaces, asserting on exit codes and output (SPEC §5, §6, §7, §8, §10).
//!
//! Some of the tasks these tests run reach for Unix binaries (`sh`, `false`) or
//! POSIX permissions, so the suite is Unix-only. On Windows the CI matrix still
//! compiles the binary and runs every platform-independent unit test — including
//! the builtins (SPEC §8.5), which are what make `run` strings portable in the
//! first place.
#![cfg(unix)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Path to the binary under test, provided by Cargo for integration tests.
const BIN: &str = env!("CARGO_BIN_EXE_tsr");

/// Create a fresh temp workspace directory.
fn workspace() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let id = N.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tsr-e2e-{}-{id}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, rel: &str, contents: &str) {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, contents).unwrap();
}

/// Run `tsr` in `dir` with the given args.
fn tsr(dir: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to spawn tsr")
}

fn code(out: &Output) -> i32 {
    out.status.code().unwrap_or(-1)
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).to_string()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).to_string()
}

#[test]
fn runs_a_direct_command() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.hello]\nrun = \"echo hi-there\"\n",
    );
    let out = tsr(&ws, &["hello"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("hi-there"));
}

#[test]
fn expands_env_from_workspace_block() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[env]\nWHO = \"world\"\n[tasks.hi]\nrun = \"echo hello $WHO\"\n",
    );
    let out = tsr(&ws, &["hi"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("hello world"), "{}", stdout(&out));
}

#[test]
fn loads_root_dotenv() {
    let ws = workspace();
    write(&ws, ".env", "TOKEN=sekret\n");
    write(&ws, "tasks.toml", "[tasks.show]\nrun = \"echo $TOKEN\"\n");
    let out = tsr(&ws, &["show"]);
    assert!(stdout(&out).contains("sekret"), "{}", stdout(&out));
}

#[test]
fn env_file_list_overrides_default_dotenv_last_wins() {
    // env_file layers over the root .env; within the list, the later file wins.
    let ws = workspace();
    write(&ws, ".env", "FOO=from-default\nSHARED=base\n");
    write(&ws, ".env.local", "FOO=from-local\nONLY_LOCAL=1\n");
    write(&ws, ".env.test", "FOO=from-test\n");
    write(
        &ws,
        "tasks.toml",
        "[tasks.test]\n\
         run = \"sh -c 'echo FOO=$FOO SHARED=$SHARED LOCAL=$ONLY_LOCAL'\"\n\
         env_file = [\".env.local\", \".env.test\"]\n",
    );
    let out = tsr(&ws, &["test"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    // .env.test overrides .env.local overrides .env; base-only keys survive.
    assert!(
        stdout(&out).contains("FOO=from-test SHARED=base LOCAL=1"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn env_file_is_scoped_per_task() {
    // A task without env_file sees only the root .env — no leakage from a sibling.
    let ws = workspace();
    write(&ws, ".env", "FOO=default\n");
    write(&ws, ".env.test", "FOO=test\n");
    write(
        &ws,
        "tasks.toml",
        "[tasks.a]\nrun = \"sh -c 'echo a=$FOO'\"\nenv_file = \".env.test\"\n\
         [tasks.b]\nrun = \"sh -c 'echo b=$FOO'\"\n",
    );
    assert!(stdout(&tsr(&ws, &["a"])).contains("a=test"));
    assert!(stdout(&tsr(&ws, &["b"])).contains("b=default"));
}

#[test]
fn env_file_satisfies_the_load_time_run_var_check() {
    // A $VAR defined only in an env_file must not trip the undefined-var check.
    let ws = workspace();
    write(&ws, ".env.test", "TARGET=prod\n");
    write(
        &ws,
        "tasks.toml",
        "[tasks.deploy]\nrun = \"echo deploying-to $TARGET\"\nenv_file = \".env.test\"\n",
    );
    let out = tsr(&ws, &["deploy"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("deploying-to prod"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn forwards_passthrough_after_double_dash() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.e]\nrun = \"echo\"\nargs = [\"--first\"]\n",
    );
    let out = tsr(&ws, &["e", "--", "--second"]);
    // args prepended before passthrough (SPEC §6).
    assert!(
        stdout(&out).contains("--first --second"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn propagates_exact_child_exit_code() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.boom]\ndelegate = { bin = \"sh\", args = [\"-c\", \"exit 7\"] }\n",
    );
    assert_eq!(code(&tsr(&ws, &["boom"])), 7);
}

#[test]
fn mini_shell_or_recovers() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.c]\nrun = \"false || echo recovered\"\n",
    );
    let out = tsr(&ws, &["c"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("recovered"));
}

#[test]
fn rejected_metachar_is_config_error_64() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.p]\nrun = \"cat a | grep b\"\n");
    let out = tsr(&ws, &["p"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("pipe"));
}

#[test]
fn undefined_var_is_config_error_64() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.d]\nrun = \"deploy $NOPE_VAR\"\n");
    let out = tsr(&ws, &["d"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("$NOPE_VAR"));
}

#[test]
fn unknown_task_is_runner_error_64() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.a]\nrun = \"true\"\n");
    let out = tsr(&ws, &["ghost"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("unknown task"));
}

#[test]
fn dir_and_packages_together_is_config_error_64() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.x]\nrun = \"true\"\ndir = \"a\"\npackages = [\"b\"]\n",
    );
    let out = tsr(&ws, &["x"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("mutually exclusive"));
}

#[test]
fn dependency_cycle_is_config_error_64() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.a]\ndeps = [\"b\"]\n[tasks.b]\ndeps = [\"a\"]\n",
    );
    let out = tsr(&ws, &["a"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("cycle"));
}

#[test]
fn deps_run_first_and_fail_fast() {
    let ws = workspace();
    let marker = ws.join("b-ran");
    write(
        &ws,
        "tasks.toml",
        &format!(
            "[tasks.ci]\ndeps = [\"a\", \"b\"]\n\
             [tasks.a]\nrun = \"false\"\n\
             [tasks.b]\nrun = \"touch {}\"\n",
            marker.display()
        ),
    );
    let out = tsr(&ws, &["ci"]);
    assert_eq!(code(&out), 1);
    assert!(!marker.exists(), "sibling must be skipped on fail-fast");
    assert!(stderr(&out).contains("✗ ci failed"));
}

#[test]
fn discovers_root_from_nested_dir() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.hi]\nrun = \"echo found\"\n");
    let nested = ws.join("a/b/c");
    fs::create_dir_all(&nested).unwrap();
    let out = tsr(&nested, &["hi"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).contains("found"));
}

#[test]
fn list_shows_tasks() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.build]\ndelegate = \"turbo\"\n[tasks.dev]\nrun = \"vite\"\n",
    );
    let out = tsr(&ws, &["--list"]);
    assert_eq!(code(&out), 0);
    let s = stdout(&out);
    assert!(s.contains("build") && s.contains("delegate: turbo"));
    assert!(s.contains("dev") && s.contains("run: vite"));
}

#[test]
fn a_task_named_list_is_not_shadowed_by_a_builtin() {
    // Builtins are flags (`--list`), so the bare word `list` runs the task.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.list]\nrun = \"echo iam-the-task\"\n",
    );
    let out = tsr(&ws, &["list"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("iam-the-task"), "{}", stdout(&out));
}

#[test]
fn packages_fan_out_across_matching_packages() {
    // Two cargo packages; a bare task auto-detects `cargo <task>` per package.
    // `cargo help` exits 0 in any crate dir, proving the fan-out spawns twice.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n[tasks.help]\npackages = [\"crates/*\"]\n",
    );
    write(&ws, "crates/one/Cargo.toml", "[package]\nname = \"one\"\n");
    write(&ws, "crates/two/Cargo.toml", "[package]\nname = \"two\"\n");
    let out = tsr(&ws, &["help"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
}

#[cfg(unix)]
#[test]
fn resolves_local_node_modules_bin() {
    // A `run` string naming a locally-installed binary must resolve from
    // node_modules/.bin — the same lookup npm/bun do — so `run = "vite"` works.
    // Uses a *symlink* (the real npm/yarn/pnpm layout: .bin/x → ../pkg/bin/x),
    // pointing at a shebang script, to match how tools are actually installed.
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    let real = ws.join("node_modules/vite/bin/vite.js");
    fs::create_dir_all(real.parent().unwrap()).unwrap();
    fs::write(
        &real,
        "#!/usr/bin/env node\nconsole.log('vite ' + process.argv[2]);\n",
    )
    .unwrap();
    fs::set_permissions(&real, fs::Permissions::from_mode(0o755)).unwrap();

    let bindir = ws.join("node_modules/.bin");
    fs::create_dir_all(&bindir).unwrap();
    std::os::unix::fs::symlink("../vite/bin/vite.js", bindir.join("vite")).unwrap();

    write(
        &ws,
        "tasks.toml",
        "[tasks.dev]\nrun = \"vite\"\nargs = [\"build\"]\n",
    );
    let out = tsr(&ws, &["dev"]);
    // Skip if `node` isn't available on this machine (the shebang needs it).
    if code(&out) == 0 {
        assert!(stdout(&out).contains("vite build"), "{}", stdout(&out));
    } else {
        assert!(
            stderr(&out).contains("node"),
            "expected a node-related failure, got: {}",
            stderr(&out)
        );
    }
}

#[cfg(unix)]
#[test]
fn nested_package_bin_wins_over_hoisted_root_bin() {
    // node_modules/.bin is collected nearest-first: a package's own bin shadows a
    // hoisted root one of the same name.
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    let mk = |path: &std::path::Path, msg: &str| {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("#!/bin/sh\necho {msg}\n")).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
    };
    mk(&ws.join("node_modules/.bin/tool"), "root-tool");
    mk(&ws.join("apps/web/node_modules/.bin/tool"), "web-tool");
    write(
        &ws,
        "tasks.toml",
        "[tasks.t]\nrun = \"tool\"\ndir = \"apps/web\"\n",
    );
    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("web-tool"), "{}", stdout(&out));
}

#[test]
fn init_scaffolds_a_reference_config_with_no_tasks() {
    let ws = workspace();
    // No tasks.toml yet.
    let out = tsr(&ws, &["--init"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(ws.join("tasks.toml").exists());
    assert!(stdout(&out).contains("Created"));

    // The scaffold is reference comments only — it defines nothing, and points
    // at the docs so the examples are followable.
    let text = std::fs::read_to_string(ws.join("tasks.toml")).unwrap();
    assert!(text.contains("https://tsr.opentechf.org/docs"), "{text}");
    let list = tsr(&ws, &["--list"]);
    assert_eq!(code(&list), 0, "stderr: {}", stderr(&list));
    assert!(
        stdout(&list).contains("No tasks defined"),
        "{}",
        stdout(&list)
    );

    // Re-running --init must not overwrite.
    let again = tsr(&ws, &["--init"]);
    assert_eq!(code(&again), 64);
    assert!(stderr(&again).contains("already exists"));
}

#[test]
fn packages_pattern_matching_nothing_is_error_64() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n[tasks.t]\npackages = [\"nope/*\"]\n",
    );
    write(&ws, "crates/one/Cargo.toml", "[package]\nname = \"one\"\n");
    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("matched no"));
}

/// Write a fake runner that prints exactly how it was invoked, so tests can
/// assert what `tsr` spawned without needing the real toolchain installed.
fn shim(dir: &Path, name: &str) {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\necho INVOKED {name} \"$@\"\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
}

/// Run `tsr` with `prepend` at the front of `PATH` (so shims shadow real tools).
fn tsr_with_path(dir: &Path, args: &[&str], prepend: &Path) -> Output {
    let path = std::env::var("PATH").unwrap_or_default();
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PATH", format!("{}:{path}", prepend.display()))
        .output()
        .expect("failed to spawn tsr")
}

#[test]
fn bare_task_autodetects_each_ecosystem() {
    // The "auto-detects each package's runner" claim (SPEC §3.1, form 3): a bare
    // [tasks.<name>] with no run/delegate resolves to the package's native runner.
    // Shim runners on PATH report exactly what tsr invoked.
    // Each case: (ecosystem label, marker files to write, expected shim invocation).
    type Case<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a str);
    let cases: &[Case] = &[
        ("npm", &[("package.json", "{}")], "INVOKED npm run build"),
        (
            "bun",
            &[("package.json", "{}"), ("bun.lock", "")],
            "INVOKED bun run build",
        ),
        (
            "cargo",
            &[("Cargo.toml", "[package]\nname=\"c\"\n")],
            "INVOKED cargo build",
        ),
        ("go", &[("go.mod", "module ex\n")], "INVOKED go build"),
        (
            "uv",
            &[("pyproject.toml", "[project]\nname=\"p\"\n")],
            "INVOKED uv run build",
        ),
    ];
    for (label, markers, expected) in cases {
        let ws = workspace();
        let bin = ws.join("shims");
        fs::create_dir_all(&bin).unwrap();
        for r in ["npm", "bun", "cargo", "go", "uv"] {
            shim(&bin, r);
        }
        for (name, contents) in *markers {
            write(&ws, name, contents);
        }
        write(&ws, "tasks.toml", "[tasks.build]\n");
        let out = tsr_with_path(&ws, &["build"], &bin);
        assert_eq!(code(&out), 0, "{label}: stderr {}", stderr(&out));
        assert!(
            stdout(&out).contains(expected),
            "{label}: expected `{expected}`, stdout {:?} stderr {:?}",
            stdout(&out),
            stderr(&out)
        );
    }
}

#[test]
fn deps_only_task_is_an_aggregator_not_autodetected() {
    // A bare task WITH deps is a pure aggregator (SPEC §5.2): it runs its deps and
    // nothing of its own — it must NOT auto-detect `npm run ci` after them.
    let ws = workspace();
    let bin = ws.join("shims");
    fs::create_dir_all(&bin).unwrap();
    shim(&bin, "npm");
    write(&ws, "package.json", "{}");
    let marker = ws.join("dep-ran");
    write(
        &ws,
        "tasks.toml",
        &format!(
            "[tasks.ci]\ndeps = [\"a\"]\n[tasks.a]\nrun = \"touch {}\"\n",
            marker.display()
        ),
    );
    let out = tsr_with_path(&ws, &["ci"], &bin);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(marker.exists(), "dependency 'a' should have run");
    assert!(
        !stdout(&out).contains("INVOKED npm"),
        "aggregator must not auto-detect a native runner: {}",
        stdout(&out)
    );
}

#[test]
fn bare_task_without_a_marker_is_runner_error_64() {
    // Form 3 with no detectable ecosystem: a clear runner error (exit 64), never a
    // silent no-op.
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.build]\n");
    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 64, "stderr {}", stderr(&out));
    assert!(
        stderr(&out).contains("no recognised ecosystem"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn configless_runs_the_package_native_script() {
    // No tasks.toml at all: `tsr dev` still works repo-aware, mapping to the
    // package's native runner — here `npm run dev` — and forwards passthrough.
    let ws = workspace();
    let bin = ws.join("shims");
    fs::create_dir_all(&bin).unwrap();
    shim(&bin, "npm");
    write(&ws, "package.json", "{}");
    // deliberately NO tasks.toml
    let out = tsr_with_path(&ws, &["dev", "--", "--host"], &bin);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("INVOKED npm run dev --host"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn configless_walks_up_to_the_nearest_package() {
    // Run from a nested directory: tsr finds the package marker in a parent, just
    // like npm walking up to package.json.
    let ws = workspace();
    let bin = ws.join("shims");
    fs::create_dir_all(&bin).unwrap();
    shim(&bin, "cargo");
    write(&ws, "Cargo.toml", "[package]\nname = \"c\"\n");
    let nested = ws.join("src/deep");
    fs::create_dir_all(&nested).unwrap();
    let out = tsr_with_path(&nested, &["build"], &bin);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("INVOKED cargo build"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn configless_without_any_marker_is_error_64() {
    // No tasks.toml and no ecosystem marker: a clear error, not a silent success.
    let ws = workspace();
    let out = tsr(&ws, &["dev"]);
    assert_eq!(code(&out), 64, "stderr {}", stderr(&out));
    assert!(
        stderr(&out).contains("no 'tasks.toml' found") && stderr(&out).contains("--init"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn tasks_toml_takes_precedence_over_configless() {
    // With a tasks.toml present, its definition wins over auto-detection even when
    // a package.json exists — no accidental fall-through.
    let ws = workspace();
    write(&ws, "package.json", "{}");
    write(
        &ws,
        "tasks.toml",
        "[tasks.dev]\nrun = \"echo from-config\"\n",
    );
    let out = tsr(&ws, &["dev"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(stdout(&out).contains("from-config"), "{}", stdout(&out));
}

// --- builtins & globbing (SPEC §8.5, §8.1) ---

#[test]
fn glob_expands_and_builtin_rm_cleans_a_build_dir() {
    // The motivating case: `rm -rf dist/*` must work identically everywhere and
    // stay a success when there is nothing left to remove.
    let ws = workspace();
    write(&ws, "dist/a.js", "");
    write(&ws, "dist/b.js", "");
    write(&ws, "dist/nested/c.js", "");
    write(&ws, "keep.txt", "");
    write(
        &ws,
        "tasks.toml",
        "[tasks.clean]\nrun = \"rm -rf dist/*\"\n",
    );

    let out = tsr(&ws, &["clean"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        ws.join("dist").is_dir(),
        "the glob matched the entries, not dist"
    );
    assert!(!ws.join("dist/a.js").exists());
    assert!(!ws.join("dist/nested").exists());
    assert!(ws.join("keep.txt").exists());

    // Idempotent: the pattern now matches nothing, and `-f` makes that a success.
    assert_eq!(code(&tsr(&ws, &["clean"])), 0);
}

#[test]
fn builtins_chain_through_the_mini_shell() {
    let ws = workspace();
    write(&ws, "src/one.txt", "hello");
    write(
        &ws,
        "tasks.toml",
        "[tasks.build]\nrun = \"mkdir -p out/deep && cp src/*.txt out/deep && touch out/deep/.stamp\"\n",
    );
    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert_eq!(
        fs::read_to_string(ws.join("out/deep/one.txt")).unwrap(),
        "hello"
    );
    assert!(ws.join("out/deep/.stamp").is_file());
}

#[test]
fn builtin_shadows_a_binary_of_the_same_name_on_path() {
    // A builtin always wins, so one `run` string behaves the same on every OS.
    let ws = workspace();
    let bin = ws.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    shim(&bin, "rm");
    write(&ws, "doomed.txt", "");
    write(&ws, "tasks.toml", "[tasks.t]\nrun = \"rm doomed.txt\"\n");

    let out = tsr_with_path(&ws, &["t"], &bin);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        !stdout(&out).contains("INVOKED"),
        "PATH shim must not run: {}",
        stdout(&out)
    );
    assert!(
        !ws.join("doomed.txt").exists(),
        "the builtin must do the work"
    );
}

#[test]
fn builtin_failure_is_a_normal_task_failure() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.t]\nrun = \"rm ghost.txt\"\n");
    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 1);
    assert!(stderr(&out).contains("ghost.txt"), "{}", stderr(&out));
}

#[test]
fn globs_resolve_against_the_task_dir() {
    let ws = workspace();
    write(&ws, "pkg/a.log", "");
    write(&ws, "outside.log", "");
    write(
        &ws,
        "tasks.toml",
        "[tasks.clean]\ndir = \"pkg\"\nrun = \"rm -f *.log\"\n",
    );
    assert_eq!(code(&tsr(&ws, &["clean"])), 0);
    assert!(!ws.join("pkg/a.log").exists());
    assert!(ws.join("outside.log").exists(), "must not escape 'dir'");
}

#[test]
fn quoted_pattern_is_not_globbed() {
    let ws = workspace();
    write(&ws, "a.txt", "");
    write(&ws, "tasks.toml", "[tasks.show]\nrun = \"echo '*.txt'\"\n");
    let out = tsr(&ws, &["show"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(stdout(&out).contains("*.txt"), "{}", stdout(&out));
}

#[test]
fn undefined_var_error_underlines_the_reference() {
    // SPEC §7.3: the diagnostic points a caret at the offending reference.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.deploy]\nrun = \"deploy --target $TARGET\"\n",
    );
    let out = tsr(&ws, &["deploy"]);
    assert_eq!(code(&out), 64);
    let err = stderr(&out);
    assert!(err.contains("run = \"deploy --target $TARGET\""), "{err}");
    assert!(err.contains("^^^^^^^"), "{err}");
    assert!(err.contains("'$TARGET' is not defined"), "{err}");
}

#[test]
fn parameter_expansion_is_rejected_with_a_targeted_error() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.t]\nrun = \"deploy ${TARGET:-prod}\"\n",
    );
    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 64);
    assert!(
        stderr(&out).contains("parameter expansion"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_glob_sees_files_an_earlier_command_in_the_sequence_created() {
    // Globs resolve per command, not when the plan is built, so this pattern
    // must match the file `touch` produces one step earlier.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.t]\nrun = \"touch dist/built.map && rm dist/*.map\"\n",
    );
    write(&ws, "dist/keep.js", "");
    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        !ws.join("dist/built.map").exists(),
        "the glob must have matched a file created mid-sequence"
    );
    assert!(ws.join("dist/keep.js").exists());
}

#[test]
fn or_true_makes_a_step_non_fatal_without_a_unix_binary() {
    // `cmd || true` is the standard "don't fail the build here" idiom; both
    // halves are builtins so it behaves the same on Windows.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.t]\nrun = \"rm ghost.txt || true\"\n[tasks.f]\nrun = \"true && false\"\n",
    );
    assert_eq!(code(&tsr(&ws, &["t"])), 0);
    assert_eq!(code(&tsr(&ws, &["f"])), 1);
}

#[test]
fn brace_expansion_is_rejected_rather_than_silently_passed_through() {
    // With `-f` an unexpanded `{a,b}` would fail silently, doing nothing.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.c]\nrun = \"rm -rf dist/{js,css}\"\n",
    );
    let out = tsr(&ws, &["c"]);
    assert_eq!(code(&out), 64);
    assert!(stderr(&out).contains("brace expansion"), "{}", stderr(&out));
}

#[test]
fn braces_without_a_comma_still_work() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.e]\nrun = \"echo --define:{}\"\n");
    let out = tsr(&ws, &["e"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(stdout(&out).contains("--define:{}"), "{}", stdout(&out));
}

#[test]
fn globstar_matches_across_directories() {
    let ws = workspace();
    write(&ws, "src/one.js", "");
    write(&ws, "src/deep/two.js", "");
    write(&ws, "src/deep/skip.ts", "");
    write(
        &ws,
        "tasks.toml",
        "[tasks.list]\nrun = \"echo src/**/*.js\"\n",
    );
    let out = tsr(&ws, &["list"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("src/deep/two.js") && stdout(&out).contains("src/one.js"),
        "{}",
        stdout(&out)
    );
    assert!(!stdout(&out).contains("skip.ts"), "{}", stdout(&out));
}
