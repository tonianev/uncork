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
//! # Play ([`play`])
//!
//! The Steam client always runs on WineD3D (its CEF UI breaks on DXMT and
//! D3DMetal, which cannot present across processes). A game gets its own
//! backend by being started *directly* in the same prefix and wineserver
//! while Steam runs ([`LaunchMode::Direct`]): Steamworks finds the client
//! through `HKCU\Software\Valve\Steam\ActiveProcess`. Games whose DRM needs
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

/// The command that starts the Steam client in `bottle` with the base
/// environment ([`crate::launch::base_env`]), WineD3D for every Direct3D
/// DLL (`d3d11,dxgi,d3d10core,d3d9,d3d12=b`), the default client flags
/// ([`uncork_steam::client::default_client_args`]) and `extra` appended.
///
/// # Errors
/// [`crate::Error::NotFound`] when Steam is not installed in the bottle.
pub fn client_command(bottle: &Bottle, wine: &WineRuntime, extra: &[String]) -> crate::Result<crate::process::CommandSpec> {
    let _ = (bottle, wine, extra);
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
/// [`is_running`] reports it, polling every 2 s up to `timeout`.
///
/// # Errors
/// [`crate::Error::Command`] on timeout, naming the client log.
pub fn ensure_running(layout: &Layout, bottle: &Bottle, wine: &WineRuntime, timeout: Duration) -> crate::Result<()> {
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
pub fn plan_game(ctx: PlanContext<'_>, appid: u32, args: &[String], options: &LaunchOptions) -> crate::Result<LaunchPlan> {
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
