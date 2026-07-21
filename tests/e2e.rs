//! End-to-end tests driving the compiled `tsr` binary against real temp
//! workspaces, asserting on exit codes and output (SPEC §5, §6, §7, §8, §10).

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
    use std::os::unix::fs::PermissionsExt;
    let ws = workspace();
    let bindir = ws.join("node_modules/.bin");
    fs::create_dir_all(&bindir).unwrap();
    let bin = bindir.join("mytool");
    fs::write(&bin, "#!/bin/sh\necho ran-local-tool\n").unwrap();
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755)).unwrap();

    write(&ws, "tasks.toml", "[tasks.lint]\nrun = \"mytool\"\n");
    let out = tsr(&ws, &["lint"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(stdout(&out).contains("ran-local-tool"), "{}", stdout(&out));
}

#[test]
fn init_scaffolds_a_runnable_config() {
    let ws = workspace();
    // No tasks.toml yet.
    let out = tsr(&ws, &["--init"]);
    assert_eq!(code(&out), 0, "stderr: {}", stderr(&out));
    assert!(ws.join("tasks.toml").exists());
    assert!(stdout(&out).contains("Created"));

    // The scaffolded `dev` task runs.
    let dev = tsr(&ws, &["dev"]);
    assert_eq!(code(&dev), 0, "stderr: {}", stderr(&dev));

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
