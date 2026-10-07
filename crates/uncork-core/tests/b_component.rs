//! Installing, listing, finding and removing components, offline: archives
//! are built in the test and installed with `install_from_archive` (or
//! `install_from_catalog` with the download already in place).

mod b_support;

use std::fs;
use std::path::{Path, PathBuf};

use b_support::{TarBuilder, tree};
use uncork_core::Error;
use uncork_core::catalog::{ArchiveFormat, CatalogEntry};
use uncork_core::component::{
    self, ComponentKind, ComponentMeta, InstalledComponent, META_FILE, Source,
};
use uncork_core::download::{NoProgress, sha256_file};
use uncork_core::paths::Layout;

struct Fixture {
    dir: tempfile::TempDir,
    layout: Layout,
}

impl Fixture {
    fn new() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path().join("home"));
        Fixture { dir, layout }
    }

    /// Write `builder` as an archive outside the data root and describe it
    /// as a catalog entry.
    fn archive(
        &self,
        kind: ComponentKind,
        version: &str,
        builder: TarBuilder,
    ) -> (PathBuf, CatalogEntry) {
        let name = format!("{kind}-{version}.tar.gz");
        let path = self.dir.path().join(&name);
        builder.write_tar_gz(&path);
        let entry = entry_for(kind, version, &path, ArchiveFormat::TarGz, &name);
        (path, entry)
    }

    fn install(
        &self,
        kind: ComponentKind,
        version: &str,
        builder: TarBuilder,
    ) -> InstalledComponent {
        let (path, entry) = self.archive(kind, version, builder);
        component::install_from_archive(&self.layout, &entry, &path).unwrap()
    }

    fn kind_dir_tree(&self, kind: ComponentKind) -> Vec<String> {
        let dir = self.layout.component_kind_dir(kind);
        if dir.exists() { tree(&dir) } else { Vec::new() }
    }

    fn installed(&self) -> Vec<(ComponentKind, String)> {
        component::list_installed(&self.layout)
            .unwrap()
            .into_iter()
            .map(|c| (c.meta.kind, c.meta.version))
            .collect()
    }
}

fn entry_for(
    kind: ComponentKind,
    version: &str,
    archive: &Path,
    format: ArchiveFormat,
    name: &str,
) -> CatalogEntry {
    CatalogEntry {
        kind,
        version: version.to_owned(),
        url: format!("https://uncork.invalid/releases/{name}"),
        sha256: sha256_file(archive).unwrap(),
        size: fs::metadata(archive).unwrap().len(),
        archive: format,
        strip_prefix: None,
        license: "MIT".to_owned(),
        source_code: "https://example.com/source".to_owned(),
        homepage: "https://example.com".to_owned(),
        features: vec!["wow64".to_owned(), "dxmt".to_owned()],
        recommended: false,
        notes: String::new(),
    }
}

fn dxmt(top: &str) -> TarBuilder {
    TarBuilder::new()
        .dir(top)
        .file(&format!("{top}/x86_64-windows/d3d11.dll"), b"d3d11")
        .file(&format!("{top}/x86_64-windows/dxgi.dll"), b"dxgi")
        .file(&format!("{top}/x86_64-windows/winemetal.dll"), b"winemetal")
        .file(&format!("{top}/x86_64-unix/winemetal.so"), b"so")
        .file(&format!("{top}/i386-windows/d3d11.dll"), b"d3d11-32")
}

fn wine_tree(builder: TarBuilder, base: &str) -> TarBuilder {
    builder
        .file_mode(&format!("{base}/bin/wine"), b"#!", 0o755)
        .file_mode(&format!("{base}/bin/wineserver"), b"#!", 0o755)
        .file(&format!("{base}/lib/wine/x86_64-windows/ntdll.dll"), b"pe")
        .file(&format!("{base}/lib/wine/x86_64-unix/ntdll.so"), b"so")
        .file(&format!("{base}/share/wine/wine.inf"), b"inf")
}

#[test]
fn installs_dxmt_and_records_its_metadata() {
    let fixture = Fixture::new();
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    let before = uncork_core::unix_now();
    let installed = component::install_from_archive(&fixture.layout, &entry, &archive).unwrap();

    let dir = fixture.layout.component_dir(ComponentKind::Dxmt, "0.80");
    assert_eq!(installed.path, dir);
    assert_eq!(installed.meta.kind, ComponentKind::Dxmt);
    assert_eq!(installed.meta.version, "0.80");
    assert_eq!(installed.meta.schema, 1);
    assert_eq!(
        installed.meta.source,
        Source::Catalog {
            url: entry.url.clone(),
            sha256: entry.sha256.clone()
        }
    );
    assert_eq!(installed.meta.license, "MIT");
    assert_eq!(
        installed.meta.source_code.as_deref(),
        Some("https://example.com/source")
    );
    assert!(installed.has_feature("dxmt"));
    assert!(!installed.has_feature("msync"));
    assert!(installed.meta.installed_unix >= before);

    assert_eq!(
        tree(&dir),
        [
            META_FILE,
            "i386-windows/",
            "i386-windows/d3d11.dll",
            "x86_64-unix/",
            "x86_64-unix/winemetal.so",
            "x86_64-windows/",
            "x86_64-windows/d3d11.dll",
            "x86_64-windows/dxgi.dll",
            "x86_64-windows/winemetal.dll",
        ]
    );
    let on_disk: ComponentMeta =
        toml::from_str(&fs::read_to_string(dir.join(META_FILE)).unwrap()).unwrap();
    assert_eq!(on_disk, installed.meta);
    assert_eq!(
        fixture.kind_dir_tree(ComponentKind::Dxmt)[0],
        "0.80/",
        "no staging left behind"
    );
    assert_eq!(
        component::list_installed(&fixture.layout).unwrap(),
        [installed]
    );
}

#[test]
fn honors_an_explicit_strip_prefix() {
    let fixture = Fixture::new();
    let builder = dxmt("dxmt-v0.70-builtin").file("dxmt-v0.70-builtin/README.md", b"");
    let (archive, mut entry) = fixture.archive(ComponentKind::Dxmt, "0.70", builder);
    entry.strip_prefix = Some("dxmt-v0.70-builtin".to_owned());
    let installed = component::install_from_archive(&fixture.layout, &entry, &archive).unwrap();
    assert!(installed.path.join("README.md").is_file());
    assert!(installed.path.join("x86_64-windows/d3d11.dll").is_file());
}

#[test]
fn renames_dxvk_architecture_directories() {
    let fixture = Fixture::new();
    let top = "dxvk-macOS-async-v1.10.3-20230507-repack-builtin";
    let builder = TarBuilder::new()
        .file(&format!("{top}/x64/d3d11.dll"), b"64")
        .file(&format!("{top}/x64/d3d10core.dll"), b"64")
        .file(&format!("{top}/x32/d3d11.dll"), b"32");
    let installed = fixture.install(ComponentKind::Dxvk, "1.10.3-20230507", builder);
    assert_eq!(
        tree(&installed.path),
        [
            META_FILE,
            "i386-windows/",
            "i386-windows/d3d11.dll",
            "x86_64-windows/",
            "x86_64-windows/d3d10core.dll",
            "x86_64-windows/d3d11.dll",
        ]
    );
    assert_eq!(
        fs::read(installed.path.join("i386-windows/d3d11.dll")).unwrap(),
        b"32"
    );
}

#[test]
fn moves_a_nested_wine_tree_to_the_top() {
    // WhiskyWine-style Libraries.tar.gz: the lone `Libraries/` is stripped,
    // then `Wine/` is found one level down.
    let fixture = Fixture::new();
    let builder = wine_tree(TarBuilder::new(), "Libraries/Wine")
        .file("Libraries/winetricks", b"sh")
        .file("Libraries/DXVK/x64/d3d11.dll", b"dll");
    let installed = fixture.install(ComponentKind::Wine, "winecx-gptk-4.7.3", builder);
    let listing = tree(&installed.path);
    for expected in [
        "bin/wine",
        "bin/wineserver",
        "lib/wine/x86_64-unix/ntdll.so",
        "share/wine/wine.inf",
        "winetricks",
        "DXVK/x64/d3d11.dll",
    ] {
        assert!(
            listing.iter().any(|p| p == expected),
            "{expected} missing from {listing:?}"
        );
    }
    assert!(!installed.path.join("Wine").exists());
    assert_eq!(installed.meta.features, ["wow64", "dxmt"]);
}

#[test]
fn moves_a_wine_bundle_out_of_a_tar_xz() {
    // A Sikarugir-style engine next to another top-level file, so the
    // bundle is not stripped automatically.
    let fixture = Fixture::new();
    let archive = fixture.dir.path().join("WS12WineSikarugir11.0_1.tar.xz");
    wine_tree(TarBuilder::new(), "wswine.bundle")
        .file("version", b"11.0_1")
        .write_tar_xz(&archive);
    let entry = entry_for(
        ComponentKind::Wine,
        "sikarugir-11.0_1",
        &archive,
        ArchiveFormat::TarXz,
        "x.tar.xz",
    );
    let installed = component::install_from_archive(&fixture.layout, &entry, &archive).unwrap();
    assert!(installed.path.join("bin/wine").is_file());
    assert!(installed.path.join("version").is_file());
    assert!(!installed.path.join("wswine.bundle").exists());
    component::validate_layout(ComponentKind::Wine, &installed.path).unwrap();
}

#[test]
fn accepts_a_wine64_only_runtime() {
    let fixture = Fixture::new();
    let builder = TarBuilder::new()
        .file("bin/wine64", b"#!")
        .file("lib/wine/x86_64-windows/ntdll.dll", b"")
        .file("lib/wine/x86_64-unix/ntdll.so", b"");
    let installed = fixture.install(ComponentKind::Wine, "8.0", builder);
    assert!(installed.path.join("bin/wine64").is_file());
}

#[test]
fn symlinks_inside_a_nested_wine_tree_survive_the_move() {
    let fixture = Fixture::new();
    let builder = wine_tree(TarBuilder::new(), "Libraries/Wine")
        .file("Libraries/Wine/lib/libwine.1.0.dylib", b"lib")
        .symlink("Libraries/Wine/lib/libwine.1.dylib", "libwine.1.0.dylib")
        .symlink(
            "Libraries/Wine/lib/wine/x86_64-unix/up",
            "../../../bin/wine",
        );
    let installed = fixture.install(ComponentKind::Wine, "11.0", builder);
    assert_eq!(
        fs::read(installed.path.join("lib/libwine.1.dylib")).unwrap(),
        b"lib"
    );
    assert_eq!(
        fs::read(installed.path.join("lib/wine/x86_64-unix/up")).unwrap(),
        b"#!"
    );
}

#[test]
fn a_symlink_that_would_leave_the_component_blocks_the_wine_move() {
    // Inside the archive the link reaches `winetricks` next to `Wine/`; once
    // `Wine/`'s contents move up it would point outside the component.
    let fixture = Fixture::new();
    let builder = wine_tree(TarBuilder::new(), "Libraries/Wine")
        .file("Libraries/winetricks", b"sh")
        .symlink("Libraries/Wine/bin/winetricks", "../../winetricks");
    let (archive, entry) = fixture.archive(ComponentKind::Wine, "11.0", builder);
    match component::install_from_archive(&fixture.layout, &entry, &archive) {
        Err(Error::BrokenComponent { kind, message, .. }) => {
            assert_eq!(kind, ComponentKind::Wine);
            assert!(
                message.starts_with("cannot move Wine to the top"),
                "{message}"
            );
            assert!(message.contains("outside the destination"), "{message}");
        }
        other => panic!("expected a broken component, got {other:?}"),
    }
    assert!(fixture.kind_dir_tree(ComponentKind::Wine).is_empty());
}

#[test]
fn a_checksum_mismatch_installs_nothing() {
    let fixture = Fixture::new();
    let (archive, mut entry) = fixture.archive(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    let actual = entry.sha256.clone();
    entry.sha256 = "0".repeat(64);
    match component::install_from_archive(&fixture.layout, &entry, &archive) {
        Err(Error::Checksum {
            url,
            expected,
            actual: got,
        }) => {
            assert_eq!(url, entry.url);
            assert_eq!(expected, "0".repeat(64));
            assert_eq!(got, actual);
        }
        other => panic!("expected a checksum error, got {other:?}"),
    }
    assert!(fixture.kind_dir_tree(ComponentKind::Dxmt).is_empty());
    assert!(fixture.installed().is_empty());
}

#[test]
fn an_incomplete_release_is_reported_and_cleaned_up() {
    let fixture = Fixture::new();
    let builder = TarBuilder::new()
        .file("v0.80/x86_64-windows/d3d11.dll", b"")
        .file("v0.80/x86_64-windows/dxgi.dll", b"")
        .file("v0.80/x86_64-windows/winemetal.dll", b"");
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", builder);
    match component::install_from_archive(&fixture.layout, &entry, &archive) {
        Err(Error::BrokenComponent { kind, message, .. }) => {
            assert_eq!(kind, ComponentKind::Dxmt);
            assert_eq!(message, "missing x86_64-unix/winemetal.so");
        }
        other => panic!("expected a broken component, got {other:?}"),
    }
    assert!(
        fixture.kind_dir_tree(ComponentKind::Dxmt).is_empty(),
        "{:?}",
        fixture.kind_dir_tree(ComponentKind::Dxmt)
    );
}

#[test]
fn an_unsafe_archive_installs_nothing() {
    let fixture = Fixture::new();
    let builder = dxmt("v0.80").symlink("v0.80/x86_64-unix/evil", "/etc/passwd");
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", builder);
    let result = component::install_from_archive(&fixture.layout, &entry, &archive);
    assert!(matches!(result, Err(Error::Archive { .. })), "{result:?}");
    assert!(fixture.kind_dir_tree(ComponentKind::Dxmt).is_empty());
}

#[test]
fn a_leftover_staging_directory_is_replaced() {
    let fixture = Fixture::new();
    let staging = fixture
        .layout
        .component_kind_dir(ComponentKind::Dxmt)
        .join(".staging-0.80");
    fs::create_dir_all(staging.join("junk")).unwrap();
    let installed = fixture.install(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    assert!(!installed.path.join("junk").exists());
    assert!(!staging.exists());
}

#[test]
fn installing_twice_fails() {
    let fixture = Fixture::new();
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    component::install_from_archive(&fixture.layout, &entry, &archive).unwrap();
    match component::install_from_archive(&fixture.layout, &entry, &archive) {
        Err(Error::AlreadyExists { what, name, path }) => {
            assert_eq!(what, "component");
            assert_eq!(name, "dxmt 0.80");
            assert_eq!(
                path,
                fixture.layout.component_dir(ComponentKind::Dxmt, "0.80")
            );
        }
        other => panic!("expected already exists, got {other:?}"),
    }
    // The catalog path refuses before it would download anything.
    let result = component::install_from_catalog(&fixture.layout, &entry, &mut NoProgress);
    assert!(
        matches!(result, Err(Error::AlreadyExists { .. })),
        "{result:?}"
    );
}

#[test]
fn install_from_catalog_reuses_a_verified_download() {
    let fixture = Fixture::new();
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    let downloads = fixture.layout.downloads_dir();
    fs::create_dir_all(&downloads).unwrap();
    let cached = downloads.join(format!("{}-dxmt-0.80.tar.gz", &entry.sha256[..16]));
    fs::copy(&archive, &cached).unwrap();
    // The URL cannot resolve, so this only passes if nothing is downloaded.
    let installed =
        component::install_from_catalog(&fixture.layout, &entry, &mut NoProgress).unwrap();
    assert!(installed.path.join("x86_64-unix/winemetal.so").is_file());
    assert!(cached.is_file(), "the verified download is kept");
}

#[test]
fn invalid_entries_are_refused_before_touching_anything() {
    let fixture = Fixture::new();
    let (archive, entry) = fixture.archive(ComponentKind::Dxmt, "0.80", dxmt("v0.80"));
    let mut cases = Vec::new();
    let mut bad = entry.clone();
    bad.version = "../escape".to_owned();
    cases.push(bad);
    let mut bad = entry.clone();
    bad.kind = ComponentKind::D3dmetal;
    cases.push(bad);
    let mut bad = entry;
    bad.url = "http://example.com/x.tar.gz".to_owned();
    cases.push(bad);
    for bad in cases {
        let result = component::install_from_archive(&fixture.layout, &bad, &archive);
        assert!(
            matches!(
                result,
                Err(Error::Config {
                    what: "catalog",
                    ..
                })
            ),
            "{result:?}"
        );
        let result = component::install_from_catalog(&fixture.layout, &bad, &mut NoProgress);
        assert!(
            matches!(
                result,
                Err(Error::Config {
                    what: "catalog",
                    ..
                })
            ),
            "{result:?}"
        );
    }
    assert!(!fixture.layout.root().exists());
}

#[test]
fn lists_by_kind_then_newest_version() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxvk, "1.10", dxmt("t"));
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    fixture.install(ComponentKind::Dxmt, "0.10", dxmt("t"));
    fixture.install(
        ComponentKind::Wine,
        "10.0",
        wine_tree(TarBuilder::new(), "w"),
    );
    assert_eq!(
        fixture.installed(),
        [
            (ComponentKind::Wine, "10.0".to_owned()),
            (ComponentKind::Dxmt, "0.10".to_owned()),
            (ComponentKind::Dxmt, "0.9".to_owned()),
            (ComponentKind::Dxvk, "1.10".to_owned()),
        ]
    );
}

#[test]
fn listing_without_a_components_directory_is_empty() {
    let fixture = Fixture::new();
    assert!(
        component::list_installed(&fixture.layout)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn listing_skips_leftovers_and_unreadable_metadata() {
    let fixture = Fixture::new();
    let good = fixture.install(ComponentKind::Dxmt, "0.80", dxmt("t"));
    let kind_dir = fixture.layout.component_kind_dir(ComponentKind::Dxmt);
    // A staging directory that got as far as writing its metadata.
    let staging = kind_dir.join(".staging-0.81");
    fs::create_dir_all(&staging).unwrap();
    fs::copy(good.path.join(META_FILE), staging.join(META_FILE)).unwrap();
    fs::create_dir_all(kind_dir.join(".removing-0.70")).unwrap();
    fs::create_dir_all(kind_dir.join("no-metadata")).unwrap();
    fs::create_dir_all(kind_dir.join("garbage")).unwrap();
    fs::write(kind_dir.join("garbage").join(META_FILE), "not = [toml").unwrap();
    // Metadata copied into a directory of another name.
    fs::create_dir_all(kind_dir.join("0.99")).unwrap();
    fs::copy(
        good.path.join(META_FILE),
        kind_dir.join("0.99").join(META_FILE),
    )
    .unwrap();
    fs::write(kind_dir.join(".DS_Store"), b"").unwrap();
    // A directory for a kind Uncork does not know.
    fs::create_dir_all(fixture.layout.components_dir().join("proton/9.0")).unwrap();

    assert_eq!(
        fixture.installed(),
        [(ComponentKind::Dxmt, "0.80".to_owned())]
    );
}

#[test]
fn listing_fails_when_the_components_directory_is_unusable() {
    let fixture = Fixture::new();
    fs::create_dir_all(fixture.layout.root()).unwrap();
    fs::write(fixture.layout.components_dir(), b"not a directory").unwrap();
    let result = component::list_installed(&fixture.layout);
    assert!(matches!(result, Err(Error::Io { .. })), "{result:?}");
}

#[test]
fn finds_the_newest_or_an_exact_version() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    fixture.install(ComponentKind::Dxmt, "0.10", dxmt("t"));
    let newest = component::find_installed(&fixture.layout, ComponentKind::Dxmt, None).unwrap();
    assert_eq!(newest.meta.version, "0.10");
    assert_eq!(
        newest.path,
        fixture.layout.component_dir(ComponentKind::Dxmt, "0.10")
    );
    let exact =
        component::find_installed(&fixture.layout, ComponentKind::Dxmt, Some("0.9")).unwrap();
    assert_eq!(exact.meta.version, "0.9");
}

#[test]
fn not_found_names_the_install_command() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    let error =
        component::find_installed(&fixture.layout, ComponentKind::Dxmt, Some("0.8")).unwrap_err();
    assert_eq!(
        error.to_string(),
        "component \"dxmt 0.8\" not found; run: uncork runtime install dxmt"
    );
    let error = component::find_installed(&fixture.layout, ComponentKind::Wine, None).unwrap_err();
    assert_eq!(
        error.to_string(),
        "component \"wine\" not found; run: uncork runtime install wine"
    );
    let error =
        component::find_installed(&fixture.layout, ComponentKind::D3dmetal, None).unwrap_err();
    assert!(
        error.to_string().contains("uncork runtime import-gptk"),
        "{error}"
    );
}

#[test]
fn removes_a_component() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    fixture.install(ComponentKind::Dxmt, "0.10", dxmt("t"));
    component::remove(&fixture.layout, ComponentKind::Dxmt, "0.10").unwrap();
    assert_eq!(
        fixture.installed(),
        [(ComponentKind::Dxmt, "0.9".to_owned())]
    );
    let kind_dir = fixture.layout.component_kind_dir(ComponentKind::Dxmt);
    let names: Vec<String> = fs::read_dir(&kind_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["0.9"], "no .removing-* leftover");

    match component::remove(&fixture.layout, ComponentKind::Dxmt, "0.10") {
        Err(Error::NotFound { what, name, .. }) => {
            assert_eq!(what, "component");
            assert_eq!(name, "dxmt 0.10");
        }
        other => panic!("expected not found, got {other:?}"),
    }
}

#[test]
fn removal_refuses_path_tricks() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    for version in ["..", "../dxmt", "", "0.9/../0.9", "/"] {
        let result = component::remove(&fixture.layout, ComponentKind::Dxvk, version);
        assert!(
            matches!(result, Err(Error::InvalidName { .. })),
            "{version:?}: {result:?}"
        );
    }
    assert_eq!(fixture.installed().len(), 1);
}

#[test]
fn removal_clears_a_leftover_from_an_interrupted_removal() {
    let fixture = Fixture::new();
    fixture.install(ComponentKind::Dxmt, "0.9", dxmt("t"));
    let leftover = fixture
        .layout
        .component_kind_dir(ComponentKind::Dxmt)
        .join(".removing-0.9");
    fs::create_dir_all(leftover.join("old")).unwrap();
    component::remove(&fixture.layout, ComponentKind::Dxmt, "0.9").unwrap();
    assert!(fixture.kind_dir_tree(ComponentKind::Dxmt).is_empty());
}

#[cfg(target_os = "macos")]
mod gptk {
    use std::os::unix::fs::symlink;

    use super::*;

    const LIBD3DSHARED_LINK: &str = "../../external/libd3dshared.dylib";

    fn plist(version: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleIdentifier</key>
	<string>com.apple.D3DMetal</string>
	<key>CFBundleShortVersionString</key>
	<string>{version}</string>
</dict>
</plist>
"#
        )
    }

    /// A GPTK volume with the layout of the real one: a versioned framework
    /// with its symlinks, and `.so` files that are symlinks to libd3dshared.
    fn fake_volume(root: &Path, version: &str) -> PathBuf {
        let lib = root.join("redist/lib");
        let framework = lib.join("external/D3DMetal.framework");
        let resources = framework.join("Versions/A/Resources");
        fs::create_dir_all(&resources).unwrap();
        fs::write(resources.join("Info.plist"), plist(version)).unwrap();
        fs::write(framework.join("Versions/A/D3DMetal"), b"mach-o").unwrap();
        symlink("A", framework.join("Versions/Current")).unwrap();
        symlink("Versions/Current/Resources", framework.join("Resources")).unwrap();
        symlink("Versions/Current/D3DMetal", framework.join("D3DMetal")).unwrap();
        fs::write(lib.join("external/libd3dshared.dylib"), b"dylib").unwrap();
        let windows = lib.join("wine/x86_64-windows");
        let unix = lib.join("wine/x86_64-unix");
        fs::create_dir_all(&windows).unwrap();
        fs::create_dir_all(&unix).unwrap();
        for dll in ["d3d11", "d3d12", "dxgi"] {
            fs::write(windows.join(format!("{dll}.dll")), b"pe").unwrap();
            symlink(LIBD3DSHARED_LINK, unix.join(format!("{dll}.so"))).unwrap();
        }
        root.to_path_buf()
    }

    fn volume(fixture: &Fixture, version: &str) -> PathBuf {
        fake_volume(
            &fixture
                .dir
                .path()
                .join("Evaluation environment for Windows games"),
            version,
        )
    }

    #[test]
    fn imports_from_the_volume_redist_or_redist_lib() {
        for suffix in ["", "redist", "redist/lib"] {
            let fixture = Fixture::new();
            let root = volume(&fixture, "3.0");
            let source = if suffix.is_empty() {
                root.clone()
            } else {
                root.join(suffix)
            };
            let installed = component::import_gptk(&fixture.layout, &source).unwrap();

            assert_eq!(installed.meta.kind, ComponentKind::D3dmetal);
            assert_eq!(installed.meta.version, "3.0");
            assert_eq!(
                installed.path,
                fixture.layout.component_dir(ComponentKind::D3dmetal, "3.0")
            );
            assert_eq!(
                installed.meta.source,
                Source::Local {
                    path: root.join("redist/lib")
                }
            );
            assert!(installed.meta.license.starts_with("LicenseRef-"));
            assert_eq!(installed.meta.source_code, None);
            component::validate_layout(ComponentKind::D3dmetal, &installed.path).unwrap();
            assert_eq!(
                component::list_installed(&fixture.layout).unwrap(),
                [installed]
            );
        }
    }

    #[test]
    fn keeps_symlinks_as_symlinks() {
        let fixture = Fixture::new();
        let installed = component::import_gptk(&fixture.layout, &volume(&fixture, "3.0")).unwrap();
        let listing = tree(&installed.path);
        for expected in [
            "external/D3DMetal.framework/D3DMetal -> Versions/Current/D3DMetal",
            "external/D3DMetal.framework/Resources -> Versions/Current/Resources",
            "external/D3DMetal.framework/Versions/Current -> A",
            "external/libd3dshared.dylib",
            "wine/x86_64-unix/d3d11.so -> ../../external/libd3dshared.dylib",
            "wine/x86_64-windows/dxgi.dll",
            META_FILE,
        ] {
            assert!(
                listing.iter().any(|p| p == expected),
                "{expected} missing from {listing:?}"
            );
        }
        // The relative links resolve inside the component, not the volume.
        let so = installed.path.join("wine/x86_64-unix/dxgi.so");
        assert_eq!(
            fs::canonicalize(&so).unwrap(),
            fs::canonicalize(installed.path.join("external/libd3dshared.dylib")).unwrap()
        );
    }

    #[test]
    fn falls_back_to_the_versioned_info_plist() {
        let fixture = Fixture::new();
        let root = volume(&fixture, "4.0");
        fs::remove_file(root.join("redist/lib/external/D3DMetal.framework/Resources")).unwrap();
        let installed = component::import_gptk(&fixture.layout, &root).unwrap();
        assert_eq!(installed.meta.version, "4.0");
    }

    #[test]
    fn importing_twice_fails() {
        let fixture = Fixture::new();
        let root = volume(&fixture, "3.0");
        component::import_gptk(&fixture.layout, &root).unwrap();
        let result = component::import_gptk(&fixture.layout, &root);
        assert!(
            matches!(result, Err(Error::AlreadyExists { .. })),
            "{result:?}"
        );
    }

    #[test]
    fn a_directory_without_the_framework_is_not_found() {
        let fixture = Fixture::new();
        let empty = fixture.dir.path().join("empty");
        fs::create_dir_all(empty.join("redist/lib/external")).unwrap();
        match component::import_gptk(&fixture.layout, &empty) {
            Err(error @ Error::NotFound { .. }) => {
                assert!(error.to_string().contains("D3DMetal.framework"), "{error}");
            }
            other => panic!("expected not found, got {other:?}"),
        }
    }

    #[test]
    fn a_plist_without_a_version_is_a_command_error() {
        let fixture = Fixture::new();
        let root = volume(&fixture, "3.0");
        let plist_path =
            root.join("redist/lib/external/D3DMetal.framework/Versions/A/Resources/Info.plist");
        fs::write(
            &plist_path,
            plist("3.0").replace("CFBundleShortVersionString", "CFBundleVersion"),
        )
        .unwrap();
        let result = component::import_gptk(&fixture.layout, &root);
        assert!(
            matches!(result, Err(Error::Command { ref program, .. }) if program == "plutil"),
            "{result:?}"
        );
        assert!(fixture.kind_dir_tree(ComponentKind::D3dmetal).is_empty());
    }

    #[test]
    fn an_unusable_version_string_is_refused() {
        let fixture = Fixture::new();
        let result = component::import_gptk(&fixture.layout, &volume(&fixture, "4.0 beta 2"));
        assert!(
            matches!(
                result,
                Err(Error::InvalidName {
                    what: "version",
                    ..
                })
            ),
            "{result:?}"
        );
        assert!(fixture.kind_dir_tree(ComponentKind::D3dmetal).is_empty());
    }

    #[test]
    fn an_incomplete_toolkit_is_reported_and_cleaned_up() {
        let fixture = Fixture::new();
        let root = volume(&fixture, "3.0");
        fs::remove_dir_all(root.join("redist/lib/wine")).unwrap();
        match component::import_gptk(&fixture.layout, &root) {
            Err(Error::BrokenComponent { message, .. }) => {
                assert_eq!(message, "missing wine/x86_64-windows");
            }
            other => panic!("expected a broken component, got {other:?}"),
        }
        assert!(fixture.kind_dir_tree(ComponentKind::D3dmetal).is_empty());
    }
}
