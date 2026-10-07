//! Game Mode wrapper bundles: the generated `Info.plist` is a valid
//! property list with the keys macOS looks at, and `prepare_bundle` lays
//! out, refreshes and ad-hoc signs a bundle that `codesign` accepts.

mod d_support;

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use d_support::{CX_FEATURES, Fixture, ctx};
use uncork_core::gamemode::{info_plist, open_command, plan_path, prepare_bundle, read_plan};
use uncork_core::launch::{self, LaunchOptions, LaunchPlan, Target};

fn plutil(args: &[&str], file: &Path) -> std::process::Output {
    Command::new("/usr/bin/plutil")
        .args(args)
        .arg(file)
        .output()
        .unwrap()
}

/// The property list as JSON, via Apple's own parser.
fn parse_plist(file: &Path) -> serde_json::Value {
    let output = plutil(&["-convert", "json", "-o", "-"], file);
    assert!(output.status.success(), "{output:?}");
    serde_json::from_slice(&output.stdout).unwrap()
}

fn winecfg_plan(fx: &Fixture) -> LaunchPlan {
    let bottle = fx.bottle("steam", |_| {});
    launch::plan(
        ctx(fx, &bottle, &[], None),
        &Target::WineProgram {
            name: "winecfg".to_owned(),
            args: Vec::new(),
        },
        &LaunchOptions::default(),
    )
    .unwrap()
}

#[test]
fn info_plist_is_a_valid_property_list_with_the_game_keys() {
    if !Path::new("/usr/bin/plutil").exists() {
        return; // not macOS
    }
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("Info.plist");
    let name = "Tom & Jerry's <\"Game\">\u{7}";
    fs::write(&file, info_plist(name, "dev.uncork.game.tom-and-jerry")).unwrap();

    let lint = plutil(&["-lint"], &file);
    assert!(lint.status.success(), "{lint:?}");
    let plist = parse_plist(&file);
    let expected: BTreeMap<&str, serde_json::Value> = BTreeMap::from([
        ("CFBundleName", "Tom & Jerry's <\"Game\">".into()),
        ("CFBundleDisplayName", "Tom & Jerry's <\"Game\">".into()),
        ("CFBundleIdentifier", "dev.uncork.game.tom-and-jerry".into()),
        ("CFBundleExecutable", "launch".into()),
        ("CFBundlePackageType", "APPL".into()),
        (
            "LSApplicationCategoryType",
            "public.app-category.games".into(),
        ),
        ("LSSupportsGameMode", true.into()),
        ("GCSupportsGameMode", true.into()),
        ("LSMinimumSystemVersion", "14.0".into()),
        ("NSHighResolutionCapable", true.into()),
    ]);
    for (key, value) in expected {
        assert_eq!(plist[key], value, "{key}");
    }
}

#[test]
fn prepare_bundle_lays_out_and_signs_the_bundle() {
    if !Path::new("/usr/bin/codesign").exists() {
        return; // not macOS
    }
    let fx = Fixture::new(CX_FEATURES);
    let plan = winecfg_plan(&fx);
    let launcher = PathBuf::from("/usr/bin/true");

    let bundle = prepare_bundle(
        &fx.layout,
        "winecfg",
        "Wine Configuration",
        &plan,
        &launcher,
    )
    .unwrap();
    assert_eq!(bundle, fx.layout.apps_dir().join("winecfg.app"));
    let contents = bundle.join("Contents");
    let executable = contents.join("MacOS/launch");
    assert!(uncork_core::process::is_executable(&executable));
    assert_eq!(read_plan(&plan_path(&bundle)).unwrap(), plan.command);
    assert_eq!(
        parse_plist(&contents.join("Info.plist"))["CFBundleIdentifier"],
        "dev.uncork.game.winecfg"
    );

    let verify = Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(&bundle)
        .output()
        .unwrap();
    assert!(verify.status.success(), "{verify:?}");

    // Nothing changed: not re-signed.
    let signature = contents.join("_CodeSignature/CodeResources");
    let signed = fs::metadata(&signature).unwrap().modified().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    prepare_bundle(
        &fx.layout,
        "winecfg",
        "Wine Configuration",
        &plan,
        &launcher,
    )
    .unwrap();
    assert_eq!(
        fs::metadata(&signature).unwrap().modified().unwrap(),
        signed
    );
}

#[test]
fn prepare_bundle_rejects_ids_that_are_not_file_names() {
    let fx = Fixture::new(CX_FEATURES);
    let plan = winecfg_plan(&fx);
    let err = prepare_bundle(
        &fx.layout,
        "../escape",
        "X",
        &plan,
        Path::new("/usr/bin/true"),
    )
    .unwrap_err();
    assert!(
        matches!(
            err,
            uncork_core::Error::InvalidName {
                what: "app bundle",
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!fx.layout.root().join("escape.app").exists());
}

#[test]
fn bundles_open_through_launch_services_and_wait() {
    let spec = open_command(Path::new("/u/apps/ron.app"));
    assert_eq!(spec.program, Path::new("/usr/bin/open"));
    assert_eq!(spec.args, ["-n", "-W", "/u/apps/ron.app"]);
}
