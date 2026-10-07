//! Game profiles: per-game settings that make a title run well.
//!
//! Built-in profiles live in `profiles/<id>.toml` at the repository root and
//! are compiled into the binary (see `build.rs`). Users can add or override
//! profiles in `$UNCORK_HOME/profiles/<id>.toml`; a user profile with the
//! same `id` replaces the built-in one entirely. The schema is documented in
//! `profiles/README.md`.
//!
//! ```toml
//! schema = 1
//! id = "rise-of-nations-extended-edition"
//! name = "Rise of Nations: Extended Edition"
//!
//! [steam]
//! appid = 287450
//!
//! [exe]
//! path = "riseofnations.exe"      # relative to the install directory, '/' separators
//! bitness = "x86"
//! api = "d3d11"
//!
//! [graphics]
//! backend = "dxmt"
//! fallbacks = ["dxvk", "wined3d"]
//!
//! [compat]
//! status = "untested"
//! notes = "..."
//! ```

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use uncork_pe::{Bitness, GraphicsApi};

use crate::bottle::WindowsVersion;
use crate::graphics::Backend;
use crate::paths::Layout;
use crate::steam::LaunchMode;

/// Steam identity of a game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SteamRef {
    /// Steam app id.
    pub appid: u32,
}

/// The executable that renders the game.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExeRef {
    /// Path relative to the install directory, `/`-separated.
    pub path: String,
    /// Bitness, if known (otherwise detected with `uncork-pe`).
    #[serde(default)]
    pub bitness: Option<Bitness>,
    /// Primary graphics API, if known (otherwise detected).
    #[serde(default)]
    pub api: Option<GraphicsApi>,
    /// The game uses geometry shaders (rules out DXVK on MoltenVK).
    #[serde(default)]
    pub geometry_shaders: bool,
}

/// Graphics preferences.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileGraphics {
    /// Preferred backend; `None` lets [`crate::graphics::recommend`] decide.
    #[serde(default)]
    pub backend: Option<Backend>,
    /// Backends to try, in order, if the preferred one is not installed.
    #[serde(default)]
    pub fallbacks: Vec<Backend>,
}

/// Wine settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileWine {
    /// Windows version the game needs, if not the bottle's.
    #[serde(default)]
    pub windows_version: Option<WindowsVersion>,
    /// Force msync on or off.
    #[serde(default)]
    pub msync: Option<bool>,
}

/// Performance overrides; `None` keeps the bottle's value.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfilePerformance {
    /// Retina mode.
    #[serde(default)]
    pub retina: Option<bool>,
    /// MetalFX upscaling.
    #[serde(default)]
    pub metalfx: Option<bool>,
    /// Advertise AVX through Rosetta.
    #[serde(default)]
    pub avx: Option<bool>,
}

/// How to start the game.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileLaunch {
    /// Extra arguments passed to the game.
    #[serde(default)]
    pub args: Vec<String>,
    /// How to start it; default [`LaunchMode::Direct`] for Steam games and
    /// [`LaunchMode::Standalone`] otherwise.
    #[serde(default)]
    pub mode: Option<LaunchMode>,
}

/// One INI key a profile enforces before each launch.
///
/// `file` is a Windows-style path relative to a base: `%APPDATA%\...`,
/// `%LOCALAPPDATA%\...`, `%USERPROFILE%\...` (resolved inside the bottle
/// for the prefix's user: the one directory under `drive_c/users` other than
/// `Public`), or `%INSTALLDIR%\...` (the game's install directory). `/` and
/// `\` are both accepted.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileIni {
    /// File path with a `%BASE%` prefix.
    pub file: String,
    /// Section.
    pub section: String,
    /// Key.
    pub key: String,
    /// Value.
    pub value: String,
    /// Why (shown by `uncork profile show`).
    #[serde(default)]
    pub reason: String,
}

/// How well the game runs. Ordered worst to best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CompatStatus {
    /// Nobody has reported a result with Uncork yet.
    #[default]
    Untested,
    /// Does not start or is unplayable.
    Broken,
    /// Starts and is playable with notable problems (listed in `notes`).
    Runs,
    /// Playable start to finish with minor issues.
    Playable,
    /// Indistinguishable from Windows.
    Perfect,
}

/// One hands-on test result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestReport {
    /// ISO date, `YYYY-MM-DD`.
    pub date: String,
    /// Uncork version or commit.
    pub uncork: String,
    /// macOS version, e.g. `27.0.1`.
    pub macos: String,
    /// Chip, e.g. `M5 Max`.
    pub chip: String,
    /// Backend used.
    pub backend: Backend,
    /// Result.
    pub status: CompatStatus,
    /// Free text: resolution, settings, FPS.
    #[serde(default)]
    pub notes: String,
}

/// Compatibility information.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compat {
    /// Overall status (the best recent report).
    #[serde(default)]
    pub status: CompatStatus,
    /// What players should know.
    #[serde(default)]
    pub notes: String,
    /// Test history, newest first.
    #[serde(default)]
    pub reports: Vec<TestReport>,
}

/// A game profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameProfile {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Kebab-case id; equals the file stem.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Steam identity, if the game is on Steam.
    #[serde(default)]
    pub steam: Option<SteamRef>,
    /// Executable.
    pub exe: ExeRef,
    /// Graphics preferences.
    #[serde(default)]
    pub graphics: ProfileGraphics,
    /// Wine settings.
    #[serde(default)]
    pub wine: ProfileWine,
    /// Performance overrides.
    #[serde(default)]
    pub performance: ProfilePerformance,
    /// Extra environment variables.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// Extra DLL overrides.
    #[serde(default)]
    pub dll_overrides: BTreeMap<String, String>,
    /// Launch settings.
    #[serde(default)]
    pub launch: ProfileLaunch,
    /// INI keys to enforce before launch.
    #[serde(default)]
    pub ini: Vec<ProfileIni>,
    /// Compatibility information.
    #[serde(default)]
    pub compat: Compat,
}

impl GameProfile {
    /// Parse one profile. `origin` names the file in errors.
    ///
    /// # Errors
    /// [`crate::Error::Config`] with `what = "profile"`.
    pub fn parse(text: &str, origin: &std::path::Path) -> crate::Result<GameProfile> {
        let _ = (text, origin);
        todo!()
    }

    /// Problems that make a profile unusable: schema != 1; id not
    /// kebab-case (`^[a-z0-9]+(-[a-z0-9]+)*$`, ≤ 64 chars); empty name;
    /// `exe.path` empty, absolute, containing `\` or `..`; env keys not
    /// `^[A-Za-z_][A-Za-z0-9_]*$`; DLL override values not one of `n`, `b`,
    /// `n,b`, `b,n`, `d`, `` (empty = disabled); the preferred backend
    /// repeated in `fallbacks`; a backend that cannot run the declared
    /// `bitness`+`api` ([`crate::graphics::supports`]); DXVK listed while
    /// `geometry_shaders` is true; `mode = "direct"`/`"applaunch"` without
    /// `[steam]`; an `ini.file` without a known `%BASE%` prefix or with `..`.
    /// Empty when valid.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        todo!()
    }

    /// The effective launch mode.
    #[must_use]
    pub fn launch_mode(&self) -> LaunchMode {
        self.launch.mode.unwrap_or(if self.steam.is_some() {
            LaunchMode::Direct
        } else {
            LaunchMode::Standalone
        })
    }
}

/// `(file name, contents)` of every built-in profile, generated by `build.rs`
/// from `profiles/*.toml`, sorted by file name.
pub const BUILTIN_FILES: &[(&str, &str)] = include!(concat!(env!("OUT_DIR"), "/builtin_profiles.rs"));

/// Parse every built-in profile.
///
/// # Panics
/// If a built-in profile is invalid (unit tests guarantee none is).
#[must_use]
pub fn builtin() -> Vec<GameProfile> {
    todo!()
}

/// Built-in profiles overlaid with `$UNCORK_HOME/profiles/*.toml` (user
/// profiles replace built-ins with the same id), sorted by id. Invalid user
/// files are returned as errors in the second vector, not fatal.
///
/// # Errors
/// [`crate::Error::Io`] if the profiles directory exists but cannot be listed.
pub fn load_all(layout: &Layout) -> crate::Result<(Vec<GameProfile>, Vec<crate::Error>)> {
    let _ = layout;
    todo!()
}

/// Find a profile by, in order: exact id; Steam app id (if `query` is all
/// digits); case-insensitive exact name; unique case-insensitive substring of
/// id or name. Ambiguous substrings return `None`.
#[must_use]
pub fn resolve<'a>(profiles: &'a [GameProfile], query: &str) -> Option<&'a GameProfile> {
    let _ = (profiles, query);
    todo!()
}
