//! Running the Windows Steam client in a bottle, and playing Steam games.
//!
//! # Install ([`install`])
//!
//! 1. Download Valve's `SteamSetup.exe` ([`uncork_steam::client::INSTALLER_URLS`])
//!    into `cache/downloads/`. A hash different from
//!    [`uncork_steam::client::INSTALLER_SHA256`] is a warning (Valve replaces
//!    the file in place), not an error, but the file must parse as a 32-bit
//!    GUI PE.
//! 2. `wine SteamSetup.exe /S` (silent; installs to
//!    `C:\Program Files (x86)\Steam`), then `wineserver --kill` to stop the
//!    `steam.exe` the installer may start.
//! 3. Start the client once in the foreground so the user can sign in
//!    (Uncork never handles Steam credentials). The first start downloads
//!    the client and can take 15–25 minutes.
//!
//! # Graphics for the Steam client
//!
//! Steam's web UI (CEF) needs a Direct3D 11 device: Wine's OpenGL-based
//! WineD3D gives Chromium too low a feature level, DXMT refuses the
//! cross-process swapchains the web helper creates, and software CEF
//! (`-cef-disable-gpu`) leaves every window black on macOS. DXVK works.
//! Uncork copies DXVK's 64-bit `d3d11.dll` and `d3d10core.dll` (builtin
//! marker removed) into the web helper's own directory
//! ([`uncork_steam::client::CEF_DIR`]) before every start
//! ([`ensure_client_dxvk`]) and starts the client with
//! `d3d10core,d3d11=n,b` and `dxgi,d3d9,d3d12=b`. App-local DLLs are found
//! before `system32`, so games keep `system32`/`syswow64` for their own
//! backend and the two never conflict.
//!
//! # Play
//!
//! A game gets its own backend by being started *directly* in the same
//! prefix and wineserver while Steam runs ([`LaunchMode::Direct`]):
//! Steamworks finds the client through
//! `HKCU\Software\Valve\Steam\ActiveProcess`. Games whose DRM needs
//! Steam to start them use [`LaunchMode::Applaunch`]: `steam.exe -applaunch
//! <appid>`, where the game inherits the client's environment, so Uncork
//! restarts Steam with the game's backend environment first.

use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::bottle::Bottle;
use crate::download::Progress;
use crate::launch::{LaunchOptions, LaunchPlan, PlanContext};
use crate::paths::Layout;
use crate::wine::WineRuntime;

/// How a Steam game is started.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LaunchMode {
    /// Make sure Steam runs, then start the game executable directly.
    #[default]
    Direct,
    /// `steam.exe -applaunch <appid>` (Steam restarted with the game's env).
    Applaunch,
    /// No Steam involvement (DRM-free or non-Steam games).
    Standalone,
}

/// Download Valve's installer and install Steam into `bottle`.
/// Skips the installer when `steam.exe` already exists.
///
/// # Errors
/// Download, PE validation or command errors.
pub fn install(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    progress: &mut dyn Progress,
) -> crate::Result<()> {
    let _ = (layout, bottle, wine, progress);
    todo!()
}

/// The command that starts the Steam client in `bottle`: `env_clear`, the
/// base environment ([`crate::launch::base_env`]) plus the bottle's `env`,
/// `DXVK_LOG_LEVEL=none`, `WINEDLLOVERRIDES` =
/// `d3d10core,d3d11=n,b;d3d12,d3d9,dxgi=b;winemenubuilder.exe=d` (rendered
/// with [`crate::graphics::render_overrides`]), cwd = the Steam directory,
/// log = `logs/<bottle>-steam-<unix secs>.log`, arguments from
/// [`uncork_steam::client::client_args`] with `extra` appended.
///
/// # Errors
/// [`crate::Error::NotFound`] when Steam is not installed in the bottle.
pub fn client_command(
    bottle: &Bottle,
    wine: &WineRuntime,
    extra: &[String],
) -> crate::Result<crate::process::CommandSpec> {
    let _ = (bottle, wine, extra);
    todo!()
}

/// Put DXVK's 64-bit `d3d11.dll` and `d3d10core.dll` (from the component's
/// `x86_64-windows`, builtin marker stripped with
/// [`uncork_pe::strip_builtin_marker`]) into `<steam>/`
/// [`uncork_steam::client::CEF_DIR`], skipping files that are already
/// identical. Returns `Ok(false)` without copying when `dxvk` is `None` (the
/// caller warns that the Steam window will be black), `Ok(true)` otherwise.
///
/// # Errors
/// [`crate::Error::Io`], or [`crate::Error::BrokenComponent`] if the DXVK
/// component lacks the DLLs.
pub fn ensure_client_dxvk(
    steam: &uncork_steam::SteamInstall,
    dxvk: Option<&crate::component::InstalledComponent>,
) -> crate::Result<bool> {
    let _ = (steam, dxvk);
    todo!()
}

/// Ask the client to exit (`steam.exe -shutdown`), wait up to `timeout` for
/// [`is_running`] to report it gone, then stop everything left in the
/// prefix with `wineserver --kill` (measured: `-shutdown` alone can leave
/// the client running for over a minute). Not running is a no-op.
///
/// # Errors
/// Command errors.
pub fn stop(bottle: &Bottle, wine: &WineRuntime, timeout: Duration) -> crate::Result<()> {
    let _ = (bottle, wine, timeout);
    todo!()
}

/// Is a Steam client running in this bottle? Reads
/// `HKCU\Software\Valve\Steam\ActiveProcess\pid` with `wine reg query`
/// (non-zero means running) — authoritative because `user.reg` on disk is
/// flushed lazily.
///
/// # Errors
/// Command errors other than "key not found" (which means not running).
pub fn is_running(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<bool> {
    let _ = (bottle, wine);
    todo!()
}

/// Start Steam (`-silent`) if it is not running and wait until
/// [`is_running`] reports it, polling every 2 s up to `timeout`. Calls
/// [`ensure_client_dxvk`] first (with the newest installed DXVK).
///
/// # Errors
/// [`crate::Error::Command`] on timeout, naming the client log.
pub fn ensure_running(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
) -> crate::Result<()> {
    let _ = (layout, bottle, wine, timeout);
    todo!()
}

/// Plan playing Steam app `appid`: find it in the bottle's libraries, pick
/// the executable (profile `exe.path`, else the largest `.exe` in the
/// install directory that is not a known launcher/crash reporter/redist),
/// and plan a launch of it ([`crate::launch::plan`]).
///
/// # Errors
/// [`crate::Error::NotFound`] if the game is not installed (hint: install it
/// in Steam first), plus planning errors.
pub fn plan_game(
    ctx: PlanContext<'_>,
    appid: u32,
    args: &[String],
    options: &LaunchOptions,
) -> crate::Result<LaunchPlan> {
    let _ = (ctx, appid, args, options);
    todo!()
}

/// Steam's own log directory in the bottle, for error messages.
#[must_use]
pub fn logs_dir(bottle: &Bottle) -> std::path::PathBuf {
    bottle
        .drive_c()
        .join(Path::new(uncork_steam::client::STEAM_DIR))
        .join("logs")
}
