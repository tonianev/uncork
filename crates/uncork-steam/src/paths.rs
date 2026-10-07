//! Map Windows paths written by Steam (`C:\Program Files (x86)\Steam`) to
//! macOS paths inside a Wine prefix.
//!
//! A Wine prefix stores drive `C:` at `<prefix>/drive_c` and every drive
//! letter as a symlink `<prefix>/dosdevices/<letter>:` (lowercase letter).
//! `C:` resolves to `<prefix>/drive_c` without touching the file system;
//! other letters resolve through the `dosdevices` symlink target.

use std::path::{Component, Path, PathBuf};

/// Directory of drive `C:` inside a prefix.
pub(crate) const DRIVE_C: &str = "drive_c";

/// Directory of the per-letter drive symlinks inside a prefix.
const DOSDEVICES: &str = "dosdevices";

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
///   anything without a drive letter. A drive letter must be followed by a
///   separator: `C:foo` is relative to the drive's current directory.
///
/// # Examples
/// ```
/// use std::path::Path;
/// use uncork_steam::paths::windows_to_unix;
///
/// let prefix = Path::new("/bottles/steam/prefix");
/// assert_eq!(
///     windows_to_unix(prefix, r"D:\SteamLibrary"),
///     Some(prefix.join("dosdevices/d:/SteamLibrary")),
/// );
/// assert_eq!(windows_to_unix(prefix, r"\\server\share"), None);
/// ```
#[must_use]
pub fn windows_to_unix(prefix: &Path, windows_path: &str) -> Option<PathBuf> {
    let (letter, rest) = split_drive(windows_path)?;
    let mut path = drive_root(prefix, letter);
    for component in rest.split(['\\', '/']) {
        match component {
            "" | "." => {}
            ".." => return None,
            name => path.push(name),
        }
    }
    Some(path)
}

/// The inverse for paths under `prefix/drive_c`: `prefix/drive_c/a/b` →
/// `C:\a\b`. Returns `None` for paths outside `drive_c`, and for paths that
/// have no faithful Windows spelling (`..`, non-UTF-8 names, or names that
/// contain a `\`).
#[must_use]
pub fn unix_to_windows(prefix: &Path, unix_path: &Path) -> Option<String> {
    let relative = unix_path.strip_prefix(prefix.join(DRIVE_C)).ok()?;
    let names = relative
        .components()
        .map(|component| match component {
            Component::Normal(name) => name.to_str().filter(|name| !name.contains('\\')),
            _ => None,
        })
        .collect::<Option<Vec<&str>>>()?;
    Some(format!(r"C:\{}", names.join(r"\")))
}

/// Split `X:\rest` into the lowercase drive letter and `\rest`.
fn split_drive(path: &str) -> Option<(char, &str)> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    let rest = chars.as_str();
    rest.starts_with(['\\', '/'])
        .then_some((letter.to_ascii_lowercase(), rest))
}

fn drive_root(prefix: &Path, letter: char) -> PathBuf {
    if letter == 'c' {
        prefix.join(DRIVE_C)
    } else {
        prefix.join(DOSDEVICES).join(format!("{letter}:"))
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    const PREFIX: &str = "/bottles/steam/prefix";

    fn to_unix(windows: &str) -> Option<PathBuf> {
        windows_to_unix(Path::new(PREFIX), windows)
    }

    fn under_prefix(relative: &str) -> Option<PathBuf> {
        Some(Path::new(PREFIX).join(relative))
    }

    #[test]
    fn drive_c_maps_to_drive_c_without_dosdevices() {
        assert_eq!(
            to_unix(r"C:\Program Files (x86)\Steam"),
            under_prefix("drive_c/Program Files (x86)/Steam")
        );
    }

    #[test]
    fn drive_letter_case_does_not_matter() {
        assert_eq!(to_unix(r"c:\Games"), under_prefix("drive_c/Games"));
        assert_eq!(to_unix(r"D:\Games"), under_prefix("dosdevices/d:/Games"));
        assert_eq!(to_unix(r"d:\Games"), under_prefix("dosdevices/d:/Games"));
    }

    #[test]
    fn other_letters_map_to_the_unresolved_dosdevices_symlink() {
        assert_eq!(
            to_unix(r"Z:\Users\me\SteamLibrary"),
            under_prefix("dosdevices/z:/Users/me/SteamLibrary")
        );
    }

    #[test]
    fn forward_slashes_and_mixed_separators_are_accepted() {
        assert_eq!(
            to_unix("c:/program files (x86)/steam"),
            under_prefix("drive_c/program files (x86)/steam")
        );
        assert_eq!(to_unix(r"D:/a\b/c"), under_prefix("dosdevices/d:/a/b/c"));
    }

    #[test]
    fn doubled_separators_and_dots_are_dropped() {
        assert_eq!(
            to_unix(r"C:\\Program Files (x86)\\Steam\\"),
            under_prefix("drive_c/Program Files (x86)/Steam")
        );
        assert_eq!(to_unix(r"C:\.\a\.\b"), under_prefix("drive_c/a/b"));
    }

    #[test]
    fn drive_root_maps_to_the_drive_directory() {
        assert_eq!(to_unix(r"C:\"), under_prefix("drive_c"));
        assert_eq!(to_unix("E:/"), under_prefix("dosdevices/e:"));
    }

    #[test]
    fn parent_components_are_rejected() {
        assert_eq!(to_unix(r"C:\..\..\etc"), None);
        assert_eq!(to_unix(r"D:\Games\..\Other"), None);
    }

    #[test]
    fn paths_without_an_absolute_drive_are_rejected() {
        for path in [
            "",
            "Games",
            r"Games\Steam",
            r"\Games",
            "/Users/me",
            "C:",
            "C:Games",
            r"\\server\share\Steam",
            "//server/share",
            r"\\?\C:\Steam",
            r"1:\Games",
            r"é:\Games",
            r"CD:\Games",
        ] {
            assert_eq!(to_unix(path), None, "input {path:?}");
        }
    }

    #[test]
    fn names_with_spaces_and_unicode_are_kept() {
        assert_eq!(
            to_unix(r"C:\Spiele\Rise of Nations — Édition"),
            under_prefix("drive_c/Spiele/Rise of Nations — Édition")
        );
    }

    fn to_windows(relative: &str) -> Option<String> {
        unix_to_windows(Path::new(PREFIX), &Path::new(PREFIX).join(relative))
    }

    #[test]
    fn unix_to_windows_maps_drive_c_paths() {
        assert_eq!(
            to_windows("drive_c/Program Files (x86)/Steam/steam.exe"),
            Some(r"C:\Program Files (x86)\Steam\steam.exe".to_owned())
        );
        assert_eq!(to_windows("drive_c"), Some(r"C:\".to_owned()));
        assert_eq!(to_windows("drive_c/"), Some(r"C:\".to_owned()));
        assert_eq!(to_windows("drive_c/./a"), Some(r"C:\a".to_owned()));
    }

    #[test]
    fn unix_to_windows_rejects_paths_outside_drive_c() {
        assert_eq!(to_windows("dosdevices/d:/Games"), None);
        assert_eq!(to_windows("drive_cx/Games"), None);
        assert_eq!(to_windows(""), None);
        assert_eq!(
            unix_to_windows(Path::new(PREFIX), Path::new("/elsewhere/drive_c/a")),
            None
        );
    }

    #[test]
    fn unix_to_windows_rejects_names_without_a_windows_spelling() {
        assert_eq!(to_windows("drive_c/a/../b"), None);
        assert_eq!(to_windows(r"drive_c/back\slash"), None);
    }

    #[cfg(unix)]
    #[test]
    fn unix_to_windows_rejects_non_utf8_names() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let name = OsStr::from_bytes(b"caf\xe9");
        let path = Path::new(PREFIX).join(DRIVE_C).join(name);
        assert_eq!(unix_to_windows(Path::new(PREFIX), &path), None);
    }

    fn component() -> impl Strategy<Value = String> {
        "[^\\\\/\u{0}]{1,12}".prop_filter("not a dot component", |name| name != "." && name != "..")
    }

    proptest! {
        #[test]
        fn drive_c_paths_round_trip(components in prop::collection::vec(component(), 0..6)) {
            let windows = format!(r"C:\{}", components.join(r"\"));
            let unix = to_unix(&windows).expect("absolute C: path");
            prop_assert_eq!(unix_to_windows(Path::new(PREFIX), &unix), Some(windows));
        }

        #[test]
        fn mapped_paths_stay_inside_the_prefix(
            letter in prop::char::range('a', 'z'),
            rest in "[a-zA-Z0-9 ./\\\\]{0,24}",
        ) {
            if let Some(path) = to_unix(&format!("{letter}:\\{rest}")) {
                prop_assert!(path.starts_with(PREFIX));
                prop_assert!(path.components().all(|c| c != Component::ParentDir));
            }
        }
    }
}
