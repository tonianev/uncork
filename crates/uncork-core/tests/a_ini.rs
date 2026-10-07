//! `ini::apply_to_file` on real files: encodings, atomic replacement,
//! permissions, symlinks and error paths.

use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use uncork_core::Error;
use uncork_core::ini::{self, IniSet};

fn skip_intro() -> Vec<IniSet> {
    vec![IniSet {
        section: "RISE OF NATIONS".to_owned(),
        key: "SkipIntroMovies".to_owned(),
        value: "1".to_owned(),
    }]
}

fn utf16le(text: &str) -> Vec<u8> {
    [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect()
}

fn utf16be(text: &str) -> Vec<u8> {
    [0xFE, 0xFF]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
        .collect()
}

/// Write `before`, apply the Rise of Nations set, and return the new bytes.
fn edit(dir: &Path, before: &[u8]) -> (bool, Vec<u8>) {
    let path = dir.join("rise2.ini");
    fs::write(&path, before).unwrap();
    let changed = ini::apply_to_file(&path, &skip_intro()).unwrap();
    (changed, fs::read(&path).unwrap())
}

fn invalid_data(error: &Error) -> bool {
    matches!(error, Error::Io { source, .. } if source.kind() == std::io::ErrorKind::InvalidData)
}

const BEFORE: &str = "[RISE OF NATIONS]\r\nFullscreen=1\r\nSkipIntroMovies=0\r\n";
const AFTER: &str = "[RISE OF NATIONS]\r\nFullscreen=1\r\nSkipIntroMovies=1\r\n";

#[test]
fn missing_file_is_not_an_error_and_is_not_created() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rise2.ini");
    assert!(!ini::apply_to_file(&path, &skip_intro()).unwrap());
    assert!(!path.exists());
}

#[test]
fn utf8_file_is_edited_in_place() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(
        edit(temp.path(), BEFORE.as_bytes()),
        (true, AFTER.as_bytes().to_vec())
    );
}

#[test]
fn utf8_bom_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    let before = [b"\xEF\xBB\xBF".as_slice(), BEFORE.as_bytes()].concat();
    let after = [b"\xEF\xBB\xBF".as_slice(), AFTER.as_bytes()].concat();
    assert_eq!(edit(temp.path(), &before), (true, after));
}

#[test]
fn utf16le_with_bom_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(edit(temp.path(), &utf16le(BEFORE)), (true, utf16le(AFTER)));
}

#[test]
fn utf16be_with_bom_round_trips() {
    let temp = tempfile::tempdir().unwrap();
    assert_eq!(edit(temp.path(), &utf16be(BEFORE)), (true, utf16be(AFTER)));
}

#[test]
fn utf16_new_section_is_written_as_utf16() {
    let temp = tempfile::tempdir().unwrap();
    let (changed, bytes) = edit(temp.path(), &utf16le("[Other]\r\nx=1\r\n"));
    assert!(changed);
    assert_eq!(
        bytes,
        utf16le("[Other]\r\nx=1\r\n\r\n[RISE OF NATIONS]\r\nSkipIntroMovies=1\r\n")
    );
}

#[test]
fn ansi_bytes_elsewhere_are_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let before = b"; Caf\xE9 \x93quoted\x94\r\n[RISE OF NATIONS]\r\nSkipIntroMovies=0\r\n";
    let after = b"; Caf\xE9 \x93quoted\x94\r\n[RISE OF NATIONS]\r\nSkipIntroMovies=1\r\n";
    assert_eq!(edit(temp.path(), before), (true, after.to_vec()));
}

#[test]
fn ansi_file_rejects_a_value_it_cannot_hold() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("game.ini");
    let before = b"[A]\r\nname=Caf\xE9\r\n";
    fs::write(&path, before).unwrap();
    let sets = [IniSet {
        section: "A".to_owned(),
        key: "name".to_owned(),
        value: "€uro".to_owned(),
    }];
    let error = ini::apply_to_file(&path, &sets).unwrap_err();
    assert!(invalid_data(&error), "{error:?}");
    assert_eq!(fs::read(&path).unwrap(), before, "file must be left alone");
}

#[test]
fn unchanged_file_is_not_rewritten() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rise2.ini");
    fs::write(&path, AFTER).unwrap();
    let inode = fs::metadata(&path).unwrap().ino();
    assert!(!ini::apply_to_file(&path, &skip_intro()).unwrap());
    assert_eq!(fs::metadata(&path).unwrap().ino(), inode);
    assert_eq!(fs::read_to_string(&path).unwrap(), AFTER);
}

#[test]
fn changed_file_is_replaced_atomically_without_leftovers() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rise2.ini");
    fs::write(&path, BEFORE).unwrap();
    assert!(ini::apply_to_file(&path, &skip_intro()).unwrap());
    let names: Vec<String> = fs::read_dir(temp.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["rise2.ini"]);
}

#[test]
fn permissions_are_kept() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("rise2.ini");
    fs::write(&path, BEFORE).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    assert!(ini::apply_to_file(&path, &skip_intro()).unwrap());
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn symlinks_are_written_through() {
    let temp = tempfile::tempdir().unwrap();
    let real_dir = temp.path().join("real");
    fs::create_dir(&real_dir).unwrap();
    let real = real_dir.join("rise2.ini");
    fs::write(&real, BEFORE).unwrap();
    let link = temp.path().join("link.ini");
    std::os::unix::fs::symlink(&real, &link).unwrap();

    assert!(ini::apply_to_file(&link, &skip_intro()).unwrap());
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&real).unwrap(), AFTER);
}

#[test]
fn undecodable_files_are_refused_untouched() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("odd.ini");
    let bom_less_utf16: Vec<u8> = "[A]\r\n"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    for bytes in [
        bom_less_utf16,
        vec![0xFF, 0xFE, b'['],
        vec![0xEF, 0xBB, 0xBF, 0xC3],
    ] {
        fs::write(&path, &bytes).unwrap();
        let error = ini::apply_to_file(&path, &skip_intro()).unwrap_err();
        assert!(invalid_data(&error), "{error:?}");
        assert!(error.to_string().contains("odd.ini"), "{error}");
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn a_directory_is_an_io_error() {
    let temp = tempfile::tempdir().unwrap();
    let error = ini::apply_to_file(temp.path(), &skip_intro()).unwrap_err();
    assert!(
        matches!(&error, Error::Io { path, .. } if path == temp.path()),
        "{error:?}"
    );
}
