//! Running external programs (Wine, `wineserver`, `plutil`, `cp`, ...).
//!
//! Every program Uncork runs is described by a [`CommandSpec`] first. Plans
//! are plain data, so tests assert on them without running Wine, and
//! `uncork run --dry-run` prints them.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// A fully specified program invocation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    /// Absolute path of the program.
    pub program: PathBuf,
    /// Arguments, not including the program.
    pub args: Vec<OsString>,
    /// Variables set on top of the inherited environment. Sorted, so
    /// rendering is deterministic.
    pub env: BTreeMap<String, String>,
    /// Start from an empty environment plus the variables in
    /// [`crate::launch::ALLOWED_ENV`] copied from the parent and
    /// `PATH=`[`crate::launch::WINE_PATH`], instead of inheriting everything.
    pub env_clear: bool,
    /// Working directory; inherited when `None`.
    pub cwd: Option<PathBuf>,
    /// When set, stdout and stderr are appended to this file (created with
    /// parents) instead of being inherited.
    pub log: Option<PathBuf>,
}

impl CommandSpec {
    /// A spec for `program` with no args, env or cwd.
    pub fn new(program: impl Into<PathBuf>) -> CommandSpec {
        CommandSpec {
            program: program.into(),
            ..CommandSpec::default()
        }
    }

    /// Append one argument (builder style).
    #[must_use]
    pub fn arg(mut self, arg: impl Into<OsString>) -> CommandSpec {
        self.args.push(arg.into());
        self
    }

    /// Append arguments (builder style).
    #[must_use]
    pub fn args<I, S>(mut self, args: I) -> CommandSpec
    where
        I: IntoIterator<Item = S>,
        S: Into<OsString>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    /// Set one environment variable (builder style).
    #[must_use]
    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> CommandSpec {
        self.env.insert(key.into(), value.into());
        self
    }

    /// Render as a shell line a user can paste: `KEY='v' ... '/path/prog' 'arg'`,
    /// every token single-quoted with `'` escaped as `'\''`, `cd '<cwd>' && `
    /// prefix when `cwd` is set, `env -i` semantics noted as a leading
    /// `env -i HOME=... ` only when `env_clear` is set. Only `env` is shown,
    /// not the inherited environment.
    #[must_use]
    pub fn to_shell(&self) -> String {
        todo!()
    }

    /// Build a [`std::process::Command`] from this spec (log redirection
    /// included; the log file's parent directory is created).
    ///
    /// # Errors
    /// [`crate::Error::Io`] if the log file cannot be opened.
    pub fn to_command(&self) -> crate::Result<std::process::Command> {
        todo!()
    }
}

/// Run to completion. A non-zero exit becomes [`crate::Error::Command`] whose
/// message includes the log path when there is one.
///
/// # Errors
/// [`crate::Error::Command`] if the program cannot start or exits non-zero.
pub fn run(spec: &CommandSpec) -> crate::Result<()> {
    let _ = spec;
    todo!()
}

/// Run to completion and return stdout as UTF-8 (lossy), trimmed. `log` is
/// ignored; stderr is inherited.
///
/// # Errors
/// As [`run`].
pub fn output(spec: &CommandSpec) -> crate::Result<String> {
    let _ = spec;
    todo!()
}

/// Start without waiting and return the child. Stdin is null. Uses
/// `std::process::Command` (`posix_spawn` on macOS): never a shell, `env` or
/// `arch` wrapper, which would strip `DYLD_*` under SIP.
///
/// # Errors
/// [`crate::Error::Command`] if the program cannot start.
pub fn spawn(spec: &CommandSpec) -> crate::Result<std::process::Child> {
    let _ = spec;
    todo!()
}

/// `true` if `path` is a regular file with any execute bit set.
#[must_use]
pub fn is_executable(path: &Path) -> bool {
    let _ = path;
    todo!()
}
