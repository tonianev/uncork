//! Installed components: the Wine runtime and the graphics translation
//! layers that Uncork puts into bottles.
//!
//! A component is a directory `components/<kind>/<version>/` holding the
//! unpacked upstream release plus a `component.toml` ([`ComponentMeta`]).
//! Components come from the pinned [`crate::catalog`] (downloaded and
//! SHA-256-verified) or, for `D3DMetal`, from the user's own copy of Apple's
//! Game Porting Toolkit ([`import_gptk`]); Uncork never downloads or
//! redistributes `D3DMetal`. See `docs/LEGAL.md`.
//!
//! Installs are staged in `components/<kind>/.staging-<version>/` and only
//! renamed into place once complete and validated, so a component directory
//! without its `component.toml` is never mistaken for an installed one.
//! Removal renames to `.removing-<version>/` first for the same reason.

use std::cmp::Ordering;
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::catalog::CatalogEntry;
use crate::download::Progress;
use crate::paths::Layout;

/// File name of the metadata written into every component directory.
pub const META_FILE: &str = "component.toml";

/// Schema version of [`ComponentMeta`] written by this build.
const META_SCHEMA: u32 = 1;

/// Longest accepted version string.
const MAX_VERSION_LEN: usize = 64;

/// Prefix of the directory an install is assembled in.
const STAGING_PREFIX: &str = ".staging-";

/// Prefix a component directory is renamed to before it is deleted.
const REMOVING_PREFIX: &str = ".removing-";

/// Separates alternatives in a [`required_paths`] entry.
const ALTERNATIVE_SEPARATOR: char = '|';

/// How deep below the component root to look for a nested Wine tree.
const WINE_SEARCH_DEPTH: usize = 3;

/// DXVK's release directories and the Wine names Uncork uses for them.
const DXVK_ARCH_RENAMES: [(&str, &str); 2] = [("x64", "x86_64-windows"), ("x32", "i386-windows")];

/// Where the framework sits inside GPTK's `redist/lib`.
const GPTK_FRAMEWORK: &str = "external/D3DMetal.framework";

/// The parts of GPTK's `redist/lib` that make up the component.
const GPTK_SUBDIRS: [&str; 2] = ["external", "wine"];

/// Apple's Game Porting Toolkit license has no SPDX identifier.
const GPTK_LICENSE: &str = "LicenseRef-Apple-Game-Porting-Toolkit";

const PLUTIL: &str = "/usr/bin/plutil";
const CP: &str = "/bin/cp";

/// What a component provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComponentKind {
    /// A Wine build for macOS (`x86_64`, new-style `WoW64`).
    Wine,
    /// DXMT: Direct3D 10/11 → Metal.
    Dxmt,
    /// DXVK (macOS fork): Direct3D 9/10/11 → Vulkan, run on `MoltenVK`.
    Dxvk,
    /// Apple's `D3DMetal` from the Game Porting Toolkit (user-supplied).
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

    /// The command that installs a component of this kind, for hints.
    fn install_hint(self) -> String {
        match self {
            ComponentKind::D3dmetal => {
                "; run: uncork runtime import-gptk <mounted Game Porting Toolkit volume>".to_owned()
            }
            kind => format!("; run: uncork runtime install {kind}"),
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

    /// Case-insensitive; accepts `d3dmetal`, `d3d-metal` and `gptk` for `D3DMetal`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "wine" => Ok(ComponentKind::Wine),
            "dxmt" => Ok(ComponentKind::Dxmt),
            "dxvk" => Ok(ComponentKind::Dxvk),
            "d3dmetal" | "d3d-metal" | "gptk" => Ok(ComponentKind::D3dmetal),
            _ => Err(format!(
                "unknown component kind {s:?}; expected wine, dxmt, dxvk or d3dmetal"
            )),
        }
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
    /// DXMT needs), `d3dmetal` (CrossOver-derived glue `D3DMetal` needs),
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

/// Versions are directory names: 1–64 chars of ASCII letters, digits, `.`,
/// `_`, `+` and `-`, not starting with `.` or `-`.
///
/// # Errors
/// [`crate::Error::InvalidName`].
pub fn validate_version(version: &str) -> crate::Result<()> {
    let is_allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-');
    let reason = if version.is_empty() {
        "must not be empty"
    } else if !version.chars().all(is_allowed) {
        "may only contain ASCII letters, digits, '.', '_', '+' and '-'"
    } else if version.starts_with(['.', '-']) {
        "must not start with '.' or '-'"
    } else if version.len() > MAX_VERSION_LEN {
        "must be at most 64 characters"
    } else {
        return Ok(());
    };
    Err(Error::InvalidName {
        what: "version",
        name: version.to_owned(),
        reason,
    })
}

/// Compare versions "naturally": split into runs of digits and non-digits,
/// compare digit runs numerically and other runs lexically, so
/// `0.10 > 0.9` and `11.0-uncork2 > 11.0-uncork1`. At the same position a
/// digit run sorts before a non-digit run, and a version that is a prefix of
/// another sorts first. Versions that only differ in leading zeros
/// (`1.01`, `1.1`) are ordered by their plain string order, so the result is
/// `Equal` only for identical strings.
#[must_use]
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let mut left = chunks(a);
    let mut right = chunks(b);
    loop {
        match (left.next(), right.next()) {
            (Some(x), Some(y)) => match compare_chunks(x, y) {
                Ordering::Equal => {}
                unequal => return unequal,
            },
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (None, None) => return a.cmp(b),
        }
    }
}

/// Maximal runs of ASCII digits and of everything else.
fn chunks(s: &str) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        let digits = rest.bytes().next()?.is_ascii_digit();
        let end = rest
            .bytes()
            .position(|b| b.is_ascii_digit() != digits)
            .unwrap_or(rest.len());
        // Splitting next to an ASCII digit is always on a char boundary.
        let (chunk, tail) = rest.split_at(end);
        rest = tail;
        Some(chunk)
    })
}

fn compare_chunks(x: &str, y: &str) -> Ordering {
    let is_number = |chunk: &str| chunk.bytes().next().is_some_and(|b| b.is_ascii_digit());
    match (is_number(x), is_number(y)) {
        (true, true) => {
            let (x, y) = (x.trim_start_matches('0'), y.trim_start_matches('0'));
            x.len().cmp(&y.len()).then_with(|| x.cmp(y))
        }
        (false, false) => x.cmp(y),
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
    }
}

/// Every installed component, sorted by kind then version (newest first).
/// Directories without a readable `component.toml` (including `.staging-*`
/// leftovers) are skipped with a `tracing::warn!`, as are directories whose
/// `component.toml` names a different kind or version.
///
/// # Errors
/// [`crate::Error::Io`] if `components/` exists but cannot be listed.
pub fn list_installed(layout: &Layout) -> crate::Result<Vec<InstalledComponent>> {
    let mut installed = Vec::new();
    for kind in ComponentKind::ALL {
        installed.extend(list_kind(layout, kind)?);
    }
    installed.sort_by(|a, b| {
        a.meta
            .kind
            .cmp(&b.meta.kind)
            .then_with(|| compare_versions(&b.meta.version, &a.meta.version))
    });
    Ok(installed)
}

fn list_kind(layout: &Layout, kind: ComponentKind) -> crate::Result<Vec<InstalledComponent>> {
    let kind_dir = layout.component_kind_dir(kind);
    let entries = match fs::read_dir(&kind_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(Error::io("cannot list directory", kind_dir, e)),
    };
    let mut found = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("cannot list directory", &kind_dir, e))?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match read_installed(kind, &path) {
            Ok(component) => found.push(component),
            Err(reason) => {
                tracing::warn!(path = %path.display(), "skipping component directory: {reason}");
            }
        }
    }
    Ok(found)
}

fn read_installed(kind: ComponentKind, dir: &Path) -> Result<InstalledComponent, String> {
    let name = dir
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("its name is not valid UTF-8")?;
    if name.starts_with(STAGING_PREFIX) || name.starts_with(REMOVING_PREFIX) {
        return Err("it is left over from an interrupted install or removal".to_owned());
    }
    let meta = read_meta(dir)?;
    if meta.kind != kind || meta.version != name {
        return Err(format!(
            "{META_FILE} describes {} {}, not {kind} {name}",
            meta.kind, meta.version
        ));
    }
    Ok(InstalledComponent {
        meta,
        path: dir.to_path_buf(),
    })
}

fn read_meta(dir: &Path) -> Result<ComponentMeta, String> {
    let path = dir.join(META_FILE);
    let text = fs::read_to_string(&path).map_err(|e| format!("cannot read {META_FILE}: {e}"))?;
    toml::from_str(&text).map_err(|e| format!("invalid {META_FILE}: {e}"))
}

fn write_meta(dir: &Path, meta: &ComponentMeta) -> crate::Result<()> {
    let path = dir.join(META_FILE);
    let text = toml::to_string(meta).map_err(|e| Error::Config {
        what: "component metadata",
        path: path.clone(),
        message: e.to_string(),
    })?;
    fs::write(&path, text).map_err(|e| Error::io("cannot write", path, e))
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
    let mut candidates = list_kind(layout, kind)?;
    candidates.sort_by(|a, b| compare_versions(&b.meta.version, &a.meta.version));
    let found = match version {
        Some(version) => candidates.into_iter().find(|c| c.meta.version == version),
        None => candidates.into_iter().next(),
    };
    found.ok_or_else(|| Error::NotFound {
        what: "component",
        name: version.map_or_else(|| kind.to_string(), |version| format!("{kind} {version}")),
        hint: kind.install_hint(),
    })
}

/// Download, verify, unpack and register a catalog entry.
///
/// Steps: download to `downloads/<sha256[..16]>-<file name>` (reused if it
/// already verifies), then [`install_from_archive`]'s steps without hashing
/// the archive a second time. Fails with [`crate::Error::AlreadyExists`]
/// before downloading anything if that version is installed.
///
/// # Errors
/// Download, checksum, archive, layout or I/O errors. The staging directory
/// is removed on failure; a verified download is kept for the next attempt.
pub fn install_from_catalog(
    layout: &Layout,
    entry: &CatalogEntry,
    progress: &mut dyn Progress,
) -> crate::Result<InstalledComponent> {
    entry.validate()?;
    ensure_not_installed(layout, entry.kind, &entry.version)?;
    let archive = layout.downloads_dir().join(download_file_name(entry));
    crate::download::download(&entry.url, &archive, Some(&entry.sha256), progress)?;
    install_verified_archive(layout, entry, &archive)
}

/// Install a catalog entry from an archive that is already on disk.
///
/// Steps: check the archive's SHA-256 against the entry, extract into
/// `components/<kind>/.staging-<version>` (removed first if left over),
/// [normalize](#layout-normalization) the layout, check [`validate_layout`],
/// write `component.toml` (the entry's license, source code and features;
/// [`Source::Catalog`]), then rename to the final directory.
///
/// # Layout normalization
///
/// Upstream archives do not all use the layout Uncork expects, so after
/// unpacking (and the automatic top-level strip of
/// [`crate::download::extract`]):
///
/// - DXVK: `x64/` and `x32/` are renamed to `x86_64-windows/` and
///   `i386-windows/` unless those already exist.
/// - Wine: when neither `bin/wine` nor `bin/wine64` is at the top, the
///   shallowest directory at most three levels down that has one of them
///   plus `lib/wine/` (e.g. Sikarugir's `wswine.bundle/`, Whisky's
///   `Libraries/Wine/`) is taken as the Wine tree and its contents are moved
///   to the top. Everything else in the archive stays where it is.
///
/// # Errors
/// [`crate::Error::Checksum`] if the archive does not match the entry,
/// [`crate::Error::AlreadyExists`] if that version is installed, archive,
/// layout ([`crate::Error::BrokenComponent`]) or I/O errors. The staging
/// directory is removed on failure.
pub fn install_from_archive(
    layout: &Layout,
    entry: &CatalogEntry,
    archive_path: &Path,
) -> crate::Result<InstalledComponent> {
    entry.validate()?;
    ensure_not_installed(layout, entry.kind, &entry.version)?;
    let actual = crate::download::sha256_file(archive_path)?;
    if !actual.eq_ignore_ascii_case(&entry.sha256) {
        return Err(Error::Checksum {
            url: entry.url.clone(),
            expected: entry.sha256.clone(),
            actual,
        });
    }
    install_verified_archive(layout, entry, archive_path)
}

fn install_verified_archive(
    layout: &Layout,
    entry: &CatalogEntry,
    archive_path: &Path,
) -> crate::Result<InstalledComponent> {
    warn_on_size_mismatch(entry, archive_path);
    let meta = ComponentMeta {
        schema: META_SCHEMA,
        kind: entry.kind,
        version: entry.version.clone(),
        source: Source::Catalog {
            url: entry.url.clone(),
            sha256: entry.sha256.to_ascii_lowercase(),
        },
        license: entry.license.clone(),
        source_code: Some(entry.source_code.clone()),
        features: entry.features.clone(),
        installed_unix: crate::unix_now(),
    };
    let path = install_staged(layout, &meta, |staging| {
        crate::download::extract(
            archive_path,
            entry.archive,
            staging,
            entry.strip_prefix.as_deref(),
        )?;
        normalize_layout(entry.kind, staging)
    })?;
    tracing::info!(kind = %meta.kind, version = %meta.version, path = %path.display(), "installed");
    Ok(InstalledComponent { meta, path })
}

/// The size is redundant with the checksum; a mismatch means the catalog
/// entry itself is wrong, which is worth a warning but not a failure.
fn warn_on_size_mismatch(entry: &CatalogEntry, archive_path: &Path) {
    if let Ok(meta) = fs::metadata(archive_path)
        && meta.len() != entry.size
    {
        tracing::warn!(
            kind = %entry.kind,
            version = %entry.version,
            expected = entry.size,
            actual = meta.len(),
            "archive size differs from the catalog"
        );
    }
}

/// `<sha256[..16]>-<file name from the URL>`.
fn download_file_name(entry: &CatalogEntry) -> String {
    let short_hash = entry.sha256.get(..16).unwrap_or(&entry.sha256);
    let name = crate::download::file_name_from_url(&entry.url);
    let name = if name.is_empty() { "archive" } else { name };
    format!("{short_hash}-{name}")
}

fn ensure_not_installed(layout: &Layout, kind: ComponentKind, version: &str) -> crate::Result<()> {
    let dir = layout.component_dir(kind, version);
    if fs::symlink_metadata(&dir).is_ok() {
        return Err(Error::AlreadyExists {
            what: "component",
            name: format!("{kind} {version}"),
            path: dir,
        });
    }
    Ok(())
}

/// Build a component in its staging directory with `fill` (which must
/// create the directory), validate it, record `meta` and move it into place.
/// The staging directory never survives a failure.
fn install_staged(
    layout: &Layout,
    meta: &ComponentMeta,
    fill: impl FnOnce(&Path) -> crate::Result<()>,
) -> crate::Result<PathBuf> {
    let kind_dir = layout.component_kind_dir(meta.kind);
    fs::create_dir_all(&kind_dir)
        .map_err(|e| Error::io("cannot create directory", &kind_dir, e))?;
    let staging = kind_dir.join(format!("{STAGING_PREFIX}{}", meta.version));
    remove_if_present(&staging)?;
    let final_dir = layout.component_dir(meta.kind, &meta.version);

    let result = fill(&staging)
        .and_then(|()| validate_layout(meta.kind, &staging))
        .and_then(|()| write_meta(&staging, meta))
        .and_then(|()| {
            ensure_not_installed(layout, meta.kind, &meta.version)?;
            fs::rename(&staging, &final_dir)
                .map_err(|e| Error::io("cannot move into place", &staging, e))
        });
    if result.is_err()
        && let Err(e) = remove_if_present(&staging)
    {
        tracing::warn!(error = %e, "cannot clean up the staging directory");
    }
    result.map(|()| final_dir)
}

/// Delete whatever is at `path` (without following a symlink there).
fn remove_if_present(path: &Path) -> crate::Result<()> {
    let removed = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => Err(e),
    };
    removed.map_err(|e| Error::io("cannot remove", path, e))
}

/// Bring an unpacked release into the layout [`required_paths`] describes.
/// See [`install_from_archive`] for the rules.
fn normalize_layout(kind: ComponentKind, dir: &Path) -> crate::Result<()> {
    match kind {
        ComponentKind::Wine => hoist_nested_wine(dir),
        ComponentKind::Dxvk => rename_dxvk_arch_dirs(dir),
        ComponentKind::Dxmt | ComponentKind::D3dmetal => Ok(()),
    }
}

fn rename_dxvk_arch_dirs(dir: &Path) -> crate::Result<()> {
    for (from, to) in DXVK_ARCH_RENAMES {
        let (from, to) = (dir.join(from), dir.join(to));
        if is_real_dir(&from) && fs::symlink_metadata(&to).is_err() {
            fs::rename(&from, &to).map_err(|e| Error::io("cannot rename", &from, e))?;
        }
    }
    Ok(())
}

fn hoist_nested_wine(dir: &Path) -> crate::Result<()> {
    if has_wine_loader(dir) {
        return Ok(());
    }
    // Nothing found is not an error here: validate_layout reports what is missing.
    let Some(nested) = find_wine_tree(dir)? else {
        return Ok(());
    };
    // Moving up shortens every symlink's way to the top; one that reached
    // a sibling of the Wine tree would end up pointing outside the component.
    if let Some((link, problem)) = crate::download::find_escaping_symlink(&nested)? {
        let nested_name = nested.strip_prefix(dir).unwrap_or(&nested);
        return Err(Error::BrokenComponent {
            kind: ComponentKind::Wine,
            path: dir.to_path_buf(),
            message: format!(
                "cannot move {} to the top: entry {:?} {problem}",
                nested_name.display(),
                link.display().to_string()
            ),
        });
    }
    tracing::debug!(nested = %nested.display(), "moving the nested Wine tree to the top");
    crate::download::hoist_directory(dir, &nested)
}

fn has_wine_loader(dir: &Path) -> bool {
    dir.join("bin/wine").is_file() || dir.join("bin/wine64").is_file()
}

fn is_wine_tree(dir: &Path) -> bool {
    has_wine_loader(dir) && dir.join("lib/wine").is_dir()
}

/// Breadth first, by name, never following symlinks.
fn find_wine_tree(root: &Path) -> crate::Result<Option<PathBuf>> {
    let mut level = vec![root.to_path_buf()];
    for _ in 0..WINE_SEARCH_DEPTH {
        let mut next = Vec::new();
        for parent in &level {
            for child in real_subdirs(parent)? {
                if is_wine_tree(&child) {
                    return Ok(Some(child));
                }
                next.push(child);
            }
        }
        level = next;
    }
    Ok(None)
}

fn real_subdirs(dir: &Path) -> crate::Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).map_err(|e| Error::io("cannot list directory", dir, e))?;
    let mut dirs = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("cannot list directory", dir, e))?;
        let file_type = entry
            .file_type()
            .map_err(|e| Error::io("cannot inspect", entry.path(), e))?;
        if file_type.is_dir() {
            dirs.push(entry.path());
        }
    }
    dirs.sort();
    Ok(dirs)
}

fn is_real_dir(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// Remove an installed component directory. The directory is first renamed
/// to `.removing-<version>` so a half-deleted component is never listed.
///
/// # Errors
/// [`crate::Error::InvalidName`] if `version` is not a valid version,
/// [`crate::Error::NotFound`] or [`crate::Error::Io`].
pub fn remove(layout: &Layout, kind: ComponentKind, version: &str) -> crate::Result<()> {
    validate_version(version)?;
    let dir = layout.component_dir(kind, version);
    match fs::symlink_metadata(&dir) {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(Error::NotFound {
                what: "component",
                name: format!("{kind} {version}"),
                hint: "; run: uncork runtime list".to_owned(),
            });
        }
        Err(e) => return Err(Error::io("cannot inspect", dir, e)),
    }
    let doomed = layout
        .component_kind_dir(kind)
        .join(format!("{REMOVING_PREFIX}{version}"));
    remove_if_present(&doomed)?;
    fs::rename(&dir, &doomed).map_err(|e| Error::io("cannot remove", &dir, e))?;
    remove_if_present(&doomed)?;
    tracing::info!(%kind, version, "removed");
    Ok(())
}

/// Check that `dir` has the files Uncork relies on for `kind`.
/// The required paths per kind are listed in `docs/RUNTIME.md` and in
/// [`required_paths`]. Symlinks count when their target exists.
///
/// # Errors
/// [`crate::Error::BrokenComponent`] naming the first missing path.
pub fn validate_layout(kind: ComponentKind, dir: &Path) -> crate::Result<()> {
    for requirement in required_paths(kind) {
        let alternatives: Vec<&str> = requirement.split(ALTERNATIVE_SEPARATOR).collect();
        if !alternatives.iter().any(|path| dir.join(path).exists()) {
            return Err(Error::BrokenComponent {
                kind,
                path: dir.to_path_buf(),
                message: format!("missing {}", alternatives.join(" or ")),
            });
        }
    }
    Ok(())
}

/// Paths (relative to the component directory) that must exist for `kind`.
/// For Wine, any one of the listed alternatives for the loader suffices; see
/// [`crate::wine::WineRuntime`]. An entry lists its alternatives separated
/// by `|` (`bin/wine|bin/wine64`).
///
/// | Kind | Required |
/// |---|---|
/// | `wine` | `bin/wine` or `bin/wine64`, `lib/wine/x86_64-windows`, `lib/wine/x86_64-unix` |
/// | `dxmt` | `x86_64-windows/{d3d11,dxgi,winemetal}.dll`, `x86_64-unix/winemetal.so` (`i386-windows` is optional) |
/// | `dxvk` | `x86_64-windows/d3d11.dll` |
/// | `d3dmetal` | `external/D3DMetal.framework`, `wine/x86_64-windows` |
#[must_use]
pub fn required_paths(kind: ComponentKind) -> &'static [&'static str] {
    match kind {
        ComponentKind::Wine => &[
            "bin/wine|bin/wine64",
            "lib/wine/x86_64-windows",
            "lib/wine/x86_64-unix",
        ],
        ComponentKind::Dxmt => &[
            "x86_64-windows/d3d11.dll",
            "x86_64-windows/dxgi.dll",
            "x86_64-windows/winemetal.dll",
            "x86_64-unix/winemetal.so",
        ],
        ComponentKind::Dxvk => &["x86_64-windows/d3d11.dll"],
        ComponentKind::D3dmetal => &["external/D3DMetal.framework", "wine/x86_64-windows"],
    }
}

/// Import `D3DMetal` from the user's copy of Apple's Game Porting Toolkit.
///
/// `source` may be the mounted GPTK disk image (the directory containing
/// `redist/`), its `redist` directory, or its `redist/lib` directory. The
/// version is read from `D3DMetal.framework`'s `Info.plist`
/// (`CFBundleShortVersionString`, from `Resources/Info.plist`, falling back
/// to `Versions/A/Resources/Info.plist`) with `/usr/bin/plutil` and must
/// pass [`validate_version`]. Files are copied with `/bin/cp -c -R -P`
/// (APFS clones when possible; symlinks such as the `wine/x86_64-unix/*.so`
/// links to `../../external/libd3dshared.dylib` stay symlinks) into
/// `components/d3dmetal/<version>/`, preserving the `external/` and `wine/`
/// subdirectories of `redist/lib`. The result must pass [`validate_layout`].
///
/// # Errors
/// [`crate::Error::NotFound`] if no `D3DMetal.framework` is found under
/// `source`, [`crate::Error::AlreadyExists`], or I/O / command errors.
pub fn import_gptk(layout: &Layout, source: &Path) -> crate::Result<InstalledComponent> {
    // Absolute, so no path handed to a command can be mistaken for an option.
    let source = std::path::absolute(source).map_err(|e| Error::io("cannot resolve", source, e))?;
    let lib = find_gptk_lib(&source).ok_or_else(|| Error::NotFound {
        what: "D3DMetal.framework in",
        name: source.display().to_string(),
        hint: "; pass the mounted Game Porting Toolkit volume, its redist folder or redist/lib"
            .to_owned(),
    })?;
    let version = d3dmetal_version(&lib.join(GPTK_FRAMEWORK))?;
    validate_version(&version)?;
    ensure_not_installed(layout, ComponentKind::D3dmetal, &version)?;

    let meta = ComponentMeta {
        schema: META_SCHEMA,
        kind: ComponentKind::D3dmetal,
        version,
        source: Source::Local { path: lib.clone() },
        license: GPTK_LICENSE.to_owned(),
        source_code: None,
        features: Vec::new(),
        installed_unix: crate::unix_now(),
    };
    let path = install_staged(layout, &meta, |staging| copy_gptk(&lib, staging))?;
    tracing::info!(version = %meta.version, path = %path.display(), "imported D3DMetal");
    Ok(InstalledComponent { meta, path })
}

/// The `redist/lib` directory among `source`, `source/lib` and
/// `source/redist/lib`.
fn find_gptk_lib(source: &Path) -> Option<PathBuf> {
    [
        source.to_path_buf(),
        source.join("lib"),
        source.join("redist").join("lib"),
    ]
    .into_iter()
    .find(|candidate| candidate.join(GPTK_FRAMEWORK).is_dir())
}

fn d3dmetal_version(framework: &Path) -> crate::Result<String> {
    let plists = [
        framework.join("Resources/Info.plist"),
        framework.join("Versions/A/Resources/Info.plist"),
    ];
    let mut last_error = None;
    for plist in plists.iter().filter(|plist| plist.is_file()) {
        match plist_string(plist, "CFBundleShortVersionString") {
            Ok(version) => return Ok(version),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.unwrap_or_else(|| Error::NotFound {
        what: "Info.plist in",
        name: framework.display().to_string(),
        hint: String::new(),
    }))
}

/// A string value from a property list, read with `plutil`.
fn plist_string(plist: &Path, key: &str) -> crate::Result<String> {
    let mut command = Command::new(PLUTIL);
    command.args(["-extract", key, "raw"]).arg(plist);
    let stdout = run(&mut command, "plutil")?;
    let value = stdout.trim();
    if value.is_empty() {
        return Err(Error::Command {
            program: "plutil".to_owned(),
            status: format!("printed an empty {key} for {}", plist.display()),
            log_hint: String::new(),
        });
    }
    Ok(value.to_owned())
}

fn copy_gptk(lib: &Path, staging: &Path) -> crate::Result<()> {
    fs::create_dir(staging).map_err(|e| Error::io("cannot create directory", staging, e))?;
    for name in GPTK_SUBDIRS {
        let from = lib.join(name);
        if !from.exists() {
            // validate_layout names whatever turns out to be missing.
            continue;
        }
        let mut command = Command::new(CP);
        // -c: clone on APFS; -R -P: recurse, copy symlinks as symlinks.
        command
            .args(["-c", "-R", "-P"])
            .arg(&from)
            .arg(staging.join(name));
        run(&mut command, "cp")?;
    }
    Ok(())
}

/// Run `command` to completion; its stdout on success.
fn run(command: &mut Command, program: &str) -> crate::Result<String> {
    let output = command.output().map_err(|e| Error::Command {
        program: program.to_owned(),
        status: format!("could not start: {e}"),
        log_hint: String::new(),
    })?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let status = output.status.code().map_or_else(
        || "was killed by a signal".to_owned(),
        |code| format!("exited with status {code}"),
    );
    // plutil reports problems on stdout, cp on stderr.
    let detail = [&output.stderr, &output.stdout]
        .into_iter()
        .map(|bytes| String::from_utf8_lossy(bytes).trim().to_owned())
        .find(|text| !text.is_empty());
    Err(Error::Command {
        program: program.to_owned(),
        status: detail.map_or_else(|| status.clone(), |detail| format!("{status}: {detail}")),
        log_hint: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    #[test]
    fn kinds_parse_case_insensitively_with_aliases() {
        let cases = [
            ("wine", ComponentKind::Wine),
            ("WINE", ComponentKind::Wine),
            ("dxmt", ComponentKind::Dxmt),
            ("DxMt", ComponentKind::Dxmt),
            ("dxvk", ComponentKind::Dxvk),
            ("d3dmetal", ComponentKind::D3dmetal),
            ("D3DMetal", ComponentKind::D3dmetal),
            ("d3d-metal", ComponentKind::D3dmetal),
            ("GPTK", ComponentKind::D3dmetal),
        ];
        for (text, kind) in cases {
            assert_eq!(text.parse::<ComponentKind>(), Ok(kind), "{text}");
        }
    }

    #[test]
    fn unknown_kinds_are_rejected_with_the_choices() {
        for text in ["", "proton", " wine", "d3d_metal", "all"] {
            let error = text.parse::<ComponentKind>().unwrap_err();
            assert!(error.contains("wine, dxmt, dxvk or d3dmetal"), "{error}");
        }
    }

    #[test]
    fn kinds_round_trip_through_display() {
        for kind in ComponentKind::ALL {
            assert_eq!(kind.to_string().parse::<ComponentKind>(), Ok(kind));
        }
    }

    #[test]
    fn valid_versions() {
        let longest = "9".repeat(MAX_VERSION_LEN);
        for version in [
            "0.80",
            "1.10.3-20230507",
            "sikarugir-11.0_1",
            "_x",
            "a+b",
            "A",
            longest.as_str(),
        ] {
            assert!(validate_version(version).is_ok(), "{version}");
        }
    }

    #[test]
    fn invalid_versions() {
        let too_long = "9".repeat(MAX_VERSION_LEN + 1);
        let cases = [
            ("", "empty"),
            (".", "start"),
            ("..", "start"),
            (".hidden", "start"),
            ("-rc", "start"),
            ("a/b", "only contain"),
            ("../../etc", "only contain"),
            ("1 0", "only contain"),
            ("1.0\n", "only contain"),
            ("versión", "only contain"),
            (too_long.as_str(), "64"),
        ];
        for (version, reason_part) in cases {
            match validate_version(version) {
                Err(Error::InvalidName { what, name, reason }) => {
                    assert_eq!(what, "version");
                    assert_eq!(name, version);
                    assert!(reason.contains(reason_part), "{version:?}: {reason}");
                }
                other => panic!("{version:?} gave {other:?}"),
            }
        }
    }

    #[test]
    fn versions_compare_naturally() {
        let ascending = [
            "0.8",
            "0.9",
            "0.10",
            "1",
            "1.0",
            "1.0.1",
            "1.9",
            "1.10",
            "1.10.3-20230507",
            "2",
            "10",
            "11.0-uncork1",
            "11.0-uncork2",
            "11.0-uncork10",
            "a",
            "b",
        ];
        for pair in ascending.windows(2) {
            assert_eq!(
                compare_versions(pair[0], pair[1]),
                Ordering::Less,
                "{} < {}",
                pair[0],
                pair[1]
            );
            assert_eq!(
                compare_versions(pair[1], pair[0]),
                Ordering::Greater,
                "{} > {}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn leading_zeros_are_numerically_equal_but_still_ordered() {
        assert_eq!(compare_versions("1.01", "1.01"), Ordering::Equal);
        assert_ne!(compare_versions("1.01", "1.1"), Ordering::Equal);
        assert_eq!(compare_versions("1.01", "1.2"), Ordering::Less);
        assert_eq!(compare_versions("007", "8"), Ordering::Less);
    }

    #[test]
    fn huge_numbers_do_not_overflow() {
        let big = "1".repeat(60);
        let bigger = format!("{big}0");
        assert_eq!(compare_versions(&big, &bigger), Ordering::Less);
    }

    #[test]
    fn chunks_split_digit_runs() {
        assert_eq!(
            chunks("11.0-uncork2").collect::<Vec<_>>(),
            ["11", ".", "0", "-uncork", "2"]
        );
        assert_eq!(chunks("é1").collect::<Vec<_>>(), ["é", "1"]);
        assert_eq!(chunks("").count(), 0);
    }

    proptest! {
        #[test]
        fn compare_is_antisymmetric(a in "[0-9a-z._+-]{0,12}", b in "[0-9a-z._+-]{0,12}") {
            prop_assert_eq!(compare_versions(&a, &b), compare_versions(&b, &a).reverse());
        }

        #[test]
        fn compare_is_equal_exactly_for_equal_strings(a in "[0-9.]{0,8}", b in "[0-9.]{0,8}") {
            prop_assert_eq!(compare_versions(&a, &b) == Ordering::Equal, a == b);
            prop_assert_eq!(compare_versions(&a, &a), Ordering::Equal);
        }

        #[test]
        fn compare_is_transitive(
            a in "[0-9a.-]{0,6}",
            b in "[0-9a.-]{0,6}",
            c in "[0-9a.-]{0,6}",
        ) {
            let mut sorted = [a, b, c];
            sorted.sort_by(|x, y| compare_versions(x, y));
            prop_assert_ne!(compare_versions(&sorted[0], &sorted[1]), Ordering::Greater);
            prop_assert_ne!(compare_versions(&sorted[1], &sorted[2]), Ordering::Greater);
            prop_assert_ne!(compare_versions(&sorted[0], &sorted[2]), Ordering::Greater);
        }

        #[test]
        fn numeric_parts_compare_as_numbers(x in 0u64..1_000_000, y in 0u64..1_000_000) {
            prop_assert_eq!(compare_versions(&format!("1.{x}"), &format!("1.{y}")), x.cmp(&y));
        }
    }

    #[test]
    fn required_paths_cover_every_kind() {
        for kind in ComponentKind::ALL {
            assert!(!required_paths(kind).is_empty(), "{kind}");
        }
        assert!(required_paths(ComponentKind::Wine).contains(&"bin/wine|bin/wine64"));
    }

    fn touch(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"").unwrap();
    }

    fn make_layout(root: &Path, kind: ComponentKind) {
        for requirement in required_paths(kind) {
            let first = requirement.split(ALTERNATIVE_SEPARATOR).next().unwrap();
            if first.ends_with(".dll") || first.ends_with(".so") || first.starts_with("bin/") {
                touch(root, first);
            } else {
                fs::create_dir_all(root.join(first)).unwrap();
            }
        }
    }

    #[test]
    fn complete_layouts_validate() {
        for kind in ComponentKind::ALL {
            let dir = tempfile::tempdir().unwrap();
            make_layout(dir.path(), kind);
            validate_layout(kind, dir.path()).unwrap();
        }
    }

    #[test]
    fn wine_accepts_either_loader() {
        for loader in ["bin/wine", "bin/wine64"] {
            let dir = tempfile::tempdir().unwrap();
            touch(dir.path(), loader);
            fs::create_dir_all(dir.path().join("lib/wine/x86_64-windows")).unwrap();
            fs::create_dir_all(dir.path().join("lib/wine/x86_64-unix")).unwrap();
            validate_layout(ComponentKind::Wine, dir.path()).unwrap();
        }
    }

    #[test]
    fn missing_paths_are_named() {
        let dir = tempfile::tempdir().unwrap();
        match validate_layout(ComponentKind::Wine, dir.path()) {
            Err(Error::BrokenComponent {
                kind,
                path,
                message,
            }) => {
                assert_eq!(kind, ComponentKind::Wine);
                assert_eq!(path, dir.path());
                assert_eq!(message, "missing bin/wine or bin/wine64");
            }
            other => panic!("expected a broken component, got {other:?}"),
        }
        make_layout(dir.path(), ComponentKind::Dxmt);
        fs::remove_file(dir.path().join("x86_64-unix/winemetal.so")).unwrap();
        let error = validate_layout(ComponentKind::Dxmt, dir.path()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing x86_64-unix/winemetal.so"),
            "{error}"
        );
    }

    #[test]
    fn dangling_symlinks_do_not_count() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("x86_64-windows")).unwrap();
        std::os::unix::fs::symlink("nowhere.dll", dir.path().join("x86_64-windows/d3d11.dll"))
            .unwrap();
        assert!(validate_layout(ComponentKind::Dxvk, dir.path()).is_err());
    }

    #[test]
    fn dxvk_arch_dirs_are_renamed_unless_taken() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "x64/d3d11.dll");
        touch(dir.path(), "x32/d3d11.dll");
        normalize_layout(ComponentKind::Dxvk, dir.path()).unwrap();
        assert!(dir.path().join("x86_64-windows/d3d11.dll").is_file());
        assert!(dir.path().join("i386-windows/d3d11.dll").is_file());
        assert!(!dir.path().join("x64").exists());

        let taken = tempfile::tempdir().unwrap();
        touch(taken.path(), "x64/a.dll");
        touch(taken.path(), "x86_64-windows/b.dll");
        normalize_layout(ComponentKind::Dxvk, taken.path()).unwrap();
        assert!(taken.path().join("x64/a.dll").is_file());
        assert!(!taken.path().join("x86_64-windows/a.dll").exists());
    }

    #[test]
    fn wine_at_the_top_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "bin/wine");
        touch(dir.path(), "other/bin/wine");
        fs::create_dir_all(dir.path().join("other/lib/wine")).unwrap();
        normalize_layout(ComponentKind::Wine, dir.path()).unwrap();
        assert!(dir.path().join("other/bin/wine").is_file());
    }

    #[test]
    fn nested_wine_trees_are_found_breadth_first() {
        let dir = tempfile::tempdir().unwrap();
        for tree in ["b/deep/deeper", "a/x", "c"] {
            touch(dir.path(), &format!("{tree}/bin/wine64"));
            fs::create_dir_all(dir.path().join(tree).join("lib/wine")).unwrap();
        }
        assert_eq!(
            find_wine_tree(dir.path()).unwrap(),
            Some(dir.path().join("c"))
        );
    }

    #[test]
    fn wine_trees_deeper_than_the_limit_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "1/2/3/4/bin/wine");
        fs::create_dir_all(dir.path().join("1/2/3/4/lib/wine")).unwrap();
        assert_eq!(find_wine_tree(dir.path()).unwrap(), None);
        touch(dir.path(), "1/2/3/bin/wine");
        fs::create_dir_all(dir.path().join("1/2/3/lib/wine")).unwrap();
        assert_eq!(
            find_wine_tree(dir.path()).unwrap(),
            Some(dir.path().join("1/2/3"))
        );
    }

    #[test]
    fn wine_search_does_not_follow_symlinks() {
        let outside = tempfile::tempdir().unwrap();
        touch(outside.path(), "bin/wine");
        fs::create_dir_all(outside.path().join("lib/wine")).unwrap();
        let dir = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        assert_eq!(find_wine_tree(dir.path()).unwrap(), None);
    }

    #[test]
    fn download_names_use_the_short_hash_and_url_file_name() {
        let mut entry = crate::catalog::Catalog::builtin().components[0].clone();
        entry.sha256 = "0123456789abcdef".repeat(4);
        entry.url = "https://example.com/a/Libraries.tar.gz?raw=1".to_owned();
        assert_eq!(
            download_file_name(&entry),
            "0123456789abcdef-Libraries.tar.gz"
        );
        entry.url = "https://example.com/a/".to_owned();
        assert_eq!(download_file_name(&entry), "0123456789abcdef-archive");
    }

    #[test]
    fn metadata_round_trips_through_toml() {
        let metas = [
            ComponentMeta {
                schema: 1,
                kind: ComponentKind::Wine,
                version: "11.0".to_owned(),
                source: Source::Catalog {
                    url: "https://example.com/w.tar.xz".to_owned(),
                    sha256: "ab".repeat(32),
                },
                license: "LGPL-2.1-or-later".to_owned(),
                source_code: Some("https://example.com/src".to_owned()),
                features: vec!["wow64".to_owned(), "msync".to_owned()],
                installed_unix: 1_790_000_000,
            },
            ComponentMeta {
                schema: 1,
                kind: ComponentKind::D3dmetal,
                version: "3.0".to_owned(),
                source: Source::Local {
                    path: PathBuf::from("/Volumes/GPTK/redist/lib"),
                },
                license: GPTK_LICENSE.to_owned(),
                source_code: None,
                features: Vec::new(),
                installed_unix: 0,
            },
        ];
        for meta in metas {
            let dir = tempfile::tempdir().unwrap();
            write_meta(dir.path(), &meta).unwrap();
            assert_eq!(read_meta(dir.path()).unwrap(), meta);
        }
    }

    #[test]
    fn run_reports_exit_status_and_output() {
        let error = run(
            Command::new("/bin/sh").args(["-c", "echo oops >&2; exit 3"]),
            "sh",
        )
        .unwrap_err();
        match error {
            Error::Command {
                program, status, ..
            } => {
                assert_eq!(program, "sh");
                assert_eq!(status, "exited with status 3: oops");
            }
            other => panic!("expected a command error, got {other:?}"),
        }
        let missing = run(&mut Command::new("/nonexistent/uncork-test"), "x").unwrap_err();
        assert!(
            missing.to_string().starts_with("x could not start:"),
            "{missing}"
        );
        assert_eq!(
            run(Command::new("/bin/echo").arg("hi"), "echo").unwrap(),
            "hi\n"
        );
    }
}
