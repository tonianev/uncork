//! Running external programs (Wine, `wineserver`, `plutil`, `cp`, ...).
//!
//! Every program Uncork runs is described by a [`CommandSpec`] first. Plans
//! are plain data, so tests assert on them without running Wine, and
//! `uncork run --dry-run` prints them.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use serde::Serialize;

use crate::Error;
use crate::launch::{ALLOWED_ENV, WINE_PATH};

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
    ///
    /// Precisely, with `env_clear` the line starts
    /// `env -i HOME="$HOME" USER="$USER" ... PATH='/usr/bin:/bin:/usr/sbin:/sbin'`:
    /// the [`crate::launch::ALLOWED_ENV`] variables expand in the pasting
    /// shell, so the output never contains this process's own values and is
    /// the same on every machine. Variables named in `env` are not repeated
    /// there. Without `env_clear`, a key that is not a shell name (such as
    /// `A-B`) makes the line start with `env ` and that assignment is quoted
    /// whole. Non-UTF-8 paths and arguments are rendered lossily.
    #[must_use]
    pub fn to_shell(&self) -> String {
        let mut words: Vec<String> = Vec::new();
        if let Some(cwd) = &self.cwd {
            words.extend(["cd".to_owned(), shell_quote(cwd), "&&".to_owned()]);
        }
        if self.env_clear {
            words.extend(["env".to_owned(), "-i".to_owned()]);
            words.extend(
                ALLOWED_ENV
                    .iter()
                    .filter(|name| !self.env.contains_key(**name))
                    .map(|name| format!("{name}=\"${name}\"")),
            );
            if !self.env.contains_key("PATH") {
                words.push(format!("PATH={}", shell_quote(WINE_PATH)));
            }
        } else if self.env.keys().any(|key| !is_shell_name(key)) {
            words.push("env".to_owned());
        }
        words.extend(
            self.env
                .iter()
                .map(|(key, value)| shell_assignment(key, value)),
        );
        words.push(shell_quote(&self.program));
        words.extend(self.args.iter().map(shell_quote));
        words.join(" ")
    }

    /// Build a [`std::process::Command`] from this spec (log redirection
    /// included; the log file's parent directory is created).
    ///
    /// # Errors
    /// [`crate::Error::Io`] if the log file cannot be opened.
    pub fn to_command(&self) -> crate::Result<std::process::Command> {
        let mut command = self.command_without_log();
        if let Some(log) = &self.log {
            let stdout = open_log(log)?;
            let stderr = stdout
                .try_clone()
                .map_err(|e| Error::io("cannot open log file", log, e))?;
            command.stdout(stdout).stderr(stderr);
        }
        Ok(command)
    }

    /// Program, arguments, environment and working directory; stdio untouched.
    fn command_without_log(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        if self.env_clear {
            command.env_clear();
            for name in ALLOWED_ENV {
                if let Some(value) = std::env::var_os(name) {
                    command.env(name, value);
                }
            }
            command.env("PATH", WINE_PATH);
        }
        command.envs(&self.env);
        if let Some(cwd) = &self.cwd {
            command.current_dir(cwd);
        }
        command
    }

    /// The program's file name, as error messages show it.
    fn program_name(&self) -> String {
        self.program.file_name().map_or_else(
            || self.program.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// [`Error::Command`] for a program that could not be started.
    fn start_error(&self, error: &io::Error) -> Error {
        Error::Command {
            program: self.program_name(),
            status: format!("could not start: {error}"),
            log_hint: String::new(),
        }
    }

    /// `Ok` for a successful exit, otherwise [`Error::Command`] naming `log`.
    fn check_exit(&self, status: ExitStatus, log: Option<&Path>) -> crate::Result<()> {
        if status.success() {
            return Ok(());
        }
        Err(Error::Command {
            program: self.program_name(),
            status: describe_exit(status),
            log_hint: log.map_or_else(String::new, |log| format!(" (log: {})", log.display())),
        })
    }
}

/// Run to completion. A non-zero exit becomes [`crate::Error::Command`] whose
/// message includes the log path when there is one.
///
/// # Errors
/// [`crate::Error::Command`] if the program cannot start or exits non-zero.
/// [`crate::Error::Io`] if the log file cannot be opened.
pub fn run(spec: &CommandSpec) -> crate::Result<()> {
    let status = spec
        .to_command()?
        .status()
        .map_err(|e| spec.start_error(&e))?;
    spec.check_exit(status, spec.log.as_deref())
}

/// Run to completion and return stdout as UTF-8 (lossy), trimmed. `log` is
/// ignored; stderr is inherited.
///
/// # Errors
/// As [`run`].
pub fn output(spec: &CommandSpec) -> crate::Result<String> {
    let output = spec
        .command_without_log()
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|e| spec.start_error(&e))?;
    spec.check_exit(output.status, None)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Start without waiting and return the child. Stdin is null. Uses
/// `std::process::Command` (`posix_spawn` on macOS): never a shell, `env` or
/// `arch` wrapper, which would strip `DYLD_*` under SIP.
///
/// # Errors
/// [`crate::Error::Command`] if the program cannot start.
/// [`crate::Error::Io`] if the log file cannot be opened.
pub fn spawn(spec: &CommandSpec) -> crate::Result<std::process::Child> {
    spec.to_command()?
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| spec.start_error(&e))
}

/// `true` if `path` is a regular file with any execute bit set.
#[must_use]
pub fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

/// Open `log` for appending, creating it and its parent directories.
fn open_log(log: &Path) -> crate::Result<File> {
    if let Some(parent) = log.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| Error::io("cannot create directory", parent, e))?;
    }
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .map_err(|e| Error::io("cannot open log file", log, e))
}

/// "exited with status N" or "was killed by signal N".
fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with status {code}"),
        (None, Some(signal)) => format!("was killed by signal {signal}"),
        (None, None) => format!("ended abnormally ({status})"),
    }
}

/// POSIX single quoting: `'...'` with each `'` written as `'\''`.
fn shell_quote(word: impl AsRef<OsStr>) -> String {
    let word = word.as_ref().to_string_lossy();
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `KEY='value'`, or `'KEY=value'` when `KEY` cannot be a shell assignment.
fn shell_assignment(key: &str, value: &str) -> String {
    if is_shell_name(key) {
        format!("{key}={}", shell_quote(value))
    } else {
        shell_quote(format!("{key}={value}"))
    }
}

/// `[A-Za-z_][A-Za-z0-9_]*`: what a shell accepts left of `=` in an assignment.
fn is_shell_name(name: &str) -> bool {
    let mut chars = name.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passthrough() -> String {
        ALLOWED_ENV
            .iter()
            .map(|name| format!("{name}=\"${name}\""))
            .collect::<Vec<_>>()
            .join(" ")
    }

    #[test]
    fn quotes_every_token() {
        let spec = CommandSpec::new("/usr/bin/open").arg("-n").arg("");
        assert_eq!(spec.to_shell(), "'/usr/bin/open' '-n' ''");
    }

    #[test]
    fn escapes_single_quotes() {
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote("''"), r"''\'''\'''");
        assert_eq!(shell_quote("$HOME `x` \"y\" \\"), "'$HOME `x` \"y\" \\'");
    }

    #[test]
    fn renders_env_sorted_before_the_program() {
        let spec = CommandSpec::new("/opt/wine/bin/wine")
            .arg("C:\\Program Files\\Game\\game.exe")
            .env("WINEPREFIX", "/b/steam")
            .env("DXMT_LOG_LEVEL", "none");
        assert_eq!(
            spec.to_shell(),
            r"DXMT_LOG_LEVEL='none' WINEPREFIX='/b/steam' '/opt/wine/bin/wine' 'C:\Program Files\Game\game.exe'"
        );
    }

    #[test]
    fn prefixes_cd_when_cwd_is_set() {
        let mut spec = CommandSpec::new("/bin/game").arg("x");
        spec.cwd = Some(PathBuf::from("/Games/It's here"));
        assert_eq!(
            spec.to_shell(),
            r"cd '/Games/It'\''s here' && '/bin/game' 'x'"
        );
    }

    #[test]
    fn env_clear_renders_env_i_with_passthrough_and_wine_path() {
        let mut spec = CommandSpec::new("/w/wine")
            .arg("a")
            .env("WINEDEBUG", "-all");
        spec.env_clear = true;
        let expected = format!(
            "env -i {} PATH='/usr/bin:/bin:/usr/sbin:/sbin' WINEDEBUG='-all' '/w/wine' 'a'",
            passthrough()
        );
        assert_eq!(spec.to_shell(), expected);
        assert!(spec.to_shell().starts_with("env -i HOME=\"$HOME\" "));
    }

    #[test]
    fn env_clear_does_not_repeat_variables_set_in_env() {
        let mut spec = CommandSpec::new("/p")
            .env("HOME", "/h")
            .env("PATH", "/custom");
        spec.env_clear = true;
        let line = spec.to_shell();
        assert!(!line.contains("HOME=\"$HOME\""), "{line}");
        assert!(!line.contains("/usr/sbin"), "{line}");
        assert!(line.ends_with("HOME='/h' PATH='/custom' '/p'"), "{line}");
    }

    #[test]
    fn non_shell_names_go_through_env() {
        let spec = CommandSpec::new("/p").env("A-B", "1").env("OK", "2");
        assert_eq!(spec.to_shell(), "env 'A-B=1' OK='2' '/p'");
    }

    #[test]
    fn recognizes_shell_names() {
        for name in ["A", "_", "WINE_PATH", "a1", "__CF_USER_TEXT_ENCODING"] {
            assert!(is_shell_name(name), "{name}");
        }
        for name in ["", "1A", "A-B", "A B", "É", "A=B"] {
            assert!(!is_shell_name(name), "{name}");
        }
    }

    #[test]
    fn to_command_carries_program_args_env_and_cwd() {
        let mut spec = CommandSpec::new("/bin/echo").args(["a", "b"]).env("K", "v");
        spec.cwd = Some(PathBuf::from("/tmp"));
        let command = spec.to_command().unwrap();
        assert_eq!(command.get_program(), "/bin/echo");
        assert_eq!(command.get_args().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(
            command.get_envs().collect::<Vec<_>>(),
            [(OsStr::new("K"), Some(OsStr::new("v")))]
        );
        assert_eq!(command.get_current_dir(), Some(Path::new("/tmp")));
    }

    #[test]
    fn to_command_with_env_clear_sets_wine_path_then_env() {
        let mut spec = CommandSpec::new("/bin/echo").env("PATH", "/override");
        spec.env_clear = true;
        let command = spec.to_command().unwrap();
        let envs: BTreeMap<&OsStr, Option<&OsStr>> = command.get_envs().collect();
        assert_eq!(envs[OsStr::new("PATH")], Some(OsStr::new("/override")));
        for (name, _) in envs {
            let name = name.to_str().unwrap();
            assert!(
                name == "PATH" || ALLOWED_ENV.contains(&name),
                "{name} leaked"
            );
        }
    }

    #[test]
    fn to_command_creates_the_log_directory() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("logs").join("deep").join("run.log");
        let mut spec = CommandSpec::new("/bin/echo");
        spec.log = Some(log.clone());
        spec.to_command().unwrap();
        assert!(log.is_file());
    }

    #[test]
    fn to_command_reports_an_unopenable_log() {
        let temp = tempfile::tempdir().unwrap();
        let mut spec = CommandSpec::new("/bin/echo");
        // The log path is an existing directory.
        spec.log = Some(temp.path().to_path_buf());
        let err = spec.to_command().unwrap_err();
        assert!(
            matches!(&err, Error::Io { path, .. } if path == temp.path()),
            "{err:?}"
        );
    }

    #[test]
    fn describes_exit_statuses() {
        // Raw wait(2) statuses: exit code in the high byte, signal in the low bits.
        assert_eq!(
            describe_exit(ExitStatus::from_raw(3 << 8)),
            "exited with status 3"
        );
        assert_eq!(
            describe_exit(ExitStatus::from_raw(9)),
            "was killed by signal 9"
        );
    }

    #[test]
    fn errors_name_the_program_file_and_log() {
        let mut spec = CommandSpec::new("/opt/wine/bin/wine");
        spec.log = Some(PathBuf::from("/l/run.log"));
        let err = spec
            .check_exit(ExitStatus::from_raw(1 << 8), spec.log.as_deref())
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "wine exited with status 1 (log: /l/run.log)"
        );
        let err = spec.start_error(&io::Error::from_raw_os_error(2));
        assert_eq!(
            err.to_string(),
            format!("wine could not start: {}", io::Error::from_raw_os_error(2))
        );
        assert!(spec.check_exit(ExitStatus::from_raw(0), None).is_ok());
    }

    #[test]
    fn program_name_falls_back_to_the_whole_path() {
        assert_eq!(CommandSpec::new("/").program_name(), "/");
        assert_eq!(CommandSpec::new("relative/tool").program_name(), "tool");
    }

    #[test]
    fn is_executable_requires_a_file_with_an_execute_bit() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("tool");
        fs::write(&file, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&file, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!is_executable(&file));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o744)).unwrap();
        assert!(is_executable(&file));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o701)).unwrap();
        assert!(is_executable(&file));
        assert!(
            !is_executable(temp.path()),
            "directories are not executables"
        );
        assert!(!is_executable(&temp.path().join("missing")));

        let link = temp.path().join("link");
        std::os::unix::fs::symlink(&file, &link).unwrap();
        assert!(is_executable(&link), "symlinks are followed");
    }
}
