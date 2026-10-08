//! Steam libraries and installed apps.
//!
//! Files read (all text `KeyValues`, see [`crate::vdf`]):
//!
//! - `<steam>/steamapps/libraryfolders.vdf`: root key `libraryfolders`, one
//!   child object per library keyed `"0"`, `"1"`, ... with a `path` string
//!   (Windows path, backslashes escaped in the file) and an `apps` object
//!   mapping appid → size. Older files (before 2021) put bare paths as string
//!   values (`"1" "D:\\Games"`); accept both shapes.
//! - `<library>/steamapps/appmanifest_<appid>.acf`: root key `AppState` with
//!   `appid`, `name`, `installdir`, `StateFlags`, optionally `SizeOnDisk`,
//!   `buildid`, `LastUpdated`.

use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Serialize;

use crate::{client, paths, vdf};

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
    /// File is not valid `KeyValues`.
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

impl LibraryError {
    /// The same error, attributed to `file`.
    fn with_path(mut self, file: &Path) -> Self {
        let (LibraryError::Io { path, .. }
        | LibraryError::Parse { path, .. }
        | LibraryError::Schema { path, .. }) = &mut self;
        file.clone_into(path);
        self
    }
}

/// One entry of `libraryfolders.vdf`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LibraryFolder {
    /// The Windows path exactly as Steam stored it (unescaped).
    pub path: String,
    /// App ids listed under `apps`, sorted. Empty for the old format.
    pub apps: Vec<u32>,
}

/// Parse `libraryfolders.vdf` text. Entries without a `path` (or with an
/// empty one) are skipped, as are the old format's non-numeric metadata keys
/// such as `ContentStatsID`. Duplicate app ids are listed once.
///
/// # Errors
/// [`vdf::ParseError`] for malformed text; a missing `libraryfolders` root key
/// yields an empty list, not an error.
pub fn parse_library_folders(text: &str) -> Result<Vec<LibraryFolder>, vdf::ParseError> {
    let document = vdf::parse(text)?;
    let Some(root) = document.get_object("libraryfolders") else {
        return Ok(Vec::new());
    };
    Ok(root
        .iter()
        .filter_map(|(key, value)| library_folder(key, value))
        .collect())
}

fn library_folder(key: &str, value: &vdf::Value) -> Option<LibraryFolder> {
    let folder = match value {
        vdf::Value::Object(entry) => LibraryFolder {
            path: entry.get_str("path")?.to_owned(),
            apps: app_ids(entry.get_object("apps")),
        },
        // Old files mix library paths with metadata strings
        // (`"ContentStatsID" "-1234"`); only numbered keys are libraries.
        vdf::Value::Str(path) if is_decimal(key) => LibraryFolder {
            path: path.clone(),
            apps: Vec::new(),
        },
        vdf::Value::Str(_) => return None,
    };
    (!folder.path.is_empty()).then_some(folder)
}

fn app_ids(apps: Option<&vdf::Object>) -> Vec<u32> {
    let mut ids: Vec<u32> = apps
        .into_iter()
        .flat_map(vdf::Object::iter)
        .filter_map(|(id, _)| parse_decimal(id))
        .collect();
    ids.sort_unstable();
    ids.dedup();
    ids
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
/// `StateFlags` defaults to 0 when absent. Numbers are plain decimal digits.
/// `installdir` must be a single directory name (no separators, not `.` or
/// `..`) so that it cannot point outside `steamapps/common`.
///
/// The error's `path` is left empty; [`SteamInstall`] fills it in.
///
/// # Errors
/// [`LibraryError::Parse`] or [`LibraryError::Schema`].
pub fn parse_app_manifest(text: &str) -> Result<AppManifest, LibraryError> {
    let document = vdf::parse(text).map_err(|source| LibraryError::Parse {
        path: PathBuf::new(),
        source,
    })?;
    let state = document
        .get_object("AppState")
        .ok_or_else(|| schema_error("missing \"AppState\" block"))?;
    let appid = required_number(state, "appid")?;
    let name = required_str(state, "name")?.to_owned();
    let installdir = required_str(state, "installdir")?;
    if !is_plain_dir_name(installdir) {
        return Err(schema_error(format!(
            "\"installdir\" must be a single directory name, found {installdir:?}"
        )));
    }
    Ok(AppManifest {
        appid,
        name,
        installdir: installdir.to_owned(),
        state_flags: optional_number(state, "StateFlags")?.unwrap_or(0),
        size_on_disk: state.get_str("SizeOnDisk").and_then(parse_decimal),
        build_id: state.get_str("buildid").and_then(parse_decimal),
    })
}

fn schema_error(message: impl Into<String>) -> LibraryError {
    LibraryError::Schema {
        path: PathBuf::new(),
        message: message.into(),
    }
}

fn required_str<'a>(state: &'a vdf::Object, key: &str) -> Result<&'a str, LibraryError> {
    optional_str(state, key)?
        .ok_or_else(|| schema_error(format!("missing {key:?} in \"AppState\"")))
}

/// The string under `key`, `None` when absent, an error when it is a block.
fn optional_str<'a>(state: &'a vdf::Object, key: &str) -> Result<Option<&'a str>, LibraryError> {
    match state.get(key) {
        None => Ok(None),
        Some(vdf::Value::Str(value)) => Ok(Some(value)),
        Some(vdf::Value::Object(_)) => Err(schema_error(format!(
            "{key:?} is a block, expected a string"
        ))),
    }
}

fn required_number<T: FromStr>(state: &vdf::Object, key: &str) -> Result<T, LibraryError> {
    optional_number(state, key)?
        .ok_or_else(|| schema_error(format!("missing {key:?} in \"AppState\"")))
}

fn optional_number<T: FromStr>(state: &vdf::Object, key: &str) -> Result<Option<T>, LibraryError> {
    let Some(text) = optional_str(state, key)? else {
        return Ok(None);
    };
    parse_decimal(text)
        .map(Some)
        .ok_or_else(|| schema_error(format!("{key:?} is not a valid number: {text:?}")))
}

fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// Parse plain decimal digits; rejects signs, spaces and overflow.
fn parse_decimal<T: FromStr>(text: &str) -> Option<T> {
    if is_decimal(text) {
        text.parse().ok()
    } else {
        None
    }
}

fn is_plain_dir_name(name: &str) -> bool {
    !matches!(name, "" | "." | "..") && !name.contains(['/', '\\'])
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

impl InstalledApp {
    fn new(manifest: AppManifest, library: &Path) -> Self {
        let install_path = library
            .join("steamapps")
            .join("common")
            .join(&manifest.installdir);
        Self {
            manifest,
            library: library.to_owned(),
            install_path,
        }
    }
}

/// The file in `root` named [`client::STEAM_EXE`] ignoring ASCII case; with
/// several spellings (a case-sensitive volume), the first in byte order.
fn client_exe_in(root: &Path) -> Option<PathBuf> {
    let mut names: Vec<std::ffi::OsString> = fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .filter(|name| {
            name.to_str()
                .is_some_and(|name| name.eq_ignore_ascii_case(client::STEAM_EXE))
        })
        .collect();
    names.sort();
    names
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
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
    /// Returns `None` when the directory has no client executable: a file
    /// named [`crate::client::STEAM_EXE`] in any ASCII case (Valve's
    /// installer writes `Steam.exe`; a case-sensitive volume keeps that
    /// spelling, so the name is matched, not assumed).
    #[must_use]
    pub fn find(prefix: &Path) -> Option<SteamInstall> {
        let root = prefix.join(paths::DRIVE_C).join(client::STEAM_DIR);
        client_exe_in(&root).map(|_| SteamInstall {
            prefix: prefix.to_owned(),
            root,
        })
    }

    /// The client executable with its on-disk name (see
    /// [`SteamInstall::find`]); `root/`[`crate::client::STEAM_EXE`] when it
    /// is gone. Reads the directory on every call.
    #[must_use]
    pub fn exe(&self) -> PathBuf {
        client_exe_in(&self.root).unwrap_or_else(|| self.root.join(client::STEAM_EXE))
    }

    /// Library roots as macOS paths: `root` first, then every path in
    /// `steamapps/libraryfolders.vdf` mapped with
    /// [`crate::paths::windows_to_unix`], deduplicated, in file order.
    /// A missing `libraryfolders.vdf` yields just `root`.
    ///
    /// Paths that cannot be mapped (relative or UNC) are skipped. Duplicates
    /// are detected ignoring ASCII case, as Windows paths are
    /// case-insensitive; the first spelling wins. Libraries are not checked
    /// for existence: a listed drive may be unplugged.
    ///
    /// # Errors
    /// [`LibraryError`] if `libraryfolders.vdf` exists but cannot be read or parsed.
    pub fn libraries(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let mut libraries = vec![self.root.clone()];
        let file = self.root.join("steamapps").join("libraryfolders.vdf");
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(libraries),
            Err(source) => return Err(LibraryError::Io { path: file, source }),
        };
        let folders = parse_library_folders(&text)
            .map_err(|source| LibraryError::Parse { path: file, source })?;
        for folder in folders {
            let Some(library) = paths::windows_to_unix(&self.prefix, &folder.path) else {
                continue;
            };
            let known = libraries
                .iter()
                .any(|known| known.as_os_str().eq_ignore_ascii_case(library.as_os_str()));
            if !known {
                libraries.push(library);
            }
        }
        Ok(libraries)
    }

    /// Every `appmanifest_*.acf` in every library, sorted by app id.
    /// Unreadable or malformed manifests are returned in the second vector
    /// instead of failing the whole listing.
    ///
    /// A library without a `steamapps` directory contributes nothing; any
    /// other failure to list one is reported in the second vector. An app
    /// present in several libraries is listed once per library, in library
    /// order.
    ///
    /// # Errors
    /// Only if [`SteamInstall::libraries`] fails.
    pub fn installed_apps(&self) -> Result<(Vec<InstalledApp>, Vec<LibraryError>), LibraryError> {
        let mut apps = Vec::new();
        let mut errors = Vec::new();
        for library in self.libraries()? {
            let files = match manifest_files(&library.join("steamapps")) {
                Ok(files) => files,
                Err(error) => {
                    errors.push(error);
                    continue;
                }
            };
            for file in files {
                match read_manifest(&file) {
                    Ok(manifest) => apps.push(InstalledApp::new(manifest, &library)),
                    Err(error) => errors.push(error),
                }
            }
        }
        apps.sort_by_key(|app| app.manifest.appid);
        Ok((apps, errors))
    }

    /// The installed app with this id, if any.
    ///
    /// # Errors
    /// As [`SteamInstall::installed_apps`].
    pub fn find_app(&self, appid: u32) -> Result<Option<InstalledApp>, LibraryError> {
        let (apps, _unreadable) = self.installed_apps()?;
        Ok(apps.into_iter().find(|app| app.manifest.appid == appid))
    }
}

/// The manifest files in a `steamapps` directory, sorted by name so that
/// results and errors come out in a stable order.
fn manifest_files(steamapps: &Path) -> Result<Vec<PathBuf>, LibraryError> {
    let io_error = |source| LibraryError::Io {
        path: steamapps.to_owned(),
        source,
    };
    let entries = match fs::read_dir(steamapps) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(io_error(source)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error)?;
        let path = entry.path();
        if is_manifest_name(&entry.file_name()) && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// `appmanifest_*.acf`, ignoring ASCII case like Steam on Windows.
fn is_manifest_name(name: &OsStr) -> bool {
    name.to_str().is_some_and(|name| {
        let name = name.to_ascii_lowercase();
        name.starts_with("appmanifest_") && name.ends_with(".acf")
    })
}

fn read_manifest(file: &Path) -> Result<AppManifest, LibraryError> {
    let text = fs::read_to_string(file).map_err(|source| LibraryError::Io {
        path: file.to_owned(),
        source,
    })?;
    parse_app_manifest(&text).map_err(|error| error.with_path(file))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(body: &str) -> String {
        format!("\"AppState\"\n{{\n{body}\n}}\n")
    }

    const MINIMAL: &str = "\"appid\" \"287450\"\n\"name\" \"Rise of Nations: Extended Edition\"\n\"installdir\" \"Rise of Nations\"";

    fn schema_message(text: &str) -> String {
        match parse_app_manifest(text) {
            Err(LibraryError::Schema { path, message }) => {
                assert_eq!(path, PathBuf::new());
                message
            }
            other => panic!("expected a schema error, got {other:?}"),
        }
    }

    #[test]
    fn parses_new_format_library_folders() {
        let text = r#"
"libraryfolders"
{
	"contentstatsid"		"1234567890"
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"287450"		"3123456789"
			"228980"		"29212173"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
		}
	}
}
"#;
        assert_eq!(
            parse_library_folders(text),
            Ok(vec![
                LibraryFolder {
                    path: r"C:\Program Files (x86)\Steam".to_owned(),
                    apps: vec![228_980, 287_450],
                },
                LibraryFolder {
                    path: r"D:\SteamLibrary".to_owned(),
                    apps: vec![],
                },
            ])
        );
    }

    #[test]
    fn parses_legacy_library_folders() {
        let text = r#"
"LibraryFolders"
{
	"TimeNextStatsReport"		"1561832478"
	"ContentStatsID"		"-1234567890123456789"
	"1"		"D:\\Games\\Steam"
	"2"		"E:\\SteamLibrary"
}
"#;
        let folders = parse_library_folders(text).expect("valid");
        let paths: Vec<&str> = folders.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, [r"D:\Games\Steam", r"E:\SteamLibrary"]);
        assert!(folders.iter().all(|f| f.apps.is_empty()));
    }

    #[test]
    fn library_entries_without_a_usable_path_are_skipped() {
        let text = r#""libraryfolders" { "0" { "label" "x" } "1" { "path" "" } "2" "" "3" { "path" { } } "4" { "path" "E:\\L" } }"#;
        let folders = parse_library_folders(text).expect("valid");
        assert_eq!(
            folders,
            vec![LibraryFolder {
                path: r"E:\L".to_owned(),
                apps: vec![],
            }]
        );
    }

    #[test]
    fn library_app_ids_are_sorted_deduplicated_and_numeric() {
        let text = r#""libraryfolders" { "0" { "path" "C:\\S" "apps" { "30" "1" "10" "2" "x" "3" "-5" "4" "30" "5" "99999999999" "6" } } }"#;
        let folders = parse_library_folders(text).expect("valid");
        assert_eq!(folders[0].apps, [10, 30]);
    }

    #[test]
    fn missing_libraryfolders_root_is_an_empty_list() {
        assert_eq!(parse_library_folders(""), Ok(vec![]));
        assert_eq!(parse_library_folders("\"other\" { }"), Ok(vec![]));
        assert_eq!(
            parse_library_folders("\"libraryfolders\" \"x\""),
            Ok(vec![])
        );
    }

    #[test]
    fn malformed_library_folders_is_a_parse_error() {
        let error = parse_library_folders("\"libraryfolders\"\n{\n").expect_err("malformed");
        assert_eq!(error.message, "unclosed '{' opened at line 2");
    }

    #[test]
    fn parses_a_full_manifest() {
        let text = manifest(
            "\"appid\" \"287450\"\n\"universe\" \"1\"\n\"name\" \"Rise of Nations: Extended Edition\"\n\
             \"StateFlags\" \"4\"\n\"installdir\" \"Rise of Nations\"\n\"SizeOnDisk\" \"3123456789\"\n\
             \"buildid\" \"10000001\"\n\"InstalledDepots\" { \"287451\" { \"manifest\" \"1\" } }",
        );
        assert_eq!(
            parse_app_manifest(&text).expect("valid"),
            AppManifest {
                appid: 287_450,
                name: "Rise of Nations: Extended Edition".to_owned(),
                installdir: "Rise of Nations".to_owned(),
                state_flags: 4,
                size_on_disk: Some(3_123_456_789),
                build_id: Some(10_000_001),
            }
        );
    }

    #[test]
    fn manifest_keys_are_case_insensitive() {
        let text = "\"appstate\" { \"AppID\" \"1\" \"NAME\" \"n\" \"InstallDir\" \"d\" \"stateflags\" \"6\" }";
        let parsed = parse_app_manifest(text).expect("valid");
        assert_eq!((parsed.appid, parsed.state_flags), (1, 6));
    }

    #[test]
    fn state_flags_default_to_zero_and_optionals_to_none() {
        let parsed = parse_app_manifest(&manifest(MINIMAL)).expect("valid");
        assert_eq!(parsed.state_flags, 0);
        assert_eq!((parsed.size_on_disk, parsed.build_id), (None, None));
        assert!(!parsed.is_fully_installed());
    }

    #[test]
    fn non_numeric_optional_sizes_are_ignored() {
        let text = manifest(&format!(
            "{MINIMAL}\n\"SizeOnDisk\" \"big\"\n\"buildid\" {{ }}"
        ));
        let parsed = parse_app_manifest(&text).expect("valid");
        assert_eq!((parsed.size_on_disk, parsed.build_id), (None, None));
    }

    #[test]
    fn fully_installed_checks_only_its_bit() {
        let with_flags = |state_flags| AppManifest {
            appid: 1,
            name: String::new(),
            installdir: "d".to_owned(),
            state_flags,
            size_on_disk: None,
            build_id: None,
        };
        assert!(with_flags(STATE_FULLY_INSTALLED).is_fully_installed());
        assert!(with_flags(STATE_FULLY_INSTALLED | STATE_UPDATE_REQUIRED).is_fully_installed());
        assert!(!with_flags(1026).is_fully_installed());
        assert!(!with_flags(0).is_fully_installed());
    }

    #[test]
    fn required_keys_are_enforced() {
        for (key, body) in [
            ("appid", "\"name\" \"n\"\n\"installdir\" \"d\""),
            ("name", "\"appid\" \"1\"\n\"installdir\" \"d\""),
            ("installdir", "\"appid\" \"1\"\n\"name\" \"n\""),
        ] {
            assert_eq!(
                schema_message(&manifest(body)),
                format!("missing \"{key}\" in \"AppState\"")
            );
        }
    }

    #[test]
    fn missing_appstate_block_is_a_schema_error() {
        assert_eq!(schema_message(""), "missing \"AppState\" block");
        assert_eq!(
            schema_message("\"AppState\" \"x\""),
            "missing \"AppState\" block"
        );
        assert_eq!(
            schema_message("\"libraryfolders\" { }"),
            "missing \"AppState\" block"
        );
    }

    #[test]
    fn non_numeric_required_numbers_are_schema_errors() {
        for (appid, flags) in [
            ("abc", "4"),
            ("-1", "4"),
            ("+1", "4"),
            ("4294967296", "4"),
            ("1", "x"),
            ("1", " 4"),
        ] {
            let body = format!(
                "\"appid\" \"{appid}\"\n\"name\" \"n\"\n\"installdir\" \"d\"\n\"StateFlags\" \"{flags}\""
            );
            let message = schema_message(&manifest(&body));
            let expected = if appid == "1" {
                format!("\"StateFlags\" is not a valid number: {flags:?}")
            } else {
                format!("\"appid\" is not a valid number: {appid:?}")
            };
            assert_eq!(message, expected);
        }
    }

    #[test]
    fn block_where_a_string_is_required_is_a_schema_error() {
        let body = "\"appid\" { }\n\"name\" \"n\"\n\"installdir\" \"d\"";
        assert_eq!(
            schema_message(&manifest(body)),
            "\"appid\" is a block, expected a string"
        );
    }

    #[test]
    fn installdir_must_stay_inside_common() {
        for dir in ["", ".", "..", "../../etc", "/Users/me", r"a\b", "a/b"] {
            let body = format!(
                "\"appid\" \"1\"\n\"name\" \"n\"\n\"installdir\" \"{}\"",
                dir.replace('\\', "\\\\")
            );
            let message = schema_message(&manifest(&body));
            assert!(
                message.starts_with("\"installdir\" must be a single directory name"),
                "{dir:?}: {message}"
            );
        }
    }

    #[test]
    fn malformed_manifest_is_a_parse_error_without_path() {
        match parse_app_manifest("\"AppState\"\n{\n\t\"appid\"\t\t\"28") {
            Err(LibraryError::Parse { path, source }) => {
                assert_eq!(path, PathBuf::new());
                assert_eq!((source.line, source.column), (3, 11));
                assert_eq!(source.message, "unterminated string");
            }
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    #[test]
    fn with_path_fills_in_every_variant() {
        let file = Path::new("/lib/steamapps/appmanifest_1.acf");
        let io = LibraryError::Io {
            path: PathBuf::new(),
            source: io::Error::other("boom"),
        }
        .with_path(file);
        assert!(matches!(&io, LibraryError::Io { path, .. } if path == file));
        let parse = parse_app_manifest("\"").expect_err("bad").with_path(file);
        assert!(matches!(&parse, LibraryError::Parse { path, .. } if path == file));
        let schema = schema_error("m").with_path(file);
        assert_eq!(schema.to_string(), "/lib/steamapps/appmanifest_1.acf: m");
    }

    #[test]
    fn error_messages_name_the_file() {
        let parse = parse_app_manifest("}")
            .expect_err("bad")
            .with_path(Path::new("/x.acf"));
        assert_eq!(
            parse.to_string(),
            "/x.acf: line 1, column 1: unexpected '}'"
        );
        let io = LibraryError::Io {
            path: PathBuf::from("/x.acf"),
            source: io::Error::new(io::ErrorKind::PermissionDenied, "denied"),
        };
        assert_eq!(io.to_string(), "cannot read /x.acf: denied");
    }

    #[test]
    fn manifest_file_names_match_case_insensitively() {
        for name in [
            "appmanifest_287450.acf",
            "AppManifest_1.ACF",
            "appmanifest_.acf",
        ] {
            assert!(is_manifest_name(OsStr::new(name)), "{name}");
        }
        for name in [
            "appmanifest_287450.acf.tmp",
            "appmanifest_287450.vdf",
            "libraryfolders.vdf",
            "manifest_1.acf",
            "",
        ] {
            assert!(!is_manifest_name(OsStr::new(name)), "{name}");
        }
    }

    #[test]
    fn decimal_parsing_is_strict() {
        assert_eq!(parse_decimal::<u32>("0"), Some(0));
        assert_eq!(parse_decimal::<u32>("4294967295"), Some(u32::MAX));
        assert_eq!(parse_decimal::<u64>("18446744073709551615"), Some(u64::MAX));
        for text in ["", "+1", "-1", " 1", "1 ", "0x10", "1.0", "4294967296"] {
            assert_eq!(parse_decimal::<u32>(text), None, "{text:?}");
        }
    }

    #[test]
    fn install_path_is_under_steamapps_common() {
        let manifest = parse_app_manifest(&manifest(MINIMAL)).expect("valid");
        let app = InstalledApp::new(manifest, Path::new("/lib"));
        assert_eq!(app.library, Path::new("/lib"));
        assert_eq!(
            app.install_path,
            Path::new("/lib/steamapps/common/Rise of Nations")
        );
    }
}
