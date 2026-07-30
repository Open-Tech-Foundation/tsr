//! Process-tree containment and interrupt handling (SPEC §5.2, §12).
//!
//! Two problems that only appear once `tsr` starts *killing* things.
//!
//! **Orphans.** A fail-fast abort kills the child `tsr` spawned — but that child
//! is usually a launcher, not the work: `npm run dev` is a Node process that
//! spawns `vite`, and killing the former leaves the latter holding a port. So a
//! child that may have to be killed is spawned into its own process group (unix)
//! or job object (windows), and the *group* is what gets torn down.
//!
//! Isolation is not free, though: a unix process group other than the terminal's
//! foreground one is stopped with `SIGTTIN` the moment it reads stdin, which
//! would break every interactive task (`tsr dev`, `tsr test -- --watch`). It is
//! therefore applied only when the run contains parallelism — which is exactly
//! when `tsr` can be the one doing the killing, and never when a task is alone
//! on the terminal. See [`Isolation`].
//!
//! **Interrupts.** Without a handler, Ctrl-C tears `tsr` down mid-`wait()` and
//! whatever it had spawned is inherited by init. The handler here only flips an
//! atomic (all an async-signal-safe handler may do); the run loop polls it, tears
//! its children down through the same path a fail-fast uses, and exits `130`. A
//! second interrupt bypasses all of it and exits immediately, so a wedged child
//! can never trap the terminal.

use std::process::{Child, Command};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Exit code for a run cut short by an interrupt: the shell convention of
/// `128 + SIGINT`, which is also what a child killed by Ctrl-C reports.
pub const EXIT_INTERRUPTED: i32 = 130;

/// How long a terminated group gets to exit on its own before it is killed
/// outright. Long enough for a dev server to close its listeners, short enough
/// that Ctrl-C still feels immediate.
const GRACE: Duration = Duration::from_millis(2000);
/// Poll interval while waiting out [`GRACE`].
const GRACE_POLL: Duration = Duration::from_millis(10);

static INTERRUPTED: AtomicBool = AtomicBool::new(false);

/// Whether an interrupt has been received. Polled by the run loop, which then
/// aborts in flight work the same way a task failure does.
pub fn interrupted() -> bool {
    INTERRUPTED.load(Ordering::Relaxed)
}

/// Install the Ctrl-C handler. Idempotent, and a failure to install is ignored:
/// losing the graceful path is not a reason to refuse to run.
pub fn install_interrupt_handler() {
    imp::install();
}

/// Whether a child is spawned into its own process group / job object.
///
/// `Isolated` is what makes a kill reach a whole process tree, but on unix it
/// also moves the child out of the terminal's foreground group, so anything it
/// reads from stdin raises `SIGTTIN`. Runs without parallelism cannot reach the
/// kill path at all — nothing else is executing to abort them — so they keep the
/// inherited group and stay interactive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Isolation {
    /// Own process group / job object; teardown reaches descendants.
    Isolated,
    /// The group `tsr` itself is in; teardown reaches the direct child only.
    Inherited,
}

/// A spawned child's containment, created before the spawn and attached after.
pub struct Contained(imp::Handle);

/// Prepare `cmd` so its process tree can be torn down as a unit, per `mode`.
/// Call before `spawn`, then [`Contained::attach`] on the resulting child.
pub fn contain(cmd: &mut Command, mode: Isolation) -> Contained {
    Contained(imp::prepare(cmd, mode))
}

impl Contained {
    /// Finish containment once the child exists. A no-op on unix, where the
    /// group is established by the child itself before it execs.
    pub fn attach(&mut self, child: &Child) {
        imp::attach(&mut self.0, child);
    }

    /// Terminate `child` and everything it started, then reap it.
    ///
    /// Escalates: a polite signal first, then an unconditional kill once
    /// [`GRACE`] has passed, so a child that ignores the first one still dies.
    pub fn terminate(&self, child: &mut Child) {
        imp::signal(&self.0, child);
        let deadline = Instant::now() + GRACE;
        while Instant::now() < deadline {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(GRACE_POLL),
                Err(_) => break,
            }
        }
        imp::kill(&self.0, child);
        // Reap, so the child never lingers as a zombie.
        let _ = child.wait();
    }
}

// --- unix ---

#[cfg(unix)]
mod imp {
    use super::{INTERRUPTED, Isolation};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};
    use std::sync::atomic::Ordering;

    pub enum Handle {
        /// The child leads its own group; its pid *is* the group id.
        Group,
        Inherited,
    }

    pub fn prepare(cmd: &mut Command, mode: Isolation) -> Handle {
        match mode {
            Isolation::Isolated => {
                // `0` means "a new group led by the child", so the group id is
                // the child's pid — which is what `killpg` is then given.
                cmd.process_group(0);
                Handle::Group
            }
            Isolation::Inherited => Handle::Inherited,
        }
    }

    pub fn attach(_handle: &mut Handle, _child: &Child) {}

    pub fn signal(handle: &Handle, child: &mut Child) {
        send(handle, child, libc::SIGTERM);
    }

    pub fn kill(handle: &Handle, child: &mut Child) {
        send(handle, child, libc::SIGKILL);
    }

    fn send(handle: &Handle, child: &mut Child, sig: i32) {
        match handle {
            // SAFETY: `killpg` only signals; the pid is still ours (unreaped, so
            // it cannot have been reused), and an already-dead group is a
            // harmless `ESRCH`.
            Handle::Group => unsafe {
                libc::killpg(child.id() as i32, sig);
            },
            Handle::Inherited if sig == libc::SIGKILL => {
                let _ = child.kill();
            }
            // Without a group of its own there is nothing to escalate through:
            // the polite signal goes to the one child, and `Child::kill` (which
            // is `SIGKILL`) follows if it is still alive.
            Handle::Inherited => unsafe {
                libc::kill(child.id() as i32, libc::SIGTERM);
            },
        }
    }

    /// Flip the interrupt flag; a second interrupt leaves immediately.
    ///
    /// Only an atomic load/store and `_exit`, both async-signal-safe.
    extern "C" fn on_interrupt(_sig: i32) {
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            unsafe { libc::_exit(super::EXIT_INTERRUPTED) };
        }
    }

    pub fn install() {
        for sig in [libc::SIGINT, libc::SIGTERM] {
            // SAFETY: installing a handler that only touches an atomic.
            unsafe { libc::signal(sig, on_interrupt as *const () as libc::sighandler_t) };
        }
    }
}

// --- windows ---

#[cfg(windows)]
mod imp {
    use super::{INTERRUPTED, Isolation};
    use std::os::windows::io::AsRawHandle;
    use std::process::{Child, Command};
    use std::sync::atomic::Ordering;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::Console::{
        CTRL_BREAK_EVENT, CTRL_C_EVENT, SetConsoleCtrlHandler,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub enum Handle {
        /// A job object the child is assigned to. Killing the job kills every
        /// process in it, which is the whole tree.
        Job(HANDLE),
        Inherited,
    }

    // The handle is only ever touched by the thread that spawned the child.
    unsafe impl Send for Handle {}

    impl Drop for Handle {
        fn drop(&mut self) {
            if let Handle::Job(job) = *self {
                // SAFETY: a handle this module created and has not closed.
                unsafe { CloseHandle(job) };
            }
        }
    }

    pub fn prepare(_cmd: &mut Command, mode: Isolation) -> Handle {
        if mode == Isolation::Inherited {
            return Handle::Inherited;
        }
        // SAFETY: a plain create-then-configure of an unnamed job object; a
        // failure at any step falls back to killing the direct child only.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Handle::Inherited;
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            // Tie the tree's lifetime to the handle, so children cannot outlive
            // `tsr` even if it dies before it can tear them down.
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&info).cast(),
                size_of_val(&info) as u32,
            );
            Handle::Job(job)
        }
    }

    /// Assign the child to its job. Done after the spawn because `std` gives no
    /// pre-exec hook here; a grandchild started in the interval between the two
    /// escapes the job, which is a window of microseconds.
    pub fn attach(handle: &mut Handle, child: &Child) {
        if let Handle::Job(job) = *handle {
            // SAFETY: both handles are live — the child has not been reaped.
            unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as HANDLE) };
        }
    }

    /// Windows has no graceful per-process signal that works for non-console
    /// children, so the polite step is a no-op and teardown is the kill below.
    pub fn signal(_handle: &Handle, _child: &mut Child) {}

    pub fn kill(handle: &Handle, child: &mut Child) {
        match handle {
            // SAFETY: a live job handle owned by this module.
            Handle::Job(job) => unsafe {
                TerminateJobObject(*job, 1);
            },
            Handle::Inherited => {
                let _ = child.kill();
            }
        }
    }

    unsafe extern "system" fn on_interrupt(ctrl_type: u32) -> i32 {
        if ctrl_type != CTRL_C_EVENT && ctrl_type != CTRL_BREAK_EVENT {
            return 0; // not ours — let the default handler run
        }
        if INTERRUPTED.swap(true, Ordering::SeqCst) {
            std::process::exit(super::EXIT_INTERRUPTED);
        }
        1
    }

    pub fn install() {
        // SAFETY: registering a handler for the lifetime of the process.
        unsafe { SetConsoleCtrlHandler(Some(on_interrupt), 1) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An isolated child and everything it spawns must die together — the
    /// orphaned-dev-server case a bare `Child::kill` leaves behind.
    #[test]
    #[cfg(unix)]
    fn terminate_reaches_a_grandchild() {
        use std::process::Stdio;

        let marker = std::env::temp_dir().join(format!("tsr-proc-{}.pid", std::process::id()));
        let _ = std::fs::remove_file(&marker);
        // A shell that backgrounds a long sleep, records its pid and waits: the
        // sleep is the grandchild `Child::kill` cannot reach.
        let script = format!("sleep 30 & echo $! > {}; wait", marker.display());
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(&script)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut contained = contain(&mut cmd, Isolation::Isolated);
        let mut child = cmd.spawn().expect("sh should spawn");
        contained.attach(&child);

        // Wait for the grandchild's pid to appear.
        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Ok(text) = std::fs::read_to_string(&marker)
                && let Ok(pid) = text.trim().parse::<i32>()
            {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "grandchild never reported its pid"
            );
            std::thread::sleep(Duration::from_millis(20));
        };

        contained.terminate(&mut child);
        let _ = std::fs::remove_file(&marker);

        // SAFETY: signal 0 only probes for the process's existence.
        let alive = unsafe { libc::kill(pid, 0) } == 0;
        assert!(!alive, "grandchild {pid} survived the group teardown");
    }

    /// Without isolation the direct child is still killed — the interactive
    /// case, where the group is deliberately left alone.
    #[test]
    fn terminate_kills_an_inherited_child() {
        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", "timeout /T 30 /NOBREAK"]);
            c
        } else {
            let mut c = Command::new("sleep");
            c.arg("30");
            c
        };
        cmd.stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let mut contained = contain(&mut cmd, Isolation::Inherited);
        let Ok(mut child) = cmd.spawn() else {
            return; // no shell available; nothing to assert
        };
        contained.attach(&child);
        contained.terminate(&mut child);
        assert!(
            matches!(child.try_wait(), Ok(Some(_)) | Err(_)),
            "an inherited child must still be killed and reaped"
        );
    }
}
