//! Built-in commands (SPEC §8.5).
//!
//! A `run` string like `rm -rf dist` must behave identically on Linux, macOS and
//! Windows, but the coreutils it names are Unix binaries that simply do not
//! exist on Windows. So `tsr` implements the handful of file operations tasks
//! actually use, in-process, and **always** prefers its own implementation over
//! anything on `PATH`. One `run` string, one behaviour, everywhere.
//!
//! Builtins apply to `run` strings only. `delegate` is the explicit escape hatch
//! when a task genuinely needs the platform's own tool (SPEC §8.3).
//!
//! Paths are resolved against the job's directory rather than the process
//! working directory: builtins run in-process, and parallel jobs share that
//! process, so there is no per-job `cwd` to rely on.

use std::fs::{self, File, FileTimes, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Exit code for a usage error (bad flag, missing operand), as in coreutils.
const USAGE: i32 = 2;
/// Exit code for a failed operation.
const FAIL: i32 = 1;

/// Every command `tsr` implements itself.
pub const NAMES: &[&str] = &[
    "cat", "cp", "echo", "false", "mkdir", "mv", "pwd", "rm", "touch", "true",
];

/// Whether `name` is handled in-process rather than spawned.
pub fn is_builtin(name: &str) -> bool {
    NAMES.contains(&name)
}

/// Run a builtin and return its exit code. `cwd` is the job's directory, which
/// every relative path operand is resolved against.
pub fn run(name: &str, args: &[String], cwd: &Path) -> i32 {
    match name {
        "cat" => cat(args, cwd),
        "cp" => cp(args, cwd),
        "echo" => echo(args),
        // POSIX: both ignore their operands entirely. `cmd || true` is the
        // portable way to make a step non-fatal, so these have to exist on
        // Windows too, where there is no `/bin/true` to fall back on.
        "false" => FAIL,
        "true" => 0,
        "mkdir" => mkdir(args, cwd),
        "mv" => mv(args, cwd),
        "pwd" => pwd(cwd),
        "rm" => rm(args, cwd),
        "touch" => touch(args, cwd),
        _ => {
            errln(format_args!("tsr: '{name}' is not a builtin"));
            USAGE
        }
    }
}

// --- shared helpers ---

/// Resolve an operand against the job directory, leaving absolute paths alone.
fn resolve(cwd: &Path, arg: &str) -> PathBuf {
    let p = Path::new(arg);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        cwd.join(p)
    }
}

fn errln(args: std::fmt::Arguments) {
    let _ = writeln!(io::stderr(), "{args}");
}

/// Report a failed operation on `path` and yield the failure exit code.
fn fail(cmd: &str, path: &Path, e: &io::Error) -> i32 {
    errln(format_args!("{cmd}: {}: {e}", path.display()));
    FAIL
}

fn usage(cmd: &str, msg: &str) -> i32 {
    errln(format_args!("{cmd}: {msg}"));
    USAGE
}

/// Split `args` into the flags it sets and its operands.
///
/// `allowed` maps each accepted short flag to its long spelling (`""` when it
/// has none). Short flags bundle (`-rf`), `--` ends option parsing, and a lone
/// `-` is an operand. Returns the set of short letters seen, or `Err` with the
/// usage exit code when an unrecognised flag appears.
fn flags<'a>(
    cmd: &str,
    args: &'a [String],
    allowed: &[(char, &str)],
) -> Result<(String, Vec<&'a str>), i32> {
    let mut seen = String::new();
    let mut operands = Vec::new();
    let mut only_operands = false;

    fn accept(seen: &mut String, c: char) {
        if !seen.contains(c) {
            seen.push(c);
        }
    }

    for arg in args {
        if only_operands || arg == "-" || !arg.starts_with('-') {
            operands.push(arg.as_str());
            continue;
        }
        if arg == "--" {
            only_operands = true;
            continue;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match allowed.iter().find(|(_, l)| !l.is_empty() && *l == long) {
                Some((short, _)) => accept(&mut seen, *short),
                None => return Err(usage(cmd, &format!("unrecognised option '--{long}'"))),
            }
            continue;
        }
        for c in arg.chars().skip(1) {
            match allowed.iter().find(|(short, _)| *short == c) {
                Some((short, _)) => accept(&mut seen, *short),
                None => return Err(usage(cmd, &format!("invalid option -- '{c}'"))),
            }
        }
    }
    Ok((seen, operands))
}

/// Resolve `SRC... DST` operands into the (source, destination) pairs the
/// operation should perform, following the coreutils rule: with more than one
/// source — or a destination that is an existing directory — each source lands
/// *inside* the destination directory.
fn pairs(cmd: &str, operands: &[&str], cwd: &Path) -> Result<Vec<(PathBuf, PathBuf)>, i32> {
    let (srcs, dst) = match operands.split_last() {
        Some((dst, srcs)) if !srcs.is_empty() => (srcs, resolve(cwd, dst)),
        _ => return Err(usage(cmd, "missing destination operand")),
    };
    let into_dir = dst.is_dir();
    if srcs.len() > 1 && !into_dir {
        return Err(usage(
            cmd,
            &format!("target '{}' is not a directory", dst.display()),
        ));
    }
    Ok(srcs
        .iter()
        .map(|s| {
            let src = resolve(cwd, s);
            let target = match (into_dir, src.file_name()) {
                (true, Some(name)) => dst.join(name),
                _ => dst.clone(),
            };
            (src, target)
        })
        .collect())
}

// --- builtins ---

/// `echo [-n] [ARG]...` — arguments joined by a space, newline unless `-n`.
fn echo(args: &[String]) -> i32 {
    // Only a leading `-n` is a flag; everything after it is text, so
    // `echo --first` prints `--first` rather than erroring.
    let (newline, rest) = match args.split_first() {
        Some((first, rest)) if first == "-n" => (false, rest),
        _ => (true, args),
    };
    let mut out = rest.join(" ");
    if newline {
        out.push('\n');
    }
    match io::stdout().write_all(out.as_bytes()) {
        Ok(()) => 0,
        Err(e) => {
            errln(format_args!("echo: {e}"));
            FAIL
        }
    }
}

/// `pwd` — the job's working directory.
fn pwd(cwd: &Path) -> i32 {
    let _ = writeln!(io::stdout(), "{}", cwd.display());
    0
}

/// `cat [FILE]...` — concatenate to stdout; stdin when no operands.
fn cat(args: &[String], cwd: &Path) -> i32 {
    let (_, operands) = match flags("cat", args, &[]) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let mut out = io::stdout();
    if operands.is_empty() {
        return match io::copy(&mut io::stdin().lock(), &mut out) {
            Ok(_) => 0,
            Err(e) => {
                errln(format_args!("cat: {e}"));
                FAIL
            }
        };
    }
    let mut code = 0;
    for op in operands {
        let path = resolve(cwd, op);
        // A failed file does not abort the rest, matching `cat`.
        match File::open(&path).and_then(|mut f| io::copy(&mut f, &mut out)) {
            Ok(_) => {}
            Err(e) => code = fail("cat", &path, &e),
        }
    }
    code
}

/// `mkdir [-p] DIR...`
fn mkdir(args: &[String], cwd: &Path) -> i32 {
    let (set, operands) = match flags("mkdir", args, &[('p', "parents")]) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if operands.is_empty() {
        return usage("mkdir", "missing operand");
    }
    let parents = set.contains('p');
    let mut code = 0;
    for op in operands {
        let path = resolve(cwd, op);
        // `-p` also makes an existing directory a no-op, as in coreutils.
        let r = if parents {
            fs::create_dir_all(&path)
        } else {
            fs::create_dir(&path)
        };
        if let Err(e) = r {
            code = fail("mkdir", &path, &e);
        }
    }
    code
}

/// `touch FILE...` — create when missing, otherwise bump the timestamps.
fn touch(args: &[String], cwd: &Path) -> i32 {
    let (_, operands) = match flags("touch", args, &[]) {
        Ok(v) => v,
        Err(code) => return code,
    };
    if operands.is_empty() {
        return usage("touch", "missing file operand");
    }
    let now = SystemTime::now();
    let times = FileTimes::new().set_accessed(now).set_modified(now);
    let mut code = 0;
    for op in operands {
        let path = resolve(cwd, op);
        let r = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .and_then(|f| f.set_times(times));
        if let Err(e) = r {
            code = fail("touch", &path, &e);
        }
    }
    code
}

/// `rm [-r] [-f] FILE...`
fn rm(args: &[String], cwd: &Path) -> i32 {
    let (set, operands) = match flags("rm", args, &[('r', "recursive"), ('R', ""), ('f', "force")])
    {
        Ok(v) => v,
        Err(code) => return code,
    };
    let recursive = set.contains('r') || set.contains('R');
    let force = set.contains('f');

    if operands.is_empty() {
        // POSIX: `rm -f` with nothing to remove succeeds silently. This is what
        // makes `rm -rf dist/*` idempotent when `dist` is already empty.
        return if force {
            0
        } else {
            usage("rm", "missing operand")
        };
    }

    let mut code = 0;
    for op in operands {
        let path = resolve(cwd, op);
        // Refuse to recurse into a filesystem root, as coreutils does.
        if path.parent().is_none() {
            errln(format_args!(
                "rm: refusing to remove root directory '{}'",
                path.display()
            ));
            code = FAIL;
            continue;
        }
        // `symlink_metadata` so a symlink is removed, not its target.
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                if !force {
                    code = fail("rm", &path, &e);
                }
                continue;
            }
        };
        let r = if meta.is_dir() {
            if !recursive {
                errln(format_args!("rm: {}: is a directory", path.display()));
                code = FAIL;
                continue;
            }
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        // `-f` suppresses only the "no such file" case, handled above; a real
        // failure (permissions, a busy file) is still reported.
        if let Err(e) = r {
            code = fail("rm", &path, &e);
        }
    }
    code
}

/// `cp [-r] SRC... DST`
fn cp(args: &[String], cwd: &Path) -> i32 {
    let (set, operands) = match flags("cp", args, &[('r', "recursive"), ('R', "")]) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let recursive = set.contains('r') || set.contains('R');
    let jobs = match pairs("cp", &operands, cwd) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let mut code = 0;
    for (src, dst) in jobs {
        if src.is_dir() && !recursive {
            errln(format_args!(
                "cp: {}: is a directory (use -r)",
                src.display()
            ));
            code = FAIL;
            continue;
        }
        if let Err(e) = copy_tree(&src, &dst) {
            code = fail("cp", &src, &e);
        }
    }
    code
}

/// `mv SRC... DST`
fn mv(args: &[String], cwd: &Path) -> i32 {
    let (_, operands) = match flags("mv", args, &[]) {
        Ok(v) => v,
        Err(code) => return code,
    };
    let jobs = match pairs("mv", &operands, cwd) {
        Ok(v) => v,
        Err(code) => return code,
    };

    let mut code = 0;
    for (src, dst) in jobs {
        if fs::rename(&src, &dst).is_ok() {
            continue;
        }
        // A rename across filesystems fails on every platform; fall back to a
        // copy-then-delete so `mv build/out /tmp/x` still works.
        let r = copy_tree(&src, &dst).and_then(|()| {
            if src.is_dir() {
                fs::remove_dir_all(&src)
            } else {
                fs::remove_file(&src)
            }
        });
        if let Err(e) = r {
            code = fail("mv", &src, &e);
        }
    }
    code
}

/// Copy a file, or a directory tree, to `dst`.
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    if fs::symlink_metadata(src)?.is_dir() {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            copy_tree(&entry.path(), &dst.join(entry.file_name()))?;
        }
        Ok(())
    } else {
        fs::copy(src, dst).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch() -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let id = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("tsr-builtin-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn call(name: &str, list: &[&str], cwd: &Path) -> i32 {
        run(name, &args(list), cwd)
    }

    fn write(dir: &Path, rel: &str, body: &str) -> PathBuf {
        let p = dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn true_and_false_are_portable_exit_codes() {
        let dir = scratch();
        assert_eq!(call("true", &[], &dir), 0);
        assert_eq!(call("false", &[], &dir), FAIL);
        // POSIX: operands are ignored rather than parsed as flags.
        assert_eq!(call("true", &["--anything", "x"], &dir), 0);
        assert_eq!(call("false", &["--anything", "x"], &dir), FAIL);
    }

    #[test]
    fn only_listed_names_are_builtins() {
        for name in NAMES {
            assert!(is_builtin(name), "{name} is listed but not recognised");
        }
        for name in ["vite", "cargo", "npm", "ls", "sh", "rmdir"] {
            assert!(!is_builtin(name), "{name} must be spawned, not built in");
        }
    }

    // --- rm ---

    #[test]
    fn rm_removes_files_and_trees() {
        let dir = scratch();
        write(&dir, "a.txt", "");
        write(&dir, "tree/nested/b.txt", "");
        assert_eq!(call("rm", &["a.txt"], &dir), 0);
        assert!(!dir.join("a.txt").exists());
        assert_eq!(call("rm", &["-r", "tree"], &dir), 0);
        assert!(!dir.join("tree").exists());
    }

    #[test]
    fn rm_needs_r_for_a_directory() {
        let dir = scratch();
        fs::create_dir(dir.join("d")).unwrap();
        assert_eq!(call("rm", &["d"], &dir), FAIL);
        assert!(dir.join("d").exists());
    }

    #[test]
    fn rm_force_ignores_missing_and_empty_operands() {
        let dir = scratch();
        // The `rm -rf dist/*` case: an unmatched glob leaves a literal that
        // does not exist, and `-f` must make that a success.
        assert_eq!(call("rm", &["-rf", "dist/*"], &dir), 0);
        assert_eq!(call("rm", &["-f"], &dir), 0);
        assert_eq!(call("rm", &["missing"], &dir), FAIL);
        assert_eq!(call("rm", &[], &dir), USAGE);
    }

    #[test]
    fn rm_accepts_bundled_and_long_flags() {
        let dir = scratch();
        write(&dir, "t1/x", "");
        write(&dir, "t2/x", "");
        assert_eq!(call("rm", &["-rf", "t1"], &dir), 0);
        assert_eq!(call("rm", &["--recursive", "--force", "t2"], &dir), 0);
        assert!(!dir.join("t1").exists() && !dir.join("t2").exists());
    }

    #[test]
    fn rm_rejects_unknown_flags() {
        let dir = scratch();
        assert_eq!(call("rm", &["-z", "x"], &dir), USAGE);
        assert_eq!(call("rm", &["--zap", "x"], &dir), USAGE);
    }

    #[test]
    fn rm_treats_double_dash_as_end_of_options() {
        let dir = scratch();
        write(&dir, "-weird", "");
        assert_eq!(call("rm", &["--", "-weird"], &dir), 0);
        assert!(!dir.join("-weird").exists());
    }

    // --- mkdir / touch ---

    #[test]
    fn mkdir_p_creates_parents_and_is_idempotent() {
        let dir = scratch();
        assert_eq!(call("mkdir", &["-p", "a/b/c"], &dir), 0);
        assert!(dir.join("a/b/c").is_dir());
        assert_eq!(call("mkdir", &["-p", "a/b/c"], &dir), 0);
        // Without -p, a missing parent and an existing dir both fail.
        assert_eq!(call("mkdir", &["x/y"], &dir), FAIL);
        assert_eq!(call("mkdir", &["a"], &dir), FAIL);
        assert_eq!(call("mkdir", &[], &dir), USAGE);
    }

    #[test]
    fn touch_creates_without_truncating() {
        let dir = scratch();
        assert_eq!(call("touch", &["new.txt"], &dir), 0);
        assert!(dir.join("new.txt").is_file());
        write(&dir, "kept.txt", "body");
        assert_eq!(call("touch", &["kept.txt"], &dir), 0);
        assert_eq!(fs::read_to_string(dir.join("kept.txt")).unwrap(), "body");
        assert_eq!(call("touch", &[], &dir), USAGE);
    }

    #[test]
    fn touch_resolves_absolute_paths() {
        let dir = scratch();
        let other = scratch();
        let target = other.join("marker");
        assert_eq!(
            call("touch", &[target.to_str().unwrap()], &dir),
            0,
            "absolute operands must not be joined onto cwd"
        );
        assert!(target.is_file());
    }

    // --- cp / mv ---

    #[test]
    fn cp_copies_file_and_tree() {
        let dir = scratch();
        write(&dir, "src.txt", "body");
        assert_eq!(call("cp", &["src.txt", "dst.txt"], &dir), 0);
        assert_eq!(fs::read_to_string(dir.join("dst.txt")).unwrap(), "body");

        write(&dir, "tree/nested/leaf.txt", "deep");
        assert_eq!(call("cp", &["-r", "tree", "copy"], &dir), 0);
        assert_eq!(
            fs::read_to_string(dir.join("copy/nested/leaf.txt")).unwrap(),
            "deep"
        );
    }

    #[test]
    fn cp_needs_r_for_a_directory() {
        let dir = scratch();
        fs::create_dir(dir.join("d")).unwrap();
        assert_eq!(call("cp", &["d", "e"], &dir), FAIL);
    }

    #[test]
    fn cp_into_existing_directory_keeps_basenames() {
        let dir = scratch();
        write(&dir, "a.txt", "a");
        write(&dir, "b.txt", "b");
        fs::create_dir(dir.join("out")).unwrap();
        assert_eq!(call("cp", &["a.txt", "b.txt", "out"], &dir), 0);
        assert!(dir.join("out/a.txt").is_file() && dir.join("out/b.txt").is_file());
    }

    #[test]
    fn cp_rejects_multiple_sources_without_a_directory() {
        let dir = scratch();
        write(&dir, "a.txt", "a");
        write(&dir, "b.txt", "b");
        assert_eq!(call("cp", &["a.txt", "b.txt", "nope"], &dir), USAGE);
        assert_eq!(call("cp", &["only.txt"], &dir), USAGE);
    }

    #[test]
    fn mv_renames_and_moves_into_directory() {
        let dir = scratch();
        write(&dir, "a.txt", "a");
        assert_eq!(call("mv", &["a.txt", "b.txt"], &dir), 0);
        assert!(!dir.join("a.txt").exists());
        assert_eq!(fs::read_to_string(dir.join("b.txt")).unwrap(), "a");

        fs::create_dir(dir.join("out")).unwrap();
        assert_eq!(call("mv", &["b.txt", "out"], &dir), 0);
        assert!(dir.join("out/b.txt").is_file());
    }

    #[test]
    fn mv_reports_a_missing_source() {
        let dir = scratch();
        assert_eq!(call("mv", &["ghost.txt", "x.txt"], &dir), FAIL);
    }

    // --- echo / pwd / cat ---

    #[test]
    fn echo_joins_args_and_honours_leading_n() {
        // `echo` writes to the inherited stdout, so assert on the flag split
        // rather than capturing: only a leading `-n` is an option.
        assert_eq!(echo(&args(&["-n", "a", "b"])), 0);
        assert_eq!(echo(&args(&["--first", "--second"])), 0);
        assert_eq!(echo(&[]), 0);
    }

    #[test]
    fn cat_reports_a_missing_file_but_continues() {
        let dir = scratch();
        write(&dir, "a.txt", "a");
        assert_eq!(call("cat", &["a.txt"], &dir), 0);
        assert_eq!(call("cat", &["a.txt", "ghost.txt"], &dir), FAIL);
    }

    #[test]
    fn pwd_reports_the_job_directory() {
        let dir = scratch();
        assert_eq!(call("pwd", &[], &dir), 0);
    }
}
