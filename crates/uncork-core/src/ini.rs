//! Minimal edits to Windows INI files that games keep their settings in.
//!
//! Profiles can set keys (for example Rise of Nations' `SkipIntroMovies=1`
//! in `%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini`). Edits preserve
//! everything else byte for byte: line order, comments, unknown keys, the
//! file's line endings (CRLF or LF) and its encoding (UTF-8/ASCII, or UTF-16LE
//! with BOM, which is written back as UTF-16LE with BOM).

use std::path::Path;

/// One `key=value` to enforce in `[section]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IniSet {
    /// Section name without brackets, matched case-insensitively.
    pub section: String,
    /// Key, matched case-insensitively and with surrounding whitespace trimmed.
    pub key: String,
    /// Value to write.
    pub value: String,
}

/// Apply `sets` to INI text. Existing keys are rewritten in place (keeping
/// the original key spelling); missing keys are appended at the end of
/// their section; missing sections are appended at the end of the text.
/// Returns the new text and whether anything changed.
#[must_use]
pub fn apply(text: &str, sets: &[IniSet]) -> (String, bool) {
    let _ = (text, sets);
    todo!()
}

/// Apply `sets` to the file at `path` if it exists (games create their INI
/// on first run; a missing file is not an error and returns `Ok(false)`).
/// Writes atomically only when something changed.
///
/// # Errors
/// [`crate::Error::Io`].
pub fn apply_to_file(path: &Path, sets: &[IniSet]) -> crate::Result<bool> {
    let _ = (path, sets);
    todo!()
}
