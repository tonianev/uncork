//! `download::extract`: unpacking, safety rules and automatic top-level
//! stripping, on archives built in the test.

mod b_support;

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use b_support::{TarBuilder, tree};
use proptest::prelude::*;
use tar::EntryType;
use uncork_core::Error;
use uncork_core::catalog::ArchiveFormat;
use uncork_core::download::extract;

struct Scratch {
    dir: tempfile::TempDir,
}

impl Scratch {
    fn new() -> Scratch {
        Scratch {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn archive(&self) -> PathBuf {
        self.dir.path().join("archive.tar.gz")
    }

    /// The extraction target, nested so escapes would land in `outside()`.
    fn dest(&self) -> PathBuf {
        self.dir.path().join("out").join("dest")
    }

    fn outside(&self) -> PathBuf {
        self.dir.path().join("out")
    }

    fn extract(&self, builder: TarBuilder, strip_prefix: Option<&str>) -> uncork_core::Result<()> {
        builder.write_tar_gz(&self.archive());
        extract(
            &self.archive(),
            ArchiveFormat::TarGz,
            &self.dest(),
            strip_prefix,
        )
    }

    fn dest_tree(&self) -> Vec<String> {
        tree(&self.dest())
    }

    /// Assert extraction failed safely: an archive error mentioning
    /// `fragment`, no destination left behind, nothing written next to it.
    fn assert_rejected(&self, result: uncork_core::Result<()>, fragment: &str) {
        match result {
            Err(Error::Archive { path, message }) => {
                assert_eq!(path, self.archive());
                assert!(message.contains(fragment), "{message:?} lacks {fragment:?}");
            }
            other => panic!("expected an archive error, got {other:?}"),
        }
        assert!(!self.dest().exists(), "destination left behind");
        assert!(
            tree(&self.outside()).is_empty(),
            "wrote outside: {:?}",
            tree(&self.outside())
        );
        assert!(!self.dir.path().join("escape").exists());
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

#[test]
fn unpacks_files_directories_and_symlinks() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .dir("bin")
        .file_mode("bin/wine", b"#!/bin/sh\n", 0o775)
        .dir("lib")
        .file("lib/libwine.1.0.dylib", b"lib")
        .symlink("lib/libwine.1.dylib", "libwine.1.0.dylib")
        .symlink("lib/up", "../bin/wine")
        .file("share/nested/deep/file.txt", b"implicit parents");
    scratch.extract(builder, Some("")).unwrap();

    assert_eq!(
        scratch.dest_tree(),
        [
            "bin/",
            "bin/wine",
            "lib/",
            "lib/libwine.1.0.dylib",
            "lib/libwine.1.dylib -> libwine.1.0.dylib",
            "lib/up -> ../bin/wine",
            "share/",
            "share/nested/",
            "share/nested/deep/",
            "share/nested/deep/file.txt",
        ]
    );
    let dest = scratch.dest();
    assert_eq!(fs::read(dest.join("bin/wine")).unwrap(), b"#!/bin/sh\n");
    assert_eq!(fs::read(dest.join("lib/libwine.1.dylib")).unwrap(), b"lib");
    assert_eq!(fs::read(dest.join("lib/up")).unwrap(), b"#!/bin/sh\n");
    assert_eq!(
        fs::read(dest.join("share/nested/deep/file.txt")).unwrap(),
        b"implicit parents"
    );
}

#[test]
fn unpacks_tar_xz() {
    let scratch = Scratch::new();
    let archive = scratch.dir.path().join("archive.tar.xz");
    TarBuilder::new()
        .file("a/one", b"1")
        .file("b/two", b"2")
        .write_tar_xz(&archive);
    extract(&archive, ArchiveFormat::TarXz, &scratch.dest(), None).unwrap();
    assert_eq!(scratch.dest_tree(), ["a/", "a/one", "b/", "b/two"]);
}

#[test]
fn rejects_a_format_mismatch() {
    let scratch = Scratch::new();
    TarBuilder::new()
        .file("a", b"1")
        .write_tar_gz(&scratch.archive());
    let result = extract(
        &scratch.archive(),
        ArchiveFormat::TarXz,
        &scratch.dest(),
        None,
    );
    scratch.assert_rejected(result, "corrupt");
}

#[test]
fn normalizes_permissions() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .dir_mode("private", 0o700)
        .file_mode("private/tool", b"", 0o777)
        .file_mode("private/owner-exec", b"", 0o700)
        .file_mode("private/world-writable", b"", 0o666)
        .file_mode("private/read-only", b"", 0o444)
        .file_mode("private/setuid", b"", 0o4755)
        .file_mode("private/secret", b"", 0o600);
    scratch.extract(builder, Some("")).unwrap();
    let dest = scratch.dest();
    assert_eq!(mode(&dest.join("private")), 0o755);
    assert_eq!(mode(&dest.join("private/tool")), 0o755);
    assert_eq!(mode(&dest.join("private/owner-exec")), 0o700);
    assert_eq!(mode(&dest.join("private/world-writable")), 0o644);
    assert_eq!(mode(&dest.join("private/read-only")), 0o644);
    assert_eq!(mode(&dest.join("private/setuid")), 0o755);
    assert_eq!(mode(&dest.join("private/secret")), 0o600);
}

#[test]
fn implicit_directories_are_world_readable() {
    let scratch = Scratch::new();
    scratch
        .extract(TarBuilder::new().file("x/y/z", b""), Some(""))
        .unwrap();
    assert_eq!(mode(&scratch.dest().join("x")), 0o755);
    assert_eq!(mode(&scratch.dest().join("x/y")), 0o755);
}

#[test]
fn handles_long_paths() {
    let scratch = Scratch::new();
    let long = format!("{}/{}.dll", "d".repeat(120), "f".repeat(120));
    scratch
        .extract(TarBuilder::new().file(&long, b"long"), Some(""))
        .unwrap();
    assert_eq!(fs::read(scratch.dest().join(&long)).unwrap(), b"long");
}

#[test]
fn later_entries_replace_earlier_files() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("a", b"old")
        .file("a", b"new")
        .symlink("b", "a")
        .file("b", b"file now");
    scratch.extract(builder, Some("")).unwrap();
    assert_eq!(fs::read(scratch.dest().join("a")).unwrap(), b"new");
    assert_eq!(scratch.dest_tree(), ["a", "b"]);
    assert_eq!(fs::read(scratch.dest().join("b")).unwrap(), b"file now");
}

#[test]
fn rejects_a_file_replacing_a_directory() {
    let scratch = Scratch::new();
    let result = scratch.extract(TarBuilder::new().file("a/b", b"").file("a", b""), Some(""));
    scratch.assert_rejected(result, "would replace a directory");
}

#[test]
fn rejects_a_directory_replacing_a_file() {
    let scratch = Scratch::new();
    let result = scratch.extract(TarBuilder::new().file("a", b"").dir("a"), Some(""));
    scratch.assert_rejected(result, "would replace a file");
}

#[test]
fn rejects_absolute_paths() {
    let scratch = Scratch::new();
    let escape = scratch.dir.path().join("escape");
    let builder = TarBuilder::new().raw(
        escape.to_str().unwrap().as_bytes(),
        EntryType::Regular,
        0o644,
        b"x",
        None,
    );
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "absolute path");
}

#[test]
fn rejects_parent_components() {
    for path in ["../escape", "a/../../escape", "a/../b", "./.."] {
        let scratch = Scratch::new();
        let builder = TarBuilder::new().file("ok", b"").raw(
            path.as_bytes(),
            EntryType::Regular,
            0o644,
            b"x",
            None,
        );
        let result = scratch.extract(builder, Some(""));
        scratch.assert_rejected(result, "'..'");
    }
}

#[test]
fn rejects_parent_components_in_directories_and_symlinks() {
    let scratch = Scratch::new();
    let result = scratch.extract(
        TarBuilder::new().raw(b"../escape/", EntryType::Directory, 0o755, b"", None),
        None,
    );
    scratch.assert_rejected(result, "'..'");

    let scratch = Scratch::new();
    let result = scratch.extract(
        TarBuilder::new().raw(b"../escape", EntryType::Symlink, 0o777, b"", Some("x")),
        None,
    );
    scratch.assert_rejected(result, "'..'");
}

#[test]
fn rejects_absolute_symlinks() {
    let scratch = Scratch::new();
    let result = scratch.extract(TarBuilder::new().symlink("etc", "/etc"), Some(""));
    scratch.assert_rejected(result, "absolute path /etc");
}

#[test]
fn rejects_symlinks_pointing_outside() {
    for (link, target) in [
        ("link", ".."),
        ("link", "../escape"),
        ("a/b/link", "../../../escape"),
    ] {
        let scratch = Scratch::new();
        let result = scratch.extract(TarBuilder::new().symlink(link, target), Some(""));
        scratch.assert_rejected(result, "outside the destination");
    }
}

#[test]
fn rejects_symlinks_with_parent_components_after_names() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .dir("a")
        .dir("a/b")
        .symlink("a/link", "b/../../..");
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "after a directory name");
}

#[test]
fn rejects_symlinks_without_targets() {
    let scratch = Scratch::new();
    let result = scratch.extract(
        TarBuilder::new().raw(b"link", EntryType::Symlink, 0o777, b"", None),
        Some(""),
    );
    scratch.assert_rejected(result, "symlink");
}

#[test]
fn never_writes_through_a_symlink() {
    // A symlink that stays inside, then an entry below it: the classic
    // two-step escape when the link is later swapped, and never legitimate.
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .dir("real")
        .symlink("lib", "real")
        .file("lib/x", b"x");
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "which is not a directory");
}

#[test]
fn never_writes_through_a_symlink_to_a_file() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("target", b"keep")
        .symlink("link", "target")
        .dir("link/sub");
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "which is not a directory");
}

#[test]
fn rejects_hard_links() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new().file("a", b"x").hard_link("b", "a");
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "hard link");
}

#[test]
fn rejects_hard_links_to_outside_files() {
    let scratch = Scratch::new();
    let result = scratch.extract(
        TarBuilder::new().hard_link("passwd", "/etc/passwd"),
        Some(""),
    );
    scratch.assert_rejected(result, "hard link");
}

#[test]
fn rejects_devices_and_fifos() {
    for (entry_type, fragment) in [
        (EntryType::Char, "device"),
        (EntryType::Block, "device"),
        (EntryType::Fifo, "FIFO"),
    ] {
        let scratch = Scratch::new();
        let result = scratch.extract(
            TarBuilder::new().raw(b"dev", entry_type, 0o644, b"", None),
            Some(""),
        );
        scratch.assert_rejected(result, fragment);
    }
}

#[test]
fn rejects_unknown_entry_types() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new().raw(b"volume", EntryType::new(b'V'), 0o644, b"", None);
    let result = scratch.extract(builder, Some(""));
    scratch.assert_rejected(result, "unsupported type 'V'");
}

#[test]
fn rejects_corrupt_archives() {
    let scratch = Scratch::new();
    fs::write(scratch.archive(), b"not a gzip stream at all").unwrap();
    let result = extract(
        &scratch.archive(),
        ArchiveFormat::TarGz,
        &scratch.dest(),
        None,
    );
    scratch.assert_rejected(result, "corrupt");
}

#[test]
fn rejects_truncated_archives() {
    let scratch = Scratch::new();
    let tar = TarBuilder::new().file("big", &[7; 4096]).into_tar();
    let truncated = &tar[..1024];
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    std::io::Write::write_all(&mut encoder, truncated).unwrap();
    fs::write(scratch.archive(), encoder.finish().unwrap()).unwrap();
    let result = extract(
        &scratch.archive(),
        ArchiveFormat::TarGz,
        &scratch.dest(),
        None,
    );
    scratch.assert_rejected(result, "corrupt");
}

#[test]
fn a_missing_archive_is_an_io_error() {
    let scratch = Scratch::new();
    let result = extract(
        &scratch.archive(),
        ArchiveFormat::TarGz,
        &scratch.dest(),
        None,
    );
    assert!(
        matches!(result, Err(Error::Io { ref path, .. }) if path == &scratch.archive()),
        "{result:?}"
    );
    assert!(!scratch.dest().exists());
}

#[test]
fn the_destination_must_not_exist() {
    let scratch = Scratch::new();
    fs::create_dir_all(scratch.dest()).unwrap();
    fs::write(scratch.dest().join("keep"), b"mine").unwrap();
    let result = scratch.extract(TarBuilder::new().file("a", b""), None);
    assert!(
        matches!(result, Err(Error::Io { ref path, .. }) if path == &scratch.dest()),
        "{result:?}"
    );
    assert_eq!(fs::read(scratch.dest().join("keep")).unwrap(), b"mine");
    assert_eq!(scratch.dest_tree(), ["keep"]);
}

#[test]
fn strips_an_explicit_prefix() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .dir("./dxmt-v0.70-builtin/")
        .file("./dxmt-v0.70-builtin/x86_64-windows/d3d11.dll", b"dll")
        .symlink("dxmt-v0.70-builtin/link", "x86_64-windows/d3d11.dll");
    scratch
        .extract(builder, Some("dxmt-v0.70-builtin"))
        .unwrap();
    assert_eq!(
        scratch.dest_tree(),
        [
            "link -> x86_64-windows/d3d11.dll",
            "x86_64-windows/",
            "x86_64-windows/d3d11.dll"
        ]
    );
}

#[test]
fn rejects_entries_outside_an_explicit_prefix() {
    for path in ["other/file", "README", "prefix-not/file"] {
        let scratch = Scratch::new();
        let builder = TarBuilder::new().file("prefix/ok", b"").file(path, b"");
        let result = scratch.extract(builder, Some("prefix"));
        scratch.assert_rejected(result, "outside the top-level directory \"prefix\"");
    }
}

#[test]
fn rejects_a_non_directory_in_place_of_the_prefix() {
    let scratch = Scratch::new();
    let result = scratch.extract(TarBuilder::new().file("prefix", b""), Some("prefix"));
    scratch.assert_rejected(result, "takes the place of the destination");
}

#[test]
fn strips_a_single_top_level_directory_automatically() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .pax_global_header()
        .dir("./")
        .dir("./v0.80/")
        .file("./v0.80/x86_64-unix/winemetal.so", b"so")
        .file("v0.80/x86_64-windows/d3d11.dll", b"dll")
        .symlink("v0.80/x86_64-windows/alias.dll", "d3d11.dll")
        .symlink("v0.80/x86_64-windows/up", "../x86_64-unix/winemetal.so");
    scratch.extract(builder, None).unwrap();
    assert_eq!(
        scratch.dest_tree(),
        [
            "x86_64-unix/",
            "x86_64-unix/winemetal.so",
            "x86_64-windows/",
            "x86_64-windows/alias.dll -> d3d11.dll",
            "x86_64-windows/d3d11.dll",
            "x86_64-windows/up -> ../x86_64-unix/winemetal.so",
        ]
    );
    assert_eq!(
        fs::read(scratch.dest().join("x86_64-windows/up")).unwrap(),
        b"so"
    );
}

#[test]
fn auto_strip_handles_a_child_named_like_the_top_level_directory() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("wine/wine/inner", b"")
        .file("wine/bin/wine", b"");
    scratch.extract(builder, None).unwrap();
    assert_eq!(
        scratch.dest_tree(),
        ["bin/", "bin/wine", "wine/", "wine/inner"]
    );
}

#[test]
fn keeps_several_top_level_entries() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("x86_64-windows/d3d11.dll", b"")
        .file("i386-windows/d3d11.dll", b"");
    scratch.extract(builder, None).unwrap();
    assert_eq!(
        scratch.dest_tree(),
        [
            "i386-windows/",
            "i386-windows/d3d11.dll",
            "x86_64-windows/",
            "x86_64-windows/d3d11.dll"
        ]
    );
}

#[test]
fn a_top_level_file_prevents_auto_strip() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("wswine.bundle/bin/wine", b"")
        .file("README", b"");
    scratch.extract(builder, None).unwrap();
    assert_eq!(
        scratch.dest_tree(),
        [
            "README",
            "wswine.bundle/",
            "wswine.bundle/bin/",
            "wswine.bundle/bin/wine"
        ]
    );
}

#[test]
fn a_top_level_symlink_prevents_auto_strip() {
    let scratch = Scratch::new();
    let builder = TarBuilder::new()
        .file("a/file", b"")
        .symlink("current", "a");
    scratch.extract(builder, None).unwrap();
    assert_eq!(scratch.dest_tree(), ["a/", "a/file", "current -> a"]);
}

#[test]
fn an_empty_prefix_disables_auto_strip() {
    let scratch = Scratch::new();
    scratch
        .extract(TarBuilder::new().file("x64/d3d11.dll", b""), Some(""))
        .unwrap();
    assert_eq!(scratch.dest_tree(), ["x64/", "x64/d3d11.dll"]);
}

#[test]
fn auto_strip_rechecks_symlinks_that_would_escape_after_stripping() {
    let scratch = Scratch::new();
    // Inside `dest` as unpacked, outside once `top/` is stripped.
    let builder = TarBuilder::new()
        .file("top/file", b"")
        .symlink("top/link", "../top/file");
    let result = scratch.extract(builder, None);
    scratch.assert_rejected(result, "outside the destination");
}

#[test]
fn an_empty_archive_unpacks_to_an_empty_directory() {
    let scratch = Scratch::new();
    scratch.extract(TarBuilder::new(), None).unwrap();
    assert!(scratch.dest().is_dir());
    assert!(scratch.dest_tree().is_empty());
}

fn safe_tree() -> impl Strategy<Value = BTreeMap<String, Vec<u8>>> {
    // Directories are `d*` and files `f*`, so no path is both.
    let path = (
        prop::collection::vec("d[a-c]{1,2}", 0..3),
        "f[a-z0-9._-]{1,8}",
    )
        .prop_map(|(dirs, file)| dirs.into_iter().chain([file]).collect::<Vec<_>>().join("/"));
    prop::collection::btree_map(path, prop::collection::vec(any::<u8>(), 0..64), 1..12)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    #[test]
    fn regular_files_round_trip(files in safe_tree(), xz in any::<bool>()) {
        let scratch = Scratch::new();
        let mut builder = TarBuilder::new();
        for (path, contents) in &files {
            builder = builder.file(path, contents);
        }
        let (archive, format) = if xz {
            let archive = scratch.dir.path().join("archive.tar.xz");
            builder.write_tar_xz(&archive);
            (archive, ArchiveFormat::TarXz)
        } else {
            builder.write_tar_gz(&scratch.archive());
            (scratch.archive(), ArchiveFormat::TarGz)
        };
        extract(&archive, format, &scratch.dest(), Some("")).unwrap();
        let unpacked: Vec<String> = scratch.dest_tree().into_iter().filter(|p| !p.ends_with('/')).collect();
        prop_assert_eq!(unpacked, files.keys().cloned().collect::<Vec<_>>());
        for (path, contents) in &files {
            prop_assert_eq!(&fs::read(scratch.dest().join(path)).unwrap(), contents);
        }
    }
}
