//! `uncork steam ...`: the Windows Steam client in a bottle.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::{Child, ExitCode};

use anyhow::{Context as _, anyhow};
use serde::Serialize;
use uncork_core::bottle::Bottle;
use uncork_core::component::{ComponentKind, InstalledComponent};
use uncork_core::process::CommandSpec;
use uncork_core::wine::WineRuntime;
use uncork_steam::SteamInstall;
use uncork_steam::library::STATE_FULLY_INSTALLED;

use super::launch::{launch_options, play_steam_game};
use super::setup::STEAM_INSTALLER_BYTES;
use super::{Ctx, bottle_wine, confirm_download, newest};
use crate::cli::{LaunchFlags, SteamCommand};
use crate::output::{self, TerminalProgress, human_bytes};

/// Dispatch a `steam` subcommand.
pub(super) fn run(ctx: &Ctx, command: &SteamCommand) -> anyhow::Result<ExitCode> {
    match command {
        SteamCommand::Install { bottle, yes } => install(ctx, bottle.as_deref(), *yes),
        SteamCommand::Start { launch } => start(ctx, launch),
        SteamCommand::Games { bottle } => games(ctx, bottle.as_deref()),
        SteamCommand::Launch {
            appid,
            launch,
            args,
        } => {
            let options = launch_options(launch)?;
            let profiles = super::profile::load_profiles(ctx)?;
            let profile = profiles
                .iter()
                .find(|profile| profile.steam.as_ref().is_some_and(|s| s.appid == *appid));
            play_steam_game(ctx, profile, *appid, launch, &options, args)
        }
    }
}

fn install(ctx: &Ctx, bottle: Option<&str>, yes: bool) -> anyhow::Result<ExitCode> {
    let bottle = ctx.open_bottle(bottle)?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    if SteamInstall::find(bottle.prefix()).is_none() {
        confirm_download(
            &format!(
                "Valve's Steam installer ({}, from Valve)",
                human_bytes(STEAM_INSTALLER_BYTES)
            ),
            yes,
        )?;
    }
    install_and_start(ctx, &bottle, &wine)
}

/// Install Steam into `bottle` (skipped when it is there) and start the
/// client in the foreground so the user can sign in.
pub(super) fn install_and_start(
    ctx: &Ctx,
    bottle: &Bottle,
    wine: &WineRuntime,
) -> anyhow::Result<ExitCode> {
    if SteamInstall::find(bottle.prefix()).is_some() {
        eprintln!(
            "Steam is already installed in bottle {}",
            bottle.config.name
        );
    } else {
        eprintln!("Installing Steam into bottle {}", bottle.config.name);
    }
    uncork_core::steam::install(&ctx.layout, bottle, wine, &mut TerminalProgress::default())
        .with_context(|| format!("cannot install Steam into bottle {}", bottle.config.name))?;
    let components = ctx.components()?;
    start_client(bottle, wine, &components, &BTreeMap::new(), None)?;
    println!(
        "Sign in to Steam in the window that opened, install your game, then run: uncork play <game>"
    );
    println!(
        "The first start updates the client and can take 15 to 25 minutes; progress is in {}",
        uncork_core::steam::logs_dir(bottle)
            .join("bootstrap_log.txt")
            .display()
    );
    Ok(ExitCode::SUCCESS)
}

/// The client command with `--env` and `--wine-debug` applied.
fn client_spec(
    bottle: &Bottle,
    wine: &WineRuntime,
    env: &BTreeMap<String, String>,
    wine_debug: Option<&str>,
) -> anyhow::Result<CommandSpec> {
    let mut spec = uncork_core::steam::client_command(bottle, wine, &[])?;
    if let Some(channels) = wine_debug {
        spec.env.insert("WINEDEBUG".to_owned(), channels.to_owned());
    }
    spec.env.extend(env.clone());
    Ok(spec)
}

fn steam_missing(bottle: &Bottle) -> anyhow::Error {
    anyhow!(
        "Steam is not installed in bottle {name}; run: uncork steam install --bottle {name}",
        name = bottle.config.name
    )
}

/// Start the Steam client with its window visible (not `-silent`), after
/// putting DXVK next to its web helper. Does nothing when it already runs.
fn start_client(
    bottle: &Bottle,
    wine: &WineRuntime,
    components: &[InstalledComponent],
    env: &BTreeMap<String, String>,
    wine_debug: Option<&str>,
) -> anyhow::Result<Option<Child>> {
    let name = &bottle.config.name;
    let install = SteamInstall::find(bottle.prefix()).ok_or_else(|| steam_missing(bottle))?;
    if uncork_core::steam::is_running(bottle, wine)
        .context("cannot tell whether Steam is running")?
    {
        println!("Steam is already running in bottle {name}.");
        return Ok(None);
    }
    let dxvk = newest(components, ComponentKind::Dxvk);
    if !uncork_core::steam::ensure_client_dxvk(&install, dxvk)? {
        eprintln!(
            "warning: DXVK is not installed, so Steam's window will stay black; run: uncork runtime install dxvk"
        );
    }
    let spec = client_spec(bottle, wine, env, wine_debug)?;
    let child = uncork_core::process::spawn(&spec).context("cannot start Steam")?;
    println!("Started Steam in bottle {name} (pid {}).", child.id());
    if let Some(log) = &spec.log {
        println!("Log: {}", log.display());
    }
    Ok(Some(child))
}

fn start(ctx: &Ctx, flags: &LaunchFlags) -> anyhow::Result<ExitCode> {
    let options = launch_options(flags)?;
    if flags.backend.is_some() || flags.hud || flags.metalfx || flags.retina {
        eprintln!(
            "note: --backend, --hud, --metalfx and --retina apply to games; the Steam client always runs on DXVK"
        );
    }
    let bottle = ctx.open_bottle(flags.bottle.as_deref())?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    if flags.dry_run {
        if SteamInstall::find(bottle.prefix()).is_none() {
            return Err(steam_missing(&bottle));
        }
        let spec = client_spec(&bottle, &wine, &options.env, options.wine_debug.as_deref())?;
        if ctx.json {
            output::print_json(&spec)?;
        } else {
            if let Some(log) = &spec.log {
                println!("Log: {}", log.display());
            }
            println!("Command:\n  {}", spec.to_shell());
        }
        return Ok(ExitCode::SUCCESS);
    }
    let components = ctx.components()?;
    let child = start_client(
        &bottle,
        &wine,
        &components,
        &options.env,
        options.wine_debug.as_deref(),
    )?;
    if let (true, Some(mut child)) = (flags.wait, child) {
        let status = child.wait().context("cannot wait for Steam")?;
        uncork_core::process::run(&uncork_core::launch::in_bottle(
            wine.wait_command(bottle.prefix()),
            &bottle,
            &wine,
        ))
        .context("cannot wait for the bottle's wineserver")?;
        println!("Steam exited ({status}).");
    }
    Ok(ExitCode::SUCCESS)
}

/// One installed game in `steam games --json`.
#[derive(Debug, Serialize)]
struct GameRow<'a> {
    appid: u32,
    name: &'a str,
    /// `ready`, `updating` or `partial`.
    state: &'static str,
    state_flags: u32,
    size_on_disk: Option<u64>,
    build_id: Option<u64>,
    install_path: &'a Path,
    /// Id of the matching game profile, if there is one.
    profile: Option<&'a str>,
}

/// `StateFlags` bits that mean an update or download is pending or running:
/// update required, update running, paused, started, downloading, staging,
/// committing (Steam's `EAppState`).
const UPDATING_BITS: u32 = 2 | 256 | 512 | 1024 | 1_048_576 | 2_097_152 | 4_194_304;

/// `StateFlags` bits that rule out "ready" even when fully installed:
/// update required, files missing, files corrupt, update running, update
/// started, downloading (`docs/STEAM.md`).
const NOT_READY_BITS: u32 = 2 | 32 | 128 | 256 | 1024 | 1_048_576;

/// Ready (fully installed, nothing pending), Updating, or Partial.
fn app_state(state_flags: u32) -> &'static str {
    if state_flags & STATE_FULLY_INSTALLED != 0 && state_flags & NOT_READY_BITS == 0 {
        "ready"
    } else if state_flags & UPDATING_BITS != 0 {
        "updating"
    } else {
        "partial"
    }
}

/// `Ready`, `Updating` or `Partial` for the table.
fn state_label(state: &str) -> &'static str {
    match state {
        "ready" => "Ready",
        "updating" => "Updating",
        _ => "Partial",
    }
}

fn games(ctx: &Ctx, bottle: Option<&str>) -> anyhow::Result<ExitCode> {
    let bottle = ctx.open_bottle(bottle)?;
    let steam = SteamInstall::find(bottle.prefix()).ok_or_else(|| steam_missing(&bottle))?;
    let (apps, unreadable) = steam
        .installed_apps()
        .context("cannot read Steam's library folders")?;
    for err in &unreadable {
        eprintln!("warning: {err}");
    }
    let profiles = super::profile::load_profiles(ctx)?;
    let rows: Vec<GameRow<'_>> = apps
        .iter()
        .map(|app| {
            let manifest = &app.manifest;
            GameRow {
                appid: manifest.appid,
                name: &manifest.name,
                state: app_state(manifest.state_flags),
                state_flags: manifest.state_flags,
                size_on_disk: manifest.size_on_disk,
                build_id: manifest.build_id,
                install_path: &app.install_path,
                profile: profiles
                    .iter()
                    .find(|p| p.steam.as_ref().is_some_and(|s| s.appid == manifest.appid))
                    .map(|p| p.id.as_str()),
            }
        })
        .collect();
    if ctx.json {
        output::print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }
    if rows.is_empty() {
        println!(
            "No games installed in bottle {}'s Steam libraries yet. Install one in the Steam window (`uncork steam start`).",
            bottle.config.name
        );
        return Ok(ExitCode::SUCCESS);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.appid.to_string(),
                row.name.to_owned(),
                state_label(row.state).to_owned(),
                row.size_on_disk.map_or_else(String::new, human_bytes),
                row.profile.unwrap_or("").to_owned(),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(&["APPID", "NAME", "STATE", "SIZE", "PROFILE"], &table)
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states() {
        assert_eq!(app_state(4), "ready");
        assert_eq!(app_state(4 | 64), "ready", "running is still ready");
        assert_eq!(app_state(6), "updating");
        assert_eq!(app_state(4 | 1024), "updating");
        assert_eq!(app_state(1_048_576), "updating");
        assert_eq!(app_state(4 | 32), "partial");
        assert_eq!(app_state(0), "partial");
        assert_eq!(app_state(1), "partial");
        assert_eq!(state_label(app_state(4)), "Ready");
        assert_eq!(state_label(app_state(6)), "Updating");
        assert_eq!(state_label(app_state(0)), "Partial");
    }
}
