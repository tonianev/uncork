//! Where Uncork keeps its files.
//!
//! ```text
//! $UNCORK_HOME  (default: ~/Library/Application Support/Uncork)
//! ├── config.toml                 global settings (crate::config)
//! ├── components/<kind>/<version>/ installed Wine and graphics components (crate::component)
//! │   └── component.toml          what it is, where it came from, its license
//! ├── bottles/<name>/             one Wine prefix per bottle; WINEPREFIX points here
//! │   ├── uncork.toml             bottle settings (crate::bottle)
//! │   ├── uncork-state.toml       machine-written state (active graphics backend)
//! │   ├── drive_c/ dosdevices/ *.reg   Wine's own files
//! ├── profiles/*.toml             user game profiles; override built-ins by id
//! ├── apps/                       Game Mode launcher bundles (crate::gamemode)
//! ├── cache/downloads/            verified downloads, named <sha256[..16]>-<file>
//! └── logs/                       one log per launch (crate::launch)
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::component::ComponentKind;

/// Environment variable that overrides the data root. Tests and portable
/// installs set it.
pub const HOME_ENV: &str = "UNCORK_HOME";

/// Resolved data directories. Cheap to clone; creates nothing until
/// [`Layout::ensure`] is called.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    root: PathBuf,
}

impl Layout {
    /// `$UNCORK_HOME` if set and non-empty, otherwise
    /// `$HOME/Library/Application Support/Uncork`.
    ///
    /// # Errors
    /// [`crate::Error::NoHome`] when neither variable is set.
    pub fn discover() -> crate::Result<Layout> {
        Layout::from_vars(std::env::var_os(HOME_ENV), std::env::var_os("HOME"))
    }

    /// [`Layout::discover`] over explicit variable values, so the rules are
    /// testable without mutating the process environment. An empty `HOME`
    /// counts as unset: it would turn the data root into a relative path.
    fn from_vars(uncork_home: Option<OsString>, home: Option<OsString>) -> crate::Result<Layout> {
        if let Some(root) = uncork_home.filter(|value| !value.is_empty()) {
            return Ok(Layout::at(root));
        }
        let home = home
            .filter(|value| !value.is_empty())
            .ok_or(crate::Error::NoHome)?;
        Ok(Layout::at(
            PathBuf::from(home)
                .join("Library")
                .join("Application Support")
                .join("Uncork"),
        ))
    }

    /// A layout rooted at `root`.
    #[must_use]
    pub fn at(root: impl Into<PathBuf>) -> Layout {
        Layout { root: root.into() }
    }

    /// The data root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `root/config.toml`.
    #[must_use]
    pub fn config_file(&self) -> PathBuf {
        self.root.join("config.toml")
    }

    /// `root/components`.
    #[must_use]
    pub fn components_dir(&self) -> PathBuf {
        self.root.join("components")
    }

    /// `root/components/<kind>`.
    #[must_use]
    pub fn component_kind_dir(&self, kind: ComponentKind) -> PathBuf {
        self.components_dir().join(kind.as_str())
    }

    /// `root/components/<kind>/<version>`.
    #[must_use]
    pub fn component_dir(&self, kind: ComponentKind, version: &str) -> PathBuf {
        self.component_kind_dir(kind).join(version)
    }

    /// `root/bottles`.
    #[must_use]
    pub fn bottles_dir(&self) -> PathBuf {
        self.root.join("bottles")
    }

    /// `root/bottles/<name>`. Does not validate `name`; see
    /// [`crate::bottle::validate_name`].
    #[must_use]
    pub fn bottle_dir(&self, name: &str) -> PathBuf {
        self.bottles_dir().join(name)
    }

    /// `root/profiles`.
    #[must_use]
    pub fn profiles_dir(&self) -> PathBuf {
        self.root.join("profiles")
    }

    /// `root/apps`.
    #[must_use]
    pub fn apps_dir(&self) -> PathBuf {
        self.root.join("apps")
    }

    /// `root/cache/downloads`.
    #[must_use]
    pub fn downloads_dir(&self) -> PathBuf {
        self.root.join("cache").join("downloads")
    }

    /// `root/logs`.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.root.join("logs")
    }

    /// Create every directory above that does not exist yet.
    ///
    /// # Errors
    /// [`crate::Error::Io`] naming the directory that could not be created.
    pub fn ensure(&self) -> crate::Result<()> {
        let dirs = [
            self.root.clone(),
            self.components_dir(),
            self.bottles_dir(),
            self.profiles_dir(),
            self.apps_dir(),
            self.downloads_dir(),
            self.logs_dir(),
        ];
        for dir in dirs {
            std::fs::create_dir_all(&dir)
                .map_err(|e| crate::Error::io("cannot create directory", &dir, e))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    #[test]
    fn uncork_home_wins_over_home() {
        let layout = Layout::from_vars(os("/data/uncork"), os("/Users/me")).unwrap();
        assert_eq!(layout.root(), Path::new("/data/uncork"));
    }

    #[test]
    fn falls_back_to_application_support() {
        let layout = Layout::from_vars(None, os("/Users/me")).unwrap();
        assert_eq!(
            layout.root(),
            Path::new("/Users/me/Library/Application Support/Uncork")
        );
    }

    #[test]
    fn empty_uncork_home_is_ignored() {
        let layout = Layout::from_vars(os(""), os("/Users/me")).unwrap();
        assert_eq!(
            layout.root(),
            Path::new("/Users/me/Library/Application Support/Uncork")
        );
    }

    #[test]
    fn no_home_at_all_is_an_error() {
        assert!(matches!(
            Layout::from_vars(None, None),
            Err(crate::Error::NoHome)
        ));
        assert!(matches!(
            Layout::from_vars(os(""), os("")),
            Err(crate::Error::NoHome)
        ));
    }

    #[test]
    fn discover_agrees_with_the_environment() {
        // Read-only: compares against whatever this process was started with.
        let expected = Layout::from_vars(std::env::var_os(HOME_ENV), std::env::var_os("HOME"));
        match (Layout::discover(), expected) {
            (Ok(found), Ok(expected)) => assert_eq!(found, expected),
            (Err(crate::Error::NoHome), Err(crate::Error::NoHome)) => {}
            (found, expected) => panic!("discover() = {found:?}, expected {expected:?}"),
        }
    }

    #[test]
    fn derived_paths_hang_off_the_root() {
        let layout = Layout::at("/u");
        assert_eq!(layout.config_file(), Path::new("/u/config.toml"));
        assert_eq!(
            layout.component_dir(ComponentKind::Dxmt, "0.80"),
            Path::new("/u/components/dxmt/0.80")
        );
        assert_eq!(layout.bottle_dir("steam"), Path::new("/u/bottles/steam"));
        assert_eq!(layout.downloads_dir(), Path::new("/u/cache/downloads"));
        assert_eq!(layout.logs_dir(), Path::new("/u/logs"));
    }

    #[test]
    fn ensure_creates_every_directory_and_is_idempotent() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::at(temp.path().join("nested").join("root"));
        layout.ensure().unwrap();
        layout.ensure().unwrap();
        for dir in [
            layout.root().to_path_buf(),
            layout.components_dir(),
            layout.bottles_dir(),
            layout.profiles_dir(),
            layout.apps_dir(),
            layout.downloads_dir(),
            layout.logs_dir(),
        ] {
            assert!(dir.is_dir(), "{} was not created", dir.display());
        }
    }

    #[test]
    fn ensure_names_the_directory_it_could_not_create() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("logs"), b"a file where a directory belongs").unwrap();
        let err = Layout::at(&root).ensure().unwrap_err();
        match err {
            crate::Error::Io { path, .. } => assert_eq!(path, root.join("logs")),
            other => panic!("expected Error::Io, got {other:?}"),
        }
    }
}
