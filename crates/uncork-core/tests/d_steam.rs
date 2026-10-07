//! The Steam client in a bottle, through the public API with a fake Wine:
//! the client command, its app-local DXVK, detecting, starting and stopping
//! it, finding a game's executable, and the `play` flows that need no
//! polling (the polling flows are unit-tested in `steam.rs` with a short
//! interval).

mod d_support;
#[path = "../../uncork-pe/tests/common/mod.rs"]
mod pe;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use d_support::{CX_FEATURES, Fixture, builtin_dll, ctx, profile, write};
use pe::PeBuilder;
use uncork_core::Error;
use uncork_core::bottle::Bottle;
use uncork_core::component::ComponentKind;
use uncork_core::graphics::Backend;
use uncork_core::launch::LaunchOptions;
use uncork_core::steam::{
    self, client_command, ensure_client_dxvk, ensure_running, is_running, plan_game, stop,
};
use uncork_steam::SteamInstall;

const APPID: u32 = 287450;

/// [`PeBuilder::write`], creating the directory first.
trait WriteTo {
    fn write_to(&self, path: &Path);
}

impl WriteTo for PeBuilder {
    fn write_to(&self, path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        self.write(path);
    }
}

fn strings(spec: &uncork_core::process::CommandSpec) -> Vec<String> {
    spec.args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn steam_of(bottle: &Bottle) -> SteamInstall {
    SteamInstall::find(bottle.prefix()).unwrap()
}

fn cef_dir(bottle: &Bottle) -> PathBuf {
    steam_of(bottle).root.join(uncork_steam::client::CEF_DIR)
}

fn mtime(path: &Path) -> std::time::SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

// ----- the client command -----

#[test]
fn client_command_has_the_documented_shape() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
        config
            .env
            .insert("WINEDLLOVERRIDES".to_owned(), "dxgi=n".to_owned());
    });
    let root = fx.install_steam(&bottle);
    let extra = ["-console", "-nooverlay", "-cef-force-32bit"].map(String::from);

    let spec = client_command(&bottle, &fx.wine, &extra).unwrap();

    assert_eq!(spec.program, fx.wine.wine_bin());
    assert_eq!(
        strings(&spec),
        [
            root.join("steam.exe").to_str().unwrap(),
            "-nofriendsui",
            "-console"
        ],
        "dead flags are stripped"
    );
    assert!(
        strings(&spec).iter().all(|arg| !arg.contains("cef")),
        "software CEF leaves every window black: never pass -cef-* flags"
    );
    assert_eq!(spec.cwd.as_deref(), Some(root.as_path()));
    assert!(spec.env_clear);
    assert_eq!(
        spec.env["WINEDLLOVERRIDES"],
        "d3d12,d3d9,dxgi=b;winemenubuilder.exe=d;d3d10core,d3d11=n,b"
    );
    assert_eq!(spec.env["DXVK_LOG_LEVEL"], "none");
    assert_eq!(
        spec.env["USER"], "crossover",
        "the bottle's env is included"
    );
    assert_eq!(
        spec.env["WINEMSYNC"], "1",
        "the base environment is included"
    );
    assert_eq!(spec.env["WINEDEBUG"], "-all");
    let log = spec.log.as_ref().unwrap();
    assert_eq!(log.parent(), Some(fx.layout.logs_dir().as_path()));
    let name = log.file_name().unwrap().to_str().unwrap();
    assert!(
        name.starts_with("steam-steam-") && name.ends_with(".log"),
        "{name}"
    );
}

#[test]
fn a_bottle_may_turn_dxvk_logging_on_for_the_client() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config
            .env
            .insert("DXVK_LOG_LEVEL".to_owned(), "info".to_owned());
    });
    fx.install_steam(&bottle);
    let spec = client_command(&bottle, &fx.wine, &[]).unwrap();
    assert_eq!(spec.env["DXVK_LOG_LEVEL"], "info");
}

#[test]
fn client_command_needs_steam() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("games", |_| {});
    let err = client_command(&bottle, &fx.wine, &[]).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Steam in bottle \"games\" not found; run: uncork steam install --bottle games"
    );
}

// ----- app-local DXVK -----

#[test]
fn ensure_client_dxvk_puts_marker_stripped_dlls_beside_the_web_helper() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    let dxvk = fx.dxvk("1.10.3");
    let steam = steam_of(&bottle);

    assert!(ensure_client_dxvk(&steam, Some(&dxvk)).unwrap());
    let dir = cef_dir(&bottle);
    for dll in ["d3d11", "d3d10core"] {
        let bytes = fs::read(dir.join(format!("{dll}.dll"))).unwrap();
        assert_eq!(
            uncork_pe::wine_marker(&bytes),
            None,
            "{dll}: marker stripped"
        );
        let mut expected = builtin_dll(&format!("dxvk 1.10.3 x86_64-windows {dll}"));
        assert!(uncork_pe::strip_builtin_marker(&mut expected));
        assert_eq!(bytes, expected, "{dll}: the 64-bit DLL");
    }
    let mut names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["d3d10core.dll", "d3d11.dll"],
        "nothing else, no temp files"
    );

    // Identical files are left alone.
    let d3d11 = dir.join("d3d11.dll");
    let before = mtime(&d3d11);
    std::thread::sleep(Duration::from_millis(20));
    assert!(ensure_client_dxvk(&steam, Some(&dxvk)).unwrap());
    assert_eq!(mtime(&d3d11), before);

    // A different DXVK replaces them.
    let newer = fx.dxvk("1.10.4");
    assert!(ensure_client_dxvk(&steam, Some(&newer)).unwrap());
    assert!(
        fs::read(&d3d11)
            .unwrap()
            .ends_with(b"dxvk 1.10.4 x86_64-windows d3d11")
    );
}

#[test]
fn ensure_client_dxvk_without_dxvk_does_nothing() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    assert!(!ensure_client_dxvk(&steam_of(&bottle), None).unwrap());
    assert!(!cef_dir(&bottle).exists());
}

#[test]
fn ensure_client_dxvk_reports_an_incomplete_component() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    let partial = fx.component(
        ComponentKind::Dxvk,
        "partial",
        &[("x86_64-windows/d3d11.dll", builtin_dll("d3d11"))],
    );
    let err = ensure_client_dxvk(&steam_of(&bottle), Some(&partial)).unwrap_err();
    assert!(
        matches!(&err, Error::BrokenComponent { kind: ComponentKind::Dxvk, message, .. } if message == "x86_64-windows/d3d10core.dll is missing"),
        "{err:?}"
    );
    assert!(
        !cef_dir(&bottle).join("d3d11.dll").exists(),
        "nothing half-copied"
    );

    let dxmt = fx.dxmt("0.80");
    let err = ensure_client_dxvk(&steam_of(&bottle), Some(&dxmt)).unwrap_err();
    assert!(matches!(err, Error::BrokenComponent { .. }), "{err:?}");
}

// ----- is it running? -----

#[test]
fn is_running_reads_the_active_process_pid() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    assert!(
        !is_running(&bottle, &fx.wine).unwrap(),
        "no key: not running"
    );
    for (pid, running) in [
        ("0x274", true),
        ("0x0", false),
        ("12", true),
        ("bogus", false),
    ] {
        fx.set_pid(pid);
        assert_eq!(is_running(&bottle, &fx.wine).unwrap(), running, "{pid}");
    }

    let calls = fx.calls();
    let query = r"wine reg query HKCU\Software\Valve\Steam\ActiveProcess /v pid | cwd=";
    assert!(
        calls.iter().all(|call| call.starts_with(query)),
        "{calls:#?}"
    );
    assert!(
        calls[0].contains(&format!("WINEPREFIX={} ", bottle.path.display())),
        "{}",
        calls[0]
    );
    assert!(
        calls[0].contains(" WINEMSYNC=1 "),
        "a wineserver the query starts gets the bottle's msync setting: {}",
        calls[0]
    );
}

#[test]
fn is_running_reports_other_failures() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fs::write(fx.state.join("reg-exit"), "3").unwrap();
    let err = is_running(&bottle, &fx.wine).unwrap_err();
    assert_eq!(err.to_string(), "wine reg query exited with status 3");

    let mut broken = fx.wine.clone();
    broken.root = fx.dir.path().join("no-runtime");
    let err = is_running(&bottle, &broken).unwrap_err();
    assert!(err.to_string().starts_with("wine could not start"), "{err}");
}

// ----- starting and stopping -----

#[test]
fn ensure_running_leaves_a_running_client_alone() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    fx.dxvk("1.10.3");
    fx.set_pid("0x274");
    ensure_running(&fx.layout, &bottle, &fx.wine, Duration::from_secs(60)).unwrap();
    assert_eq!(
        fx.calls().len(),
        1,
        "one query, no start: {:#?}",
        fx.calls()
    );
    assert!(
        !cef_dir(&bottle).exists(),
        "a running client's DLLs are not touched"
    );
}

#[test]
fn ensure_running_starts_the_client_and_gives_up_after_the_timeout() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let root = fx.install_steam(&bottle);
    fx.dxvk("1.10.3");
    fx.flag("steam-hangs");

    let started = std::time::Instant::now();
    let err =
        ensure_running(&fx.layout, &bottle, &fx.wine, Duration::from_millis(100)).unwrap_err();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    let message = err.to_string();
    assert!(
        message.starts_with("steam.exe did not report running within 100ms (log: "),
        "{message}"
    );
    assert!(
        message.contains(
            &fx.layout
                .logs_dir()
                .join("steam-steam-")
                .display()
                .to_string()
        ),
        "{message}"
    );
    assert!(
        cef_dir(&bottle).join("d3d11.dll").is_file(),
        "DXVK is put in place first"
    );
    let calls = fx.calls();
    let real_root = fs::canonicalize(&root).unwrap();
    assert!(
        calls.iter().any(|call| call.starts_with(&format!(
            "wine {} -silent -nofriendsui | cwd={} ",
            root.join("steam.exe").display(),
            real_root.display()
        ))),
        "{calls:#?}"
    );
}

#[test]
fn ensure_running_without_dxvk_still_starts_the_client() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    fx.flag("steam-hangs");
    let err = ensure_running(&fx.layout, &bottle, &fx.wine, Duration::ZERO).unwrap_err();
    assert!(matches!(err, Error::Command { .. }), "{err:?}");
    // The client was spawned, not waited for: give it a moment to log.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !fx
        .calls()
        .iter()
        .any(|call| call.contains("-silent -nofriendsui"))
    {
        assert!(std::time::Instant::now() < deadline, "{:#?}", fx.calls());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!cef_dir(&bottle).exists());
}

#[test]
fn ensure_running_needs_steam() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let err = ensure_running(&fx.layout, &bottle, &fx.wine, Duration::ZERO).unwrap_err();
    assert!(
        matches!(
            err,
            Error::NotFound {
                what: "Steam in bottle",
                ..
            }
        ),
        "{err:?}"
    );
}

#[test]
fn stop_is_a_no_op_when_steam_is_not_running() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    fx.set_pid("0x0");
    stop(&bottle, &fx.wine, Duration::from_secs(60)).unwrap();
    assert_eq!(fx.calls().len(), 1, "{:#?}", fx.calls());
}

#[test]
fn stop_kills_the_bottle_when_shutdown_does_not_work_and_forgets_the_client() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let root = fx.install_steam(&bottle);
    fx.set_pid("0x274");

    let started = std::time::Instant::now();
    stop(&bottle, &fx.wine, Duration::from_millis(200)).unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "{:?}",
        started.elapsed()
    );

    let calls = fx.calls();
    let shutdown = format!("wine {} -shutdown |", root.join("steam.exe").display());
    let position = |prefix: &str| {
        calls
            .iter()
            .position(|call| call.starts_with(prefix))
            .unwrap_or_else(|| panic!("no {prefix}: {calls:#?}"))
    };
    let kill = position("wineserver --kill");
    let forget = position(
        r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess /v pid /t REG_DWORD /d 0 /f",
    );
    assert!(position(&shutdown) < kill && kill < forget, "{calls:#?}");
    assert!(fx.state.join("killed").exists());
    assert_eq!(fx.pid().as_deref(), Some("0x0"));
    assert!(!is_running(&bottle, &fx.wine).unwrap());
}

// ----- finding the game -----

fn ron_profile(exe_path: &str) -> uncork_core::profile::GameProfile {
    profile(&format!(
        "schema = 1\nid = \"rise-of-nations-extended-edition\"\nname = \"Rise of Nations\"\n[steam]\nappid = {APPID}\n[exe]\npath = \"{exe_path}\"\nbitness = \"x86\"\napi = \"d3d11\"\n"
    ))
}

#[test]
fn plan_game_picks_the_largest_game_executable() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    let dir = fx.steam_game(
        &bottle,
        APPID,
        "Rise of Nations: Extended Edition",
        "Rise of Nations",
        4,
    );
    let padded = |size: usize| PeBuilder::pe32().data(&vec![0x90; size]);
    padded(1_000).write_to(&dir.join("riseofnations.exe"));
    padded(50_000).write_to(&dir.join("UnityCrashHandler32.exe"));
    padded(60_000).write_to(&dir.join("Launcher.exe"));
    padded(70_000).write_to(&dir.join("unins000.exe"));
    padded(200).write_to(&dir.join("patriots.exe"));
    padded(90_000).write_to(&dir.join("_CommonRedist/game-huge.exe"));
    write(&dir.join("readme.txt"), &[0; 100_000]);

    let plan = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &["-x".to_owned()],
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.command.args[0], dir.join("riseofnations.exe"));
    assert_eq!(plan.command.args[1], "-x");
    assert_eq!(plan.command.cwd.as_deref(), Some(dir.as_path()));
    assert!(
        plan.warnings.iter().all(|w| !w.contains("fully installed")),
        "{:#?}",
        plan.warnings
    );
}

#[test]
fn plan_game_looks_deeper_only_when_the_top_has_no_game() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    let dir = fx.steam_game(&bottle, APPID, "Deep", "Deep", 4);
    PeBuilder::pe64().write_to(&dir.join("vc_redist.x64.exe"));
    PeBuilder::pe64()
        .data(&[0; 5_000])
        .write_to(&dir.join("Game/Binaries/Win64/Game-Win64-Shipping.exe"));
    PeBuilder::pe64().write_to(&dir.join("Game/Binaries/Win64/tool.exe"));
    PeBuilder::pe64()
        .data(&[0; 50_000])
        .write_to(&dir.join("a/b/c/d/too-deep.exe"));

    let plan = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.command.args[0],
        dir.join("Game/Binaries/Win64/Game-Win64-Shipping.exe")
    );

    fs::remove_dir_all(dir.join("Game")).unwrap();
    let err = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(
            &err,
            Error::NotFound {
                what: "game executable",
                ..
            }
        ),
        "the search stops three levels down: {err:?}"
    );
}

#[test]
fn plan_game_follows_the_profile_ignoring_case() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    let dir = fx.steam_game(&bottle, APPID, "Rise of Nations", "Rise of Nations", 4);
    PeBuilder::pe32().write_to(&dir.join("bin/RiseOfNations.exe"));
    PeBuilder::pe32()
        .data(&[0; 90_000])
        .write_to(&dir.join("bigger.exe"));

    let ron = ron_profile("Bin/riseofnations.EXE");
    let plan = plan_game(
        ctx(&fx, &bottle, &[], Some(&ron)),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.command.args[0], dir.join("bin/RiseOfNations.exe"));

    let missing = ron_profile("riseofnations.exe");
    let err = plan_game(
        ctx(&fx, &bottle, &[], Some(&missing)),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap_err();
    let message = err.to_string();
    assert!(
        message.starts_with("game executable \"riseofnations.exe\" not found; profile rise-of-nations-extended-edition names it"),
        "{message}"
    );
}

#[test]
fn plan_game_explains_what_is_missing() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let err = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .ends_with("run: uncork steam install --bottle steam"),
        "{err}"
    );

    fx.install_steam(&bottle);
    let err = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Steam app \"287450\" not found; install it from the Steam library first (bottle steam)"
    );
}

#[test]
fn plan_game_warns_about_games_steam_has_not_finished_installing() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    // StateFlags 2: update required, not fully installed.
    let dir = fx.steam_game(&bottle, APPID, "Rise of Nations", "Rise of Nations", 2);
    PeBuilder::pe32().write_to(&dir.join("riseofnations.exe"));
    let plan = plan_game(
        ctx(&fx, &bottle, &[], None),
        APPID,
        &[],
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.warnings[0],
        "Steam does not list Rise of Nations as fully installed (StateFlags 2); let Steam finish installing or updating it first"
    );
}

// ----- play without polling -----

fn ron_game(fx: &Fixture, bottle: &Bottle) -> PathBuf {
    fx.install_steam(bottle);
    let dir = fx.steam_game(bottle, APPID, "Rise of Nations", "Rise of Nations", 4);
    PeBuilder::pe32().write_to(&dir.join("riseofnations.exe"));
    PeBuilder::pe32()
        .dll()
        .import("d3d11.dll")
        .write_to(&dir.join("d3dgl.dll"));
    dir
}

const RISE2: &str = r"%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini";

fn ron_with(mode: &str) -> uncork_core::profile::GameProfile {
    profile(&format!(
        "schema = 1\nid = \"ron\"\nname = \"RoN\"\n[steam]\nappid = {APPID}\n[exe]\npath = \"riseofnations.exe\"\n[graphics]\nbackend = \"dxmt\"\n[launch]\nmode = \"{mode}\"\nargs = [\"-nointro\"]\n[[ini]]\nfile = '{RISE2}'\nsection = \"RISE OF NATIONS\"\nkey = \"SkipIntroMovies\"\nvalue = \"1\"\n"
    ))
}

#[test]
fn a_dry_run_touches_nothing() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let mut bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    ron_game(&fx, &bottle);
    let rise2 = steam_ini(&bottle);
    write(&rise2, b"[RISE OF NATIONS]\nSkipIntroMovies=0\n");
    let ron = ron_with("direct");

    let outcome = steam::play(
        &fx.layout,
        &mut bottle,
        &fx.wine,
        &components,
        Some(&ron),
        APPID,
        &[],
        &LaunchOptions::default(),
        Duration::from_secs(60),
        true,
    )
    .unwrap();
    assert!(outcome.child.is_none());
    assert!(!outcome.started_steam);
    assert!(outcome.ini_changed.is_empty());
    assert_eq!(outcome.plan.activation.backend, Backend::Dxmt);
    assert!(fx.calls().is_empty(), "{:#?}", fx.calls());
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\nSkipIntroMovies=0\n"
    );
    assert!(!bottle.drive_c().join("windows/syswow64/d3d11.dll").exists());
}

fn steam_ini(bottle: &Bottle) -> PathBuf {
    uncork_core::launch::resolve_profile_path(bottle, None, RISE2).unwrap()
}

#[test]
fn standalone_games_start_without_steam() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let mut bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let dir = ron_game(&fx, &bottle);
    let rise2 = steam_ini(&bottle);
    write(&rise2, b"[RISE OF NATIONS]\nSkipIntroMovies=0\n");
    let ron = ron_with("standalone");

    let mut outcome = steam::play(
        &fx.layout,
        &mut bottle,
        &fx.wine,
        &components,
        Some(&ron),
        APPID,
        &["-window".to_owned()],
        &LaunchOptions::default(),
        Duration::from_secs(60),
        false,
    )
    .unwrap();
    assert!(outcome.child.take().unwrap().wait().unwrap().success());
    assert!(!outcome.started_steam);
    assert_eq!(outcome.ini_changed, std::slice::from_ref(&rise2));
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\nSkipIntroMovies=1\n"
    );
    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:#?}");
    assert!(
        calls[0].starts_with(&format!(
            "wine {} -nointro -window |",
            dir.join("riseofnations.exe").display()
        )),
        "{}",
        calls[0]
    );
    assert!(
        bottle
            .drive_c()
            .join("windows/syswow64/d3d11.dll")
            .is_file()
    );
}

#[test]
fn direct_games_reuse_a_running_client() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let mut bottle = fx.bottle("steam", |_| {});
    let dir = ron_game(&fx, &bottle);
    fx.set_pid("0x274");

    let mut outcome = steam::play(
        &fx.layout,
        &mut bottle,
        &fx.wine,
        &components,
        None,
        APPID,
        &[],
        &LaunchOptions::default(),
        Duration::from_secs(60),
        false,
    )
    .unwrap();
    assert!(outcome.child.take().unwrap().wait().unwrap().success());
    assert!(!outcome.started_steam);
    let calls = fx.calls();
    assert_eq!(calls.len(), 2, "{calls:#?}");
    assert!(calls[0].starts_with("wine reg query"), "{}", calls[0]);
    assert!(
        calls[1].starts_with(&format!(
            "wine {} |",
            dir.join("riseofnations.exe").display()
        )),
        "{}",
        calls[1]
    );
    assert!(
        calls[1].contains(" DXMT_LOG_LEVEL=none "),
        "auto picked DXMT: {}",
        calls[1]
    );
}
