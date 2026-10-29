//! `uncork status` and `uncork logs`: read-only state with a fake Wine.
//! Neither command may start, stop or change anything in a bottle.

mod support;

use std::fs;

use serde_json::{Value, json};
use support::{EXTERNAL_5K_DISPLAY, FAKE_WINE, Home};

#[test]
fn status_never_queries_the_registry_when_no_wineserver_runs() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    home.install_fake_steam("b", &[]);
    // A client that crashed: its pid is in the registry, no wineserver runs.
    home.fake_crashed_steam();

    let status = home.json(&["status", "-b", "b"]);
    assert_eq!(status["display"]["signature"], "Color LCD 1728x1117");
    assert_eq!(status["bottles"].as_array().unwrap().len(), 1);
    let bottle = &status["bottles"][0];
    assert_eq!(bottle["name"], "b");
    assert_eq!(bottle["wine"]["version"], FAKE_WINE);
    assert_eq!(bottle["wine"]["installed"], true);
    assert_eq!(bottle["running"], false);
    assert_eq!(bottle["steam"]["installed"], true);
    assert_eq!(bottle["steam"]["running"], false);
    assert_eq!(bottle["steam"]["pid"], Value::Null);
    assert_eq!(bottle["steam"]["signed_in"], false);
    assert_eq!(bottle["backend"], Value::Null);
    assert_eq!(bottle["session_display"], Value::Null);
    assert_eq!(bottle["display_changed"], Value::Null);

    let calls = home.calls();
    assert!(
        calls.iter().all(|call| call.starts_with("wineserver -k0 ")),
        "only the side-effect-free probe may run: {calls:#?}"
    );
    assert_eq!(calls.len(), 1, "{calls:#?}");
    assert_eq!(
        fs::read_to_string(home.path().join("steam-pid")).unwrap(),
        "0x274",
        "the stale pid is reported as not running, not cleared"
    );

    let text = home.stdout(&["status", "-b", "b"]);
    assert!(text.contains("Running          no\n"), "{text}");
    assert!(
        text.contains("Steam            installed, not running\n"),
        "{text}"
    );
}

#[test]
fn status_reads_steam_while_a_wineserver_runs() {
    let home = Home::new();
    home.install_fake_wine();
    let dir = home.write_bottle("b", FAKE_WINE);
    home.install_fake_steam("b", &[]);
    fs::write(
        dir.join("uncork-state.toml"),
        format!(
            "schema = 1\nprefix_wine = {FAKE_WINE:?}\nsession_display = \"Color LCD 1728x1117 @120Hz\"\n\n[active]\nbackend = \"dxmt\"\nversion = \"0.80\"\ndlls = []\n"
        ),
    )
    .unwrap();
    // A bottle whose Wine is not installed: nothing can be asked.
    home.write_bottle("old", "wine-0");
    home.fake_running_steam();

    let status = home.json(&["status"]);
    let bottles = status["bottles"].as_array().unwrap();
    assert_eq!(bottles.len(), 2, "{status}");
    let bottle = &bottles[0];
    assert_eq!(bottle["name"], "b");
    assert_eq!(bottle["default"], false);
    assert_eq!(bottle["running"], true);
    assert_eq!(bottle["steam"]["running"], true);
    assert_eq!(bottle["steam"]["pid"], 0x274);
    assert_eq!(
        bottle["steam"]["signed_in"], false,
        "the fake records no ActiveUser"
    );
    assert_eq!(bottle["steam"]["user"], Value::Null);
    assert_eq!(bottle["backend"]["name"], "dxmt");
    assert_eq!(bottle["backend"]["version"], "0.80");
    assert_eq!(
        bottle["session_display"], "Color LCD 1728x1117",
        "read without the refresh rate"
    );
    assert_eq!(bottle["display_changed"], false);
    let old = &bottles[1];
    assert_eq!(old["name"], "old");
    assert_eq!(old["wine"]["installed"], false);
    assert_eq!(old["running"], Value::Null);
    assert_eq!(old["steam"]["installed"], false);
    assert_eq!(old["steam"]["running"], false);

    let text = home.stdout(&["status", "-b", "b"]);
    assert!(
        text.starts_with("Main display  Color LCD 1728x1117 @120Hz\n\nBottle           b\n"),
        "{text}"
    );
    assert!(
        text.contains("Steam            running (pid 628), nobody signed in\n"),
        "{text}"
    );
    assert!(
        text.contains("Backend          dxmt 0.80 (DLLs in the prefix)\n"),
        "{text}"
    );
    assert!(
        text.contains("Session display  Color LCD 1728x1117 (the main display now)\n"),
        "{text}"
    );

    // Another display became the main one while the bottle ran.
    home.set_displays(EXTERNAL_5K_DISPLAY);
    let status = home.json(&["status", "-b", "b"]);
    assert_eq!(status["display"]["signature"], "LG UltraFine 2560x1440");
    assert_eq!(status["bottles"][0]["display_changed"], true);
    let text = home.stdout(&["status", "-b", "b"]);
    assert!(
        text.contains("Color LCD 1728x1117 (differs from the main display now; a running bottle is restarted before the next launch)"),
        "{text}"
    );

    let calls = home.calls();
    assert!(
        calls.iter().all(|call| {
            call.starts_with("wineserver -k0 ")
                || call.starts_with(r"wine reg query HKCU\Software\Valve\Steam\ActiveProcess /v ")
        }),
        "status only probes and reads: {calls:#?}"
    );
}

#[test]
fn status_needs_an_existing_bottle_when_one_is_named() {
    let home = Home::new();
    let stderr = home.stderr_of_failure(&["status", "-b", "nope"]);
    assert!(stderr.contains("nope"), "{stderr}");
    let status = home.json(&["status"]);
    assert_eq!(status["bottles"], json!([]));
    assert!(home.stdout(&["status"]).contains("No bottles yet."));
}

#[test]
fn logs_show_the_newest_launch_log() {
    let home = Home::new();
    home.install_fake_wine();
    home.write_bottle("b", FAKE_WINE);
    let logs = home.root().join("logs");

    let text = home.stdout(&["logs", "-b", "b"]);
    assert!(
        text.starts_with("No launch logs in bottle b yet;"),
        "{text}"
    );
    assert_eq!(home.json(&["logs", "-b", "b"]), Value::Null);
    let output = home
        .uncork()
        .args(["logs", "-b", "b", "--path"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stdout.is_empty(), "nothing to print");
    assert!(String::from_utf8_lossy(&output.stderr).contains("No launch logs in bottle b yet;"));

    fs::create_dir_all(&logs).unwrap();
    fs::write(
        logs.join("b-riseofnations-100.log"),
        "one\r\ntwo\r\nthree\r\n",
    )
    .unwrap();
    fs::write(logs.join("b-steam-200.log"), "steam a\nsteam b\n").unwrap();
    fs::write(logs.join("b-stop.log"), "not a launch log\n").unwrap();
    fs::write(logs.join("other-cmd-300.log"), "another bottle\n").unwrap();

    assert_eq!(home.stdout(&["logs", "-b", "b"]), "steam a\nsteam b\n");
    let path = home.stdout(&["logs", "-b", "b", "--path"]);
    assert_eq!(
        path.trim_end(),
        logs.join("b-steam-200.log").to_str().unwrap()
    );
    assert_eq!(
        home.stdout(&["logs", "-b", "b", "--game", "rise-of-nations", "-n", "2"]),
        "two\nthree\n",
        "a profile id names its executable's stem"
    );
    assert_eq!(
        home.stdout(&["logs", "-b", "b", "--game", "RiseOfNations.exe", "-n", "0"]),
        "one\ntwo\nthree\n",
        "an executable name, in any case; -n 0 is the whole log"
    );
    assert_eq!(
        home.stdout(&["logs", "-b", "b", "-g", "steam"]),
        "steam a\nsteam b\n"
    );

    let log = home.json(&["logs", "-b", "b", "-g", "287450"]);
    assert_eq!(
        log["path"],
        logs.join("b-riseofnations-100.log").to_str().unwrap()
    );
    assert_eq!(log["program"], "riseofnations");
    assert_eq!(log["bottle"], "b");
    assert_eq!(log["started"], 100);
    assert_eq!(log["size"], 17);
    assert!(log["modified"].is_u64(), "{log}");
    assert_eq!(log["lines"], json!(["one", "two", "three"]));
    let log = home.json(&["logs", "-b", "b", "--path"]);
    assert_eq!(log["lines"], json!([]), "--path reads no lines: {log}");

    let text = home.stdout(&["logs", "-b", "b", "--game", "cmd.exe"]);
    assert!(
        text.starts_with("No launch logs of cmd in bottle b yet;"),
        "{text}"
    );

    assert!(home.calls().is_empty(), "logs never runs Wine");
}
