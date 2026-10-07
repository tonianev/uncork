//! Map Windows paths written by Steam (`C:\Program Files (x86)\Steam`) to
//! macOS paths inside a Wine prefix.
//!
//! A Wine prefix stores drive `C:` at `<prefix>/drive_c` and every drive
//! letter as a symlink `<prefix>/dosdevices/<letter>:` (lowercase letter).
//! `C:` resolves to `<prefix>/drive_c` without touching the file system;
//! other letters resolve through the `dosdevices` symlink target.

use std::path::{Path, PathBuf};

/// Convert a Windows absolute path to a path under `prefix`.
///
/// - Accepts `\` or `/` separators, any case drive letter, and Steam's escaped
///   form with doubled backslashes already unescaped by the VDF parser.
/// - `C:` maps to `prefix/drive_c`; other letters to `prefix/dosdevices/x:`
///   (the symlink itself, unresolved, so the result is valid even if the
///   target does not exist yet).
/// - Empty components and `.` are dropped. `..` returns `None` (Steam never
///   writes it and it could escape the prefix).
/// - Returns `None` for relative paths, UNC paths (`\\server\share`) and
///   anything without a drive letter.
#[must_use]
pub fn windows_to_unix(prefix: &Path, windows_path: &str) -> Option<PathBuf> {
    let _ = (prefix, windows_path);
    todo!()
}

/// The inverse for paths under `prefix/drive_c`: `prefix/drive_c/a/b` →
/// `C:\a\b`. Returns `None` for paths outside `drive_c`.
#[must_use]
pub fn unix_to_windows(prefix: &Path, unix_path: &Path) -> Option<String> {
    let _ = (prefix, unix_path);
    todo!()
}
