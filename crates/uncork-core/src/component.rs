//! Installed components: the Wine runtime and the graphics translation
//! layers that Uncork puts into bottles.
//!
//! A component is a directory `components/<kind>/<version>/` holding the
//! unpacked upstream release plus a `component.toml` ([`ComponentMeta`]).
//! Components come from the pinned [`crate::catalog`] (downloaded and
//! SHA-256-verified) or, for D3DMetal, from the user's own copy of Apple's
//! Game Porting Toolkit ([`import_gptk`]); Uncork never downloads or
//! redistributes D3DMetal. See `docs/LEGAL.md`.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::catalog::CatalogEntry;
use crate::download::Progress;
use crate::paths::Layout;

/// File name of the metadata written into every component directory.
pub const META_FILE: &str = "component.toml";

/// What a component provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    /// A Wine build for macOS (x86_64, new-style WoW64).
    Wine,
    /// DXMT: Direct3D 10/11 → Metal.
    Dxmt,
    /// DXVK (macOS fork): Direct3D 9/10/11 → Vulkan, run on MoltenVK.
    Dxvk,
    /// Apple's D3DMetal from the Game Porting Toolkit (user-supplied).
    D3dmetal,
}

impl ComponentKind {
    /// Every kind, in display order.
    pub const ALL: [ComponentKind; 4] = [
        ComponentKind::Wine,
        ComponentKind::Dxmt,
        ComponentKind::Dxvk,
        ComponentKind::D3dmetal,
    ];

    /// Lowercase name used in paths, TOML and the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            ComponentKind::Wine => "wine",
            ComponentKind::Dxmt => "dxmt",
            ComponentKind::Dxvk => "dxvk",
            ComponentKind::D3dmetal => "d3dmetal",
        }
    }
}

impl fmt::Display for ComponentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ComponentKind {
    type Err = String;

    /// Case-insensitive; accepts `d3dmetal`, `d3d-metal` and `gptk` for D3DMetal.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let _ = s;
        todo!()
    }
}

/// Where an installed component came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Source {
    /// Downloaded from the catalog.
    Catalog {
        /// Download URL.
        url: String,
        /// Verified SHA-256 of the archive, lowercase hex.
        sha256: String,
    },
    /// Copied from a local directory (GPTK import, `--from-dir`).
    Local {
        /// The directory it was copied from.
        path: PathBuf,
    },
}

/// Contents of `component.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ComponentMeta {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Kind.
    pub kind: ComponentKind,
    /// Version string, also the directory name. Must satisfy
    /// [`validate_version`].
    pub version: String,
    /// Provenance.
    pub source: Source,
    /// SPDX expression of the component's own license (not Uncork's).
    pub license: String,
    /// Where the corresponding source code is published (LGPL compliance for
    /// Wine), when known.
    #[serde(default)]
    pub source_code: Option<String>,
    /// Feature tags copied from the catalog. Wine runtimes: `wow64` (runs
    /// 32-bit programs), `msync`, `dxmt` (winemac exports the Metal view API
    /// DXMT needs), `d3dmetal` (CrossOver-derived glue D3DMetal needs),
    /// `renderer-dllpath` (honors `WINEDLLPATH_DXMT`/`_DXVK`/`_D3DMETAL`),
    /// `dllpath-prepend` (honors `WINEDLLPATH_PREPEND`),
    /// `large-address-aware` (honors `WINE_LARGE_ADDRESS_AWARE`).
    #[serde(default)]
    pub features: Vec<String>,
    /// Seconds since the Unix epoch when it was installed.
    pub installed_unix: u64,
}

/// An installed component.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledComponent {
    /// Parsed `component.toml`.
    pub meta: ComponentMeta,
    /// The component directory.
    pub path: PathBuf,
}

impl InstalledComponent {
    /// `true` if `meta.features` contains `feature`.
    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        self.meta.features.iter().any(|f| f == feature)
    }
}

/// Versions are directory names: 1–64 chars of `[A-Za-z0-9._+-]` or `_`,
/// not starting with `.` or `-`.
///
/// # Errors
/// [`crate::Error::InvalidName`].
pub fn validate_version(version: &str) -> crate::Result<()> {
    let _ = version;
    todo!()
}

/// Compare versions "naturally": split into runs of digits and non-digits,
/// compare digit runs numerically and other runs lexically, so
/// `0.10 > 0.9` and `11.0-uncork2 > 11.0-uncork1`.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let _ = (a, b);
    todo!()
}

/// Every installed component, sorted by kind then version (newest first).
/// Directories without a readable `component.toml` (including `.staging-*`
/// leftovers) are skipped with a `tracing::warn!`.
///
/// # Errors
/// [`crate::Error::Io`] if `components/` exists but cannot be listed.
pub fn list_installed(layout: &Layout) -> crate::Result<Vec<InstalledComponent>> {
    let _ = layout;
    todo!()
}

/// The installed component of `kind` with `version`, or the newest one when
/// `version` is `None`.
///
/// # Errors
/// [`crate::Error::NotFound`] with a hint naming the install command.
pub fn find_installed(
    layout: &Layout,
    kind: ComponentKind,
    version: Option<&str>,
) -> crate::Result<InstalledComponent> {
    let _ = (layout, kind, version);
    todo!()
}

/// Download, verify, unpack and register a catalog entry.
///
/// Steps: download to `downloads/<sha256[..16]>-<file name>` (reused if it
/// already verifies), extract into `components/<kind>/.staging-<version>`
/// (removed first if left over), check [`validate_layout`], write
/// `component.toml`, then rename to the final directory. Fails with
/// [`crate::Error::AlreadyExists`] if that version is installed.
///
/// # Errors
/// Download, checksum, archive, layout or I/O errors. The staging directory
/// is removed on failure.
pub fn install_from_catalog(
    layout: &Layout,
    entry: &CatalogEntry,
    progress: &mut dyn Progress,
) -> crate::Result<InstalledComponent> {
    let _ = (layout, entry, progress);
    todo!()
}

/// Remove an installed component directory.
///
/// # Errors
/// [`crate::Error::NotFound`] or [`crate::Error::Io`].
pub fn remove(layout: &Layout, kind: ComponentKind, version: &str) -> crate::Result<()> {
    let _ = (layout, kind, version);
    todo!()
}

/// Check that `dir` has the files Uncork relies on for `kind`.
/// The required paths per kind are listed in `docs/RUNTIME.md` and in
/// [`required_paths`].
///
/// # Errors
/// [`crate::Error::BrokenComponent`] naming the first missing path.
pub fn validate_layout(kind: ComponentKind, dir: &Path) -> crate::Result<()> {
    let _ = (kind, dir);
    todo!()
}

/// Paths (relative to the component directory) that must exist for `kind`.
/// For Wine, any one of the listed alternatives for the loader suffices; see
/// [`crate::wine::WineRuntime`].
#[must_use]
pub fn required_paths(kind: ComponentKind) -> &'static [&'static str] {
    let _ = kind;
    todo!()
}

/// Import D3DMetal from the user's copy of Apple's Game Porting Toolkit.
///
/// `source` may be the mounted GPTK disk image (the directory containing
/// `redist/`), its `redist` directory, or its `redist/lib` directory. The
/// version is read from `D3DMetal.framework`'s `Info.plist`
/// (`CFBundleShortVersionString`) with `/usr/bin/plutil`. Files are copied
/// with `/bin/cp -R` (APFS clones when possible) into
/// `components/d3dmetal/<version>/`, preserving the `external/` and `wine/`
/// subdirectories of `redist/lib`.
///
/// # Errors
/// [`crate::Error::NotFound`] if no `D3DMetal.framework` is found under
/// `source`, [`crate::Error::AlreadyExists`], or I/O / command errors.
pub fn import_gptk(layout: &Layout, source: &Path) -> crate::Result<InstalledComponent> {
    let _ = (layout, source);
    todo!()
}
