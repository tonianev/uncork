//! The `uncork` binary on an empty or lightly populated `UNCORK_HOME`:
//! help, doctor, runtime, profile, inspect and argument errors.

mod common;
mod support;

use std::path::{Path, PathBuf};

use common::PeBuilder;
use predicates::prelude::*;
use predicates::str::contains;
use support::{FAKE_WINE, Home};

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
        .stdout(contains("completions"))
        .stdout(contains("__exec").not())
        .stdout(contains("__manpage").not());
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

/// What the release build runs to fill `completions/` and `man/man1/`.
#[test]
fn completions_and_man_pages_come_from_the_binary() {
    let home = Home::new();
    home.uncork()
        .args(["completions", "zsh"])
        .assert()
        .success()
        .stdout(contains("#compdef uncork"))
        .stdout(contains("'play:"))
        .stdout(contains("'doctor:"));
    home.uncork()
        .args(["completions", "bash"])
        .assert()
        .success()
        .stdout(contains("complete -F _uncork"));
    home.uncork()
        .args(["completions", "tcsh"])
        .assert()
        .code(2)
        .stderr(contains("possible values"));

    let man1 = home.path().join("man").join("man1");
    home.uncork()
        .arg("__manpage")
        .arg(&man1)
        .assert()
        .success()
        .stdout(predicates::str::is_empty())
        .stderr(contains("man pages"));
    for page in ["uncork.1", "uncork-play.1", "uncork-bottle-create.1"] {
        assert!(man1.join(page).is_file(), "{page}");
    }
    // roff writes a hyphen as `\-`: `--json` is `\-\-json` in the source.
    let root = std::fs::read_to_string(man1.join("uncork.1")).unwrap();
    assert!(root.contains(r"\-\-json"), "{root}");
    assert!(
        !home.root().exists(),
        "completions and man pages touch no data directory"
    );
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
    let stderr =
        home.stderr_of_failure_with(3, &["runtime", "install", "dxmt", "--version", "9.9", "-y"]);
    assert!(
        stderr.contains("catalog entry \"dxmt 9.9\" not found; available: 0.80"),
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
    let stderr = home.stderr_of_failure_with(3, &["runtime", "remove", "dxvk", "1.10.3-20230507"]);
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
    let stderr = home.stderr_of_failure_with(3, &["profile", "show", "half-life"]);
    assert!(
        stderr.contains("game profile \"half-life\" not found; `uncork profile list`"),
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
fn a_steam_app_with_two_profiles_names_both_instead_of_guessing() {
    let home = Home::new();
    let profiles = home.root().join("profiles");
    std::fs::create_dir_all(&profiles).unwrap();
    std::fs::write(
        profiles.join("ron-wined3d.toml"),
        "schema = 1\nid = \"ron-wined3d\"\nname = \"RoN on WineD3D\"\n[steam]\nappid = 287450\n[exe]\npath = \"riseofnations.exe\"\n[graphics]\nbackend = \"wined3d\"\n",
    )
    .unwrap();
    for args in [
        &["play", "287450", "--dry-run"][..],
        &["steam", "launch", "287450", "--dry-run"],
    ] {
        let stderr = home.stderr_of_failure(args);
        assert!(
            stderr.contains("rise-of-nations-extended-edition, ron-wined3d"),
            "{args:?}: {stderr}"
        );
    }
    let stderr = home.stderr_of_failure(&["profile", "show", "287450"]);
    assert!(stderr.contains("matches several game profiles"), "{stderr}");
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
    let stderr = home.stderr_of_failure_with(4, &["inspect", not_pe.to_str().unwrap()]);
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
    let stderr = home.stderr_of_failure_with(3, &["play", "half-life"]);
    assert!(
        stderr.contains("game profile \"half-life\" not found; `uncork profile list`"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure_with(3, &["play", "rise", "--dry-run"]);
    assert!(
        stderr.contains("default bottle \"steam\" not found; run `uncork setup`"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure_with(3, &["play", "12345", "--bottle", "nope"]);
    assert!(stderr.contains("bottle \"nope\" not found"), "{stderr}");
}

#[test]
fn setup_asks_before_downloading() {
    let home = Home::new();
    let output = home.uncork().arg("setup").output().unwrap();
    let code = output.status.code();
    let stderr = String::from_utf8(output.stderr).unwrap();
    // Either Rosetta is missing on this host (setup stops first, a `host`
    // error), or the download needs confirmation; never a download.
    assert!(
        (code == Some(1) && stderr.contains("--yes"))
            || (code == Some(7) && stderr.contains("Rosetta 2 is not installed")),
        "{code:?}: {stderr}"
    );
    assert!(!home.root().join("cache").exists());
    assert!(!home.root().join("bottles").exists());
}

#[test]
fn setup_rejects_invalid_bottle_names() {
    let home = Home::new();
    let stderr = home.stderr_of_failure_with(4, &["setup", "--bottle", "../x"]);
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
    let stderr = home.stderr_of_failure_with(4, &["bottle", "list"]);
    assert!(stderr.starts_with("error: invalid config "), "{stderr}");
    assert!(stderr.contains("colour"), "{stderr}");
}

#[test]
fn json_errors_are_one_object_with_a_kind_and_an_exit_code() {
    let home = Home::new();
    let error = home.json_failure(3, &["profile", "show", "does-not-exist"]);
    assert_eq!(error["error"]["kind"], "not-found");
    assert_eq!(error["error"]["exit_code"], 3);
    assert_eq!(error["error"]["causes"], serde_json::json!([]));
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("game profile \"does-not-exist\" not found"),
        "{message}"
    );

    // A command with a JSON form prints nothing before the error object.
    let error = home.json_failure(3, &["play", "rise", "--dry-run"]);
    assert_eq!(error["error"]["kind"], "not-found");
    let message = error["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("default bottle \"steam\" not found"),
        "{message}"
    );
}

#[test]
fn json_errors_carry_the_causes_the_text_form_prints() {
    let home = Home::new();
    std::fs::create_dir_all(home.root()).unwrap();
    std::fs::write(home.root().join("config.toml"), "schema = 1\ncolour = 2\n").unwrap();
    let error = home.json_failure(4, &["bottle", "list"]);
    assert_eq!(error["error"]["kind"], "invalid-config");
    assert_eq!(error["error"]["exit_code"], 4);
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("invalid config "), "{message}");
    assert!(message.contains("colour"), "{message}");
}

#[test]
fn text_errors_use_the_same_exit_codes_and_keep_stdout_empty() {
    let home = Home::new();
    home.uncork()
        .args(["profile", "show", "does-not-exist"])
        .assert()
        .code(3)
        .stdout("")
        .stderr(contains("error: game profile \"does-not-exist\" not found"));
}

#[test]
fn the_binary_s_own_errors_are_other_with_code_1() {
    let home = Home::new();
    let error = home.json_failure(1, &["play", "rise", "-e", "1BAD=x"]);
    assert_eq!(error["error"]["kind"], "other");
    assert_eq!(error["error"]["exit_code"], 1);
    let message = error["error"]["message"].as_str().unwrap();
    assert!(message.starts_with("invalid --env"), "{message}");
}

#[test]
fn usage_errors_stay_text_with_code_2_even_with_json() {
    let home = Home::new();
    home.uncork()
        .args(["--json", "frobnicate"])
        .assert()
        .code(2)
        .stdout("")
        .stderr(contains("unrecognized subcommand"));
}

#[test]
fn profile_validate_names_each_problem_with_its_line_and_fails() {
    let home = Home::new();
    let dir = home.path().join("profiles");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("good.toml"),
        "schema = 1\nid = \"good\"\nname = \"Good\"\n[exe]\npath = \"good.exe\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("bad.toml"),
        "schema = 1\nid = \"bad\"\nname = \"Bad\"\n\n[steam]\nappid = 0\n\n[exe]\npath = \"C:/bad.exe\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("broken.toml"),
        "schema = 1\nid = \"broken\"\nname = \"B\"\n[exe]\npath = \"b.exe\"\n[graphics]\nbackend = \"metal\"\n",
    )
    .unwrap();
    let good = dir.join("good.toml");
    let bad = dir.join("bad.toml");
    let broken = dir.join("broken.toml");

    home.uncork()
        .args(["profile", "validate"])
        .arg(&good)
        .assert()
        .success()
        .stdout(format!("{}: ok\n1 profile valid\n", good.display()));

    let output = home
        .uncork()
        .args(["profile", "validate"])
        .arg(&dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(4), "invalid-config");
    let stdout = String::from_utf8(output.stdout).unwrap();
    for expected in [
        format!("{}:6: steam.appid is 0", bad.display()),
        format!("{}:9: exe.path \"C:/bad.exe\" is absolute", bad.display()),
        format!("{}:7: unknown variant `metal`", broken.display()),
        format!("{}: ok\n", good.display()),
    ] {
        assert!(stdout.contains(&expected), "{expected:?} not in {stdout}");
    }
    assert!(
        stdout.ends_with("3 problems in 2 of 3 profiles\n"),
        "{stdout}"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("error: invalid profile ") && stderr.contains("3 problems in 2 files"),
        "{stderr}"
    );

    // With --json, stdout is one object: the files and the error.
    let json = home.json_failure(4, &["profile", "validate", bad.to_str().unwrap()]);
    assert_eq!(json["profiles"][0]["id"], "bad");
    assert_eq!(json["profiles"][0]["problems"][0]["line"], 6);
    assert_eq!(json["profiles"][0]["problems"][0]["key"], "steam.appid");
    assert_eq!(json["profiles"][0]["problems"][1]["key"], "exe.path");
    assert_eq!(json["error"]["kind"], "invalid-config");
    assert_eq!(json["error"]["exit_code"], 4);
    let json = home.json(&["profile", "validate", good.to_str().unwrap()]);
    assert_eq!(json["profiles"][0]["problems"], serde_json::json!([]));
    assert_eq!(json["error"], serde_json::Value::Null);

    let stderr = home.stderr_of_failure_with(1, &["profile", "validate", "/nonexistent/x.toml"]);
    assert!(
        stderr.contains("cannot read /nonexistent/x.toml"),
        "{stderr}"
    );
    let empty = home.path().join("empty");
    std::fs::create_dir_all(&empty).unwrap();
    let output = home
        .uncork()
        .args(["profile", "validate"])
        .arg(&empty)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3), "not-found");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has no .toml file"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn profile_new_scaffolds_a_valid_profile_from_an_executable() {
    let home = Home::new();
    let exe = d3d11_game(home.path());
    let output = home
        .uncork()
        .args(["profile", "new"])
        .arg(&exe)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "# game.\n",
        "read from game.exe and the DLLs beside it:\n# 32-bit (PE32), not large-address-aware, NX-compatible; D3D11 (d3d11.dll\n# imported by game.exe); Steamworks; no anti-cheat files.",
        "schema = 1\nid = \"game\"\nname = \"game\"\n",
        "# [steam]\n# appid = 123456",
        "[exe]\npath = \"game.exe\"\nbitness = \"x86\"\napi = \"d3d11\"\n# geometry_shaders = true",
        "[graphics]\n# 32-bit D3D11: DXMT (D3DMetal is 64-bit only)\nbackend = \"dxmt\"\nfallbacks = [\"wined3d\", \"dxvk\"]\n",
        "# [env]\n# WINE_LARGE_ADDRESS_AWARE = \"1\"",
        "# [dll_overrides]\n",
        "# [[ini]]\n",
        "[compat]\nstatus = \"untested\"\nnotes = \"\"\n",
    ] {
        assert!(text.contains(expected), "{expected:?} not in:\n{text}");
    }
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with("Next: save this as profiles/game.toml"),
        "{stderr}"
    );
    assert!(
        stderr.contains(&format!("`uncork run {}`", exe.display())),
        "{stderr}"
    );
    assert!(stderr.contains("docs/COMPATIBILITY.md"), "{stderr}");

    // The scaffold validates under the name the hint gives it.
    let file = home.path().join("game.toml");
    std::fs::write(&file, &text).unwrap();
    home.uncork()
        .args(["profile", "validate"])
        .arg(&file)
        .assert()
        .success()
        .stdout(contains("1 profile valid"));

    let stderr = home.stderr_of_failure_with(1, &["profile", "new", "/nonexistent/game.exe"]);
    assert!(stderr.contains("does not exist"), "{stderr}");
}

#[test]
fn profile_new_finds_a_steam_game_the_way_play_does() {
    let home = Home::new();
    home.write_bottle("steam", FAKE_WINE);
    let steam = home.install_fake_steam("steam", &[(70, "Half-Life: Source", 4)]);
    let dir = steam.join("steamapps/common/Half-Life Source");
    PeBuilder::pe64()
        .import("d3d12.dll")
        .data(&[0; 4096])
        .write(&dir.join("hl.exe"));
    PeBuilder::pe64()
        .import("kernel32.dll")
        .data(&[0; 65536])
        .write(&dir.join("unins000.exe"));
    let out = home.path().join("half-life-source.toml");

    let output = home
        .uncork()
        .args(["profile", "new", "70", "--out"])
        .arg(&out)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty(), "--out writes nothing to stdout");
    let text = std::fs::read_to_string(&out).unwrap();
    for expected in [
        "# Half-Life: Source (Steam app 70, build 14787288).\n",
        "id = \"half-life-source\"\nname = \"Half-Life: Source\"\n\n[steam]\nappid = 70\n",
        "# 64-bit (PE32+), large-address-aware, NX-compatible; D3D12 (d3d12.dll\n# imported by hl.exe); no Steamworks; no anti-cheat files.",
        "[exe]\npath = \"hl.exe\"\nbitness = \"x64\"\napi = \"d3d12\"\n",
        "[graphics]\n# 64-bit D3D12: D3DMetal (only D3DMetal runs D3D12)\nbackend = \"d3dmetal\"\nfallbacks = []\n",
        "# [env]\n# DXMT_CONFIG",
    ] {
        assert!(text.contains(expected), "{expected:?} not in:\n{text}");
    }
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.starts_with(&format!("Wrote {}.\n", out.display())),
        "{stderr}"
    );
    assert!(
        stderr.contains("`uncork play half-life-source`"),
        "{stderr}"
    );

    let stderr =
        home.stderr_of_failure_with(1, &["profile", "new", "70", "--out", out.to_str().unwrap()]);
    assert!(stderr.contains("already exists"), "{stderr}");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), text, "left alone");

    let json = home.json(&["profile", "new", "70", "--id", "hl"]);
    assert_eq!(json["id"], "hl");
    assert_eq!(json["appid"], 70);
    assert_eq!(json["exe"], dir.join("hl.exe").to_str().unwrap());
    assert!(
        json["toml"].as_str().unwrap().contains("id = \"hl\"\n"),
        "{json}"
    );

    let stderr = home.stderr_of_failure_with(1, &["profile", "new", "70", "--id", "Half Life"]);
    assert!(
        stderr.contains("would not validate") && stderr.contains("kebab-case"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure_with(3, &["profile", "new", "71"]);
    assert!(
        stderr.contains("Steam app \"71\" not found; install it from the Steam library first"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure_with(3, &["profile", "new", "70", "--bottle", "other"]);
    assert!(stderr.contains("bottle \"other\" not found"), "{stderr}");
}

#[test]
fn profile_schema_prints_the_checked_in_schema() {
    let home = Home::new();
    let text = home.stdout(&["profile", "schema"]);
    assert_eq!(text, include_str!("../../../profiles/schema.json"));
    let json: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(json["title"], "Uncork game profile");
    assert_eq!(json["required"][0], "schema");
}
