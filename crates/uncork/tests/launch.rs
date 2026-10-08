//! Launch commands (`run`, `play`, `steam`, `bottle env`/`tool`,
//! `winetricks`) with a fake Wine, and `__exec` / the Game Mode bundle
//! launcher.
//!
//! Tests marked `needs phase 2 core` call `uncork_core::launch`, `steam` or
//! `gamemode` functions whose bodies land in phase 2; enable them once those
//! are merged.

mod common;
mod support;

use std::fs;
use std::path::{Path, PathBuf};

use common::PeBuilder;
use predicates::prelude::*;
use predicates::str::contains;
use support::{FAKE_WINE, Home, write_script};
use uncork_core::process::CommandSpec;

/// A 32-bit D3D11 game inside the bottle's `drive_c/Games/Test`.
fn game_in_bottle(home: &Home, bottle: &str) -> PathBuf {
    let dir = home.bottle(bottle).join("drive_c/Games/Test");
    fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("game.exe");
    PeBuilder::pe32().import("d3d11.dll").write(&exe);
    exe
}

/// A bottle `test1` made by `uncork bottle create` with the fake Wine.
fn home_with_bottle() -> Home {
    let home = Home::new();
    home.install_fake_wine();
    home.uncork()
        .args(["bottle", "create", "test1"])
        .assert()
        .success();
    home
}

/// A fake DXMT component with the files activation needs.
fn install_fake_dxmt(home: &Home) {
    home.install_component(
        "dxmt",
        "0.80",
        &[],
        &[
            "x86_64-windows/d3d11.dll",
            "x86_64-windows/dxgi.dll",
            "x86_64-windows/d3d10core.dll",
            "x86_64-windows/winemetal.dll",
            "i386-windows/d3d11.dll",
            "i386-windows/dxgi.dll",
            "i386-windows/d3d10core.dll",
            "i386-windows/winemetal.dll",
            "x86_64-unix/winemetal.so",
        ],
    );
}

#[test]
fn run_checks_the_executable_before_planning() {
    let home = Home::new();
    home.write_bottle("b", FAKE_WINE);
    let stderr = home.stderr_of_failure(&["run", "-b", "b", "missing.exe", "--dry-run"]);
    assert!(stderr.contains("missing.exe does not exist"), "{stderr}");
    let stderr = home.stderr_of_failure(&["run", "-b", "b", r"C:\Games\none.exe", "--dry-run"]);
    assert!(
        stderr.contains("drive_c/Games/none.exe does not exist"),
        "{stderr}"
    );
    let stderr = home.stderr_of_failure(&["run", "-b", "b", r"C:\..\x.exe", "--dry-run"]);
    assert!(stderr.contains("cannot map"), "{stderr}");
}

#[test]
fn run_needs_the_bottles_wine() {
    let home = Home::new();
    home.write_bottle("b", "11.0-gone");
    let exe = game_in_bottle(&home, "b");
    let stderr = home.stderr_of_failure(&["run", "-b", "b", exe.to_str().unwrap(), "--dry-run"]);
    assert!(stderr.contains("bottle b needs Wine 11.0-gone"), "{stderr}");
}

#[test]
fn run_dry_run_shows_the_prefix_and_backend() {
    let home = home_with_bottle();
    install_fake_dxmt(&home);
    let exe = game_in_bottle(&home, "test1");
    let text = home.stdout(&[
        "run",
        "--bottle",
        "test1",
        exe.to_str().unwrap(),
        "--dry-run",
        "-e",
        "EXTRA=1",
        "--",
        "-windowed",
    ]);
    assert!(text.starts_with("Backend: dxmt 0.80"), "{text}");
    assert!(text.contains("Reason:  32-bit D3D11: DXMT"), "{text}");
    assert!(
        text.contains(&format!("WINEPREFIX='{}'", home.bottle("test1").display())),
        "{text}"
    );
    assert!(text.contains("EXTRA='1'"), "{text}");
    assert!(text.contains("'-windowed'"), "{text}");
    assert!(
        !home
            .bottle("test1")
            .join("drive_c/windows/syswow64/d3d11.dll")
            .exists(),
        "a dry run copies nothing"
    );

    // The same launch by Windows path, as JSON.
    let plan = home.json(&[
        "run",
        "-b",
        "test1",
        r"C:\Games\Test\game.exe",
        "--dry-run",
        "--backend",
        "wined3d",
    ]);
    assert_eq!(plan["plan"]["activation"]["backend"], "wined3d");
    assert_eq!(
        plan["plan"]["command"]["env"]["WINEPREFIX"],
        home.bottle("test1").to_str().unwrap()
    );
    assert_eq!(
        plan["plan"]["command"]["args"],
        serde_json::json!([exe.to_str().unwrap()]),
        "arguments are plain strings"
    );
}

#[test]
fn run_starts_the_program_and_waits() {
    let home = home_with_bottle();
    let exe = game_in_bottle(&home, "test1");
    home.uncork()
        .args(["run", "-b", "test1", "--wait", "--backend", "wined3d"])
        .arg(&exe)
        .assert()
        .success()
        .stdout(contains("Started game.exe on wined3d (pid "))
        .stdout(contains("Log: "))
        .stdout(contains("game.exe exited"));
    let calls = home.calls();
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("wine ") && call.contains("game.exe")),
        "{calls:#?}"
    );
}

#[test]
fn run_prints_the_plans_warnings() {
    let home = home_with_bottle();
    let exe = game_in_bottle(&home, "test1");
    home.uncork()
        .args(["run", "-b", "test1", "--retina", "--backend", "wined3d"])
        .arg(&exe)
        .assert()
        .success()
        .stdout(contains("Started game.exe on wined3d (pid "))
        .stderr(contains(
            "warning: Retina mode on was asked for, but it is a bottle-wide registry setting",
        ))
        .stderr(contains("performance.retina=true"));
}

#[test]
fn play_prints_the_plans_warnings() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    // StateFlags 2: Steam has not finished installing it.
    let steam = home.install_fake_steam("b", &[(70, "Half-Life", 2)]);
    PeBuilder::pe32().write(&steam.join("steamapps/common/Half-Life/hl.exe"));
    home.fake_running_steam();
    home.uncork()
        .args(["play", "70", "-b", "b", "--backend", "wined3d"])
        .assert()
        .success()
        .stderr(contains(
            "warning: Steam does not list Half-Life as fully installed",
        ));
}

#[test]
fn play_dry_run_shows_backend_ini_and_command() {
    let home = Home::new();
    home.install_fake_wine();
    install_fake_dxmt(&home);
    home.uncork()
        .args(["bottle", "create", "steam"])
        .assert()
        .success();
    let steam = home.install_fake_steam("steam", &[(287_450, "Rise of Nations", 4)]);
    PeBuilder::pe32()
        .import("d3d11.dll")
        .write(&steam.join("steamapps/common/Rise of Nations/riseofnations.exe"));
    let text = home.stdout(&["play", "rise", "--dry-run"]);
    assert!(text.starts_with("Backend: dxmt"), "{text}");
    assert!(text.contains("INI edits:"), "{text}");
    assert!(text.contains("SkipIntroMovies=1"), "{text}");
    assert!(text.contains("WINE_LARGE_ADDRESS_AWARE='1'"), "{text}");
    assert!(text.contains("riseofnations.exe"), "{text}");

    let by_appid = home.stdout(&["play", "287450", "--dry-run"]);
    assert!(by_appid.starts_with("Backend: dxmt"), "{by_appid}");
    let launched = home.stdout(&["steam", "launch", "287450", "--dry-run"]);
    assert!(launched.starts_with("Backend: dxmt"), "{launched}");
}

#[test]
fn play_wait_returns_when_the_game_exits_while_steam_keeps_running() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let steam = home.install_fake_steam("b", &[(70, "Half-Life", 4)]);
    PeBuilder::pe32().write(&steam.join("steamapps/common/Half-Life/hl.exe"));
    home.fake_running_steam();
    let text = home.stdout(&["play", "70", "-b", "b", "--wait", "--backend", "wined3d"]);
    assert!(text.contains("Started hl.exe on wined3d (pid "), "{text}");
    assert!(text.contains("hl.exe exited"), "{text}");
    let calls = home.calls();
    assert!(
        calls.iter().any(|call| call.contains("hl.exe")),
        "{calls:#?}"
    );
    assert!(
        !calls
            .iter()
            .any(|call| call.starts_with("wineserver --wait")),
        "Steam keeps the wineserver running; waiting for it would never end: {calls:#?}"
    );
}

#[test]
fn play_reports_games_that_are_not_installed() {
    let home = home_with_bottle();
    home.install_fake_steam("test1", &[]);
    let stderr = home.stderr_of_failure(&["play", "rise", "-b", "test1", "--dry-run"]);
    assert!(stderr.contains("cannot play Rise of Nations"), "{stderr}");
}

#[test]
fn steam_start_dry_run_prints_the_client_command() {
    let home = home_with_bottle();
    home.install_fake_steam("test1", &[]);
    let text = home.stdout(&[
        "steam",
        "start",
        "-b",
        "test1",
        "--dry-run",
        "--wine-debug",
        "+loaddll",
    ]);
    assert!(text.contains("steam.exe"), "{text}");
    assert!(text.contains("-nofriendsui"), "{text}");
    assert!(text.contains("WINEDEBUG='+loaddll'"), "{text}");
    assert!(text.contains("d3d10core,d3d11=n,b"), "{text}");
}

#[test]
fn steam_start_starts_a_client_that_crashed() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    home.install_fake_steam("b", &[]);
    home.fake_crashed_steam();
    let text = home.stdout(&["steam", "start", "-b", "b"]);
    assert!(text.contains("Started Steam in bottle b (pid "), "{text}");
    assert!(!text.contains("already running"), "{text}");
}

#[test]
fn steam_start_gives_the_client_the_bottles_pinned_dxvk() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let steam = home.install_fake_steam("b", &[]);
    for version in ["1.10.3-20230507", "2.0"] {
        let dir = home.install_component("dxvk", version, &[], &[]);
        for dll in ["d3d11.dll", "d3d10core.dll"] {
            let path = dir.join("x86_64-windows").join(dll);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, format!("dxvk {version} {dll}")).unwrap();
        }
    }
    home.uncork()
        .args(["bottle", "set", "b", "graphics.dxvk=1.10.3-20230507"])
        .assert()
        .success();
    let text = home.stdout(&["steam", "start", "-b", "b"]);
    assert!(text.contains("Started Steam in bottle b (pid "), "{text}");
    assert_eq!(
        fs::read_to_string(steam.join("bin/cef/cef.win64/d3d11.dll")).unwrap(),
        "dxvk 1.10.3-20230507 d3d11.dll",
        "the pinned DXVK, not the newest"
    );
}

#[test]
fn steam_start_needs_steam() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let stderr = home.stderr_of_failure(&["steam", "start", "-b", "b"]);
    assert!(
        stderr.contains("Steam is not installed in bottle b; run: uncork steam install --bottle b"),
        "{stderr}"
    );
    assert!(home.calls().is_empty(), "nothing ran");
}

#[test]
fn steam_install_asks_before_downloading() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let stderr = home.stderr_of_failure(&["steam", "install", "-b", "b"]);
    assert!(
        stderr.contains("Valve's Steam installer (2.4 MB"),
        "{stderr}"
    );
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(home.calls().is_empty(), "nothing ran");
}

#[test]
fn bottle_env_prints_exports() {
    let home = home_with_bottle();
    home.uncork()
        .args(["bottle", "set", "test1", "env.DXMT_LOG_LEVEL=info"])
        .assert()
        .success();
    let text = home.stdout(&["bottle", "env", "test1"]);
    assert!(text.starts_with("# Bottle test1"), "{text}");
    assert!(
        text.contains(&format!(
            "export WINEPREFIX='{}'\n",
            home.bottle("test1").display()
        )),
        "{text}"
    );
    assert!(text.contains("export WINE='"), "{text}");
    assert!(text.contains("export DXMT_LOG_LEVEL='info'\n"), "{text}");
    assert!(text.contains("export WINEMSYNC='1'\n"), "{text}");
    assert!(text.contains("/bin':\"$PATH\""), "{text}");
}

#[test]
fn bottle_tool_starts_a_wine_program() {
    let home = home_with_bottle();
    home.uncork()
        .args(["bottle", "tool", "test1", "winecfg"])
        .assert()
        .success()
        .stdout(contains("Started winecfg in bottle test1 (pid "));
}

#[test]
fn winetricks_runs_with_the_bottle_environment() {
    let home = home_with_bottle();
    let bin = home.path().join("bin");
    let record = home.path().join("winetricks.txt");
    write_script(
        &bin.join("winetricks"),
        &format!(
            "#!/bin/sh\nprintf '%s|%s|%s|%s\\n' \"$*\" \"$WINEPREFIX\" \"$WINE\" \"$PATH\" > '{}'\n",
            record.display()
        ),
    );
    let path = format!("{}:/usr/bin:/bin", bin.display());
    home.uncork()
        .env("PATH", &path)
        .args(["winetricks", "-b", "test1", "corefonts", "vcrun2022"])
        .assert()
        .success();
    let recorded = fs::read_to_string(&record).unwrap();
    let fields: Vec<&str> = recorded.trim_end().split('|').collect();
    assert_eq!(fields[0], "corefonts vcrun2022");
    assert_eq!(fields[1], home.bottle("test1").to_str().unwrap());
    assert!(fields[2].ends_with("/bin/wine"), "{recorded}");
    assert_eq!(fields[3], path, "the user's PATH is kept");
}

/// A command that writes `$GREETING` and its arguments to `out`.
fn greeting_plan(dir: &Path, out: &Path) -> CommandSpec {
    let mut spec = CommandSpec::new("/bin/sh")
        .arg("-c")
        .arg(format!(
            "printf '%s %s' \"$GREETING\" \"$1\" > '{}'",
            out.display()
        ))
        .arg("sh")
        .arg("world")
        .env("GREETING", "hello");
    spec.env_clear = true;
    spec.cwd = Some(dir.to_path_buf());
    spec
}

#[test]
fn exec_runs_a_saved_command_in_place() {
    let home = Home::new();
    let out = home.path().join("out.txt");
    let plan = home.path().join("plan.json");
    fs::write(
        &plan,
        serde_json::to_string(&greeting_plan(home.path(), &out)).unwrap(),
    )
    .unwrap();
    home.uncork().arg("__exec").arg(&plan).assert().success();
    assert_eq!(fs::read_to_string(&out).unwrap(), "hello world");
}

#[test]
fn exec_reports_bad_plans() {
    let home = Home::new();
    let plan = home.path().join("plan.json");
    fs::write(&plan, r#"{"program": "/nonexistent/wine"}"#).unwrap();
    home.uncork()
        .arg("__exec")
        .arg(&plan)
        .assert()
        .code(1)
        .stderr(contains("error: cannot start /nonexistent/wine"));
    fs::write(&plan, "{").unwrap();
    home.uncork()
        .arg("__exec")
        .arg(&plan)
        .assert()
        .code(1)
        .stderr(contains("invalid launch plan"));
    home.uncork()
        .arg("__exec")
        .arg(home.path().join("missing.json"))
        .assert()
        .code(1)
        .stderr(contains("cannot read launch plan"));
}

#[test]
fn a_game_mode_bundle_copy_runs_its_plan() {
    let home = Home::new();
    let bundle = home.path().join("Game.app/Contents");
    let launcher = bundle.join("MacOS/launch");
    fs::create_dir_all(launcher.parent().unwrap()).unwrap();
    fs::create_dir_all(bundle.join("Resources")).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_uncork"), &launcher).unwrap();
    let out = home.path().join("out.txt");
    fs::write(
        bundle.join("Resources/plan.json"),
        serde_json::json!({
            "command": serde_json::to_value(greeting_plan(home.path(), &out)).unwrap(),
            "backend_reason": "test",
        })
        .to_string(),
    )
    .unwrap();
    assert_cmd::Command::new(&launcher)
        .env("UNCORK_HOME", home.root())
        .assert()
        .success();
    assert_eq!(fs::read_to_string(&out).unwrap(), "hello world");

    // With other arguments the copy is just `uncork`.
    assert_cmd::Command::new(&launcher)
        .arg("--version")
        .assert()
        .success()
        .stdout(contains("uncork"))
        .stdout(contains("hello").not());
}
