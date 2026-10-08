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
//! anything else that kills a bottle's processes should do the same.

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
/// Skips the installer when `steam.exe` already exists.
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
    install_with(layout, bottle, wine, |dest| {
        download_installer(dest, progress)
    })
}

/// [`install`] with the download step supplied by the caller (tests);
/// `fetch` writes the installer to the given path and returns its URL.
fn install_with(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
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
    let url = fetch(&installer)?;
    check_installer(&installer, &url)?;

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

/// Warn on a changed hash; fail (and delete the file) unless it is a 32-bit
/// Windows GUI program.
fn check_installer(path: &Path, url: &str) -> crate::Result<()> {
    let sha256 = crate::download::sha256_file(path)?;
    if !sha256.eq_ignore_ascii_case(client::INSTALLER_SHA256) {
        tracing::warn!(
            "{INSTALLER_FILE} from {url} has SHA-256 {sha256}, not {} as last checked; Valve replaces the installer in place, so continuing",
            client::INSTALLER_SHA256
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
    let output = quiet_output(&spec)?;
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

/// Is a Steam client running in this bottle? Reads
/// `HKCU\Software\Valve\Steam\ActiveProcess\pid` with `wine reg query`
/// (non-zero means running) — authoritative because `user.reg` on disk is
/// flushed lazily.
///
/// The query runs with the bottle's environment (so a wineserver it starts
/// has the bottle's msync setting) and its output is captured, not shown.
/// `reg` exiting with status 1 means the key or value does not exist.
///
/// # Errors
/// Command errors other than "key not found" (which means not running).
pub fn is_running(bottle: &Bottle, wine: &WineRuntime) -> crate::Result<bool> {
    let spec = reg_command(
        bottle,
        wine,
        &["query", client::ACTIVE_PROCESS_KEY, "/v", "pid"],
    );
    let output = quiet_output(&spec)?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return Ok(client::parse_active_pid(&stdout).is_some_and(|pid| pid != 0));
    }
    if output.status.code() == Some(REG_NOT_FOUND) {
        tracing::debug!(
            "no Steam ActiveProcess in bottle {}: {}",
            bottle.config.name,
            String::from_utf8_lossy(&output.stderr).trim()
        );
        return Ok(false);
    }
    Err(Error::Command {
        program: "wine reg query".to_owned(),
        status: describe_exit(output.status),
        log_hint: String::new(),
    })
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
/// status lines such as `msync: up and running.` to stderr).
fn quiet_output(spec: &CommandSpec) -> crate::Result<Output> {
    spec.to_command()?
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|err| Error::Command {
            program: "wine".to_owned(),
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
/// [`ensure_client_dxvk`] first (with the newest installed DXVK).
///
/// The DXVK used is the bottle's `graphics.dxvk` pin, else the newest
/// installed; without one the client still starts (with a warning: its
/// windows will be black). A running client is left alone, DLLs included.
///
/// # Errors
/// [`crate::Error::Command`] on timeout, naming the client log.
pub fn ensure_running(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
) -> crate::Result<()> {
    start_if_needed(layout, bottle, wine, timeout, POLL_INTERVAL).map(|_started| ())
}

/// [`ensure_running`] with a configurable polling interval; `true` if it
/// started the client.
fn start_if_needed(
    layout: &Layout,
    bottle: &Bottle,
    wine: &WineRuntime,
    timeout: Duration,
    poll: Duration,
) -> crate::Result<bool> {
    if is_running(bottle, wine)? {
        return Ok(false);
    }
    let steam = find_steam(bottle)?;
    prepare_client_dxvk(layout, bottle, &steam)?;
    let (args, _) = client::silent_client_args(&[]);
    let spec = client_spec(bottle, wine, &steam, args);
    tracing::info!("starting Steam in bottle {}", bottle.config.name);
    reap_in_background(crate::process::spawn(&spec)?);
    wait_until_running(bottle, wine, timeout, poll, &spec)?;
    Ok(true)
}

/// [`ensure_client_dxvk`] with the bottle's DXVK, warning when there is none.
fn prepare_client_dxvk(
    layout: &Layout,
    bottle: &Bottle,
    steam: &SteamInstall,
) -> crate::Result<()> {
    let pin = bottle.config.graphics.dxvk.as_deref();
    let dxvk = match crate::component::find_installed(layout, ComponentKind::Dxvk, pin) {
        Ok(component) => Some(component),
        Err(Error::NotFound { .. }) => None,
        Err(err) => return Err(err),
    };
    if !ensure_client_dxvk(steam, dxvk.as_ref())? {
        tracing::warn!(
            "DXVK{} is not installed, so Steam's windows will stay black (its web UI needs Direct3D 11); install it with `uncork runtime install dxvk`",
            pin.map_or_else(String::new, |pin| format!(" {pin}"))
        );
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
    /// The started game process; `None` for a dry run. In
    /// [`LaunchMode::Applaunch`] mode this is the Steam client Uncork
    /// started, which starts the game.
    pub child: Option<std::process::Child>,
    /// INI files the profile changed before launch.
    pub ini_changed: Vec<std::path::PathBuf>,
    /// `true` if Steam had to be started first.
    pub started_steam: bool,
}

/// Play Steam app `appid` in `bottle`: the whole `uncork play` flow, shared
/// by the CLI and (later) the macOS app.
///
/// 1. Find the app ([`plan_game`] with `profile`, `args`, `options`).
/// 2. `dry_run`: return the plan without touching anything.
/// 3. Apply the profile's INI edits ([`crate::launch::apply_profile_ini`]).
/// 4. By launch mode: `Direct` → [`ensure_running`] (timeout
///    `steam_timeout`) then [`crate::launch::execute`] the plan;
///    `Applaunch` → [`stop`] a running client, then start the client with the
///    game's environment and `-applaunch <appid> <args>` (the game inherits
///    it); `Standalone` → execute the plan without Steam.
///
/// The launch mode is the profile's ([`GameProfile::launch_mode`]), and
/// [`LaunchMode::Direct`] without a profile. `%INSTALLDIR%` in INI paths is
/// the app's install directory. In `Applaunch` mode `steam_timeout` bounds
/// both stopping the old client and starting the new one, the plan's
/// backend files are put in place first, the game arguments are the
/// profile's `launch.args` then `args`, and the client's environment is the
/// game's (see the module docs for the trade-off).
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
    appid: u32,
    args: &[String],
    options: &LaunchOptions,
    steam_timeout: Duration,
    dry_run: bool,
) -> crate::Result<PlayOutcome> {
    let request = PlayRequest {
        components,
        profile,
        appid,
        args,
        options,
        steam_timeout,
        poll: POLL_INTERVAL,
    };
    play_with(layout, bottle, wine, &request, dry_run)
}

/// The inputs of [`play`] that pass through unchanged.
struct PlayRequest<'a> {
    components: &'a [InstalledComponent],
    profile: Option<&'a GameProfile>,
    appid: u32,
    args: &'a [String],
    options: &'a LaunchOptions,
    steam_timeout: Duration,
    poll: Duration,
}

/// [`play`] with a configurable polling interval.
fn play_with(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    request: &PlayRequest<'_>,
    dry_run: bool,
) -> crate::Result<PlayOutcome> {
    let app = find_game(bottle, request.appid)?;
    let ctx = PlanContext {
        layout,
        bottle,
        wine,
        components: request.components,
        profile: request.profile,
    };
    let plan = plan_found_game(ctx, &app, request.args, request.options)?;
    if dry_run {
        return Ok(PlayOutcome {
            plan,
            child: None,
            ini_changed: Vec::new(),
            started_steam: false,
        });
    }
    let ini_changed = match request.profile {
        Some(profile) => {
            crate::launch::apply_profile_ini(profile, bottle, Some(&app.install_path))?
        }
        None => Vec::new(),
    };
    let mode = request
        .profile
        .map_or(LaunchMode::Direct, GameProfile::launch_mode);
    let (child, started_steam) = match mode {
        LaunchMode::Direct => {
            let started =
                start_if_needed(layout, bottle, wine, request.steam_timeout, request.poll)?;
            (crate::launch::execute(&plan, bottle, wine)?, started)
        }
        LaunchMode::Applaunch => (applaunch(layout, bottle, wine, &plan, request)?, true),
        LaunchMode::Standalone => (crate::launch::execute(&plan, bottle, wine)?, false),
    };
    Ok(PlayOutcome {
        plan,
        child: Some(child),
        ini_changed,
        started_steam,
    })
}

/// The `Applaunch` branch of [`play`]; returns the client it started.
fn applaunch(
    layout: &Layout,
    bottle: &mut Bottle,
    wine: &WineRuntime,
    plan: &LaunchPlan,
    request: &PlayRequest<'_>,
) -> crate::Result<Child> {
    if is_running(bottle, wine)? {
        tracing::info!(
            "restarting Steam in bottle {} with the game's environment",
            bottle.config.name
        );
        stop_with(bottle, wine, request.steam_timeout, request.poll)?;
    }
    crate::launch::prepare(plan, bottle)?;
    let steam = find_steam(bottle)?;
    prepare_client_dxvk(layout, bottle, &steam)?;
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
    Ok(child)
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
            appid: 287_450,
            args,
            options,
            steam_timeout: Duration::from_secs(5),
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
        let mut outcome = play_with(&fx.layout, &mut fx.bottle, &fx.wine, &request, false).unwrap();
        assert!(outcome.started_steam);
        assert!(outcome.child.take().unwrap().wait().unwrap().success());
        let start = fx.position("steam.exe -silent -nofriendsui |");
        let game = fx.position(&format!("{} |", dir.join("riseofnations.exe").display()));
        assert!(start < game);
        assert!(fx.bottle.syswow64().join("d3d11.dll").is_file());
    }

    #[test]
    fn applaunch_restarts_steam_with_the_games_environment() {
        let mut fx = Fixture::new();
        fx.add_game();
        let components = fx.dxmt();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
        fx.flag("shutdown-works");
        let profile = applaunch_profile("dxmt");
        let args = ["-x".to_owned()];
        let options = LaunchOptions::default();
        let request = quick(&components, Some(&profile), &args, &options);

        let outcome = play_with(&fx.layout, &mut fx.bottle, &fx.wine, &request, false).unwrap();
        assert!(outcome.started_steam);
        assert_eq!(outcome.plan.activation.backend, Backend::Dxmt);
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
        let outcome = play_with(&fx.layout, &mut fx.bottle, &fx.wine, &request, false).unwrap();
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
        let err = play_with(&fx.layout, &mut fx.bottle, &fx.wine, &request, false).unwrap_err();
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
        install_with(&fx.layout, &fx.bottle, &fx.wine, |dest| {
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(dest, installer_bytes()).unwrap();
            fetched = Some(dest.to_path_buf());
            Ok("https://steam.invalid/SteamSetup.exe".to_owned())
        })
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
    fn install_skips_everything_when_steam_is_there() {
        let fx = Fixture::new();
        fx.install_steam();
        install_with(&fx.layout, &fx.bottle, &fx.wine, |_| {
            panic!("nothing should be downloaded")
        })
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
            let err = install_with(&fx.layout, &fx.bottle, &fx.wine, |dest| {
                fs::create_dir_all(dest.parent().unwrap()).unwrap();
                fs::write(dest, &bytes).unwrap();
                Ok("https://steam.invalid/SteamSetup.exe".to_owned())
            })
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
        let err = install_with(&fx.layout, &fx.bottle, &fx.wine, |dest| {
            fs::create_dir_all(dest.parent().unwrap()).unwrap();
            fs::write(dest, installer_bytes()).unwrap();
            Ok("https://steam.invalid/SteamSetup.exe".to_owned())
        })
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
        let err = install_with(&fx.layout, &fx.bottle, &fx.wine, |_| {
            Err(Error::Download {
                url: "https://steam.invalid/".to_owned(),
                message: "offline".to_owned(),
            })
        })
        .unwrap_err();
        assert!(matches!(err, Error::Download { .. }), "{err:?}");
    }

    #[test]
    fn stop_returns_once_the_client_exits() {
        let fx = Fixture::new();
        fx.install_steam();
        fs::write(fx.state.join("pid"), "0x274").unwrap();
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
            !calls.iter().any(|call| call.starts_with("wineserver")),
            "{calls:#?}"
        );
        assert_eq!(fx.pid().as_deref(), Some("0x0"));
    }

    #[test]
    fn start_reports_whether_it_started_the_client() {
        let fx = Fixture::new();
        fx.install_steam();
        let started = start_if_needed(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
            Duration::from_secs(5),
            Duration::from_millis(20),
        )
        .unwrap();
        assert!(started);
        let again = start_if_needed(
            &fx.layout,
            &fx.bottle,
            &fx.wine,
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
