//! `process` against real programs: small shell scripts written into a
//! temporary directory, plus `/bin/sh`, `/usr/bin/env` and `/usr/bin/printf`.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use proptest::prelude::*;
use uncork_core::Error;
use uncork_core::launch::{ALLOWED_ENV, WINE_PATH};
use uncork_core::process::{self, CommandSpec};

/// Write an executable `#!/bin/sh` script.
fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    path
}

fn logged(program: &Path, log: &Path) -> CommandSpec {
    let mut spec = CommandSpec::new(program);
    spec.log = Some(log.to_path_buf());
    spec
}

/// `(program, status, log_hint)` of an [`Error::Command`].
fn command_error(error: Error) -> (String, String, String) {
    match error {
        Error::Command {
            program,
            status,
            log_hint,
        } => (program, status, log_hint),
        other => panic!("expected Error::Command, got {other:?}"),
    }
}

/// `KEY=value` lines as a map.
fn parse_env(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect()
}

#[test]
fn run_succeeds_on_exit_zero() {
    let temp = tempfile::tempdir().unwrap();
    let ok = script(temp.path(), "ok.sh", "exit 0");
    process::run(&CommandSpec::new(ok)).unwrap();
}

#[test]
fn run_failure_names_program_status_and_log() {
    let temp = tempfile::tempdir().unwrap();
    let fail = script(temp.path(), "fail.sh", "echo out\necho err >&2\nexit 3");
    let log = temp.path().join("logs").join("nested").join("fail.log");
    let error = process::run(&logged(&fail, &log)).unwrap_err();
    assert_eq!(
        error.to_string(),
        format!("fail.sh exited with status 3 (log: {})", log.display())
    );
    assert_eq!(
        command_error(error),
        (
            "fail.sh".to_owned(),
            "exited with status 3".to_owned(),
            format!(" (log: {})", log.display())
        )
    );
    assert_eq!(fs::read_to_string(&log).unwrap(), "out\nerr\n");
}

#[test]
fn run_appends_to_an_existing_log() {
    let temp = tempfile::tempdir().unwrap();
    let hello = script(temp.path(), "hello.sh", "echo hello\necho oops >&2");
    let log = temp.path().join("run.log");
    fs::write(&log, "earlier\n").unwrap();
    let spec = logged(&hello, &log);
    process::run(&spec).unwrap();
    process::run(&spec).unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "earlier\nhello\noops\nhello\noops\n"
    );
}

#[test]
fn run_reports_a_signal() {
    let temp = tempfile::tempdir().unwrap();
    let killed = script(temp.path(), "killed.sh", "kill -9 $$");
    let (program, status, log_hint) =
        command_error(process::run(&CommandSpec::new(killed)).unwrap_err());
    assert_eq!(program, "killed.sh");
    assert_eq!(status, "was killed by signal 9");
    assert_eq!(log_hint, "");
}

#[test]
fn run_reports_a_program_that_cannot_start() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing-tool");
    let (program, status, log_hint) =
        command_error(process::run(&CommandSpec::new(&missing)).unwrap_err());
    assert_eq!(program, "missing-tool");
    assert!(status.starts_with("could not start: "), "{status}");
    assert_eq!(log_hint, "", "nothing was captured");

    let not_executable = temp.path().join("plain.sh");
    fs::write(&not_executable, "#!/bin/sh\n").unwrap();
    let (_, status, _) =
        command_error(process::run(&CommandSpec::new(&not_executable)).unwrap_err());
    assert!(status.starts_with("could not start: "), "{status}");
}

#[test]
fn run_reports_an_unopenable_log_as_io() {
    let temp = tempfile::tempdir().unwrap();
    let ok = script(temp.path(), "ok.sh", "exit 0");
    let blocker = temp.path().join("file");
    fs::write(&blocker, "").unwrap();
    let error = process::run(&logged(&ok, &blocker.join("x.log"))).unwrap_err();
    assert!(matches!(error, Error::Io { .. }), "{error:?}");
}

#[test]
fn output_returns_trimmed_stdout_and_ignores_log() {
    let temp = tempfile::tempdir().unwrap();
    let talk = script(temp.path(), "talk.sh", "printf '  hello world \\n\\n'");
    let log = temp.path().join("never.log");
    assert_eq!(
        process::output(&logged(&talk, &log)).unwrap(),
        "hello world"
    );
    assert!(!log.exists(), "output() must not open the log");
}

#[test]
fn output_is_lossy_for_invalid_utf8() {
    let temp = tempfile::tempdir().unwrap();
    let bytes = script(temp.path(), "bytes.sh", "printf 'a\\377b'");
    assert_eq!(
        process::output(&CommandSpec::new(bytes)).unwrap(),
        "a\u{FFFD}b"
    );
}

#[test]
fn output_failure_has_no_log_hint() {
    let temp = tempfile::tempdir().unwrap();
    let fail = script(temp.path(), "fail.sh", "echo partial\nexit 2");
    let log = temp.path().join("ignored.log");
    let (program, status, log_hint) =
        command_error(process::output(&logged(&fail, &log)).unwrap_err());
    assert_eq!(
        (program.as_str(), status.as_str(), log_hint.as_str()),
        ("fail.sh", "exited with status 2", "")
    );
}

#[test]
fn output_runs_in_cwd_with_args_and_env() {
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().join("work dir");
    fs::create_dir(&work).unwrap();
    let show = script(
        temp.path(),
        "show.sh",
        "pwd -P\nprintf '%s|' \"$@\"\necho \"$GREETING\"",
    );
    let mut spec = CommandSpec::new(show)
        .args(["a b", "it's"])
        .env("GREETING", "hi there");
    spec.cwd = Some(work.clone());
    let text = process::output(&spec).unwrap();
    let canonical = fs::canonicalize(&work).unwrap();
    assert_eq!(text, format!("{}\na b|it's|hi there", canonical.display()));
}

#[test]
fn env_clear_passes_only_allowed_variables_and_wine_path() {
    let mut spec = CommandSpec::new("/usr/bin/env").env("WINEDEBUG", "-all");
    spec.env_clear = true;
    let env = parse_env(&process::output(&spec).unwrap());
    assert_eq!(env.get("PATH").map(String::as_str), Some(WINE_PATH));
    assert_eq!(env.get("WINEDEBUG").map(String::as_str), Some("-all"));
    for key in env.keys() {
        assert!(
            key == "PATH" || key == "WINEDEBUG" || ALLOWED_ENV.contains(&key.as_str()),
            "{key} leaked into a cleared environment"
        );
    }
    for name in ALLOWED_ENV {
        assert_eq!(
            env.get(*name).map(String::as_str),
            std::env::var(name).ok().as_deref(),
            "{name} not passed through"
        );
    }
}

#[test]
fn env_clear_lets_env_override_path() {
    let mut spec = CommandSpec::new("/usr/bin/env").env("PATH", "/custom");
    spec.env_clear = true;
    let env = parse_env(&process::output(&spec).unwrap());
    assert_eq!(env.get("PATH").map(String::as_str), Some("/custom"));
}

#[test]
fn without_env_clear_the_environment_is_inherited() {
    let spec = CommandSpec::new("/usr/bin/env").env("UNCORK_TEST_EXTRA", "1");
    let env = parse_env(&process::output(&spec).unwrap());
    assert_eq!(env.get("UNCORK_TEST_EXTRA").map(String::as_str), Some("1"));
    if let Ok(path) = std::env::var("PATH") {
        assert_eq!(env.get("PATH"), Some(&path));
    }
}

#[test]
fn spawn_gives_the_child_a_null_stdin() {
    let temp = tempfile::tempdir().unwrap();
    let reader = script(
        temp.path(),
        "reader.sh",
        "if read line; then echo \"got $line\"; else echo eof; fi",
    );
    let log = temp.path().join("reader.log");
    let mut child = process::spawn(&logged(&reader, &log)).unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(fs::read_to_string(&log).unwrap(), "eof\n");
}

#[test]
fn spawn_reports_a_program_that_cannot_start() {
    let temp = tempfile::tempdir().unwrap();
    let (program, status, _) =
        command_error(process::spawn(&CommandSpec::new(temp.path().join("nope"))).unwrap_err());
    assert_eq!(program, "nope");
    assert!(status.starts_with("could not start: "), "{status}");
}

#[test]
fn to_shell_reproduces_the_cleared_environment() {
    let mut spec = CommandSpec::new("/usr/bin/env")
        .env("WINEPREFIX", "/b/it's here")
        .env("DXMT_LOG_LEVEL", "none");
    spec.env_clear = true;
    let direct = parse_env(&process::output(&spec).unwrap());
    let through_shell = Command::new("/bin/sh")
        .arg("-c")
        .arg(spec.to_shell())
        .output()
        .unwrap();
    assert!(through_shell.status.success());
    let mut pasted = parse_env(&String::from_utf8(through_shell.stdout).unwrap());
    // `NAME="$NAME"` sets a variable the pasting shell lacks to "".
    pasted.retain(|key, value| !value.is_empty() || direct.contains_key(key));
    assert_eq!(pasted, direct);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    // APFS refuses unassigned code points in file names, so the directory
    // name sticks to printable ASCII (minus `/`), a newline and an accent.
    fn to_shell_quotes_any_argument(args in prop::collection::vec("[^\0]*", 0..5), cwd in "[ -.0-~é\n]{1,12}") {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join(&cwd);
        fs::create_dir_all(&dir).unwrap();
        let mut spec = CommandSpec::new("/usr/bin/printf").arg("%s\\0").args(&args);
        spec.cwd = Some(dir);
        let out = Command::new("/bin/sh").arg("-c").arg(spec.to_shell()).output().unwrap();
        prop_assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        let mut printed: Vec<String> = String::from_utf8(out.stdout)
            .unwrap()
            .split('\0')
            .map(str::to_owned)
            .collect();
        prop_assert_eq!(printed.pop(), Some(String::new()));
        // printf with no arguments still runs its format once.
        if args.is_empty() {
            prop_assert_eq!(printed, vec![String::new()]);
        } else {
            prop_assert_eq!(printed, args);
        }
    }

    #[test]
    fn to_shell_quotes_any_env_value(
        // Prefixed so no key collides with a variable the shell manages (`_`, `PWD`, ...).
        key in "k[A-Za-z0-9_]{0,8}|k[-.][a-z]{1,3}",
        value in "[^\0]*",
    ) {
        let spec = CommandSpec::new("/usr/bin/printenv").arg(&key).env(&key, &value);
        let out = Command::new("/bin/sh").arg("-c").arg(spec.to_shell()).output().unwrap();
        prop_assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        prop_assert_eq!(String::from_utf8(out.stdout).unwrap(), format!("{value}\n"));
    }
}
