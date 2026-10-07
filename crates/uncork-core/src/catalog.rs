//! The component catalog: pinned upstream releases with SHA-256 checksums.
//!
//! The built-in catalog is `runtime/catalog.toml` at the repository root,
//! compiled into the binary. Every entry pins one archive by URL, size and
//! SHA-256, and records the component's license and where its source code is
//! published. Updating a pin is a reviewed pull request; see `docs/RUNTIME.md`.
//!
//! ```toml
//! schema = 1
//!
//! [[component]]
//! kind = "dxmt"
//! version = "0.70"
//! url = "https://github.com/3Shain/dxmt/releases/download/v0.70/dxmt-v0.70-builtin.tar.gz"
//! sha256 = "<64 lowercase hex>"
//! size = 12345678
//! archive = "tar.gz"
//! strip_prefix = "dxmt-v0.70-builtin"   # optional top-level directory to drop
//! license = "Zlib"
//! source_code = "https://github.com/3Shain/dxmt/tree/v0.70"
//! homepage = "https://github.com/3Shain/dxmt"
//! features = []
//! recommended = true
//! notes = "Direct3D 10/11 to Metal. 32- and 64-bit."
//! ```

use serde::{Deserialize, Serialize};

use crate::component::ComponentKind;

/// The catalog text compiled into the binary.
pub const BUILTIN_TOML: &str = include_str!("../../../runtime/catalog.toml");

/// Archive formats Uncork can unpack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArchiveFormat {
    /// gzip-compressed tar.
    #[serde(rename = "tar.gz")]
    TarGz,
    /// xz-compressed tar.
    #[serde(rename = "tar.xz")]
    TarXz,
}

/// One pinned release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogEntry {
    /// Component kind. `d3dmetal` is rejected: it is never downloadable.
    pub kind: ComponentKind,
    /// Version; becomes the install directory name.
    pub version: String,
    /// HTTPS download URL.
    pub url: String,
    /// SHA-256 of the archive, 64 lowercase hex chars.
    pub sha256: String,
    /// Archive size in bytes (used for progress and as a sanity check).
    pub size: u64,
    /// Archive format.
    pub archive: ArchiveFormat,
    /// Top-level directory inside the archive to strip, if any.
    #[serde(default)]
    pub strip_prefix: Option<String>,
    /// SPDX license expression of the component.
    pub license: String,
    /// Where the corresponding source code is published.
    pub source_code: String,
    /// Project homepage.
    pub homepage: String,
    /// Feature tags, e.g. `msync`, `wow64`.
    #[serde(default)]
    pub features: Vec<String>,
    /// The entry `uncork runtime install <kind>` picks by default. At most
    /// one per kind.
    #[serde(default)]
    pub recommended: bool,
    /// One-line description shown by `uncork runtime available`.
    #[serde(default)]
    pub notes: String,
}

/// A parsed catalog.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Entries in file order.
    #[serde(default, rename = "component")]
    pub components: Vec<CatalogEntry>,
}

impl Catalog {
    /// Parse and validate catalog text. Validation: schema is 1; every
    /// `url` starts with `https://`; `sha256` is 64 lowercase hex chars;
    /// `version` passes [`crate::component::validate_version`]; no
    /// `d3dmetal` entries; no duplicate (kind, version); at most one
    /// `recommended` per kind; `size > 0`.
    ///
    /// # Errors
    /// [`crate::Error::Config`] with `what = "catalog"` and the first problem.
    pub fn parse(text: &str) -> crate::Result<Catalog> {
        let _ = text;
        todo!()
    }

    /// The built-in catalog.
    ///
    /// # Panics
    /// If the compiled-in catalog is invalid (a unit test guarantees it is not).
    #[must_use]
    pub fn builtin() -> Catalog {
        Catalog::parse(BUILTIN_TOML).expect("runtime/catalog.toml is validated by tests")
    }

    /// Entries of `kind`, newest version first.
    #[must_use]
    pub fn entries(&self, kind: ComponentKind) -> Vec<&CatalogEntry> {
        let _ = kind;
        todo!()
    }

    /// The entry to install by default: the `recommended` one, else the newest.
    #[must_use]
    pub fn default_for(&self, kind: ComponentKind) -> Option<&CatalogEntry> {
        let _ = kind;
        todo!()
    }

    /// Exact lookup.
    #[must_use]
    pub fn find(&self, kind: ComponentKind, version: &str) -> Option<&CatalogEntry> {
        let _ = (kind, version);
        todo!()
    }
}
