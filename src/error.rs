//! Error types and their mapping to process exit codes.
//!
//! Exit-code contract (SPEC §10):
//! - `0`            success
//! - child's code  a task's child process failed; its exact code is propagated
//! - `64`          runner-level error (config parse, validation, delegate binary
//!                 not found, undefined `$VAR`, rejected mini-shell metacharacter)

use std::fmt;

/// The runner-level exit code for any `tsr` error that is not a task's own
/// child-process failure (SPEC §10).
pub const EXIT_RUNNER_ERROR: i32 = 64;

/// A runner-level error. Every variant maps to exit code `64`; the distinction
/// between variants exists only to shape the user-facing message.
#[derive(Debug)]
pub enum TsrError {
    /// Config parse or validation failure (bad TOML, `dir`+`packages`, illegal
    /// task name, undefined `$VAR`, rejected metacharacter, …).
    Config(String),
    /// A failure discovered while running (delegate binary not found, missing
    /// task, I/O error spawning a child, …).
    Runtime(String),
}

impl TsrError {
    pub fn config(msg: impl Into<String>) -> Self {
        TsrError::Config(msg.into())
    }

    pub fn runtime(msg: impl Into<String>) -> Self {
        TsrError::Runtime(msg.into())
    }

    /// The process exit code this error maps to. All runner-level errors are
    /// `64`; task failures are represented separately (see [`Outcome`]).
    pub fn exit_code(&self) -> i32 {
        EXIT_RUNNER_ERROR
    }
}

impl fmt::Display for TsrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TsrError::Config(m) => write!(f, "✗ config error: {m}"),
            TsrError::Runtime(m) => write!(f, "✗ error: {m}"),
        }
    }
}

impl std::error::Error for TsrError {}

impl From<std::io::Error> for TsrError {
    fn from(e: std::io::Error) -> Self {
        TsrError::Runtime(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TsrError>;

/// The result of running the requested task tree. Distinguishes clean success
/// from a task's child failure (whose exact code we must propagate verbatim).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Success,
    /// A task's child process exited non-zero; carries that exact code.
    TaskFailed(i32),
}

impl Outcome {
    pub fn exit_code(self) -> i32 {
        match self {
            Outcome::Success => 0,
            Outcome::TaskFailed(code) => code,
        }
    }
}
