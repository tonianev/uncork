//! `SteamInstall` against a fake Wine prefix built in a temporary directory:
//!
//! ```text
//! <tmp>/prefix/drive_c/Program Files (x86)/Steam/steam.exe
//!                                               /steamapps/{libraryfolders.vdf, appmanifest_*.acf}
//! <tmp>/prefix/dosdevices/c: -> ../drive_c
//! <tmp>/prefix/dosdevices/d: -> <tmp>/external
//! <tmp>/external/SteamLibrary/steamapps/appmanifest_813780.acf
//! ```
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use tempfile::TempDir;
use uncork_steam::SteamInstall;
use uncork_steam::library::LibraryError;

fn fixture(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

fn write(path: &Path, contents: impl AsRef<[u8]>) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent directories");
    }
    fs::write(path, contents).expect("write file");
}

struct FakePrefix {
    _dir: TempDir,
    prefix: PathBuf,
    /// Where `dosdevices/d:` points.
    d_drive: PathBuf,
}

impl FakePrefix {
    /// A prefix with Steam installed and no libraries configured.
    fn bare() -> Self {
        let dir = tempfile::tempdir().expect("temporary directory");
        let prefix = dir.path().join("prefix");
        let d_drive = dir.path().join("external");
        fs::create_dir_all(&d_drive).expect("create D: target");
        let dosdevices = prefix.join("dosdevices");
        fs::create_dir_all(&dosdevices).expect("create dosdevices");
        symlink("../drive_c", dosdevices.join("c:")).expect("link C:");
        symlink(&d_drive, dosdevices.join("d:")).expect("link D:");
        let fake = Self {
            _dir: dir,
            prefix,
            d_drive,
        };
        write(&fake.steam_root().join("steam.exe"), b"MZ");
        fake
    }

    /// The fixture setup: libraries on C: and D:, three valid manifests and
    /// one truncated one.
    fn with_libraries() -> Self {
        let fake = Self::bare();
        let steamapps = fake.steamapps();
        fake.write_steamapps_fixture("libraryfolders.vdf");
        for name in [
            "appmanifest_287450.acf",
            "appmanifest_228980.acf",
            "appmanifest_malformed.acf",
        ] {
            fake.write_steamapps_fixture(name);
        }
        fs::create_dir_all(steamapps.join("common").join("Rise of Nations"))
            .expect("create install directory");
        write(
            &fake
                .d_library()
                .join("steamapps")
                .join("appmanifest_813780.acf"),
            fixture("appmanifest_813780.acf"),
        );
        fake
    }

    fn steam_root(&self) -> PathBuf {
        self.prefix
            .join("drive_c")
            .join("Program Files (x86)")
            .join("Steam")
    }

    fn steamapps(&self) -> PathBuf {
        self.steam_root().join("steamapps")
    }

    /// `D:\SteamLibrary` as `SteamInstall` reports it (through the symlink).
    fn d_library(&self) -> PathBuf {
        self.prefix
            .join("dosdevices")
            .join("d:")
            .join("SteamLibrary")
    }

    fn write_steamapps_fixture(&self, name: &str) {
        write(&self.steamapps().join(name), fixture(name));
    }

    fn write_library_folders(&self, text: &str) {
        write(&self.steamapps().join("libraryfolders.vdf"), text);
    }

    fn install(&self) -> SteamInstall {
        SteamInstall::find(&self.prefix).expect("steam.exe is installed")
    }
}

fn appids(apps: &[uncork_steam::InstalledApp]) -> Vec<u32> {
    apps.iter().map(|app| app.manifest.appid).collect()
}

#[test]
fn find_locates_steam_at_the_default_path() {
    let fake = FakePrefix::bare();
    let install = fake.install();
    assert_eq!(
        install,
        SteamInstall {
            prefix: fake.prefix.clone(),
            root: fake.steam_root(),
        }
    );
    assert_eq!(install.exe(), fake.steam_root().join("steam.exe"));
}

#[test]
fn find_matches_the_installers_spelling_and_exe_keeps_it() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let root = dir.path().join("drive_c/Program Files (x86)/Steam");
    write(&root.join("Steam.exe"), b"MZ");
    let install = SteamInstall::find(dir.path()).expect("Steam.exe is found");
    assert_eq!(install.root, root);
    assert_eq!(
        install.exe().file_name().and_then(|name| name.to_str()),
        Some("Steam.exe"),
        "the name on disk, not an assumed one"
    );
}

#[test]
fn find_requires_a_steam_exe_file() {
    let dir = tempfile::tempdir().expect("temporary directory");
    assert_eq!(SteamInstall::find(dir.path()), None);
    assert_eq!(SteamInstall::find(&dir.path().join("missing")), None);

    let root = dir.path().join("drive_c/Program Files (x86)/Steam");
    fs::create_dir_all(root.join("steam.exe")).expect("directory named steam.exe");
    assert_eq!(SteamInstall::find(dir.path()), None);
}

#[test]
fn libraries_without_libraryfolders_is_just_the_root() {
    let fake = FakePrefix::bare();
    assert_eq!(
        fake.install().libraries().expect("libraries"),
        [fake.steam_root()]
    );
}

#[test]
fn libraries_maps_c_and_d_through_the_prefix() {
    let fake = FakePrefix::with_libraries();
    let libraries = fake.install().libraries().expect("libraries");
    assert_eq!(libraries, [fake.steam_root(), fake.d_library()]);
    assert!(
        libraries[1].join("steamapps").is_dir(),
        "D: resolves through the dosdevices symlink"
    );
}

#[test]
fn libraries_reads_the_legacy_format() {
    let fake = FakePrefix::bare();
    fake.write_library_folders(&fixture("libraryfolders_legacy.vdf"));
    assert_eq!(
        fake.install().libraries().expect("libraries"),
        [fake.steam_root(), fake.d_library()]
    );
}

#[test]
fn libraries_skips_unmappable_paths_and_duplicates_ignoring_case() {
    let fake = FakePrefix::bare();
    fake.write_library_folders(
        r#""libraryfolders"
{
	"0" { "path" "c:\\program files (x86)\\steam" }
	"1" { "path" "D:\\SteamLibrary" }
	"2" { "path" "\\\\nas\\games" }
	"3" { "path" "Games\\Relative" }
	"4" { "path" "d:/steamlibrary/" }
	"5" { "path" "E:\\Games" }
	"6" { "path" "D:\\SteamLibrary" }
	"7" { "path" "C:\\..\\escape" }
}
"#,
    );
    assert_eq!(
        fake.install().libraries().expect("libraries"),
        [
            fake.steam_root(),
            fake.d_library(),
            fake.prefix.join("dosdevices").join("e:").join("Games"),
        ]
    );
}

#[test]
fn libraries_reports_an_unreadable_libraryfolders() {
    let fake = FakePrefix::bare();
    let file = fake.steamapps().join("libraryfolders.vdf");
    fs::create_dir_all(&file).expect("directory in place of the file");
    match fake.install().libraries() {
        Err(LibraryError::Io { path, .. }) => assert_eq!(path, file),
        other => panic!("expected an I/O error, got {other:?}"),
    }
}

#[test]
fn libraries_reports_a_malformed_libraryfolders() {
    let fake = FakePrefix::bare();
    fake.write_library_folders("\"libraryfolders\"\n{\n\t\"0\"\n\t{\n");
    let error = fake.install().libraries().expect_err("malformed file");
    let file = fake.steamapps().join("libraryfolders.vdf");
    assert_eq!(
        error.to_string(),
        format!(
            "{}: line 5, column 1: unclosed '{{' opened at line 4",
            file.display()
        )
    );
    assert!(matches!(error, LibraryError::Parse { path, .. } if path == file));
}

#[test]
fn installed_apps_lists_every_library_sorted_by_appid() {
    let fake = FakePrefix::with_libraries();
    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert_eq!(appids(&apps), [228_980, 287_450, 813_780]);
    let libraries: Vec<&Path> = apps.iter().map(|app| app.library.as_path()).collect();
    let root = fake.steam_root();
    let d_library = fake.d_library();
    assert_eq!(
        libraries,
        [root.as_path(), root.as_path(), d_library.as_path()]
    );
    assert_eq!(
        apps[2].install_path,
        fake.d_library().join("steamapps/common/AoE2DE")
    );
    assert!(!apps[2].manifest.is_fully_installed());

    assert_eq!(errors.len(), 1, "{errors:?}");
    let malformed = fake.steamapps().join("appmanifest_malformed.acf");
    assert_eq!(
        errors[0].to_string(),
        format!(
            "{}: line 5, column 10: unterminated string",
            malformed.display()
        )
    );
}

#[test]
fn find_app_resolves_rise_of_nations() {
    let fake = FakePrefix::with_libraries();
    let app = fake
        .install()
        .find_app(287_450)
        .expect("listing")
        .expect("Rise of Nations is installed");
    assert_eq!(app.manifest.name, "Rise of Nations: Extended Edition");
    assert!(app.manifest.is_fully_installed());
    assert_eq!(app.library, fake.steam_root());
    assert_eq!(
        app.install_path,
        fake.steamapps().join("common").join("Rise of Nations")
    );
    assert!(app.install_path.is_dir());
}

#[test]
fn find_app_returns_none_for_unknown_and_malformed_apps() {
    let fake = FakePrefix::with_libraries();
    let install = fake.install();
    assert_eq!(install.find_app(1).expect("listing"), None);
    assert_eq!(install.find_app(999_990).expect("listing"), None);
}

#[test]
fn find_app_fails_when_libraries_fail() {
    let fake = FakePrefix::bare();
    fake.write_library_folders("}");
    assert!(matches!(
        fake.install().find_app(287_450),
        Err(LibraryError::Parse { .. })
    ));
}

#[test]
fn installed_apps_is_empty_for_a_fresh_install() {
    let fake = FakePrefix::bare();
    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert!(apps.is_empty());
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn missing_library_contributes_nothing() {
    let fake = FakePrefix::bare();
    fake.write_steamapps_fixture("appmanifest_287450.acf");
    fake.write_library_folders(r#""libraryfolders" { "1" { "path" "F:\\Unplugged" } }"#);
    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert_eq!(appids(&apps), [287_450]);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn unlistable_library_is_reported_and_others_still_listed() {
    let fake = FakePrefix::with_libraries();
    let not_a_directory = fake.d_library().join("steamapps");
    fs::remove_dir_all(&not_a_directory).expect("remove D: steamapps");
    write(&not_a_directory, "not a directory");
    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert_eq!(appids(&apps), [228_980, 287_450]);
    let io_paths: Vec<&Path> = errors
        .iter()
        .filter_map(|error| match error {
            LibraryError::Io { path, .. } => Some(path.as_path()),
            _ => None,
        })
        .collect();
    assert_eq!(io_paths, [not_a_directory.as_path()]);
}

#[test]
fn unreadable_and_invalid_manifests_are_collected_with_their_paths() {
    let fake = FakePrefix::bare();
    let steamapps = fake.steamapps();
    fake.write_steamapps_fixture("appmanifest_287450.acf");
    let binary = steamapps.join("appmanifest_10.acf");
    write(&binary, b"\"AppState\" { \"name\" \"\xff\xfe\" }");
    let schema = steamapps.join("appmanifest_20.acf");
    write(&schema, "\"AppState\"\n{\n\t\"appid\"\t\t\"20\"\n}\n");

    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert_eq!(appids(&apps), [287_450]);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(matches!(&errors[0], LibraryError::Io { path, .. } if *path == binary));
    match &errors[1] {
        LibraryError::Schema { path, message } => {
            assert_eq!(path, &schema);
            assert_eq!(message, "missing \"name\" in \"AppState\"");
        }
        other => panic!("expected a schema error, got {other:?}"),
    }
}

#[test]
fn only_manifest_files_are_read() {
    let fake = FakePrefix::bare();
    let steamapps = fake.steamapps();
    fake.write_steamapps_fixture("appmanifest_287450.acf");
    fs::create_dir_all(steamapps.join("appmanifest_30.acf")).expect("directory named like one");
    write(&steamapps.join("appmanifest_40.acf.tmp"), "garbage {");
    write(&steamapps.join("appmanifest_50.vdf"), "garbage {");
    write(
        &steamapps.join("AppManifest_228980.ACF"),
        fixture("appmanifest_228980.acf"),
    );
    let (apps, errors) = fake.install().installed_apps().expect("listing");
    assert_eq!(appids(&apps), [228_980, 287_450]);
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn an_app_in_two_libraries_is_listed_per_library_in_library_order() {
    let fake = FakePrefix::with_libraries();
    write(
        &fake
            .d_library()
            .join("steamapps")
            .join("appmanifest_287450.acf"),
        fixture("appmanifest_287450.acf"),
    );
    let install = fake.install();
    let (apps, _) = install.installed_apps().expect("listing");
    let copies: Vec<&Path> = apps
        .iter()
        .filter(|app| app.manifest.appid == 287_450)
        .map(|app| app.library.as_path())
        .collect();
    let root = fake.steam_root();
    let d_library = fake.d_library();
    assert_eq!(copies, [root.as_path(), d_library.as_path()]);
    let found = install
        .find_app(287_450)
        .expect("listing")
        .expect("installed");
    assert_eq!(found.library, fake.steam_root());
}

#[test]
fn d_drive_symlink_target_is_not_resolved_in_reported_paths() {
    let fake = FakePrefix::with_libraries();
    let app = fake
        .install()
        .find_app(813_780)
        .expect("listing")
        .expect("installed on D:");
    assert!(app.library.starts_with(fake.prefix.join("dosdevices")));
    assert!(!app.library.starts_with(&fake.d_drive));
    assert_eq!(
        fs::canonicalize(&app.library).expect("library exists"),
        fs::canonicalize(fake.d_drive.join("SteamLibrary")).expect("target exists")
    );
}
