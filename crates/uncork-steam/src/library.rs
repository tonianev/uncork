//! Steam libraries and installed apps.
//!
//! Files read (all text KeyValues, see [`crate::vdf`]):
//!
//! - `<steam>/steamapps/libraryfolders.vdf`: root key `libraryfolders`, one
//!   child object per library keyed `"0"`, `"1"`, ... with a `path` string
//!   (Windows path, backslashes escaped in the file) and an `apps` object
//!   mapping appid → size. Older files (before 2021) put bare paths as string
//!   values (`"1" "D:\\Games"`); accept both shapes.
//! - `<library>/steamapps/appmanifest_<appid>.acf`: root key `AppState` with
//!   `appid`, `name`, `installdir`, `StateFlags`, optionally `SizeOnDisk`,
//!   `buildid`, `LastUpdated`.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::vdf;

/// Errors reading Steam library data.
#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    /// File could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// File is not valid KeyValues.
    #[error("{path}: {source}")]
    Parse {
        /// File that failed.
        path: PathBuf,
        /// Parser error.
        source: vdf::ParseError,
    },
    /// File parsed but a required key is missing or not a number.
    #[error("{path}: {message}")]
    Schema {
        /// File that failed.
        path: PathBuf,
        /// What is missing.
        message: String,
    },
}

/// One entry of `libraryfolders.vdf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryFolder {
    /// The Windows path exactly as Steam stored it (unescaped).
    pub path: String,
    /// App ids listed under `apps`, sorted. Empty for the old format.
    pub apps: Vec<u32>,
}

/// Parse `libraryfolders.vdf` text. Entries without a `path` are skipped.
///
/// # Errors
/// [`vdf::ParseError`] for malformed text; a missing `libraryfolders` root key
/// yields an empty list, not an error.
pub fn parse_library_folders(text: &str) -> Result<Vec<LibraryFolder>, vdf::ParseError> {
    let _ = text;
    todo!()
}

/// `StateFlags` bit meaning "fully installed" (`k_EAppStateFullyInstalled`).
pub const STATE_FULLY_INSTALLED: u32 = 4;
/// `StateFlags` bit meaning "update required".
pub const STATE_UPDATE_REQUIRED: u32 = 2;

/// The interesting fields of an `appmanifest_<appid>.acf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AppManifest {
    /// Steam app id.
    pub appid: u32,
    /// Display name.
    pub name: String,
    /// Directory name under `<library>/steamapps/common/`.
    pub installdir: String,
    /// Raw `StateFlags`.
    pub state_flags: u32,
    /// `SizeOnDisk` in bytes, if present and numeric.
    pub size_on_disk: Option<u64>,
    /// `buildid`, if present and numeric.
    pub build_id: Option<u64>,
}

impl AppManifest {
    /// `StateFlags` has [`STATE_FULLY_INSTALLED`] set.
    #[must_use]
    pub fn is_fully_installed(&self) -> bool {
        self.state_flags & STATE_FULLY_INSTALLED != 0
    }
}

/// Parse appmanifest text. `appid`, `name` and `installdir` are required;
/// `StateFlags` defaults to 0 when absent.
///
/// The error's `path` is left empty; [`SteamInstall`] fills it in.
///
/// # Errors
/// [`LibraryError::Parse`] or [`LibraryError::Schema`].
pub fn parse_app_manifest(text: &str) -> Result<AppManifest, LibraryError> {
    let _ = text;
    todo!()
}

/// An installed app with its resolved location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledApp {
    /// The parsed manifest.
    pub manifest: AppManifest,
    /// macOS path of the library root that holds it (the directory containing `steamapps`).
    pub library: PathBuf,
    /// `library/steamapps/common/<installdir>`.
    pub install_path: PathBuf,
}

/// A Windows Steam client installed inside a Wine prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SteamInstall {
    /// The Wine prefix (contains `drive_c`).
    pub prefix: PathBuf,
    /// macOS path of the Steam directory, normally
    /// `<prefix>/drive_c/Program Files (x86)/Steam`.
    pub root: PathBuf,
}

impl SteamInstall {
    /// Locate Steam in `prefix` at the default path ([`crate::client::STEAM_DIR`]).
    /// Returns `None` when `steam.exe` is not there.
    #[must_use]
    pub fn find(prefix: &Path) -> Option<SteamInstall> {
        let _ = prefix;
        todo!()
    }

    /// `root/steam.exe`.
    #[must_use]
    pub fn exe(&self) -> PathBuf {
        self.root.join("steam.exe")
    }

    /// Library roots as macOS paths: `root` first, then every path in
    /// `steamapps/libraryfolders.vdf` mapped with
    /// [`crate::paths::windows_to_unix`], deduplicated, in file order.
    /// A missing `libraryfolders.vdf` yields just `root`.
    ///
    /// # Errors
    /// [`LibraryError`] if `libraryfolders.vdf` exists but cannot be read or parsed.
    pub fn libraries(&self) -> Result<Vec<PathBuf>, LibraryError> {
        todo!()
    }

    /// Every `appmanifest_*.acf` in every library, sorted by app id.
    /// Unreadable or malformed manifests are returned in the second vector
    /// instead of failing the whole listing.
    ///
    /// # Errors
    /// Only if [`SteamInstall::libraries`] fails.
    pub fn installed_apps(&self) -> Result<(Vec<InstalledApp>, Vec<LibraryError>), LibraryError> {
        todo!()
    }

    /// The installed app with this id, if any.
    ///
    /// # Errors
    /// As [`SteamInstall::installed_apps`].
    pub fn find_app(&self, appid: u32) -> Result<Option<InstalledApp>, LibraryError> {
        let _ = appid;
        todo!()
    }
}
