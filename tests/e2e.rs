//! End-to-end tests driving the compiled `tsr` binary against real temp
//! workspaces, asserting on exit codes and output (SPEC §5, §6, §7, §8, §10).
//!
//! Most of the suite runs on every platform in the CI matrix: the builtins
//! (SPEC §8.5) are what make a `run` string portable, so a task built from them
//! is testable everywhere, and [`shim`] stands up a fake runner in whichever
//! form the platform actually ships (executable script / `.cmd`). Only the
//! tests that name a Unix binary outright (`sh`) or build a symlinked shebang
//! layout carry a `#[cfg(unix)]`.
//!
//! Keep new tests platform-independent where the behaviour is: a task written
//! with builtins asserts the same thing on Windows, and that is where separator
//! and path-resolution bugs surface.

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

#[cfg(unix)]
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

#[cfg(unix)]
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

#[cfg(unix)]
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
    // The marker is named relatively: a task runs in the workspace root, and an
    // absolute Windows path would put `\b` — an invalid escape — in the TOML.
    write(
        &ws,
        "tasks.toml",
        "[tasks.ci]\ndeps = [\"a\", \"b\"]\n\
         [tasks.a]\nrun = \"false\"\n\
         [tasks.b]\nrun = \"touch b-ran\"\n",
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
///
/// Deliberately built the way each platform really ships a tool: an executable
/// shebang script on Unix, a `.cmd` batch shim on Windows — which is exactly the
/// shape (`npm.cmd`, `node_modules/.bin/vite.cmd`) that `Command`'s own PATH
/// search cannot find, and that `env::resolve_program` exists to resolve.
fn shim(dir: &Path, name: &str) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        fs::write(&p, format!("#!/bin/sh\necho INVOKED {name} \"$@\"\n")).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    #[cfg(windows)]
    {
        let p = dir.join(format!("{name}.cmd"));
        fs::write(&p, format!("@echo off\r\necho INVOKED {name} %*\r\n")).unwrap();
    }
}

/// Run `tsr` with `prepend` at the front of `PATH` (so shims shadow real tools).
fn tsr_with_path(dir: &Path, args: &[&str], prepend: &Path) -> Output {
    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    Command::new(BIN)
        .args(args)
        .current_dir(dir)
        .env("PATH", format!("{}{sep}{path}", prepend.display()))
        .output()
        .expect("failed to spawn tsr")
}

/// A locally-installed tool is found by bare name from `node_modules/.bin`, the
/// `npm run` replacement case (SPEC §9.2). On Windows that tool is a `.cmd`, so
/// this is the regression test for `run = "vite"` failing with "program not
/// found".
#[test]
fn resolves_a_local_bin_shim_by_bare_name() {
    let ws = workspace();
    let bindir = ws.join("node_modules/.bin");
    fs::create_dir_all(&bindir).unwrap();
    shim(&bindir, "vite");
    write(
        &ws,
        "tasks.toml",
        "[tasks.dev]\nrun = \"vite\"\nargs = [\"build\"]\n",
    );
    let out = tsr(&ws, &["dev"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("INVOKED vite build"),
        "{}",
        stdout(&out)
    );
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
        "[tasks.ci]\ndeps = [\"a\"]\n[tasks.a]\nrun = \"touch dep-ran\"\n",
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

// --- topological deps (`^task`, SPEC §4.2, §5) -------------------------------
//
// Every package's task is `run = "pwd"`, a builtin that prints the job's own
// directory. Because the fan-out is sequential by default, the order of `pwd`
// lines *is* the execution order — a portable ordering probe that needs no
// external binary and no shell redirection.

/// A `package.json` declaring workspace dependencies.
fn manifest(name: &str, deps: &[&str]) -> String {
    let entries = deps
        .iter()
        .map(|d| format!("\"{d}\": \"workspace:*\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!("{{\"name\": \"{name}\", \"dependencies\": {{{entries}}}}}")
}

/// Index of the `pwd` line naming `rel`, for order assertions.
fn line_of(out: &str, rel: &str) -> usize {
    let needle = rel.replace('/', std::path::MAIN_SEPARATOR_STR);
    out.lines()
        .position(|l| l.trim_end().ends_with(&needle))
        .unwrap_or_else(|| panic!("no output line for '{rel}' in:\n{out}"))
}

#[test]
fn upstream_deps_run_in_topological_order() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(
        &ws,
        "packages/ui/package.json",
        &manifest("ui", &["tokens"]),
    );
    write(
        &ws,
        "packages/tokens/package.json",
        &manifest("tokens", &[]),
    );
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\", \"packages/*\"]\n\
         deps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    // tokens → ui → web: each package strictly after everything it depends on.
    assert!(
        line_of(&s, "packages/tokens") < line_of(&s, "packages/ui"),
        "{s}"
    );
    assert!(line_of(&s, "packages/ui") < line_of(&s, "apps/web"), "{s}");
}

#[test]
fn upstream_deps_reach_packages_outside_the_selection() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        // Only `apps/*` is selected, but building web requires building ui.
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\"]\ndeps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(line_of(&s, "packages/ui") < line_of(&s, "apps/web"), "{s}");
}

#[test]
fn shared_upstream_package_builds_once() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "apps/admin/package.json", &manifest("admin", &["ui"]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\", \"packages/*\"]\n\
         deps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    let ui = format!("packages{}ui", std::path::MAIN_SEPARATOR_STR);
    let hits = s.lines().filter(|l| l.trim_end().ends_with(&ui)).count();
    assert_eq!(hits, 1, "shared upstream should build once:\n{s}");
}

#[test]
fn upstream_failure_skips_the_dependent_package() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        // `false` fails in every package; ui is built first, so web never runs.
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\"]\ndeps = [\"^build\"]\nrun = \"false\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 1, "stderr {}", stderr(&out));
    let all = format!("{}{}", stdout(&out), stderr(&out));
    // The upstream package is what failed...
    assert!(all.contains("packages/ui"), "{all}");
    // ...and the dependent never launched, so it contributes no result line.
    // (A task whose deps failed is likewise absent from a v1 summary.)
    assert!(!all.contains("apps/web"), "web should not have run:\n{all}");
}

#[test]
fn upstream_dep_without_packages_is_a_config_error() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.ci]\ndeps = [\"^build\"]\n");
    let out = tsr(&ws, &["ci"]);
    assert_eq!(code(&out), 64);
    assert!(
        stderr(&out).contains("requires 'packages'"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn package_dependency_cycle_is_a_runner_error() {
    let ws = workspace();
    write(&ws, "packages/a/package.json", &manifest("a", &["b"]));
    write(&ws, "packages/b/package.json", &manifest("b", &["a"]));
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n\
         [tasks.build]\npackages = [\"packages/*\"]\ndeps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("cycle"), "{}", stderr(&out));
}

#[test]
fn upstream_marker_can_name_a_different_task() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        // web's `build` waits on `codegen` in its upstream packages.
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\"]\ndeps = [\"^codegen\"]\nrun = \"pwd\"\n\
         [tasks.codegen]\npackages = [\"packages/*\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(line_of(&s, "packages/ui") < line_of(&s, "apps/web"), "{s}");
}

// --- affected detection (`--since`, SPEC §9.3) -------------------------------

/// Initialise a git repo in `dir` and commit everything as the baseline, so
/// `--since HEAD` sees exactly the edits a test makes afterwards.
fn git_baseline(dir: &Path) {
    let git = |args: &[&str]| {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_AUTHOR_NAME", "tsr-test")
            .env("GIT_AUTHOR_EMAIL", "tsr@example.invalid")
            .env("GIT_COMMITTER_NAME", "tsr-test")
            .env("GIT_COMMITTER_EMAIL", "tsr@example.invalid")
            .output()
            .expect("failed to spawn git");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    };
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-q", "-m", "baseline"]);
}

/// A workspace of apps/web → packages/ui, plus an independent apps/docs.
fn affected_workspace(task: &str) -> PathBuf {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "apps/docs/package.json", &manifest("docs", &[]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(&ws, "tasks.toml", task);
    ws
}

const FANOUT: &str = "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
                      [tasks.build]\npackages = [\"apps/*\", \"packages/*\"]\nrun = \"pwd\"\n";

fn ran(out: &str, rel: &str) -> bool {
    let needle = rel.replace('/', std::path::MAIN_SEPARATOR_STR);
    out.lines().any(|l| l.trim_end().ends_with(&needle))
}

#[test]
fn since_selects_the_changed_package_and_its_dependents() {
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    write(&ws, "packages/ui/index.ts", "export const x = 1;\n");

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(ran(&s, "packages/ui"), "changed package must run:\n{s}");
    assert!(ran(&s, "apps/web"), "dependent must run:\n{s}");
    assert!(
        !ran(&s, "apps/docs"),
        "unrelated package must not run:\n{s}"
    );
}

#[test]
fn since_does_not_widen_to_dependencies() {
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    // Changing the app does not change the library it consumes.
    write(&ws, "apps/web/index.ts", "export const y = 2;\n");

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(ran(&s, "apps/web"), "{s}");
    assert!(!ran(&s, "packages/ui"), "{s}");
}

#[test]
fn since_still_builds_upstream_packages_for_the_caret_marker() {
    // The selection narrows, but `^build` correctness does not: web needs ui
    // built whether or not ui changed.
    let ws = affected_workspace(
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\"]\ndeps = [\"^build\"]\nrun = \"pwd\"\n",
    );
    git_baseline(&ws);
    write(&ws, "apps/web/index.ts", "export const y = 2;\n");

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(ran(&s, "packages/ui"), "upstream must still build:\n{s}");
    assert!(ran(&s, "apps/web"), "{s}");
    assert!(!ran(&s, "apps/docs"), "{s}");
}

#[test]
fn since_counts_untracked_files() {
    // A brand-new package exists only as untracked files.
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    write(&ws, "apps/docs/new-page.md", "# hi\n");

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(ran(&stdout(&out), "apps/docs"), "{}", stdout(&out));
}

#[test]
fn since_runs_everything_when_a_change_is_outside_every_package() {
    // A root-level change could affect anything, so it must not narrow.
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    write(&ws, "README.md", "# changed\n");

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    for rel in ["apps/web", "apps/docs", "packages/ui"] {
        assert!(ran(&s, rel), "{rel} should run:\n{s}");
    }
}

#[test]
fn since_with_no_changes_runs_nothing_and_succeeds() {
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);

    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(s.contains("no packages selected"), "{s}");
    assert!(!ran(&s, "apps/web"), "{s}");
}

#[test]
fn since_with_an_unknown_ref_is_a_runner_error() {
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    let out = tsr(&ws, &["build", "--since", "no-such-ref-xyz"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("git"), "{}", stderr(&out));
}

#[test]
fn since_outside_a_git_repo_is_a_runner_error() {
    // No git_baseline: the workspace is not a repository.
    let ws = affected_workspace(FANOUT);
    let out = tsr(&ws, &["build", "--since", "HEAD"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
}

#[test]
fn without_since_every_package_still_runs() {
    // The filter is opt-in; nothing changes for an ordinary invocation.
    let ws = affected_workspace(FANOUT);
    git_baseline(&ws);
    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    for rel in ["apps/web", "apps/docs", "packages/ui"] {
        assert!(ran(&s, rel), "{rel} should run:\n{s}");
    }
}

// --- `^task` across every ecosystem (SPEC §9) --------------------------------
//
// The graph rule is the same everywhere — a declared dependency name matching a
// workspace package's manifest name — but only npm was covered end to end. Each
// task sets `run = "pwd"`, so no native runner (cargo/go/uv) is ever invoked:
// the manifests exist purely to be read for names and edges.

/// Build a workspace whose packages are `(rel, manifest_file, contents)`, run
/// `build` with `^build`, and return the ordered `pwd` output.
fn topo_order_of(members: &str, pkgs: &[(&str, &str, String)]) -> String {
    let ws = workspace();
    for (rel, file, contents) in pkgs {
        write(&ws, &format!("{rel}/{file}"), contents);
    }
    write(
        &ws,
        "tasks.toml",
        &format!(
            "[workspace]\nmembers = [{members}]\n\
             [tasks.build]\npackages = [{members}]\ndeps = [\"^build\"]\nrun = \"pwd\"\n"
        ),
    );
    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    stdout(&out)
}

#[test]
fn upstream_deps_order_a_cargo_workspace() {
    let s = topo_order_of(
        "\"crates/*\"",
        &[
            (
                "crates/app",
                "Cargo.toml",
                "[package]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n"
                    .to_string(),
            ),
            (
                "crates/core",
                "Cargo.toml",
                "[package]\nname = \"core\"\n[dependencies]\nbase = { path = \"../base\" }\n"
                    .to_string(),
            ),
            (
                "crates/base",
                "Cargo.toml",
                "[package]\nname = \"base\"\n".to_string(),
            ),
        ],
    );
    assert!(
        line_of(&s, "crates/base") < line_of(&s, "crates/core"),
        "{s}"
    );
    assert!(
        line_of(&s, "crates/core") < line_of(&s, "crates/app"),
        "{s}"
    );
}

#[test]
fn upstream_deps_follow_cargo_target_specific_dependencies() {
    // A sibling declared only under `[target.'cfg(...)']` is still an edge.
    let s = topo_order_of(
        "\"crates/*\"",
        &[
            (
                "crates/app",
                "Cargo.toml",
                "[package]\nname = \"app\"\n\
                 [target.'cfg(unix)'.dependencies]\nplat = { path = \"../plat\" }\n"
                    .to_string(),
            ),
            (
                "crates/plat",
                "Cargo.toml",
                "[package]\nname = \"plat\"\n".to_string(),
            ),
        ],
    );
    assert!(
        line_of(&s, "crates/plat") < line_of(&s, "crates/app"),
        "{s}"
    );
}

#[test]
fn upstream_deps_order_a_go_workspace() {
    let s = topo_order_of(
        "\"mods/*\"",
        &[
            (
                "mods/api",
                "go.mod",
                "module example.com/api\n\ngo 1.22\n\nrequire (\n\texample.com/lib v0.1.0\n)\n"
                    .to_string(),
            ),
            (
                "mods/lib",
                "go.mod",
                "module example.com/lib\n\ngo 1.22\n".to_string(),
            ),
        ],
    );
    assert!(line_of(&s, "mods/lib") < line_of(&s, "mods/api"), "{s}");
}

#[test]
fn upstream_deps_order_a_python_workspace() {
    let s = topo_order_of(
        "\"pkgs/*\"",
        &[
            (
                "pkgs/app",
                "pyproject.toml",
                "[project]\nname = \"app\"\ndependencies = [\"core>=1.0\"]\n".to_string(),
            ),
            (
                "pkgs/core",
                "pyproject.toml",
                "[project]\nname = \"core\"\n".to_string(),
            ),
        ],
    );
    assert!(line_of(&s, "pkgs/core") < line_of(&s, "pkgs/app"), "{s}");
}

// --- --no-bail, --reporter, --resume-from ------------------------------------

#[test]
fn no_bail_keeps_running_siblings_after_a_failure() {
    let ws = workspace();
    write(&ws, "packages/a/package.json", &manifest("a", &[]));
    write(&ws, "packages/b/package.json", &manifest("b", &[]));
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n\
         [tasks.ci]\ndeps = [\"boom\", \"after\"]\n\
         [tasks.boom]\nrun = \"false\"\n\
         [tasks.after]\nrun = \"echo AFTER-RAN\"\n",
    );

    // Default: fail-fast — `after` is never launched.
    let out = tsr(&ws, &["ci"]);
    assert_eq!(code(&out), 1, "stderr {}", stderr(&out));
    assert!(!stdout(&out).contains("AFTER-RAN"), "{}", stdout(&out));

    // --no-bail: the sibling still runs, and the failing code still propagates.
    let out = tsr(&ws, &["ci", "--no-bail"]);
    assert_eq!(code(&out), 1, "stderr {}", stderr(&out));
    assert!(stdout(&out).contains("AFTER-RAN"), "{}", stdout(&out));
}

// Names `sh` outright to produce an exact non-zero code, so it is Unix-only —
// the cross-platform half of --no-bail is covered by the test above.
#[cfg(unix)]
#[test]
fn no_bail_still_reports_the_failing_exit_code() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.ci]\ndeps = [\"a\", \"b\"]\n\
         [tasks.a]\ndelegate = { bin = \"sh\", args = [\"-c\", \"exit 3\"] }\n\
         [tasks.b]\nrun = \"true\"\n",
    );
    let out = tsr(&ws, &["ci", "--no-bail"]);
    assert_eq!(code(&out), 3, "stderr {}", stderr(&out));
}

#[test]
fn ndjson_reporter_emits_one_json_object_per_line_on_stderr() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.ci]\ndeps = [\"a\", \"b\"]\n\
         [tasks.a]\nrun = \"true\"\n[tasks.b]\nrun = \"true\"\n",
    );
    let out = tsr(&ws, &["ci", "--reporter", "ndjson"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));

    let lines: Vec<serde_json::Value> = stderr(&out)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l:?} ({e})")))
        .collect();
    assert!(
        lines.len() >= 3,
        "expected task events + summary: {lines:?}"
    );

    let summary = lines.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(summary["status"], "ok");
    assert_eq!(summary["exitCode"], 0);
    assert_eq!(summary["task"], "ci");
    assert!(summary["durationMs"].is_number(), "{summary}");

    let tasks: Vec<&serde_json::Value> = lines.iter().filter(|l| l["type"] == "task").collect();
    assert!(tasks.iter().all(|t| t["status"] == "ok"), "{tasks:?}");
}

#[test]
fn ndjson_reporter_reports_failure_and_exit_code() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.boom]\nrun = \"false\"\n");
    let out = tsr(&ws, &["boom", "--reporter=ndjson"]);
    assert_eq!(code(&out), 1);

    let last: serde_json::Value = stderr(&out)
        .lines()
        .rfind(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .expect("a summary line");
    assert_eq!(last["status"], "failed");
    assert_eq!(last["exitCode"], 1);
    assert_eq!(last["failed"], 1);
}

#[test]
fn resume_from_skips_packages_ordered_before_it() {
    let ws = workspace();
    // tokens → ui → web
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(
        &ws,
        "packages/ui/package.json",
        &manifest("ui", &["tokens"]),
    );
    write(
        &ws,
        "packages/tokens/package.json",
        &manifest("tokens", &[]),
    );
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\", \"packages/*\"]\n\
         deps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build", "--resume-from", "packages/ui"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    // tokens precedes ui, so it is treated as already built — even though web
    // reaches it as an upstream dependency.
    assert!(
        !ran(&s, "packages/tokens"),
        "tokens should be skipped:\n{s}"
    );
    assert!(ran(&s, "packages/ui"), "the resume point must run:\n{s}");
    assert!(ran(&s, "apps/web"), "dependents must still run:\n{s}");
}

#[test]
fn resume_from_accepts_a_manifest_name() {
    let ws = workspace();
    write(&ws, "apps/web/package.json", &manifest("web", &["ui"]));
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"apps/*\", \"packages/*\"]\n\
         [tasks.build]\npackages = [\"apps/*\", \"packages/*\"]\n\
         deps = [\"^build\"]\nrun = \"pwd\"\n",
    );
    let out = tsr(&ws, &["build", "--resume-from=web"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(!ran(&s, "packages/ui"), "{s}");
    assert!(ran(&s, "apps/web"), "{s}");
}

#[test]
fn resume_from_an_unknown_package_is_a_runner_error() {
    let ws = workspace();
    write(&ws, "packages/ui/package.json", &manifest("ui", &[]));
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"packages/*\"]\n\
         [tasks.build]\npackages = [\"packages/*\"]\nrun = \"pwd\"\n",
    );
    let out = tsr(&ws, &["build", "--resume-from", "packages/nope"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(
        stderr(&out).contains("matched no workspace package"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn upstream_deps_follow_cargo_workspace_inheritance() {
    // `alias = { workspace = true }` where the root renames it to `real`: the
    // member's key is not the crate name, so resolving through the workspace
    // root is what produces the edge at all.
    let ws = workspace();
    write(
        &ws,
        "Cargo.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n[workspace.dependencies]\n\
         plain = { path = \"crates/plain\" }\n\
         alias = { package = \"real\", path = \"crates/real\" }\n",
    );
    write(
        &ws,
        "crates/app/Cargo.toml",
        "[package]\nname = \"app\"\n[dependencies]\n\
         plain = { workspace = true }\nalias = { workspace = true }\n",
    );
    write(
        &ws,
        "crates/plain/Cargo.toml",
        "[package]\nname = \"plain\"\n",
    );
    write(
        &ws,
        "crates/real/Cargo.toml",
        "[package]\nname = \"real\"\n",
    );
    write(
        &ws,
        "tasks.toml",
        "[workspace]\nmembers = [\"crates/*\"]\n\
         [tasks.build]\npackages = [\"crates/*\"]\ndeps = [\"^build\"]\nrun = \"pwd\"\n",
    );

    let out = tsr(&ws, &["build"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    let s = stdout(&out);
    assert!(
        line_of(&s, "crates/plain") < line_of(&s, "crates/app"),
        "{s}"
    );
    assert!(
        line_of(&s, "crates/real") < line_of(&s, "crates/app"),
        "{s}"
    );
}

/// The reason `--reporter-file` exists: a child that logs JSON to stderr is
/// indistinguishable from a reporter event when both share a stream. The file
/// sink is written by nobody else, so it stays parseable.
#[cfg(unix)]
#[test]
fn reporter_file_is_not_polluted_by_child_output() {
    let ws = workspace();
    let noisy = ws.join("noisy.sh");
    write(
        &ws,
        "noisy.sh",
        "#!/bin/sh\n\
         echo 'warning: unused variable x' >&2\n\
         echo '{\"level\":\"info\",\"type\":\"summary\",\"msg\":\"child log\"}' >&2\n\
         exit 0\n",
    );
    fs::set_permissions(
        &noisy,
        <fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();
    write(
        &ws,
        "tasks.toml",
        &format!("[tasks.ci]\nrun = \"{}\"\n", noisy.display()),
    );

    let out = tsr(&ws, &["ci", "--reporter-file", "results.ndjson"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));

    // The child's noise reached the terminal…
    assert!(
        stderr(&out).contains("unused variable x"),
        "{}",
        stderr(&out)
    );

    // …but every line of the file is one of *our* events.
    let text = fs::read_to_string(ws.join("results.ndjson")).unwrap();
    let events: Vec<serde_json::Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("not JSON: {l:?} ({e})")))
        .collect();
    assert!(!events.is_empty(), "no events written");
    assert!(
        !text.contains("child log"),
        "child output leaked into the reporter file:\n{text}"
    );
    let summaries: Vec<&serde_json::Value> =
        events.iter().filter(|e| e["type"] == "summary").collect();
    assert_eq!(summaries.len(), 1, "exactly one summary: {events:?}");
    assert_eq!(summaries[0]["exitCode"], 0);
}

#[test]
fn reporter_file_works_without_the_ndjson_reporter() {
    // The sinks are independent: the terminal keeps the human reporter while the
    // file gets the machine-readable stream.
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.ok]\nrun = \"true\"\n");
    let out = tsr(&ws, &["ok", "--reporter-file", "events.ndjson"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    // Human reporter prints nothing on success, so stderr carries no events.
    assert!(!stderr(&out).contains("\"type\""), "{}", stderr(&out));

    let text = fs::read_to_string(ws.join("events.ndjson")).unwrap();
    assert!(text.contains("\"type\":\"summary\""), "{text}");
}

#[test]
fn reporter_file_records_failures_too() {
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.boom]\nrun = \"false\"\n");
    let out = tsr(&ws, &["boom", "--reporter-file", "events.ndjson"]);
    assert_eq!(code(&out), 1);

    let text = fs::read_to_string(ws.join("events.ndjson")).unwrap();
    let last: serde_json::Value =
        serde_json::from_str(text.lines().rfind(|l| !l.trim().is_empty()).unwrap()).unwrap();
    assert_eq!(last["status"], "failed");
    assert_eq!(last["exitCode"], 1);
    assert_eq!(last["failed"], 1);
}

#[test]
fn an_uncreatable_reporter_file_fails_before_running_anything() {
    // Discovering the sink is unwritable *after* a long build would be useless.
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.mark]\nrun = \"touch ran.txt\"\n");
    let out = tsr(&ws, &["mark", "--reporter-file", "no/such/dir/out.ndjson"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("reporter file"), "{}", stderr(&out));
    assert!(!ws.join("ran.txt").exists(), "the task must not have run");
}

// --- --dry-run (SPEC §12) ---

#[test]
fn dry_run_prints_the_plan_and_runs_nothing() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.build]\nrun = \"touch built.txt\"\n\
         [tasks.ci]\ndeps = [\"build\"]\nrun = \"touch ran.txt\"\n",
    );
    let out = tsr(&ws, &["ci", "--dry-run"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));

    let text = stdout(&out);
    // Both the dependency and the task itself are shown, in execution order.
    let build = text.find("touch built.txt").expect("dep missing from plan");
    let ci = text.find("touch ran.txt").expect("task missing from plan");
    assert!(
        build < ci,
        "deps must print before their dependent:\n{text}"
    );
    // And nothing actually ran.
    assert!(!ws.join("built.txt").exists());
    assert!(!ws.join("ran.txt").exists());
}

#[test]
fn dry_run_does_not_expand_env_into_the_plan() {
    // The plan is meant to be safe to paste into an issue or a CI log, so it
    // prints the command as written — never the value a `.env` supplied.
    let ws = workspace();
    write(&ws, ".env", "TOKEN=hunter2-should-not-leak\n");
    write(&ws, "tasks.toml", "[tasks.deploy]\nrun = \"echo $TOKEN\"\n");
    let out = tsr(&ws, &["deploy", "--dry-run"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));

    let text = format!("{}{}", stdout(&out), stderr(&out));
    assert!(
        !text.contains("hunter2-should-not-leak"),
        "an env value reached the plan:\n{text}"
    );
    assert!(
        text.contains("$TOKEN"),
        "the plan should show the variable as written:\n{text}"
    );
}

#[test]
fn dry_run_forwards_args_and_passthrough_into_the_plan() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.test]\nrun = \"vitest\"\nargs = [\"--color\"]\n",
    );
    let out = tsr(&ws, &["test", "--dry-run", "--", "--watch"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(
        stdout(&out).contains("vitest --color --watch"),
        "{}",
        stdout(&out)
    );
}

#[test]
fn dry_run_still_reports_a_broken_config() {
    // A dry run is for *inspecting* an unfamiliar config, so a config that
    // cannot be resolved must still fail rather than print a bogus plan.
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.a]\ndeps = [\"nope\"]\n");
    let out = tsr(&ws, &["a", "--dry-run"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
}

// --- workspace confinement (SPEC §12.1) ---

#[test]
fn a_builtin_refuses_to_delete_outside_the_workspace() {
    // `rm` is tsr itself, not `/bin/rm`, and it always wins over a binary of the
    // same name — so this check is the only thing between a stray `../` and
    // whatever sits next to the repo.
    let ws = workspace();
    let outside = ws.join("outside");
    fs::create_dir_all(&outside).unwrap();
    let victim = outside.join("keep.txt");
    fs::write(&victim, "precious").unwrap();

    let root = ws.join("repo");
    fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "tasks.toml",
        "[tasks.clean]\nrun = \"rm -rf ../outside/keep.txt\"\n",
    );

    let out = tsr(&root, &["clean"]);
    assert_ne!(code(&out), 0, "the run should have failed");
    assert!(
        stderr(&out).contains("outside the workspace"),
        "{}",
        stderr(&out)
    );
    assert!(
        victim.is_file(),
        "the file outside the workspace was deleted"
    );
}

#[test]
fn allow_paths_permits_a_builtin_outside_the_workspace() {
    let ws = workspace();
    let cache = ws.join("cache");
    fs::create_dir_all(&cache).unwrap();
    let root = ws.join("repo");
    fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "tasks.toml",
        "[security]\nallow_paths = [\"../cache\"]\n\n\
         [tasks.stamp]\nrun = \"touch ../cache/stamp\"\n",
    );

    let out = tsr(&root, &["stamp"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(cache.join("stamp").is_file());
}

#[test]
fn a_task_dir_outside_the_workspace_is_rejected_at_load() {
    let ws = workspace();
    let root = ws.join("repo");
    fs::create_dir_all(root.join("sub")).unwrap();
    write(
        &root,
        "tasks.toml",
        "[tasks.build]\nrun = \"touch marker\"\ndir = \"../\"\n",
    );

    let out = tsr(&root, &["build"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(
        stderr(&out).contains("outside the workspace"),
        "{}",
        stderr(&out)
    );
    assert!(!ws.join("marker").exists(), "nothing should have run");
}

#[test]
fn an_env_file_outside_the_workspace_is_rejected_at_load() {
    let ws = workspace();
    fs::write(ws.join("secrets.env"), "TOKEN=leak\n").unwrap();
    let root = ws.join("repo");
    fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "tasks.toml",
        "[tasks.deploy]\nrun = \"true\"\nenv_file = \"../secrets.env\"\n",
    );

    let out = tsr(&root, &["deploy"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(
        stderr(&out).contains("outside the workspace"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn workspace_members_outside_the_workspace_are_rejected() {
    let ws = workspace();
    let root = ws.join("repo");
    fs::create_dir_all(&root).unwrap();
    write(
        &root,
        "tasks.toml",
        "[workspace]\nmembers = [\"../*\"]\n\n[tasks.test]\npackages = [\"*\"]\n",
    );

    let out = tsr(&root, &["test"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(
        stderr(&out).contains("outside the workspace"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_glob_inside_the_workspace_is_still_allowed() {
    // The prefix check must not turn ordinary globs into errors.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.clean]\nrun = \"rm -rf dist/*\"\n",
    );
    write(&ws, "dist/a.js", "");
    let out = tsr(&ws, &["clean"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(!ws.join("dist/a.js").exists());
}

// --- guarded environment variables (SPEC §12.2) ---

#[test]
fn a_config_cannot_set_a_guarded_variable() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[env]\nLD_PRELOAD = \"./evil.so\"\n\n[tasks.test]\nrun = \"touch ran.txt\"\n",
    );
    let out = tsr(&ws, &["test"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("LD_PRELOAD"), "{}", stderr(&out));
    assert!(
        stderr(&out).contains("--allow-unsafe-env"),
        "the error must name the opt-in: {}",
        stderr(&out)
    );
    assert!(!ws.join("ran.txt").exists(), "nothing should have run");
}

#[test]
fn a_dotenv_cannot_set_a_guarded_variable() {
    // `.env` is the source people read least and commit most often.
    let ws = workspace();
    write(&ws, ".env", "NODE_OPTIONS=--require ./evil.js\n");
    write(&ws, "tasks.toml", "[tasks.test]\nrun = \"touch ran.txt\"\n");
    let out = tsr(&ws, &["test"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("NODE_OPTIONS"), "{}", stderr(&out));
    assert!(!ws.join("ran.txt").exists());
}

#[test]
fn allow_unsafe_env_lifts_the_guard() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[env]\nNODE_OPTIONS = \"--max-old-space-size=4096\"\n\n\
         [tasks.test]\nrun = \"touch ran.txt\"\n",
    );
    let out = tsr(&ws, &["test", "--allow-unsafe-env"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(ws.join("ran.txt").is_file());
}

#[test]
fn path_may_be_extended_but_not_replaced() {
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[env]\nPATH = \"./bin:$PATH\"\n\n[tasks.test]\nrun = \"touch ran.txt\"\n",
    );
    assert_eq!(code(&tsr(&ws, &["test"])), 0, "extending PATH must work");
    assert!(ws.join("ran.txt").is_file());

    let ws2 = workspace();
    write(
        &ws2,
        "tasks.toml",
        "[env]\nPATH = \"/only/mine\"\n\n[tasks.test]\nrun = \"touch ran.txt\"\n",
    );
    let out = tsr(&ws2, &["test"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("PATH"), "{}", stderr(&out));
    assert!(!ws2.join("ran.txt").exists());
}

// --- discovery boundary (SPEC §12.3) ---

#[test]
fn discovery_does_not_climb_past_the_repository() {
    // A tasks.toml above a repository must not govern it — otherwise one left
    // in /tmp or a home directory silently owns every project beneath it.
    let ws = workspace();
    write(
        &ws,
        "tasks.toml",
        "[tasks.pwn]\nrun = \"touch pwned.txt\"\n",
    );
    let repo = ws.join("repo");
    fs::create_dir_all(repo.join(".git")).unwrap();
    fs::create_dir_all(repo.join("src")).unwrap();

    let out = tsr(&repo.join("src"), &["pwn"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(!ws.join("pwned.txt").exists());

    // The repository's own config is still found from a nested directory.
    write(
        &repo,
        "tasks.toml",
        "[tasks.own]\nrun = \"touch ran.txt\"\n",
    );
    let out = tsr(&repo.join("src"), &["own"]);
    assert_eq!(code(&out), 0, "stderr {}", stderr(&out));
    assert!(repo.join("ran.txt").is_file());
}

#[test]
#[cfg(unix)]
fn a_world_writable_config_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    write(&ws, "tasks.toml", "[tasks.t]\nrun = \"touch ran.txt\"\n");
    let cfg = ws.join("tasks.toml");
    fs::set_permissions(&cfg, fs::Permissions::from_mode(0o666)).unwrap();

    let out = tsr(&ws, &["t"]);
    assert_eq!(code(&out), 64, "stdout {}", stdout(&out));
    assert!(stderr(&out).contains("world-writable"), "{}", stderr(&out));
    assert!(!ws.join("ran.txt").exists());
}
