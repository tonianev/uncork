//! `uncork play`, `uncork run` and `uncork steam launch`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitCode};
use std::time::Duration;

use anyhow::{Context as _, anyhow, bail};
use serde::Serialize;
use uncork_core::bottle::Bottle;
use uncork_core::graphics::{Backend, BackendChoice, Strategy};
use uncork_core::launch::{self, LaunchOptions, LaunchPlan, PlanContext, Target};
use uncork_core::profile::GameProfile;
use uncork_core::wine::WineRuntime;

use super::{Ctx, bottle_wine, is_env_name};
use crate::cli::{BackendArg, LaunchFlags, PlayArgs, RunArgs};
use crate::output;

/// How long `uncork play` waits for the Steam client to come up.
const STEAM_TIMEOUT: Duration = Duration::from_secs(180);

/// [`LaunchOptions`] from the shared command-line flags. `--env` values are
/// checked strictly: `KEY=VALUE` with a valid variable name.
pub(super) fn launch_options(flags: &LaunchFlags) -> anyhow::Result<LaunchOptions> {
    let mut env = BTreeMap::new();
    for item in &flags.env {
        let (key, value) = parse_env_pair(item)?;
        env.insert(key.to_owned(), value.to_owned());
    }
    Ok(LaunchOptions {
        backend: flags.backend.map(backend_choice),
        hud: flags.hud,
        metalfx: flags.metalfx,
        retina: flags.retina,
        wine_debug: flags.wine_debug.clone(),
        env,
        game_mode: flags.game_mode.then_some(true),
    })
}

/// Split one `--env` value.
fn parse_env_pair(item: &str) -> anyhow::Result<(&str, &str)> {
    let Some((key, value)) = item.split_once('=') else {
        bail!("invalid --env {item:?}: expected KEY=VALUE");
    };
    if !is_env_name(key) {
        bail!(
            "invalid --env {item:?}: {key:?} is not a variable name (letters, digits and '_', not starting with a digit)"
        );
    }
    Ok((key, value))
}

fn backend_choice(arg: BackendArg) -> BackendChoice {
    match arg {
        BackendArg::Auto => BackendChoice::Auto,
        BackendArg::D3dmetal => BackendChoice::Fixed(Backend::D3dmetal),
        BackendArg::Dxmt => BackendChoice::Fixed(Backend::Dxmt),
        BackendArg::Dxvk => BackendChoice::Fixed(Backend::Dxvk),
        BackendArg::Wined3d => BackendChoice::Fixed(Backend::Wined3d),
    }
}

/// `uncork play <game>`.
pub(super) fn play(ctx: &Ctx, args: &PlayArgs) -> anyhow::Result<ExitCode> {
    let options = launch_options(&args.launch)?;
    let profiles = super::profile::load_profiles(ctx)?;
    let profile = uncork_core::profile::resolve(&profiles, &args.game);
    let appid = match profile {
        Some(profile) => profile.steam.as_ref().map(|steam| steam.appid).ok_or_else(|| {
            anyhow!(
                "{} is not a Steam game; run its executable ({}) with `uncork run <path>`",
                profile.name,
                profile.exe.path
            )
        })?,
        None => parse_appid(&args.game).ok_or_else(|| {
            anyhow!(
                "no game profile matches {:?} (or several do); `uncork profile list` shows the known games, and `uncork play <Steam app id>` plays any installed Steam game (`uncork steam games` lists them)",
                args.game
            )
        })?,
    };
    play_steam_game(ctx, profile, appid, &args.launch, &options, &args.args)
}

/// A Steam app id typed on the command line (digits only).
fn parse_appid(text: &str) -> Option<u32> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

/// Play Steam app `appid` (shared by `play` and `steam launch`).
pub(super) fn play_steam_game(
    ctx: &Ctx,
    profile: Option<&GameProfile>,
    appid: u32,
    flags: &LaunchFlags,
    options: &LaunchOptions,
    args: &[String],
) -> anyhow::Result<ExitCode> {
    let mut bottle = ctx.open_bottle(flags.bottle.as_deref())?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    let components = ctx.components()?;
    let game = profile.map_or_else(|| format!("Steam app {appid}"), |p| p.name.clone());
    if !flags.dry_run {
        eprintln!(
            "Starting {game} in bottle {} (Steam starts first if needed; that can take a minute)",
            bottle.config.name
        );
    }
    let outcome = uncork_core::steam::play(
        &ctx.layout,
        &mut bottle,
        &wine,
        &components,
        profile,
        appid,
        args,
        options,
        STEAM_TIMEOUT,
        flags.dry_run,
    )
    .with_context(|| format!("cannot play {game}"))?;

    if flags.dry_run {
        let install_dir = steam_install_dir(&bottle, appid);
        let ini = profile.map_or_else(Vec::new, |profile| {
            ini_edits(profile, &bottle, install_dir.as_deref())
        });
        print_dry_run(ctx, &outcome.plan, &ini)?;
        return Ok(ExitCode::SUCCESS);
    }
    if outcome.started_steam {
        println!("Started Steam in bottle {}.", bottle.config.name);
    }
    for file in &outcome.ini_changed {
        println!("Updated {}", file.display());
    }
    report_launch(&outcome.plan, outcome.child, &bottle, &wine, flags.wait)
}

/// The game's install directory, if Steam and the game are installed.
fn steam_install_dir(bottle: &Bottle, appid: u32) -> Option<PathBuf> {
    let steam = uncork_steam::SteamInstall::find(bottle.prefix())?;
    match steam.find_app(appid) {
        Ok(app) => app.map(|app| app.install_path),
        Err(err) => {
            tracing::info!("cannot read the Steam libraries: {err}");
            None
        }
    }
}

/// `uncork run <exe>`.
pub(super) fn run(ctx: &Ctx, args: &RunArgs) -> anyhow::Result<ExitCode> {
    let options = launch_options(&args.launch)?;
    let mut bottle = ctx.open_bottle(args.launch.bottle.as_deref())?;
    let exe = resolve_exe(&bottle, &args.exe)?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    let components = ctx.components()?;
    let target = Target::Exe {
        path: exe.clone(),
        args: args.args.clone(),
    };
    let plan = launch::plan(
        PlanContext {
            layout: &ctx.layout,
            bottle: &bottle,
            wine: &wine,
            components: &components,
            profile: None,
        },
        &target,
        &options,
    )
    .with_context(|| format!("cannot plan a launch of {}", exe.display()))?;
    if args.launch.dry_run {
        print_dry_run(ctx, &plan, &[])?;
        return Ok(ExitCode::SUCCESS);
    }
    let child = launch::execute(&plan, &mut bottle, &wine)
        .with_context(|| format!("cannot start {}", exe.display()))?;
    report_launch(&plan, Some(child), &bottle, &wine, args.launch.wait)
}

/// The executable `run` was given: a Windows path (`C:\...`) is mapped into
/// the bottle; anything else is a macOS path.
fn resolve_exe(bottle: &Bottle, exe: &str) -> anyhow::Result<PathBuf> {
    let path = if is_windows_path(exe) {
        uncork_steam::paths::windows_to_unix(bottle.prefix(), exe).ok_or_else(|| {
            anyhow!(
                "cannot map {exe:?} into bottle {}; use an absolute Windows path such as C:\\Games\\game.exe",
                bottle.config.name
            )
        })?
    } else {
        std::path::absolute(exe).with_context(|| format!("cannot resolve {exe}"))?
    };
    if !path.is_file() {
        bail!("{} does not exist", path.display());
    }
    Ok(path)
}

/// `X:\...` or `X:/...`.
fn is_windows_path(text: &str) -> bool {
    matches!(text.as_bytes(), [letter, b':', b'\\' | b'/', ..] if letter.is_ascii_alphabetic())
}

/// One INI key a profile would set, for dry runs.
#[derive(Debug, Serialize)]
pub(super) struct IniEditView {
    /// The profile's `%BASE%\...` path.
    file: String,
    /// Where it resolves to in the bottle (`None` when it cannot be resolved).
    path: Option<PathBuf>,
    /// The file exists now (missing files are skipped at launch).
    exists: bool,
    section: String,
    key: String,
    value: String,
    reason: String,
}

/// The INI edits `profile` makes before a launch.
fn ini_edits(
    profile: &GameProfile,
    bottle: &Bottle,
    install_dir: Option<&Path>,
) -> Vec<IniEditView> {
    profile
        .ini
        .iter()
        .map(|ini| {
            let path = launch::resolve_profile_path(bottle, install_dir, &ini.file);
            IniEditView {
                exists: path.as_deref().is_some_and(Path::is_file),
                path,
                file: ini.file.clone(),
                section: ini.section.clone(),
                key: ini.key.clone(),
                value: ini.value.clone(),
                reason: ini.reason.clone(),
            }
        })
        .collect()
}

/// `--dry-run --json` output.
#[derive(Debug, Serialize)]
struct DryRunView<'a> {
    plan: &'a LaunchPlan,
    ini: &'a [IniEditView],
}

/// Print what a launch would do: backend and why, warnings, DLL copies,
/// INI edits, the log file and the exact command.
fn print_dry_run(ctx: &Ctx, plan: &LaunchPlan, ini: &[IniEditView]) -> anyhow::Result<()> {
    if ctx.json {
        return output::print_json(&DryRunView { plan, ini });
    }
    print!("{}", render_dry_run(plan, ini));
    Ok(())
}

fn render_dry_run(plan: &LaunchPlan, ini: &[IniEditView]) -> String {
    let activation = &plan.activation;
    let version = activation
        .version
        .as_deref()
        .map_or_else(String::new, |version| format!(" {version}"));
    let mut out = format!(
        "Backend: {}{version} ({})\n",
        activation.backend,
        describe_strategy(&activation.strategy)
    );
    out.push_str(&format!("Reason:  {}\n", plan.backend_reason));
    for warning in &plan.warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }
    if !activation.copies.is_empty() {
        out.push_str("DLLs to install:\n");
        for copy in &activation.copies {
            let marker = if copy.strip_builtin_marker {
                ", builtin marker removed"
            } else {
                ""
            };
            out.push_str(&format!(
                "  {}  (from {}{marker})\n",
                copy.dest.display(),
                copy.source.display()
            ));
        }
    }
    if !ini.is_empty() {
        out.push_str("INI edits:\n");
        for edit in ini {
            let location = match &edit.path {
                Some(path) if edit.exists => path.display().to_string(),
                Some(path) => format!("{} (does not exist yet; skipped)", path.display()),
                None => format!("{} (cannot be resolved; skipped)", edit.file),
            };
            out.push_str(&format!(
                "  [{}] {}={} in {location}\n",
                edit.section, edit.key, edit.value
            ));
        }
    }
    if let Some(log) = &plan.command.log {
        out.push_str(&format!("Log:     {}\n", log.display()));
    }
    out.push_str("Command:\n");
    out.push_str(&format!("  {}\n", plan.command.to_shell()));
    out
}

fn describe_strategy(strategy: &Strategy) -> String {
    match strategy {
        Strategy::RendererEnv { var } => format!("loaded per process through {var}"),
        Strategy::DllPathPrepend => "loaded per process through WINEDLLPATH_PREPEND".to_owned(),
        Strategy::PrefixNative => {
            "DLLs copied into the prefix, chosen with DLL overrides".to_owned()
        }
        Strategy::Builtin => "built into Wine".to_owned(),
    }
}

/// Print what started; with `wait`, wait for it and for the bottle's
/// wineserver, and exit with failure when the program failed.
fn report_launch(
    plan: &LaunchPlan,
    child: Option<Child>,
    bottle: &Bottle,
    wine: &WineRuntime,
    wait: bool,
) -> anyhow::Result<ExitCode> {
    let program = plan_program(plan);
    let Some(mut child) = child else {
        println!("Started {program} on {}.", plan.activation.backend);
        return Ok(ExitCode::SUCCESS);
    };
    println!(
        "Started {program} on {} (pid {}).",
        plan.activation.backend,
        child.id()
    );
    println!("Log: {}", plan.log.display());
    if !wait {
        return Ok(ExitCode::SUCCESS);
    }
    let status = child
        .wait()
        .with_context(|| format!("cannot wait for {program}"))?;
    uncork_core::process::run(&uncork_core::launch::in_bottle(
        wine.wait_command(bottle.prefix()),
        bottle,
        wine,
    ))
    .context("cannot wait for the bottle's wineserver")?;
    println!("{program} exited ({status}).");
    Ok(if status.success() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The program a plan starts, for messages: the first argument after the
/// loader (the `.exe` or Wine program), else the loader itself.
fn plan_program(plan: &LaunchPlan) -> String {
    let first = plan
        .command
        .args
        .first()
        .map_or_else(|| plan.command.program.as_os_str(), |arg| arg.as_os_str());
    let text = first.to_string_lossy();
    let name = text.rsplit(['/', '\\']).next().unwrap_or(&text);
    name.to_owned()
}

#[cfg(test)]
mod tests {
    use uncork_core::graphics::{Activation, DllCopy};
    use uncork_core::process::CommandSpec;

    use super::*;

    fn flags(env: &[&str]) -> LaunchFlags {
        LaunchFlags {
            bottle: None,
            backend: Some(BackendArg::Dxmt),
            hud: true,
            metalfx: false,
            retina: false,
            wine_debug: Some("+loaddll".to_owned()),
            env: env.iter().map(|&item| item.to_owned()).collect(),
            game_mode: false,
            dry_run: true,
            wait: false,
        }
    }

    #[test]
    fn options_come_from_flags() {
        let options = launch_options(&flags(&["A=1", "B=x=y", "EMPTY=", "A=2"])).unwrap();
        assert_eq!(options.backend, Some(BackendChoice::Fixed(Backend::Dxmt)));
        assert!(options.hud && !options.metalfx);
        assert_eq!(options.wine_debug.as_deref(), Some("+loaddll"));
        assert_eq!(
            options.env,
            BTreeMap::from([
                ("A".to_owned(), "2".to_owned()),
                ("B".to_owned(), "x=y".to_owned()),
                ("EMPTY".to_owned(), String::new()),
            ])
        );
    }

    #[test]
    fn env_values_are_checked() {
        for bad in ["NOEQUALS", "=x", "1X=2", "A-B=1", "A B=1"] {
            let err = launch_options(&flags(&[bad])).unwrap_err().to_string();
            assert!(err.starts_with("invalid --env"), "{bad}: {err}");
        }
    }

    #[test]
    fn backend_args_map_to_choices() {
        assert_eq!(backend_choice(BackendArg::Auto), BackendChoice::Auto);
        assert_eq!(
            backend_choice(BackendArg::Wined3d),
            BackendChoice::Fixed(Backend::Wined3d)
        );
        assert_eq!(
            backend_choice(BackendArg::D3dmetal),
            BackendChoice::Fixed(Backend::D3dmetal)
        );
    }

    #[test]
    fn appids_are_digits_only() {
        assert_eq!(parse_appid("287450"), Some(287_450));
        assert_eq!(parse_appid(" 70 "), Some(70));
        assert_eq!(parse_appid("rise"), None);
        assert_eq!(parse_appid("+5"), None);
        assert_eq!(parse_appid("99999999999"), None);
    }

    #[test]
    fn windows_paths() {
        assert!(is_windows_path(r"C:\Games\x.exe"));
        assert!(is_windows_path("d:/x.exe"));
        assert!(!is_windows_path("C:x.exe"));
        assert!(!is_windows_path("/Users/me/x.exe"));
        assert!(!is_windows_path("game.exe"));
    }

    #[test]
    fn dry_run_shows_backend_copies_ini_and_command() {
        let mut command = CommandSpec::new("/w/bin/wine")
            .arg("/b/drive_c/Games/game.exe")
            .env("WINEPREFIX", "/b");
        command.log = Some("/u/logs/b-game-1.log".into());
        let plan = LaunchPlan {
            command,
            activation: Activation {
                backend: Backend::Dxmt,
                version: Some("0.80".to_owned()),
                strategy: Strategy::PrefixNative,
                copies: vec![DllCopy {
                    source: "/u/components/dxmt/0.80/i386-windows/d3d11.dll".into(),
                    dest: "drive_c/windows/syswow64/d3d11.dll".into(),
                    strip_builtin_marker: true,
                }],
                env: BTreeMap::new(),
                overrides: BTreeMap::new(),
            },
            backend_reason: "32-bit D3D11: DXMT".to_owned(),
            log: "/u/logs/b-game-1.log".into(),
            warnings: vec!["anti-cheat found".to_owned()],
            game_mode: None,
        };
        let ini = vec![IniEditView {
            file: r"%APPDATA%\G\g.ini".to_owned(),
            path: Some("/b/drive_c/users/me/AppData/Roaming/G/g.ini".into()),
            exists: false,
            section: "S".to_owned(),
            key: "K".to_owned(),
            value: "1".to_owned(),
            reason: String::new(),
        }];
        let text = render_dry_run(&plan, &ini);
        assert!(
            text.starts_with(
                "Backend: dxmt 0.80 (DLLs copied into the prefix, chosen with DLL overrides)\n"
            ),
            "{text}"
        );
        assert!(text.contains("Reason:  32-bit D3D11: DXMT\n"), "{text}");
        assert!(text.contains("warning: anti-cheat found\n"), "{text}");
        assert!(
            text.contains("  drive_c/windows/syswow64/d3d11.dll  (from /u/components/dxmt/0.80/i386-windows/d3d11.dll, builtin marker removed)\n"),
            "{text}"
        );
        assert!(text.contains("[S] K=1 in /b/drive_c/users/me/AppData/Roaming/G/g.ini (does not exist yet; skipped)"), "{text}");
        assert!(text.contains("Log:     /u/logs/b-game-1.log\n"), "{text}");
        assert!(
            text.ends_with("  WINEPREFIX='/b' '/w/bin/wine' '/b/drive_c/Games/game.exe'\n"),
            "{text}"
        );
        assert_eq!(plan_program(&plan), "game.exe");
    }
}
