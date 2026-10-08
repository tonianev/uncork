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
use std::path::{Path, PathBuf};

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
        toml::from_str(text).map_err(|err| crate::Error::Config {
            what: "profile",
            path: origin.to_path_buf(),
            message: describe_toml_error(text, &err),
        })
    }

    /// Problems that make a profile unusable: schema != 1; id not
    /// kebab-case (`^[a-z0-9]+(-[a-z0-9]+)*$`, ≤ 64 chars); empty name;
    /// `exe.path` empty, absolute, containing `\` or `..`; env keys not
    /// `^[A-Za-z_][A-Za-z0-9_]*$`; DLL override values not one of `n`, `b`,
    /// `n,b`, `b,n`, `d` or empty (disabled); the preferred backend
    /// repeated in `fallbacks`; a backend that cannot run the declared
    /// `bitness`+`api` ([`crate::graphics::supports`]); DXVK listed while
    /// `geometry_shaders` is true; `mode = "direct"`/`"applaunch"` without
    /// `[steam]`; an `ini.file` without a known `%BASE%` prefix or with `..`.
    /// Empty when valid.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        if self.schema != 1 {
            problems.push(format!("schema is {}, expected 1", self.schema));
        }
        if !is_kebab_id(&self.id) {
            problems.push(format!(
                "id {:?} is not kebab-case (lowercase letters and digits joined by '-', at most {MAX_ID_LEN} characters)",
                self.id
            ));
        }
        if self.name.trim().is_empty() {
            problems.push("name is empty".to_owned());
        }
        if let Some(why) = exe_path_problem(&self.exe.path) {
            problems.push(format!("exe.path {:?} {why}", self.exe.path));
        }
        for key in self.env.keys().filter(|key| !is_env_name(key)) {
            problems.push(format!("env key {key:?} is not a valid variable name"));
        }
        for (dll, value) in &self.dll_overrides {
            if !OVERRIDE_VALUES.contains(&value.as_str()) {
                problems.push(format!(
                    "dll_overrides.{dll} = {value:?} is not one of \"n\", \"b\", \"n,b\", \"b,n\", \"d\" or \"\""
                ));
            }
        }
        problems.extend(self.graphics_problems());
        if let Some(mode @ (LaunchMode::Direct | LaunchMode::Applaunch)) = self.launch.mode
            && self.steam.is_none()
        {
            let mode = if mode == LaunchMode::Direct {
                "direct"
            } else {
                "applaunch"
            };
            problems.push(format!("launch.mode = {mode:?} needs a [steam] section"));
        }
        for ini in &self.ini {
            if let Some(why) = ini_file_problem(&ini.file) {
                problems.push(format!("ini.file {:?} {why}", ini.file));
            }
        }
        problems
    }

    /// The `[graphics]` rules of [`GameProfile::problems`].
    fn graphics_problems(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let graphics = &self.graphics;
        if let Some(preferred) = graphics.backend
            && graphics.fallbacks.contains(&preferred)
        {
            problems.push(format!(
                "graphics.fallbacks repeats the preferred backend {preferred}"
            ));
        }

        let mut listed: Vec<Backend> = Vec::new();
        for backend in graphics.backend.iter().chain(&graphics.fallbacks) {
            if !listed.contains(backend) {
                listed.push(*backend);
            }
        }
        let bitnesses = match self.exe.bitness {
            Some(bitness) => vec![bitness],
            None => vec![Bitness::X86, Bitness::X64],
        };
        for backend in listed {
            if let Some(api) = self.exe.api
                && !bitnesses
                    .iter()
                    .any(|bitness| crate::graphics::supports(backend, api, *bitness))
            {
                let program = match self.exe.bitness {
                    Some(bitness) => crate::graphics::describe_program(Some(api), bitness),
                    None => crate::graphics::api_name(api).to_owned(),
                };
                problems.push(format!("graphics backend {backend} cannot run {program}"));
            }
            if backend == Backend::Dxvk && self.exe.geometry_shaders {
                problems.push(
                    "graphics backend dxvk cannot run games that need geometry shaders (exe.geometry_shaders)"
                        .to_owned(),
                );
            }
        }
        problems
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

/// Longest profile id [`GameProfile::problems`] accepts.
const MAX_ID_LEN: usize = 64;

/// Values Wine accepts in `WINEDLLOVERRIDES` (empty = disabled).
const OVERRIDE_VALUES: [&str; 6] = ["n", "b", "n,b", "b,n", "d", ""];

/// The `%BASE%` prefixes an `ini.file` may start with.
const INI_BASES: [&str; 4] = [
    "%APPDATA%",
    "%LOCALAPPDATA%",
    "%USERPROFILE%",
    "%INSTALLDIR%",
];

/// `^[a-z0-9]+(-[a-z0-9]+)*$`, at most [`MAX_ID_LEN`] characters.
fn is_kebab_id(id: &str) -> bool {
    id.len() <= MAX_ID_LEN
        && id.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        })
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`.
fn is_env_name(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Why `exe.path` is unusable, if it is.
fn exe_path_problem(path: &str) -> Option<&'static str> {
    let has_drive = matches!(path.as_bytes(), [letter, b':', ..] if letter.is_ascii_alphabetic());
    if path.trim().is_empty() {
        Some("is empty")
    } else if path.starts_with('/') || has_drive {
        Some("is absolute; give it relative to the install directory")
    } else if path.contains('\\') {
        Some("contains '\\'; separate directories with '/'")
    } else if path.split('/').any(|part| part == "..") {
        Some("contains '..'")
    } else {
        None
    }
}

/// Why an `ini.file` is unusable, if it is.
fn ini_file_problem(file: &str) -> Option<String> {
    let rest = INI_BASES
        .iter()
        .find_map(|base| file.strip_prefix(base))
        .filter(|rest| rest.starts_with(['\\', '/']));
    match rest {
        None => Some(format!(
            "does not start with one of {} followed by '\\'",
            INI_BASES.join(", ")
        )),
        Some(rest) if rest.split(['\\', '/']).any(|part| part == "..") => {
            Some("contains '..'".to_owned())
        }
        Some(_) => None,
    }
}

/// A one-line parse error: `line N: message`.
fn describe_toml_error(text: &str, err: &toml::de::Error) -> String {
    let message = err.message().trim_end();
    match err
        .span()
        .and_then(|span| text.as_bytes().get(..span.start))
    {
        Some(before) => {
            let line = before.iter().filter(|&&b| b == b'\n').count() + 1;
            format!("line {line}: {message}")
        }
        None => message.to_owned(),
    }
}

/// `(file name, contents)` of every built-in profile, generated by `build.rs`
/// from `profiles/*.toml`, sorted by file name.
pub const BUILTIN_FILES: &[(&str, &str)] =
    include!(concat!(env!("OUT_DIR"), "/builtin_profiles.rs"));

/// Parse every built-in profile.
///
/// # Panics
/// If a built-in profile is invalid (unit tests guarantee none is).
#[must_use]
pub fn builtin() -> Vec<GameProfile> {
    BUILTIN_FILES
        .iter()
        .map(|(file, text)| {
            GameProfile::parse(text, Path::new(file))
                .unwrap_or_else(|err| panic!("built-in profile {file} is invalid: {err}"))
        })
        .collect()
}

/// Built-in profiles overlaid with `$UNCORK_HOME/profiles/*.toml` (user
/// profiles replace built-ins with the same id), sorted by id. Invalid user
/// files are returned as errors in the second vector, not fatal.
///
/// # Errors
/// [`crate::Error::Io`] if the profiles directory exists but cannot be listed.
pub fn load_all(layout: &Layout) -> crate::Result<(Vec<GameProfile>, Vec<crate::Error>)> {
    let mut profiles: BTreeMap<String, GameProfile> = builtin()
        .into_iter()
        .map(|profile| (profile.id.clone(), profile))
        .collect();
    let mut errors = Vec::new();
    for path in user_profile_files(&layout.profiles_dir())? {
        match load_user_profile(&path) {
            Ok(profile) => {
                profiles.insert(profile.id.clone(), profile);
            }
            Err(err) => errors.push(err),
        }
    }
    Ok((profiles.into_values().collect(), errors))
}

/// The `*.toml` files in `dir`, sorted; none when `dir` does not exist.
/// Hidden files (such as macOS `._*` metadata files) are skipped.
fn user_profile_files(dir: &Path) -> crate::Result<Vec<PathBuf>> {
    let list_error = |err| crate::Error::io("cannot list directory", dir, err);
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(list_error(err)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(list_error)?.path();
        let hidden = path
            .file_name()
            .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."));
        if !hidden && path.extension().is_some_and(|ext| ext == "toml") && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// Read, parse and check one user profile, whose id must match its file name.
fn load_user_profile(path: &Path) -> crate::Result<GameProfile> {
    let text =
        std::fs::read_to_string(path).map_err(|err| crate::Error::io("cannot read", path, err))?;
    let profile = GameProfile::parse(&text, path)?;
    let mut problems = profile.problems();
    if path.file_stem().and_then(|stem| stem.to_str()) != Some(profile.id.as_str()) {
        problems.push(format!("id {:?} does not match the file name", profile.id));
    }
    if problems.is_empty() {
        Ok(profile)
    } else {
        Err(crate::Error::Config {
            what: "profile",
            path: path.to_path_buf(),
            message: problems.join("; "),
        })
    }
}

/// What [`lookup`] and [`lookup_appid`] found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// Exactly one profile matches.
    Found(&'a GameProfile),
    /// Several profiles match equally well (in the order given), so none is
    /// chosen; a caller names them so the user can pick one by its id.
    Ambiguous(Vec<&'a GameProfile>),
    /// No profile matches.
    NotFound,
}

impl<'a> Lookup<'a> {
    /// The profile, when exactly one matched.
    #[must_use]
    pub fn found(self) -> Option<&'a GameProfile> {
        match self {
            Lookup::Found(profile) => Some(profile),
            Lookup::Ambiguous(_) | Lookup::NotFound => None,
        }
    }
}

/// Find a profile by, in order: exact id; Steam app id (if `query` is all
/// digits); case-insensitive exact name; unique case-insensitive substring of
/// id or name. Ambiguous matches return `None`; [`lookup`] tells them apart
/// from no match.
#[must_use]
pub fn resolve<'a>(profiles: &'a [GameProfile], query: &str) -> Option<&'a GameProfile> {
    lookup(profiles, query).found()
}

/// [`resolve`], saying whether nothing matched or several profiles did. The
/// first step that matches anything decides: one match is
/// [`Lookup::Found`], several are [`Lookup::Ambiguous`] (later steps are
/// not tried).
#[must_use]
pub fn lookup<'a>(profiles: &'a [GameProfile], query: &str) -> Lookup<'a> {
    let query = query.trim();
    if query.is_empty() {
        return Lookup::NotFound;
    }
    let appid = query
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| query.parse::<u32>().ok())
        .flatten();
    let lower = query.to_lowercase();

    let by_id = |p: &GameProfile| p.id == query;
    let by_appid = |p: &GameProfile| appid.is_some() && profile_appid(p) == appid;
    let by_name = |p: &GameProfile| p.name.to_lowercase() == lower;
    let by_substring = |p: &GameProfile| {
        p.id.to_lowercase().contains(&lower) || p.name.to_lowercase().contains(&lower)
    };
    let steps: [&dyn Fn(&GameProfile) -> bool; 4] = [&by_id, &by_appid, &by_name, &by_substring];
    for is_match in steps {
        match matching(profiles, is_match) {
            Lookup::NotFound => {}
            decided => return decided,
        }
    }
    Lookup::NotFound
}

/// The profile for Steam app `appid` (the app id step of [`lookup`] alone,
/// for commands that take an app id).
#[must_use]
pub fn lookup_appid(profiles: &[GameProfile], appid: u32) -> Lookup<'_> {
    matching(profiles, |profile| profile_appid(profile) == Some(appid))
}

fn profile_appid(profile: &GameProfile) -> Option<u32> {
    profile.steam.as_ref().map(|steam| steam.appid)
}

/// Every profile `is_match` accepts, as a [`Lookup`].
fn matching<'a>(
    profiles: &'a [GameProfile],
    is_match: impl Fn(&GameProfile) -> bool,
) -> Lookup<'a> {
    let mut found: Vec<&GameProfile> = profiles.iter().filter(|p| is_match(p)).collect();
    match found.len() {
        0 => Lookup::NotFound,
        1 => Lookup::Found(found.remove(0)),
        _ => Lookup::Ambiguous(found),
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::{prop_assert, proptest};

    use super::*;

    const MINIMAL: &str = r#"
schema = 1
id = "test-game"
name = "Test Game"

[exe]
path = "bin/game.exe"
"#;

    fn minimal() -> GameProfile {
        GameProfile::parse(MINIMAL, Path::new("test-game.toml")).unwrap()
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn profile(id: &str, name: &str, appid: Option<u32>) -> GameProfile {
        GameProfile {
            id: id.to_owned(),
            name: name.to_owned(),
            steam: appid.map(|appid| SteamRef { appid }),
            ..minimal()
        }
    }

    // ----- parse -----

    #[test]
    fn parse_fills_defaults() {
        let profile = minimal();
        assert_eq!(profile.id, "test-game");
        assert_eq!(profile.steam, None);
        assert_eq!(profile.exe.bitness, None);
        assert!(!profile.exe.geometry_shaders);
        assert_eq!(profile.graphics, ProfileGraphics::default());
        assert_eq!(profile.compat.status, CompatStatus::Untested);
        assert_eq!(profile.launch_mode(), LaunchMode::Standalone);
        assert!(profile.problems().is_empty(), "{:?}", profile.problems());
    }

    #[test]
    fn parse_reads_every_section() {
        let text = r#"
schema = 1
id = "full"
name = "Full"
[steam]
appid = 42
[exe]
path = "Game.exe"
bitness = "x64"
api = "d3d11"
[graphics]
backend = "d3dmetal"
fallbacks = ["wined3d"]
[wine]
windows_version = "win11"
msync = false
[performance]
retina = true
[env]
A_B = "1"
[dll_overrides]
xinput1_3 = "n,b"
[launch]
args = ["-windowed"]
mode = "applaunch"
[[ini]]
file = '%INSTALLDIR%\settings.ini'
section = "Video"
key = "Fullscreen"
value = "0"
[compat]
status = "playable"
[[compat.reports]]
date = "2026-10-01"
uncork = "0.1.0"
macos = "27.0"
chip = "M5"
backend = "d3dmetal"
status = "playable"
"#;
        let profile = GameProfile::parse(text, Path::new("full.toml")).unwrap();
        assert_eq!(profile.steam, Some(SteamRef { appid: 42 }));
        assert_eq!(profile.exe.api, Some(GraphicsApi::D3d11));
        assert_eq!(profile.graphics.backend, Some(Backend::D3dmetal));
        assert_eq!(profile.wine.windows_version, Some(WindowsVersion::Win11));
        assert_eq!(profile.launch_mode(), LaunchMode::Applaunch);
        assert_eq!(profile.compat.reports.len(), 1);
        assert!(profile.problems().is_empty(), "{:?}", profile.problems());
    }

    #[test]
    fn parse_errors_name_the_file_and_line() {
        let err = GameProfile::parse("schema = 1\nid = \"x\"\nname = 3\n", Path::new("/p/x.toml"))
            .unwrap_err();
        let crate::Error::Config {
            what,
            path,
            message,
        } = err
        else {
            panic!("expected a Config error, got {err:?}");
        };
        assert_eq!(what, "profile");
        assert_eq!(path, Path::new("/p/x.toml"));
        assert!(message.starts_with("line 3: "), "{message}");
        assert!(!message.contains('\n'), "{message}");
    }

    #[test]
    fn parse_rejects_unknown_and_missing_fields() {
        let unknown = format!("{MINIMAL}\n[graphics]\nbackend = \"dxmt\"\nfallback = []\n");
        assert!(GameProfile::parse(&unknown, Path::new("x.toml")).is_err());
        let missing = MINIMAL.replace("[exe]\npath = \"bin/game.exe\"\n", "");
        let err = GameProfile::parse(&missing, Path::new("x.toml")).unwrap_err();
        assert!(err.to_string().contains("exe"), "{err}");
    }

    #[test]
    fn parse_rejects_unknown_backends() {
        let text = format!("{MINIMAL}\n[graphics]\nbackend = \"metal\"\n");
        let err = GameProfile::parse(&text, Path::new("x.toml")).unwrap_err();
        assert!(err.to_string().contains("unknown variant `metal`"), "{err}");
    }

    // ----- problems -----

    fn problems_after(change: impl FnOnce(&mut GameProfile)) -> Vec<String> {
        let mut profile = minimal();
        change(&mut profile);
        profile.problems()
    }

    fn assert_one_problem(change: impl FnOnce(&mut GameProfile), expected: &str) {
        let problems = problems_after(change);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains(expected),
            "{:?} should mention {expected:?}",
            problems[0]
        );
    }

    #[test]
    fn schema_must_be_one() {
        assert_one_problem(|p| p.schema = 2, "schema is 2");
    }

    #[test]
    fn ids_must_be_kebab_case() {
        let too_long = "a".repeat(65);
        for id in [
            "",
            "Game",
            "game_1",
            "-game",
            "game-",
            "game--x",
            "gäme",
            too_long.as_str(),
        ] {
            assert_one_problem(|p| p.id = id.to_owned(), "kebab-case");
        }
        let longest = "a".repeat(64);
        for id in ["a", "rise-of-nations", "aoe2", "x-1-2", longest.as_str()] {
            assert!(problems_after(|p| p.id = id.to_owned()).is_empty(), "{id}");
        }
    }

    #[test]
    fn names_must_not_be_empty() {
        assert_one_problem(|p| p.name = "  ".to_owned(), "name is empty");
    }

    #[test]
    fn exe_paths_must_be_relative_and_slash_separated() {
        for (path, why) in [
            ("", "empty"),
            ("/Games/x.exe", "absolute"),
            ("C:/Games/x.exe", "absolute"),
            (r"C:\Games\x.exe", "absolute"),
            (r"bin\x.exe", "'\\'"),
            ("../x.exe", "'..'"),
            ("bin/../../x.exe", "'..'"),
        ] {
            assert_one_problem(|p| p.exe.path = path.to_owned(), why);
        }
        for path in ["x.exe", "bin/x.exe", "./x.exe", "a..b/x.exe"] {
            assert!(
                problems_after(|p| p.exe.path = path.to_owned()).is_empty(),
                "{path}"
            );
        }
    }

    #[test]
    fn env_keys_must_be_variable_names() {
        for key in ["", "1X", "A-B", "A B", "A=B"] {
            assert_one_problem(
                |p| {
                    p.env.insert(key.to_owned(), "1".to_owned());
                },
                "env key",
            );
        }
        for key in ["_", "A", "WINE_LARGE_ADDRESS_AWARE", "x9"] {
            let problems = problems_after(|p| {
                p.env.insert(key.to_owned(), "1".to_owned());
            });
            assert!(problems.is_empty(), "{key}: {problems:?}");
        }
    }

    #[test]
    fn dll_override_values_must_be_wine_load_orders() {
        let set = |value: &'static str| {
            move |p: &mut GameProfile| {
                p.dll_overrides
                    .insert("d3dcompiler_47".to_owned(), value.to_owned());
            }
        };
        for value in ["n", "b", "n,b", "b,n", "d", ""] {
            assert!(problems_after(set(value)).is_empty(), "{value:?}");
        }
        for value in ["native", "N", "n, b", "x"] {
            assert_one_problem(set(value), "dll_overrides.d3dcompiler_47");
        }
    }

    #[test]
    fn the_preferred_backend_is_not_a_fallback() {
        assert_one_problem(
            |p| {
                p.graphics.backend = Some(Backend::Dxmt);
                p.graphics.fallbacks = vec![Backend::Wined3d, Backend::Dxmt];
            },
            "repeats the preferred backend dxmt",
        );
    }

    fn declare(p: &mut GameProfile, bitness: Option<Bitness>, api: GraphicsApi, backend: Backend) {
        p.exe.bitness = bitness;
        p.exe.api = Some(api);
        p.graphics.backend = Some(backend);
    }

    #[test]
    fn listed_backends_must_run_the_declared_executable() {
        assert_one_problem(
            |p| declare(p, Some(Bitness::X86), GraphicsApi::D3d11, Backend::D3dmetal),
            "d3dmetal cannot run 32-bit D3D11",
        );
        assert_one_problem(
            |p| declare(p, Some(Bitness::X64), GraphicsApi::D3d9, Backend::Dxvk),
            "dxvk cannot run 64-bit D3D9",
        );
        assert_one_problem(
            |p| declare(p, None, GraphicsApi::D3d12, Backend::Dxmt),
            "dxmt cannot run D3D12",
        );
        assert_one_problem(
            |p| {
                declare(p, Some(Bitness::X86), GraphicsApi::D3d11, Backend::Dxmt);
                p.graphics.fallbacks = vec![Backend::Wined3d, Backend::D3dmetal];
            },
            "d3dmetal cannot run 32-bit D3D11",
        );
    }

    #[test]
    fn backends_are_checked_only_against_what_is_declared() {
        let undeclared_bitness = problems_after(|p| {
            p.exe.api = Some(GraphicsApi::D3d11);
            p.graphics.backend = Some(Backend::D3dmetal);
        });
        assert!(
            undeclared_bitness.is_empty(),
            "D3DMetal runs 64-bit D3D11: {undeclared_bitness:?}"
        );
        let undeclared_api = problems_after(|p| {
            p.exe.bitness = Some(Bitness::X86);
            p.graphics.backend = Some(Backend::D3dmetal);
        });
        assert!(undeclared_api.is_empty(), "{undeclared_api:?}");
    }

    #[test]
    fn dxvk_is_not_listed_for_geometry_shader_games() {
        for (backend, fallbacks) in [
            (Backend::Dxvk, vec![]),
            (Backend::Dxmt, vec![Backend::Dxvk]),
        ] {
            assert_one_problem(
                |p| {
                    p.exe.geometry_shaders = true;
                    p.graphics.backend = Some(backend);
                    p.graphics.fallbacks = fallbacks;
                },
                "geometry shaders",
            );
        }
    }

    #[test]
    fn steam_launch_modes_need_a_steam_section() {
        assert_one_problem(
            |p| p.launch.mode = Some(LaunchMode::Direct),
            r#"launch.mode = "direct""#,
        );
        assert_one_problem(
            |p| p.launch.mode = Some(LaunchMode::Applaunch),
            r#"launch.mode = "applaunch""#,
        );
        assert!(problems_after(|p| p.launch.mode = Some(LaunchMode::Standalone)).is_empty());
        let with_steam = problems_after(|p| {
            p.launch.mode = Some(LaunchMode::Direct);
            p.steam = Some(SteamRef { appid: 1 });
        });
        assert!(with_steam.is_empty(), "{with_steam:?}");
    }

    fn ini(file: &str) -> ProfileIni {
        ProfileIni {
            file: file.to_owned(),
            section: "S".to_owned(),
            key: "K".to_owned(),
            value: "V".to_owned(),
            reason: String::new(),
        }
    }

    #[test]
    fn ini_files_start_at_a_known_base() {
        for file in [
            r"%APPDATA%\Game\game.ini",
            "%LOCALAPPDATA%/Game/game.ini",
            r"%USERPROFILE%\Documents\My Games\game.ini",
            r"%INSTALLDIR%\settings.ini",
        ] {
            assert!(
                problems_after(|p| p.ini.push(ini(file))).is_empty(),
                "{file}"
            );
        }
        for (file, why) in [
            (r"C:\game.ini", "does not start with"),
            (r"%WINDIR%\game.ini", "does not start with"),
            (r"%appdata%\game.ini", "does not start with"),
            ("%APPDATA%game.ini", "does not start with"),
            ("game.ini", "does not start with"),
            (r"%APPDATA%\..\..\game.ini", "'..'"),
            ("%INSTALLDIR%/a/../../b.ini", "'..'"),
        ] {
            assert_one_problem(|p| p.ini.push(ini(file)), why);
        }
    }

    #[test]
    fn every_problem_is_reported() {
        let problems = problems_after(|p| {
            p.schema = 0;
            p.id = "Bad Id".to_owned();
            p.name = String::new();
            p.exe.path = String::new();
        });
        assert_eq!(problems.len(), 4, "{problems:?}");
    }

    proptest! {
        #[test]
        fn generated_kebab_ids_are_accepted(id in "[a-z0-9]{1,8}(-[a-z0-9]{1,8}){0,5}") {
            prop_assert!(is_kebab_id(&id));
        }

        #[test]
        fn ids_with_other_characters_are_rejected(id in "[a-z0-9-]{0,8}[^a-z0-9-][a-z0-9-]{0,8}") {
            prop_assert!(!is_kebab_id(&id));
        }

        #[test]
        fn generated_env_names_are_accepted(key in "[A-Za-z_][A-Za-z0-9_]{0,20}") {
            prop_assert!(is_env_name(&key));
        }
    }

    // ----- built-in profiles -----

    #[test]
    fn builtin_profiles_are_valid_and_named_after_their_files() {
        let profiles = builtin();
        assert_eq!(profiles.len(), BUILTIN_FILES.len());
        assert!(!profiles.is_empty());
        for ((file, _), profile) in BUILTIN_FILES.iter().zip(&profiles) {
            assert_eq!(
                Some(profile.id.as_str()),
                file.strip_suffix(".toml"),
                "{file}"
            );
            assert!(
                profile.problems().is_empty(),
                "{file}: {:?}",
                profile.problems()
            );
        }
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn builtin_profiles_include_the_first_targets() {
        let profiles = builtin();
        let ron = resolve(&profiles, "287450").expect("Rise of Nations by app id");
        assert_eq!(ron.id, "rise-of-nations-extended-edition");
        assert_eq!(ron.exe.bitness, Some(Bitness::X86));
        assert!(ron.exe.geometry_shaders);
        assert_eq!(ron.graphics.backend, Some(Backend::Dxmt));
        assert_eq!(ron.launch_mode(), LaunchMode::Direct);
        let aoe2 = resolve(&profiles, "813780").expect("AoE II by app id");
        assert_eq!(aoe2.id, "age-of-empires-2-definitive-edition");
    }

    // ----- load_all -----

    #[test]
    fn load_all_without_a_user_directory_is_the_builtins() {
        let home = tempfile::tempdir().unwrap();
        let (profiles, errors) = load_all(&Layout::at(home.path())).unwrap();
        assert_eq!(profiles, builtin());
        assert!(errors.is_empty());
    }

    #[test]
    fn user_profiles_replace_and_extend_the_builtins() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::at(home.path());
        let (_, ron_text) = BUILTIN_FILES
            .iter()
            .find(|(file, _)| file.starts_with("rise-of-nations"))
            .unwrap();
        let ron = ron_text.replace(
            r#"name = "Rise of Nations: Extended Edition""#,
            r#"name = "My RoN""#,
        );
        write(
            &layout
                .profiles_dir()
                .join("rise-of-nations-extended-edition.toml"),
            &ron,
        );
        write(
            &layout.profiles_dir().join("aaa-first.toml"),
            &MINIMAL.replace("test-game", "aaa-first"),
        );

        let (profiles, errors) = load_all(&layout).unwrap();

        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(profiles.len(), BUILTIN_FILES.len() + 1);
        assert_eq!(profiles[0].id, "aaa-first");
        let ron = resolve(&profiles, "rise-of-nations-extended-edition").unwrap();
        assert_eq!(ron.name, "My RoN");
        let ids: Vec<&str> = profiles.iter().map(|p| p.id.as_str()).collect();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn invalid_user_profiles_are_reported_not_fatal() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::at(home.path());
        let dir = layout.profiles_dir();
        let bad_schema = MINIMAL
            .replace("test-game", "bad-schema")
            .replace("schema = 1", "schema = 9");
        write(&dir.join("broken.toml"), "schema = ");
        write(&dir.join("bad-schema.toml"), &bad_schema);
        write(&dir.join("renamed.toml"), MINIMAL);
        write(
            &dir.join("good.toml"),
            &MINIMAL.replace("test-game", "good"),
        );
        write(&dir.join("notes.txt"), "not a profile");
        write(&dir.join("._good.toml"), "\0\0 macOS metadata");
        std::fs::create_dir_all(dir.join("folder.toml")).unwrap();

        let (profiles, errors) = load_all(&layout).unwrap();

        assert!(profiles.iter().any(|p| p.id == "good"));
        assert!(
            !profiles
                .iter()
                .any(|p| p.id == "bad-schema" || p.id == "test-game")
        );
        let mut failed: Vec<(PathBuf, String)> = errors
            .into_iter()
            .map(|err| match err {
                crate::Error::Config {
                    what: "profile",
                    path,
                    message,
                } => (path, message),
                other => panic!("unexpected error {other:?}"),
            })
            .collect();
        failed.sort();
        assert_eq!(failed.len(), 3, "{failed:?}");
        assert_eq!(failed[0].0, dir.join("bad-schema.toml"));
        assert!(failed[0].1.contains("schema is 9"), "{}", failed[0].1);
        assert_eq!(failed[1].0, dir.join("broken.toml"));
        assert_eq!(failed[2].0, dir.join("renamed.toml"));
        assert!(
            failed[2].1.contains("does not match the file name"),
            "{}",
            failed[2].1
        );
    }

    #[test]
    fn an_unlistable_profiles_directory_is_an_error() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::at(home.path());
        write(&layout.profiles_dir(), "a file, not a directory");
        let err = load_all(&layout).unwrap_err();
        assert!(
            matches!(&err, crate::Error::Io { path, .. } if path == &layout.profiles_dir()),
            "{err:?}"
        );
    }

    // ----- resolve -----

    fn library() -> Vec<GameProfile> {
        vec![
            profile(
                "age-of-empires-2-definitive-edition",
                "Age of Empires II: Definitive Edition",
                Some(813_780),
            ),
            profile(
                "age-of-mythology-retold",
                "Age of Mythology: Retold",
                Some(1_934_680),
            ),
            profile(
                "rise-of-nations-extended-edition",
                "Rise of Nations: Extended Edition",
                Some(287_450),
            ),
            profile("rise", "Rise", None),
            profile("287450", "A game whose id is digits", None),
        ]
    }

    #[test]
    fn resolve_follows_the_documented_precedence() {
        let profiles = library();
        let id = |query: &str| resolve(&profiles, query).map(|p| p.id.as_str());
        assert_eq!(
            id("rise"),
            Some("rise"),
            "exact id beats an ambiguous substring"
        );
        assert_eq!(id("287450"), Some("287450"), "exact id beats app id");
        assert_eq!(
            id("813780"),
            Some("age-of-empires-2-definitive-edition"),
            "app id"
        );
        assert_eq!(
            id("RISE OF NATIONS: EXTENDED EDITION"),
            Some("rise-of-nations-extended-edition"),
            "case-insensitive name"
        );
        assert_eq!(
            id("mythology"),
            Some("age-of-mythology-retold"),
            "unique substring of the name"
        );
        assert_eq!(
            id("of-nations"),
            Some("rise-of-nations-extended-edition"),
            "unique substring of the id"
        );
        assert_eq!(
            id("  Retold "),
            Some("age-of-mythology-retold"),
            "surrounding space is ignored"
        );
        assert_eq!(
            id("4"),
            Some("287450"),
            "digits that are no app id fall through to substrings"
        );
    }

    #[test]
    fn resolve_refuses_to_guess() {
        let profiles = library();
        assert!(
            resolve(&profiles, "age of").is_none(),
            "ambiguous substring"
        );
        assert!(
            resolve(&profiles, "2").is_none(),
            "digits found in two profiles"
        );
        assert!(resolve(&profiles, "civilization").is_none());
        assert!(resolve(&profiles, "").is_none());
        assert!(resolve(&profiles, "   ").is_none());
        assert!(resolve(&[], "rise").is_none());

        let twins = vec![profile("a", "Same", Some(7)), profile("b", "Same", Some(7))];
        assert!(resolve(&twins, "7").is_none(), "ambiguous app id");
        assert!(resolve(&twins, "same").is_none(), "ambiguous name");
        assert_eq!(resolve(&twins, "b").map(|p| p.id.as_str()), Some("b"));
    }

    #[test]
    fn lookup_tells_ambiguity_from_no_match() {
        let twins = vec![
            profile("ron", "Rise of Nations", Some(287_450)),
            profile("ron-wined3d", "Rise of Nations on WineD3D", Some(287_450)),
        ];
        let ids = |lookup: Lookup<'_>| -> Vec<String> {
            match lookup {
                Lookup::Ambiguous(found) => found.iter().map(|p| p.id.clone()).collect(),
                other => panic!("expected an ambiguous lookup, got {other:?}"),
            }
        };
        assert_eq!(ids(lookup(&twins, "287450")), ["ron", "ron-wined3d"]);
        assert_eq!(ids(lookup_appid(&twins, 287_450)), ["ron", "ron-wined3d"]);
        assert_eq!(ids(lookup(&twins, "nations")), ["ron", "ron-wined3d"]);
        assert_eq!(
            lookup(&twins, "ron").found().map(|p| p.id.as_str()),
            Some("ron")
        );
        assert_eq!(lookup(&twins, "civilization"), Lookup::NotFound);
        assert_eq!(lookup_appid(&twins, 70), Lookup::NotFound);
        assert_eq!(
            lookup_appid(&library(), 813_780)
                .found()
                .map(|p| p.id.as_str()),
            Some("age-of-empires-2-definitive-edition")
        );
        assert_eq!(
            lookup_appid(&library(), 287_450)
                .found()
                .map(|p| p.id.as_str()),
            Some("rise-of-nations-extended-edition"),
            "only the app id counts, not an id made of digits"
        );
    }

    #[test]
    fn resolve_ignores_digit_strings_too_long_for_an_app_id() {
        assert!(resolve(&library(), "99999999999999999999").is_none());
    }
}
