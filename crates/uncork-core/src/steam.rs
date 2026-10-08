//! Running the Windows Steam client in a bottle, and playing Steam games.
//!
//! # Install ([`install`])
//!
//! 1. Download Valve's `SteamSetup.exe` ([`uncork_steam::client::INSTALLER_URLS`])
//!    into `cache/downloads/`, unless the copy there already has the pinned
//!    hash [`uncork_steam::client::INSTALLER_SHA256`]. A downloaded file
//!    with another hash is a warning (Valve replaces the file in place), not
//!    an error, but the file must parse as a 32-bit GUI PE.
//! 2. `wine SteamSetup.exe /S` (silent; installs to
//!    `C:\Program Files (x86)\Steam`), then `wineserver --kill` to stop the
//!    `steam.exe` the installer may start.
//! 3. Start the client once in the foreground so the user can sign in
//!    (Uncork never handles Steam credentials). The first start downloads
//!    the client (about 340 MB; measured at about a minute on a fast
//!    connection, longer on slow ones).
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
//! `HKCU\Software\Valve\Steam\ActiveProcess`, and the game gets
//! `SteamAppId`/`SteamGameId` as if Steam had started it. Games whose DRM needs
//! Steam to start them use [`LaunchMode::Applaunch`]: `steam.exe -applaunch
//! <appid>`, where the game inherits the client's environment, so Uncork
//! restarts Steam with the game's backend environment first.
//!
//! ## The `-applaunch` trade-off
//!
//! In `Applaunch` mode the client runs with the *game's* environment and
//! DLL overrides, except that `d3d10core,d3d11=n,b` stays so the web helper
//! still finds its app-local DXVK. The game's own backend DLLs come from
//! `system32`/`syswow64` ([`crate::graphics::Strategy::PrefixNative`]), so
//! the game is unaffected. The client can be: with a DXMT game the web
//! helper inherits `dxgi=n,b` and loads DXMT's `dxgi` next to DXVK's
//! `d3d11`, a pairing that fails to create swapchains, so Steam's own
//! windows can stay black until it is restarted normally. `Direct` mode has
//! no such coupling and is the default.
//!
//! # Detecting the client
//!
//! [`is_running`] reads the `ActiveProcess` `pid`, which the client sets on
//! start and clears on a clean exit. A client that was killed leaves its
//! pid behind; [`stop`] clears it after killing ([`forget_client`]), and
//! anything else that kills a bottle's processes should do the same. A pid
//! found while no wineserver runs for the prefix (the client crashed, was
//! force-quit, or the Mac restarted) is stale: [`is_running`] clears it
//! and reports the client as not running.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Output, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use uncork_pe::Bitness;
use uncork_steam::client;
use uncork_steam::{InstalledApp, SteamInstall};

use crate::Error;
use crate::bottle::Bottle;
use crate::component::{ComponentKind, InstalledComponent};
use crate::display::Display;
use crate::download::Progress;
use crate::launch::{LaunchOptions, LaunchPlan, OVERRIDES_VAR, PlanContext, Target};
use crate::paths::Layout;
use crate::process::CommandSpec;
use crate::profile::GameProfile;
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

/// How often [`ensure_running`] and [`stop`] ask whether the client runs.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long a running client gets to exit after `-shutdown` whenever Uncork
/// restarts a bottle ([`play`] in [`LaunchMode::Applaunch`] mode, a changed
/// main display, a Retina, DPI or Windows-version change) before the bottle
/// is stopped with `wineserver --kill` (measured: `-shutdown` alone can take
/// over a minute).
pub const SHUTDOWN_GRACE: Duration = Duration::from_secs(15);

/// The variables Steam sets for the games it starts, set to the app id in
/// [`LaunchMode::Direct`] mode so Steamworks knows the game (and does not
/// ask Steam to start it again) without a `steam_appid.txt`.
const STEAM_APP_VARS: [&str; 2] = ["SteamAppId", "SteamGameId"];

/// The installer's file name in `cache/downloads/`.
const INSTALLER_FILE: &str = "SteamSetup.exe";

/// PE optional-header subsystem of a Windows GUI program.
const SUBSYSTEM_WINDOWS_GUI: u16 = 2;

/// Wine's `reg.exe` exits with 1 when the key or value does not exist.
const REG_NOT_FOUND: i32 = 1;

/// The DXVK DLLs the client's web helper gets, app-local.
const CLIENT_DXVK_DLLS: [&str; 2] = ["d3d11.dll", "d3d10core.dll"];

/// The DLLs the client loads natively (its app-local DXVK, see the module docs).
const CLIENT_NATIVE: [&str; 2] = ["d3d10core", "d3d11"];

/// The DLLs the client always takes from Wine, so backend DLLs a game left
/// in `system32` never reach it.
const CLIENT_BUILTIN: [&str; 3] = ["d3d12", "d3d9", "dxgi"];

/// How deep below the install directory [`plan_game`] looks for the game
/// executable when no profile names it.
const EXE_SEARCH_DEPTH: usize = 3;

/// Download Valve's installer and install Steam into `bottle`.
/// Skips the installer when `Steam.exe` already exists, and the download
/// when `cache/downloads/SteamSetup.exe` already has the pinned SHA-256
/// ([`uncork_steam::client::INSTALLER_SHA256`]); a cached file with any
/// other hash is downloaded again.
///
/// The installer runs as `wine <cache>/SteamSetup.exe /S` with the bottle's
/// environment ([`crate::launch::base_env`] plus its `env`,
/// `WINEDLLOVERRIDES=winemenubuilder.exe=d`), logging to
/// `logs/<bottle>-steam-install.log`; then `wineserver --kill` stops the
/// client the installer may have started. Steam is not started here.
///
/// # Errors
/// Download, PE validation or command errors; a [`crate::Error::Command`]
/// naming the log when the installer ran but `steam.exe` is missing.
pub fn install(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    progress: &mut dyn Progress,
) -> crate::Result<()> {
    install_with(layout, bottle, wine, client::INSTALLER_SHA256, |dest| {
        download_installer(dest, progress)
    })
}

/// [`install`] with the pinned hash and the download step supplied by the
/// caller (tests); `fetch` writes the installer to the given path and
/// returns its URL.
fn install_with(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    pinned_sha256: &str,
    fetch: impl FnOnce(&Path) -> crate::Result<String>,
) -> crate::Result<()> {
    if let Some(steam) = SteamInstall::find(bottle.prefix()) {
        tracing::info!(
            "Steam is already installed at {}; skipping the installer",
            steam.root.display()
        );
        return Ok(());
    }
    let installer = layout.downloads_dir().join(INSTALLER_FILE);
    let url = if cached_installer_is_pinned(&installer, pinned_sha256) {
        tracing::info!("using the cached {} (SHA-256 matches)", installer.display());
        installer.display().to_string()
    } else {
        fetch(&installer)?
    };
    check_installer(&installer, &url, pinned_sha256)?;

    let log = layout.logs_dir().join(format!(
        "{}-steam-install.log",
        crate::launch::sanitize_file_part(&bottle.config.name)
    ));
    tracing::info!(
        "installing Steam into bottle {} (log: {})",
        bottle.config.name,
        log.display()
    );
    let mut setup = CommandSpec::new(wine.wine_bin())
        .arg(&installer)
        .arg(client::INSTALLER_SILENT);
    setup.env_clear = true;
    setup.env = tool_env(bottle, wine);
    setup.log = Some(log.clone());
    crate::process::run(&setup)?;

    // The installer can start steam.exe on its own; the first start belongs
    // to the user, in the foreground.
    let kill = CommandSpec {
        log: Some(log.clone()),
        ..crate::launch::in_bottle(wine.kill_command(bottle.prefix()), bottle, wine)
    };
    if let Err(err) = crate::process::run(&kill) {
        tracing::debug!("wineserver --kill after the Steam installer: {err}");
    }

    if SteamInstall::find(bottle.prefix()).is_none() {
        return Err(Error::Command {
            program: INSTALLER_FILE.to_owned(),
            status: format!(
                "finished, but {} is missing",
                bottle
                    .drive_c()
                    .join(client::STEAM_DIR)
                    .join(client::STEAM_EXE)
                    .display()
            ),
            log_hint: format!(" (log: {})", log.display()),
        });
    }
    Ok(())
}

/// Download the installer from the first of [`client::INSTALLER_URLS`] that
/// works. Returns the URL used.
fn download_installer(dest: &Path, progress: &mut dyn Progress) -> crate::Result<String> {
    let mut last_error = None;
    for url in client::INSTALLER_URLS {
        match crate::download::download(url, dest, None, progress) {
            Ok(()) => return Ok((*url).to_owned()),
            Err(err) => {
                tracing::warn!("cannot download {url}: {err}");
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| Error::Download {
        url: String::new(),
        message: "no installer URL is known".to_owned(),
    }))
}

/// `true` when `path` exists and its SHA-256 is `pinned_sha256`.
fn cached_installer_is_pinned(path: &Path, pinned_sha256: &str) -> bool {
    match crate::download::sha256_file(path) {
        Ok(sha256) => sha256.eq_ignore_ascii_case(pinned_sha256),
        Err(err) => {
            tracing::debug!("no usable cached installer: {err}");
            false
        }
    }
}

/// Warn on a changed hash; fail (and delete the file) unless it is a 32-bit
/// Windows GUI program.
fn check_installer(path: &Path, url: &str, pinned_sha256: &str) -> crate::Result<()> {
    let sha256 = crate::download::sha256_file(path)?;
    if !sha256.eq_ignore_ascii_case(pinned_sha256) {
        tracing::warn!(
            "{INSTALLER_FILE} from {url} has SHA-256 {sha256}, not {pinned_sha256} as last checked; Valve replaces the installer in place, so continuing"
        );
    }
    let rejected = |detail: String| {
        if let Err(err) = fs::remove_file(path) {
            tracing::warn!("cannot remove {}: {err}", path.display());
        }
        Error::Download {
            url: url.to_owned(),
            message: format!("not a Windows installer ({detail})"),
        }
    };
    let info = uncork_pe::inspect(path).map_err(|err| rejected(err.to_string()))?;
    if info.bitness != Some(Bitness::X86) || info.subsystem != SUBSYSTEM_WINDOWS_GUI || info.is_dll
    {
        return Err(rejected(format!(
            "expected a 32-bit GUI program, found a {:?} image with subsystem {}{}",
            info.machine,
            info.subsystem,
            if info.is_dll { " (a DLL)" } else { "" }
        )));
    }
    Ok(())
}

/// The command that starts the Steam client in `bottle`: `env_clear`, the
/// base environment ([`crate::launch::base_env`]) plus the bottle's `env`,
/// `DXVK_LOG_LEVEL=none`, `WINEDLLOVERRIDES` = `d3d10core,d3d11=n,b`,
/// `d3d12,d3d9,dxgi=b` and `winemenubuilder.exe=d`, rendered with
/// [`crate::graphics::render_overrides`] (groups ordered by value:
/// `d3d12,d3d9,dxgi=b;winemenubuilder.exe=d;d3d10core,d3d11=n,b`), cwd = the Steam directory,
/// log = `logs/<bottle>-steam-<unix secs>.log`, arguments from
/// [`uncork_steam::client::client_args`] with `extra` appended.
///
/// `DXVK_LOG_LEVEL=none` is set before the bottle's `env`, so a bottle can
/// turn DXVK's logging on; `WINEDLLOVERRIDES` is always exactly the value
/// above. Flags [`uncork_steam::client::client_args`] strips are logged as
/// a warning. The log directory is the data root's `logs/` for a bottle in
/// `$UNCORK_HOME/bottles/`, else the bottle's own `logs/`.
///
/// # Errors
/// [`crate::Error::NotFound`] when Steam is not installed in the bottle.
pub fn client_command(
    bottle: &Bottle,
    wine: &WineRuntime,
    extra: &[String],
) -> crate::Result<crate::process::CommandSpec> {
    let steam = find_steam(bottle)?;
    let (args, stripped) = client::client_args(extra);
    warn_stripped(&stripped);
    Ok(client_spec(bottle, wine, &steam, args))
}

/// [`client_command`] with its argument list already built.
fn client_spec(
    bottle: &Bottle,
    wine: &WineRuntime,
    steam: &SteamInstall,
    args: Vec<String>,
) -> CommandSpec {
    let mut env = crate::launch::base_env(bottle, wine, None);
    env.insert("DXVK_LOG_LEVEL".to_owned(), "none".to_owned());
    env.extend(bottle.config.env.clone());
    env.insert(
        OVERRIDES_VAR.to_owned(),
        crate::graphics::render_overrides(&client_overrides()),
    );
    steam_spec(bottle, wine, steam, args, env)
}

/// `wine <steam.exe> <args>` in the Steam directory, logging to a new
/// client log.
fn steam_spec(
    bottle: &Bottle,
    wine: &WineRuntime,
    steam: &SteamInstall,
    args: Vec<String>,
    env: BTreeMap<String, String>,
) -> CommandSpec {
    let log = crate::launch::logs_dir_for(bottle).join(format!(
        "{}-steam-{}.log",
        crate::launch::sanitize_file_part(&bottle.config.name),
        crate::unix_now()
    ));
    CommandSpec {
        program: wine.wine_bin(),
        args: std::iter::once(steam.exe().into_os_string())
            .chain(args.into_iter().map(Into::into))
            .collect(),
        env,
        env_clear: true,
        cwd: Some(steam.root.clone()),
        log: Some(log),
    }
}

/// The client's DLL overrides (see the module docs).
fn client_overrides() -> BTreeMap<String, String> {
    let (menu, disabled) = crate::launch::NO_MENU_BUILDER;
    CLIENT_NATIVE
        .iter()
        .map(|dll| ((*dll).to_owned(), "n,b".to_owned()))
        .chain(
            CLIENT_BUILTIN
                .iter()
                .map(|dll| ((*dll).to_owned(), "b".to_owned())),
        )
        .chain(std::iter::once((menu.to_owned(), disabled.to_owned())))
        .collect()
}

fn warn_stripped(stripped: &[String]) {
    if !stripped.is_empty() {
        tracing::warn!(
            "not passing {} to Steam: dead or harmful client flags (see docs/STEAM.md)",
            stripped.join(", ")
        );
    }
}

/// Put DXVK's 64-bit `d3d11.dll` and `d3d10core.dll` (from the component's
/// `x86_64-windows`, builtin marker stripped with
/// [`uncork_pe::strip_builtin_marker`]) into `<steam>/`
/// [`uncork_steam::client::CEF_DIR`], skipping files that are already
/// identical. Returns `Ok(false)` without copying when `dxvk` is `None` (the
/// caller warns that the Steam window will be black), `Ok(true)` otherwise.
///
/// Both DLLs are read before either is written, and each is replaced
/// atomically, so a running web helper never sees a half-written file. The
/// directory is created when the client has not unpacked it yet.
///
/// # Errors
/// [`crate::Error::Io`], or [`crate::Error::BrokenComponent`] if the DXVK
/// component lacks the DLLs.
pub fn ensure_client_dxvk(
    steam: &uncork_steam::SteamInstall,
    dxvk: Option<&crate::component::InstalledComponent>,
) -> crate::Result<bool> {
    let Some(dxvk) = dxvk else {
        return Ok(false);
    };
    let broken = |message: String| Error::BrokenComponent {
        kind: dxvk.meta.kind,
        path: dxvk.path.clone(),
        message,
    };
    if dxvk.meta.kind != ComponentKind::Dxvk {
        return Err(broken(format!(
            "expected a {} component",
            ComponentKind::Dxvk
        )));
    }
    let mut dlls = Vec::with_capacity(CLIENT_DXVK_DLLS.len());
    for dll in CLIENT_DXVK_DLLS {
        let source = dxvk.path.join("x86_64-windows").join(dll);
        let mut bytes = match fs::read(&source) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                return Err(broken(format!("x86_64-windows/{dll} is missing")));
            }
            Err(err) => return Err(Error::io("cannot read", &source, err)),
        };
        uncork_pe::strip_builtin_marker(&mut bytes);
        dlls.push((dll, bytes));
    }

    let dir = steam.root.join(client::CEF_DIR);
    fs::create_dir_all(&dir).map_err(|err| Error::io("cannot create directory", &dir, err))?;
    for (dll, bytes) in dlls {
        let dest = dir.join(dll);
        match fs::read(&dest) {
            Ok(existing) if existing == bytes => continue,
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(Error::io("cannot read", &dest, err)),
        }
        crate::config::write_atomic(&dest, &bytes)?;
    }
    Ok(true)
}

/// Ask the client to exit (`steam.exe -shutdown`), wait up to `timeout` for
/// [`is_running`] to report it gone, then stop everything left in the
/// prefix with `wineserver --kill` (measured: `-shutdown` alone can leave
/// the client running for over a minute). Not running is a no-op.
///
/// `wineserver --kill` only runs when the client is still running after
/// `timeout` (it ends every process in the bottle, games included); it is
/// followed by [`forget_client`], since a killed client cannot clear its
/// `ActiveProcess` pid. The state is polled every 2 s (less near the
/// deadline).
///
/// # Errors
/// Command errors.
pub fn stop(bottle: &Bottle, wine: &WineRuntime, timeout: Duration) -> crate::Result<()> {
    stop_with(bottle, wine, timeout, POLL_INTERVAL)
}

/// [`stop`] with a configurable polling interval.
fn stop_with(
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
    poll: Duration,
) -> crate::Result<()> {
    if !is_running(bottle, wine)? {
        return Ok(());
    }
    let name = &bottle.config.name;
    match SteamInstall::find(bottle.prefix()) {
        Some(steam) => {
            let shutdown = client_spec(bottle, wine, &steam, client::shutdown_args());
            match crate::process::spawn(&shutdown) {
                Ok(child) => reap_in_background(child),
                Err(err) => tracing::warn!("cannot ask Steam in bottle {name} to exit: {err}"),
            }
            if wait_for(bottle, wine, timeout, poll, false)? {
                return Ok(());
            }
            tracing::warn!(
                "Steam in bottle {name} did not exit within {timeout:?}; stopping every process in the bottle"
            );
        }
        None => tracing::warn!(
            "bottle {name} reports a running Steam client but has no steam.exe; stopping every process in the bottle"
        ),
    }
    let kill = CommandSpec {
        log: Some(crate::launch::logs_dir_for(bottle).join(format!(
            "{}-steam-stop.log",
            crate::launch::sanitize_file_part(name)
        ))),
        ..crate::launch::in_bottle(wine.kill_command(bottle.prefix()), bottle, wine)
    };
    crate::process::run(&kill)?;
    forget_client(bottle, wine)
}

/// Stop everything that runs in `bottle`, the Steam client cleanly where it
/// can, so that the next Wine process starts a fresh wineserver (which
/// reads the displays, Retina mode and the DPI anew). Returns `false`
/// without doing anything when no wineserver runs
/// ([`server_running`]), `true` after stopping one:
///
/// 1. When Steam is installed in the bottle, [`stop`] it: a running client
///    gets `-shutdown` and `grace` to exit before `wineserver --kill`.
/// 2. `wineserver --kill` ends whatever is left (games, Wine's own
///    services); it failing because nothing is left is fine.
/// 3. When Steam is installed, reset its running marker ([`forget_client`];
///    a failure is logged), since a killed client cannot.
/// 4. `wineserver --wait`, so the wineserver step 3 started has exited.
///
/// The kill's output goes to `logs/<bottle>-stop.log`.
///
/// # Errors
/// Command errors.
pub fn stop_bottle(bottle: &Bottle, wine: &WineRuntime, grace: Duration) -> crate::Result<bool> {
    stop_bottle_with(bottle, wine, grace, POLL_INTERVAL)
}

/// [`stop_bottle`] with a configurable polling interval.
fn stop_bottle_with(
    bottle: &Bottle,
    wine: &WineRuntime,
    grace: Duration,
    poll: Duration,
) -> crate::Result<bool> {
    if !server_running(bottle, wine)? {
        return Ok(false);
    }
    let name = &bottle.config.name;
    tracing::info!("stopping every Windows program in bottle {name}");
    let steam = SteamInstall::find(bottle.prefix()).is_some();
    if steam {
        stop_with(bottle, wine, grace, poll)?;
    }
    let kill = CommandSpec {
        log: Some(crate::launch::logs_dir_for(bottle).join(format!(
            "{}-stop.log",
            crate::launch::sanitize_file_part(name)
        ))),
        ..crate::launch::in_bottle(wine.kill_command(bottle.prefix()), bottle, wine)
    };
    match crate::process::run(&kill) {
        Ok(()) => {}
        // `wineserver --kill` exits 1 when nothing was left to stop.
        Err(Error::Command { status, .. }) if status.starts_with("exited") => {
            tracing::debug!("wineserver --kill in bottle {name}: {status}");
        }
        Err(err) => return Err(err),
    }
    if steam && let Err(err) = forget_client(bottle, wine) {
        tracing::warn!("cannot reset Steam's running marker in bottle {name}: {err}");
    }
    crate::process::run(&crate::launch::in_bottle(
        wine.wait_command(bottle.prefix()),
        bottle,
        wine,
    ))?;
    Ok(true)
}

/// The main display changed while a bottle ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DisplayChange {
    /// The main display when the bottle's Wine session started
    /// ([`crate::bottle::BottleState::session_display`]).
    pub before: String,
    /// The main display now ([`Display::signature`]).
    pub now: String,
}

/// Whether the main display has changed since the running Wine session of
/// `bottle` started: Wine reads the displays when its wineserver starts, so
/// after another display becomes the main one (often when a display is
/// plugged in or unplugged) or the main display's "looks like" size
/// changes, a game started in that session sees the old one (wrong sizes
/// and offsets). Compares [`Display::signature`]s, which leave out the
/// refresh rate (a recorded one with a rate is read without it,
/// [`crate::display::without_refresh`]); a display that is not the main
/// one does not count. `None` when either signature is unknown (a probe
/// that failed never triggers a restart), when they are equal, or when no
/// wineserver runs (the next session starts fresh); only then is the
/// wineserver asked about.
///
/// # Errors
/// [`server_running`]'s.
pub fn display_change(
    bottle: &Bottle,
    wine: &WineRuntime,
    display: Option<&Display>,
) -> crate::Result<Option<DisplayChange>> {
    let (Some(before), Some(now)) = (
        bottle
            .state
            .session_display
            .as_deref()
            .map(crate::display::without_refresh),
        display.map(Display::signature),
    ) else {
        return Ok(None);
    };
    if before == now || !server_running(bottle, wine)? {
        return Ok(None);
    }
    Ok(Some(DisplayChange {
        before: before.to_owned(),
        now,
    }))
}

/// Record that no Steam client runs in `bottle` (set
/// `HKCU\Software\Valve\Steam\ActiveProcess\pid` to 0 with `wine reg add`).
/// For after the bottle's processes were killed: a killed client leaves its
/// pid behind and [`is_running`] would keep reporting it. Never call it
/// while the client may still run.
///
/// # Errors
/// Command errors.
pub fn forget_client(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<()> {
    let spec = reg_command(
        bottle,
        wine,
        &[
            "add",
            client::ACTIVE_PROCESS_KEY,
            "/v",
            "pid",
            "/t",
            "REG_DWORD",
            "/d",
            "0",
            "/f",
        ],
    );
    let output = quiet_output(&spec, "wine")?;
    if output.status.success() {
        Ok(())
    } else {
        Err(Error::Command {
            program: "wine reg add".to_owned(),
            status: describe_exit(output.status),
            log_hint: String::new(),
        })
    }
}

/// `HKCU\Software\Valve\Steam\ActiveProcess` as a section name of
/// `user.reg` (relative to `HKEY_CURRENT_USER`, backslashes doubled).
const ACTIVE_PROCESS_SECTION: &[u8] = br"Software\\Valve\\Steam\\ActiveProcess";

/// [`forget_client`] without Wine, for a prefix no wineserver runs for (a
/// bottle just imported): set the `ActiveProcess` `pid` in the prefix's
/// `user.reg` to `dword:00000000` by editing the file. Returns whether it
/// changed. A missing `user.reg`, section or value is left alone; the rest
/// of the file is kept byte for byte.
///
/// # Errors
/// [`crate::Error::Io`] when `user.reg` exists but cannot be read or written.
pub(crate) fn forget_client_on_disk(prefix: &Path) -> crate::Result<bool> {
    let path = prefix.join("user.reg");
    let text = match fs::read(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(Error::io("cannot read", &path, err)),
    };
    match zero_active_pid(&text) {
        Some(edited) => {
            crate::config::write_atomic(&path, &edited)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

/// `user.reg` text with a non-zero `ActiveProcess` `pid` set to 0, or
/// `None` when there is nothing to change.
fn zero_active_pid(text: &[u8]) -> Option<Vec<u8>> {
    const PID: &[u8] = b"\"pid\"=";
    const ZERO: &[u8] = b"dword:00000000";
    let mut out = Vec::with_capacity(text.len());
    let mut in_section = false;
    let mut changed = false;
    for line in text.split_inclusive(|byte| *byte == b'\n') {
        let content = line.trim_ascii_end();
        if let Some(header) = content.strip_prefix(b"[") {
            in_section = header
                .split(|byte| *byte == b']')
                .next()
                .is_some_and(|name| name.eq_ignore_ascii_case(ACTIVE_PROCESS_SECTION));
        } else if in_section
            && content.len() >= PID.len()
            && content[..PID.len()].eq_ignore_ascii_case(PID)
            && !content[PID.len()..].eq_ignore_ascii_case(ZERO)
        {
            out.extend_from_slice(PID);
            out.extend_from_slice(ZERO);
            out.extend_from_slice(&line[content.len()..]);
            changed = true;
            continue;
        }
        out.extend_from_slice(line);
    }
    changed.then_some(out)
}

/// Is a Steam client running in this bottle? Reads
/// `HKCU\Software\Valve\Steam\ActiveProcess\pid` with `wine reg query`
/// (non-zero means running) — authoritative because `user.reg` on disk is
/// flushed lazily.
///
/// A non-zero pid is trusted only when a wineserver was already running for
/// the prefix before the query (asked with
/// [`WineRuntime::server_probe_command`] first, since the query itself
/// starts one): a client that crashed, was force-quit or did not survive a
/// restart of the Mac leaves its pid behind, and with no wineserver no
/// client can be running. Such a stale pid is cleared ([`forget_client`])
/// and reported as not running.
///
/// The commands run with the bottle's environment (so a wineserver they
/// start has the bottle's msync setting) and their output is captured, not
/// shown. `reg` exiting with status 1 after printing its own `reg: ...`
/// message (`reg: Unable to find the specified registry key`, translated
/// in other languages) means the key or value does not exist.
///
/// # Errors
/// Command errors other than "key not found" (which means not running). A
/// Wine client that exits without `reg`'s message (for example because its
/// msync setting differs from the running wineserver's) is an error whose
/// message includes its output and suggests `uncork bottle kill <bottle>`.
pub fn is_running(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<bool> {
    client_status(bottle, wine).map(|status| status.running)
}

/// What [`client_status`] found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClientStatus {
    /// A wineserver ran for the bottle before the query (which starts one).
    pub server: bool,
    /// The Steam client runs ([`is_running`]).
    pub running: bool,
}

/// [`is_running`], also saying whether a wineserver ran before the query:
/// when none did, whatever Uncork starts next begins a new Wine session.
///
/// # Errors
/// As [`is_running`].
pub fn client_status(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<ClientStatus> {
    // Asked before the query, which starts a wineserver of its own.
    let server = server_running(bottle, wine)?;
    let running = match active_pid(bottle, wine)? {
        None | Some(0) => false,
        Some(_) if server => true,
        Some(pid) => {
            tracing::info!(
                "Steam's ActiveProcess pid {pid:#x} in bottle {} is left over from a client that did not exit cleanly (no wineserver was running); clearing it",
                bottle.config.name
            );
            if let Err(err) = forget_client(bottle, wine) {
                tracing::warn!(
                    "cannot clear Steam's stale running marker in bottle {}: {err}",
                    bottle.config.name
                );
            }
            false
        }
    };
    Ok(ClientStatus { server, running })
}

/// Whether a wineserver runs for `bottle`
/// ([`WineRuntime::server_probe_command`], with the bottle's environment;
/// output captured). An exit status other than 0 or 1 means the answer is
/// unknown, which counts as running, so nothing is thrown away or skipped
/// on a guess.
///
/// # Errors
/// [`crate::Error::Command`] when `wineserver` cannot be started.
pub fn server_running(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<bool> {
    let spec = crate::launch::in_bottle(wine.server_probe_command(bottle.prefix()), bottle, wine);
    let output = quiet_output(&spec, "wineserver")?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => {
            tracing::debug!(
                "wineserver -k0 for bottle {} {}; assuming a wineserver runs",
                bottle.config.name,
                describe_exit(output.status)
            );
            Ok(true)
        }
    }
}

/// The `ActiveProcess` pid, or `None` when the key or value does not exist
/// (see [`is_running`]).
fn active_pid(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<Option<u32>> {
    let spec = reg_command(
        bottle,
        wine,
        &["query", client::ACTIVE_PROCESS_KEY, "/v", "pid"],
    );
    let output = quiet_output(&spec, "wine")?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(client::parse_active_pid(&stdout));
    }
    if output.status.code() == Some(REG_NOT_FOUND) && is_reg_message(&stderr) {
        tracing::debug!(
            "no Steam ActiveProcess in bottle {}: {}",
            bottle.config.name,
            stderr.trim()
        );
        return Ok(None);
    }
    let printed = stderr.trim();
    Err(Error::Command {
        program: "wine reg query".to_owned(),
        status: format!(
            "{} ({}); if the bottle's Wine or msync setting changed while it was running, restart it with `uncork bottle kill {}`",
            describe_exit(output.status),
            if printed.is_empty() {
                "no output".to_owned()
            } else {
                format!("output: {printed}")
            },
            bottle.config.name
        ),
        log_hint: String::new(),
    })
}

/// `true` when `stderr` holds one of `reg`'s own messages. Wine's `reg`
/// starts every message with `reg: `, in every translation
/// (`reg: Unable to find the specified registry key`, `reg: Der angegebene
/// Schlüssel wurde nicht gefunden`).
fn is_reg_message(stderr: &str) -> bool {
    stderr
        .lines()
        .any(|line| line.trim_start().starts_with("reg:"))
}

/// `wine reg <args>` with the bottle's environment.
fn reg_command(bottle: &Bottle, wine: &WineRuntime, args: &[&str]) -> CommandSpec {
    let mut spec = CommandSpec::new(wine.wine_bin()).arg("reg").args(args);
    spec.env_clear = true;
    spec.env = tool_env(bottle, wine);
    spec
}

/// The environment for Wine's own tools in `bottle`: the base environment,
/// the bottle's `env`, and `WINEDLLOVERRIDES=winemenubuilder.exe=d`.
fn tool_env(bottle: &Bottle, wine: &WineRuntime) -> BTreeMap<String, String> {
    let mut env = crate::launch::base_env(bottle, wine, None);
    env.extend(bottle.config.env.clone());
    let (menu, disabled) = crate::launch::NO_MENU_BUILDER;
    env.insert(OVERRIDES_VAR.to_owned(), format!("{menu}={disabled}"));
    env
}

/// Run `spec` to completion with stdout and stderr captured (Wine prints
/// status lines such as `msync: up and running.` to stderr); `program`
/// names it in errors.
fn quiet_output(spec: &CommandSpec, program: &str) -> crate::Result<Output> {
    spec.to_command()?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| Error::Command {
            program: program.to_owned(),
            status: format!("could not start: {err}"),
            log_hint: String::new(),
        })
}

/// "exited with status N" or "was killed by signal N".
fn describe_exit(status: ExitStatus) -> String {
    match (status.code(), status.signal()) {
        (Some(code), _) => format!("exited with status {code}"),
        (None, Some(signal)) => format!("was killed by signal {signal}"),
        (None, None) => format!("ended abnormally ({status})"),
    }
}

/// Start Steam (`-silent`) if it is not running and wait until
/// [`is_running`] reports it, polling every 2 s up to `timeout`. Calls
/// [`prepare_client_dxvk`] first.
///
/// The DXVK used is the bottle's `graphics.dxvk` pin, else the newest
/// installed; without one the client still starts (with a warning: its
/// windows will be black). A running client is left alone, DLLs included;
/// a stale pid does not count as one ([`is_running`]). When no wineserver
/// ran before, the client starts a new Wine session, and `display` (the
/// main display now, `None` when unknown) is recorded as the session's
/// ([`Bottle::record_session_display`]).
///
/// # Errors
/// [`crate::Error::Command`] on timeout, naming the client log.
pub fn ensure_running(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    display: Option<&Display>,
    timeout: Duration,
) -> crate::Result<()> {
    start_if_needed(layout, bottle, wine, display, timeout, POLL_INTERVAL).map(|_started| ())
}

/// [`ensure_running`] with a configurable polling interval; `true` if it
/// started the client.
fn start_if_needed(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    display: Option<&Display>,
    timeout: Duration,
    poll: Duration,
) -> crate::Result<bool> {
    let status = client_status(bottle, wine)?;
    if status.running {
        return Ok(false);
    }
    let steam = find_steam(bottle)?;
    prepare_client_dxvk_logged(layout, bottle, &steam)?;
    let (args, _) = client::silent_client_args(&[]);
    let spec = client_spec(bottle, wine, &steam, args);
    tracing::info!("starting Steam in bottle {}", bottle.config.name);
    if !status.server {
        bottle.record_session_display(display.map(Display::signature).as_deref());
    }
    reap_in_background(crate::process::spawn(&spec)?);
    wait_until_running(bottle, wine, timeout, poll, &spec)?;
    Ok(true)
}

/// [`ensure_client_dxvk`] with the bottle's DXVK: its `graphics.dxvk` pin,
/// else the newest installed. Every way of starting the client uses this, so
/// they all give it the same DLLs. Returns a warning to show when that DXVK
/// is not installed (the client still starts, but its windows stay black),
/// `None` when the DLLs are in place.
///
/// # Errors
/// As [`ensure_client_dxvk`], and component lookup errors other than "not
/// installed".
pub fn prepare_client_dxvk(
    layout: &Layout,
    bottle: &Bottle,
    steam: &SteamInstall,
) -> crate::Result<Option<String>> {
    let pin = bottle.config.graphics.dxvk.as_deref();
    let dxvk = match crate::component::find_installed(layout, ComponentKind::Dxvk, pin) {
        Ok(component) => Some(component),
        Err(Error::NotFound { .. }) => None,
        Err(err) => return Err(err),
    };
    if ensure_client_dxvk(steam, dxvk.as_ref())? {
        return Ok(None);
    }
    let effect = "so Steam's windows will stay black (its web UI needs Direct3D 11)";
    Ok(Some(match pin {
        Some(pin) => format!(
            "DXVK {pin} (pinned by graphics.dxvk in bottle {name}) is not installed, {effect}; install it with `uncork runtime install dxvk --version {pin}` or unpin it with `uncork bottle set {name} graphics.dxvk=`",
            name = bottle.config.name
        ),
        None => format!(
            "DXVK is not installed, {effect}; install it with `uncork runtime install dxvk`"
        ),
    }))
}

/// [`prepare_client_dxvk`], logging its warning.
fn prepare_client_dxvk_logged(
    layout: &Layout,
    bottle: &Bottle,
    steam: &SteamInstall,
) -> crate::Result<()> {
    if let Some(warning) = prepare_client_dxvk(layout, bottle, steam)? {
        tracing::warn!("{warning}");
    }
    Ok(())
}

/// Poll until the client runs; a timeout is an error naming `spec`'s log.
fn wait_until_running(
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
    poll: Duration,
    spec: &CommandSpec,
) -> crate::Result<()> {
    if wait_for(bottle, wine, timeout, poll, true)? {
        return Ok(());
    }
    Err(Error::Command {
        program: "steam.exe".to_owned(),
        status: format!("did not report running within {timeout:?}"),
        log_hint: spec
            .log
            .as_ref()
            .map_or_else(String::new, |log| format!(" (log: {})", log.display())),
    })
}

/// Poll [`is_running`] every `poll` (shortened to the time left) until it
/// equals `running`; `false` once `timeout` has passed. Checks at least once.
fn wait_for(
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
    poll: Duration,
    running: bool,
) -> crate::Result<bool> {
    let deadline = Instant::now().checked_add(timeout);
    loop {
        let left = deadline.map_or(poll, |deadline| {
            deadline.saturating_duration_since(Instant::now())
        });
        std::thread::sleep(poll.min(left));
        if is_running(bottle, wine)? == running {
            return Ok(true);
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Ok(false);
        }
    }
}

/// Wait for `child` on a background thread so it does not linger as a
/// zombie once it exits (Steam's bootstrapper restarts itself; nobody else
/// waits for it).
fn reap_in_background(mut child: Child) {
    let pid = child.id();
    let spawned = std::thread::Builder::new()
        .name(format!("uncork-reap-{pid}"))
        .spawn(move || {
            if let Err(err) = child.wait() {
                tracing::debug!("cannot wait for process {pid}: {err}");
            }
        });
    if let Err(err) = spawned {
        tracing::debug!("cannot start a thread to wait for process {pid}: {err}");
    }
}

/// Plan playing Steam app `appid`: find it in the bottle's libraries, pick
/// the executable (profile `exe.path`, else the largest `.exe` in the
/// install directory that is not a known launcher/crash reporter/redist),
/// and plan a launch of it ([`crate::launch::plan`]).
///
/// The profile's `exe.path` is matched component by component ignoring
/// ASCII case. Without a profile the search looks at the install directory
/// first, then one level deeper at a time (up to three), and takes the
/// largest `.exe` at the first level that has one, skipping names that
/// start with one of [`uncork_steam::client::NON_GAME_EXE_PREFIXES`]. A
/// game Steam does not list as fully installed gets a warning.
///
/// In [`LaunchMode::Direct`] mode (the profile's launch mode, and Direct
/// without a profile) the game's environment also gets `SteamAppId` and
/// `SteamGameId` = `appid`, as Steam gives the games it starts; a value the
/// bottle, the profile or `--env` sets wins.
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
    let app = find_game(ctx.bottle, appid)?;
    plan_found_game(ctx, &app, args, options)
}

/// The launch mode of a game with `profile` (Direct without one).
fn launch_mode(profile: Option<&GameProfile>) -> LaunchMode {
    profile.map_or(LaunchMode::Direct, GameProfile::launch_mode)
}

/// [`plan_game`] for an app already found.
fn plan_found_game(
    ctx: PlanContext<'_>,
    app: &InstalledApp,
    args: &[String],
    options: &LaunchOptions,
) -> crate::Result<LaunchPlan> {
    let exe = game_exe(app, ctx.profile)?;
    let target = Target::Exe {
        path: exe,
        args: args.to_vec(),
    };
    let mut plan = crate::launch::plan(ctx, &target, options)?;
    if launch_mode(ctx.profile) == LaunchMode::Direct {
        for var in STEAM_APP_VARS {
            plan.command
                .env
                .entry(var.to_owned())
                .or_insert_with(|| app.manifest.appid.to_string());
        }
    }
    if !app.manifest.is_fully_installed() {
        plan.warnings.insert(
            0,
            format!(
                "Steam does not list {} as fully installed (StateFlags {}); let Steam finish installing or updating it first",
                app.manifest.name, app.manifest.state_flags
            ),
        );
    }
    Ok(plan)
}

/// Steam in `bottle`, or a `NotFound` saying how to install it.
fn find_steam(bottle: &Bottle) -> crate::Result<SteamInstall> {
    SteamInstall::find(bottle.prefix()).ok_or_else(|| Error::NotFound {
        what: "Steam in bottle",
        name: bottle.config.name.clone(),
        hint: format!(
            "; run: uncork steam install --bottle {}",
            bottle.config.name
        ),
    })
}

/// The installed app `appid` in `bottle`'s Steam libraries.
fn find_game(bottle: &Bottle, appid: u32) -> crate::Result<InstalledApp> {
    let steam = find_steam(bottle)?;
    steam.find_app(appid)?.ok_or_else(|| Error::NotFound {
        what: "Steam app",
        name: appid.to_string(),
        hint: format!(
            "; install it from the Steam library first (bottle {})",
            bottle.config.name
        ),
    })
}

/// The executable to start for `app` (see [`plan_game`]).
fn game_exe(app: &InstalledApp, profile: Option<&GameProfile>) -> crate::Result<PathBuf> {
    if let Some(profile) = profile {
        let parts: Vec<&str> = profile
            .exe
            .path
            .split(['/', '\\'])
            .filter(|part| !part.is_empty() && *part != ".")
            .collect();
        let path = crate::launch::join_case_insensitive(&app.install_path, &parts);
        if !parts.contains(&"..") && path.is_file() {
            return Ok(path);
        }
        return Err(Error::NotFound {
            what: "game executable",
            name: profile.exe.path.clone(),
            hint: format!(
                "; profile {} names it, but it is not in {} (verify the game's files in Steam)",
                profile.id,
                app.install_path.display()
            ),
        });
    }
    find_game_exe(&app.install_path).ok_or_else(|| Error::NotFound {
        what: "game executable",
        name: app.manifest.name.clone(),
        hint: format!(
            "; no game .exe found in {}; a profile with exe.path can name it",
            app.install_path.display()
        ),
    })
}

/// The largest plausible game executable at the shallowest level of
/// `install_dir` that has one (ties broken by path).
fn find_game_exe(install_dir: &Path) -> Option<PathBuf> {
    let mut level = vec![install_dir.to_path_buf()];
    for _depth in 0..=EXE_SEARCH_DEPTH {
        let mut best: Option<(u64, PathBuf)> = None;
        let mut subdirs = Vec::new();
        for dir in &level {
            let Ok(entries) = fs::read_dir(dir) else {
                continue;
            };
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                let Ok(metadata) = fs::metadata(&path) else {
                    continue;
                };
                if metadata.is_dir() {
                    subdirs.push(path);
                } else if metadata.is_file() && is_game_exe_name(&entry.file_name()) {
                    let better = best.as_ref().is_none_or(|(size, known)| {
                        metadata.len() > *size || (metadata.len() == *size && path < *known)
                    });
                    if better {
                        best = Some((metadata.len(), path));
                    }
                }
            }
        }
        if let Some((_, exe)) = best {
            return Some(exe);
        }
        subdirs.sort();
        level = subdirs;
    }
    None
}

/// `*.exe` (any case) not starting with a known non-game prefix.
fn is_game_exe_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".exe")
        && !client::NON_GAME_EXE_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
}

/// Steam's own log directory in the bottle, for error messages.
#[must_use]
pub fn logs_dir(bottle: &Bottle) -> std::path::PathBuf {
    bottle
        .drive_c()
        .join(Path::new(uncork_steam::client::STEAM_DIR))
        .join("logs")
}

/// What [`play`] did.
#[derive(Debug)]
pub struct PlayOutcome {
    /// The game's launch plan.
    pub plan: LaunchPlan,
    /// How the game was (or, for a dry run, would be) started.
    pub mode: LaunchMode,
    /// The started game process; `None` for a dry run. In
    /// [`LaunchMode::Applaunch`] mode this is the Steam client Uncork
    /// started, which starts the game.
    pub child: Option<std::process::Child>,
    /// Where `child`'s output goes: the plan's log, except in
    /// [`LaunchMode::Applaunch`] mode, where it is the Steam client's log
    /// (`logs/<bottle>-steam-<unix secs>.log`).
    pub log: PathBuf,
    /// INI files the profile changed before launch.
    pub ini_changed: Vec<std::path::PathBuf>,
    /// `true` if Steam had to be started first.
    pub started_steam: bool,
    /// The main display changed since the bottle's running Wine session
    /// started (a dry run only reports it).
    pub display_change: Option<DisplayChange>,
    /// The bottle was stopped because of `display_change` (the
    /// [`ConfirmRestart`] agreed), so the game started in a new session.
    pub restarted: bool,
}

/// Asked by [`play`] once the game is found and planned, before it stops a
/// running bottle whose main display changed ([`DisplayChange`]): `true`
/// stops the bottle (Steam and anything else running in it), `false`
/// starts the game in the running session, which still sees the old
/// display. The CLI says what is about to happen here and asks on a
/// terminal.
pub type ConfirmRestart<'a> = dyn FnMut(&DisplayChange) -> bool + 'a;

/// Play Steam app `appid` in `bottle`: the whole `uncork play` flow, shared
/// by the CLI and (later) the macOS app.
///
/// 1. Find the app ([`plan_game`] with `profile`, `display`, `args`,
///    `options`). Nothing is stopped or changed before this succeeds.
/// 2. Check whether the main display changed since the bottle's running
///    Wine session started ([`display_change`]). `dry_run`: add a warning
///    saying so to the plan and return it without touching anything.
/// 3. When it changed and `confirm_restart` agrees, stop the bottle
///    ([`stop_bottle`], Steam getting [`SHUTDOWN_GRACE`] at most
///    `steam_timeout`), so Steam and the game start again in a session
///    that knows the new display.
/// 4. Apply the profile's INI edits ([`crate::launch::apply_profile_ini`],
///    `{display.*}` values from `display`).
/// 5. By launch mode: `Direct` → [`ensure_running`] (timeout
///    `steam_timeout`) then [`crate::launch::execute`] the plan;
///    `Applaunch` → [`stop`] a running client, then start the client with the
///    game's environment and `-applaunch <appid> <args>` (the game inherits
///    it); `Standalone` → execute the plan without Steam. Whatever starts a
///    new Wine session records `display` as its main display.
///
/// The launch mode is the profile's ([`GameProfile::launch_mode`]), and
/// [`LaunchMode::Direct`] without a profile. `%INSTALLDIR%` in INI paths is
/// the app's install directory. In `Applaunch` mode a running client gets
/// [`SHUTDOWN_GRACE`] (at most `steam_timeout`) to exit before the
/// bottle is stopped with `wineserver --kill`, `steam_timeout` bounds
/// starting the new one, the plan's backend files are put in place first,
/// the game arguments are the profile's `launch.args` then `args`, and the
/// client's environment is the game's (see the module docs for the
/// trade-off).
///
/// # Errors
/// Lookup, planning, Steam start or spawn errors.
#[allow(clippy::too_many_arguments)]
pub fn play(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    components: &[crate::component::InstalledComponent],
    profile: Option<&crate::profile::GameProfile>,
    display: Option<&Display>,
    confirm_restart: &mut ConfirmRestart<'_>,
    appid: u32,
    args: &[String],
    options: &LaunchOptions,
    steam_timeout: Duration,
    dry_run: bool,
) -> crate::Result<PlayOutcome> {
    let request = PlayRequest {
        components,
        profile,
        display,
        appid,
        args,
        options,
        steam_timeout,
        shutdown_grace: SHUTDOWN_GRACE,
        poll: POLL_INTERVAL,
    };
    play_with(layout, bottle, wine, &request, confirm_restart, dry_run)
}

/// The inputs of [`play`] that pass through unchanged.
struct PlayRequest<'a> {
    components: &'a [InstalledComponent],
    profile: Option<&'a GameProfile>,
    display: Option<&'a Display>,
    appid: u32,
    args: &'a [String],
    options: &'a LaunchOptions,
    steam_timeout: Duration,
    /// [`SHUTDOWN_GRACE`].
    shutdown_grace: Duration,
    poll: Duration,
}

/// [`play`] with a configurable polling interval.
fn play_with(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    request: &PlayRequest<'_>,
    confirm_restart: &mut ConfirmRestart<'_>,
    dry_run: bool,
) -> crate::Result<PlayOutcome> {
    let app = find_game(bottle, request.appid)?;
    let ctx = PlanContext {
        layout,
        bottle,
        wine,
        components: request.components,
        profile: request.profile,
        display: request.display,
    };
    let mut plan = plan_found_game(ctx, &app, request.args, request.options)?;
    let mode = launch_mode(request.profile);
    let display_change = display_change(bottle, wine, request.display)?;
    if dry_run {
        if let Some(change) = &display_change {
            plan.warnings.push(format!(
                "the main display changed since bottle {} started ({} → {}); a real launch stops the bottle first, so the game sees the new display",
                bottle.config.name, change.before, change.now
            ));
        }
        return Ok(PlayOutcome {
            log: plan.log.clone(),
            plan,
            mode,
            child: None,
            ini_changed: Vec::new(),
            started_steam: false,
            display_change,
            restarted: false,
        });
    }
    let restarted = display_change.as_ref().is_some_and(|change| {
        let restart = confirm_restart(change);
        tracing::info!(
            "the main display changed since bottle {} started ({} → {}); {}",
            bottle.config.name,
            change.before,
            change.now,
            if restart {
                "restarting it"
            } else {
                "not restarting it, as asked"
            }
        );
        restart
    });
    if restarted {
        let grace = request.shutdown_grace.min(request.steam_timeout);
        stop_bottle_with(bottle, wine, grace, request.poll)?;
    }
    let ini_changed = match request.profile {
        Some(profile) => crate::launch::apply_profile_ini(
            profile,
            bottle,
            Some(&app.install_path),
            request.display,
        )?,
        None => Vec::new(),
    };
    let (child, log, started_steam) = match mode {
        LaunchMode::Direct => {
            let started = start_if_needed(
                layout,
                bottle,
                wine,
                request.display,
                request.steam_timeout,
                request.poll,
            )?;
            let child = crate::launch::execute(&plan, bottle, wine)?;
            (child, plan.log.clone(), started)
        }
        LaunchMode::Applaunch => {
            let (child, log) = applaunch(layout, bottle, wine, &plan, request)?;
            (child, log, true)
        }
        LaunchMode::Standalone => {
            let child = crate::launch::execute(&plan, bottle, wine)?;
            (child, plan.log.clone(), false)
        }
    };
    Ok(PlayOutcome {
        plan,
        mode,
        child: Some(child),
        log,
        ini_changed,
        started_steam,
        display_change,
        restarted,
    })
}

/// The `Applaunch` branch of [`play`]; returns the client it started and
/// its log.
fn applaunch(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    plan: &LaunchPlan,
    request: &PlayRequest<'_>,
) -> crate::Result<(Child, PathBuf)> {
    if is_running(bottle, wine)? {
        tracing::info!(
            "restarting Steam in bottle {} with the game's environment",
            bottle.config.name
        );
        let grace = request.shutdown_grace.min(request.steam_timeout);
        stop_with(bottle, wine, grace, request.poll)?;
    }
    crate::launch::prepare(plan, bottle)?;
    let steam = find_steam(bottle)?;
    prepare_client_dxvk_logged(layout, bottle, &steam)?;
    if !server_running(bottle, wine)? {
        bottle.record_session_display(request.display.map(Display::signature).as_deref());
    }
    let mut game_args: Vec<String> = request
        .profile
        .map(|profile| profile.launch.args.clone())
        .unwrap_or_default();
    game_args.extend(request.args.iter().cloned());
    let spec = applaunch_spec(bottle, wine, &steam, plan, request.appid, &game_args);
    let child = crate::process::spawn(&spec)?;
    if let Err(err) = wait_until_running(bottle, wine, request.steam_timeout, request.poll, &spec) {
        reap_in_background(child);
        return Err(err);
    }
    let log = spec.log.unwrap_or_else(|| plan.log.clone());
    Ok((child, log))
}

/// `steam.exe -silent -nofriendsui -applaunch <appid> <game args>` with the
/// game's environment, `d3d10core,d3d11=n,b` forced for the client's
/// app-local DXVK, and `DXVK_LOG_LEVEL=none` unless the game sets it.
fn applaunch_spec(
    bottle: &Bottle,
    wine: &WineRuntime,
    steam: &SteamInstall,
    plan: &LaunchPlan,
    appid: u32,
    game_args: &[String],
) -> CommandSpec {
    let mut env = plan.command.env.clone();
    let mut overrides =
        crate::launch::parse_overrides(env.get(OVERRIDES_VAR).map_or("", String::as_str));
    for dll in CLIENT_NATIVE {
        overrides.insert(dll.to_owned(), "n,b".to_owned());
    }
    env.insert(
        OVERRIDES_VAR.to_owned(),
        crate::graphics::render_overrides(&overrides),
    );
    env.entry("DXVK_LOG_LEVEL".to_owned())
        .or_insert_with(|| "none".to_owned());
    // Client flags are filtered; the game's arguments are passed verbatim.
    let (mut args, _) = client::silent_client_args(&[]);
    args.extend(client::applaunch_args(appid, game_args));
    steam_spec(bottle, wine, steam, args, env)
}

#[cfg(test)]
#[path = "../../uncork-pe/tests/common/mod.rs"]
mod pe_builder;

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;
    use crate::bottle::{BottleConfig, BottleState};
    use crate::component::{ComponentMeta, Source};
    use crate::graphics::Backend;

    const FAKE_WINE: &str = include_str!("../tests/d_support/fake_wine.sh");
    const FAKE_WINESERVER: &str = include_str!("../tests/d_support/fake_wineserver.sh");

    struct Fixture {
        _dir: tempfile::TempDir,
        layout: Layout,
        state: PathBuf,
        wine: WineRuntime,
        bottle: Bottle,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::tempdir().unwrap();
            let layout = Layout::at(dir.path().join("home"));
            let state = dir.path().join("state");
            fs::create_dir_all(&state).unwrap();
            let root = dir.path().join("wine");
            for (name, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
                let path = root.join("bin").join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, script.replace("@STATE@", state.to_str().unwrap())).unwrap();
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let bridge = root.join("lib/wine/x86_64-unix/winemetal.so");
            fs::create_dir_all(bridge.parent().unwrap()).unwrap();
            fs::write(&bridge, "bridge").unwrap();
            let wine = WineRuntime {
                root,
                version: "11.17-fake".to_owned(),
                features: ["msync", "wow64", "dxmt"].map(str::to_owned).to_vec(),
            };
            let path = layout.bottle_dir("steam");
            fs::create_dir_all(path.join("drive_c")).unwrap();
            let config: BottleConfig =
                toml::from_str("schema = 1\nname = \"steam\"\nwine = \"11.17-fake\"\n").unwrap();
            let bottle = Bottle {
                path,
                config,
                state: BottleState::default(),
            };
            Fixture {
                _dir: dir,
                layout,
                state,
                wine,
                bottle,
            }
        }

        fn calls(&self) -> Vec<String> {
            fs::read_to_string(self.state.join("calls.log"))
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }

        fn flag(&self, name: &str) {
            fs::write(self.state.join(name), "").unwrap();
        }

        fn pid(&self) -> Option<String> {
            fs::read_to_string(self.state.join("pid")).ok()
        }

        fn install_steam(&self) {
            let steam = self.bottle.drive_c().join(client::STEAM_DIR);
            fs::create_dir_all(&steam).unwrap();
            fs::write(steam.join("steam.exe"), "steam").unwrap();
        }

        /// Steam with app 287450 installed: a 32-bit executable whose
        /// renderer DLL imports `d3d11.dll`.
        fn add_game(&self) -> PathBuf {
            self.install_steam();
            let steamapps = self
                .bottle
                .drive_c()
                .join(client::STEAM_DIR)
                .join("steamapps");
            let dir = steamapps.join("common").join("Rise of Nations");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                steamapps.join("appmanifest_287450.acf"),
                "\"AppState\"\n{\n\"appid\" \"287450\"\n\"name\" \"RoN\"\n\"installdir\" \"Rise of Nations\"\n\"StateFlags\" \"4\"\n}\n",
            )
            .unwrap();
            fs::write(
                dir.join("riseofnations.exe"),
                pe_builder::PeBuilder::pe32().build(),
            )
            .unwrap();
            let renderer = pe_builder::PeBuilder::pe32().dll().import("d3d11.dll");
            fs::write(dir.join("d3dgl.dll"), renderer.build()).unwrap();
            dir
        }

        /// DXMT 0.80 under `components/`; returns every installed component.
        fn dxmt(&self) -> Vec<InstalledComponent> {
            let dir = self.layout.component_dir(ComponentKind::Dxmt, "0.80");
            for arch in ["x86_64-windows", "i386-windows"] {
                fs::create_dir_all(dir.join(arch)).unwrap();
                for dll in ["d3d11", "dxgi", "d3d10core", "winemetal"] {
                    let mut bytes = b"MZ".to_vec();
                    bytes.resize(uncork_pe::WINE_MARKER_OFFSET, 0);
                    bytes.extend_from_slice(uncork_pe::WINE_BUILTIN_SIGNATURE);
                    fs::write(dir.join(arch).join(format!("{dll}.dll")), bytes).unwrap();
                }
            }
            let meta = ComponentMeta {
                schema: 1,
                kind: ComponentKind::Dxmt,
                version: "0.80".to_owned(),
                source: Source::Local {
                    path: PathBuf::from("/fake"),
                },
                license: "test".to_owned(),
                source_code: None,
                features: Vec::new(),
                installed_unix: 0,
            };
            fs::write(
                dir.join(crate::component::META_FILE),
                toml::to_string(&meta).unwrap(),
            )
            .unwrap();
            crate::component::list_installed(&self.layout).unwrap()
        }

        fn position(&self, needle: &str) -> usize {
            let calls = self.calls();
            calls
                .iter()
                .position(|call| call.contains(needle))
                .unwrap_or_else(|| panic!("no call contains {needle:?}: {calls:#?}"))
        }
    }

    fn quick<'a>(
        components: &'a [InstalledComponent],
        profile: Option<&'a GameProfile>,
        args: &'a [String],
        options: &'a LaunchOptions,
    ) -> PlayRequest<'a> {
        PlayRequest {
            components,
            profile,
            display: None,
            appid: 287_450,
            args,
            options,
            steam_timeout: Duration::from_secs(5),
            shutdown_grace: Duration::from_secs(5),
            poll: Duration::from_millis(10),
        }
    }

    fn applaunch_profile(backend: &str) -> GameProfile {
        GameProfile::parse(
            &format!(
                "schema = 1\nid = \"ron\"\nname = \"RoN\"\n[steam]\nappid = 287450\n[exe]\npath = \"riseofnations.exe\"\n[graphics]\nbackend = \"{backend}\"\n[launch]\nmode = \"applaunch\"\nargs = [\"-nointro\"]\n"
            ),
            Path::new("ron.toml"),
        )
        .unwrap()
    }

    #[test]
    fn direct_play_starts_steam_before_the_game() {
        let mut fx = Fixture::new();
        let dir = fx.add_game();
        let components = fx.dxmt();
        let options = LaunchOptions::default();
        let request = quick(&components, None, &[], &options);
        let mut outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        assert!(outcome.started_steam);
        assert_eq!(outcome.mode, LaunchMode::Direct);
        assert_eq!(outcome.log, outcome.plan.log, "the game's own log");
        assert!(outcome.child.take().unwrap().wait().unwrap().success());
        let start = fx.position("steam.exe -silent -nofriendsui |");
        let game = fx.position(&format!("{} |", dir.join("riseofnations.exe").display()));
        assert!(start < game);
        assert!(fx.bottle.syswow64().join("d3d11.dll").is_file());
        let env = &outcome.plan.command.env;
        assert_eq!(env.get("SteamAppId").map(String::as_str), Some("287450"));
        assert_eq!(env.get("SteamGameId").map(String::as_str), Some("287450"));
    }

    fn display(name: &str, width: u32, height: u32, hz: f64) -> Display {
        Display {
            name: name.to_owned(),
            points: (width, height),
            pixels: None,
            refresh_hz: Some(hz),
            main: true,
            built_in: false,
        }
    }

    #[test]
    fn steam_records_the_display_its_session_starts_with() {
        let mut fx = Fixture::new();
        fx.add_game();
        let components = fx.dxmt();
        let options = LaunchOptions::default();
        let built_in = display("Color LCD", 1728, 1117, 120.0);
        let request = PlayRequest {
            display: Some(&built_in),
            ..quick(&components, None, &[], &options)
        };
        let mut outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        outcome.child.take().unwrap().wait().unwrap();
        assert!(outcome.started_steam);
        assert_eq!(outcome.display_change, None);
        assert_eq!(
            fx.bottle.state.session_display.as_deref(),
            Some("Color LCD 1728x1117")
        );
        let saved = fs::read_to_string(fx.bottle.path.join(crate::bottle::STATE_FILE)).unwrap();
        assert!(
            saved.contains("session_display = \"Color LCD 1728x1117\""),
            "{saved}"
        );
    }

    #[test]
    fn a_changed_main_display_restarts_steam_before_the_game() {
        let mut fx = Fixture::new();
        let dir = fx.add_game();
        let components = fx.dxmt();
        let options = LaunchOptions::default();
        // Steam runs in a session that started on the built-in display.
        fx.bottle
            .record_session_display(Some("Color LCD 1728x1117 @120Hz"));
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        fx.flag("shutdown-works");
        let external = display("LG UltraFine", 2560, 1440, 60.0);
        let request = PlayRequest {
            display: Some(&external),
            ..quick(&components, None, &[], &options)
        };

        let mut outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        outcome.child.take().unwrap().wait().unwrap();

        let change = outcome.display_change.unwrap();
        assert_eq!(change.before, "Color LCD 1728x1117");
        assert_eq!(change.now, "LG UltraFine 2560x1440");
        assert!(outcome.restarted);
        assert!(outcome.started_steam, "Steam started again");
        let shutdown = fx.position("steam.exe -shutdown");
        let kill = fx.position("wineserver --kill");
        let start = fx.position("steam.exe -silent -nofriendsui |");
        let game = fx.position(&format!("{} |", dir.join("riseofnations.exe").display()));
        assert!(
            shutdown < kill && kill < start && start < game,
            "{:#?}",
            fx.calls()
        );
        assert_eq!(
            fx.bottle.state.session_display.as_deref(),
            Some("LG UltraFine 2560x1440")
        );
    }

    #[test]
    fn a_declined_restart_starts_the_game_in_the_running_session() {
        let mut fx = Fixture::new();
        let dir = fx.add_game();
        let components = fx.dxmt();
        let options = LaunchOptions::default();
        fx.bottle
            .record_session_display(Some("Color LCD 1728x1117 @120Hz"));
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        let external = display("LG UltraFine", 2560, 1440, 60.0);
        let request = PlayRequest {
            display: Some(&external),
            ..quick(&components, None, &[], &options)
        };

        let mut asked = Vec::new();
        let mut outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |change| {
                asked.push(change.clone());
                false
            },
            false,
        )
        .unwrap();
        outcome.child.take().unwrap().wait().unwrap();

        assert_eq!(asked.len(), 1);
        assert_eq!(outcome.display_change.as_ref(), asked.first());
        assert!(!outcome.restarted);
        assert!(!outcome.started_steam, "Steam kept running");
        let calls = fx.calls();
        assert!(
            !calls
                .iter()
                .any(|call| call.contains("-shutdown") || call.contains("--kill")),
            "{calls:#?}"
        );
        fx.position(&format!("{} |", dir.join("riseofnations.exe").display()));
        assert_eq!(
            fx.bottle.state.session_display.as_deref(),
            Some("Color LCD 1728x1117 @120Hz"),
            "still the running session's"
        );
    }

    #[test]
    fn a_game_that_cannot_be_found_never_stops_the_bottle() {
        let mut fx = Fixture::new();
        fx.install_steam();
        let components = fx.dxmt();
        let options = LaunchOptions::default();
        fx.bottle
            .record_session_display(Some("Color LCD 1728x1117 @120Hz"));
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        let external = display("LG UltraFine", 2560, 1440, 60.0);
        let request = PlayRequest {
            display: Some(&external),
            appid: 999_999,
            ..quick(&components, None, &[], &options)
        };

        let err = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| panic!("nothing to restart for"),
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("999999"), "{err}");
        assert!(fx.calls().is_empty(), "{:#?}", fx.calls());
    }

    #[test]
    fn stopping_a_bottle_asks_steam_to_exit_first() {
        let fx = Fixture::new();
        fx.install_steam();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        fx.flag("shutdown-works");
        let stopped = stop_bottle_with(
            &fx.bottle,
            &fx.wine,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .unwrap();
        assert!(stopped);
        let shutdown = fx.position("steam.exe -shutdown");
        let kill = fx.position("wineserver --kill");
        let forget = fx.position(r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess");
        assert!(shutdown < kill && kill < forget, "{:#?}", fx.calls());
        assert!(
            fx.calls().last().unwrap().starts_with("wineserver --wait"),
            "{:#?}",
            fx.calls()
        );
        assert!(!fx.state.join("server").exists());
        assert_eq!(fx.pid().as_deref(), Some("0x0"));
    }

    #[test]
    fn a_profile_or_env_value_wins_over_the_steam_app_id() {
        let mut fx = Fixture::new();
        fx.add_game();
        let components = fx.dxmt();
        let options = LaunchOptions {
            env: BTreeMap::from([("SteamGameId".to_owned(), "1".to_owned())]),
            ..LaunchOptions::default()
        };
        let request = quick(&components, None, &[], &options);
        let outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            true,
        )
        .unwrap();
        let env = &outcome.plan.command.env;
        assert_eq!(env.get("SteamAppId").map(String::as_str), Some("287450"));
        assert_eq!(env.get("SteamGameId").map(String::as_str), Some("1"));
    }

    #[test]
    fn applaunch_does_not_wait_long_for_a_client_that_ignores_shutdown() {
        let mut fx = Fixture::new();
        fx.add_game();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        let profile = applaunch_profile("wined3d");
        let options = LaunchOptions::default();
        let mut request = quick(&[], Some(&profile), &[], &options);
        request.steam_timeout = Duration::from_secs(60);
        request.shutdown_grace = Duration::from_millis(50);

        let started = std::time::Instant::now();
        let outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        assert!(outcome.child.unwrap().wait().unwrap().success());
        let shutdown = fx.position("steam.exe -shutdown |");
        let kill = fx.position("wineserver --kill");
        let forget = fx.position(r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess /v pid");
        let applaunch = fx.position("-applaunch 287450 -nointro |");
        assert!(
            shutdown < kill && kill < forget && forget < applaunch,
            "{:#?}",
            fx.calls()
        );
    }

    #[test]
    fn applaunch_restarts_steam_with_the_games_environment() {
        let mut fx = Fixture::new();
        fx.add_game();
        let components = fx.dxmt();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        fx.flag("shutdown-works");
        let profile = applaunch_profile("dxmt");
        let args = ["-x".to_owned()];
        let options = LaunchOptions::default();
        let request = quick(&components, Some(&profile), &args, &options);

        let outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        assert!(outcome.started_steam);
        assert_eq!(outcome.mode, LaunchMode::Applaunch);
        assert_eq!(outcome.plan.activation.backend, Backend::Dxmt);
        let log = outcome.log.file_name().unwrap().to_str().unwrap();
        assert!(log.starts_with("steam-steam-"), "the client's log: {log}");
        assert!(
            !outcome.plan.command.env.contains_key("SteamAppId"),
            "Steam sets it for the game it starts"
        );
        assert!(outcome.child.unwrap().wait().unwrap().success());

        let shutdown = fx.position("steam.exe -shutdown |");
        let applaunch =
            fx.position("steam.exe -silent -nofriendsui -applaunch 287450 -nointro -x |");
        assert!(shutdown < applaunch);
        let line = &fx.calls()[applaunch];
        assert!(
            line.contains(" WINEDLLOVERRIDES=d3d10,d3d12,d3d12core=b;winemenubuilder.exe=d;d3d10core,d3d11,dxgi=n,b "),
            "{line}"
        );
        assert!(
            line.contains(" DXMT_LOG_LEVEL=none "),
            "the game's env: {line}"
        );
        assert!(
            line.contains(" DXVK_LOG_LEVEL=none "),
            "the client's DXVK: {line}"
        );
        assert!(
            !fx.calls()
                .iter()
                .any(|call| call.contains("riseofnations.exe")),
            "Steam starts the game, not Uncork"
        );
        assert!(
            fx.bottle.syswow64().join("d3d11.dll").is_file(),
            "the game's backend is in place before Steam starts it"
        );
    }

    #[test]
    fn applaunch_keeps_the_clients_dxvk_for_a_wined3d_game() {
        let mut fx = Fixture::new();
        fx.add_game();
        let profile = applaunch_profile("wined3d");
        let options = LaunchOptions::default();
        let request = quick(&[], Some(&profile), &[], &options);
        let outcome = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap();
        assert!(outcome.child.unwrap().wait().unwrap().success());
        assert!(
            !fx.calls().iter().any(|call| call.contains("-shutdown")),
            "nothing to stop"
        );
        let line = &fx.calls()[fx.position("-applaunch 287450 -nointro |")];
        assert!(
            line.contains(" WINEDLLOVERRIDES=d3d10,d3d12,d3d12core,dxgi=b;winemenubuilder.exe=d;d3d10core,d3d11=n,b "),
            "{line}"
        );
    }

    #[test]
    fn applaunch_fails_when_steam_does_not_come_up() {
        let mut fx = Fixture::new();
        fx.add_game();
        fx.flag("steam-hangs");
        let profile = applaunch_profile("wined3d");
        let options = LaunchOptions::default();
        let mut request = quick(&[], Some(&profile), &[], &options);
        request.steam_timeout = Duration::from_millis(30);
        let err = play_with(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            &request,
            &mut |_| true,
            false,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("steam.exe did not report running within 30ms"),
            "{err}"
        );
    }

    fn installer_bytes() -> Vec<u8> {
        pe_builder::PeBuilder::pe32().build()
    }

    #[test]
    fn install_runs_the_installer_and_stops_the_client_it_started() {
        let fx = Fixture::new();
        let mut fetched = None;
        install_with(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
            client::INSTALLER_SHA256,
            |dest| {
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                fs::write(dest, installer_bytes()).unwrap();
                fetched = Some(dest.to_path_buf());
                Ok("https://steam.invalid/SteamSetup.exe".to_owned())
            },
        )
        .unwrap();
        let installer = fx.layout.downloads_dir().join("SteamSetup.exe");
        assert_eq!(fetched.as_deref(), Some(installer.as_path()));
        assert!(SteamInstall::find(fx.bottle.prefix()).is_some());

        let calls = fx.calls();
        assert_eq!(calls.len(), 2, "{calls:#?}");
        assert!(
            calls[0].starts_with(&format!("wine {} /S |", installer.display())),
            "{}",
            calls[0]
        );
        assert!(calls[0].contains("WINEMSYNC=1"), "{}", calls[0]);
        assert!(
            calls[0].contains("WINEDLLOVERRIDES=winemenubuilder.exe=d "),
            "{}",
            calls[0]
        );
        assert!(calls[1].starts_with("wineserver --kill"), "{}", calls[1]);
        assert!(
            fx.layout
                .logs_dir()
                .join("steam-steam-install.log")
                .is_file()
        );
    }

    #[test]
    fn install_reuses_a_cached_installer_with_the_pinned_hash() {
        let fx = Fixture::new();
        let installer = fx.layout.downloads_dir().join("SteamSetup.exe");
        fs::create_dir_all(installer.parent().unwrap()).unwrap();
        fs::write(&installer, installer_bytes()).unwrap();
        let pinned = crate::download::sha256_file(&installer).unwrap();
        install_with(&fx.layout, &fx.bottle, &fx.wine, &pinned, |_| {
            panic!("a verified cached installer is not downloaded again")
        })
        .unwrap();
        assert!(SteamInstall::find(fx.bottle.prefix()).is_some());
        assert!(
            fx.calls()[0].starts_with(&format!("wine {} /S |", installer.display())),
            "{:#?}",
            fx.calls()
        );
    }

    #[test]
    fn install_downloads_again_when_the_cached_installer_differs() {
        let fx = Fixture::new();
        let installer = fx.layout.downloads_dir().join("SteamSetup.exe");
        fs::create_dir_all(installer.parent().unwrap()).unwrap();
        fs::write(&installer, b"stale or partial").unwrap();
        let pinned = {
            let fresh = fx.layout.downloads_dir().join("fresh.exe");
            fs::write(&fresh, installer_bytes()).unwrap();
            crate::download::sha256_file(&fresh).unwrap()
        };
        let mut fetched = false;
        install_with(&fx.layout, &fx.bottle, &fx.wine, &pinned, |dest| {
            fetched = true;
            fs::write(dest, installer_bytes()).unwrap();
            Ok("https://steam.invalid/SteamSetup.exe".to_owned())
        })
        .unwrap();
        assert!(fetched);
        assert_eq!(fs::read(&installer).unwrap(), installer_bytes());
    }

    #[test]
    fn install_skips_everything_when_steam_is_there() {
        let fx = Fixture::new();
        fx.install_steam();
        install_with(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
            client::INSTALLER_SHA256,
            |_| panic!("nothing should be downloaded"),
        )
        .unwrap();
        assert!(fx.calls().is_empty());
    }

    #[test]
    fn install_rejects_files_that_are_not_windows_installers() {
        let fx = Fixture::new();
        for bytes in [
            b"<html>blocked</html>".to_vec(),
            pe_builder::PeBuilder::pe64().build(),
            pe_builder::PeBuilder::pe32()
                .subsystem(pe_builder::IMAGE_SUBSYSTEM_WINDOWS_CUI)
                .build(),
            pe_builder::PeBuilder::pe32().dll().build(),
        ] {
            let err = install_with(
                &fx.layout,
                &fx.bottle,
                &fx.wine,
                client::INSTALLER_SHA256,
                |dest| {
                    fs::create_dir_all(dest.parent().unwrap()).unwrap();
                    fs::write(dest, &bytes).unwrap();
                    Ok("https://steam.invalid/SteamSetup.exe".to_owned())
                },
            )
            .unwrap_err();
            assert!(
                matches!(&err, Error::Download { url, message } if url.contains("steam.invalid") && message.starts_with("not a Windows installer")),
                "{err:?}"
            );
            assert!(!fx.layout.downloads_dir().join("SteamSetup.exe").exists());
        }
        assert!(fx.calls().is_empty(), "no installer was run");
    }

    #[test]
    fn install_fails_naming_the_log_when_steam_exe_does_not_appear() {
        let fx = Fixture::new();
        fx.flag("no-steam-exe");
        let err = install_with(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
            client::INSTALLER_SHA256,
            |dest| {
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                fs::write(dest, installer_bytes()).unwrap();
                Ok("https://steam.invalid/SteamSetup.exe".to_owned())
            },
        )
        .unwrap_err();
        let message = err.to_string();
        assert!(
            message.starts_with("SteamSetup.exe finished, but "),
            "{message}"
        );
        assert!(message.contains("steam-steam-install.log"), "{message}");
    }

    #[test]
    fn install_passes_download_errors_through() {
        let fx = Fixture::new();
        let err = install_with(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
            client::INSTALLER_SHA256,
            |_| {
                Err(Error::Download {
                    url: "https://steam.invalid/".to_owned(),
                    message: "offline".to_owned(),
                })
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::Download { .. }), "{err:?}");
    }

    #[test]
    fn stop_returns_once_the_client_exits() {
        let fx = Fixture::new();
        fx.install_steam();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("server");
        fx.flag("shutdown-works");
        stop_with(
            &fx.bottle,
            &fx.wine,
            Duration::from_secs(5),
            Duration::from_millis(20),
        )
        .unwrap();
        let calls = fx.calls();
        assert!(
            calls
                .iter()
                .any(|call| call.contains("steam.exe -shutdown")),
            "{calls:#?}"
        );
        assert!(
            !calls
                .iter()
                .any(|call| call.starts_with("wineserver --kill")),
            "{calls:#?}"
        );
        assert_eq!(fx.pid().as_deref(), Some("0x0"));
    }

    #[test]
    fn a_crashed_client_is_started_again_after_its_pid_is_cleared() {
        let fx = Fixture::new();
        fx.install_steam();
        // The pid of a client that crashed; its wineserver has exited.
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        let mut fx = fx;
        let started = start_if_needed(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            None,
            Duration::from_secs(5),
            Duration::from_millis(10),
        )
        .unwrap();
        assert!(started, "a stale pid does not count as a running client");
        let forget = fx.position(r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess /v pid");
        let start = fx.position("steam.exe -silent -nofriendsui |");
        assert!(forget < start, "{:#?}", fx.calls());
    }

    #[test]
    fn a_left_over_pid_in_user_reg_is_zeroed() {
        let text = b"WINE REGISTRY Version 2\r\n\
[Software\\\\Valve\\\\Steam] 1759860000\r\n\
\"pid\"=dword:00000111\r\n\
\r\n\
[Software\\\\Valve\\\\Steam\\\\ActiveProcess] 1759860000\r\n\
#time=1dc37c5c8d0e3a4\r\n\
\"ActiveUser\"=dword:00000000\r\n\
\"pid\"=dword:00000274\r\n\
\"SteamClientDll\"=\"C:\\\\Program Files (x86)\\\\Steam\\\\steamclient.dll\"\r\n";
        let edited = zero_active_pid(text).expect("changed");
        let expected = String::from_utf8_lossy(text)
            .replace("\"pid\"=dword:00000274", "\"pid\"=dword:00000000");
        assert_eq!(String::from_utf8_lossy(&edited), expected);
        assert_eq!(zero_active_pid(&edited), None, "already zero");
        assert_eq!(zero_active_pid(b"WINE REGISTRY Version 2\n"), None);
    }

    #[test]
    fn forgetting_on_disk_edits_only_user_reg() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!forget_client_on_disk(dir.path()).unwrap(), "no user.reg");
        let user_reg = dir.path().join("user.reg");
        fs::write(
            &user_reg,
            "[Software\\\\Valve\\\\Steam\\\\ActiveProcess] 1\n\"pid\"=dword:00000274\n",
        )
        .unwrap();
        assert!(forget_client_on_disk(dir.path()).unwrap());
        assert_eq!(
            fs::read_to_string(&user_reg).unwrap(),
            "[Software\\\\Valve\\\\Steam\\\\ActiveProcess] 1\n\"pid\"=dword:00000000\n"
        );
    }

    #[test]
    fn regs_messages_are_recognized_in_any_language() {
        for stderr in [
            "reg: Unable to find the specified registry key\n",
            "reg: Unable to find the specified registry value\r\n",
            "msync: up and running.\nreg: Der angegebene Schlüssel wurde nicht gefunden\n",
        ] {
            assert!(is_reg_message(stderr), "{stderr:?}");
        }
        for stderr in ["", "\n", "wine client error:0: version mismatch\n"] {
            assert!(!is_reg_message(stderr), "{stderr:?}");
        }
    }

    #[test]
    fn start_reports_whether_it_started_the_client() {
        let fx = Fixture::new();
        fx.install_steam();
        let mut fx = fx;
        let started = start_if_needed(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            None,
            Duration::from_secs(5),
            Duration::from_millis(20),
        )
        .unwrap();
        assert!(started);
        let again = start_if_needed(
            &fx.layout,
            &mut fx.bottle,
            &fx.wine,
            None,
            Duration::from_secs(5),
            Duration::from_millis(20),
        )
        .unwrap();
        assert!(!again);
        let starts = fx
            .calls()
            .iter()
            .filter(|call| call.contains("steam.exe -silent -nofriendsui |"))
            .count();
        assert_eq!(starts, 1);
    }

    #[test]
    fn game_exe_names_are_filtered() {
        use std::ffi::OsStr;
        for name in [
            "riseofnations.exe",
            "Game.EXE",
            "AoE2DE_s.exe",
            "patriots.exe",
        ] {
            assert!(is_game_exe_name(OsStr::new(name)), "{name}");
        }
        for name in [
            "UnityCrashHandler64.exe",
            "unins000.exe",
            "vc_redist.x64.exe",
            "DXSETUP.exe",
            "Launcher.exe",
            "readme.txt",
            "game.exe.bak",
        ] {
            assert!(!is_game_exe_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn describes_exit_statuses() {
        assert_eq!(
            describe_exit(ExitStatus::from_raw(3 << 8)),
            "exited with status 3"
        );
        assert_eq!(
            describe_exit(ExitStatus::from_raw(9)),
            "was killed by signal 9"
        );
    }
}
