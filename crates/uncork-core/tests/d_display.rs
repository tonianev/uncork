//! Retina mode, DPI and the main display through the public API with a
//! fake Wine: registry changes made with the bottle stopped, the session
//! display a launch records, restarting a bottle whose main display
//! changed, the frame cap, MetalFX in a Retina bottle, and `{display.*}`
//! placeholders in profile INI values.

mod d_support;
#[path = "../../uncork-pe/tests/common/mod.rs"]
mod pe;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use d_support::{CX_FEATURES, Fixture, ctx, profile, write};
use pe::PeBuilder;
use uncork_core::bottle::{self, Bottle, DisplayRegistry};
use uncork_core::display::Display;
use uncork_core::graphics::{Backend, BackendChoice};
use uncork_core::launch::{self, LaunchOptions, LaunchPlan, PlanContext, Target};
use uncork_core::profile::GameProfile;
use uncork_core::steam;

const APPID: u32 = 287_450;

/// A 16-inch `MacBook` Pro's built-in display.
fn built_in() -> Display {
    Display {
        name: "Color LCD".to_owned(),
        points: (1728, 1117),
        pixels: Some((3456, 2234)),
        refresh_hz: Some(120.0),
        main: true,
        built_in: true,
    }
}

/// A 5K external display at 60 Hz.
fn external() -> Display {
    Display {
        name: "LG UltraFine".to_owned(),
        points: (2560, 1440),
        pixels: Some((5120, 2880)),
        refresh_hz: Some(60.0),
        main: true,
        built_in: false,
    }
}

const BUILT_IN: &str = "Color LCD 1728x1117 @120Hz";
const EXTERNAL: &str = "LG UltraFine 2560x1440 @60Hz";

/// A 32-bit D3D11 game like Rise of Nations in its own directory.
fn ron_like(dir: &Path) -> PathBuf {
    let game = dir.join("Rise of Nations");
    fs::create_dir_all(&game).unwrap();
    let exe = game.join("riseofnations.exe");
    PeBuilder::pe32().write(&exe);
    PeBuilder::pe32()
        .dll()
        .import("d3d11.dll")
        .write(&game.join("d3dgl.dll"));
    exe
}

fn target(path: &Path) -> Target {
    Target::Exe {
        path: path.to_path_buf(),
        args: Vec::new(),
    }
}

const RISE2: &str = r"%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini";

/// A Rise of Nations profile with `extra` appended.
fn ron(extra: &str) -> GameProfile {
    profile(&format!(
        "schema = 1\nid = \"ron\"\nname = \"RoN\"\n[steam]\nappid = {APPID}\n[exe]\npath = \"riseofnations.exe\"\nbitness = \"x86\"\napi = \"d3d11\"\n[graphics]\nbackend = \"dxmt\"\n{extra}"
    ))
}

/// `[[ini]]` entries for the borderless desktop-size window.
fn display_ini() -> String {
    let mut text = String::new();
    for (key, value) in [
        ("Fullscreen", "2"),
        ("Windowed Width", "{display.width}"),
        ("Windowed Height", "{display.height}"),
    ] {
        text.push_str(&format!(
            "[[ini]]\nfile = '{RISE2}'\nsection = \"RISE OF NATIONS\"\nkey = \"{key}\"\nvalue = \"{value}\"\n"
        ));
    }
    text
}

fn plan_with(
    fx: &Fixture,
    bottle: &Bottle,
    profile: Option<&GameProfile>,
    display: Option<&Display>,
    options: &LaunchOptions,
) -> LaunchPlan {
    let components = fx.components();
    let exe = ron_like(fx.dir.path());
    launch::plan(
        PlanContext {
            display,
            ..ctx(fx, bottle, &components, profile)
        },
        &target(&exe),
        options,
    )
    .unwrap()
}

fn position(calls: &[String], needle: &str) -> usize {
    calls
        .iter()
        .position(|call| call.contains(needle))
        .unwrap_or_else(|| panic!("no call contains {needle:?}: {calls:#?}"))
}

// ----- registry changes with the bottle stopped -----

#[test]
fn registry_changes_in_a_stopped_bottle_wait_for_their_own_wineserver() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| config.performance.retina = true);
    let keys = bottle::default_registry(&bottle.config);

    let stopped = bottle::apply_registry_stopped(&bottle, &fx.wine, &keys).unwrap();

    assert!(!stopped);
    let calls = fx.calls();
    assert_eq!(calls.len(), 3, "{calls:#?}");
    assert!(calls[0].starts_with("wineserver -k0 |"), "{calls:#?}");
    assert!(
        calls[1].starts_with(r"wine regedit /S C:\windows\temp\uncork-"),
        "{calls:#?}"
    );
    assert!(
        calls[1].contains(" WINEMSYNC=1 "),
        "with the bottle's environment: {}",
        calls[1]
    );
    assert!(calls[2].starts_with("wineserver --wait |"), "{calls:#?}");
    assert!(
        !fx.state.join("server").exists(),
        "no wineserver outlives the change"
    );
}

#[test]
fn registry_changes_stop_whatever_runs_in_the_bottle_first() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.flag("server");

    let stopped = bottle::apply_registry_stopped(
        &bottle,
        &fx.wine,
        &bottle::default_registry(&bottle.config),
    )
    .unwrap();

    assert!(stopped);
    let calls = fx.calls();
    let kill = position(&calls, "wineserver --kill");
    let regedit = position(&calls, "wine regedit /S");
    assert!(kill < regedit, "{calls:#?}");
    assert!(
        calls[kill + 1].starts_with("wineserver --wait"),
        "the killed wineserver is gone before regedit runs: {calls:#?}"
    );
    assert!(
        calls.last().unwrap().starts_with("wineserver --wait"),
        "{calls:#?}"
    );
    assert!(
        !calls.iter().any(|call| call.contains("reg add")),
        "no Steam, no running marker to reset: {calls:#?}"
    );
    assert!(fx.state.join("killed").exists());
}

#[test]
fn stopping_a_bottle_with_steam_installed_resets_its_running_marker() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    fx.install_steam(&bottle);
    assert!(!steam::stop_bottle(&bottle, &fx.wine, Duration::ZERO).unwrap());
    assert_eq!(fx.calls().len(), 1, "only the probe: {:#?}", fx.calls());

    // A wineserver runs, but no Steam client (no pid).
    fx.flag("server");
    assert!(steam::stop_bottle(&bottle, &fx.wine, Duration::ZERO).unwrap());
    let calls = fx.calls();
    let kill = position(&calls, "wineserver --kill");
    let forget = position(
        &calls,
        r"wine reg add HKCU\Software\Valve\Steam\ActiveProcess",
    );
    assert!(kill < forget, "{calls:#?}");
    assert!(
        calls.last().unwrap().starts_with("wineserver --wait"),
        "{calls:#?}"
    );
    assert_eq!(fx.pid().as_deref(), Some("0x0"));
}

// ----- the session display -----

#[test]
fn a_display_change_needs_known_displays_and_a_running_bottle() {
    let fx = Fixture::new(CX_FEATURES);
    let mut bottle = fx.bottle("steam", |_| {});
    let change = |bottle: &Bottle, display: Option<&Display>| {
        steam::display_change(bottle, &fx.wine, display).unwrap()
    };

    assert_eq!(change(&bottle, Some(&external())), None, "nothing recorded");
    bottle.state.session_display = Some(BUILT_IN.to_owned());
    assert_eq!(change(&bottle, None), None, "a failed probe never restarts");
    assert_eq!(change(&bottle, Some(&built_in())), None, "unchanged");
    assert!(
        fx.calls().is_empty(),
        "the wineserver is asked only when the display changed: {:#?}",
        fx.calls()
    );
    assert_eq!(
        change(&bottle, Some(&external())),
        None,
        "nothing runs, so the next session starts fresh"
    );
    assert_eq!(fx.calls().len(), 1);

    fx.flag("server");
    let found = change(&bottle, Some(&external())).unwrap();
    assert_eq!(
        (found.before.as_str(), found.now.as_str()),
        (BUILT_IN, EXTERNAL)
    );
}

#[test]
fn a_launch_that_starts_the_session_records_the_main_display() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let mut bottle = fx.bottle("steam", |_| {});
    let plan = plan_with(
        &fx,
        &bottle,
        None,
        Some(&built_in()),
        &LaunchOptions::default(),
    );
    assert_eq!(plan.display.as_deref(), Some(BUILT_IN));

    let mut child = launch::execute(&plan, &mut bottle, &fx.wine).unwrap();
    child.wait().unwrap();
    assert_eq!(bottle.state.session_display.as_deref(), Some(BUILT_IN));
    assert_eq!(
        Bottle::open(&bottle.path)
            .unwrap()
            .state
            .session_display
            .as_deref(),
        Some(BUILT_IN),
        "saved"
    );

    // A second program joins the running session: nothing is recorded.
    let plan = plan_with(
        &fx,
        &bottle,
        None,
        Some(&external()),
        &LaunchOptions::default(),
    );
    let mut child = launch::execute(&plan, &mut bottle, &fx.wine).unwrap();
    child.wait().unwrap();
    assert_eq!(bottle.state.session_display.as_deref(), Some(BUILT_IN));
}

/// Steam with Rise of Nations installed in `bottle`; returns `rise2.ini`.
fn ron_in_steam(fx: &Fixture, bottle: &Bottle) -> PathBuf {
    fx.install_steam(bottle);
    let dir = fx.steam_game(bottle, APPID, "Rise of Nations", "Rise of Nations", 4);
    ron_like(fx.dir.path());
    fs::copy(
        fx.dir.path().join("Rise of Nations/riseofnations.exe"),
        dir.join("riseofnations.exe"),
    )
    .unwrap();
    let rise2 = launch::resolve_profile_path(bottle, None, RISE2).unwrap();
    write(
        &rise2,
        b"[RISE OF NATIONS]\r\nFullscreen=3\r\nWindowed Width=1024\r\n",
    );
    rise2
}

fn play(
    fx: &Fixture,
    bottle: &mut Bottle,
    profile: &GameProfile,
    display: Option<&Display>,
    dry_run: bool,
) -> steam::PlayOutcome {
    let components = fx.components();
    steam::play(
        &fx.layout,
        bottle,
        &fx.wine,
        &components,
        Some(profile),
        display,
        &mut |_| true,
        APPID,
        &[],
        &LaunchOptions::default(),
        Duration::from_secs(60),
        dry_run,
    )
    .unwrap()
}

#[test]
fn play_restarts_a_bottle_whose_main_display_changed() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let mut bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let rise2 = ron_in_steam(&fx, &bottle);
    let profile = ron(&format!(
        "[launch]\nmode = \"standalone\"\n{}",
        display_ini()
    ));
    bottle.record_session_display(Some(BUILT_IN));
    fx.flag("server");

    // A dry run says so and touches nothing.
    let outcome = play(&fx, &mut bottle, &profile, Some(&external()), true);
    assert_eq!(
        outcome.display_change.as_ref().map(|c| c.now.as_str()),
        Some(EXTERNAL)
    );
    assert!(
        outcome.plan.warnings.iter().any(|w| w.contains(
            "the main display changed since bottle steam started (Color LCD 1728x1117 @120Hz → LG UltraFine 2560x1440 @60Hz)"
        )),
        "{:#?}",
        outcome.plan.warnings
    );
    assert!(!fx.calls().iter().any(|call| call.contains("--kill")));

    let mut outcome = play(&fx, &mut bottle, &profile, Some(&external()), false);
    outcome.child.take().unwrap().wait().unwrap();
    assert!(outcome.display_change.is_some());
    assert!(outcome.restarted);
    let calls = fx.calls();
    let kill = position(&calls, "wineserver --kill");
    let game = position(&calls, "riseofnations.exe |");
    assert!(kill < game, "{calls:#?}");
    assert_eq!(
        bottle.state.session_display.as_deref(),
        Some(EXTERNAL),
        "the game started the new session"
    );
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\r\nFullscreen=2\r\nWindowed Width=2560\r\nWindowed Height=1440\r\n",
        "the INI follows the new main display"
    );
}

#[test]
fn an_unknown_display_never_restarts_the_bottle() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let mut bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let rise2 = ron_in_steam(&fx, &bottle);
    let profile = ron(&format!(
        "[launch]\nmode = \"standalone\"\n{}",
        display_ini()
    ));
    bottle.record_session_display(Some(BUILT_IN));
    fx.flag("server");

    let mut outcome = play(&fx, &mut bottle, &profile, None, false);
    outcome.child.take().unwrap().wait().unwrap();
    assert_eq!(outcome.display_change, None);
    assert!(!fx.calls().iter().any(|call| call.contains("--kill")));
    assert_eq!(bottle.state.session_display.as_deref(), Some(BUILT_IN));
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\r\nFullscreen=2\r\nWindowed Width=1024\r\n",
        "keys that need the display are left alone"
    );
    assert!(
        outcome
            .plan
            .warnings
            .iter()
            .any(|w| w.contains("the main display is unknown")
                && w.contains("leaves Windowed Width, Windowed Height as they are")),
        "{:#?}",
        outcome.plan.warnings
    );
}

// ----- the frame cap -----

fn dxmt_config(plan: &LaunchPlan) -> Option<&str> {
    plan.command.env.get("DXMT_CONFIG").map(String::as_str)
}

#[test]
fn the_frame_cap_follows_the_main_display() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let bottle = fx.bottle("steam", |_| {});
    let plan = |display: Option<Display>| {
        plan_with(
            &fx,
            &bottle,
            None,
            display.as_ref(),
            &LaunchOptions::default(),
        )
    };

    let on_built_in = plan(Some(built_in()));
    assert_eq!(on_built_in.activation.backend, Backend::Dxmt);
    assert_eq!(
        dxmt_config(&on_built_in),
        Some("d3d11.preferredMaxFrameRate=120")
    );
    let cap = on_built_in.frame_cap.unwrap();
    assert_eq!(cap.fps, Some(120));
    assert_eq!(
        cap.source,
        "the main display's refresh rate (Color LCD 1728x1117 @120Hz)"
    );
    assert!(!cap.overridden);

    let on_external = plan(Some(external()));
    assert_eq!(
        dxmt_config(&on_external),
        Some("d3d11.preferredMaxFrameRate=60")
    );

    let unknown = plan(None);
    assert_eq!(
        dxmt_config(&unknown),
        Some("d3d11.preferredMaxFrameRate=60")
    );
    assert_eq!(
        unknown.frame_cap.unwrap().source,
        "the default (the main display's refresh rate is unknown)"
    );
    let no_refresh = Display {
        refresh_hz: None,
        ..external()
    };
    assert_eq!(
        dxmt_config(&plan(Some(no_refresh))),
        Some("d3d11.preferredMaxFrameRate=60")
    );
}

#[test]
fn an_explicit_frame_cap_wins_and_zero_uncaps() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let capped = fx.bottle("capped", |config| config.performance.max_fps = Some(60));
    let options = LaunchOptions::default();

    let plan = plan_with(&fx, &capped, None, Some(&built_in()), &options);
    assert_eq!(dxmt_config(&plan), Some("d3d11.preferredMaxFrameRate=60"));
    assert_eq!(
        plan.frame_cap.as_ref().unwrap().source,
        "performance.max_fps in bottle capped"
    );
    assert!(
        !plan.warnings.iter().any(|w| w.contains("frame cap")),
        "60 divides 120: {:#?}",
        plan.warnings
    );

    let uncapped = ron("[performance]\nmax_fps = 0\n");
    let plan = plan_with(&fx, &capped, Some(&uncapped), Some(&built_in()), &options);
    assert_eq!(dxmt_config(&plan), None, "the profile's 0 wins");
    let cap = plan.frame_cap.unwrap();
    assert_eq!(cap.fps, None);
    assert_eq!(cap.source, "performance.max_fps in profile ron");

    let odd = ron("[performance]\nmax_fps = 50\n");
    let plan = plan_with(&fx, &capped, Some(&odd), Some(&built_in()), &options);
    assert_eq!(dxmt_config(&plan), Some("d3d11.preferredMaxFrameRate=50"));
    assert!(
        plan.warnings.iter().any(|w| w.contains(
            "a frame cap of 50 FPS (performance.max_fps in profile ron) does not divide the main display's 120 Hz"
        )),
        "{:#?}",
        plan.warnings
    );
}

#[test]
fn dxmt_config_from_the_user_replaces_the_frame_cap() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let bottle = fx.bottle("steam", |_| {});
    let options = LaunchOptions {
        env: [(
            "DXMT_CONFIG".to_owned(),
            "d3d11.metalSpatialUpscaleFactor=2;".to_owned(),
        )]
        .into(),
        ..LaunchOptions::default()
    };
    let plan = plan_with(&fx, &bottle, None, Some(&built_in()), &options);
    assert_eq!(
        dxmt_config(&plan),
        Some("d3d11.metalSpatialUpscaleFactor=2;")
    );
    assert!(plan.frame_cap.unwrap().overridden);
}

#[test]
fn only_dxmt_gets_a_frame_cap() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| config.performance.max_fps = Some(60));
    let options = LaunchOptions {
        backend: Some(BackendChoice::Fixed(Backend::Wined3d)),
        ..LaunchOptions::default()
    };
    let plan = plan_with(&fx, &bottle, None, Some(&built_in()), &options);
    assert_eq!(plan.frame_cap, None);
    assert_eq!(dxmt_config(&plan), None);
}

// ----- MetalFX in a Retina bottle -----

#[test]
fn metalfx_stays_off_on_dxmt_in_a_retina_bottle() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let options = LaunchOptions {
        metalfx: true,
        ..LaunchOptions::default()
    };
    let retina = fx.bottle("retina", |config| config.performance.retina = true);
    let plan = plan_with(&fx, &retina, None, Some(&built_in()), &options);
    assert!(
        !plan
            .command
            .env
            .contains_key("DXMT_METALFX_SPATIAL_SWAPCHAIN")
    );
    assert!(
        plan.warnings.iter().any(|w| w.starts_with(
            "MetalFX upscaling stays off for this launch: bottle retina has Retina mode on"
        )),
        "{:#?}",
        plan.warnings
    );

    let plain = fx.bottle("plain", |_| {});
    let plan = plan_with(&fx, &plain, None, Some(&built_in()), &options);
    assert_eq!(plan.command.env["DXMT_METALFX_SPATIAL_SWAPCHAIN"], "1");
    assert!(!plan.warnings.iter().any(|w| w.contains("MetalFX")));
}

// ----- Retina mode and DPI on disk -----

#[test]
fn plans_warn_when_retina_mode_and_dpi_disagree() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let bottle = fx.bottle("cx", |_| {});
    let warning = |bottle: &Bottle| {
        plan_with(&fx, bottle, None, None, &LaunchOptions::default())
            .warnings
            .into_iter()
            .find(|w| w.starts_with("Retina mode and DPI disagree"))
    };
    assert_eq!(warning(&bottle), None, "no user.reg");

    write(
        &bottle.path.join("user.reg"),
        b"WINE REGISTRY Version 2\n\n[Control Panel\\\\Desktop] 1\n\"LogPixels\"=dword:000000c0\n\n[Software\\\\Wine\\\\Mac Driver] 1\n\"RetinaMode\"=\"n\"\n",
    );
    assert_eq!(
        DisplayRegistry::read(&bottle.path).unwrap().describe(),
        "RetinaMode n, LogPixels 192"
    );
    let found = warning(&bottle).unwrap();
    assert!(
        found.contains("bottle cx (RetinaMode n, LogPixels 192)")
            && found.ends_with("`uncork bottle set cx performance.retina=false`"),
        "{found}"
    );
}

// ----- INI placeholders -----

#[test]
fn display_placeholders_resolve_from_the_main_display() {
    let resolve =
        |value: &str, display: Option<&Display>| launch::resolve_ini_value(value, display);
    let display = built_in();
    assert_eq!(
        resolve(
            "{display.width}x{display.height}@{display.refresh}",
            Some(&display)
        )
        .as_deref(),
        Some("1728x1117@120")
    );
    assert_eq!(resolve("2", None).as_deref(), Some("2"));
    assert_eq!(resolve("{display.width}", None), None);
    assert_eq!(
        resolve(
            "{display.refresh}",
            Some(&Display {
                refresh_hz: None,
                ..display.clone()
            })
        ),
        None
    );
    assert_eq!(
        resolve("{other} {display.width", Some(&display)).as_deref(),
        Some("{other} {display.width"),
        "other braces stay as written"
    );
}

#[test]
fn ini_placeholders_are_written_with_the_main_display() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let rise2 = launch::resolve_profile_path(&bottle, None, RISE2).unwrap();
    write(&rise2, b"[RISE OF NATIONS]\nFullscreen=1\n");
    let profile = ron(&display_ini());

    let changed = launch::apply_profile_ini(&profile, &bottle, None, None).unwrap();
    assert_eq!(changed, std::slice::from_ref(&rise2));
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\nFullscreen=2\n",
        "without a display only the plain value is written"
    );

    launch::apply_profile_ini(&profile, &bottle, None, Some(&built_in())).unwrap();
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\nFullscreen=2\nWindowed Width=1728\nWindowed Height=1117\n"
    );
    assert_eq!(
        launch::apply_profile_ini(&profile, &bottle, None, Some(&built_in())).unwrap(),
        Vec::<PathBuf>::new(),
        "idempotent"
    );
}

#[test]
fn the_built_in_rise_of_nations_profile_sizes_the_window_to_the_display() {
    let profiles = uncork_core::profile::builtin();
    let ron = profiles
        .iter()
        .find(|profile| profile.id == "rise-of-nations-extended-edition")
        .unwrap();
    let sets: Vec<(&str, &str)> = ron
        .ini
        .iter()
        .map(|ini| (ini.key.as_str(), ini.value.as_str()))
        .collect();
    assert_eq!(
        sets,
        [
            ("SkipIntroMovies", "1"),
            ("Fullscreen", "2"),
            ("Windowed Width", "{display.width}"),
            ("Windowed Height", "{display.height}"),
            ("IgnoreMinimizeOnTabOut", "1"),
        ]
    );
    assert!(ron.ini.iter().all(|ini| ini.file == RISE2
        && ini.section == "RISE OF NATIONS"
        && !ini.reason.is_empty()));
}
