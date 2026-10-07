//! HTTPS downloads with streaming SHA-256 verification, and archive unpacking.

use std::path::Path;

use crate::catalog::ArchiveFormat;

/// Download progress sink. The CLI draws a bar; tests use [`NoProgress`].
pub trait Progress {
    /// A download of `total` bytes (if known) is starting.
    fn start(&mut self, label: &str, total: Option<u64>);
    /// `bytes` more bytes arrived.
    fn advance(&mut self, bytes: u64);
    /// The download finished (successfully or not).
    fn finish(&mut self);
}

/// A [`Progress`] that ignores everything.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoProgress;

impl Progress for NoProgress {
    fn start(&mut self, _label: &str, _total: Option<u64>) {}
    fn advance(&mut self, _bytes: u64) {}
    fn finish(&mut self) {}
}

/// User-Agent sent with every request.
pub const USER_AGENT: &str = concat!("uncork/", env!("CARGO_PKG_VERSION"), " (+https://github.com/tonianev/uncork)");

/// SHA-256 of a file, lowercase hex.
///
/// # Errors
/// [`crate::Error::Io`].
pub fn sha256_file(path: &Path) -> crate::Result<String> {
    let _ = path;
    todo!()
}

/// Download `url` to `dest`, verifying SHA-256 while streaming.
///
/// - If `dest` already exists and hashes to `expected_sha256`, returns
///   immediately without network access.
/// - Writes to `dest` + `.part`, then renames on success; the `.part` file
///   is removed on failure.
/// - Only `https://` URLs are accepted (redirects to https are followed).
/// - `expected_sha256` of `None` skips verification (used only for Valve's
///   unversioned `SteamSetup.exe`, which is checked by
///   [`uncork_pe::inspect`] afterwards instead).
///
/// # Errors
/// [`crate::Error::Download`], [`crate::Error::Checksum`] or [`crate::Error::Io`].
pub fn download(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    progress: &mut dyn Progress,
) -> crate::Result<()> {
    let _ = (url, dest, expected_sha256, progress);
    todo!()
}

/// Unpack `archive` into `dest`, which must not exist yet (it is created).
///
/// Safety rules, each violation an [`crate::Error::Archive`]: no absolute
/// paths, no `..` components, no entry outside `strip_prefix` when one is
/// given (the prefix itself is dropped from every path), no symlink whose
/// target is absolute or resolves outside `dest`, no hard links, no device
/// files. Regular files keep their permission bits (masked with 0o755 for
/// executables, 0o644 otherwise); directories are 0o755.
///
/// # Errors
/// [`crate::Error::Archive`] or [`crate::Error::Io`].
pub fn extract(
    archive: &Path,
    format: ArchiveFormat,
    dest: &Path,
    strip_prefix: Option<&str>,
) -> crate::Result<()> {
    let _ = (archive, format, dest, strip_prefix);
    todo!()
}
