//! `uncork bottle ...` and `uncork steam games` with a fake Wine component
//! whose loader and wineserver are shell scripts.

mod support;

use std::fs;

use predicates::prelude::*;
use predicates::str::contains;
use support::{FAKE_WINE, Home};

#[test]
fn bottle_lifecycle_with_a_fake_wine() {
    let home = Home::new();
    home.install_fake_wine();

    home.uncork()
        .args(["bottle", "create", "test1"])
        .assert()
        .success()
        .stdout(contains("Created bottle test1"))
        .stdout(contains("(Wine fake-1, win10)"));
    let prefix = home.bottle("test1");
    assert!(prefix.join("system.reg").is_file(), "wineboot ran");
    assert!(prefix.join("uncork.toml").is_file());
    let calls = home.calls();
    assert!(
        calls[0].starts_with("wine wineboot -u | WINEPREFIX="),
        "{calls:#?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call.starts_with("wineserver --wait")),
        "{calls:#?}"
    );
    assert!(
        calls.iter().any(|call| call.starts_with("wine regedit /S")),
        "{calls:#?}"
    );

    let list = home.stdout(&["bottle", "list"]);
    assert_eq!(
        list,
        "NAME   WINE    WINDOWS  BACKEND  STEAM\ntest1  fake-1  win10    auto     no\n"
    );

    let info = home.json(&["bottle", "info", "test1"]);
    assert_eq!(info["name"], "test1");
    assert_eq!(info["initialized"], true);
    assert_eq!(info["wine_installed"], true);
    assert_eq!(info["steam_installed"], false);
    assert_eq!(info["default"], false);
    assert_eq!(info["config"]["wine"], FAKE_WINE);
    assert_eq!(info["config"]["graphics"]["backend"], "auto");
    assert_eq!(info["config"]["performance"]["hud"], false);
    assert_eq!(info["state"]["prefix_wine"], FAKE_WINE);

    home.uncork()
        .args([
            "bottle",
            "set",
            "test1",
            "performance.hud=true",
            "graphics.backend=dxmt",
            "env.DXMT_LOG_LEVEL=info",
            "dll_overrides.d3dcompiler_47=native,builtin",
        ])
        .assert()
        .success()
        .stdout(contains("test1: performance.hud = true"))
        .stdout(contains("test1: graphics.backend = dxmt"))
        .stdout(contains("test1: dll_overrides.d3dcompiler_47 = \"n,b\""));
    let info = home.json(&["bottle", "info", "test1"]);
    assert_eq!(info["config"]["performance"]["hud"], true);
    assert_eq!(info["config"]["graphics"]["backend"], "dxmt");
    assert_eq!(info["config"]["env"]["DXMT_LOG_LEVEL"], "info");
    assert_eq!(info["config"]["dll_overrides"]["d3dcompiler_47"], "n,b");

    let text = home.stdout(&["bottle", "info", "test1"]);
    assert!(text.contains("Bottle         test1\n"), "{text}");
    assert!(
        text.contains("Initialized    yes (by Wine fake-1)\n"),
        "{text}"
    );
    assert!(text.contains("Graphics       backend dxmt;"), "{text}");
    assert!(text.contains("hud on"), "{text}");

    // A Retina change rewrites the Mac driver registry with regedit.
    let regedits = |home: &Home| {
        home.calls()
            .iter()
            .filter(|call| call.starts_with("wine regedit"))
            .count()
    };
    let before = regedits(&home);
    home.uncork()
        .args(["bottle", "set", "test1", "performance.retina=on"])
        .assert()
        .success()
        .stdout(contains("Updated the bottle's registry"));
    assert_eq!(regedits(&home), before + 1);
    // What Wine's regedit leaves in user.reg (the fake one writes nothing).
    fs::write(prefix.join("user.reg"), user_reg("y", 192)).unwrap();
    home.uncork()
        .args(["bottle", "set", "test1", "performance.retina=yes"])
        .assert()
        .success()
        .stdout("Nothing changed in bottle test1.\n");
    assert_eq!(regedits(&home), before + 1);

    home.uncork()
        .args(["bottle", "kill", "test1"])
        .assert()
        .success()
        .stdout("Stopped every Windows process in bottle test1.\n");
    assert!(
        home.calls()
            .iter()
            .any(|call| call.starts_with("wineserver --kill"))
    );

    let stderr = home.stderr_of_failure(&["bottle", "delete", "test1"]);
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(prefix.exists(), "not deleted without confirmation");
    let kills = |home: &Home| {
        home.calls()
            .iter()
            .filter(|call| call.starts_with("wineserver --kill"))
            .count()
    };
    let kills_before = kills(&home);
    home.uncork()
        .args(["bottle", "delete", "test1", "-y"])
        .assert()
        .success()
        .stdout("Deleted bottle test1.\n");
    assert!(!prefix.exists());
    assert_eq!(
        kills(&home),
        kills_before + 1,
        "the wineserver is stopped first"
    );
    assert_eq!(home.json(&["bottle", "list"]), serde_json::json!([]));
}

#[test]
fn bottle_set_rejects_bad_settings_and_changes_nothing() {
    let home = Home::new();
    home.write_bottle("b", FAKE_WINE);
    let original = fs::read_to_string(home.bottle("b").join("uncork.toml")).unwrap();
    for (setting, message) in [
        (
            "colour=red",
            "unknown setting \"colour\"; valid keys: wine, windows_version",
        ),
        (
            "performance.turbo=1",
            "unknown setting \"performance.turbo\"",
        ),
        ("performance.hud=maybe", "not a switch value"),
        ("graphics.backend=metal", "unknown backend \"metal\""),
        ("windows_version=xp", "unknown Windows version"),
        ("wine=9.9", "not found"),
        ("env.1X=a", "not a variable name"),
        ("dll_overrides.d3d11=fast", "not a DLL load order"),
        ("noequals", "expected key=value"),
    ] {
        let stderr = home.stderr_of_failure(&["bottle", "set", "b", "performance.hud=on", setting]);
        assert!(stderr.contains(message), "{setting}: {stderr}");
    }
    assert_eq!(
        fs::read_to_string(home.bottle("b").join("uncork.toml")).unwrap(),
        original,
        "a failed set writes nothing"
    );
}

#[test]
fn bottle_set_keeps_the_config_until_the_registry_has_the_change() {
    let home = Home::new();
    let wine = home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let config = home.bottle("b").join("uncork.toml");
    let original = fs::read_to_string(&config).unwrap();

    // regedit fails (say, a Wine client that cannot join the wineserver).
    let script = wine.join("bin/wine");
    let working = fs::read_to_string(&script).unwrap();
    support::write_script(
        &script,
        &working.replace("case \"$1\" in", "case \"$1\" in\n    regedit) exit 1 ;;"),
    );
    let stderr = home.stderr_of_failure(&["bottle", "set", "b", "performance.retina=on"]);
    assert!(
        stderr.contains("cannot update the bottle's registry; nothing was changed"),
        "{stderr}"
    );
    assert_eq!(fs::read_to_string(&config).unwrap(), original);

    // Running the same command again once Wine works applies it.
    support::write_script(&script, &working);
    home.uncork()
        .args(["bottle", "set", "b", "performance.retina=on"])
        .assert()
        .success()
        .stdout(contains("b: performance.retina = true"))
        .stdout(contains("Updated the bottle's registry"));
    assert_eq!(
        home.json(&["bottle", "info", "b"])["config"]["performance"]["retina"],
        true
    );
}

#[test]
fn bottle_set_without_the_bottles_wine_changes_nothing_and_can_be_repeated() {
    let home = Home::new();
    home.write_bottle("b", "11.0-missing");
    let config = home.bottle("b").join("uncork.toml");
    let original = fs::read_to_string(&config).unwrap();
    let stderr = home.stderr_of_failure(&["bottle", "set", "b", "windows_version=win7"]);
    assert!(stderr.contains("nothing was changed"), "{stderr}");
    assert!(stderr.contains("run this command again"), "{stderr}");
    assert_eq!(fs::read_to_string(&config).unwrap(), original);

    // Settings that do not touch the registry still work without Wine.
    home.uncork()
        .args(["bottle", "set", "b", "performance.hud=on"])
        .assert()
        .success();
}

/// A `user.reg` with Retina mode `retina` and `dpi` DPI, as CrossOver and
/// Wine write it.
fn user_reg(retina: &str, dpi: u32) -> String {
    format!(
        "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n\
         [Control Panel\\\\Desktop] 1759800000\n\"LogPixels\"=dword:{dpi:08x}\n\n\
         [Software\\\\Wine\\\\Mac Driver] 1759800000\n\"RetinaMode\"=\"{retina}\"\n"
    )
}

#[test]
fn bottle_set_retina_stops_a_running_bottle_first() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    home.start_fake_wineserver();

    home.uncork()
        .args(["bottle", "set", "b", "performance.retina=on"])
        .assert()
        .success()
        .stdout(contains("b: performance.retina = true"))
        .stdout(contains(
            "Stopped the Windows programs that were running in bottle b (Steam included)",
        ))
        .stdout(contains(
            "Updated the bottle's registry (Retina mode on, 192 DPI, Windows version win10).",
        ));
    let calls = home.calls();
    let at = |prefix: &str| {
        calls
            .iter()
            .position(|call| call.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix}: {calls:#?}"))
    };
    assert!(at("wineserver -k0") < at("wineserver --kill"), "{calls:#?}");
    assert!(
        at("wineserver --kill") < at("wine regedit /S"),
        "{calls:#?}"
    );
    assert!(
        calls.last().unwrap().starts_with("wineserver --wait"),
        "the next launch starts a fresh wineserver: {calls:#?}"
    );

    // With nothing running, nothing is stopped.
    home.uncork()
        .args(["bottle", "set", "b", "windows_version=win7"])
        .assert()
        .success()
        .stdout(contains("Stopped").not())
        .stdout(contains("Windows version win7"));
}

#[test]
fn bottle_set_repairs_a_retina_and_dpi_pair_that_disagrees() {
    let home = Home::new();
    home.install_fake_wine();
    let dir = home.write_bottle("cx", FAKE_WINE);
    // Uncork's RetinaMode n next to the 192 DPI CrossOver left.
    fs::write(dir.join("user.reg"), user_reg("n", 192)).unwrap();

    let doctor = home.json(&["doctor"]);
    let check = doctor["checks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|check| check["id"] == "bottle-dpi")
        .unwrap_or_else(|| panic!("{doctor:#}"));
    assert_eq!(check["status"], "warn");
    assert!(
        check["summary"]
            .as_str()
            .unwrap()
            .contains("Retina mode and DPI disagree (RetinaMode n, LogPixels 192)"),
        "{check}"
    );
    assert_eq!(
        check["fix"],
        "uncork bottle set cx performance.retina=false"
    );

    // The fix rewrites the registry although uncork.toml stays the same.
    let regedits = |home: &Home| {
        home.calls()
            .iter()
            .filter(|call| call.starts_with("wine regedit"))
            .count()
    };
    home.uncork()
        .args(["bottle", "set", "cx", "performance.retina=false"])
        .assert()
        .success()
        .stdout(contains("Nothing changed").not())
        .stdout(contains("Retina mode off, 96 DPI"));
    assert_eq!(regedits(&home), 1);

    // Other settings leave the registry alone.
    home.uncork()
        .args(["bottle", "set", "cx", "performance.hud=on"])
        .assert()
        .success()
        .stdout(contains("Updated the bottle's registry").not());
    assert_eq!(regedits(&home), 1);

    // A consistent pair needs no repair.
    fs::write(dir.join("user.reg"), user_reg("n", 96)).unwrap();
    home.uncork()
        .args(["bottle", "set", "cx", "performance.retina=false"])
        .assert()
        .success()
        .stdout("Nothing changed in bottle cx.\n");
    let doctor = home.json(&["doctor"]);
    assert!(
        !doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|check| check["id"] == "bottle-dpi"),
        "{doctor:#}"
    );
}

#[test]
fn bottle_set_repairs_a_retina_bottle_with_a_missing_value() {
    let home = Home::new();
    home.install_fake_wine();
    let dir = home.write_bottle("r", FAKE_WINE);
    fs::write(
        dir.join("uncork.toml"),
        format!("schema = 1\nname = \"r\"\nwine = {FAKE_WINE:?}\n[performance]\nretina = true\n"),
    )
    .unwrap();
    let regedits = |home: &Home| {
        home.calls()
            .iter()
            .filter(|call| call.starts_with("wine regedit"))
            .count()
    };
    let header = "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n#arch=win64\n\n";
    for (user_reg, found) in [
        // An import whose registry write failed: CrossOver's 192 DPI, no
        // RetinaMode, so Wine runs with Retina mode off.
        (
            "[Control Panel\\\\Desktop] 1759800000\n\"LogPixels\"=dword:000000c0\n",
            "RetinaMode not set, LogPixels 192",
        ),
        // A Retina bottle made before Uncork wrote the DPI: Wine's 96.
        (
            "[Software\\\\Wine\\\\Mac Driver] 1759800000\n\"RetinaMode\"=\"y\"\n",
            "RetinaMode y, LogPixels not set",
        ),
    ] {
        fs::write(dir.join("user.reg"), format!("{header}{user_reg}")).unwrap();
        let doctor = home.json(&["doctor"]);
        let check = doctor["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == "bottle-dpi")
            .unwrap_or_else(|| panic!("{found}: {doctor:#}"))
            .clone();
        assert!(
            check["summary"]
                .as_str()
                .unwrap()
                .contains(&format!("Retina mode and DPI disagree ({found})")),
            "{check}"
        );
        assert_eq!(check["fix"], "uncork bottle set r performance.retina=true");

        let before = regedits(&home);
        home.uncork()
            .args(["bottle", "set", "r", "performance.retina=true"])
            .assert()
            .success()
            .stdout(contains("Nothing changed").not())
            .stdout(contains("Retina mode on, 192 DPI"));
        assert_eq!(regedits(&home), before + 1, "{found}");
    }
}

#[test]
fn bottle_set_frame_cap() {
    let home = Home::new();
    home.write_bottle("b", FAKE_WINE);
    let max_fps = |home: &Home| {
        home.json(&["bottle", "info", "b"])["config"]["performance"]["max_fps"].clone()
    };
    assert_eq!(max_fps(&home), serde_json::Value::Null);

    home.uncork()
        .args(["bottle", "set", "b", "performance.max_fps=60"])
        .assert()
        .success()
        .stdout("b: performance.max_fps = 60\n");
    assert_eq!(max_fps(&home), 60);
    let info = home.stdout(&["bottle", "info", "b"]);
    assert!(info.contains("max_fps 60"), "{info}");

    home.uncork()
        .args(["bottle", "set", "b", "performance.max_fps=0"])
        .assert()
        .success()
        .stdout(contains("performance.max_fps = 0 (uncapped)"));
    assert!(
        home.stdout(&["bottle", "info", "b"])
            .contains("max_fps uncapped")
    );

    home.uncork()
        .args(["bottle", "set", "b", "performance.max_fps="])
        .assert()
        .success()
        .stdout(contains(
            "performance.max_fps unset (the main display's refresh rate)",
        ));
    assert_eq!(max_fps(&home), serde_json::Value::Null);
    assert!(
        home.stdout(&["bottle", "info", "b"])
            .contains("max_fps display")
    );

    for bad in ["fast", "-1", "60.5", "100000"] {
        let stderr =
            home.stderr_of_failure(&["bottle", "set", "b", &format!("performance.max_fps={bad}")]);
        assert!(stderr.contains("is not a frame cap"), "{bad}: {stderr}");
    }
}

#[test]
fn bottle_set_says_to_restart_after_an_msync_change() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    home.uncork()
        .args(["bottle", "set", "b", "performance.msync=off"])
        .assert()
        .success()
        .stderr(contains("uncork bottle kill b"));
    home.uncork()
        .args(["bottle", "set", "b", "performance.hud=on"])
        .assert()
        .success()
        .stderr(contains("bottle kill").not());
}

#[test]
fn bottle_create_needs_wine_and_a_free_name() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["bottle", "create", "x"]);
    assert!(stderr.contains("uncork runtime install wine"), "{stderr}");
    let stderr = home.stderr_of_failure(&["bottle", "create", "bad/name"]);
    assert!(stderr.contains("invalid bottle name"), "{stderr}");

    home.install_fake_wine();
    let stderr = home.stderr_of_failure(&["bottle", "create", "x", "--wine", "9.9"]);
    assert!(stderr.contains("\"wine 9.9\" not found"), "{stderr}");
    let stderr = home.stderr_of_failure(&["bottle", "create", "x", "--windows", "vista"]);
    assert!(stderr.contains("unknown Windows version"), "{stderr}");
    home.write_bottle("x", FAKE_WINE);
    let stderr = home.stderr_of_failure(&["bottle", "create", "x"]);
    assert!(stderr.contains("already exists"), "{stderr}");
}

#[test]
fn bottle_create_with_windows_version() {
    let home = Home::new();
    home.install_fake_wine();
    home.uncork()
        .args(["bottle", "create", "old", "--windows", "win7"])
        .assert()
        .success()
        .stdout(contains("(Wine fake-1, win7)"));
    let info = home.json(&["bottle", "info", "old"]);
    assert_eq!(info["config"]["windows_version"], "win7");
}

#[test]
fn bottle_import_of_a_crossover_prefix_keeps_its_user() {
    let home = Home::new();
    home.install_fake_wine();
    let source = home.path().join("CrossOver Bottles").join("My Steam");
    fs::create_dir_all(source.join("drive_c/users/crossover/AppData")).unwrap();
    fs::create_dir_all(source.join("drive_c/users/Public")).unwrap();
    fs::write(source.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    fs::write(source.join("drive_c/users/crossover/AppData/x.txt"), "x").unwrap();

    home.uncork()
        .args(["bottle", "import", source.to_str().unwrap()])
        .assert()
        .success()
        .stdout(contains("as bottle my-steam"))
        .stdout(contains("cloned; the original is untouched"))
        .stdout(contains("USER=crossover and LOGNAME=crossover"))
        .stdout(contains(
            "The prefix set neither Retina mode nor its DPI, so the bottle has Uncork's default: Retina mode off with 96 DPI",
        ));
    let imported = home.bottle("my-steam");
    assert!(
        imported
            .join("drive_c/users/crossover/AppData/x.txt")
            .is_file()
    );
    assert!(source.join("system.reg").is_file(), "the original stays");

    let info = home.json(&["bottle", "info", "my-steam"]);
    assert_eq!(info["config"]["env"]["USER"], "crossover");
    assert_eq!(info["config"]["env"]["LOGNAME"], "crossover");
    assert_eq!(info["config"]["wine"], FAKE_WINE);
    assert!(
        info["config"]["imported_from"]
            .as_str()
            .unwrap()
            .ends_with("My Steam")
    );

    // The same name again is refused; --name picks another one; --move moves.
    let stderr = home.stderr_of_failure(&["bottle", "import", source.to_str().unwrap()]);
    assert!(stderr.contains("already exists"), "{stderr}");
    home.uncork()
        .args([
            "bottle",
            "import",
            source.to_str().unwrap(),
            "--name",
            "moved",
            "--move",
        ])
        .assert()
        .success()
        .stdout(contains("as bottle moved"))
        .stdout(contains("(moved)"));
    assert!(!source.exists());
    assert!(home.bottle("moved").join("system.reg").is_file());
}

#[test]
fn bottle_import_keeps_crossovers_high_resolution_mode() {
    let home = Home::new();
    home.install_fake_wine();
    let source = home.path().join("Steam");
    fs::create_dir_all(source.join("drive_c/users/crossover")).unwrap();
    fs::write(source.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    fs::write(source.join("user.reg"), user_reg("y", 192)).unwrap();

    home.uncork()
        .args(["bottle", "import", source.to_str().unwrap(), "--name", "cx"])
        .assert()
        .success()
        .stdout(contains(
            "The prefix had RetinaMode y, LogPixels 192, so Retina mode stays on (performance.retina = true) with 192 DPI to match",
        ))
        .stdout(contains("uncork bottle set cx performance.retina=false"));
    let info = home.json(&["bottle", "info", "cx"]);
    assert_eq!(info["config"]["performance"]["retina"], true);
    let calls = home.calls();
    let regedit = calls
        .iter()
        .position(|call| call.starts_with("wine regedit /S"))
        .unwrap_or_else(|| panic!("{calls:#?}"));
    assert!(
        calls[regedit].contains(&home.bottle("cx").display().to_string()),
        "{calls:#?}"
    );
    assert!(
        calls[regedit + 1].starts_with("wineserver --wait"),
        "{calls:#?}"
    );
}

#[test]
fn bottle_import_needs_a_prefix_and_a_wine() {
    let home = Home::new();
    let source = home.path().join("prefix");
    fs::create_dir_all(source.join("drive_c")).unwrap();
    fs::write(source.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
    let stderr = home.stderr_of_failure(&["bottle", "import", source.to_str().unwrap()]);
    assert!(stderr.contains("needs an installed Wine"), "{stderr}");

    home.install_fake_wine();
    let not_prefix = home.path().join("empty");
    fs::create_dir_all(&not_prefix).unwrap();
    let stderr = home.stderr_of_failure(&["bottle", "import", not_prefix.to_str().unwrap()]);
    assert!(stderr.contains("drive_c and system.reg"), "{stderr}");
    let stderr = home.stderr_of_failure(&[
        "bottle",
        "import",
        home.path().join("missing").to_str().unwrap(),
    ]);
    assert!(stderr.contains("cannot find"), "{stderr}");
}

#[test]
fn bottle_commands_name_missing_bottles() {
    let home = Home::new();
    for args in [
        &["bottle", "info", "ghost"][..],
        &["bottle", "set", "ghost", "performance.hud=on"],
        &["bottle", "kill", "ghost"],
        &["bottle", "env", "ghost"],
        &["bottle", "delete", "ghost", "-y"],
        &["steam", "games", "--bottle", "ghost"],
    ] {
        let stderr = home.stderr_of_failure(args);
        assert!(
            stderr.contains("bottle \"ghost\" not found; run `uncork bottle list`"),
            "{args:?}: {stderr}"
        );
    }
}

#[test]
fn bottle_tool_rejects_unknown_tools() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["bottle", "tool", "x", "notepad"]);
    assert!(
        stderr.contains("unknown tool \"notepad\"; choose one of winecfg, regedit"),
        "{stderr}"
    );
}

#[test]
fn bottle_kill_clears_a_crashed_steam_clients_pid_when_nothing_runs() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("steam", FAKE_WINE);
    home.install_fake_steam("steam", &[]);
    home.fake_crashed_steam();
    home.uncork()
        .args(["bottle", "kill", "steam"])
        .assert()
        .success()
        .stdout("Nothing was running in bottle steam.\n");
    assert!(
        home.calls().iter().any(|call| call.starts_with(
            r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess /v pid /t REG_DWORD /d 0 /f |"
        )),
        "{:#?}",
        home.calls()
    );
}

#[test]
fn bottle_kill_reports_an_uninstalled_wine() {
    let home = Home::new();
    home.write_bottle("b", "11.0-gone");
    let stderr = home.stderr_of_failure(&["bottle", "kill", "b"]);
    assert!(stderr.contains("bottle b needs Wine 11.0-gone"), "{stderr}");
    assert!(stderr.contains("uncork runtime install wine"), "{stderr}");
}

#[test]
fn steam_games_lists_installed_apps_with_their_profiles() {
    let home = Home::new();
    home.write_bottle("steam", FAKE_WINE);
    let stderr = home.stderr_of_failure(&["steam", "games"]);
    assert!(
        stderr.contains("Steam is not installed in bottle steam; run: uncork steam install"),
        "{stderr}"
    );

    home.install_fake_steam(
        "steam",
        &[
            (287_450, "Rise of Nations", 4),
            (813_780, "Age of Empires II: Definitive Edition", 6),
            (70, "Half-Life", 0),
        ],
    );
    let games = home.json(&["steam", "games"]);
    let games = games.as_array().unwrap();
    assert_eq!(games.len(), 3);
    assert_eq!(games[0]["appid"], 70);
    assert_eq!(games[0]["state"], "partial");
    assert_eq!(games[0]["profile"], serde_json::Value::Null);
    assert_eq!(games[1]["appid"], 287_450);
    assert_eq!(games[1]["state"], "ready");
    assert_eq!(games[1]["profile"], "rise-of-nations-extended-edition");
    assert_eq!(games[1]["size_on_disk"], 1_500_000_000_u64);
    assert_eq!(games[2]["state"], "updating");

    let text = home.stdout(&["steam", "games"]);
    assert!(text.starts_with("APPID   NAME"), "{text}");
    assert!(
        text.contains("287450  Rise of Nations                        Ready     1.5 GB  rise-of-nations-extended-edition"),
        "{text}"
    );
    assert!(text.contains("Updating"), "{text}");
    assert!(text.contains("70      Half-Life"), "{text}");
    assert!(text.contains("Partial"), "{text}");
}

#[test]
fn the_default_bottle_comes_from_the_config() {
    let home = Home::new();
    home.write_bottle("games", FAKE_WINE);
    home.install_fake_steam("games", &[]);
    fs::write(
        home.root().join("config.toml"),
        "schema = 1\ndefault_bottle = \"games\"\n",
    )
    .unwrap();
    home.uncork()
        .args(["steam", "games"])
        .assert()
        .success()
        .stdout(contains("No games installed in bottle games"));
    home.uncork()
        .args(["bottle", "list"])
        .assert()
        .success()
        .stdout(contains("games (default)"))
        .stdout(contains("fake-1 (not installed)"));
}

#[test]
fn bottle_delete_of_a_bottle_with_a_broken_config() {
    let home = Home::new();
    let dir = home.write_bottle("b", FAKE_WINE);
    fs::write(dir.join("uncork.toml"), "this is not toml").unwrap();
    home.uncork()
        .args(["bottle", "delete", "b", "--yes"])
        .assert()
        .success()
        .stdout("Deleted bottle b.\n")
        .stderr(contains("error").not());
    assert!(!dir.exists());
}

/// Run `uncork <args>`; `None` when this host has no Rosetta (setup stops
/// there before doing anything), else the output.
fn setup_output(home: &Home, args: &[&str]) -> Option<std::process::Output> {
    let output = home.uncork().args(args).output().unwrap();
    if String::from_utf8_lossy(&output.stderr).contains("Rosetta 2 is not installed") {
        eprintln!("skipped: Rosetta 2 is not installed on this host");
        return None;
    }
    Some(output)
}

#[test]
fn setup_with_everything_installed_creates_the_bottle() {
    let home = Home::new();
    home.install_fake_wine();
    home.install_component("dxmt", "0.80", &[], &[]);
    home.install_component("dxvk", "1.10.3-20230507", &[], &[]);
    let Some(output) = setup_output(&home, &["setup", "--no-steam", "--bottle", "games"]) else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("Created bottle games"), "{stdout}");
    assert!(stdout.contains("Default bottle is now games."), "{stdout}");
    assert!(stdout.contains("uncork steam install"), "{stdout}");
    assert!(home.bottle("games").join("system.reg").is_file());
    let config = fs::read_to_string(home.root().join("config.toml")).unwrap();
    assert!(config.contains("default_bottle = \"games\""), "{config}");

    let Some(again) = setup_output(&home, &["setup", "--no-steam", "--bottle", "games"]) else {
        return;
    };
    assert!(again.status.success());
    let stdout = String::from_utf8_lossy(&again.stdout);
    assert!(stdout.contains("Bottle games already exists"), "{stdout}");
    assert!(!stdout.contains("Default bottle is now"), "{stdout}");
}

#[test]
fn setup_finishes_a_bottle_an_earlier_run_left_half_made() {
    let home = Home::new();
    home.install_fake_wine();
    home.install_component("dxmt", "0.80", &[], &[]);
    home.install_component("dxvk", "1.10.3-20230507", &[], &[]);
    home.write_half_made_bottle("games", FAKE_WINE);
    let Some(output) = setup_output(&home, &["setup", "--no-steam", "--bottle", "games"]) else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Finished creating bottle games."),
        "{stdout}"
    );
    assert!(home.bottle("games").join("system.reg").is_file());
    let calls = home.calls();
    assert!(calls[0].starts_with("wine wineboot -u |"), "{calls:#?}");
    assert!(
        calls.iter().any(|call| call.starts_with("wine regedit /S")),
        "{calls:#?}"
    );
    assert_eq!(home.json(&["bottle", "info", "games"])["initialized"], true);
}

#[test]
fn steam_install_finishes_a_half_made_bottle_first() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_half_made_bottle("b", FAKE_WINE);
    let output = home
        .uncork()
        .args(["steam", "install", "-b", "b"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stdout.contains("Finished creating bottle b."), "{stdout}");
    // Then it stops at the download question (no terminal, no --yes).
    assert!(!output.status.success());
    assert!(stderr.contains("--yes"), "{stderr}");
}

#[test]
fn setup_starts_an_installed_steam_visibly() {
    let home = Home::new();
    home.install_fake_wine();
    home.install_component("dxmt", "0.80", &[], &[]);
    home.install_component(
        "dxvk",
        "1.10.3-20230507",
        &[],
        &["x86_64-windows/d3d11.dll", "x86_64-windows/d3d10core.dll"],
    );
    home.uncork()
        .args(["bottle", "create", "steam"])
        .assert()
        .success();
    let steam = home.install_fake_steam("steam", &[]);
    let Some(output) = setup_output(&home, &["setup"]) else {
        return;
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("Started Steam in bottle steam (pid "),
        "{stdout}"
    );
    assert!(
        stdout.contains(
            "Sign in to Steam in the window that opened, install your game, then run: uncork play <game>"
        ),
        "{stdout}"
    );
    assert!(
        steam.join("bin/cef/cef.win64/d3d11.dll").is_file(),
        "DXVK app-local for the web helper"
    );
}
