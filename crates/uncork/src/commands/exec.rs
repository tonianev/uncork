//! `uncork __exec <plan.json>` and the Game Mode bundle launcher.
//!
//! A Game Mode app bundle (`uncork_core::gamemode`) holds a copy of this
//! binary as `Contents/MacOS/launch` and the launch command as
//! `Contents/Resources/plan.json`. LaunchServices starts the copy without
//! arguments; it then replaces itself with the Wine loader (`exec`), so the
//! game runs as the process macOS attributed to the bundle.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::os::unix::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, anyhow, bail};
use serde::Deserialize;
use uncork_core::process::CommandSpec;

/// The fields of a serialized [`CommandSpec`] (which is `Serialize` only).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpecFile {
    program: PathBuf,
    #[serde(default)]
    args: Vec<SpecArg>,
    #[serde(default)]
    env: BTreeMap<String, String>,
    #[serde(default)]
    env_clear: bool,
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    log: Option<PathBuf>,
}

/// One argument: a plain string (as `--json` output shows it) or serde's
/// form of an `OsString`, `{"Unix": [<bytes>]}` (as Game Mode's
/// `plan.json` keeps it, exact for any bytes).
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SpecArg {
    Text(String),
    Os(OsString),
}

impl From<SpecArg> for OsString {
    fn from(arg: SpecArg) -> OsString {
        match arg {
            SpecArg::Text(text) => text.into(),
            SpecArg::Os(os) => os,
        }
    }
}

impl From<SpecFile> for CommandSpec {
    fn from(file: SpecFile) -> CommandSpec {
        CommandSpec {
            program: file.program,
            args: file.args.into_iter().map(OsString::from).collect(),
            env: file.env,
            env_clear: file.env_clear,
            cwd: file.cwd,
            log: file.log,
        }
    }
}

/// Parse a plan file: a serialized [`CommandSpec`], a serialized
/// `LaunchPlan` whose `command` is used, or `--dry-run --json` output (its
/// `plan`'s `command`). Arguments may be strings or `OsString` byte lists.
fn parse_plan(text: &str) -> anyhow::Result<CommandSpec> {
    let value: serde_json::Value = serde_json::from_str(text).context("not valid JSON")?;
    let value = match value.get("plan") {
        Some(plan) if value.get("program").is_none() => plan.clone(),
        _ => value,
    };
    let spec = match value.get("command") {
        Some(command) if value.get("program").is_none() => command.clone(),
        _ => value,
    };
    let spec: SpecFile =
        serde_json::from_value(spec).context("not a launch command (CommandSpec) or LaunchPlan")?;
    if !spec.program.is_absolute() {
        bail!(
            "the program {} is not an absolute path",
            spec.program.display()
        );
    }
    Ok(spec.into())
}

/// Read `plan` and run its command in place of this process. Returns only
/// when that fails.
pub(super) fn run(plan: &Path) -> anyhow::Result<ExitCode> {
    let text = std::fs::read_to_string(plan)
        .with_context(|| format!("cannot read launch plan {}", plan.display()))?;
    let spec =
        parse_plan(&text).with_context(|| format!("invalid launch plan {}", plan.display()))?;
    tracing::info!("exec {}", spec.to_shell());
    let error = spec.to_command()?.exec();
    Err(anyhow!(error).context(format!("cannot start {}", spec.program.display())))
}

/// When this binary runs as a Game Mode bundle's `Contents/MacOS/launch`,
/// run the bundle's plan and return its (failed) result; `None` otherwise.
///
/// The bundle's own `Resources/plan.json` is used when LaunchServices passes
/// no arguments (or only the legacy `-psn_...` process serial number);
/// `exec <plan>` or `__exec <plan>` name another plan file.
pub fn bundle_launch() -> Option<anyhow::Result<ExitCode>> {
    let exe = std::env::current_exe().ok()?;
    let resources = bundle_resources(&exe)?;
    let args: Vec<OsString> = std::env::args_os()
        .skip(1)
        .filter(|arg| !arg.to_string_lossy().starts_with("-psn_"))
        .collect();
    let plan = match args.as_slice() {
        [] => resources.join("plan.json"),
        [verb, plan] if verb == "exec" || verb == "__exec" => PathBuf::from(plan),
        _ => return None,
    };
    Some(run(&plan))
}

/// `<bundle>/Contents/Resources` when `exe` is `<bundle>/Contents/MacOS/launch`.
fn bundle_resources(exe: &Path) -> Option<PathBuf> {
    if exe.file_name()? != "launch" {
        return None;
    }
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    if macos.file_name()? != "MacOS" || contents.file_name()? != "Contents" {
        return None;
    }
    Some(contents.join("Resources"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> CommandSpec {
        let mut spec = CommandSpec::new("/opt/wine/bin/wine")
            .arg("C:\\Games\\game.exe")
            .arg("-windowed")
            .env("WINEPREFIX", "/data/bottles/steam");
        spec.env_clear = true;
        spec.cwd = Some("/data/bottles/steam/drive_c/Games".into());
        spec.log = Some("/data/logs/steam-game-1.log".into());
        spec
    }

    #[test]
    fn a_serialized_command_spec_round_trips() {
        let text = serde_json::to_string(&spec()).unwrap();
        assert_eq!(parse_plan(&text).unwrap(), spec());
    }

    #[test]
    fn a_launch_plan_uses_its_command() {
        let text = serde_json::json!({
            "command": serde_json::to_value(spec()).unwrap(),
            "backend_reason": "32-bit D3D11: DXMT",
            "warnings": [],
        })
        .to_string();
        assert_eq!(parse_plan(&text).unwrap(), spec());
    }

    #[test]
    fn arguments_may_be_strings_or_byte_lists() {
        use std::os::unix::ffi::OsStringExt as _;
        let mut expected = spec();
        expected
            .args
            .push(OsString::from_vec(b"bad-\xff-arg".to_vec()));
        let byte_form = serde_json::to_value(&expected).unwrap();
        assert!(
            byte_form["args"][0].get("Unix").is_some(),
            "Game Mode's plan.json form: {byte_form}"
        );
        assert_eq!(parse_plan(&byte_form.to_string()).unwrap(), expected);

        let mut strings = serde_json::to_value(spec()).unwrap();
        strings["args"] = serde_json::json!(["C:\\Games\\game.exe", "-windowed"]);
        assert_eq!(parse_plan(&strings.to_string()).unwrap(), spec());

        let dry_run = serde_json::json!({ "plan": { "command": strings }, "ini": [] });
        assert_eq!(parse_plan(&dry_run.to_string()).unwrap(), spec());
    }

    #[test]
    fn missing_fields_take_defaults() {
        let spec = parse_plan(r#"{"program": "/bin/true"}"#).unwrap();
        assert_eq!(spec, CommandSpec::new("/bin/true"));
    }

    #[test]
    fn bad_plans_are_errors() {
        for text in [
            "not json",
            "{}",
            r#"{"program": "wine"}"#,
            r#"{"program": "/bin/true", "colour": 1}"#,
        ] {
            assert!(parse_plan(text).is_err(), "{text}");
        }
    }

    #[test]
    fn bundle_layout_is_recognized() {
        assert_eq!(
            bundle_resources(Path::new("/A/Game.app/Contents/MacOS/launch")),
            Some(PathBuf::from("/A/Game.app/Contents/Resources"))
        );
        for exe in [
            "/usr/local/bin/uncork",
            "/A/Game.app/Contents/MacOS/uncork",
            "/A/Game.app/Contents/Other/launch",
            "/A/Game.app/Stuff/MacOS/launch",
            "launch",
        ] {
            assert_eq!(bundle_resources(Path::new(exe)), None, "{exe}");
        }
    }
}
