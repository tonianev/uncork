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
//!
//! `strip_prefix` is rarely needed: when it is left out and every entry of
//! the archive lives under one top-level directory, that directory is
//! stripped automatically (see [`crate::download::extract`]). Set it to `""`
//! to keep a single top-level directory.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::component::{ComponentKind, compare_versions, validate_version};

/// The catalog text compiled into the binary.
pub const BUILTIN_TOML: &str = include_str!("../../../runtime/catalog.toml");

/// The only schema version this build understands.
const SCHEMA: u32 = 1;

/// Names the catalog in [`crate::Error::Config`] messages. The text being
/// parsed has no path of its own; in practice it is always the built-in file.
const CATALOG_ORIGIN: &str = "runtime/catalog.toml";

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
    /// Top-level directory inside the archive to strip, if any. A single
    /// path component (no `/`, not `.` or `..`). `None` strips a lone
    /// top-level directory automatically; `Some("")` keeps the archive's
    /// paths exactly as they are.
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

impl CatalogEntry {
    /// The per-entry part of [`Catalog::parse`]'s validation, for entries
    /// that did not come from a parsed catalog.
    ///
    /// # Errors
    /// [`crate::Error::Config`] with `what = "catalog"`.
    pub(crate) fn validate(&self) -> crate::Result<()> {
        self.problem().map_or(Ok(()), |problem| {
            Err(invalid(format!("{}: {problem}", self.label())))
        })
    }

    /// The first thing wrong with this entry on its own, if anything.
    fn problem(&self) -> Option<String> {
        if self.kind == ComponentKind::D3dmetal {
            return Some(
                "d3dmetal is never downloaded; it is imported from Apple's Game Porting Toolkit"
                    .to_owned(),
            );
        }
        if let Err(error) = validate_version(&self.version) {
            return Some(error.to_string());
        }
        if !self.url.starts_with("https://") {
            return Some(format!("url {:?} does not start with https://", self.url));
        }
        if !is_lowercase_sha256(&self.sha256) {
            return Some(format!(
                "sha256 {:?} is not 64 lowercase hex characters",
                self.sha256
            ));
        }
        if self.size == 0 {
            return Some("size must be greater than 0".to_owned());
        }
        self.strip_prefix.as_deref().and_then(strip_prefix_problem)
    }

    /// `wine 11.0` style label for messages.
    fn label(&self) -> String {
        format!("{} {}", self.kind, self.version)
    }
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
    /// `recommended` per kind; `size > 0`; `strip_prefix`, when set, is
    /// empty or a single path component (no `/`, not `.` or `..`).
    ///
    /// # Errors
    /// [`crate::Error::Config`] with `what = "catalog"` and the first problem.
    pub fn parse(text: &str) -> crate::Result<Catalog> {
        let catalog: Catalog = toml::from_str(text).map_err(|error| invalid(error.to_string()))?;
        catalog.validate()?;
        Ok(catalog)
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
        let mut entries: Vec<&CatalogEntry> = self
            .components
            .iter()
            .filter(|entry| entry.kind == kind)
            .collect();
        entries.sort_by(|a, b| compare_versions(&b.version, &a.version));
        entries
    }

    /// The entry to install by default: the `recommended` one, else the newest.
    #[must_use]
    pub fn default_for(&self, kind: ComponentKind) -> Option<&CatalogEntry> {
        self.components
            .iter()
            .find(|entry| entry.kind == kind && entry.recommended)
            .or_else(|| self.entries(kind).into_iter().next())
    }

    /// Exact lookup.
    #[must_use]
    pub fn find(&self, kind: ComponentKind, version: &str) -> Option<&CatalogEntry> {
        self.components
            .iter()
            .find(|entry| entry.kind == kind && entry.version == version)
    }

    fn validate(&self) -> crate::Result<()> {
        if self.schema != SCHEMA {
            return Err(invalid(format!(
                "unsupported schema {} (this version of Uncork reads schema {SCHEMA})",
                self.schema
            )));
        }
        let mut seen = HashSet::new();
        let mut recommended: HashMap<ComponentKind, &str> = HashMap::new();
        for entry in &self.components {
            entry.validate()?;
            if !seen.insert((entry.kind, entry.version.as_str())) {
                return Err(invalid(format!("{} is listed twice", entry.label())));
            }
            if entry.recommended
                && let Some(previous) = recommended.insert(entry.kind, &entry.version)
            {
                return Err(invalid(format!(
                    "more than one recommended {} entry ({previous} and {})",
                    entry.kind, entry.version
                )));
            }
        }
        Ok(())
    }
}

fn invalid(message: String) -> crate::Error {
    crate::Error::Config {
        what: "catalog",
        path: PathBuf::from(CATALOG_ORIGIN),
        message,
    }
}

fn is_lowercase_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

fn strip_prefix_problem(prefix: &str) -> Option<String> {
    if prefix.contains('/') || prefix == "." || prefix == ".." {
        Some(format!(
            "strip_prefix {prefix:?} must be a single directory name (no '/', not '.' or '..')"
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA: &str = "8f260e36b5739e68f3bad613381441385c4dc7b85b78ba8de653d5a6a264529d";

    fn component(kind: &str, version: &str) -> String {
        format!(
            r#"
[[component]]
kind = "{kind}"
version = "{version}"
url = "https://example.com/{kind}-{version}.tar.gz"
sha256 = "{SHA}"
size = 1024
archive = "tar.gz"
license = "MIT"
source_code = "https://example.com/src"
homepage = "https://example.com"
"#
        )
    }

    fn catalog(components: &[String]) -> String {
        format!("schema = 1\n{}", components.concat())
    }

    fn parse_error(text: &str) -> String {
        match Catalog::parse(text) {
            Ok(catalog) => panic!("expected an error, parsed {catalog:?}"),
            Err(crate::Error::Config {
                what,
                path,
                message,
            }) => {
                assert_eq!(what, "catalog");
                assert_eq!(path, PathBuf::from(CATALOG_ORIGIN));
                message
            }
            Err(other) => panic!("expected a catalog error, got {other:?}"),
        }
    }

    #[test]
    fn builtin_catalog_parses_and_recommends_every_downloadable_kind() {
        let catalog = Catalog::builtin();
        assert_eq!(catalog.schema, 1);
        for kind in [
            ComponentKind::Wine,
            ComponentKind::Dxmt,
            ComponentKind::Dxvk,
        ] {
            let entry = catalog
                .default_for(kind)
                .unwrap_or_else(|| panic!("no {kind} entry"));
            assert!(entry.recommended, "{kind} has no recommended entry");
        }
        assert!(catalog.entries(ComponentKind::D3dmetal).is_empty());
    }

    #[test]
    fn builtin_catalog_entries_are_complete() {
        for entry in &Catalog::builtin().components {
            assert!(
                !entry.license.is_empty(),
                "{} has no license",
                entry.label()
            );
            assert!(
                entry.source_code.starts_with("https://"),
                "{}",
                entry.label()
            );
            assert!(entry.homepage.starts_with("https://"), "{}", entry.label());
            assert!(
                !entry.url.ends_with('/'),
                "{} url has no file name",
                entry.label()
            );
        }
    }

    #[test]
    fn parses_a_minimal_catalog() {
        let parsed = Catalog::parse(&catalog(&[component("dxmt", "0.80")])).unwrap();
        let entry = &parsed.components[0];
        assert_eq!(entry.kind, ComponentKind::Dxmt);
        assert_eq!(entry.archive, ArchiveFormat::TarGz);
        assert_eq!(entry.strip_prefix, None);
        assert!(entry.features.is_empty());
        assert!(!entry.recommended);
        assert_eq!(entry.notes, "");
    }

    #[test]
    fn parses_an_empty_catalog() {
        assert!(
            Catalog::parse("schema = 1\n")
                .unwrap()
                .components
                .is_empty()
        );
    }

    #[test]
    fn parses_tar_xz_and_optional_fields() {
        let text = catalog(&[
            component("wine", "11.0").replace("tar.gz\"\n", "tar.xz\"\n")
                + "strip_prefix = \"wswine.bundle\"\nfeatures = [\"wow64\"]\nrecommended = true\nnotes = \"n\"\n",
        ]);
        let parsed = Catalog::parse(&text).unwrap();
        let entry = &parsed.components[0];
        assert_eq!(entry.archive, ArchiveFormat::TarXz);
        assert_eq!(entry.strip_prefix.as_deref(), Some("wswine.bundle"));
        assert_eq!(entry.features, ["wow64"]);
        assert!(entry.recommended);
    }

    #[test]
    fn accepts_an_empty_strip_prefix() {
        let text = catalog(&[component("dxvk", "1.10") + "strip_prefix = \"\"\n"]);
        assert_eq!(
            Catalog::parse(&text).unwrap().components[0]
                .strip_prefix
                .as_deref(),
            Some("")
        );
    }

    #[test]
    fn rejects_invalid_toml() {
        assert!(!parse_error("schema = ").is_empty());
    }

    #[test]
    fn rejects_unknown_fields() {
        assert!(
            parse_error(&catalog(&[component("dxmt", "1") + "mirror = \"x\"\n"]))
                .contains("mirror")
        );
        assert!(parse_error("schema = 1\nextra = true\n").contains("extra"));
    }

    #[test]
    fn rejects_other_schemas() {
        assert!(parse_error("schema = 2\n").contains("unsupported schema 2"));
    }

    #[test]
    fn rejects_d3dmetal() {
        let message = parse_error(&catalog(&[component("d3dmetal", "3.0")]));
        assert!(message.contains("Game Porting Toolkit"), "{message}");
    }

    #[test]
    fn rejects_plain_http() {
        let text =
            catalog(&[component("dxmt", "1")
                .replace("https://example.com/dxmt", "http://example.com/dxmt")]);
        assert!(parse_error(&text).contains("https://"));
    }

    #[test]
    fn rejects_malformed_checksums() {
        let cases = [
            SHA.to_uppercase(),
            SHA[..63].to_owned(),
            format!("{SHA}0"),
            SHA.replacen('8', "g", 1),
            String::new(),
        ];
        for bad in cases {
            let text = catalog(&[component("dxmt", "1").replace(SHA, &bad)]);
            assert!(parse_error(&text).contains("sha256"), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_invalid_versions() {
        for bad in ["", "../x", "a/b", ".hidden", "-x", "has space"] {
            let message = parse_error(&catalog(&[component("dxmt", bad)]));
            assert!(
                message.contains("invalid version name"),
                "{bad:?}: {message}"
            );
        }
    }

    #[test]
    fn rejects_zero_size() {
        let text = catalog(&[component("dxmt", "1").replace("size = 1024", "size = 0")]);
        assert!(parse_error(&text).contains("size"));
    }

    #[test]
    fn rejects_unsafe_strip_prefixes() {
        for bad in ["a/b", "/abs", "..", ".", "dir/"] {
            let text = catalog(&[component("dxmt", "1") + &format!("strip_prefix = {bad:?}\n")]);
            assert!(
                parse_error(&text).contains("strip_prefix"),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn rejects_duplicates() {
        let message = parse_error(&catalog(&[component("dxmt", "1"), component("dxmt", "1")]));
        assert!(message.contains("dxmt 1 is listed twice"), "{message}");
    }

    #[test]
    fn same_version_of_different_kinds_is_fine() {
        assert!(
            Catalog::parse(&catalog(&[component("dxmt", "1"), component("dxvk", "1")])).is_ok()
        );
    }

    #[test]
    fn rejects_two_recommended_entries_of_one_kind() {
        let message = parse_error(&catalog(&[
            component("dxmt", "1") + "recommended = true\n",
            component("dxmt", "2") + "recommended = true\n",
        ]));
        assert!(
            message.contains("more than one recommended dxmt"),
            "{message}"
        );
    }

    #[test]
    fn error_names_the_offending_entry() {
        let text = catalog(&[
            component("dxmt", "1"),
            component("dxvk", "2").replace("size = 1024", "size = 0"),
        ]);
        assert!(parse_error(&text).starts_with("dxvk 2:"));
    }

    fn sample() -> Catalog {
        Catalog::parse(&catalog(&[
            component("dxmt", "0.9"),
            component("dxmt", "0.10"),
            component("dxmt", "0.8") + "recommended = true\n",
            component("wine", "10.0"),
            component("wine", "11.0"),
        ]))
        .unwrap()
    }

    fn versions(entries: &[&CatalogEntry]) -> Vec<String> {
        entries.iter().map(|entry| entry.version.clone()).collect()
    }

    #[test]
    fn entries_are_newest_first() {
        let catalog = sample();
        assert_eq!(
            versions(&catalog.entries(ComponentKind::Dxmt)),
            ["0.10", "0.9", "0.8"]
        );
        assert_eq!(
            versions(&catalog.entries(ComponentKind::Wine)),
            ["11.0", "10.0"]
        );
        assert!(catalog.entries(ComponentKind::Dxvk).is_empty());
    }

    #[test]
    fn default_prefers_recommended_then_newest() {
        let catalog = sample();
        assert_eq!(
            catalog.default_for(ComponentKind::Dxmt).unwrap().version,
            "0.8"
        );
        assert_eq!(
            catalog.default_for(ComponentKind::Wine).unwrap().version,
            "11.0"
        );
        assert!(catalog.default_for(ComponentKind::Dxvk).is_none());
    }

    #[test]
    fn find_is_exact() {
        let catalog = sample();
        assert_eq!(
            catalog.find(ComponentKind::Dxmt, "0.9").unwrap().version,
            "0.9"
        );
        assert!(catalog.find(ComponentKind::Dxmt, "0.09").is_none());
        assert!(catalog.find(ComponentKind::Wine, "0.9").is_none());
    }

    #[test]
    fn validate_reports_entry_problems() {
        let mut entry = sample().components[0].clone();
        assert!(entry.validate().is_ok());
        entry.kind = ComponentKind::D3dmetal;
        assert!(matches!(
            entry.validate(),
            Err(crate::Error::Config {
                what: "catalog",
                ..
            })
        ));
    }
}
