//! A Wine runtime: the `wine` component's binaries and the commands Uncork
//! builds from them.

use std::path::{Path, PathBuf};

use crate::component::{ComponentKind, InstalledComponent};
use crate::process::CommandSpec;

/// A usable Wine installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRuntime {
    /// Component directory (contains `bin/` and `lib/`).
    pub root: PathBuf,
    /// Component version.
    pub version: String,
    /// Feature tags from the component (`msync`, `wow64`, ...).
    pub features: Vec<String>,
}

impl WineRuntime {
    /// Wrap an installed `wine` component, checking the loader exists.
    ///
    /// # Errors
    /// [`crate::Error::BrokenComponent`] if `kind` is not Wine or no loader
    /// binary is found.
    pub fn from_component(component: &InstalledComponent) -> crate::Result<WineRuntime> {
        let _ = (component, ComponentKind::Wine);
        todo!()
    }

    /// The loader: `bin/wine` if present (new-style WoW64 builds ship only
    /// this), else `bin/wine64`.
    #[must_use]
    pub fn wine_bin(&self) -> PathBuf {
        todo!()
    }

    /// `bin/wineserver`.
    #[must_use]
    pub fn wineserver_bin(&self) -> PathBuf {
        self.root.join("bin").join("wineserver")
    }

    /// `true` if the component declares `feature`.
    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// A command running `program` (an absolute path, e.g. the loader or
    /// `wineserver`) with only `WINEPREFIX=<prefix>`, `WINEDEBUG=-all` and
    /// `WINEDLLOVERRIDES=winemenubuilder.exe=d` set. Callers that launch
    /// programs add [`crate::launch::base_env`] on top. `env_clear` is set:
    /// the inherited environment is replaced by
    /// [`crate::launch::ALLOWED_ENV`] and [`crate::launch::WINE_PATH`].
    #[must_use]
    pub fn base_command(&self, program: &Path, prefix: &Path) -> CommandSpec {
        let _ = (program, prefix);
        todo!()
    }

    /// `wine wineboot -u` for a new prefix (creates it when missing). Also
    /// sets `WINEDLLOVERRIDES=mscoree,mshtml,winemenubuilder.exe=d` so Wine
    /// does not offer to download Mono and Gecko during creation.
    #[must_use]
    pub fn boot_init_command(&self, prefix: &Path) -> CommandSpec {
        let _ = prefix;
        todo!()
    }

    /// `wine wineboot --update` (after a Wine version change).
    #[must_use]
    pub fn boot_update_command(&self, prefix: &Path) -> CommandSpec {
        let _ = prefix;
        todo!()
    }

    /// `wineserver --wait`: blocks until every process in the prefix exits.
    #[must_use]
    pub fn wait_command(&self, prefix: &Path) -> CommandSpec {
        let _ = prefix;
        todo!()
    }

    /// `wineserver --kill`.
    #[must_use]
    pub fn kill_command(&self, prefix: &Path) -> CommandSpec {
        let _ = prefix;
        todo!()
    }

    /// `wine regedit /S <C:\windows\temp\file.reg>`.
    #[must_use]
    pub fn regedit_import_command(&self, prefix: &Path, windows_path: &str) -> CommandSpec {
        let _ = (prefix, windows_path);
        todo!()
    }

    /// `wine winecfg /v <name>`.
    #[must_use]
    pub fn set_windows_version_command(&self, prefix: &Path, version: crate::bottle::WindowsVersion) -> CommandSpec {
        let _ = (prefix, version);
        todo!()
    }

    /// `wine --version`, for `doctor` and `runtime list`.
    #[must_use]
    pub fn version_command(&self) -> CommandSpec {
        CommandSpec::new(self.wine_bin()).arg("--version")
    }
}
