//! The `uncork` binary on an empty or lightly populated `UNCORK_HOME`:
//! help, doctor, runtime, profile, inspect and argument errors.

mod common;
mod support;

use std::path::{Path, PathBuf};

use common::PeBuilder;
use predicates::prelude::*;
use predicates::str::contains;
use support::Home;

/// A 32-bit D3D11 game: `game.exe` importing `d3d11.dll`, plus
/// `steam_api.dll` beside it.
fn d3d11_game(dir: &Path) -> PathBuf {
    let game = dir.join("game");
    std::fs::create_dir_all(&game).unwrap();
    let exe = game.join("game.exe");
    PeBuilder::pe32()
        .import("kernel32.dll")
        .import("d3d11.dll")
        .write(&exe);
    PeBuilder::pe32()
        .dll()
        .import("kernel32.dll")
        .write(&game.join("steam_api.dll"));
    exe
}

#[test]
fn help_and_version() {
    let home = Home::new();
    home.uncork()
        .arg("--help")
        .assert()
        .success()
        .stdout(contains("Run Windows games on Apple Silicon Macs"))
        .stdout(contains("doctor"))
        .stdout(contains("__exec").not());
    home.uncork()
        .arg("--version")
        .assert()
        .success()
        .stdout(format!("uncork {}\n", env!("CARGO_PKG_VERSION")));
    home.uncork()
        .args(["runtime", "install", "--help"])
        .assert()
        .success()
        .stdout(contains("--version <VERSION>"));
    home.uncork()
        .arg("frobnicate")
        .assert()
        .code(2)
        .stderr(contains("unrecognized subcommand"));
}

#[test]
fn doctor_on_an_empty_home_fails_with_the_wine_fix() {
    let home = Home::new();
    home.uncork()
        .arg("doctor")
        .assert()
        .code(1)
        .stdout(contains(format!("Uncork {}", env!("CARGO_PKG_VERSION"))))
        .stdout(contains(format!("Data: {}", home.root().display())))
        .stdout(contains(
            "FAIL  no Wine runtime is installed\n      fix: uncork runtime install wine\n",
        ))
        .stdout(contains("info  no bottles yet"));
    assert!(
        !home.root().join("bottles").exists(),
        "doctor creates nothing"
    );
}

#[test]
fn doctor_json_lists_every_check() {
    let home = Home::new();
    let output = home.uncork().args(["doctor", "--json"]).output().unwrap();
    assert_eq!(output.status.code(), Some(1));
    let doctor: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(doctor["ok"], false);
    assert_eq!(doctor["version"], env!("CARGO_PKG_VERSION"));
    let checks = doctor["checks"].as_array().unwrap();
    let wine = checks
        .iter()
        .find(|check| check["id"] == "wine-installed")
        .unwrap();
    assert_eq!(wine["status"], "fail");
    assert_eq!(wine["fix"], "uncork runtime install wine");
}

#[test]
fn doctor_passes_the_wine_check_with_wine_installed() {
    let home = Home::new();
    home.install_fake_wine();
    let doctor = home.uncork().args(["--json", "doctor"]).output().unwrap();
    let doctor: serde_json::Value = serde_json::from_slice(&doctor.stdout).unwrap();
    let wine = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "wine-installed")
        .unwrap()
        .clone();
    assert_eq!(wine["status"], "ok", "{wine}");
}

#[test]
fn runtime_available_lists_the_real_catalog() {
    let home = Home::new();
    let text = home.stdout(&["runtime", "available"]);
    let header = text.lines().next().unwrap();
    assert_eq!(
        header,
        "KIND  VERSION            SIZE      LICENSE            STATUS       NOTES"
    );
    assert!(text.contains("wine  winecx-gptk-4.7.3  461.1 MB"), "{text}");
    assert!(text.contains("dxmt  0.80"), "{text}");
    assert!(text.contains("\ndxvk  1.10.3-20230507"), "{text}");
    assert!(text.contains("import-gptk"), "{text}");

    let entries = home.json(&["runtime", "available"]);
    let entries = entries.as_array().unwrap();
    let kinds: Vec<&str> = entries
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["wine", "dxmt", "dxvk"]);
    assert_eq!(entries[0]["version"], "winecx-gptk-4.7.3");
    assert_eq!(entries[0]["size"], 461_131_598);
    assert_eq!(entries[0]["recommended"], true);
    assert_eq!(entries[0]["installed"], false);
}

#[test]
fn runtime_list_shows_installed_components() {
    let home = Home::new();
    assert_eq!(
        home.stdout(&["runtime", "list"]),
        "No components installed. Run `uncork runtime install` (or `uncork setup`).\n"
    );
    assert_eq!(home.json(&["runtime", "list"]), serde_json::json!([]));

    home.install_fake_wine();
    home.install_component("dxmt", "0.80", &[], &[]);
    let text = home.stdout(&["runtime", "list"]);
    assert!(
        text.starts_with("KIND  VERSION  LICENSE  SOURCE  FEATURES\n"),
        "{text}"
    );
    assert!(
        text.contains("wine  fake-1   MIT      local   wow64,msync,dxmt\n"),
        "{text}"
    );
    assert!(text.contains("dxmt  0.80     MIT      local\n"), "{text}");

    let list = home.json(&["runtime", "list"]);
    assert_eq!(list[0]["kind"], "wine");
    assert_eq!(list[0]["version"], "fake-1");
    assert_eq!(list[0]["source"], "local");
    assert_eq!(list[1]["kind"], "dxmt");

    let available = home.json(&["runtime", "available"]);
    assert_eq!(available[1]["installed"], true, "dxmt 0.80 is installed");
}

#[test]
fn runtime_install_needs_confirmation_without_a_terminal() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["runtime", "install", "wine"]);
    assert!(stderr.contains("needs your confirmation"), "{stderr}");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(
        stderr.contains("461.1 MB (wine winecx-gptk-4.7.3)"),
        "{stderr}"
    );
    assert!(
        !home.root().join("cache").exists(),
        "nothing was downloaded"
    );
}

#[test]
fn runtime_install_rejects_bad_requests_before_downloading() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["runtime", "install", "vulkan", "-y"]);
    assert!(
        stderr.contains("unknown component kind \"vulkan\""),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure(&["runtime", "install", "d3dmetal", "-y"]);
    assert!(stderr.contains("uncork runtime import-gptk"), "{stderr}");
    let stderr = home.stderr_of_failure(&["runtime", "install", "dxmt", "--version", "9.9", "-y"]);
    assert!(
        stderr.contains("the catalog has no dxmt 9.9; available: 0.80"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure(&["runtime", "install", "--version", "0.80", "-y"]);
    assert!(stderr.contains("exactly one component kind"), "{stderr}");
    assert!(!home.root().join("cache").exists());
}

#[test]
fn runtime_install_skips_what_is_installed() {
    let home = Home::new();
    home.install_component("dxmt", "0.80", &[], &[]);
    home.uncork()
        .args(["runtime", "install", "dxmt"])
        .assert()
        .success()
        .stdout("dxmt 0.80 is already installed.\n");
}

#[test]
fn runtime_remove_deletes_a_component() {
    let home = Home::new();
    let dir = home.install_component("dxvk", "1.10.3-20230507", &[], &[]);
    home.uncork()
        .args(["runtime", "remove", "dxvk", "1.10.3-20230507"])
        .assert()
        .success()
        .stdout("Removed dxvk 1.10.3-20230507.\n");
    assert!(!dir.exists());
    let stderr = home.stderr_of_failure(&["runtime", "remove", "dxvk", "1.10.3-20230507"]);
    assert!(stderr.contains("not found"), "{stderr}");
}

#[test]
fn runtime_remove_warns_about_bottles_left_without_wine() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("steam", support::FAKE_WINE);
    home.uncork()
        .args(["runtime", "remove", "wine", support::FAKE_WINE])
        .assert()
        .success()
        .stderr(contains("warning: bottle steam uses Wine fake-1"));
}

#[test]
fn import_gptk_needs_a_framework() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["runtime", "import-gptk", home.path().to_str().unwrap()]);
    assert!(stderr.contains("D3DMetal.framework"), "{stderr}");
}

#[test]
fn profile_list_includes_the_built_in_profiles() {
    let home = Home::new();
    let text = home.stdout(&["profile", "list"]);
    assert!(text.starts_with("ID "), "{text}");
    assert!(
        text.contains("rise-of-nations-extended-edition     Rise of Nations: Extended Edition      287450  untested"),
        "{text}"
    );
    assert!(
        text.contains("age-of-empires-2-definitive-edition"),
        "{text}"
    );

    let list = home.json(&["profile", "list"]);
    let ids: Vec<&str> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|profile| profile["id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"rise-of-nations-extended-edition"), "{ids:?}");
    assert!(
        ids.contains(&"age-of-empires-2-definitive-edition"),
        "{ids:?}"
    );
}

#[test]
fn profile_show_by_app_id_as_json() {
    let home = Home::new();
    let profile = home.json(&["profile", "show", "287450"]);
    assert_eq!(profile["id"], "rise-of-nations-extended-edition");
    assert_eq!(profile["steam"]["appid"], 287_450);
    assert_eq!(profile["exe"]["path"], "riseofnations.exe");
    assert_eq!(profile["exe"]["bitness"], "x86");
    assert_eq!(profile["graphics"]["backend"], "dxmt");
    assert_eq!(profile["dll_overrides"]["d3dcompiler_47"], "n,b");
}

#[test]
fn profile_show_as_text_and_unknown_profiles() {
    let home = Home::new();
    let text = home.stdout(&["profile", "show", "age of empires"]);
    assert!(
        text.starts_with("Name          Age of Empires II"),
        "{text}"
    );
    assert!(text.contains("AoE2DE_s.exe (64-bit, D3D11)"), "{text}");
    assert!(
        text.contains("dxmt, then d3dmetal, then dxvk, then wined3d"),
        "{text}"
    );
    let stderr = home.stderr_of_failure(&["profile", "show", "half-life"]);
    assert!(
        stderr.contains("no game profile matches \"half-life\""),
        "{stderr}"
    );
}

#[test]
fn user_profiles_override_and_invalid_ones_warn() {
    let home = Home::new();
    let profiles = home.root().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("my-game.toml"),
        "schema = 1\nid = \"my-game\"\nname = \"My Game\"\n[exe]\npath = \"my.exe\"\n",
    )
    .unwrap();
    std::fs::write(
        profiles.join("broken.toml"),
        "schema = 1\nid = \"broken\"\n",
    )
    .unwrap();
    home.uncork()
        .args(["profile", "list"])
        .assert()
        .success()
        .stdout(contains("my-game"))
        .stderr(contains("warning: skipping a profile"))
        .stderr(contains("broken.toml"));
}

#[test]
fn bottle_list_is_empty_on_a_new_home() {
    let home = Home::new();
    assert_eq!(
        home.stdout(&["bottle", "list"]),
        "No bottles yet. Run `uncork setup`, or `uncork bottle create <name>`.\n"
    );
    assert_eq!(home.json(&["bottle", "list"]), serde_json::json!([]));
}

#[test]
fn inspect_reports_bitness_api_and_backend() {
    let home = Home::new();
    let exe = d3d11_game(home.path());
    let exe_arg = exe.to_str().unwrap();

    // Nothing installed: DXMT is not available, so WineD3D.
    let scan = home.json(&["inspect", exe_arg]);
    assert_eq!(scan["bitness"], "x86");
    assert_eq!(scan["large_address_aware"], false);
    assert_eq!(scan["nx_compat"], true);
    assert_eq!(scan["apis"], serde_json::json!(["d3d11"]));
    assert_eq!(scan["primary_api"], "d3d11");
    assert_eq!(scan["uses_steamworks"], true);
    assert_eq!(scan["availability"]["dxmt"], false);
    assert_eq!(scan["recommendation"]["backend"], "wined3d");
    let evidence = &scan["evidence"][0];
    assert_eq!(evidence["file"], "game.exe");
    assert_eq!(evidence["import"], "d3d11.dll");

    // With DXMT installed (and the catalog Wine's features), DXMT.
    home.install_component("dxmt", "0.80", &[], &[]);
    let scan = home.json(&["inspect", exe_arg]);
    assert_eq!(scan["availability"]["dxmt"], true);
    assert_eq!(scan["recommendation"]["backend"], "dxmt");

    let text = home.stdout(&["inspect", exe_arg]);
    assert!(
        text.contains("Architecture    32-bit (PE32, x86)\n"),
        "{text}"
    );
    assert!(
        text.contains("D3D11: d3d11.dll imported by game.exe"),
        "{text}"
    );
    assert!(text.contains("Large address   no: 2 GiB"), "{text}");
    assert!(text.contains("Steamworks      yes"), "{text}");
    assert!(text.contains("Backend         dxmt\n"), "{text}");
}

#[test]
fn inspect_judges_d3dmetal_by_what_the_installed_wine_can_load() {
    let home = Home::new();
    // The catalog's CrossOver-derived Wine: `d3dmetal`, but no per-process
    // DLL path, so D3DMetal cannot be activated.
    home.install_fake_wine();
    let wine_meta = home.root().join("components/wine/fake-1/component.toml");
    let meta = std::fs::read_to_string(&wine_meta).unwrap();
    std::fs::write(
        &wine_meta,
        meta.replace("\"dxmt\"", "\"dxmt\", \"d3dmetal\""),
    )
    .unwrap();
    home.install_component("d3dmetal", "3.0", &[], &[]);
    home.install_component("dxvk", "1.10.3", &[], &[]);
    let dir = home.path().join("d3d12");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("game.exe");
    PeBuilder::pe64().import("d3d12.dll").write(&exe);

    let scan = home.json(&["inspect", exe.to_str().unwrap()]);
    assert_eq!(scan["availability"]["d3dmetal"], false);
    assert_eq!(scan["recommendation"]["backend"], "wined3d");
    let reason = scan["unavailable"]["d3dmetal"].as_str().unwrap();
    assert!(reason.contains("can load it per process"), "{reason}");

    let text = home.stdout(&["inspect", exe.to_str().unwrap()]);
    assert!(
        text.contains(
            "Unavailable     d3dmetal: D3DMetal needs a Wine runtime that can load it per process"
        ),
        "{text}"
    );
}

#[test]
fn inspect_explains_unreadable_files() {
    let home = Home::new();
    let not_pe = home.path().join("readme.exe");
    std::fs::write(&not_pe, "hello").unwrap();
    let stderr = home.stderr_of_failure(&["inspect", not_pe.to_str().unwrap()]);
    assert!(stderr.starts_with("error: cannot inspect "), "{stderr}");
    assert!(stderr.contains("not a PE image"), "{stderr}");
}

#[test]
fn bad_env_values_are_rejected() {
    let home = Home::new();
    for args in [
        &["run", "game.exe", "--env", "NOEQUALS"][..],
        &["play", "rise", "-e", "1BAD=x"],
        &["steam", "launch", "287450", "-e", "A-B=1"],
        &["steam", "start", "-e", "=x"],
    ] {
        let stderr = home.stderr_of_failure(args);
        assert!(
            stderr.starts_with("error: invalid --env"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn play_needs_a_known_game_and_a_bottle() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["play", "half-life"]);
    assert!(
        stderr.contains("no game profile matches \"half-life\""),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure(&["play", "rise", "--dry-run"]);
    assert!(
        stderr.contains("the default bottle \"steam\" does not exist yet; run `uncork setup`"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure(&["play", "12345", "--bottle", "nope"]);
    assert!(stderr.contains("bottle \"nope\" not found"), "{stderr}");
}

#[test]
fn setup_asks_before_downloading() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["setup"]);
    // Either Rosetta is missing on this host (setup stops first), or the
    // download needs confirmation; never a download.
    assert!(
        stderr.contains("--yes") || stderr.contains("Rosetta 2 is not installed"),
        "{stderr}"
    );
    assert!(!home.root().join("cache").exists());
    assert!(!home.root().join("bottles").exists());
}

#[test]
fn setup_rejects_invalid_bottle_names() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["setup", "--bottle", "../x"]);
    assert!(stderr.contains("invalid bottle name"), "{stderr}");
}

#[test]
fn winetricks_missing_says_how_to_install_it() {
    let home = Home::new();
    let empty = home.path().join("empty-path");
    std::fs::create_dir_all(&empty).unwrap();
    if ["/opt/homebrew/bin/winetricks", "/usr/local/bin/winetricks"]
        .iter()
        .any(|path| Path::new(path).exists())
    {
        eprintln!("skipped: winetricks is installed in a Homebrew directory on this machine");
        return;
    }
    let output = home
        .uncork()
        .env("PATH", &empty)
        .args(["winetricks", "corefonts"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("winetricks is not installed"), "{stderr}");
    assert!(stderr.contains("brew install winetricks"), "{stderr}");
}

#[test]
fn errors_print_their_causes() {
    let home = Home::new();
    std::fs::create_dir_all(home.root()).unwrap();
    std::fs::write(home.root().join("config.toml"), "schema = 1\ncolour = 2\n").unwrap();
    let stderr = home.stderr_of_failure(&["bottle", "list"]);
    assert!(stderr.starts_with("error: invalid config "), "{stderr}");
    assert!(stderr.contains("colour"), "{stderr}");
}
