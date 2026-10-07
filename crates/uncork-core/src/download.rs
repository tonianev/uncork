//! HTTPS downloads with streaming SHA-256 verification, and archive unpacking.

use std::collections::HashSet;
use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufReader, Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt, symlink};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tar::EntryType;

use crate::Error;
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
pub const USER_AGENT: &str = concat!(
    "uncork/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/tonianev/uncork)"
);

/// Read and write buffer size for hashing, downloading and unpacking.
const BUFFER_SIZE: usize = 256 * 1024;

/// Upper bound for opening a connection, TLS handshake included.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Upper bound for the server to answer once the request is sent. The body
/// itself has no deadline: a large runtime on a slow line takes a while.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

/// Suffix of the file a download is written to before it is verified.
const PART_SUFFIX: &str = ".part";

/// Directories created while unpacking are `rwxr-xr-x`.
const DIR_MODE: u32 = 0o755;

/// SHA-256 of a file, lowercase hex.
///
/// # Errors
/// [`crate::Error::Io`].
pub fn sha256_file(path: &Path) -> crate::Result<String> {
    let mut file = File::open(path).map_err(|e| Error::io("cannot open", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        let read = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(Error::io("cannot read", path, e)),
        };
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
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
/// - `progress` is told about the transfer once the server has answered,
///   with the file name from the URL as label and `Content-Length` as total.
/// - Requests carry [`USER_AGENT`]. `dest`'s parent directory is created
///   when missing.
///
/// # Errors
/// [`crate::Error::Download`], [`crate::Error::Checksum`] or [`crate::Error::Io`].
pub fn download(
    url: &str,
    dest: &Path,
    expected_sha256: Option<&str>,
    progress: &mut dyn Progress,
) -> crate::Result<()> {
    if !url.starts_with("https://") {
        return Err(Error::Download {
            url: url.to_owned(),
            message: "only https:// URLs are allowed".to_owned(),
        });
    }
    if let Some(expected) = expected_sha256
        && is_already_downloaded(dest, expected)?
    {
        tracing::debug!(path = %dest.display(), "already downloaded and verified");
        return Ok(());
    }
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| Error::io("cannot create directory", parent, e))?;
    }

    tracing::info!(%url, path = %dest.display(), "downloading");
    let response = get(url)?;
    let total = response.body().content_length();
    let body = response.into_body().into_reader();
    save_verified(url, body, total, dest, expected_sha256, progress)
}

/// The last path segment of `url`, without query or fragment. May be empty.
pub(crate) fn file_name_from_url(url: &str) -> &str {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rsplit('/').next().unwrap_or(path)
}

fn is_already_downloaded(dest: &Path, expected: &str) -> crate::Result<bool> {
    match fs::metadata(dest) {
        Ok(meta) if meta.is_file() => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io("cannot inspect", dest, e)),
    }
    let actual = sha256_file(dest)?;
    if actual.eq_ignore_ascii_case(expected) {
        return Ok(true);
    }
    tracing::warn!(path = %dest.display(), "existing file does not match its checksum; downloading again");
    Ok(false)
}

fn get(url: &str) -> crate::Result<ureq::http::Response<ureq::Body>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .user_agent(USER_AGENT)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .build()
        .into();
    agent.get(url).call().map_err(|error| Error::Download {
        url: url.to_owned(),
        message: error.to_string(),
    })
}

/// Stream `body` into `dest.part` while hashing it, check the hash, and move
/// the file into place. The `.part` file never survives a failure.
fn save_verified(
    url: &str,
    body: impl Read,
    total: Option<u64>,
    dest: &Path,
    expected_sha256: Option<&str>,
    progress: &mut dyn Progress,
) -> crate::Result<()> {
    let part = part_path(dest);
    progress.start(file_name_from_url(url), total);
    let streamed = stream_to_file(url, body, &part, progress);
    progress.finish();

    let result = streamed
        .and_then(|actual| check_checksum(url, expected_sha256, actual))
        .and_then(|()| fs::rename(&part, dest).map_err(|e| Error::io("cannot rename", &part, e)));
    if result.is_err() {
        remove_quietly(&part);
    }
    result
}

/// Copy `body` to a new file at `path`, returning the SHA-256 of the bytes.
fn stream_to_file(
    url: &str,
    mut body: impl Read,
    path: &Path,
    progress: &mut dyn Progress,
) -> crate::Result<String> {
    let mut file = File::create(path).map_err(|e| Error::io("cannot create", path, e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; BUFFER_SIZE];
    loop {
        let read = match body.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => {
                return Err(Error::Download {
                    url: url.to_owned(),
                    message: e.to_string(),
                });
            }
        };
        let chunk = &buffer[..read];
        hasher.update(chunk);
        file.write_all(chunk)
            .map_err(|e| Error::io("cannot write", path, e))?;
        progress.advance(read as u64);
    }
    file.sync_all()
        .map_err(|e| Error::io("cannot write", path, e))?;
    Ok(hex::encode(hasher.finalize()))
}

fn check_checksum(url: &str, expected: Option<&str>, actual: String) -> crate::Result<()> {
    match expected {
        Some(expected) if !actual.eq_ignore_ascii_case(expected) => Err(Error::Checksum {
            url: url.to_owned(),
            expected: expected.to_owned(),
            actual,
        }),
        _ => Ok(()),
    }
}

fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(PART_SUFFIX);
    PathBuf::from(name)
}

fn remove_quietly(path: &Path) {
    if let Err(e) = fs::remove_file(path)
        && e.kind() != io::ErrorKind::NotFound
    {
        tracing::warn!(path = %path.display(), error = %e, "cannot remove partial download");
    }
}

/// Unpack `archive` into `dest`, which must not exist yet (it is created,
/// along with missing parents, and removed again if unpacking fails).
///
/// Safety rules, each violation an [`crate::Error::Archive`]: no absolute
/// paths, no `..` components, no entry outside `strip_prefix` when one is
/// given (the prefix itself is dropped from every path), no symlink whose
/// target is absolute or resolves outside `dest`, no hard links, no device
/// files or FIFOs, and nothing is ever written through a symlink. A symlink
/// target may only use `..` at its start (`../../lib/x.dylib`, not
/// `a/../../x`), which keeps "resolves inside `dest`" decidable from the
/// text alone. Regular files keep their permission bits, masked with 0o755
/// when any execute bit is set and 0o644 otherwise, and are always readable
/// and writable by their owner; directories are 0o755. Ownership, times and
/// extended attributes are not restored. pax global headers are ignored.
///
/// When `strip_prefix` is `None` and every entry lives under one single
/// top-level directory (release tarballs usually wrap their contents in
/// `<name>-<version>/`), that directory is stripped as if it had been given
/// as `strip_prefix`. `Some("")` disables this and keeps paths as they are.
///
/// # Errors
/// [`crate::Error::Archive`] or [`crate::Error::Io`].
pub fn extract(
    archive: &Path,
    format: ArchiveFormat,
    dest: &Path,
    strip_prefix: Option<&str>,
) -> crate::Result<()> {
    let file = File::open(archive).map_err(|e| Error::io("cannot open", archive, e))?;
    let reader = BufReader::with_capacity(BUFFER_SIZE, file);
    if let Some(parent) = dest.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent).map_err(|e| Error::io("cannot create directory", parent, e))?;
    }
    create_dir(dest)?;

    let unpacker = Unpacker::new(archive, dest, strip_prefix);
    let result = match format {
        ArchiveFormat::TarGz => unpacker.run(flate2::read::MultiGzDecoder::new(reader)),
        ArchiveFormat::TarXz => unpacker.run(liblzma::read::XzDecoder::new_multi_decoder(reader)),
    };
    if result.is_err()
        && let Err(e) = fs::remove_dir_all(dest)
    {
        tracing::warn!(path = %dest.display(), error = %e, "cannot clean up after a failed extraction");
    }
    result
}

/// Move everything inside `nested`, a directory somewhere below `root`, up
/// into `root`. `nested` and any of its ancestors left empty are removed.
///
/// # Errors
/// [`crate::Error::Io`], also when an entry of `nested` would replace an
/// existing entry of `root`.
pub(crate) fn hoist_directory(root: &Path, nested: &Path) -> crate::Result<()> {
    let children = child_names(nested)?;
    // Park `nested` directly under `root` under a name nothing else uses, so
    // its ancestors can be cleared away and a child that shares a name with
    // one of them (`a/b/a`) can still take its place.
    let parked = root.join(unused_name(root, &children));
    fs::rename(nested, &parked).map_err(|e| Error::io("cannot move", nested, e))?;
    remove_empty_ancestors(root, nested)?;

    for name in &children {
        let to = root.join(name);
        if fs::symlink_metadata(&to).is_ok() {
            return Err(Error::io(
                "cannot move nested contents onto",
                to,
                io::ErrorKind::AlreadyExists.into(),
            ));
        }
        let from = parked.join(name);
        fs::rename(&from, &to).map_err(|e| Error::io("cannot move", from, e))?;
    }
    fs::remove_dir(&parked).map_err(|e| Error::io("cannot remove directory", parked, e))
}

/// The first symlink below `root` whose target is absolute or leaves `root`
/// (see [`extract`] for the rule), as its path relative to `root` and the
/// problem. Run before moving a subtree up, which shortens every link's
/// distance to the top.
///
/// # Errors
/// [`crate::Error::Io`] if the tree cannot be read.
pub(crate) fn find_escaping_symlink(root: &Path) -> crate::Result<Option<(PathBuf, String)>> {
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let dir = root.join(&relative);
        let entries =
            fs::read_dir(&dir).map_err(|e| Error::io("cannot list directory", &dir, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| Error::io("cannot list directory", &dir, e))?;
            let path = entry.path();
            let file_type = entry
                .file_type()
                .map_err(|e| Error::io("cannot inspect", &path, e))?;
            let child = relative.join(entry.file_name());
            if file_type.is_symlink() {
                let target =
                    fs::read_link(&path).map_err(|e| Error::io("cannot read symlink", &path, e))?;
                if let Err(problem) = check_symlink(&child, &target) {
                    return Ok(Some((child, problem)));
                }
            } else if file_type.is_dir() {
                pending.push(child);
            }
        }
    }
    Ok(None)
}

fn child_names(dir: &Path) -> crate::Result<Vec<OsString>> {
    let entries = fs::read_dir(dir).map_err(|e| Error::io("cannot list directory", dir, e))?;
    let mut names = Vec::new();
    for entry in entries {
        names.push(
            entry
                .map_err(|e| Error::io("cannot list directory", dir, e))?
                .file_name(),
        );
    }
    Ok(names)
}

fn unused_name(dir: &Path, taken: &[OsString]) -> OsString {
    (0u32..)
        .map(|n| OsString::from(format!(".uncork-hoist-{n}")))
        .find(|name| !taken.contains(name) && fs::symlink_metadata(dir.join(name)).is_err())
        .unwrap_or_else(|| OsString::from(".uncork-hoist"))
}

/// Remove the now-empty former parents of `moved`, stopping at `root` or at
/// the first directory that still has something in it.
fn remove_empty_ancestors(root: &Path, moved: &Path) -> crate::Result<()> {
    for dir in moved
        .ancestors()
        .skip(1)
        .take_while(|dir| *dir != root && dir.starts_with(root))
    {
        let is_empty = fs::read_dir(dir)
            .map_err(|e| Error::io("cannot list directory", dir, e))?
            .next()
            .is_none();
        if !is_empty {
            break;
        }
        fs::remove_dir(dir).map_err(|e| Error::io("cannot remove directory", dir, e))?;
    }
    Ok(())
}

/// Top-level names seen so far, to decide whether to strip automatically.
#[derive(Debug)]
enum TopLevel {
    /// No entry yet.
    Empty,
    /// Every entry so far is this directory or lives below it.
    Directory(OsString),
    /// More than one top-level name, or a top-level file or symlink.
    Mixed,
}

impl TopLevel {
    /// Account for an entry at `relative` (never empty).
    fn observe(&mut self, relative: &Path, is_dir: bool) {
        let mut components = relative.components();
        let Some(first) = components.next() else {
            return;
        };
        let first = first.as_os_str();
        let is_top_level_non_dir = components.next().is_none() && !is_dir;
        *self = match std::mem::replace(self, TopLevel::Mixed) {
            _ if is_top_level_non_dir => TopLevel::Mixed,
            TopLevel::Empty => TopLevel::Directory(first.to_owned()),
            TopLevel::Directory(name) if name == first => TopLevel::Directory(name),
            TopLevel::Directory(_) | TopLevel::Mixed => TopLevel::Mixed,
        };
    }
}

/// One extraction: the safety checks and the bookkeeping they need.
struct Unpacker<'a> {
    archive: &'a Path,
    dest: &'a Path,
    strip_prefix: Option<&'a str>,
    top_level: TopLevel,
    /// Directories (relative to `dest`) known to be real directories, not
    /// symlinks. A directory is never replaced, so this never goes stale.
    real_dirs: HashSet<PathBuf>,
    /// Copy buffer shared by every file.
    buffer: Vec<u8>,
}

impl<'a> Unpacker<'a> {
    fn new(archive: &'a Path, dest: &'a Path, strip_prefix: Option<&'a str>) -> Unpacker<'a> {
        Unpacker {
            archive,
            dest,
            strip_prefix,
            top_level: TopLevel::Empty,
            real_dirs: HashSet::new(),
            buffer: vec![0; BUFFER_SIZE],
        }
    }

    fn run(mut self, reader: impl Read) -> crate::Result<()> {
        let mut archive = tar::Archive::new(reader);
        let entries = archive.entries().map_err(|e| self.corrupt(&e))?;
        for entry in entries {
            let mut entry = entry.map_err(|e| self.corrupt(&e))?;
            self.unpack_entry(&mut entry)?;
        }
        self.finish()
    }

    fn unpack_entry(&mut self, entry: &mut tar::Entry<'_, impl Read>) -> crate::Result<()> {
        let entry_type = entry.header().entry_type();
        if entry_type == EntryType::XGlobalHeader {
            return Ok(());
        }
        let raw = entry.path().map_err(|e| self.corrupt(&e))?.into_owned();
        let relative = self
            .relative_path(&raw)
            .map_err(|problem| self.unsafe_entry(&raw, &problem))?;
        let is_dir = entry_type == EntryType::Directory;
        if relative.as_os_str().is_empty() {
            // The archive root itself (`./` or the stripped prefix).
            return if is_dir {
                Ok(())
            } else {
                Err(self.unsafe_entry(
                    &raw,
                    "is not a directory but takes the place of the destination",
                ))
            };
        }
        if self.strip_prefix.is_none() {
            self.top_level.observe(&relative, is_dir);
        }

        match entry_type {
            EntryType::Directory => self.make_dir(&raw, &relative),
            EntryType::Regular | EntryType::Continuous | EntryType::GNUSparse => {
                let mode = entry.header().mode().map_err(|e| self.corrupt(&e))?;
                self.write_file(&raw, &relative, entry, mode)
            }
            EntryType::Symlink => {
                let target = entry
                    .link_name()
                    .map_err(|e| self.corrupt(&e))?
                    .ok_or_else(|| self.unsafe_entry(&raw, "is a symlink without a target"))?
                    .into_owned();
                self.make_symlink(&raw, &relative, &target)
            }
            EntryType::Link => Err(self.unsafe_entry(&raw, "is a hard link")),
            EntryType::Char | EntryType::Block => Err(self.unsafe_entry(&raw, "is a device file")),
            EntryType::Fifo => Err(self.unsafe_entry(&raw, "is a FIFO")),
            other => Err(self.unsafe_entry(
                &raw,
                &format!("has unsupported type {:?}", char::from(other.as_byte())),
            )),
        }
    }

    /// `raw` relative to `dest`: `.` components dropped and the strip prefix
    /// removed. Empty for the archive root itself.
    fn relative_path(&self, raw: &Path) -> Result<PathBuf, String> {
        let mut normal = PathBuf::new();
        for component in raw.components() {
            match component {
                Component::Normal(name) => normal.push(name),
                Component::CurDir => {}
                Component::ParentDir => return Err("contains a '..' component".to_owned()),
                Component::RootDir | Component::Prefix(_) => {
                    return Err("is an absolute path".to_owned());
                }
            }
        }
        match self.strip_prefix.filter(|prefix| !prefix.is_empty()) {
            None => Ok(normal),
            Some(prefix) => normal
                .strip_prefix(prefix)
                .map(Path::to_path_buf)
                .map_err(|_| format!("is outside the top-level directory {prefix:?}")),
        }
    }

    fn make_dir(&mut self, raw: &Path, relative: &Path) -> crate::Result<()> {
        self.create_parents(raw, relative)?;
        let path = self.dest.join(relative);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => set_mode(&path, DIR_MODE)?,
            Ok(_) => return Err(self.unsafe_entry(raw, "would replace a file with a directory")),
            Err(e) if e.kind() == io::ErrorKind::NotFound => create_dir(&path)?,
            Err(e) => return Err(Error::io("cannot inspect", path, e)),
        }
        self.real_dirs.insert(relative.to_path_buf());
        Ok(())
    }

    fn write_file(
        &mut self,
        raw: &Path,
        relative: &Path,
        contents: &mut impl Read,
        header_mode: u32,
    ) -> crate::Result<()> {
        self.create_parents(raw, relative)?;
        let path = self.dest.join(relative);
        self.clear_for_non_dir(raw, &path)?;
        // `create_new` refuses to follow a symlink planted at `path`.
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
            .map_err(|e| Error::io("cannot create", &path, e))?;
        match copy(&mut self.buffer, contents, &mut file) {
            Ok(()) => {}
            Err(CopyError::Read(e)) => return Err(self.corrupt(&e)),
            Err(CopyError::Write(e)) => return Err(Error::io("cannot write", &path, e)),
        }
        file.set_permissions(fs::Permissions::from_mode(file_mode(header_mode)))
            .map_err(|e| Error::io("cannot set permissions of", &path, e))
    }

    fn make_symlink(&mut self, raw: &Path, relative: &Path, target: &Path) -> crate::Result<()> {
        check_symlink(relative, target).map_err(|problem| self.unsafe_entry(raw, &problem))?;
        self.create_parents(raw, relative)?;
        let path = self.dest.join(relative);
        self.clear_for_non_dir(raw, &path)?;
        symlink(target, &path).map_err(|e| Error::io("cannot create symlink", &path, e))
    }

    /// Create the missing directories leading to `relative`, refusing to pass
    /// through anything that is not a real directory.
    fn create_parents(&mut self, raw: &Path, relative: &Path) -> crate::Result<()> {
        let Some(parent) = relative.parent() else {
            return Ok(());
        };
        let mut current = PathBuf::new();
        for component in parent.components() {
            current.push(component);
            if self.real_dirs.contains(&current) {
                continue;
            }
            let path = self.dest.join(&current);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.is_dir() => {}
                Ok(_) => {
                    return Err(self.unsafe_entry(
                        raw,
                        &format!("lies below {}, which is not a directory", current.display()),
                    ));
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => create_dir(&path)?,
                Err(e) => return Err(Error::io("cannot inspect", path, e)),
            }
            self.real_dirs.insert(current.clone());
        }
        Ok(())
    }

    /// Make room for a file or symlink at `path`. A later entry for the same
    /// path replaces an earlier file or symlink, never a directory.
    fn clear_for_non_dir(&self, raw: &Path, path: &Path) -> crate::Result<()> {
        match fs::symlink_metadata(path) {
            Ok(meta) if meta.is_dir() => Err(self.unsafe_entry(raw, "would replace a directory")),
            Ok(_) => fs::remove_file(path).map_err(|e| Error::io("cannot replace", path, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::io("cannot inspect", path, e)),
        }
    }

    /// Strip a lone top-level directory unless told otherwise.
    fn finish(self) -> crate::Result<()> {
        if self.strip_prefix.is_some() {
            return Ok(());
        }
        let TopLevel::Directory(top) = &self.top_level else {
            return Ok(());
        };
        // Every symlink moves one level up: check again that its target still
        // stays inside `dest`.
        let nested = self.dest.join(top);
        if let Some((link, problem)) = find_escaping_symlink(&nested)? {
            return Err(self.unsafe_entry(&Path::new(top).join(link), &problem));
        }
        tracing::debug!(top = %nested.display(), "stripping the archive's top-level directory");
        hoist_directory(self.dest, &nested)
    }

    fn unsafe_entry(&self, raw: &Path, problem: &str) -> Error {
        Error::Archive {
            path: self.archive.to_path_buf(),
            message: format!("entry {:?} {problem}", raw.display().to_string()),
        }
    }

    fn corrupt(&self, error: &io::Error) -> Error {
        Error::Archive {
            path: self.archive.to_path_buf(),
            message: format!("corrupt or unreadable archive: {error}"),
        }
    }
}

/// Check that a symlink at `link` (relative to the destination) pointing to
/// `target` stays inside the destination: `target` is relative, uses `..`
/// only at its start, and does not climb above the destination.
fn check_symlink(link: &Path, target: &Path) -> Result<(), String> {
    if target.as_os_str().is_empty() {
        return Err("is a symlink with an empty target".to_owned());
    }
    let mut climbs = 0usize;
    let mut descended = false;
    for component in target.components() {
        match component {
            Component::RootDir | Component::Prefix(_) => {
                return Err(format!(
                    "is a symlink to the absolute path {}",
                    target.display()
                ));
            }
            Component::CurDir => {}
            Component::ParentDir if descended => {
                return Err(format!(
                    "is a symlink to {}, which uses '..' after a directory name",
                    target.display()
                ));
            }
            Component::ParentDir => climbs += 1,
            Component::Normal(_) => descended = true,
        }
    }
    let depth = link.components().count().saturating_sub(1);
    if climbs > depth {
        return Err(format!(
            "is a symlink to {}, which is outside the destination",
            target.display()
        ));
    }
    Ok(())
}

/// Which side of a [`copy`] failed.
enum CopyError {
    Read(io::Error),
    Write(io::Error),
}

/// Copy `reader` to `writer` through `buffer`.
fn copy(
    buffer: &mut [u8],
    reader: &mut impl Read,
    writer: &mut impl Write,
) -> Result<(), CopyError> {
    loop {
        let read = match reader.read(buffer) {
            Ok(0) => return Ok(()),
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(CopyError::Read(e)),
        };
        writer
            .write_all(&buffer[..read])
            .map_err(CopyError::Write)?;
    }
}

/// The permission bits a regular file is unpacked with.
fn file_mode(header_mode: u32) -> u32 {
    let mask = if header_mode & 0o111 == 0 {
        0o644
    } else {
        0o755
    };
    (header_mode & mask) | 0o600
}

fn create_dir(path: &Path) -> crate::Result<()> {
    fs::create_dir(path).map_err(|e| Error::io("cannot create directory", path, e))?;
    set_mode(path, DIR_MODE)
}

fn set_mode(path: &Path, mode: u32) -> crate::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| Error::io("cannot set permissions of", path, e))
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use std::ffi::OsStr;

    use super::*;

    fn lossy(name: &OsStr) -> String {
        name.to_string_lossy().into_owned()
    }

    const EMPTY_SHA: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    const ABC_SHA: &str = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";

    #[derive(Debug, Default)]
    struct Recorder {
        events: Vec<String>,
    }

    impl Progress for Recorder {
        fn start(&mut self, label: &str, total: Option<u64>) {
            self.events.push(format!("start {label} {total:?}"));
        }
        fn advance(&mut self, bytes: u64) {
            self.events.push(format!("advance {bytes}"));
        }
        fn finish(&mut self) {
            self.events.push("finish".to_owned());
        }
    }

    /// Yields `data`, then fails.
    struct Broken {
        data: io::Cursor<Vec<u8>>,
    }

    impl Read for Broken {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.data.read(buf)? {
                0 => Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "connection reset",
                )),
                n => Ok(n),
            }
        }
    }

    /// A [`Progress`] sharing its log with the test.
    #[derive(Clone, Default)]
    struct Shared(Rc<RefCell<Vec<u64>>>);

    impl Progress for Shared {
        fn start(&mut self, _label: &str, _total: Option<u64>) {}
        fn advance(&mut self, bytes: u64) {
            self.0.borrow_mut().push(bytes);
        }
        fn finish(&mut self) {}
    }

    #[test]
    fn user_agent_names_uncork_and_its_version() {
        assert!(USER_AGENT.starts_with(&format!("uncork/{} ", env!("CARGO_PKG_VERSION"))));
    }

    #[test]
    fn sha256_file_matches_known_vectors() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty");
        let abc = dir.path().join("abc");
        fs::write(&empty, b"").unwrap();
        fs::write(&abc, b"abc").unwrap();
        assert_eq!(sha256_file(&empty).unwrap(), EMPTY_SHA);
        assert_eq!(sha256_file(&abc).unwrap(), ABC_SHA);
    }

    #[test]
    fn sha256_file_streams_files_larger_than_the_buffer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big");
        let data: Vec<u8> = (0..BUFFER_SIZE * 3 + 17).map(|i| (i % 251) as u8).collect();
        fs::write(&path, &data).unwrap();
        assert_eq!(
            sha256_file(&path).unwrap(),
            hex::encode(Sha256::digest(&data))
        );
    }

    #[test]
    fn sha256_file_reports_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("missing");
        match sha256_file(&missing) {
            Err(Error::Io { path, .. }) => assert_eq!(path, missing),
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }

    #[test]
    fn file_name_from_url_takes_the_last_segment() {
        assert_eq!(
            file_name_from_url("https://h/a/b/file.tar.gz"),
            "file.tar.gz"
        );
        assert_eq!(
            file_name_from_url("https://h/a/file.tar.gz?x=1#frag"),
            "file.tar.gz"
        );
        assert_eq!(file_name_from_url("https://h/a/"), "");
        assert_eq!(file_name_from_url("nothing"), "nothing");
    }

    #[test]
    fn download_refuses_plain_http_without_touching_the_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("sub").join("file");
        let error = download("http://example.com/file", &dest, None, &mut NoProgress).unwrap_err();
        assert!(matches!(error, Error::Download { ref message, .. } if message.contains("https")));
        assert!(!dir.path().join("sub").exists());
    }

    #[test]
    fn download_reuses_a_verified_file_without_network_access() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("abc");
        fs::write(&dest, b"abc").unwrap();
        let mut progress = Recorder::default();
        // `.invalid` never resolves: reaching the network would fail the test.
        download(
            "https://uncork.invalid/abc",
            &dest,
            Some(ABC_SHA),
            &mut progress,
        )
        .unwrap();
        download(
            "https://uncork.invalid/abc",
            &dest,
            Some(&ABC_SHA.to_uppercase()),
            &mut progress,
        )
        .unwrap();
        assert!(progress.events.is_empty());
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
    }

    #[test]
    fn save_verified_writes_the_file_and_reports_progress() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("abc");
        let mut progress = Recorder::default();
        save_verified(
            "https://h/x/abc.bin",
            &b"abc"[..],
            Some(3),
            &dest,
            Some(ABC_SHA),
            &mut progress,
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"abc");
        assert!(!part_path(&dest).exists());
        assert_eq!(
            progress.events,
            ["start abc.bin Some(3)", "advance 3", "finish"]
        );
    }

    #[test]
    fn save_verified_accepts_anything_without_an_expected_hash() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("setup.exe");
        save_verified(
            "https://h/setup.exe",
            &b"MZ"[..],
            None,
            &dest,
            None,
            &mut NoProgress,
        )
        .unwrap();
        assert_eq!(fs::read(&dest).unwrap(), b"MZ");
    }

    #[test]
    fn save_verified_counts_every_byte() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("big");
        let data = vec![7u8; BUFFER_SIZE * 2 + 5];
        let progress = Shared::default();
        let mut sink = progress.clone();
        save_verified(
            "https://h/big",
            data.as_slice(),
            None,
            &dest,
            None,
            &mut sink,
        )
        .unwrap();
        assert_eq!(progress.0.borrow().iter().sum::<u64>(), data.len() as u64);
    }

    #[test]
    fn save_verified_rejects_a_checksum_mismatch_and_removes_the_part_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("abc");
        let mut progress = Recorder::default();
        let error = save_verified(
            "https://h/abc",
            &b"abd"[..],
            None,
            &dest,
            Some(ABC_SHA),
            &mut progress,
        )
        .unwrap_err();
        match error {
            Error::Checksum {
                url,
                expected,
                actual,
            } => {
                assert_eq!(url, "https://h/abc");
                assert_eq!(expected, ABC_SHA);
                assert_eq!(actual, hex::encode(Sha256::digest(b"abd")));
            }
            other => panic!("expected a checksum error, got {other:?}"),
        }
        assert!(!dest.exists());
        assert!(!part_path(&dest).exists());
        assert_eq!(progress.events.last().map(String::as_str), Some("finish"));
    }

    #[test]
    fn save_verified_reports_transfer_errors_and_keeps_an_older_file() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("file");
        fs::write(&dest, b"old").unwrap();
        let body = Broken {
            data: io::Cursor::new(b"partial".to_vec()),
        };
        let error =
            save_verified("https://h/file", body, None, &dest, None, &mut NoProgress).unwrap_err();
        assert!(matches!(error, Error::Download { ref message, .. } if message.contains("reset")));
        assert!(!part_path(&dest).exists());
        assert_eq!(fs::read(&dest).unwrap(), b"old");
    }

    #[test]
    fn save_verified_reports_unwritable_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("missing-dir").join("file");
        let error = save_verified(
            "https://h/file",
            &b"x"[..],
            None,
            &dest,
            None,
            &mut NoProgress,
        )
        .unwrap_err();
        assert!(matches!(error, Error::Io { .. }), "{error:?}");
    }

    #[test]
    fn part_path_appends_a_suffix() {
        assert_eq!(
            part_path(Path::new("/a/b.tar.gz")),
            Path::new("/a/b.tar.gz.part")
        );
    }

    #[test]
    fn file_mode_masks_and_keeps_owner_access() {
        assert_eq!(file_mode(0o755), 0o755);
        assert_eq!(file_mode(0o777), 0o755);
        assert_eq!(file_mode(0o4755), 0o755);
        assert_eq!(file_mode(0o700), 0o700);
        assert_eq!(file_mode(0o644), 0o644);
        assert_eq!(file_mode(0o666), 0o644);
        assert_eq!(file_mode(0o600), 0o600);
        assert_eq!(file_mode(0o444), 0o644);
        assert_eq!(file_mode(0o000), 0o600);
        assert_eq!(file_mode(0o111), 0o711);
    }

    #[test]
    fn symlinks_may_climb_only_as_far_as_their_depth() {
        let ok = [
            ("a", "b"),
            ("a", "./b"),
            ("a", "."),
            ("d/a", ".."),
            ("d/a", "../b"),
            (
                "lib/wine/x86_64-unix/d3d11.so",
                "../../../external/libd3dshared.dylib",
            ),
            ("F.framework/Versions/Current", "A"),
            ("F.framework/F", "Versions/Current/F"),
        ];
        for (link, target) in ok {
            assert!(
                check_symlink(Path::new(link), Path::new(target)).is_ok(),
                "{link} -> {target}"
            );
        }
        let bad = [
            ("a", ".."),
            ("a", "../a"),
            ("d/a", "../.."),
            ("a", "/etc/passwd"),
            ("a", ""),
            ("d/a", "b/../../.."),
            ("d/a", "b/../c"),
        ];
        for (link, target) in bad {
            assert!(
                check_symlink(Path::new(link), Path::new(target)).is_err(),
                "{link} -> {target}"
            );
        }
    }

    #[test]
    fn top_level_tracks_a_single_directory() {
        let observe = |names: &[(&str, bool)]| {
            let mut top = TopLevel::Empty;
            for (name, is_dir) in names {
                top.observe(Path::new(name), *is_dir);
            }
            top
        };
        assert!(
            matches!(observe(&[("a", true), ("a/b", false)]), TopLevel::Directory(ref n) if lossy(n) == "a")
        );
        assert!(matches!(
            observe(&[("a/b", false), ("a/c/d", false)]),
            TopLevel::Directory(_)
        ));
        assert!(matches!(
            observe(&[("a/b", false), ("c/d", false)]),
            TopLevel::Mixed
        ));
        assert!(matches!(
            observe(&[("a/b", false), ("README", false)]),
            TopLevel::Mixed
        ));
        assert!(matches!(observe(&[("README", false)]), TopLevel::Mixed));
        assert!(matches!(
            observe(&[("a/b", false), ("a", false)]),
            TopLevel::Mixed
        ));
        assert!(matches!(observe(&[]), TopLevel::Empty));
    }

    #[test]
    fn hoist_directory_moves_contents_up_and_cleans_up() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("a/b/bin")).unwrap();
        fs::write(root.join("a/b/bin/wine"), b"").unwrap();
        // A child sharing a name with an ancestor of the nested directory.
        fs::create_dir_all(root.join("a/b/a")).unwrap();
        fs::write(root.join("keep"), b"").unwrap();
        hoist_directory(root, &root.join("a/b")).unwrap();
        assert!(root.join("bin/wine").is_file());
        assert!(root.join("a").is_dir());
        assert!(!root.join("a/b").exists());
        assert!(root.join("keep").is_file());
        let mut names: Vec<String> = child_names(root)
            .unwrap()
            .iter()
            .map(|n| lossy(n))
            .collect();
        names.sort();
        assert_eq!(names, ["a", "bin", "keep"]);
    }

    #[test]
    fn hoist_directory_keeps_non_empty_ancestors() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Libraries/Wine/bin")).unwrap();
        fs::create_dir_all(root.join("Libraries/DXVK")).unwrap();
        hoist_directory(root, &root.join("Libraries/Wine")).unwrap();
        assert!(root.join("bin").is_dir());
        assert!(root.join("Libraries/DXVK").is_dir());
        assert!(!root.join("Libraries/Wine").exists());
    }

    #[test]
    fn hoist_directory_refuses_to_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("nested/lib")).unwrap();
        fs::create_dir_all(root.join("lib")).unwrap();
        let error = hoist_directory(root, &root.join("nested")).unwrap_err();
        assert!(
            matches!(error, Error::Io { ref path, .. } if path == &root.join("lib")),
            "{error:?}"
        );
    }

    #[test]
    fn find_escaping_symlink_measures_from_the_given_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("tree/lib/deep")).unwrap();
        symlink("../x", root.join("tree/lib/ok")).unwrap();
        symlink("../../lib", root.join("tree/lib/deep/ok")).unwrap();
        assert_eq!(find_escaping_symlink(&root.join("tree")).unwrap(), None);
        // Fine relative to `tree`, outside once `tree/lib` is the root.
        let (link, problem) = find_escaping_symlink(&root.join("tree/lib"))
            .unwrap()
            .unwrap();
        assert_eq!(link, Path::new("ok"));
        assert!(problem.contains("outside the destination"), "{problem}");
    }

    #[test]
    fn find_escaping_symlink_flags_absolute_targets() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        symlink("/usr/lib", dir.path().join("a/b/abs")).unwrap();
        let (link, _) = find_escaping_symlink(dir.path()).unwrap().unwrap();
        assert_eq!(link, Path::new("a/b/abs"));
    }

    #[test]
    fn copy_tells_read_and_write_failures_apart() {
        let mut buffer = [0; 4];
        let mut sink = Vec::new();
        let mut broken = Broken {
            data: io::Cursor::new(b"0123456789".to_vec()),
        };
        assert!(matches!(
            copy(&mut buffer, &mut broken, &mut sink),
            Err(CopyError::Read(_))
        ));
        assert_eq!(sink, b"0123456789");

        let mut full = [0u8; 2];
        let mut writer = &mut full[..];
        let result = copy(&mut buffer, &mut &b"abc"[..], &mut writer);
        assert!(matches!(result, Err(CopyError::Write(_))));
    }
}
