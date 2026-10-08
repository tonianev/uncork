//! Bottles: Wine prefixes with Uncork settings.
//!
//! A bottle is a directory that is both the `WINEPREFIX` and the home of
//! `uncork.toml` ([`BottleConfig`], user-editable) and `uncork-state.toml`
//! ([`BottleState`], machine-written). Keeping Uncork's files at the prefix
//! root (like CrossOver's `cxbottle.conf`) makes import and export a plain
//! directory copy.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::{read_toml, write_toml_atomic};
use crate::graphics::{Backend, BackendChoice};
use crate::launch::in_bottle;
use crate::paths::Layout;
use crate::process::CommandSpec;
use crate::registry::{RegKey, RegValue};
use crate::wine::WineRuntime;

/// User-editable settings file at the bottle root.
pub const CONFIG_FILE: &str = "uncork.toml";
/// Machine-written state file at the bottle root.
pub const STATE_FILE: &str = "uncork-state.toml";

/// The Windows version Wine reports to programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowsVersion {
    /// Windows 7.
    Win7,
    /// Windows 8.1.
    Win81,
    /// Windows 10 (default; what Steam and most games expect).
    #[default]
    Win10,
    /// Windows 11.
    Win11,
}

impl WindowsVersion {
    /// The argument `winecfg /v` takes: `win7`, `win81`, `win10`, `win11`.
    #[must_use]
    pub fn winecfg_name(self) -> &'static str {
        match self {
            WindowsVersion::Win7 => "win7",
            WindowsVersion::Win81 => "win81",
            WindowsVersion::Win10 => "win10",
            WindowsVersion::Win11 => "win11",
        }
    }
}

/// Graphics settings of a bottle.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphicsConfig {
    /// Backend for programs without a profile that says otherwise.
    #[serde(default)]
    pub backend: BackendChoice,
    /// Pin a DXMT version; newest installed when `None`.
    #[serde(default)]
    pub dxmt: Option<String>,
    /// Pin a DXVK version; newest installed when `None`.
    #[serde(default)]
    pub dxvk: Option<String>,
    /// Pin a D3DMetal version; newest installed when `None`.
    #[serde(default)]
    pub d3dmetal: Option<String>,
}

/// Performance switches. Each maps to environment variables or registry
/// values; `docs/PERFORMANCE.md` explains them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PerformanceConfig {
    /// Mach-semaphore synchronization (`WINEMSYNC=1`) when the Wine
    /// component has the `msync` feature. Default on.
    #[serde(default = "yes")]
    pub msync: bool,
    /// Render at native Retina resolution (Mac driver `RetinaMode`). Default off:
    /// costs four times the pixels.
    #[serde(default)]
    pub retina: bool,
    /// Show Apple's Metal performance HUD (`MTL_HUD_ENABLED=1`).
    #[serde(default)]
    pub hud: bool,
    /// Ask the backend to upscale with MetalFX where supported.
    #[serde(default)]
    pub metalfx: bool,
    /// Let Rosetta advertise AVX/AVX2 (`ROSETTA_ADVERTISE_AVX=1`). Rosetta
    /// executes AVX/AVX2 on macOS 15 and later but hides it from CPUID unless
    /// asked; some games refuse to start without it. Default on.
    #[serde(default = "yes")]
    pub avx: bool,
    /// Launch through a Game Mode app bundle (see [`crate::gamemode`]).
    /// Experimental; default off.
    #[serde(default)]
    pub game_mode: bool,
}

fn yes() -> bool {
    true
}

impl Default for PerformanceConfig {
    fn default() -> PerformanceConfig {
        PerformanceConfig {
            msync: true,
            retina: false,
            hud: false,
            metalfx: false,
            avx: true,
            game_mode: false,
        }
    }
}

/// Contents of `uncork.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BottleConfig {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Bottle name (also the directory name).
    pub name: String,
    /// Version of the Wine component this bottle runs with.
    pub wine: String,
    /// Windows version reported to programs.
    #[serde(default)]
    pub windows_version: WindowsVersion,
    /// Graphics settings.
    #[serde(default)]
    pub graphics: GraphicsConfig,
    /// Performance switches.
    #[serde(default)]
    pub performance: PerformanceConfig,
    /// Extra environment variables for every launch in this bottle.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extra DLL overrides (`name` → `n,b` / `b` / `n` / `d` / ...).
    #[serde(default)]
    pub dll_overrides: BTreeMap<String, String>,
    /// Where it was imported from, if it was.
    #[serde(default)]
    pub imported_from: Option<PathBuf>,
}

/// One DLL Uncork copied into the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledDll {
    /// Path relative to the bottle, e.g. `drive_c/windows/syswow64/d3d11.dll`.
    pub path: PathBuf,
    /// SHA-256 of what was copied.
    pub sha256: String,
}

/// Which backend's DLLs currently sit in `system32`/`syswow64`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveBackend {
    /// The backend.
    pub backend: Backend,
    /// Version of the component the DLLs came from.
    pub version: String,
    /// Every DLL copied for it.
    pub dlls: Vec<InstalledDll>,
}

/// Contents of `uncork-state.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BottleState {
    /// Schema version, currently 1.
    #[serde(default = "one")]
    pub schema: u32,
    /// Backend DLLs present in the prefix, if any.
    #[serde(default)]
    pub active: Option<ActiveBackend>,
    /// Wine version that last initialized or updated the prefix.
    #[serde(default)]
    pub prefix_wine: Option<String>,
}

fn one() -> u32 {
    1
}

/// The state of a bottle Uncork has not touched yet: schema 1, nothing
/// recorded (the same as parsing an empty `uncork-state.toml`).
impl Default for BottleState {
    fn default() -> BottleState {
        BottleState {
            schema: one(),
            active: None,
            prefix_wine: None,
        }
    }
}

/// An opened bottle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bottle {
    /// Bottle directory (= `WINEPREFIX`).
    pub path: PathBuf,
    /// Settings.
    pub config: BottleConfig,
    /// State.
    pub state: BottleState,
}

impl Bottle {
    /// Open the bottle at `path` (reads both files; a missing state file is
    /// the default state).
    ///
    /// # Errors
    /// [`crate::Error::NotFound`] if `uncork.toml` is missing, otherwise
    /// [`crate::Error::Config`] / [`crate::Error::Io`].
    pub fn open(path: &Path) -> crate::Result<Bottle> {
        let config =
            read_toml_if_exists(&path.join(CONFIG_FILE), "bottle config")?.ok_or_else(|| {
                crate::Error::NotFound {
                    what: "bottle",
                    name: path.display().to_string(),
                    hint: format!("; the directory has no {CONFIG_FILE}"),
                }
            })?;
        let state =
            read_toml_if_exists(&path.join(STATE_FILE), "bottle state")?.unwrap_or_default();
        Ok(Bottle {
            path: path.to_path_buf(),
            config,
            state,
        })
    }

    /// Open `bottles/<name>`.
    ///
    /// # Errors
    /// As [`Bottle::open`]; `NotFound` hints at `uncork bottle list`.
    pub fn open_named(layout: &Layout, name: &str) -> crate::Result<Bottle> {
        validate_name(name)?;
        let path = layout.bottle_dir(name);
        if !path.is_dir() {
            return Err(bottle_not_found(name));
        }
        Bottle::open(&path).map_err(|err| match err {
            crate::Error::NotFound { .. } => bottle_not_found(name),
            other => other,
        })
    }

    /// The prefix directory (same as `path`).
    #[must_use]
    pub fn prefix(&self) -> &Path {
        &self.path
    }

    /// `drive_c`.
    #[must_use]
    pub fn drive_c(&self) -> PathBuf {
        self.path.join("drive_c")
    }

    /// `drive_c/windows/system32` (64-bit DLLs in a 64-bit prefix).
    #[must_use]
    pub fn system32(&self) -> PathBuf {
        self.drive_c().join("windows").join("system32")
    }

    /// `drive_c/windows/syswow64` (32-bit DLLs in a 64-bit prefix).
    #[must_use]
    pub fn syswow64(&self) -> PathBuf {
        self.drive_c().join("windows").join("syswow64")
    }

    /// `true` once Wine has initialized the prefix (`system.reg` exists).
    #[must_use]
    pub fn is_initialized(&self) -> bool {
        self.path.join("system.reg").is_file()
    }

    /// Write `uncork.toml` atomically.
    ///
    /// # Errors
    /// [`crate::Error::Io`].
    pub fn save_config(&self) -> crate::Result<()> {
        write_toml_atomic(&self.path.join(CONFIG_FILE), &self.config)
    }

    /// Write `uncork-state.toml` atomically.
    ///
    /// # Errors
    /// [`crate::Error::Io`].
    pub fn save_state(&self) -> crate::Result<()> {
        write_toml_atomic(&self.path.join(STATE_FILE), &self.state)
    }
}

/// Read a TOML file, or `None` if it does not exist.
fn read_toml_if_exists<T: serde::de::DeserializeOwned>(
    path: &Path,
    what: &'static str,
) -> crate::Result<Option<T>> {
    match read_toml(path, what) {
        Ok(value) => Ok(Some(value)),
        Err(crate::Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn bottle_not_found(name: &str) -> crate::Error {
    crate::Error::NotFound {
        what: "bottle",
        name: name.to_owned(),
        hint: "; run `uncork bottle list` to see your bottles".to_owned(),
    }
}

/// Bottle names: 1–64 chars of `[A-Za-z0-9._-]`, not starting with `.` or
/// `-`, not `.`/`..`.
///
/// # Errors
/// [`crate::Error::InvalidName`].
pub fn validate_name(name: &str) -> crate::Result<()> {
    let invalid = |reason| crate::Error::InvalidName {
        what: "bottle",
        name: name.to_owned(),
        reason,
    };
    if name.is_empty() {
        return Err(invalid("it is empty"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid("use only letters, digits, '.', '_' and '-'"));
    }
    if name.len() > MAX_NAME_LEN {
        return Err(invalid("it is longer than 64 characters"));
    }
    if name.starts_with(['.', '-']) {
        return Err(invalid("it must not start with '.' or '-'"));
    }
    Ok(())
}

/// Longest bottle name [`validate_name`] accepts.
const MAX_NAME_LEN: usize = 64;

/// Every bottle under `bottles/`, sorted by name. Directories without
/// `uncork.toml` are skipped; unreadable configs are logged and skipped.
///
/// # Errors
/// [`crate::Error::Io`] if `bottles/` exists but cannot be listed.
pub fn list(layout: &Layout) -> crate::Result<Vec<Bottle>> {
    let dir = layout.bottles_dir();
    let list_error = |err| crate::Error::io("cannot list directory", &dir, err);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(list_error(err)),
    };
    let mut bottles = Vec::new();
    for entry in entries {
        let path = entry.map_err(list_error)?.path();
        if !path.join(CONFIG_FILE).is_file() {
            continue;
        }
        // Only list what `open_named` can open again.
        let named_validly = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| validate_name(name).is_ok());
        if !named_validly {
            tracing::warn!("skipping {}: not a valid bottle name", path.display());
            continue;
        }
        match Bottle::open(&path) {
            Ok(bottle) => bottles.push(bottle),
            Err(err) => tracing::warn!("skipping bottle {}: {err}", path.display()),
        }
    }
    bottles.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(bottles)
}

/// Options for [`create`].
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CreateOptions {
    /// Windows version.
    pub windows_version: WindowsVersion,
    /// Initial graphics settings.
    pub graphics: GraphicsConfig,
    /// Initial performance settings.
    pub performance: PerformanceConfig,
}

/// Create and initialize a bottle.
///
/// 1. Validate the name; fail with `AlreadyExists` if the directory exists.
/// 2. Create the directory and write `uncork.toml`.
/// 3. Run [`crate::wine::WineRuntime::boot_init_command`] (`wineboot -u`)
///    under a watchdog ([`BOOT_TIMEOUT`]; on timeout `wineserver --kill` and
///    fail — wineboot can hang on macOS 26+, Wine bug 59595), then wait for
///    the wineserver to exit ([`crate::wine::WineRuntime::wait_command`]).
/// 4. Apply [`default_registry`] (Mac driver settings and the Windows
///    version as `HKCU\Software\Wine` `Version`) with `regedit /S`.
/// 5. Record `prefix_wine` in the state file.
/// 6. Prewarm Rosetta's translation cache by running `wine --version`.
///
/// Logs go to `logs/bottle-<name>-create.log`. On failure the half-made
/// directory is left in place for inspection and the error names the log.
///
/// # Errors
/// Name, I/O or command errors.
pub fn create(
    layout: &Layout,
    name: &str,
    wine: &WineRuntime,
    options: &CreateOptions,
) -> crate::Result<Bottle> {
    create_with_timeout(layout, name, wine, options, BOOT_TIMEOUT)
}

/// How long `wineboot` may take before [`create`] gives up.
pub const BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// How often the `wineboot` watchdog checks whether it has exited.
const BOOT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// [`create`] with a configurable `wineboot` watchdog (tests use a short one).
fn create_with_timeout(
    layout: &Layout,
    name: &str,
    wine: &WineRuntime,
    options: &CreateOptions,
    boot_timeout: Duration,
) -> crate::Result<Bottle> {
    validate_name(name)?;
    let path = create_bottle_dir(layout, name)?;
    let mut bottle = Bottle {
        path,
        config: BottleConfig {
            windows_version: options.windows_version,
            graphics: options.graphics.clone(),
            performance: options.performance.clone(),
            ..new_config(name, &wine.version)
        },
        state: BottleState::default(),
    };
    bottle.save_config()?;

    let log = layout.logs_dir().join(format!("bottle-{name}-create.log"));
    tracing::info!(
        "initializing bottle {name} with Wine {} (log: {})",
        wine.version,
        log.display()
    );
    boot_prefix(&bottle, wine, &log, boot_timeout)?;
    crate::process::run(&logged(
        in_bottle(wine.wait_command(&bottle.path), &bottle, wine),
        Some(&log),
    ))?;
    import_registry(&bottle, wine, &default_registry(&bottle.config), Some(&log))?;
    bottle.state.prefix_wine = Some(wine.version.clone());
    bottle.save_state()?;

    // The bottle is complete at this point; a failed prewarm only costs the
    // first launch some translation time.
    if let Err(err) = crate::process::run(&logged(wine.version_command(), Some(&log))) {
        tracing::warn!("could not prewarm Rosetta for Wine {}: {err}", wine.version);
    }
    Ok(bottle)
}

/// A fresh `uncork.toml` with every optional setting at its default.
fn new_config(name: &str, wine_version: &str) -> BottleConfig {
    BottleConfig {
        schema: 1,
        name: name.to_owned(),
        wine: wine_version.to_owned(),
        windows_version: WindowsVersion::default(),
        graphics: GraphicsConfig::default(),
        performance: PerformanceConfig::default(),
        env: BTreeMap::new(),
        dll_overrides: BTreeMap::new(),
        imported_from: None,
    }
}

/// Create `bottles/<name>`, failing if anything (even a dangling symlink)
/// already has that name.
fn create_bottle_dir(layout: &Layout, name: &str) -> crate::Result<PathBuf> {
    let bottles = layout.bottles_dir();
    std::fs::create_dir_all(&bottles)
        .map_err(|err| crate::Error::io("cannot create directory", &bottles, err))?;
    let path = layout.bottle_dir(name);
    match std::fs::create_dir(&path) {
        Ok(()) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => Err(bottle_exists(name, path)),
        Err(err) => Err(crate::Error::io("cannot create directory", path, err)),
    }
}

fn bottle_exists(name: &str, path: PathBuf) -> crate::Error {
    crate::Error::AlreadyExists {
        what: "bottle",
        name: name.to_owned(),
        path,
    }
}

/// Run `wineboot -u` under the watchdog. On timeout the loader is killed and
/// so is every other process of the prefix (`wineserver --kill`).
fn boot_prefix(
    bottle: &Bottle,
    wine: &WineRuntime,
    log: &Path,
    timeout: Duration,
) -> crate::Result<()> {
    let failed = |status: String| crate::Error::Command {
        program: "wineboot".to_owned(),
        status,
        log_hint: format!(" (log: {})", log.display()),
    };
    let mut child = crate::process::spawn(&logged(
        in_bottle(wine.boot_init_command(&bottle.path), bottle, wine),
        Some(log),
    ))?;
    let exit = wait_with_timeout(&mut child, timeout)
        .map_err(|err| crate::Error::io("cannot wait for wineboot in", &bottle.path, err))?;
    match exit {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(failed(describe_exit(status))),
        None => {
            stop(&mut child);
            if let Err(err) = crate::process::run(&logged(
                in_bottle(wine.kill_command(&bottle.path), bottle, wine),
                Some(log),
            )) {
                tracing::warn!(
                    "could not stop the wineserver of {}: {err}",
                    bottle.path.display()
                );
            }
            Err(failed(format!(
                "did not finish within {timeout:?} and was stopped (it can hang on macOS 26 and later, Wine bug 59595)"
            )))
        }
    }
}

/// Wait for `child`, polling every [`BOOT_POLL_INTERVAL`]; `None` when it
/// is still running after `timeout`.
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        std::thread::sleep(BOOT_POLL_INTERVAL.min(deadline - now));
    }
}

/// Kill and reap `child`.
fn stop(child: &mut Child) {
    if let Err(err) = child.kill() {
        tracing::debug!("cannot kill process {}: {err}", child.id());
    }
    if let Err(err) = child.wait() {
        tracing::debug!("cannot reap process {}: {err}", child.id());
    }
}

/// "exited with status N", matching [`crate::Error::Command`]'s wording.
fn describe_exit(status: ExitStatus) -> String {
    match status.code() {
        Some(code) => format!("exited with status {code}"),
        None => format!("ended abnormally ({status})"),
    }
}

/// `spec` with its output appended to `log`, if given.
fn logged(spec: CommandSpec, log: Option<&Path>) -> CommandSpec {
    CommandSpec {
        log: log.map(Path::to_path_buf),
        ..spec
    }
}

/// `HKEY_CURRENT_USER\Software\Wine`, where `winecfg` keeps the Windows version.
const WINE_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine";

/// Registry defaults for a bottle, derived from its config. Under
/// `HKEY_CURRENT_USER\Software\Wine\Mac Driver` (string values `y`/`n`;
/// key names from Wine's `winemac.drv/macdrv_main.c`):
///
/// - `RetinaMode` = `y` if `performance.retina` else `n`
/// - `UseConfinementCursorClipping` = `y`, `CursorClippingLocksWindows` = `y`
///   (keeps the cursor in the window: RTS edge scrolling)
/// - `AllowVerticalSync` = `y`
/// - `EnableAppNap` = `n` (App Nap throttles a backgrounded Steam client)
///
/// and under `HKEY_CURRENT_USER\Software\Wine`: `Version` = the
/// [`WindowsVersion::winecfg_name`].
#[must_use]
pub fn default_registry(config: &BottleConfig) -> Vec<crate::registry::RegKey> {
    let flag = |name: &str, on: bool| {
        (
            name.to_owned(),
            RegValue::Sz(if on { "y" } else { "n" }.to_owned()),
        )
    };
    vec![
        RegKey {
            path: crate::registry::MAC_DRIVER_KEY.to_owned(),
            values: vec![
                flag("RetinaMode", config.performance.retina),
                flag("UseConfinementCursorClipping", true),
                flag("CursorClippingLocksWindows", true),
                flag("AllowVerticalSync", true),
                flag("EnableAppNap", false),
            ],
        },
        RegKey {
            path: WINE_KEY.to_owned(),
            values: vec![(
                "Version".to_owned(),
                RegValue::Sz(config.windows_version.winecfg_name().to_owned()),
            )],
        },
    ]
}

/// Apply [`default_registry`] to an existing bottle (used by `bottle set`
/// and before a launch when `retina` changes).
///
/// # Errors
/// I/O or command errors.
pub fn apply_registry(
    bottle: &Bottle,
    wine: &WineRuntime,
    keys: &[crate::registry::RegKey],
) -> crate::Result<()> {
    import_registry(bottle, wine, keys, None)
}

/// Distinguishes the `.reg` files of concurrent imports within one process.
static REG_FILE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Render `keys` into `drive_c/windows/temp/uncork-<pid>-<n>.reg`, import
/// it with `regedit /S`, and remove the file again.
fn import_registry(
    bottle: &Bottle,
    wine: &WineRuntime,
    keys: &[RegKey],
    log: Option<&Path>,
) -> crate::Result<()> {
    if keys.is_empty() {
        return Ok(());
    }
    let temp_dir = bottle.drive_c().join("windows").join("temp");
    std::fs::create_dir_all(&temp_dir)
        .map_err(|err| crate::Error::io("cannot create directory", &temp_dir, err))?;
    let file_name = format!(
        "uncork-{}-{}.reg",
        std::process::id(),
        REG_FILE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let reg_file = temp_dir.join(&file_name);
    std::fs::write(&reg_file, crate::registry::render(keys))
        .map_err(|err| crate::Error::io("cannot write", &reg_file, err))?;

    let windows_path = format!(r"C:\windows\temp\{file_name}");
    let imported = crate::process::run(&logged(
        in_bottle(
            wine.regedit_import_command(&bottle.path, &windows_path),
            bottle,
            wine,
        ),
        log,
    ));
    if let Err(err) = std::fs::remove_file(&reg_file) {
        tracing::warn!("cannot remove {}: {err}", reg_file.display());
    }
    imported
}

/// Delete a bottle directory after killing its wineserver.
///
/// # Errors
/// [`crate::Error::NotFound`] or [`crate::Error::Io`].
pub fn delete(layout: &Layout, name: &str, wine: Option<&WineRuntime>) -> crate::Result<()> {
    validate_name(name)?;
    let path = layout.bottle_dir(name);
    if !path.is_dir() {
        return Err(bottle_not_found(name));
    }
    if let Some(wine) = wine {
        // Fails harmlessly when no wineserver runs for the prefix.
        let kill = match Bottle::open(&path) {
            Ok(bottle) => in_bottle(wine.kill_command(&path), &bottle, wine),
            Err(_) => wine.kill_command(&path),
        };
        if let Err(err) = crate::process::run(&kill) {
            tracing::debug!("wineserver --kill for bottle {name}: {err}");
        }
    }
    std::fs::remove_dir_all(&path).map_err(|err| crate::Error::io("cannot delete", path, err))
}

/// How [`import`] brings an existing prefix in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Copy with APFS clones (`/bin/cp -c -R`): instant, no extra space until
    /// files change, and the original is never touched. Default.
    Clone,
    /// Move the directory (`rename`); the original location stops existing.
    Move,
}

/// Import an existing Wine prefix (a CrossOver bottle, a Whisky bottle or a
/// plain `WINEPREFIX`) as bottle `name`. `source` must contain `drive_c` and
/// `system.reg`. Writes a fresh `uncork.toml` with `imported_from` set; the
/// prefix is updated by Wine on first launch. When `drive_c/users` holds
/// exactly one user other than `Public` and it is not the current `USER`
/// (CrossOver bottles use `crossover`), the bottle's `env` sets `USER` and
/// `LOGNAME` to it so Wine keeps using that profile.
///
/// A Steam client that was running in the source (a CrossOver bottle copied
/// while Steam ran) left its `ActiveProcess` pid in the copy's `user.reg`;
/// it is set to 0 ([`crate::steam`] would otherwise take the copy's Steam
/// for running). A failure to do so is only logged.
///
/// # Errors
/// [`crate::Error::NotFound`] when `source` is not a prefix,
/// [`crate::Error::AlreadyExists`], or I/O / command errors.
pub fn import(
    layout: &Layout,
    source: &Path,
    name: &str,
    wine_version: &str,
    mode: ImportMode,
) -> crate::Result<Bottle> {
    validate_name(name)?;
    let source = prefix_source(source)?;
    let dest = layout.bottle_dir(name);
    if std::fs::symlink_metadata(&dest).is_ok() {
        return Err(bottle_exists(name, dest));
    }
    let bottles = layout.bottles_dir();
    std::fs::create_dir_all(&bottles)
        .map_err(|err| crate::Error::io("cannot create directory", &bottles, err))?;
    if std::fs::canonicalize(&bottles).is_ok_and(|bottles| bottles.starts_with(&source)) {
        return Err(crate::Error::io(
            "cannot import a prefix that contains the bottles directory",
            source,
            io::ErrorKind::InvalidInput.into(),
        ));
    }

    match mode {
        ImportMode::Clone => {
            let log = layout.logs_dir().join(format!("bottle-{name}-import.log"));
            clone_dir(&source, &dest, &log)?;
        }
        ImportMode::Move => {
            std::fs::rename(&source, &dest)
                .map_err(|err| crate::Error::io("cannot move", &source, err))?;
        }
    }

    let mut config = BottleConfig {
        imported_from: Some(source),
        ..new_config(name, wine_version)
    };
    if let Some(user) = prefix_user(&dest) {
        // Wine names the Windows profile after USER. CrossOver bottles use
        // `crossover`; running as anyone else would start from an empty
        // AppData (no Steam sign-in, no game settings).
        if std::env::var("USER").ok().as_deref() != Some(user.as_str()) {
            config.env.insert("USER".to_owned(), user.clone());
            config.env.insert("LOGNAME".to_owned(), user);
        }
    }
    write_toml_atomic(&dest.join(CONFIG_FILE), &config)?;
    match crate::steam::forget_client_on_disk(&dest) {
        Ok(true) => tracing::info!("cleared the Steam client pid the source prefix left behind"),
        Ok(false) => {}
        Err(err) => tracing::warn!("cannot clear Steam's running marker in {name}: {err}"),
    }
    // Keeps an `uncork-state.toml` that came with the prefix: it still
    // describes the files that were copied along with it.
    Bottle::open(&dest)
}

/// The prefix's Windows user: the only directory in `drive_c/users` other
/// than `Public` (case-insensitive). `None` when there is not exactly one.
fn prefix_user(prefix: &Path) -> Option<String> {
    let entries = std::fs::read_dir(prefix.join("drive_c").join("users")).ok()?;
    let mut users = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.eq_ignore_ascii_case("public") && !name.starts_with('.'));
    let user = users.next()?;
    users.next().is_none().then_some(user)
}

/// `source`, resolved, if it is a Wine prefix (`drive_c` and `system.reg`).
fn prefix_source(source: &Path) -> crate::Result<PathBuf> {
    if !source.join("drive_c").is_dir() || !source.join("system.reg").is_file() {
        return Err(crate::Error::NotFound {
            what: "Wine prefix",
            name: source.display().to_string(),
            hint: "; a prefix directory contains drive_c and system.reg".to_owned(),
        });
    }
    std::fs::canonicalize(source).map_err(|err| crate::Error::io("cannot resolve", source, err))
}

/// The copy program; `-c` asks for APFS clones (`clonefile(2)`).
const CP: &str = "/bin/cp";

/// Copy `source` to `dest` (which must not exist) with APFS clones, falling
/// back to a plain copy when cloning is impossible (another volume, a file
/// system without clones). A failed copy leaves nothing behind.
fn clone_dir(source: &Path, dest: &Path, log: &Path) -> crate::Result<()> {
    let clone = CommandSpec::new(CP)
        .args(["-c", "-R"])
        .arg(source)
        .arg(dest);
    match crate::process::run(&logged(clone, Some(log))) {
        Ok(()) => return Ok(()),
        Err(err) => tracing::info!(
            "cloning {} failed, copying instead: {err}",
            source.display()
        ),
    }
    // `cp -R` into an existing directory would nest the copy inside it.
    remove_partial_copy(dest)?;
    let copy = CommandSpec::new(CP).arg("-R").arg(source).arg(dest);
    crate::process::run(&logged(copy, Some(log))).inspect_err(|_| {
        if let Err(err) = remove_partial_copy(dest) {
            tracing::warn!("{err}");
        }
    })
}

fn remove_partial_copy(dest: &Path) -> crate::Result<()> {
    match std::fs::remove_dir_all(dest) {
        Err(err) if err.kind() != io::ErrorKind::NotFound => {
            Err(crate::Error::io("cannot remove partial copy", dest, err))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use proptest::prelude::{prop_assert, proptest};

    use super::*;

    fn home() -> (tempfile::TempDir, Layout) {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path().join("home"));
        (dir, layout)
    }

    fn config(name: &str) -> BottleConfig {
        new_config(name, "11.0")
    }

    fn mkdirs(path: &Path) {
        std::fs::create_dir_all(path).unwrap();
    }

    fn write(path: &Path, text: &str) {
        mkdirs(path.parent().unwrap());
        std::fs::write(path, text).unwrap();
    }

    /// A directory that looks like a Wine prefix.
    fn fake_prefix(path: &Path) {
        write(
            &path.join("drive_c/windows/system32/kernel32.dll"),
            "kernel32",
        );
        write(&path.join("system.reg"), "WINE REGISTRY Version 2\n");
        write(&path.join("user.reg"), "WINE REGISTRY Version 2\n");
        mkdirs(&path.join("dosdevices"));
        std::os::unix::fs::symlink("../drive_c", path.join("dosdevices/c:")).unwrap();
    }

    // ----- names -----

    #[test]
    fn valid_names() {
        for name in ["steam", "Steam-2", "a", "a.b_c-d", "9", &"x".repeat(64)] {
            assert!(validate_name(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn invalid_names() {
        for (name, why) in [
            ("", "empty"),
            (".", "start with"),
            ("..", "start with"),
            (".hidden", "start with"),
            ("-flag", "start with"),
            ("a/b", "only letters"),
            ("a b", "only letters"),
            ("ünï", "only letters"),
            ("a\0", "only letters"),
            (&"x".repeat(65), "longer than 64"),
        ] {
            let err = validate_name(name).unwrap_err();
            let crate::Error::InvalidName { what, reason, .. } = &err else {
                panic!("{name:?}: expected InvalidName, got {err:?}");
            };
            assert_eq!(*what, "bottle");
            assert!(reason.contains(why), "{name:?}: {reason}");
        }
    }

    proptest! {
        #[test]
        fn names_from_the_documented_alphabet_are_valid(name in "[A-Za-z0-9_][A-Za-z0-9._-]{0,63}") {
            prop_assert!(validate_name(&name).is_ok());
        }

        #[test]
        fn names_with_other_characters_are_invalid(name in "[a-z]{0,5}[^A-Za-z0-9._-][a-z]{0,5}") {
            prop_assert!(validate_name(&name).is_err());
        }
    }

    // ----- files -----

    #[test]
    fn a_minimal_config_gets_the_defaults() {
        let parsed: BottleConfig =
            toml::from_str("schema = 1\nname = \"steam\"\nwine = \"11.0\"\n").unwrap();
        assert_eq!(parsed, config("steam"));
        assert!(parsed.performance.msync);
        assert!(parsed.performance.avx);
        assert!(!parsed.performance.retina);
        assert_eq!(parsed.graphics.backend, BackendChoice::Auto);
        assert_eq!(parsed.windows_version, WindowsVersion::Win10);
    }

    #[test]
    fn config_typos_are_rejected() {
        let text = "schema = 1\nname = \"steam\"\nwine = \"11.0\"\n[performance]\nretna = true\n";
        assert!(toml::from_str::<BottleConfig>(text).is_err());
    }

    #[test]
    fn a_full_config_round_trips() {
        let original = BottleConfig {
            windows_version: WindowsVersion::Win81,
            graphics: GraphicsConfig {
                backend: BackendChoice::Fixed(Backend::Dxmt),
                dxmt: Some("0.80".to_owned()),
                dxvk: None,
                d3dmetal: Some("3.0".to_owned()),
            },
            performance: PerformanceConfig {
                msync: false,
                retina: true,
                hud: true,
                metalfx: true,
                avx: false,
                game_mode: true,
            },
            env: BTreeMap::from([("DXMT_LOG_LEVEL".to_owned(), "info".to_owned())]),
            dll_overrides: BTreeMap::from([("d3dcompiler_47".to_owned(), "n,b".to_owned())]),
            imported_from: Some(PathBuf::from("/Users/me/Bottles/Steam")),
            ..config("steam")
        };
        let text = toml::to_string(&original).unwrap();
        assert_eq!(
            toml::from_str::<BottleConfig>(&text).unwrap(),
            original,
            "{text}"
        );
    }

    proptest! {
        #[test]
        fn configs_round_trip(
            name in "[a-z][a-z0-9-]{0,10}",
            env in proptest::collection::btree_map("[A-Z_]{1,8}", "[ -~]{0,16}", 0..4),
            overrides in proptest::collection::btree_map("[a-z0-9_.]{1,8}", "(n|b|n,b|b,n|d|)", 0..4),
            (retina, hud, msync) in proptest::arbitrary::any::<(bool, bool, bool)>(),
        ) {
            let original = BottleConfig {
                env,
                dll_overrides: overrides,
                performance: PerformanceConfig { retina, hud, msync, ..PerformanceConfig::default() },
                ..config(&name)
            };
            let text = toml::to_string(&original).unwrap();
            prop_assert!(toml::from_str::<BottleConfig>(&text).unwrap() == original, "{}", text);
        }
    }

    #[test]
    fn the_default_state_is_an_empty_file() {
        let state = BottleState::default();
        assert_eq!(state.schema, 1);
        assert_eq!(toml::from_str::<BottleState>("").unwrap(), state);
    }

    #[test]
    fn a_state_round_trips() {
        let state = BottleState {
            schema: 1,
            active: Some(ActiveBackend {
                backend: Backend::Dxmt,
                version: "0.80".to_owned(),
                dlls: vec![InstalledDll {
                    path: PathBuf::from("drive_c/windows/syswow64/winemetal.dll"),
                    sha256: "ab".repeat(32),
                }],
            }),
            prefix_wine: Some("11.0".to_owned()),
        };
        let text = toml::to_string(&state).unwrap();
        assert_eq!(
            toml::from_str::<BottleState>(&text).unwrap(),
            state,
            "{text}"
        );
    }

    #[test]
    fn bottle_paths() {
        let bottle = Bottle {
            path: PathBuf::from("/b/steam"),
            config: config("steam"),
            state: BottleState::default(),
        };
        assert_eq!(bottle.prefix(), Path::new("/b/steam"));
        assert_eq!(
            bottle.system32(),
            Path::new("/b/steam/drive_c/windows/system32")
        );
        assert_eq!(
            bottle.syswow64(),
            Path::new("/b/steam/drive_c/windows/syswow64")
        );
        assert!(!bottle.is_initialized());
    }

    // ----- registry -----

    fn sz(value: &str) -> RegValue {
        RegValue::Sz(value.to_owned())
    }

    #[test]
    fn default_registry_sets_the_mac_driver_and_windows_version() {
        let keys = default_registry(&config("steam"));
        assert_eq!(
            keys,
            [
                RegKey {
                    path: r"HKEY_CURRENT_USER\Software\Wine\Mac Driver".to_owned(),
                    values: vec![
                        ("RetinaMode".to_owned(), sz("n")),
                        ("UseConfinementCursorClipping".to_owned(), sz("y")),
                        ("CursorClippingLocksWindows".to_owned(), sz("y")),
                        ("AllowVerticalSync".to_owned(), sz("y")),
                        ("EnableAppNap".to_owned(), sz("n")),
                    ],
                },
                RegKey {
                    path: r"HKEY_CURRENT_USER\Software\Wine".to_owned(),
                    values: vec![("Version".to_owned(), sz("win10"))],
                },
            ]
        );
    }

    #[test]
    fn default_registry_follows_the_config() {
        let mut config = config("steam");
        config.performance.retina = true;
        config.windows_version = WindowsVersion::Win7;
        let keys = default_registry(&config);
        assert_eq!(keys[0].values[0], ("RetinaMode".to_owned(), sz("y")));
        assert_eq!(keys[1].values[0], ("Version".to_owned(), sz("win7")));
    }

    #[test]
    fn applying_no_keys_runs_nothing() {
        let (_dir, layout) = home();
        let bottle = Bottle {
            path: layout.bottle_dir("x"),
            config: config("x"),
            state: BottleState::default(),
        };
        let wine = WineRuntime {
            root: PathBuf::from("/nonexistent"),
            version: "11.0".to_owned(),
            features: Vec::new(),
        };
        apply_registry(&bottle, &wine, &[]).unwrap();
        assert!(!bottle.path.exists());
    }

    // ----- lookups that fail before reading anything -----

    #[test]
    fn listing_without_a_bottles_directory_is_empty() {
        let (_dir, layout) = home();
        assert!(list(&layout).unwrap().is_empty());
    }

    #[test]
    fn listing_skips_directories_that_are_not_bottles() {
        let (_dir, layout) = home();
        mkdirs(&layout.bottle_dir("empty"));
        fake_prefix(&layout.bottle_dir("plain-prefix"));
        write(&layout.bottles_dir().join("stray-file"), "");
        assert!(list(&layout).unwrap().is_empty());
    }

    #[test]
    fn listing_an_unreadable_bottles_path_fails() {
        let (_dir, layout) = home();
        write(
            &layout.bottles_dir(),
            "a file where the directory should be",
        );
        let err = list(&layout).unwrap_err();
        assert!(
            matches!(&err, crate::Error::Io { path, .. } if path == &layout.bottles_dir()),
            "{err:?}"
        );
    }

    #[test]
    fn opening_by_an_invalid_name_fails_early() {
        let (_dir, layout) = home();
        let err = Bottle::open_named(&layout, "../escape").unwrap_err();
        assert!(matches!(err, crate::Error::InvalidName { .. }), "{err:?}");
    }

    #[test]
    fn opening_a_missing_bottle_points_at_the_list() {
        let (_dir, layout) = home();
        let err = Bottle::open_named(&layout, "nope").unwrap_err();
        assert!(
            matches!(&err, crate::Error::NotFound { what: "bottle", .. }),
            "{err:?}"
        );
        assert!(err.to_string().contains("uncork bottle list"), "{err}");
    }

    // ----- create: failures before Wine runs -----

    fn unused_wine() -> WineRuntime {
        WineRuntime {
            root: PathBuf::from("/nonexistent/wine"),
            version: "11.0".to_owned(),
            features: Vec::new(),
        }
    }

    #[test]
    fn create_rejects_invalid_names() {
        let (_dir, layout) = home();
        let err = create(&layout, "a/b", &unused_wine(), &CreateOptions::default()).unwrap_err();
        assert!(matches!(err, crate::Error::InvalidName { .. }), "{err:?}");
        assert!(!layout.bottles_dir().exists());
    }

    #[test]
    fn create_refuses_to_reuse_a_name() {
        let (_dir, layout) = home();
        write(&layout.bottle_dir("steam").join("keep.txt"), "mine");
        write(&layout.bottle_dir("file"), "");
        std::os::unix::fs::symlink("/nonexistent", layout.bottle_dir("dangling")).unwrap();

        for name in ["steam", "file", "dangling"] {
            let err = create(&layout, name, &unused_wine(), &CreateOptions::default()).unwrap_err();
            assert!(
                matches!(&err, crate::Error::AlreadyExists { what: "bottle", path, .. } if path == &layout.bottle_dir(name)),
                "{name}: {err:?}"
            );
        }
        assert_eq!(
            std::fs::read_to_string(layout.bottle_dir("steam").join("keep.txt")).unwrap(),
            "mine"
        );
        assert!(!layout.bottle_dir("steam").join(CONFIG_FILE).exists());
    }

    // ----- the watchdog -----

    #[test]
    fn waiting_returns_the_exit_status() {
        let mut child = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 3"])
            .spawn()
            .unwrap();
        let status = wait_with_timeout(&mut child, Duration::from_secs(10))
            .unwrap()
            .expect("exited");
        assert_eq!(status.code(), Some(3));
        assert_eq!(describe_exit(status), "exited with status 3");
    }

    #[test]
    fn waiting_gives_up_after_the_timeout() {
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        let started = Instant::now();
        assert_eq!(
            wait_with_timeout(&mut child, Duration::from_millis(250)).unwrap(),
            None
        );
        let waited = started.elapsed();
        assert!(
            waited >= Duration::from_millis(250) && waited < Duration::from_secs(5),
            "{waited:?}"
        );

        stop(&mut child);
        let status = child.try_wait().unwrap().expect("stop reaps the child");
        assert!(
            describe_exit(status).starts_with("ended abnormally"),
            "{}",
            describe_exit(status)
        );
    }

    #[test]
    fn stopping_an_exited_child_is_harmless() {
        let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
        child.wait().unwrap();
        stop(&mut child);
    }

    #[test]
    fn logged_only_sets_the_log() {
        let spec = CommandSpec::new("/bin/echo").arg("hi").env("A", "1");
        let with_log = logged(spec.clone(), Some(Path::new("/tmp/x.log")));
        assert_eq!(with_log.log.as_deref(), Some(Path::new("/tmp/x.log")));
        assert_eq!(
            CommandSpec {
                log: None,
                ..with_log
            },
            spec
        );
        assert_eq!(logged(spec.clone(), None), spec);
    }

    // ----- delete and import: checks before anything changes -----

    #[test]
    fn delete_checks_the_name_and_existence() {
        let (_dir, layout) = home();
        assert!(matches!(
            delete(&layout, "..", None),
            Err(crate::Error::InvalidName { .. })
        ));
        assert!(matches!(
            delete(&layout, "nope", None),
            Err(crate::Error::NotFound { .. })
        ));
    }

    #[test]
    fn delete_removes_the_directory() {
        let (_dir, layout) = home();
        fake_prefix(&layout.bottle_dir("old"));
        write(&layout.bottle_dir("keep").join(CONFIG_FILE), "");
        delete(&layout, "old", None).unwrap();
        assert!(!layout.bottle_dir("old").exists());
        assert!(layout.bottle_dir("keep").exists());
    }

    #[test]
    fn delete_removes_a_symlinked_bottle_but_not_its_target() {
        let (dir, layout) = home();
        let elsewhere = dir.path().join("elsewhere");
        fake_prefix(&elsewhere);
        mkdirs(&layout.bottles_dir());
        std::os::unix::fs::symlink(&elsewhere, layout.bottle_dir("linked")).unwrap();
        delete(&layout, "linked", None).unwrap();
        assert!(std::fs::symlink_metadata(layout.bottle_dir("linked")).is_err());
        assert!(elsewhere.join("system.reg").is_file());
    }

    #[test]
    fn import_needs_a_prefix() {
        let (dir, layout) = home();
        let source = dir.path().join("source");
        let check = |source: &Path| {
            import(&layout, source, "imported", "11.0", ImportMode::Clone).unwrap_err()
        };

        assert!(matches!(
            check(&source),
            crate::Error::NotFound {
                what: "Wine prefix",
                ..
            }
        ));
        mkdirs(&source.join("drive_c"));
        let err = check(&source);
        assert!(err.to_string().contains("drive_c and system.reg"), "{err}");
        std::fs::remove_dir(source.join("drive_c")).unwrap();
        write(&source.join("system.reg"), "");
        assert!(matches!(check(&source), crate::Error::NotFound { .. }));
        assert!(!layout.bottle_dir("imported").exists());
    }

    #[test]
    fn import_checks_the_name_and_destination() {
        let (dir, layout) = home();
        let source = dir.path().join("source");
        fake_prefix(&source);

        let err = import(&layout, &source, "bad name", "11.0", ImportMode::Move).unwrap_err();
        assert!(matches!(err, crate::Error::InvalidName { .. }), "{err:?}");

        mkdirs(&layout.bottle_dir("taken"));
        let err = import(&layout, &source, "taken", "11.0", ImportMode::Move).unwrap_err();
        assert!(matches!(err, crate::Error::AlreadyExists { .. }), "{err:?}");
        assert!(
            source.join("system.reg").is_file(),
            "the source is untouched"
        );
    }

    #[test]
    fn import_refuses_a_prefix_that_contains_the_bottles() {
        let (_dir, layout) = home();
        fake_prefix(layout.root());
        let err = import(&layout, layout.root(), "self", "11.0", ImportMode::Clone).unwrap_err();
        assert!(
            matches!(&err, crate::Error::Io { source, .. } if source.kind() == io::ErrorKind::InvalidInput),
            "{err:?}"
        );
        assert!(!layout.bottle_dir("self").exists());
    }

    // ----- with a fake Wine -----

    /// A Wine runtime made of shell scripts: `wine` and `wineserver` append
    /// their arguments and Wine variables to `calls`; `wine regedit` saves
    /// the imported file as `imported_reg`.
    struct FakeWine {
        runtime: WineRuntime,
        calls: PathBuf,
        imported_reg: PathBuf,
    }

    const BOOT_OK: &str =
        r#"/bin/mkdir -p "$WINEPREFIX/drive_c/windows/system32" && : > "$WINEPREFIX/system.reg""#;
    const BOOT_HANGS: &str = "exec /bin/sleep 30";
    const BOOT_FAILS: &str = "exit 3";

    const WINE_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "wine $* | WINEPREFIX=$WINEPREFIX WINEDEBUG=$WINEDEBUG WINEDLLOVERRIDES=$WINEDLLOVERRIDES" >> '@CALLS@'
case "$1" in
    wineboot) @WINEBOOT@ ;;
    regedit) name=${3##*\\}; /bin/cp "$WINEPREFIX/drive_c/windows/temp/$name" '@IMPORTED@' ;;
esac
"#;

    const WINESERVER_SCRIPT: &str = r#"#!/bin/sh
printf '%s\n' "wineserver $* | WINEPREFIX=$WINEPREFIX" >> '@CALLS@'
"#;

    fn fake_wine(dir: &Path, wineboot: &str) -> FakeWine {
        let root = dir.join("wine");
        let calls = dir.join("calls.log");
        let imported_reg = dir.join("imported.reg");
        let fill = |script: &str| {
            script
                .replace("@CALLS@", calls.to_str().unwrap())
                .replace("@IMPORTED@", imported_reg.to_str().unwrap())
                .replace("@WINEBOOT@", wineboot)
        };
        for (name, script) in [("wine", WINE_SCRIPT), ("wineserver", WINESERVER_SCRIPT)] {
            let path = root.join("bin").join(name);
            write(&path, &fill(script));
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        FakeWine {
            runtime: WineRuntime {
                root,
                version: "11.0-fake".to_owned(),
                features: Vec::new(),
            },
            calls,
            imported_reg,
        }
    }

    impl FakeWine {
        fn calls(&self) -> Vec<String> {
            std::fs::read_to_string(&self.calls)
                .unwrap_or_default()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    #[test]
    fn create_initializes_a_prefix() {
        let (dir, layout) = home();
        let wine = fake_wine(dir.path(), BOOT_OK);
        let options = CreateOptions {
            windows_version: WindowsVersion::Win7,
            graphics: GraphicsConfig {
                backend: BackendChoice::Fixed(Backend::Dxmt),
                ..GraphicsConfig::default()
            },
            performance: PerformanceConfig {
                retina: true,
                ..PerformanceConfig::default()
            },
        };

        let bottle = create(&layout, "steam", &wine.runtime, &options).unwrap();

        let prefix = layout.bottle_dir("steam");
        assert_eq!(bottle.path, prefix);
        assert!(bottle.is_initialized());
        assert_eq!(bottle.config.name, "steam");
        assert_eq!(bottle.config.wine, "11.0-fake");
        assert_eq!(bottle.config.windows_version, WindowsVersion::Win7);
        assert_eq!(bottle.config.graphics, options.graphics);
        assert_eq!(bottle.state.prefix_wine.as_deref(), Some("11.0-fake"));
        assert_eq!(
            Bottle::open(&prefix).unwrap(),
            bottle,
            "both files are on disk"
        );

        let calls = wine.calls();
        assert_eq!(calls.len(), 4, "{calls:#?}");
        let p = prefix.display();
        assert_eq!(
            calls[0],
            format!(
                "wine wineboot -u | WINEPREFIX={p} WINEDEBUG=-all WINEDLLOVERRIDES=mscoree,mshtml,winemenubuilder.exe=d"
            )
        );
        assert_eq!(calls[1], format!("wineserver --wait | WINEPREFIX={p}"));
        assert!(
            calls[2].starts_with(r"wine regedit /S C:\windows\temp\uncork-"),
            "{}",
            calls[2]
        );
        assert!(
            calls[2].ends_with(&format!(
                ".reg | WINEPREFIX={p} WINEDEBUG=-all WINEDLLOVERRIDES=winemenubuilder.exe=d"
            )),
            "{}",
            calls[2]
        );
        assert!(calls[3].starts_with("wine --version"), "{}", calls[3]);

        let imported = std::fs::read_to_string(&wine.imported_reg).unwrap();
        assert_eq!(
            imported,
            crate::registry::render(&default_registry(&bottle.config))
        );
        assert!(imported.contains(r#""RetinaMode"="y""#), "{imported}");
        assert!(imported.contains(r#""Version"="win7""#), "{imported}");
        let temp = std::fs::read_dir(prefix.join("drive_c/windows/temp"))
            .unwrap()
            .count();
        assert_eq!(temp, 0, "the .reg file is removed");
        assert!(layout.logs_dir().join("bottle-steam-create.log").is_file());
    }

    #[test]
    fn create_gives_up_on_a_hung_wineboot() {
        let (dir, layout) = home();
        let wine = fake_wine(dir.path(), BOOT_HANGS);
        let started = Instant::now();

        let err = create_with_timeout(
            &layout,
            "hang",
            &wine.runtime,
            &CreateOptions::default(),
            Duration::from_millis(300),
        )
        .unwrap_err();

        assert!(
            started.elapsed() < Duration::from_secs(10),
            "{:?}",
            started.elapsed()
        );
        let crate::Error::Command {
            program,
            status,
            log_hint,
        } = &err
        else {
            panic!("expected a Command error, got {err:?}");
        };
        assert_eq!(program, "wineboot");
        assert!(
            status.contains("did not finish within 300ms") && status.contains("59595"),
            "{status}"
        );
        let log = layout.logs_dir().join("bottle-hang-create.log");
        assert_eq!(log_hint, &format!(" (log: {})", log.display()));
        let calls = wine.calls();
        assert_eq!(
            calls.last().map(String::as_str),
            Some(&*format!(
                "wineserver --kill | WINEPREFIX={}",
                layout.bottle_dir("hang").display()
            ))
        );
        assert!(
            layout.bottle_dir("hang").join(CONFIG_FILE).is_file(),
            "left in place for inspection"
        );
        assert!(!layout.bottle_dir("hang").join(STATE_FILE).exists());
    }

    #[test]
    fn create_reports_a_failing_wineboot() {
        let (dir, layout) = home();
        let wine = fake_wine(dir.path(), BOOT_FAILS);
        let err = create(&layout, "broken", &wine.runtime, &CreateOptions::default()).unwrap_err();
        assert!(
            matches!(&err, crate::Error::Command { program, status, .. } if program == "wineboot" && status == "exited with status 3"),
            "{err:?}"
        );
        assert!(
            err.to_string().contains("bottle-broken-create.log"),
            "{err}"
        );
        assert_eq!(wine.calls().len(), 1, "nothing runs after wineboot fails");
    }

    #[test]
    fn apply_registry_imports_and_cleans_up() {
        let (dir, layout) = home();
        let wine = fake_wine(dir.path(), BOOT_OK);
        let path = layout.bottle_dir("x");
        fake_prefix(&path);
        let bottle = Bottle {
            path: path.clone(),
            config: config("x"),
            state: BottleState::default(),
        };
        let keys = default_registry(&bottle.config);

        apply_registry(&bottle, &wine.runtime, &keys).unwrap();
        apply_registry(&bottle, &wine.runtime, &keys).unwrap();

        let calls = wine.calls();
        assert_eq!(calls.len(), 2);
        assert_ne!(calls[0], calls[1], "every import uses a fresh file name");
        assert_eq!(
            std::fs::read_to_string(&wine.imported_reg).unwrap(),
            crate::registry::render(&keys)
        );
        assert_eq!(
            std::fs::read_dir(path.join("drive_c/windows/temp"))
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn open_reads_both_files() {
        let (_dir, layout) = home();
        let mut bottle = Bottle {
            path: layout.bottle_dir("steam"),
            config: config("steam"),
            state: BottleState::default(),
        };
        mkdirs(&bottle.path);
        bottle.save_config().unwrap();
        assert_eq!(
            Bottle::open(&bottle.path).unwrap(),
            bottle,
            "a missing state is the default"
        );

        bottle.state.prefix_wine = Some("11.0".to_owned());
        bottle.config.performance.hud = true;
        bottle.save_config().unwrap();
        bottle.save_state().unwrap();
        assert_eq!(Bottle::open_named(&layout, "steam").unwrap(), bottle);
    }

    #[test]
    fn open_reports_missing_and_invalid_configs() {
        let (_dir, layout) = home();
        let path = layout.bottle_dir("x");
        fake_prefix(&path);
        assert!(matches!(
            Bottle::open(&path),
            Err(crate::Error::NotFound { what: "bottle", .. })
        ));
        assert!(matches!(
            Bottle::open_named(&layout, "x"),
            Err(crate::Error::NotFound { .. })
        ));

        write(&path.join(CONFIG_FILE), "schema = 1\nname = \"x\"\n");
        assert!(matches!(
            Bottle::open(&path),
            Err(crate::Error::Config { .. })
        ));
    }

    #[test]
    fn list_returns_readable_bottles_by_name() {
        let (_dir, layout) = home();
        for name in ["zeta", "alpha", "Mid"] {
            let bottle = Bottle {
                path: layout.bottle_dir(name),
                config: config(name),
                state: BottleState::default(),
            };
            mkdirs(&bottle.path);
            bottle.save_config().unwrap();
        }
        write(&layout.bottle_dir("broken").join(CONFIG_FILE), "not toml");
        write(&layout.bottle_dir("bad name").join(CONFIG_FILE), "");

        let names: Vec<String> = list(&layout)
            .unwrap()
            .into_iter()
            .map(|b| b.config.name)
            .collect();
        assert_eq!(names, ["Mid", "alpha", "zeta"]);
    }

    #[test]
    fn import_clones_a_prefix() {
        let (dir, layout) = home();
        let source = dir.path().join("CrossOver Bottle");
        fake_prefix(&source);

        let bottle = import(&layout, &source, "imported", "11.0", ImportMode::Clone).unwrap();

        let dest = layout.bottle_dir("imported");
        assert_eq!(bottle.path, dest);
        assert_eq!(
            std::fs::read_to_string(dest.join("drive_c/windows/system32/kernel32.dll")).unwrap(),
            "kernel32"
        );
        assert_eq!(
            std::fs::read_link(dest.join("dosdevices/c:")).unwrap(),
            Path::new("../drive_c")
        );
        assert!(bottle.is_initialized());
        assert_eq!(
            bottle.config.imported_from,
            Some(source.canonicalize().unwrap())
        );
        assert_eq!(bottle.config.wine, "11.0");
        assert_eq!(bottle.state, BottleState::default());
        assert!(
            source.join("system.reg").is_file(),
            "cloning leaves the original"
        );
        assert!(!source.join(CONFIG_FILE).exists());
    }

    #[test]
    fn import_clears_a_steam_pid_left_in_the_copy() {
        let (dir, layout) = home();
        let source = dir.path().join("CrossOver Steam");
        fake_prefix(&source);
        let running = "WINE REGISTRY Version 2\n\n[Software\\\\Valve\\\\Steam\\\\ActiveProcess] 1\n\"pid\"=dword:00000274\n";
        write(&source.join("user.reg"), running);

        let bottle = import(&layout, &source, "cx", "11.0", ImportMode::Clone).unwrap();

        assert_eq!(
            std::fs::read_to_string(bottle.path.join("user.reg")).unwrap(),
            running.replace("dword:00000274", "dword:00000000")
        );
        assert_eq!(
            std::fs::read_to_string(source.join("user.reg")).unwrap(),
            running,
            "the original is untouched"
        );
    }

    #[test]
    fn import_keeps_the_prefix_windows_user() {
        let (dir, layout) = home();
        let source = dir.path().join("CrossOver Steam");
        fake_prefix(&source);
        std::fs::create_dir_all(source.join("drive_c/users/Public")).unwrap();
        std::fs::create_dir_all(source.join("drive_c/users/crossover")).unwrap();

        let bottle = import(&layout, &source, "cx", "11.0", ImportMode::Clone).unwrap();

        assert_eq!(
            bottle.config.env.get("USER").map(String::as_str),
            Some("crossover")
        );
        assert_eq!(
            bottle.config.env.get("LOGNAME").map(String::as_str),
            Some("crossover")
        );
    }

    #[test]
    fn import_leaves_user_alone_when_ambiguous() {
        let (dir, layout) = home();
        let source = dir.path().join("two-users");
        fake_prefix(&source);
        std::fs::create_dir_all(source.join("drive_c/users/alice")).unwrap();
        std::fs::create_dir_all(source.join("drive_c/users/bob")).unwrap();

        let bottle = import(&layout, &source, "two", "11.0", ImportMode::Clone).unwrap();

        assert!(!bottle.config.env.contains_key("USER"));
    }

    #[test]
    fn import_moves_a_prefix_and_keeps_its_state() {
        let (dir, layout) = home();
        let source = dir.path().join("old-uncork-bottle");
        fake_prefix(&source);
        write(&source.join(CONFIG_FILE), "stale");
        write(
            &source.join(STATE_FILE),
            "schema = 1\nprefix_wine = \"10.0\"\n",
        );

        let bottle = import(&layout, &source, "moved", "11.0", ImportMode::Move).unwrap();

        assert!(!source.exists());
        assert_eq!(bottle.config.name, "moved");
        assert_eq!(bottle.state.prefix_wine.as_deref(), Some("10.0"));
        assert_eq!(Bottle::open_named(&layout, "moved").unwrap(), bottle);
    }

    #[test]
    fn delete_stops_the_wineserver_first() {
        let (dir, layout) = home();
        let wine = fake_wine(dir.path(), BOOT_OK);
        fake_prefix(&layout.bottle_dir("old"));
        delete(&layout, "old", Some(&wine.runtime)).unwrap();
        assert_eq!(
            wine.calls(),
            [format!(
                "wineserver --kill | WINEPREFIX={}",
                layout.bottle_dir("old").display()
            )]
        );
        assert!(!layout.bottle_dir("old").exists());
    }
}
