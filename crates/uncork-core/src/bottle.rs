//! Bottles: Wine prefixes with Uncork settings.
//!
//! A bottle is a directory that is both the `WINEPREFIX` and the home of
//! `uncork.toml` ([`BottleConfig`], user-editable) and `uncork-state.toml`
//! ([`BottleState`], machine-written). Keeping Uncork's files at the prefix
//! root (like CrossOver's `cxbottle.conf`) makes import and export a plain
//! directory copy.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::graphics::{Backend, BackendChoice};
use crate::paths::Layout;
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
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
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
        let _ = path;
        todo!()
    }

    /// Open `bottles/<name>`.
    ///
    /// # Errors
    /// As [`Bottle::open`]; `NotFound` hints at `uncork bottle list`.
    pub fn open_named(layout: &Layout, name: &str) -> crate::Result<Bottle> {
        let _ = (layout, name);
        todo!()
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
        todo!()
    }

    /// Write `uncork-state.toml` atomically.
    ///
    /// # Errors
    /// [`crate::Error::Io`].
    pub fn save_state(&self) -> crate::Result<()> {
        todo!()
    }
}

/// Bottle names: 1–64 chars of `[A-Za-z0-9._-]`, not starting with `.` or
/// `-`, not `.`/`..`.
///
/// # Errors
/// [`crate::Error::InvalidName`].
pub fn validate_name(name: &str) -> crate::Result<()> {
    let _ = name;
    todo!()
}

/// Every bottle under `bottles/`, sorted by name. Directories without
/// `uncork.toml` are skipped; unreadable configs are logged and skipped.
///
/// # Errors
/// [`crate::Error::Io`] if `bottles/` exists but cannot be listed.
pub fn list(layout: &Layout) -> crate::Result<Vec<Bottle>> {
    let _ = layout;
    todo!()
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
    let _ = (layout, name, wine, options);
    todo!()
}

/// How long `wineboot` may take before [`create`] gives up.
pub const BOOT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

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
    let _ = config;
    todo!()
}

/// Apply [`default_registry`] to an existing bottle (used by `bottle set`
/// and before a launch when `retina` changes).
///
/// # Errors
/// I/O or command errors.
pub fn apply_registry(bottle: &Bottle, wine: &WineRuntime, keys: &[crate::registry::RegKey]) -> crate::Result<()> {
    let _ = (bottle, wine, keys);
    todo!()
}

/// Delete a bottle directory after killing its wineserver.
///
/// # Errors
/// [`crate::Error::NotFound`] or [`crate::Error::Io`].
pub fn delete(layout: &Layout, name: &str, wine: Option<&WineRuntime>) -> crate::Result<()> {
    let _ = (layout, name, wine);
    todo!()
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
/// prefix is updated by Wine on first launch.
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
    let _ = (layout, source, name, wine_version, mode);
    todo!()
}
