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
//!    `msync`; `ROSETTA_ADVERTISE_AVX=1` when `avx`; `MVK_CONFIG_LOG_LEVEL=1`
//!    (MoltenVK errors only; it otherwise prints device info on every start).
//! 4. Backend activation env ([`crate::graphics::activation`]).
//! 5. Bottle `env`, then profile `env`, then command-line `--env`.
//!
//! `WINEDLLOVERRIDES` is built the same way from: `winemenubuilder.exe=d`,
//! the activation's overrides, bottle `dll_overrides`, profile
//! `dll_overrides`; later layers win per DLL (names compared ignoring ASCII
//! case). A `WINEDLLOVERRIDES` value given in one of the layer-5 `env` maps
//! is not passed through verbatim: it is parsed and merged on top, so it
//! can change single DLLs without dropping the backend's overrides.
//!
//! Layers 1–3 ([`base_env`]) are the same for every process of a bottle,
//! with one exception: a profile's `performance.avx` replaces the bottle's
//! `ROSETTA_ADVERTISE_AVX` for that game (it is read per process).
//! `WINEMSYNC` is never changed per game: every client of a wineserver must
//! agree on it, and a mismatched client exits.
//!
//! # Backend selection
//!
//! First match wins:
//!
//! 1. `--backend <b>` ([`LaunchOptions::backend`] = `Fixed`).
//! 2. The profile's `graphics.backend` and `fallbacks` (also used by an
//!    explicit `--backend auto`, which only overrides the bottle's setting).
//! 3. The bottle's `graphics.backend` when it is fixed.
//! 4. Automatic: [`crate::graphics::recommend`] from the scanned or
//!    profile-declared bitness and API.
//!
//! A fixed backend (1 or 3) that is not usable is an error naming the fix;
//! a profile's preference that is not usable falls back silently in the
//! choice but is reported in [`LaunchPlan::warnings`].
//!
//! # The main display
//!
//! [`PlanContext::display`] is the main display ([`crate::display`]), or
//! `None` when it could not be read. It sets the frame cap
//! ([`LaunchPlan::frame_cap`]), resolves `{display.*}` placeholders in a
//! profile's INI values ([`resolve_ini_value`]), and its signature is
//! recorded as the bottle's session display when a launch starts a new
//! Wine session ([`execute`]).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;
use uncork_pe::{Bitness, GameScan, GraphicsApi};

use crate::Error;
use crate::bottle::Bottle;
use crate::component::{InstalledComponent, compare_versions};
use crate::display::Display;
use crate::graphics::{Activation, Availability, Backend, BackendChoice, BackendOptions};
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

/// Runtime feature: Mach-semaphore synchronization.
const FEATURE_MSYNC: &str = "msync";
/// Runtime feature: runs 32-bit programs (new-style `WoW64`).
const FEATURE_WOW64: &str = "wow64";
/// Runtime feature: honors [`LAA_VAR`].
const FEATURE_LARGE_ADDRESS_AWARE: &str = "large-address-aware";

/// The variable CrossOver-derived Wine reads to make a 32-bit program
/// large-address-aware.
const LAA_VAR: &str = "WINE_LARGE_ADDRESS_AWARE";
/// Wine's DLL load-order variable.
pub(crate) const OVERRIDES_VAR: &str = "WINEDLLOVERRIDES";
/// Rosetta's switch to report AVX in CPUID.
const AVX_VAR: &str = "ROSETTA_ADVERTISE_AVX";

/// The override every Wine process gets: no macOS menu entries or launchers
/// for programs Wine installs.
pub(crate) const NO_MENU_BUILDER: (&str, &str) = ("winemenubuilder.exe", "d");

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
    /// Ask for Retina mode. It is a bottle-wide registry setting that a
    /// launch does not change; [`plan`] warns when the bottle's differs
    /// (`uncork bottle set <bottle> performance.retina=true` changes it).
    pub retina: bool,
    /// `WINEDEBUG` channels; also turns backend logs on. Blank means none.
    pub wine_debug: Option<String>,
    /// `KEY=VALUE` pairs from `--env`, already split.
    pub env: BTreeMap<String, String>,
    /// Force the Game Mode bundle on or off for this launch; `None` keeps the
    /// bottle's `performance.game_mode` (see [`crate::gamemode`]).
    pub game_mode: Option<bool>,
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
    /// Start through a Game Mode app bundle instead of directly
    /// (experimental; [`crate::gamemode`]).
    pub game_mode: Option<GameModeLaunch>,
    /// The frame-rate cap the backend is asked for; `None` when the backend
    /// has no cap Uncork sets (every backend but DXMT).
    pub frame_cap: Option<FrameCap>,
    /// The main display's signature ([`Display::signature`]) at planning
    /// time; recorded as the bottle's session display when this launch
    /// starts a new Wine session ([`execute`]).
    pub display: Option<String>,
}

/// The frame-rate cap of a launch (DXMT:
/// `DXMT_CONFIG=d3d11.preferredMaxFrameRate=<fps>`). DXMT paces frames
/// itself; without a cap it uses the refresh rate it reads when the game
/// creates its swapchain, and after the game leaves exclusive full screen
/// it runs uncapped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FrameCap {
    /// Frames per second; `None` is uncapped (`max_fps = 0`).
    pub fps: Option<u32>,
    /// Where the value comes from, for `--dry-run`: `performance.max_fps in
    /// profile <id>`, `performance.max_fps in bottle <name>`, `the main
    /// display's refresh rate (<display>)` ([`Display::describe`]), `the
    /// default (the main display's refresh rate is unknown)` or
    /// `d3d11.preferredMaxFrameRate in DXMT_CONFIG from the bottle, the
    /// profile or --env`.
    pub source: String,
    /// A `DXMT_CONFIG` from the bottle's or the profile's `env` or from
    /// `--env` sets `d3d11.preferredMaxFrameRate` itself: `fps` is its
    /// value. (One that sets only other options keeps Uncork's cap, which
    /// is appended to it.)
    pub overridden: bool,
}

/// The frame cap when nothing sets one and the main display's refresh rate
/// is unknown: it divides every common refresh rate (60, 120, 240 Hz).
pub const DEFAULT_MAX_FPS: u32 = 60;

/// DXMT's configuration variable.
const DXMT_CONFIG: &str = "DXMT_CONFIG";

/// Where and as what a plan is wrapped for Game Mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameModeLaunch {
    /// Data root whose `apps/` holds the bundle.
    pub root: PathBuf,
    /// Bundle id part (`apps/<id>.app`, `dev.uncork.game.<id>`).
    pub id: String,
    /// Display name.
    pub name: String,
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
    /// The main display ([`crate::display::probe_main`]); `None` when
    /// unknown.
    pub display: Option<&'a Display>,
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
/// Details:
///
/// - The executable path is made absolute (without resolving symlinks), so
///   a relative path still names the same file from the new working
///   directory. Wine maps the macOS path itself.
/// - A profile's `exe.bitness` and `exe.api` override what the scan finds;
///   the executable is still scanned for warnings when it exists. A missing
///   executable is [`crate::Error::NotFound`] unless the profile declares
///   its bitness.
/// - A `WineProgram` target runs on WineD3D unless `--backend` fixes another
///   backend (the bottle's fixed backend is meant for games, not for
///   `winecfg`). It has no working directory.
/// - The effective performance settings are the bottle's, overridden by the
///   profile's `performance` values that are set, and forced on by `--hud`,
///   `--metalfx` and `--retina`. Retina mode and msync are bottle-wide; a
///   request that differs from the bottle's setting becomes a warning.
/// - Frame cap ([`LaunchPlan::frame_cap`], DXMT only): the profile's
///   `performance.max_fps`, else the bottle's, else the main display's
///   refresh rate rounded to whole Hz, else [`DEFAULT_MAX_FPS`]; `0` is
///   uncapped. An explicit value that does not divide the display's refresh
///   rate gets a warning (DXMT needs one that does). A `DXMT_CONFIG` from
///   the bottle, the profile or `--env` keeps the cap appended to its own
///   options, unless it sets `d3d11.preferredMaxFrameRate` itself, which
///   then is the cap ([`FrameCap::overridden`]).
/// - DXMT's MetalFX swapchain (`--metalfx`, `performance.metalfx`) is left
///   off, with a warning, in a bottle with Retina mode on: it would upscale
///   to twice the native resolution.
/// - A profile INI value with a `{display.*}` placeholder that cannot be
///   resolved (the main display is unknown) gets a warning; the launch
///   leaves that key alone ([`apply_profile_ini`]).
/// - The bottle's registry having a Retina mode and DPI pair Uncork never
///   writes ([`crate::bottle::DisplayRegistry::disagrees`], read from
///   `user.reg`) gets a warning naming `uncork bottle set`.
///
/// # Errors
/// PE scan, component lookup, or backend errors.
pub fn plan(
    ctx: PlanContext<'_>,
    target: &Target,
    options: &LaunchOptions,
) -> crate::Result<LaunchPlan> {
    let Selection {
        backend,
        component,
        reason,
        program,
        mut warnings,
    } = select(ctx, target, options)?;
    let performance = Performance::effective(ctx.bottle, ctx.profile, options);
    let metalfx_over_retina =
        backend == Backend::Dxmt && performance.metalfx && ctx.bottle.config.performance.retina;
    let cap = choose_frame_cap(ctx, &performance);
    let activation = crate::graphics::activation(
        backend,
        component,
        ctx.wine,
        ctx.bottle,
        BackendOptions {
            metalfx: performance.metalfx && !metalfx_over_retina,
            hud: performance.hud,
            debug_logs: options
                .wine_debug
                .as_deref()
                .is_some_and(|channels| !channels.trim().is_empty()),
            max_fps: cap.fps,
        },
    )?;
    let mut env = launch_env(ctx, options, &activation, performance.avx);
    warnings.extend(program_warnings(ctx, &program, backend, &env));
    warnings.extend(setting_warnings(ctx, &performance));
    if metalfx_over_retina {
        let bottle = &ctx.bottle.config.name;
        warnings.push(format!(
            "MetalFX upscaling stays off for this launch: bottle {bottle} has Retina mode on, and DXMT's MetalFX swapchain would then render at twice the native resolution; turn one of them off (`uncork bottle set {bottle} performance.retina=false`, or leave out --metalfx and performance.metalfx)"
        ));
    }
    let frame_cap = (backend == Backend::Dxmt).then(|| {
        let cap = match keep_frame_cap(&mut env, activation.env.get(DXMT_CONFIG)) {
            Some(fps) => FrameCap {
                fps,
                source: format!(
                    "{FRAME_RATE_KEY} in DXMT_CONFIG from the bottle, the profile or --env"
                ),
                overridden: true,
            },
            None => cap,
        };
        let explicit = cap.overridden || performance.max_fps.is_some();
        if let Some(warning) = cap_warning(ctx, &cap, explicit) {
            warnings.push(warning);
        }
        cap
    });
    warnings.extend(display_warnings(ctx));

    let profile_args = ctx
        .profile
        .map_or(&[][..], |profile| profile.launch.args.as_slice());
    let (first_arg, target_args, cwd, stem) = match target {
        Target::Exe { path, args } => {
            let exe =
                std::path::absolute(path).map_err(|err| Error::io("cannot resolve", path, err))?;
            let cwd = exe.parent().map(Path::to_path_buf);
            let stem = exe
                .file_stem()
                .map_or_else(String::new, |stem| stem.to_string_lossy().into_owned());
            (exe.into_os_string(), args, cwd, stem)
        }
        Target::WineProgram { name, args } => (
            OsString::from(name),
            args,
            None,
            windows_stem(name).to_owned(),
        ),
    };
    let mut args = vec![first_arg];
    args.extend(profile_args.iter().map(OsString::from));
    args.extend(target_args.iter().map(OsString::from));

    let log = ctx.layout.logs_dir().join(format!(
        "{}-{}-{}.log",
        sanitize_file_part(&ctx.bottle.config.name),
        sanitize_file_part(&stem),
        crate::unix_now()
    ));
    let command = CommandSpec {
        program: ctx.wine.wine_bin(),
        args,
        env,
        env_clear: true,
        cwd,
        log: Some(log.clone()),
    };
    let game_mode = options
        .game_mode
        .unwrap_or(ctx.bottle.config.performance.game_mode)
        .then(|| GameModeLaunch {
            root: ctx.layout.root().to_path_buf(),
            id: ctx.profile.map_or_else(
                || game_mode_id(&ctx.bottle.config.name, &stem),
                |profile| profile.id.clone(),
            ),
            name: ctx
                .profile
                .map_or_else(|| stem.clone(), |profile| profile.name.clone()),
        });
    Ok(LaunchPlan {
        command,
        activation,
        backend_reason: reason,
        log,
        warnings,
        game_mode,
        frame_cap,
        display: ctx.display.map(Display::signature),
    })
}

/// The frame cap of the precedence documented on [`plan`].
fn choose_frame_cap(ctx: PlanContext<'_>, performance: &Performance) -> FrameCap {
    let (fps, source) = match &performance.max_fps {
        Some((fps, source)) => (*fps, source.clone()),
        None => match ctx
            .display
            .and_then(|display| display.refresh_rounded().map(|hz| (hz, display.describe())))
        {
            Some((hz, display)) => (hz, format!("the main display's refresh rate ({display})")),
            None => (
                DEFAULT_MAX_FPS,
                "the default (the main display's refresh rate is unknown)".to_owned(),
            ),
        },
    };
    FrameCap {
        fps: (fps > 0).then_some(fps),
        source,
        overridden: false,
    }
}

/// DXMT's frame-rate option.
const FRAME_RATE_KEY: &str = "d3d11.preferredMaxFrameRate";

/// Keep the frame cap when the bottle, the profile or `--env` set a
/// `DXMT_CONFIG` of their own, which replaced `ours` (the activation's,
/// `d3d11.preferredMaxFrameRate=<n>`) in `env`. DXMT reads `;`-separated
/// `key=value` entries, so `ours` is appended to theirs unless theirs sets
/// [`FRAME_RATE_KEY`] itself. Returns `Some` with the cap theirs sets
/// (`None` inside for 0 or a value DXMT cannot read, both uncapped; the
/// last entry wins, as in DXMT), `None` when Uncork's cap applies.
fn keep_frame_cap(
    env: &mut BTreeMap<String, String>,
    ours: Option<&String>,
) -> Option<Option<u32>> {
    let theirs = env.get_mut(DXMT_CONFIG)?;
    if Some(&*theirs) == ours {
        return None;
    }
    let set = theirs
        .split(';')
        .rev()
        .filter_map(|entry| entry.split_once('='))
        .find(|(key, _)| key.trim() == FRAME_RATE_KEY)
        .map(|(_, value)| {
            value
                .trim()
                .trim_matches('"')
                .parse::<u32>()
                .ok()
                .filter(|fps| *fps > 0)
        });
    if set.is_none()
        && let Some(ours) = ours
    {
        if !theirs.trim_end().is_empty() && !theirs.trim_end().ends_with(';') {
            theirs.push(';');
        }
        theirs.push_str(ours);
    }
    set
}

/// A warning when an `explicit` cap (from `max_fps` or `DXMT_CONFIG`)
/// does not divide the main display's refresh rate.
fn cap_warning(ctx: PlanContext<'_>, cap: &FrameCap, explicit: bool) -> Option<String> {
    if !explicit {
        return None;
    }
    let fps = cap.fps?;
    let hz = ctx.display?.refresh_rounded()?;
    (hz % fps != 0).then(|| {
        format!(
            "a frame cap of {fps} FPS ({}) does not divide the main display's {hz} Hz, and DXMT paces frames to the display, so frame times will be uneven; use a divisor of {hz} such as {}",
            cap.source,
            if hz % 60 == 0 { 60 } else { hz }
        )
    })
}

/// Warnings about the main display and the bottle's Retina mode and DPI.
fn display_warnings(ctx: PlanContext<'_>) -> Vec<String> {
    let mut warnings = Vec::new();
    let bottle = &ctx.bottle.config.name;
    if let Some(registry) = crate::bottle::DisplayRegistry::read(&ctx.bottle.path)
        && registry.disagrees()
    {
        warnings.push(format!(
            "Retina mode and DPI disagree in bottle {bottle} ({}), so games that are not DPI-aware see a screen of the wrong size (cropped, offset full screen; crashes); fix it with `uncork bottle set {bottle} performance.retina={}`",
            registry.describe(),
            ctx.bottle.config.performance.retina
        ));
    }
    if let Some(profile) = ctx.profile {
        let unresolved: Vec<&str> = profile
            .ini
            .iter()
            .filter(|ini| resolve_ini_value(&ini.value, ctx.display).is_none())
            .map(|ini| ini.key.as_str())
            .collect();
        if !unresolved.is_empty() {
            let unknown = match ctx.display {
                None => format!(
                    "the main display is unknown (`{} SPDisplaysDataType -json` gave no answer)",
                    crate::display::SYSTEM_PROFILER
                ),
                Some(display) => format!(
                    "the refresh rate of the main display ({}) is unknown",
                    display.signature()
                ),
            };
            warnings.push(format!(
                "{unknown}, so {} leaves {} as they are",
                profile.id,
                unresolved.join(", ")
            ));
        }
    }
    warnings
}

/// `value` with the placeholders of
/// [`crate::profile::INI_PLACEHOLDERS`] replaced from `display`:
/// `{display.width}` and `{display.height}` by its "looks like" size in
/// points, `{display.refresh}` by its refresh rate rounded to whole Hz.
/// `None` when `value` has a placeholder whose value is unknown (no
/// display, or no refresh rate). Other `{...}` text is kept as written
/// ([`GameProfile::problems`] rejects it in profiles).
#[must_use]
pub fn resolve_ini_value(value: &str, display: Option<&Display>) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        let token = &rest[start + 1..start + len];
        let replacement = match token {
            "display.width" => Some(display?.points.0.to_string()),
            "display.height" => Some(display?.points.1.to_string()),
            "display.refresh" => Some(display?.refresh_rounded()?.to_string()),
            _ => None,
        };
        out.push_str(&rest[..start]);
        match replacement {
            Some(text) => out.push_str(&text),
            None => out.push_str(&rest[start..=start + len]),
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// A bundle id for a program without a profile: `<bottle>-<stem>`, lowercase,
/// with anything but ASCII letters and digits turned into `-`.
fn game_mode_id(bottle: &str, stem: &str) -> String {
    format!("{bottle}-{stem}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned()
}

/// Apply the plan's backend copies and any registry the launch needs, then
/// spawn the command (not waiting). Returns the child. With
/// [`LaunchPlan::game_mode`] set, the command is wrapped in a Game Mode
/// bundle ([`crate::gamemode::prepare_bundle`], launcher = this executable)
/// and the child is `/usr/bin/open -n -W <bundle>`.
///
/// Creates `<bottle>/cache/dxmt` and `<bottle>/cache/dxvk` (the shader
/// caches the activation points at) and performs the activation's copies
/// with [`crate::graphics::apply_copies`], which records them in the bottle
/// state. The registry is not touched: Retina mode is a bottle setting
/// (`uncork bottle set`), and [`plan`] warns when a launch asks for another
/// value. When no wineserver runs for the bottle
/// ([`crate::steam::server_running`]), the program starts a new Wine
/// session, and [`LaunchPlan::display`] is recorded as its main display
/// ([`Bottle::record_session_display`]).
///
/// # Errors
/// I/O, registry or spawn errors.
pub fn execute(
    plan: &LaunchPlan,
    bottle: &mut Bottle,
    wine: &WineRuntime,
) -> crate::Result<std::process::Child> {
    prepare(plan, bottle)?;
    if !crate::steam::server_running(bottle, wine)? {
        bottle.record_session_display(plan.display.as_deref());
    }
    match &plan.game_mode {
        None => crate::process::spawn(&plan.command),
        Some(game_mode) => {
            // The bundle's launcher is this program; it `exec`s the plan.
            let launcher = std::env::current_exe()
                .map_err(|err| Error::io("cannot locate", "the uncork binary", err))?;
            let layout = Layout::at(&game_mode.root);
            let bundle = crate::gamemode::prepare_bundle(
                &layout,
                &game_mode.id,
                &game_mode.name,
                plan,
                &launcher,
            )?;
            crate::process::spawn(&crate::gamemode::open_command(&bundle))
        }
    }
}

/// Everything [`execute`] does before spawning: cache directories and the
/// activation's file copies. Shared with Steam's `-applaunch` mode, where
/// the game is started by the client instead.
pub(crate) fn prepare(plan: &LaunchPlan, bottle: &mut Bottle) -> crate::Result<()> {
    for cache in ["dxmt", "dxvk"] {
        let dir = bottle.path.join("cache").join(cache);
        std::fs::create_dir_all(&dir)
            .map_err(|err| Error::io("cannot create directory", &dir, err))?;
    }
    crate::graphics::apply_copies(&plan.activation, bottle)
}

/// Build the base environment (layers 1–3 of the module docs) for any Wine
/// command in `bottle`; used by launches, Steam and bottle tools so they all
/// share one wineserver configuration (msync is per wineserver: mixing
/// `WINEMSYNC` values in one prefix makes the second client exit).
///
/// Exactly: `WINEPREFIX=<bottle>`; `WINEDEBUG=<wine_debug>` (or `-all` when
/// `None` or blank); `WINEMSYNC=1` when the bottle's `performance.msync` is
/// on and the runtime has the `msync` feature; `ROSETTA_ADVERTISE_AVX=1`
/// when `performance.avx` is on; `MVK_CONFIG_LOG_LEVEL=1`. The bottle's own
/// `env` is not included; callers layer it on top.
#[must_use]
pub fn base_env(
    bottle: &Bottle,
    wine: &WineRuntime,
    wine_debug: Option<&str>,
) -> BTreeMap<String, String> {
    let performance = &bottle.config.performance;
    let debug = wine_debug
        .map(str::trim)
        .filter(|channels| !channels.is_empty())
        .unwrap_or("-all");
    let mut env = BTreeMap::new();
    let mut set = |key: &str, value: &str| {
        env.insert(key.to_owned(), value.to_owned());
    };
    set("WINEPREFIX", &bottle.path.to_string_lossy());
    set("WINEDEBUG", debug);
    if performance.msync && wine.has_feature(FEATURE_MSYNC) {
        set("WINEMSYNC", "1");
    }
    if performance.avx {
        set(AVX_VAR, "1");
    }
    set("MVK_CONFIG_LOG_LEVEL", "1");
    env
}

/// Run a command built by [`WineRuntime`] (`wineboot`, `regedit`,
/// `wineserver`, ...) with `bottle`'s shared environment: `env_clear`,
/// [`base_env`], then the bottle's own `env` (for example `USER=crossover`
/// in an imported CrossOver bottle), then the command's own variables, which
/// win. Every process in a prefix must agree on `WINEMSYNC`; a client whose
/// setting differs from the running wineserver's exits with status 1 and no
/// output (measured: the first command after a `wineboot` without msync).
#[must_use]
pub fn in_bottle(spec: CommandSpec, bottle: &Bottle, wine: &WineRuntime) -> CommandSpec {
    let mut env = base_env(bottle, wine, None);
    env.extend(
        bottle
            .config
            .env
            .iter()
            .filter(|(key, _)| key.as_str() != OVERRIDES_VAR)
            .map(|(key, value)| (key.clone(), value.clone())),
    );
    env.extend(spec.env);
    CommandSpec {
        env,
        env_clear: true,
        ..spec
    }
}

/// The backend a launch of `target` would use, without building the whole
/// plan (for `uncork inspect` and `uncork play --dry-run`).
///
/// # Errors
/// As [`plan`].
pub fn choose_backend(
    ctx: PlanContext<'_>,
    target: &Target,
    options: &LaunchOptions,
) -> crate::Result<(Backend, String)> {
    let selection = select(ctx, target, options)?;
    Ok((selection.backend, selection.reason))
}

/// Resolve a profile INI path (`%APPDATA%\...`, `%LOCALAPPDATA%\...`,
/// `%USERPROFILE%\...`, `%INSTALLDIR%\...`; `/` or `\` separators) to a
/// macOS path. `%APPDATA%` is `drive_c/users/<user>/AppData/Roaming`,
/// `%LOCALAPPDATA%` is `.../AppData/Local`, `%USERPROFILE%` is
/// `drive_c/users/<user>`, where `<user>` is the bottle's `env.USER` if set,
/// else the only directory in `drive_c/users` other than `Public`, else the
/// current `USER`. `%INSTALLDIR%` needs `install_dir`. Returns `None` for an
/// unknown base, a missing `install_dir`, or `..` components.
///
/// The `%BASE%` token is matched ignoring ASCII case. Below the base, each
/// component that exists on disk under a different case is used with its
/// on-disk spelling (Windows paths are case-insensitive; a case-sensitive
/// volume is not); components that do not exist yet are kept as written.
/// A path naming no file below the base (`%APPDATA%\`) is `None`.
#[must_use]
pub fn resolve_profile_path(
    bottle: &Bottle,
    install_dir: Option<&std::path::Path>,
    file: &str,
) -> Option<PathBuf> {
    let (base, rest) = split_ini_base(file)?;
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split(['\\', '/']) {
        match part {
            "" | "." => {}
            ".." => return None,
            name => parts.push(name),
        }
    }
    if parts.is_empty() {
        return None;
    }
    let (root, mut components) = match base {
        IniBase::InstallDir => (install_dir?.to_path_buf(), Vec::new()),
        IniBase::AppData | IniBase::LocalAppData | IniBase::UserProfile => {
            let user = windows_user(bottle)?;
            let mut components = vec!["users".to_owned(), user];
            match base {
                IniBase::AppData => components.extend(["AppData".into(), "Roaming".into()]),
                IniBase::LocalAppData => components.extend(["AppData".into(), "Local".into()]),
                IniBase::UserProfile | IniBase::InstallDir => {}
            }
            (bottle.drive_c(), components)
        }
    };
    components.extend(parts.into_iter().map(str::to_owned));
    Some(join_case_insensitive(&root, &components))
}

/// Apply a profile's `[[ini]]` edits (grouped per file, via
/// [`crate::ini::apply_to_file`]; files that do not exist yet are skipped).
/// Returns the files that changed.
///
/// Files are processed in the order they first appear in the profile, and
/// edits to one file are applied together in profile order. Values are
/// resolved with [`resolve_ini_value`] from `display`; an edit whose value
/// cannot be resolved (the main display is unknown) is skipped, and
/// [`plan`] warns about it.
///
/// # Errors
/// [`crate::Error::Io`] from writing, or [`crate::Error::Config`] when a
/// path cannot be resolved.
pub fn apply_profile_ini(
    profile: &GameProfile,
    bottle: &Bottle,
    install_dir: Option<&std::path::Path>,
    display: Option<&Display>,
) -> crate::Result<Vec<PathBuf>> {
    let mut groups: Vec<(PathBuf, Vec<crate::ini::IniSet>)> = Vec::new();
    for ini in &profile.ini {
        let path = resolve_profile_path(bottle, install_dir, &ini.file).ok_or_else(|| {
            let hint = if install_dir.is_none()
                && ini.file.to_ascii_uppercase().starts_with("%INSTALLDIR%")
            {
                " (%INSTALLDIR% needs the game's install directory)"
            } else {
                ""
            };
            Error::Config {
                what: "profile",
                path: PathBuf::from(&profile.id),
                message: format!(
                    "ini.file {:?} cannot be resolved in bottle {}{hint}",
                    ini.file, bottle.config.name
                ),
            }
        })?;
        let Some(value) = resolve_ini_value(&ini.value, display) else {
            tracing::debug!(
                "not setting {} in {}: the main display is unknown",
                ini.key,
                path.display()
            );
            continue;
        };
        let set = crate::ini::IniSet {
            section: ini.section.clone(),
            key: ini.key.clone(),
            value,
        };
        match groups.iter_mut().find(|(known, _)| *known == path) {
            Some((_, sets)) => sets.push(set),
            None => groups.push((path, vec![set])),
        }
    }
    let mut changed = Vec::new();
    for (path, sets) in groups {
        if !path.is_file() {
            tracing::debug!(
                "not editing {}: it does not exist yet (games create their settings on first run)",
                path.display()
            );
            continue;
        }
        if crate::ini::apply_to_file(&path, &sets)? {
            changed.push(path);
        }
    }
    Ok(changed)
}

/// Parse a `WINEDLLOVERRIDES` value (`a,b=n,b;c=d`) into DLL name →
/// load order, names lowercased. Entries without `=` are ignored, as Wine
/// ignores them.
pub(crate) fn parse_overrides(value: &str) -> BTreeMap<String, String> {
    let mut overrides = BTreeMap::new();
    for entry in value.split(';') {
        let Some((names, order)) = entry.split_once('=') else {
            continue;
        };
        for name in names
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            overrides.insert(name.to_ascii_lowercase(), order.trim().to_owned());
        }
    }
    overrides
}

/// Merge `layer` into `overrides`; names compared ignoring ASCII case.
pub(crate) fn layer_overrides(
    overrides: &mut BTreeMap<String, String>,
    layer: &BTreeMap<String, String>,
) {
    for (name, order) in layer {
        overrides.insert(name.to_ascii_lowercase(), order.clone());
    }
}

/// Where logs of `bottle` go when no [`Layout`] is at hand: the data
/// root's `logs/` for a bottle at `<root>/bottles/<name>` (every bottle
/// Uncork creates or imports), otherwise `<bottle>/logs`.
pub(crate) fn logs_dir_for(bottle: &Bottle) -> PathBuf {
    let bottles = bottle.path.parent();
    match bottles.and_then(|dir| Some((dir.file_name()?, dir.parent()?))) {
        Some((name, root)) if name == "bottles" => Layout::at(root).logs_dir(),
        _ => bottle.path.join("logs"),
    }
}

/// A file-name-safe version of `name`: ASCII letters, digits, `.`, `_` and
/// `-` kept, everything else `_`; `program` when nothing is left.
pub(crate) fn sanitize_file_part(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.chars().all(|c| matches!(c, '.' | '_')) {
        "program".to_owned()
    } else {
        cleaned
    }
}

// ----- backend selection -----

/// Where a fixed backend came from, for messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `--backend`.
    CommandLine,
    /// The bottle's `graphics.backend`.
    Bottle,
}

/// What the launch asks for, before availability is checked.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Request {
    /// This backend or an error.
    Fixed(Backend, Origin),
    /// [`crate::graphics::recommend`] with these preferences.
    Auto {
        preferred: Option<Backend>,
        fallbacks: Vec<Backend>,
    },
}

/// The precedence documented on [`plan`].
fn request(ctx: PlanContext<'_>, options: &LaunchOptions) -> Request {
    if let Some(BackendChoice::Fixed(backend)) = options.backend {
        return Request::Fixed(backend, Origin::CommandLine);
    }
    let graphics = ctx.profile.map(|profile| &profile.graphics);
    let profile_prefers = graphics
        .is_some_and(|graphics| graphics.backend.is_some() || !graphics.fallbacks.is_empty());
    if options.backend.is_none()
        && !profile_prefers
        && let BackendChoice::Fixed(backend) = ctx.bottle.config.graphics.backend
    {
        return Request::Fixed(backend, Origin::Bottle);
    }
    Request::Auto {
        preferred: graphics.and_then(|graphics| graphics.backend),
        fallbacks: graphics
            .map(|graphics| graphics.fallbacks.clone())
            .unwrap_or_default(),
    }
}

/// What is known about the program being launched.
#[derive(Debug, Default)]
struct Program {
    /// From the profile, else the scan; `None` for `WineProgram` targets.
    bitness: Option<Bitness>,
    /// From the profile, else the scan.
    api: Option<GraphicsApi>,
    /// From the profile.
    geometry_shaders: bool,
    /// The scan, when the executable could be scanned.
    scan: Option<GameScan>,
}

impl Program {
    /// "32-bit D3D11", when the bitness is known.
    fn describe(&self) -> Option<String> {
        self.bitness
            .map(|bitness| crate::graphics::describe_program(self.api, bitness))
    }
}

/// The outcome of backend selection.
struct Selection<'a> {
    backend: Backend,
    component: Option<&'a InstalledComponent>,
    reason: String,
    program: Program,
    warnings: Vec<String>,
}

/// Backend selection shared by [`plan`] and [`choose_backend`].
fn select<'a>(
    ctx: PlanContext<'a>,
    target: &Target,
    options: &LaunchOptions,
) -> crate::Result<Selection<'a>> {
    let mut warnings = Vec::new();
    let program = inspect_target(ctx, target, &mut warnings)?;
    let (backend, reason) = match (request(ctx, options), target) {
        (Request::Fixed(backend, origin @ Origin::CommandLine), _)
        | (Request::Fixed(backend, origin @ Origin::Bottle), Target::Exe { .. }) => {
            let reason = check_fixed(ctx, backend, origin, &program)?;
            (backend, reason)
        }
        (_, Target::WineProgram { name, .. }) => (
            Backend::Wined3d,
            format!("{name} is a program Wine runs itself: WineD3D"),
        ),
        (
            Request::Auto {
                preferred,
                fallbacks,
            },
            Target::Exe { .. },
        ) => choose_auto(ctx, &program, preferred, &fallbacks, &mut warnings),
    };
    Ok(Selection {
        backend,
        component: component_for(ctx, backend),
        reason,
        program,
        warnings,
    })
}

/// Learn the program's bitness and API from the profile and the scan.
fn inspect_target(
    ctx: PlanContext<'_>,
    target: &Target,
    warnings: &mut Vec<String>,
) -> crate::Result<Program> {
    let exe = ctx.profile.map(|profile| &profile.exe);
    let declared = Program {
        bitness: exe.and_then(|exe| exe.bitness),
        api: exe.and_then(|exe| exe.api),
        geometry_shaders: exe.is_some_and(|exe| exe.geometry_shaders),
        scan: None,
    };
    let Target::Exe { path, .. } = target else {
        return Ok(Program {
            bitness: None,
            api: None,
            ..declared
        });
    };
    if !path.is_file() {
        if declared.bitness.is_none() {
            return Err(Error::NotFound {
                what: "program",
                name: path.display().to_string(),
                hint: String::new(),
            });
        }
        warnings.push(format!(
            "{} does not exist; planned from the profile alone",
            path.display()
        ));
        return Ok(declared);
    }
    match uncork_pe::scan_game(path) {
        Ok(scan) => {
            let scanned = scan.bitness();
            if let Some(bitness) = declared.bitness
                && bitness != scanned
            {
                warnings.push(format!(
                    "the profile says {} is {}, but it is {}; using the profile",
                    file_name(path),
                    bits(bitness),
                    bits(scanned)
                ));
            }
            Ok(Program {
                bitness: Some(declared.bitness.unwrap_or(scanned)),
                api: declared.api.or_else(|| scan.primary_api()),
                geometry_shaders: declared.geometry_shaders,
                scan: Some(scan),
            })
        }
        Err(err) if declared.bitness.is_some() => {
            warnings.push(format!("cannot inspect {}: {err}", path.display()));
            Ok(declared)
        }
        Err(err) => Err(err.into()),
    }
}

/// Check a fixed backend; the reason sentence on success.
fn check_fixed(
    ctx: PlanContext<'_>,
    backend: Backend,
    origin: Origin,
    program: &Program,
) -> crate::Result<String> {
    let bottle = &ctx.bottle.config.name;
    let (asked, fix) = match origin {
        Origin::CommandLine => (
            format!("--backend {backend}"),
            "choose another backend, or let Uncork pick with `--backend auto`".to_owned(),
        ),
        Origin::Bottle => (
            format!("graphics.backend = {backend} in bottle {bottle}"),
            format!(
                "pass --backend, or let Uncork pick with `uncork bottle set {bottle} graphics.backend=auto`"
            ),
        ),
    };
    if let Some(why) = incompatibility(backend, program) {
        return Err(Error::Unsupported(format!("{asked}: {why}; {fix}")));
    }
    if let Some(why) = unavailable_reason(ctx, backend) {
        return Err(Error::Unsupported(format!("{asked}: {why}")));
    }
    let chosen = format!("{} ({asked})", display_name(backend));
    Ok(match program.describe() {
        Some(described) => format!("{described}: {chosen}"),
        None => chosen,
    })
}

/// Automatic choice with the profile's preferences.
fn choose_auto(
    ctx: PlanContext<'_>,
    program: &Program,
    preferred: Option<Backend>,
    fallbacks: &[Backend],
    warnings: &mut Vec<String>,
) -> (Backend, String) {
    let Some(bitness) = program.bitness else {
        // `inspect_target` always knows the bitness of an executable.
        return (Backend::Wined3d, "bitness unknown: WineD3D".to_owned());
    };
    let available = availability(ctx);
    let recommendation = crate::graphics::recommend(
        program.api,
        bitness,
        &available,
        preferred,
        fallbacks,
        program.geometry_shaders,
    );
    if let Some(preferred) = preferred
        && recommendation.backend != preferred
    {
        let why = incompatibility(preferred, program)
            .or_else(|| unavailable_reason(ctx, preferred))
            .unwrap_or_else(|| format!("{} was passed over", display_name(preferred)));
        let profile = ctx
            .profile
            .map_or("the profile", |profile| profile.id.as_str());
        warnings.push(format!(
            "{profile} prefers {}, but {why}; using {}",
            display_name(preferred),
            display_name(recommendation.backend)
        ));
    }
    (recommendation.backend, recommendation.reason)
}

/// Why `backend` cannot run `program` at all, if it cannot.
fn incompatibility(backend: Backend, program: &Program) -> Option<String> {
    if program.geometry_shaders && backend == Backend::Dxvk {
        return Some(
            "DXVK cannot run games that need geometry shaders (MoltenVK has none)".to_owned(),
        );
    }
    let bitness = program.bitness?;
    if backend == Backend::D3dmetal && bitness == Bitness::X86 {
        return Some(format!(
            "D3DMetal is 64-bit only and this is a {} program",
            bits(bitness)
        ));
    }
    let api = program.api?;
    (!crate::graphics::supports(backend, api, bitness)).then(|| {
        format!(
            "{} cannot run {}",
            display_name(backend),
            crate::graphics::describe_program(Some(api), bitness)
        )
    })
}

/// Why `backend` cannot be used in this bottle, with the fix; `None` when
/// it can (WineD3D always can): its component (or the bottle's pinned
/// version) must be installed and the runtime must be able to activate it
/// ([`crate::graphics::runtime_blocker`]), so [`crate::graphics::recommend`]
/// never picks a backend that [`crate::graphics::activation`] would reject.
fn unavailable_reason(ctx: PlanContext<'_>, backend: Backend) -> Option<String> {
    let kind = backend.component_kind()?;
    let name = display_name(backend);
    let pin = version_pin(ctx.bottle, backend);
    if component_for(ctx, backend).is_none() {
        let install = match (backend, pin) {
            (Backend::D3dmetal, _) => {
                "import it from Apple's Game Porting Toolkit with `uncork runtime import-gptk <path>`"
                    .to_owned()
            }
            (_, Some(version)) => {
                format!("install it with `uncork runtime install {kind} --version {version}`")
            }
            (_, None) => format!("install it with `uncork runtime install {kind}`"),
        };
        return Some(match pin {
            Some(version) => format!(
                "{name} {version} (pinned by graphics.{kind} in bottle {}) is not installed; {install}",
                ctx.bottle.config.name
            ),
            None => format!("{name} is not installed; {install}"),
        });
    }
    crate::graphics::runtime_blocker(backend, ctx.wine)
}

/// What [`crate::graphics::recommend`] may choose from.
fn availability(ctx: PlanContext<'_>) -> Availability {
    let usable = |backend| unavailable_reason(ctx, backend).is_none();
    Availability {
        dxmt: usable(Backend::Dxmt),
        dxvk: usable(Backend::Dxvk),
        d3dmetal: usable(Backend::D3dmetal),
    }
}

/// The installed component for `backend`: the bottle's pinned version, else
/// the newest. `None` for WineD3D.
fn component_for<'a>(ctx: PlanContext<'a>, backend: Backend) -> Option<&'a InstalledComponent> {
    let kind = backend.component_kind()?;
    let pin = version_pin(ctx.bottle, backend);
    ctx.components
        .iter()
        .filter(|component| component.meta.kind == kind)
        .filter(|component| pin.is_none_or(|pin| component.meta.version == pin))
        .max_by(|a, b| compare_versions(&a.meta.version, &b.meta.version))
}

/// The bottle's version pin for `backend`'s component.
fn version_pin(bottle: &Bottle, backend: Backend) -> Option<&str> {
    let graphics = &bottle.config.graphics;
    match backend {
        Backend::Dxmt => graphics.dxmt.as_deref(),
        Backend::Dxvk => graphics.dxvk.as_deref(),
        Backend::D3dmetal => graphics.d3dmetal.as_deref(),
        Backend::Wined3d => None,
    }
}

/// The project's own spelling of a backend, for messages.
fn display_name(backend: Backend) -> &'static str {
    match backend {
        Backend::D3dmetal => "D3DMetal",
        Backend::Dxmt => "DXMT",
        Backend::Dxvk => "DXVK",
        Backend::Wined3d => "WineD3D",
    }
}

fn bits(bitness: Bitness) -> &'static str {
    match bitness {
        Bitness::X86 => "32-bit",
        Bitness::X64 => "64-bit",
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

// ----- environment and warnings -----

/// Performance settings after the profile and command line are applied.
#[derive(Debug, Clone)]
struct Performance {
    retina: bool,
    metalfx: bool,
    hud: bool,
    avx: bool,
    /// An explicit `max_fps` and where it was set.
    max_fps: Option<(u32, String)>,
}

impl Performance {
    fn effective(
        bottle: &Bottle,
        profile: Option<&GameProfile>,
        options: &LaunchOptions,
    ) -> Performance {
        let base = &bottle.config.performance;
        let overrides = profile.map(|profile| &profile.performance);
        let max_fps = match (profile, overrides.and_then(|o| o.max_fps)) {
            (Some(profile), Some(fps)) => Some((
                fps,
                format!("performance.max_fps in profile {}", profile.id),
            )),
            _ => base.max_fps.map(|fps| {
                (
                    fps,
                    format!("performance.max_fps in bottle {}", bottle.config.name),
                )
            }),
        };
        Performance {
            retina: options.retina || overrides.and_then(|o| o.retina).unwrap_or(base.retina),
            metalfx: options.metalfx || overrides.and_then(|o| o.metalfx).unwrap_or(base.metalfx),
            hud: options.hud || base.hud,
            avx: overrides.and_then(|o| o.avx).unwrap_or(base.avx),
            max_fps,
        }
    }
}

/// The environment of the module docs, `WINEDLLOVERRIDES` included.
fn launch_env(
    ctx: PlanContext<'_>,
    options: &LaunchOptions,
    activation: &Activation,
    avx: bool,
) -> BTreeMap<String, String> {
    let bottle = &ctx.bottle.config;
    let mut env = base_env(ctx.bottle, ctx.wine, options.wine_debug.as_deref());
    if avx {
        env.insert(AVX_VAR.to_owned(), "1".to_owned());
    } else {
        env.remove(AVX_VAR);
    }
    env.extend(activation.env.clone());

    let mut overrides =
        BTreeMap::from([(NO_MENU_BUILDER.0.to_owned(), NO_MENU_BUILDER.1.to_owned())]);
    layer_overrides(&mut overrides, &activation.overrides);
    layer_overrides(&mut overrides, &bottle.dll_overrides);
    if let Some(profile) = ctx.profile {
        layer_overrides(&mut overrides, &profile.dll_overrides);
    }

    let empty = BTreeMap::new();
    let profile_env = ctx.profile.map_or(&empty, |profile| &profile.env);
    for layer in [&bottle.env, profile_env, &options.env] {
        for (key, value) in layer {
            if key == OVERRIDES_VAR {
                layer_overrides(&mut overrides, &parse_overrides(value));
            } else {
                env.insert(key.clone(), value.clone());
            }
        }
    }
    env.insert(
        OVERRIDES_VAR.to_owned(),
        crate::graphics::render_overrides(&overrides),
    );
    env
}

/// Warnings about the program itself (from its facts and scan).
fn program_warnings(
    ctx: PlanContext<'_>,
    program: &Program,
    backend: Backend,
    env: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    let scan = program.scan.as_ref();
    if let Some(scan) = scan
        && !scan.anti_cheat.is_empty()
    {
        warnings.push(format!(
            "kernel anti-cheat found next to the game ({}): it does not run under Wine, so the game or its online modes will likely refuse to start",
            scan.anti_cheat.join(", ")
        ));
    }
    let Some(bitness) = program.bitness else {
        return warnings;
    };
    let described = crate::graphics::describe_program(program.api, bitness);
    if bitness == Bitness::X86 && !ctx.wine.has_feature(FEATURE_WOW64) {
        warnings.push(format!(
            "this is a 32-bit program, but Wine {} lacks the `wow64` feature and cannot start 32-bit programs; use a Wine runtime that has it",
            ctx.wine.version
        ));
    }
    if program.api == Some(GraphicsApi::D3d12) && backend != Backend::D3dmetal {
        warnings.push(match bitness {
            Bitness::X86 => format!(
                "{described}: no backend on macOS runs 32-bit Direct3D 12 (D3DMetal is 64-bit only), so it will fail to render"
            ),
            Bitness::X64 => {
                let why = unavailable_reason(ctx, Backend::D3dmetal)
                    .unwrap_or_else(|| "it was not chosen".to_owned());
                format!(
                    "{described}: only D3DMetal runs Direct3D 12 and {} will fail; D3DMetal: {why}",
                    display_name(backend)
                )
            }
        });
    }
    if bitness == Bitness::X86
        && let Some(scan) = scan
    {
        if !scan.non_nx_modules.is_empty() {
            warnings.push(format!(
                "modules without NX_COMPAT ({}): when one loads, Wine turns DEP off for the whole 32-bit process, which makes first-touch page faults about 200 times costlier under Rosetta (expect stutter)",
                scan.non_nx_modules.join(", ")
            ));
        }
        // Only games need the hint; Windows tools (cmd, winecfg) never do.
        if !scan.info.large_address_aware
            && scan.primary_api().is_some()
            && ctx.wine.has_feature(FEATURE_LARGE_ADDRESS_AWARE)
            && !env.contains_key(LAA_VAR)
        {
            warnings.push(format!(
                "{} is not large-address-aware (2 GiB of address space); Wine {} gives it 4 GiB with {LAA_VAR}=1: pass `--env {LAA_VAR}=1` or add it to the profile's [env]",
                file_name(&scan.exe),
                ctx.wine.version
            ));
        }
    }
    warnings
}

/// Warnings about settings this launch cannot change on its own.
fn setting_warnings(ctx: PlanContext<'_>, performance: &Performance) -> Vec<String> {
    let mut warnings = Vec::new();
    let config = &ctx.bottle.config;
    let bottle = &config.name;
    let on_off = |on: bool| if on { "on" } else { "off" };
    if performance.retina != config.performance.retina {
        warnings.push(format!(
            "Retina mode {} was asked for, but it is a bottle-wide registry setting and bottle {bottle} has it {}; this launch keeps the bottle's setting (change it with `uncork bottle set {bottle} performance.retina={}`)",
            on_off(performance.retina),
            on_off(config.performance.retina),
            performance.retina
        ));
    }
    if let Some(profile) = ctx.profile {
        if let Some(msync) = profile.wine.msync
            && msync != config.performance.msync
        {
            warnings.push(format!(
                "{} wants msync {}, but msync is per bottle (every process of a bottle must agree, and a mismatched one exits); run `uncork bottle set {bottle} performance.msync={msync}` and restart the bottle with `uncork bottle kill {bottle}`",
                profile.id,
                on_off(msync)
            ));
        }
        if let Some(version) = profile.wine.windows_version
            && version != config.windows_version
        {
            warnings.push(format!(
                "{} wants Windows version {}, but bottle {bottle} reports {}; change it with `uncork bottle set {bottle} windows_version={}`",
                profile.id,
                version.winecfg_name(),
                config.windows_version.winecfg_name(),
                version.winecfg_name()
            ));
        }
    }
    warnings
}

/// The file stem of a Windows program name or path: `C:\a\b.exe` → `b`.
fn windows_stem(name: &str) -> &str {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    match file.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => stem,
        _ => file,
    }
}

// ----- profile paths -----

/// The `%BASE%` tokens of profile INI paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IniBase {
    AppData,
    LocalAppData,
    UserProfile,
    InstallDir,
}

const INI_BASES: [(&str, IniBase); 4] = [
    ("%APPDATA%", IniBase::AppData),
    ("%LOCALAPPDATA%", IniBase::LocalAppData),
    ("%USERPROFILE%", IniBase::UserProfile),
    ("%INSTALLDIR%", IniBase::InstallDir),
];

/// Split `%BASE%\rest` (base matched ignoring ASCII case); the rest must
/// start with a separator.
fn split_ini_base(file: &str) -> Option<(IniBase, &str)> {
    INI_BASES.iter().find_map(|(token, base)| {
        let head = file.get(..token.len())?;
        let rest = &file[token.len()..];
        (head.eq_ignore_ascii_case(token) && rest.starts_with(['\\', '/'])).then_some((*base, rest))
    })
}

/// The bottle's Windows user: `env.USER`, else the only user directory,
/// else this process's `USER`. Never a name that could leave `users/`.
fn windows_user(bottle: &Bottle) -> Option<String> {
    let user = bottle
        .config
        .env
        .get("USER")
        .filter(|user| !user.is_empty())
        .cloned()
        .or_else(|| only_prefix_user(&bottle.drive_c().join("users")))
        .or_else(|| std::env::var("USER").ok().filter(|user| !user.is_empty()))?;
    let safe = !matches!(user.as_str(), "." | "..") && !user.contains(['/', '\\']);
    safe.then_some(user)
}

/// The only directory in `users` other than `Public` (and hidden ones).
fn only_prefix_user(users: &Path) -> Option<String> {
    let entries = std::fs::read_dir(users).ok()?;
    let mut names = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| !name.eq_ignore_ascii_case("public") && !name.starts_with('.'));
    let user = names.next()?;
    names.next().is_none().then_some(user)
}

/// `root` joined with `components`, each spelled as the directory entry it
/// names on disk: an exact match if there is one, else the first entry (in
/// sorted order) that equals it ignoring ASCII case. Components below one
/// that does not exist are kept as written.
pub(crate) fn join_case_insensitive<S: AsRef<str>>(root: &Path, components: &[S]) -> PathBuf {
    let mut path = root.to_path_buf();
    let mut on_disk = true;
    for component in components {
        let component = component.as_ref();
        if on_disk {
            if let Some(entry) = entry_named(&path, component) {
                path.push(entry);
                continue;
            }
            on_disk = false;
        }
        path.push(component);
    }
    path
}

/// The entry of `dir` named `name`, as [`join_case_insensitive`] matches it.
fn entry_named(dir: &Path, name: &str) -> Option<OsString> {
    let mut candidates: Vec<OsString> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|entry| {
            entry
                .to_str()
                .is_some_and(|entry| entry.eq_ignore_ascii_case(name))
        })
        .collect();
    if candidates.iter().any(|candidate| candidate == name) {
        return Some(OsString::from(name));
    }
    candidates.sort();
    candidates.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_overrides_like_wine() {
        let parsed = parse_overrides("d3d10core,D3D11=n,b; dxgi=b;;bogus;winemenubuilder.exe=d;x=");
        assert_eq!(
            parsed,
            BTreeMap::from([
                ("d3d10core".to_owned(), "n,b".to_owned()),
                ("d3d11".to_owned(), "n,b".to_owned()),
                ("dxgi".to_owned(), "b".to_owned()),
                ("winemenubuilder.exe".to_owned(), "d".to_owned()),
                ("x".to_owned(), String::new()),
            ])
        );
        let rendered = crate::graphics::render_overrides(&parsed);
        assert_eq!(parse_overrides(&rendered), parsed, "{rendered}");
    }

    #[test]
    fn layering_overrides_ignores_case() {
        let mut overrides = BTreeMap::from([("d3d11".to_owned(), "n,b".to_owned())]);
        layer_overrides(
            &mut overrides,
            &BTreeMap::from([("D3D11".to_owned(), "b".to_owned())]),
        );
        assert_eq!(
            overrides,
            BTreeMap::from([("d3d11".to_owned(), "b".to_owned())])
        );
    }

    #[test]
    fn sanitizes_file_parts() {
        assert_eq!(sanitize_file_part("Rise of Nations"), "Rise_of_Nations");
        assert_eq!(sanitize_file_part("AoE2DE_s"), "AoE2DE_s");
        assert_eq!(sanitize_file_part("a/b\\c:d"), "a_b_c_d");
        assert_eq!(sanitize_file_part("ünï"), "_n_");
        assert_eq!(sanitize_file_part(""), "program");
        assert_eq!(sanitize_file_part(".."), "program");
        assert_eq!(sanitize_file_part("x-1.2"), "x-1.2");
    }

    #[test]
    fn windows_stems() {
        assert_eq!(
            windows_stem(r"C:\Games\RoN\riseofnations.exe"),
            "riseofnations"
        );
        assert_eq!(windows_stem("winecfg"), "winecfg");
        assert_eq!(windows_stem("regedit.exe"), "regedit");
        assert_eq!(windows_stem("C:/a/.hidden"), ".hidden");
        assert_eq!(windows_stem(r"C:\dir\"), "");
    }

    #[test]
    fn splits_ini_bases_ignoring_case() {
        assert_eq!(
            split_ini_base(r"%appdata%\x.ini"),
            Some((IniBase::AppData, r"\x.ini"))
        );
        assert_eq!(
            split_ini_base("%INSTALLDIR%/a/b.ini"),
            Some((IniBase::InstallDir, "/a/b.ini"))
        );
        assert_eq!(split_ini_base("%APPDATA%x.ini"), None);
        assert_eq!(split_ini_base("%TEMP%\\x.ini"), None);
        assert_eq!(split_ini_base("%APP"), None);
        assert_eq!(split_ini_base("ü%APPDATA%\\x"), None);
    }

    #[test]
    fn logs_of_bottles_outside_the_layout_stay_in_the_bottle() {
        let bottle = |path: &str| Bottle {
            path: PathBuf::from(path),
            config: toml::from_str("schema = 1\nname = \"b\"\nwine = \"1\"\n").unwrap(),
            state: crate::bottle::BottleState::default(),
        };
        assert_eq!(
            logs_dir_for(&bottle("/data/uncork/bottles/steam")),
            Path::new("/data/uncork/logs")
        );
        assert_eq!(
            logs_dir_for(&bottle("/elsewhere/steam")),
            Path::new("/elsewhere/steam/logs")
        );
        assert_eq!(logs_dir_for(&bottle("/")), Path::new("/logs"));
    }

    #[test]
    fn joins_paths_with_the_on_disk_spelling() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("Users/CrossOver/AppData")).unwrap();
        let joined = join_case_insensitive(
            temp.path(),
            &["users", "crossover", "appdata", "Roaming", "x.ini"],
        );
        assert_eq!(
            joined,
            temp.path().join("Users/CrossOver/AppData/Roaming/x.ini")
        );
        let missing = join_case_insensitive(temp.path(), &["nope", "Deeper"]);
        assert_eq!(missing, temp.path().join("nope").join("Deeper"));
    }

    #[test]
    fn exact_names_win_over_case_insensitive_ones() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("Rise2.INI"), "").unwrap();
        assert_eq!(
            entry_named(temp.path(), "rise2.ini"),
            Some(OsString::from("Rise2.INI"))
        );
        assert_eq!(
            entry_named(temp.path(), "Rise2.INI"),
            Some(OsString::from("Rise2.INI"))
        );
        assert_eq!(entry_named(temp.path(), "other.ini"), None);
        assert_eq!(entry_named(&temp.path().join("missing"), "x"), None);
    }
}
