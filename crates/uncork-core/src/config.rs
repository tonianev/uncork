//! Global settings in `config.toml`.
//!
//! ```toml
//! schema = 1
//! default_bottle = "steam"     # used when --bottle is omitted
//! ```

use std::ffi::OsString;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::paths::Layout;

/// Name of the bottle Uncork creates for Steam and uses by default.
pub const DEFAULT_BOTTLE: &str = "steam";

/// Parsed `config.toml`. Unknown keys are rejected so typos surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Bottle used when a command does not name one.
    #[serde(default = "default_bottle")]
    pub default_bottle: String,
}

fn default_bottle() -> String {
    DEFAULT_BOTTLE.to_owned()
}

impl Default for Config {
    fn default() -> Config {
        Config {
            schema: 1,
            default_bottle: default_bottle(),
        }
    }
}

impl Config {
    /// Read `config.toml`; a missing file yields [`Config::default`].
    ///
    /// # Errors
    /// [`crate::Error::Config`] for invalid TOML, [`crate::Error::Io`] otherwise.
    pub fn load(layout: &Layout) -> crate::Result<Config> {
        match read_toml(&layout.config_file(), "config") {
            Err(Error::Io { source, .. }) if source.kind() == io::ErrorKind::NotFound => {
                Ok(Config::default())
            }
            result => result,
        }
    }

    /// Write atomically (temp file in the same directory, then rename).
    ///
    /// # Errors
    /// [`crate::Error::Io`].
    pub fn save(&self, layout: &Layout) -> crate::Result<()> {
        let root = layout.root();
        fs::create_dir_all(root).map_err(|e| Error::io("cannot create directory", root, e))?;
        write_toml_atomic(&layout.config_file(), self)
    }
}

/// Serialize `value` to TOML and write it atomically to `path` (temp file
/// `.<name>.tmp` in the same directory, fsync, rename). Shared by every
/// module that writes TOML.
///
/// # Errors
/// [`crate::Error::Io`].
pub fn write_toml_atomic<T: Serialize>(path: &Path, value: &T) -> crate::Result<()> {
    let text = toml::to_string(value).map_err(|e| {
        Error::io(
            "cannot serialize TOML for",
            path,
            io::Error::new(io::ErrorKind::InvalidData, e),
        )
    })?;
    write_atomic(path, text.as_bytes())
}

/// Read and deserialize a TOML file. `what` names the file kind in errors.
///
/// # Errors
/// [`crate::Error::Io`] or [`crate::Error::Config`].
pub fn read_toml<T: serde::de::DeserializeOwned>(
    path: &Path,
    what: &'static str,
) -> crate::Result<T> {
    let invalid = |message: String| Error::Config {
        what,
        path: path.to_path_buf(),
        message,
    };
    let bytes = fs::read(path).map_err(|e| Error::io("cannot read", path, e))?;
    let text = String::from_utf8(bytes).map_err(|e| invalid(format!("not valid UTF-8: {e}")))?;
    toml::from_str(&text).map_err(|e| invalid(e.to_string().trim_end().to_owned()))
}

/// Replace `path` with `contents` atomically: write `.<name>.tmp` next to
/// it, fsync, then rename it over `path`, so readers see either the old file
/// or the new one, never a torn write. A replaced file keeps its permissions.
///
/// # Errors
/// [`Error::Io`] naming the file that could not be written. The temporary
/// file is removed on failure.
pub(crate) fn write_atomic(path: &Path, contents: &[u8]) -> crate::Result<()> {
    let temp = temp_path(path)?;
    // A leftover from an interrupted write may be read-only (permissions are
    // copied from the target), which would make `File::create` fail.
    let _ = fs::remove_file(&temp);
    let result = write_temp(&temp, path, contents)
        .and_then(|()| fs::rename(&temp, path).map_err(|e| Error::io("cannot replace", path, e)));
    if result.is_err() {
        // Best effort: the error being returned is the one worth reporting.
        let _ = fs::remove_file(&temp);
    }
    result
}

/// `dir/.<name>.tmp` for `dir/<name>`.
fn temp_path(path: &Path) -> crate::Result<PathBuf> {
    let name = path.file_name().ok_or_else(|| {
        Error::io(
            "cannot write",
            path,
            io::Error::new(io::ErrorKind::InvalidInput, "the path does not name a file"),
        )
    })?;
    let mut temp_name = OsString::from(".");
    temp_name.push(name);
    temp_name.push(".tmp");
    Ok(path.with_file_name(temp_name))
}

fn write_temp(temp: &Path, target: &Path, contents: &[u8]) -> crate::Result<()> {
    let mut file = File::create(temp).map_err(|e| Error::io("cannot create", temp, e))?;
    file.write_all(contents)
        .map_err(|e| Error::io("cannot write", temp, e))?;
    if let Ok(existing) = fs::metadata(target) {
        file.set_permissions(existing.permissions())
            .map_err(|e| Error::io("cannot set permissions of", temp, e))?;
    }
    file.sync_all()
        .map_err(|e| Error::io("cannot sync", temp, e))
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::fs::PermissionsExt as _;

    use proptest::prelude::*;

    use super::*;

    fn temp_layout() -> (tempfile::TempDir, Layout) {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::at(temp.path().join("uncork"));
        (temp, layout)
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn missing_config_loads_as_default() {
        let (_temp, layout) = temp_layout();
        assert_eq!(Config::load(&layout).unwrap(), Config::default());
        assert_eq!(Config::default().default_bottle, DEFAULT_BOTTLE);
    }

    #[test]
    fn save_creates_the_root_and_round_trips() {
        let (_temp, layout) = temp_layout();
        let config = Config {
            schema: 1,
            default_bottle: "games".to_owned(),
        };
        config.save(&layout).unwrap();
        assert_eq!(Config::load(&layout).unwrap(), config);
        assert_eq!(file_names(layout.root()), ["config.toml"]);
    }

    #[test]
    fn omitted_default_bottle_uses_the_default() {
        let (_temp, layout) = temp_layout();
        fs::create_dir_all(layout.root()).unwrap();
        fs::write(layout.config_file(), "schema = 1\n").unwrap();
        assert_eq!(Config::load(&layout).unwrap(), Config::default());
    }

    #[test]
    fn unknown_keys_are_rejected_with_the_path() {
        let (_temp, layout) = temp_layout();
        fs::create_dir_all(layout.root()).unwrap();
        fs::write(layout.config_file(), "schema = 1\ndefault_botle = \"x\"\n").unwrap();
        match Config::load(&layout).unwrap_err() {
            Error::Config {
                what,
                path,
                message,
            } => {
                assert_eq!(what, "config");
                assert_eq!(path, layout.config_file());
                assert!(message.contains("default_botle"), "{message}");
            }
            other => panic!("expected Error::Config, got {other:?}"),
        }
    }

    #[test]
    fn malformed_toml_is_a_config_error() {
        let (_temp, layout) = temp_layout();
        fs::create_dir_all(layout.root()).unwrap();
        fs::write(layout.config_file(), "schema = = 1\n").unwrap();
        let err = Config::load(&layout).unwrap_err();
        assert!(matches!(err, Error::Config { .. }), "{err:?}");
        assert!(!err.to_string().ends_with('\n'));
    }

    #[test]
    fn non_utf8_toml_is_a_config_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bad.toml");
        fs::write(&path, b"schema = 1\n# \xff\n").unwrap();
        let err = read_toml::<Config>(&path, "config").unwrap_err();
        assert!(
            matches!(&err, Error::Config { message, .. } if message.contains("UTF-8")),
            "{err:?}"
        );
    }

    #[test]
    fn unreadable_config_is_an_io_error() {
        let (_temp, layout) = temp_layout();
        // A directory where the file should be: present, but not readable as a file.
        fs::create_dir_all(layout.config_file()).unwrap();
        let err = Config::load(&layout).unwrap_err();
        assert!(
            matches!(&err, Error::Io { path, .. } if *path == layout.config_file()),
            "{err:?}"
        );
    }

    #[test]
    fn read_toml_reports_a_missing_file_as_io() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("absent.toml");
        let err = read_toml::<Config>(&path, "config").unwrap_err();
        assert!(
            matches!(&err, Error::Io { source, .. } if source.kind() == io::ErrorKind::NotFound),
            "{err:?}"
        );
    }

    #[test]
    fn write_toml_atomic_replaces_and_leaves_no_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("state.toml");
        fs::write(&path, "stale = true\n").unwrap();
        write_toml_atomic(&path, &Config::default()).unwrap();
        assert_eq!(
            read_toml::<Config>(&path, "config").unwrap(),
            Config::default()
        );
        assert_eq!(file_names(temp.path()), ["state.toml"]);
    }

    #[test]
    fn write_toml_atomic_keeps_the_permissions_of_the_replaced_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        fs::write(&path, "schema = 1\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        write_toml_atomic(&path, &Config::default()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn write_toml_atomic_overwrites_a_stale_read_only_temp_file() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        let stale = temp.path().join(".config.toml.tmp");
        fs::write(&stale, "garbage").unwrap();
        fs::set_permissions(&stale, fs::Permissions::from_mode(0o444)).unwrap();
        write_toml_atomic(&path, &Config::default()).unwrap();
        assert_eq!(file_names(temp.path()), ["config.toml"]);
    }

    #[test]
    fn write_toml_atomic_into_a_missing_directory_fails_cleanly() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("missing").join("config.toml");
        let err = write_toml_atomic(&path, &Config::default()).unwrap_err();
        assert!(
            matches!(&err, Error::Io { path: p, .. } if *p == temp.path().join("missing").join(".config.toml.tmp")),
            "{err:?}"
        );
        assert!(!path.exists());
    }

    #[test]
    fn write_toml_atomic_rejects_a_path_without_a_file_name() {
        let err = write_toml_atomic(Path::new("/"), &Config::default()).unwrap_err();
        assert!(
            matches!(&err, Error::Io { source, .. } if source.kind() == io::ErrorKind::InvalidInput),
            "{err:?}"
        );
    }

    #[test]
    fn unserializable_values_are_reported_not_written() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("bad.toml");
        // A bare string is not a TOML document.
        let err = write_toml_atomic(&path, &"just a string").unwrap_err();
        assert!(
            matches!(&err, Error::Io { source, .. } if source.kind() == io::ErrorKind::InvalidData),
            "{err:?}"
        );
        assert!(file_names(temp.path()).is_empty());
    }

    #[test]
    fn write_atomic_writes_exact_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("raw.bin");
        write_atomic(&path, b"\xff\xfe\r\n\0").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"\xff\xfe\r\n\0");
    }

    proptest! {
        #[test]
        fn config_round_trips(schema in any::<u32>(), default_bottle in any::<String>()) {
            let temp = tempfile::tempdir().unwrap();
            let layout = Layout::at(temp.path());
            let config = Config { schema, default_bottle };
            config.save(&layout).unwrap();
            prop_assert_eq!(Config::load(&layout).unwrap(), config);
        }

        #[test]
        fn string_maps_round_trip(map in proptest::collection::btree_map(any::<String>(), any::<String>(), 0..8)) {
            let temp = tempfile::tempdir().unwrap();
            let path = temp.path().join("map.toml");
            write_toml_atomic(&path, &map).unwrap();
            prop_assert_eq!(read_toml::<BTreeMap<String, String>>(&path, "map").unwrap(), map);
        }
    }
}
