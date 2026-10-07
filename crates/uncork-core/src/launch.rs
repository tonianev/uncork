//! Launch planning: turn "run this program in that bottle" into one exact
//! [`CommandSpec`] plus the backend files to put in place, and execute it.
//!
//! Planning ([`plan`]) is pure apart from reading the bottle and component
//! directories, so `uncork run --dry-run` and the unit tests see exactly
//! what would run.
//!
//! # Environment
//!
//! Wine processes get a constructed environment, not the user's shell
//! environment ([`CommandSpec::env_clear`] semantics via [`ALLOWED_ENV`]):
//! a stray `DYLD_*`, `WINE*` or `MVK_*` variable from a shell profile is a
//! classic cause of "works on my Mac". Layers, lowest precedence first:
//!
//! 1. [`ALLOWED_ENV`] copied from the parent, `PATH=/usr/bin:/bin:/usr/sbin:/sbin`.
//! 2. Wine basics: `WINEPREFIX`, `WINEDEBUG` (`-all`, or the debug channels).
//! 3. Bottle performance: `WINEMSYNC=1` when enabled and the runtime has
//!    `msync`; `ROSETTA_ADVERTISE_AVX=1` when `avx`.
//! 4. Backend activation env ([`crate::graphics::activation`]).
//! 5. Bottle `env`, then profile `env`, then command-line `--env`.
//!
//! `WINEDLLOVERRIDES` is built the same way from: `winemenubuilder.exe=d`,
//! the activation's overrides, bottle `dll_overrides`, profile
//! `dll_overrides`; later layers win per DLL.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::bottle::Bottle;
use crate::graphics::{Activation, Backend, BackendChoice};
use crate::paths::Layout;
use crate::process::CommandSpec;
use crate::profile::GameProfile;
use crate::wine::WineRuntime;

/// Variables passed through from the parent environment to Wine.
pub const ALLOWED_ENV: &[&str] = &[
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "LC_MESSAGES",
    "__CF_USER_TEXT_ENCODING",
];

/// `PATH` given to Wine processes.
pub const WINE_PATH: &str = "/usr/bin:/bin:/usr/sbin:/sbin";

/// What to start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Target {
    /// A Windows executable, as a macOS path (inside or outside the prefix).
    Exe {
        /// The `.exe`.
        path: PathBuf,
        /// Arguments.
        args: Vec<String>,
    },
    /// A program Wine resolves itself (`winecfg`, `regedit`, `C:\...\x.exe`).
    WineProgram {
        /// Name or Windows path.
        name: String,
        /// Arguments.
        args: Vec<String>,
    },
}

/// Per-launch options from the command line.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LaunchOptions {
    /// Backend override.
    pub backend: Option<BackendChoice>,
    /// Force the Metal HUD on.
    pub hud: bool,
    /// Force MetalFX on.
    pub metalfx: bool,
    /// Force Retina mode on (applied to the prefix registry before launch).
    pub retina: bool,
    /// `WINEDEBUG` channels; also turns backend logs on.
    pub wine_debug: Option<String>,
    /// `KEY=VALUE` pairs from `--env`, already split.
    pub env: BTreeMap<String, String>,
}

/// A planned launch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LaunchPlan {
    /// The exact command.
    pub command: CommandSpec,
    /// Backend files and settings.
    pub activation: Activation,
    /// Why this backend was chosen.
    pub backend_reason: String,
    /// Where output goes: `logs/<bottle>-<program stem>-<unix secs>.log`.
    pub log: PathBuf,
    /// Warnings worth printing (anti-cheat detected, 32-bit game on a
    /// runtime without `wow64`, D3D12 without D3DMetal, ...).
    pub warnings: Vec<String>,
}

/// Inputs for [`plan`].
#[derive(Debug, Clone, Copy)]
pub struct PlanContext<'a> {
    /// Data layout.
    pub layout: &'a Layout,
    /// Bottle to run in.
    pub bottle: &'a Bottle,
    /// Its Wine runtime.
    pub wine: &'a WineRuntime,
    /// Installed components (to find backends).
    pub components: &'a [crate::component::InstalledComponent],
    /// Matching game profile, if any.
    pub profile: Option<&'a GameProfile>,
}

/// Plan a launch.
///
/// Backend selection: `options.backend` if given; else the profile's
/// `graphics.backend`/`fallbacks`; else the bottle's `graphics.backend`;
/// `Auto` scans the executable with [`uncork_pe::scan_game`] (for
/// [`Target::Exe`]; `WineProgram` targets use WineD3D) and calls
/// [`crate::graphics::recommend`]. A fixed backend that is not installed or
/// cannot run the program is an [`crate::Error::Unsupported`] error naming
/// the install command, never a silent fallback.
///
/// The command is `<wine> <exe path> <profile args> <args>` with `cwd` set
/// to the executable's directory, `log` set, and `env` built as described
/// in the module docs.
///
/// # Errors
/// PE scan, component lookup, or backend errors.
pub fn plan(ctx: PlanContext<'_>, target: &Target, options: &LaunchOptions) -> crate::Result<LaunchPlan> {
    let _ = (ctx, target, options);
    todo!()
}

/// Apply the plan's backend copies and any registry the launch needs, then
/// spawn the command (not waiting). Returns the child.
///
/// # Errors
/// I/O, registry or spawn errors.
pub fn execute(plan: &LaunchPlan, bottle: &mut Bottle, wine: &WineRuntime) -> crate::Result<std::process::Child> {
    let _ = (plan, bottle, wine);
    todo!()
}

/// Build the base environment (layers 1–3 of the module docs) for any Wine
/// command in `bottle`; used by launches, Steam and bottle tools so they all
/// share one wineserver configuration (msync is per wineserver: mixing
/// `WINEMSYNC` values in one prefix makes the second client exit).
#[must_use]
pub fn base_env(bottle: &Bottle, wine: &WineRuntime, wine_debug: Option<&str>) -> BTreeMap<String, String> {
    let _ = (bottle, wine, wine_debug);
    todo!()
}

/// The backend a launch of `target` would use, without building the whole
/// plan (for `uncork inspect` and `uncork play --dry-run`).
///
/// # Errors
/// As [`plan`].
pub fn choose_backend(ctx: PlanContext<'_>, target: &Target, options: &LaunchOptions) -> crate::Result<(Backend, String)> {
    let _ = (ctx, target, options);
    todo!()
}
