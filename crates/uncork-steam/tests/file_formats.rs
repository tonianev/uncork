//! Parsing the synthetic Steam files under `tests/fixtures/`, which follow
//! the exact layout the Windows client writes.

use std::fs;
use std::path::PathBuf;

use uncork_steam::library::{
    LibraryError, LibraryFolder, STATE_UPDATE_REQUIRED, parse_app_manifest, parse_library_folders,
};
use uncork_steam::{AppManifest, vdf};

const FIXTURES: [&str; 6] = [
    "libraryfolders.vdf",
    "libraryfolders_legacy.vdf",
    "appmanifest_287450.acf",
    "appmanifest_228980.acf",
    "appmanifest_813780.acf",
    "appmanifest_malformed.acf",
];

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join(name)
}

fn fixture(name: &str) -> String {
    let path = fixture_path(name);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

#[test]
fn library_folders_lists_both_libraries_with_their_apps() {
    let folders = parse_library_folders(&fixture("libraryfolders.vdf")).expect("valid fixture");
    assert_eq!(
        folders,
        [
            LibraryFolder {
                path: r"C:\Program Files (x86)\Steam".to_owned(),
                apps: vec![228_980, 287_450],
            },
            LibraryFolder {
                path: r"D:\SteamLibrary".to_owned(),
                apps: vec![813_780],
            },
        ]
    );
}

#[test]
fn legacy_library_folders_lists_only_numbered_paths() {
    let folders =
        parse_library_folders(&fixture("libraryfolders_legacy.vdf")).expect("valid fixture");
    assert_eq!(
        folders,
        [LibraryFolder {
            path: r"D:\SteamLibrary".to_owned(),
            apps: vec![],
        }]
    );
}

#[test]
fn rise_of_nations_manifest() {
    let manifest = parse_app_manifest(&fixture("appmanifest_287450.acf")).expect("valid fixture");
    assert_eq!(
        manifest,
        AppManifest {
            appid: 287_450,
            name: "Rise of Nations: Extended Edition".to_owned(),
            installdir: "Rise of Nations".to_owned(),
            state_flags: 4,
            size_on_disk: Some(3_175_284_103),
            build_id: Some(10_234_567),
        }
    );
    assert!(manifest.is_fully_installed());
}

#[test]
fn partially_installed_manifest_is_not_ready() {
    let manifest = parse_app_manifest(&fixture("appmanifest_813780.acf")).expect("valid fixture");
    assert_eq!(manifest.state_flags, 1026);
    assert!(!manifest.is_fully_installed());
    assert_ne!(manifest.state_flags & STATE_UPDATE_REQUIRED, 0);
}

#[test]
fn truncated_manifest_reports_where_it_broke() {
    match parse_app_manifest(&fixture("appmanifest_malformed.acf")) {
        Err(LibraryError::Parse { source, .. }) => {
            assert_eq!(source.to_string(), "line 5, column 10: unterminated string");
        }
        other => panic!("expected a parse error, got {other:?}"),
    }
}

#[test]
fn fixtures_render_back_byte_for_byte() {
    for name in FIXTURES.iter().filter(|name| !name.contains("malformed")) {
        let text = fixture(name);
        let parsed = vdf::parse(&text).expect("valid fixture");
        assert_eq!(vdf::to_string(&parsed), text, "{name}");
    }
}

#[test]
fn bom_and_crlf_variants_parse_identically() {
    for name in FIXTURES.iter().filter(|name| !name.contains("malformed")) {
        let text = fixture(name);
        let variant = format!("\u{feff}{}", text.replace('\n', "\r\n"));
        assert_eq!(vdf::parse(&variant), vdf::parse(&text), "{name}");
    }
}

#[test]
fn manifest_lookups_ignore_key_case() {
    let parsed = vdf::parse(&fixture("appmanifest_287450.acf")).expect("valid fixture");
    let state = parsed.get_object("appstate").expect("AppState block");
    let depots = state
        .get_object("shareddepots")
        .expect("SharedDepots block");
    let shared: Vec<(&str, Option<&str>)> = depots.iter().map(|(k, v)| (k, v.as_str())).collect();
    assert_eq!(
        shared,
        [
            ("228984", Some("228980")),
            ("228986", Some("228980")),
            ("228990", Some("228980")),
        ]
    );
    assert_eq!(
        state
            .get_object("UserConfig")
            .and_then(|config| config.get_str("LANGUAGE")),
        Some("english")
    );
}
