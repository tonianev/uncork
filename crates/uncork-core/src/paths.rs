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
        todo!()
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
        todo!()
    }
}
