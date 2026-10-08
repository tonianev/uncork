//! Launch planning and execution through the public API: environment
//! layering, DLL overrides, backend precedence (command line > profile >
//! bottle > automatic), automatic choice from real PE files, warnings, log
//! paths, file copies, and profile INI paths.

mod d_support;
#[path = "../../uncork-pe/tests/common/mod.rs"]
mod pe;

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use d_support::{CX_FEATURES, Fixture, builtin_dll, ctx, profile, write};
use pe::PeBuilder;
use uncork_core::Error;
use uncork_core::bottle::Bottle;
use uncork_core::graphics::{Backend, BackendChoice, Strategy};
use uncork_core::launch::{
    self, LaunchOptions, LaunchPlan, Target, apply_profile_ini, base_env, choose_backend,
    resolve_profile_path,
};

/// A 32-bit D3D11 game like Rise of Nations: the executable imports
/// nothing graphical, its renderer DLL beside it imports `d3d11.dll`.
fn ron_like(dir: &Path) -> PathBuf {
    let game = dir.join("Rise of Nations");
    fs::create_dir_all(&game).unwrap();
    let exe = game.join("riseofnations.exe");
    PeBuilder::pe32()
        .large_address_aware(false)
        .import("kernel32.dll")
        .write(&exe);
    PeBuilder::pe32()
        .dll()
        .import("d3d11.dll")
        .import("d3dcompiler_47.dll")
        .write(&game.join("d3dgl.dll"));
    exe
}

/// A game executable built by `builder` in its own directory.
fn game(dir: &Path, name: &str, builder: &PeBuilder) -> PathBuf {
    let exe = dir.join(name).join(format!("{name}.exe"));
    fs::create_dir_all(exe.parent().unwrap()).unwrap();
    builder.write(&exe);
    exe
}

fn target(path: &Path) -> Target {
    Target::Exe {
        path: path.to_path_buf(),
        args: Vec::new(),
    }
}

fn fixed(backend: Backend) -> LaunchOptions {
    LaunchOptions {
        backend: Some(BackendChoice::Fixed(backend)),
        ..LaunchOptions::default()
    }
}

fn args(plan: &LaunchPlan) -> Vec<String> {
    plan.command
        .args
        .iter()
        .map(|arg| arg.to_string_lossy().into_owned())
        .collect()
}

fn env(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

fn unsupported(err: Error) -> String {
    match err {
        Error::Unsupported(message) => message,
        other => panic!("expected Error::Unsupported, got {other:?}"),
    }
}

const RON_PROFILE: &str = r#"
schema = 1
id = "rise-of-nations-extended-edition"
name = "Rise of Nations: Extended Edition"
[steam]
appid = 287450
[exe]
path = "riseofnations.exe"
bitness = "x86"
api = "d3d11"
geometry_shaders = true
[graphics]
backend = "dxmt"
fallbacks = ["wined3d"]
[env]
WINE_LARGE_ADDRESS_AWARE = "1"
[dll_overrides]
d3dcompiler_47 = "n,b"
"#;

// ----- base environment -----

#[test]
fn base_env_is_exactly_the_wine_basics() {
    let fx = Fixture::new(&["msync", "wow64"]);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let prefix = bottle.path.to_string_lossy().into_owned();
    assert_eq!(
        base_env(&bottle, &fx.wine, None),
        env(&[
            ("MVK_CONFIG_LOG_LEVEL", "1"),
            ("ROSETTA_ADVERTISE_AVX", "1"),
            ("WINEDEBUG", "-all"),
            ("WINEMSYNC", "1"),
            ("WINEPREFIX", &prefix),
        ]),
        "the bottle's own env is not part of it"
    );
    assert_eq!(
        base_env(&bottle, &fx.wine, Some("+loaddll,+d3d"))["WINEDEBUG"],
        "+loaddll,+d3d"
    );
    assert_eq!(base_env(&bottle, &fx.wine, Some("  "))["WINEDEBUG"], "-all");
}

#[test]
fn base_env_follows_the_bottle_and_the_runtime() {
    let fx = Fixture::new(&["wow64"]);
    let bottle = fx.bottle("steam", |_| {});
    assert!(
        !base_env(&bottle, &fx.wine, None).contains_key("WINEMSYNC"),
        "a runtime without msync never gets WINEMSYNC"
    );

    let fx = Fixture::new(&["msync"]);
    let bottle = fx.bottle("steam", |config| {
        config.performance.msync = false;
        config.performance.avx = false;
    });
    let env = base_env(&bottle, &fx.wine, None);
    assert!(!env.contains_key("WINEMSYNC"));
    assert!(!env.contains_key("ROSETTA_ADVERTISE_AVX"));
    assert_eq!(env["MVK_CONFIG_LOG_LEVEL"], "1");
}

#[test]
fn wine_tools_share_the_bottle_environment() {
    let fx = Fixture::new(&["msync"]);
    let bottle = fx.bottle("cx", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
        config
            .env
            .insert("WINEDLLOVERRIDES".to_owned(), "dinput8=n,b".to_owned());
    });
    let boot = launch::in_bottle(fx.wine.boot_init_command(&bottle.path), &bottle, &fx.wine);
    assert!(boot.env_clear);
    assert_eq!(
        boot.env["WINEMSYNC"], "1",
        "wineboot must match later clients"
    );
    assert_eq!(boot.env["USER"], "crossover");
    assert_eq!(boot.env["WINEPREFIX"], bottle.path.to_str().unwrap());
    assert!(
        boot.env["WINEDLLOVERRIDES"].contains("mscoree"),
        "the tool's own overrides win over the bottle's game overrides: {}",
        boot.env["WINEDLLOVERRIDES"]
    );
    assert_eq!(boot.args, fx.wine.boot_init_command(&bottle.path).args);
}

// ----- the command -----

#[test]
fn plans_the_documented_command() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let exe = ron_like(fx.dir.path());
    let ron = profile(&format!("{RON_PROFILE}\n[launch]\nargs = [\"-nointro\"]\n"));
    let target = Target::Exe {
        path: exe.clone(),
        args: vec!["-window".to_owned(), "a b".to_owned()],
    };

    let plan = launch::plan(
        ctx(&fx, &bottle, &components, Some(&ron)),
        &target,
        &LaunchOptions::default(),
    )
    .unwrap();

    assert_eq!(plan.command.program, fx.wine.wine_bin());
    assert_eq!(
        args(&plan),
        [exe.to_str().unwrap(), "-nointro", "-window", "a b"],
        "exe, then the profile's arguments, then the caller's"
    );
    assert_eq!(plan.command.cwd.as_deref(), exe.parent());
    assert!(plan.command.env_clear);
    assert_eq!(plan.command.log.as_ref(), Some(&plan.log));
    let log_name = plan.log.file_name().unwrap().to_str().unwrap();
    assert_eq!(plan.log.parent(), Some(fx.layout.logs_dir().as_path()));
    let secs = log_name
        .strip_prefix("steam-riseofnations-")
        .and_then(|rest| rest.strip_suffix(".log"))
        .unwrap_or_else(|| panic!("{log_name}"));
    assert!(secs.parse::<u64>().unwrap() > 1_700_000_000, "{log_name}");
    assert_eq!(plan.activation.backend, Backend::Dxmt);
    assert_eq!(plan.activation.strategy, Strategy::PrefixNative);
}

#[test]
fn log_names_are_sanitized() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| config.name = "my bottle".to_owned());
    let exe = game(fx.dir.path(), "Game (Final)", &PeBuilder::pe64());
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    let name = plan.log.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with("my_bottle-Game__Final_-"), "{name}");
}

#[test]
fn wine_programs_run_on_wined3d_without_a_working_directory() {
    let fx = Fixture::new(CX_FEATURES);
    // A fixed bottle backend is for games; it must not break winecfg even
    // when it is not installed.
    let bottle = fx.bottle("steam", |config| {
        config.graphics.backend = BackendChoice::Fixed(Backend::Dxmt);
    });
    let target = Target::WineProgram {
        name: r"C:\windows\system32\winecfg.exe".to_owned(),
        args: vec!["/v".to_owned()],
    };
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], None),
        &target,
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(args(&plan), [r"C:\windows\system32\winecfg.exe", "/v"]);
    assert_eq!(plan.command.cwd, None);
    assert_eq!(plan.activation.backend, Backend::Wined3d);
    assert_eq!(plan.activation.env["WINE_D3D_CONFIG"], "renderer=gl");
    let name = plan.log.file_name().unwrap().to_str().unwrap().to_owned();
    assert!(name.starts_with("steam-winecfg-"), "{name}");

    // An explicit --backend is honored.
    fx.dxvk("1.10.3");
    let components = fx.components();
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, None),
        &target,
        &fixed(Backend::Dxvk),
    )
    .unwrap();
    assert_eq!(plan.activation.backend, Backend::Dxvk);
}

#[test]
fn game_mode_wraps_the_plan_when_the_bottle_or_the_command_line_asks() {
    let fx = Fixture::new(CX_FEATURES);
    let target = Target::WineProgram {
        name: "winecfg".to_owned(),
        args: Vec::new(),
    };

    let off = fx.bottle("steam", |_| {});
    let plan = launch::plan(
        ctx(&fx, &off, &[], None),
        &target,
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.game_mode, None, "game mode is off by default");

    let forced = LaunchOptions {
        game_mode: Some(true),
        ..LaunchOptions::default()
    };
    let plan = launch::plan(ctx(&fx, &off, &[], None), &target, &forced).unwrap();
    let game_mode = plan.game_mode.expect("--game-mode turns it on");
    assert_eq!(game_mode.root, fx.layout.root());
    assert_eq!(game_mode.id, "steam-winecfg");
    assert_eq!(game_mode.name, "winecfg");

    let on = fx.bottle("Games 2", |config| config.performance.game_mode = true);
    let plan = launch::plan(ctx(&fx, &on, &[], None), &target, &LaunchOptions::default()).unwrap();
    assert_eq!(plan.game_mode.unwrap().id, "games-2-winecfg");

    let declined = LaunchOptions {
        game_mode: Some(false),
        ..LaunchOptions::default()
    };
    let plan = launch::plan(ctx(&fx, &on, &[], None), &target, &declined).unwrap();
    assert_eq!(
        plan.game_mode, None,
        "--game-mode=false wins over the bottle"
    );
}

#[test]
fn a_missing_executable_needs_a_profile_that_declares_it() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let missing = fx.dir.path().join("nowhere/game.exe");
    let err = launch::plan(
        ctx(&fx, &bottle, &[], None),
        &target(&missing),
        &LaunchOptions::default(),
    )
    .unwrap_err();
    assert!(
        matches!(&err, Error::NotFound { what: "program", name, .. } if name.ends_with("game.exe")),
        "{err:?}"
    );

    let ron = profile(RON_PROFILE);
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], Some(&ron)),
        &target(&missing),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.activation.backend,
        Backend::Wined3d,
        "DXMT is not installed"
    );
    assert!(
        plan.warnings.iter().any(|w| w.contains("does not exist")),
        "{:#?}",
        plan.warnings
    );
}

#[test]
fn a_file_that_is_not_a_pe_is_a_scan_error_unless_the_profile_knows_better() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let fake = fx.dir.path().join("game/game.exe");
    write(&fake, b"#!/bin/sh\n");
    let err = launch::plan(
        ctx(&fx, &bottle, &[], None),
        &target(&fake),
        &LaunchOptions::default(),
    )
    .unwrap_err();
    assert!(matches!(err, Error::Pe(_)), "{err:?}");

    let ron = profile(RON_PROFILE);
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], Some(&ron)),
        &target(&fake),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.starts_with("cannot inspect")),
        "{:#?}",
        plan.warnings
    );
}

// ----- environment layering -----

#[test]
fn environment_layers_override_in_order() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |config| {
        config.env = env(&[
            ("LAYER_A", "bottle"),
            ("LAYER_B", "bottle"),
            ("DXMT_LOG_LEVEL", "info"),
            ("USER", "crossover"),
        ]);
    });
    let mut game_profile = profile(RON_PROFILE);
    game_profile.env = env(&[("LAYER_B", "profile"), ("LAYER_C", "profile")]);
    let options = LaunchOptions {
        env: env(&[("LAYER_C", "cli"), ("MVK_CONFIG_LOG_LEVEL", "4")]),
        ..LaunchOptions::default()
    };
    let exe = ron_like(fx.dir.path());

    let plan = launch::plan(
        ctx(&fx, &bottle, &components, Some(&game_profile)),
        &target(&exe),
        &options,
    )
    .unwrap();
    let env = &plan.command.env;
    assert_eq!(env["LAYER_A"], "bottle");
    assert_eq!(env["LAYER_B"], "profile");
    assert_eq!(env["LAYER_C"], "cli");
    assert_eq!(
        env["MVK_CONFIG_LOG_LEVEL"], "4",
        "--env beats the base layer"
    );
    assert_eq!(
        env["DXMT_LOG_LEVEL"], "info",
        "the bottle beats the activation"
    );
    assert_eq!(env["USER"], "crossover");
    assert_eq!(env["WINEMSYNC"], "1");
    assert_eq!(env["WINEPREFIX"], bottle.path.to_str().unwrap());
    assert_eq!(
        env["DXMT_SHADER_CACHE_PATH"],
        bottle.path.join("cache/dxmt").to_str().unwrap()
    );
    assert!(!env.contains_key("PATH"), "PATH comes from env_clear");
    assert!(
        !env.contains_key("HOME"),
        "the parent environment is not copied into the plan"
    );
}

#[test]
fn dll_overrides_layer_per_dll() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |config| {
        config.dll_overrides = env(&[("d3dcompiler_47", "n,b"), ("dxgi", "b")]);
    });
    let mut game_profile = profile(RON_PROFILE);
    game_profile.dll_overrides = env(&[("d3dcompiler_47", "b"), ("gameoverlayrenderer", "")]);
    let options = LaunchOptions {
        env: env(&[("WINEDLLOVERRIDES", "D3D10=n;bogus")]),
        ..LaunchOptions::default()
    };
    let exe = ron_like(fx.dir.path());

    let plan = launch::plan(
        ctx(&fx, &bottle, &components, Some(&game_profile)),
        &target(&exe),
        &options,
    )
    .unwrap();
    assert_eq!(
        plan.command.env["WINEDLLOVERRIDES"],
        "gameoverlayrenderer=;d3d12,d3d12core,d3dcompiler_47,dxgi=b;winemenubuilder.exe=d;d3d10=n;d3d10core,d3d11=n,b"
    );
}

#[test]
fn performance_settings_combine_bottle_profile_and_command_line() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let game_profile = profile(&format!(
        "{RON_PROFILE}\n[performance]\nmetalfx = true\navx = false\n"
    ));
    let options = LaunchOptions {
        hud: true,
        wine_debug: Some("+loaddll".to_owned()),
        ..LaunchOptions::default()
    };
    let exe = ron_like(fx.dir.path());
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, Some(&game_profile)),
        &target(&exe),
        &options,
    )
    .unwrap();
    let env = &plan.command.env;
    assert_eq!(env["DXMT_METALFX_SPATIAL_SWAPCHAIN"], "1");
    assert_eq!(env["MTL_HUD_ENABLED"], "1");
    assert!(
        !env.contains_key("ROSETTA_ADVERTISE_AVX"),
        "the profile turns AVX off"
    );
    assert_eq!(env["WINEDEBUG"], "+loaddll");
    assert_eq!(
        env["DXMT_LOG_LEVEL"], "info",
        "debug channels turn backend logs on"
    );
    assert_eq!(env["WINEMSYNC"], "1", "msync stays the bottle's");
}

// ----- backend precedence -----

#[test]
fn backend_precedence_is_command_line_profile_bottle_auto() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    fx.dxvk("1.10.3");
    let components = fx.components();
    let exe = ron_like(fx.dir.path());
    let target = target(&exe);
    let prefers_wined3d = profile(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[exe]\npath = \"g.exe\"\n[graphics]\nbackend = \"wined3d\"\n",
    );
    let no_preference = profile("schema = 1\nid = \"g\"\nname = \"G\"\n[exe]\npath = \"g.exe\"\n");
    let bottle_dxvk = fx.bottle("dxvk", |config| {
        config.graphics.backend = BackendChoice::Fixed(Backend::Dxvk);
    });
    let bottle_auto = fx.bottle("auto", |_| {});
    let auto = LaunchOptions {
        backend: Some(BackendChoice::Auto),
        ..LaunchOptions::default()
    };
    let none = LaunchOptions::default();

    let cases: [(&Bottle, Option<_>, &LaunchOptions, Backend); 7] = [
        // --backend beats the profile and the bottle.
        (
            &bottle_dxvk,
            Some(&prefers_wined3d),
            &fixed(Backend::Dxmt),
            Backend::Dxmt,
        ),
        // The profile beats the bottle.
        (
            &bottle_dxvk,
            Some(&prefers_wined3d),
            &none,
            Backend::Wined3d,
        ),
        // The bottle beats automatic.
        (&bottle_dxvk, Some(&no_preference), &none, Backend::Dxvk),
        (&bottle_dxvk, None, &none, Backend::Dxvk),
        // Automatic: 32-bit D3D11 → DXMT.
        (&bottle_auto, None, &none, Backend::Dxmt),
        // An explicit --backend auto ignores the bottle's fixed backend...
        (&bottle_dxvk, None, &auto, Backend::Dxmt),
        // ...but keeps the profile's preference.
        (
            &bottle_dxvk,
            Some(&prefers_wined3d),
            &auto,
            Backend::Wined3d,
        ),
    ];
    for (bottle, game_profile, options, expected) in cases {
        let context = ctx(&fx, bottle, &components, game_profile);
        let (backend, reason) = choose_backend(context, &target, options).unwrap();
        assert_eq!(
            backend, expected,
            "{} {game_profile:?} {options:?}: {reason}",
            bottle.config.name
        );
        let planned = launch::plan(context, &target, options).unwrap();
        assert_eq!(planned.activation.backend, expected);
        assert_eq!(planned.backend_reason, reason);
    }
}

#[test]
fn reasons_say_where_a_fixed_backend_came_from() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxvk("1.10.3");
    let components = fx.components();
    let exe = ron_like(fx.dir.path());
    let bottle = fx.bottle("steam", |config| {
        config.graphics.backend = BackendChoice::Fixed(Backend::Dxvk);
    });
    let (_, reason) = choose_backend(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        reason,
        "32-bit D3D11: DXVK (graphics.backend = dxvk in bottle steam)"
    );
    let (_, reason) = choose_backend(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &fixed(Backend::Wined3d),
    )
    .unwrap();
    assert_eq!(reason, "32-bit D3D11: WineD3D (--backend wined3d)");
}

// ----- automatic choice from real PE files -----

#[test]
fn auto_scans_the_executable_and_its_dlls() {
    // A runtime that can also load D3DMetal per process.
    let fx = Fixture::new(&[CX_FEATURES, &["dllpath-prepend"]].concat());
    fx.dxmt("0.80");
    fx.d3dmetal("3.0");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let dir = fx.dir.path();
    let cases = [
        (ron_like(dir), Backend::Dxmt, "32-bit D3D11: DXMT"),
        (
            game(dir, "d3d9", &PeBuilder::pe32().import("d3d9.dll")),
            Backend::Wined3d,
            "32-bit D3D9: WineD3D",
        ),
        (
            game(
                dir,
                "d3d12",
                &PeBuilder::pe64().import("d3d12.dll").import("dxgi.dll"),
            ),
            Backend::D3dmetal,
            "64-bit D3D12: D3DMetal",
        ),
        (
            game(
                dir,
                "d3d11x64",
                &PeBuilder::pe64().delay_import("d3d11.dll"),
            ),
            Backend::Dxmt,
            "64-bit D3D11: DXMT",
        ),
        (
            game(dir, "plain", &PeBuilder::pe64()),
            Backend::Wined3d,
            "64-bit, no Direct3D detected: WineD3D",
        ),
    ];
    for (path, expected, reason_start) in cases {
        let (backend, reason) = choose_backend(
            ctx(&fx, &bottle, &components, None),
            &target(&path),
            &LaunchOptions::default(),
        )
        .unwrap();
        assert_eq!(backend, expected, "{}: {reason}", path.display());
        assert!(reason.starts_with(reason_start), "{reason}");
    }
}

#[test]
fn d3dmetal_the_runtime_cannot_load_is_passed_over() {
    // The catalog's CrossOver-derived runtime has the `d3dmetal` feature
    // but no per-process DLL path, so D3DMetal can never be activated.
    let fx = Fixture::new(CX_FEATURES);
    fx.d3dmetal("3.0");
    fx.dxvk("1.10.3");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(
        fx.dir.path(),
        "d3d11x64",
        &PeBuilder::pe64().import("d3d11.dll"),
    );
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.activation.backend,
        Backend::Dxvk,
        "{}",
        plan.backend_reason
    );

    let aoe2 = profile(
        "schema = 1\nid = \"aoe2\"\nname = \"AoE2\"\n[exe]\npath = \"d3d11x64.exe\"\n[graphics]\nbackend = \"dxmt\"\nfallbacks = [\"d3dmetal\", \"dxvk\", \"wined3d\"]\n",
    );
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, Some(&aoe2)),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.activation.backend, Backend::Dxvk);

    let d3d12 = game(
        fx.dir.path(),
        "d3d12",
        &PeBuilder::pe64().import("d3d12.dll"),
    );
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, None),
        &target(&d3d12),
        &LaunchOptions::default(),
    )
    .unwrap();
    let warning = plan
        .warnings
        .iter()
        .find(|warning| warning.contains("only D3DMetal runs Direct3D 12"))
        .unwrap_or_else(|| panic!("{:#?}", plan.warnings));
    assert!(
        warning.contains("can load it per process"),
        "the real reason: {warning}"
    );

    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &components, None),
            &target(&exe),
            &fixed(Backend::D3dmetal),
        )
        .unwrap_err(),
    );
    assert!(
        message.starts_with(
            "--backend d3dmetal: D3DMetal needs a Wine runtime that can load it per process"
        ),
        "{message}"
    );
}

#[test]
fn dxmt_copied_into_the_prefix_needs_the_runtimes_bridge() {
    let fx = Fixture::new(CX_FEATURES);
    fs::remove_file(fx.wine.root.join("lib/wine/x86_64-unix/winemetal.so")).unwrap();
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let exe = ron_like(fx.dir.path());
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(
        plan.activation.backend,
        Backend::Wined3d,
        "{}",
        plan.backend_reason
    );
}

#[test]
fn availability_needs_the_component_and_the_runtime_feature() {
    let exe_dir = tempfile::tempdir().unwrap();
    let exe = ron_like(exe_dir.path());

    // DXMT installed, but the runtime lacks the `dxmt` feature.
    let fx = Fixture::new(&["wow64", "msync"]);
    fx.dxmt("0.80");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let (backend, reason) = choose_backend(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(backend, Backend::Wined3d, "{reason}");
    assert!(reason.contains("DXMT not available"), "{reason}");
    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &components, None),
            &target(&exe),
            &fixed(Backend::Dxmt),
        )
        .unwrap_err(),
    );
    assert!(
        message.starts_with("--backend dxmt: Wine 11.17-fake cannot load DXMT"),
        "{message}"
    );
    assert!(message.contains("`dxmt` feature"), "{message}");
}

#[test]
fn version_pins_choose_the_component() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    fx.dxmt("0.90");
    let components = fx.components();
    let exe = ron_like(fx.dir.path());
    let newest = fx.bottle("newest", |_| {});
    let pinned = fx.bottle("pinned", |config| {
        config.graphics.dxmt = Some("0.80".to_owned());
    });
    let missing = fx.bottle("missing", |config| {
        config.graphics.dxmt = Some("0.79".to_owned());
    });

    let planned = launch::plan(
        ctx(&fx, &newest, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(planned.activation.version.as_deref(), Some("0.90"));
    let planned = launch::plan(
        ctx(&fx, &pinned, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(planned.activation.version.as_deref(), Some("0.80"));
    assert!(planned.activation.copies.iter().all(|copy| {
        copy.source.starts_with(
            fx.layout
                .component_dir(uncork_core::component::ComponentKind::Dxmt, "0.80"),
        )
    }));

    let message = unsupported(
        launch::plan(
            ctx(&fx, &missing, &components, None),
            &target(&exe),
            &fixed(Backend::Dxmt),
        )
        .unwrap_err(),
    );
    assert_eq!(
        message,
        "--backend dxmt: DXMT 0.79 (pinned by graphics.dxmt in bottle missing) is not installed; install it with `uncork runtime install dxmt --version 0.79`"
    );
}

// ----- fixed backends that cannot run -----

#[test]
fn a_fixed_backend_that_is_not_installed_names_the_install_command() {
    let fx = Fixture::new(CX_FEATURES);
    let exe = ron_like(fx.dir.path());
    let bottle = fx.bottle("steam", |_| {});
    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &[], None),
            &target(&exe),
            &fixed(Backend::Dxmt),
        )
        .unwrap_err(),
    );
    assert_eq!(
        message,
        "--backend dxmt: DXMT is not installed; install it with `uncork runtime install dxmt`"
    );

    let bottle = fx.bottle("fixed", |config| {
        config.graphics.backend = BackendChoice::Fixed(Backend::Dxvk);
    });
    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &[], None),
            &target(&exe),
            &LaunchOptions::default(),
        )
        .unwrap_err(),
    );
    assert_eq!(
        message,
        "graphics.backend = dxvk in bottle fixed: DXVK is not installed; install it with `uncork runtime install dxvk`"
    );
}

#[test]
fn a_fixed_backend_that_cannot_run_the_game_is_an_error() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    fx.dxvk("1.10.3");
    fx.d3dmetal("3.0");
    let components = fx.components();
    let bottle = fx.bottle("steam", |_| {});
    let ron_exe = ron_like(fx.dir.path());
    let ron = profile(RON_PROFILE);
    let d3d12 = game(
        fx.dir.path(),
        "d3d12",
        &PeBuilder::pe64().import("d3d12.dll"),
    );

    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &components, None),
            &target(&ron_exe),
            &fixed(Backend::D3dmetal),
        )
        .unwrap_err(),
    );
    assert!(
        message.starts_with("--backend d3dmetal: D3DMetal is 64-bit only"),
        "{message}"
    );
    assert!(
        message.ends_with("let Uncork pick with `--backend auto`"),
        "{message}"
    );

    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &components, Some(&ron)),
            &target(&ron_exe),
            &fixed(Backend::Dxvk),
        )
        .unwrap_err(),
    );
    assert!(message.contains("geometry shaders"), "{message}");

    let message = unsupported(
        launch::plan(
            ctx(&fx, &bottle, &components, None),
            &target(&d3d12),
            &fixed(Backend::Wined3d),
        )
        .unwrap_err(),
    );
    assert!(
        message.starts_with("--backend wined3d: WineD3D cannot run 64-bit D3D12"),
        "{message}"
    );

    let fixed_bottle = fx.bottle("dxvk", |config| {
        config.graphics.backend = BackendChoice::Fixed(Backend::Dxvk);
    });
    let message = unsupported(
        launch::plan(
            ctx(&fx, &fixed_bottle, &components, None),
            &target(&d3d12),
            &LaunchOptions::default(),
        )
        .unwrap_err(),
    );
    assert!(
        message.ends_with(
            "pass --backend, or let Uncork pick with `uncork bottle set dxvk graphics.backend=auto`"
        ),
        "{message}"
    );
}

#[test]
fn an_unavailable_profile_preference_falls_back_with_a_warning() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = ron_like(fx.dir.path());
    let ron = profile(RON_PROFILE);
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], Some(&ron)),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    assert_eq!(plan.activation.backend, Backend::Wined3d);
    assert!(
        plan.warnings.iter().any(|w| w
            == "rise-of-nations-extended-edition prefers DXMT, but DXMT is not installed; install it with `uncork runtime install dxmt`; using WineD3D"),
        "{:#?}",
        plan.warnings
    );
}

// ----- warnings -----

fn warnings_for(
    fx: &Fixture,
    bottle: &Bottle,
    exe_path: &Path,
    options: &LaunchOptions,
) -> Vec<String> {
    let components = fx.components();
    launch::plan(
        ctx(fx, bottle, &components, None),
        &target(exe_path),
        options,
    )
    .unwrap()
    .warnings
}

#[test]
fn warns_about_anti_cheat() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(fx.dir.path(), "online", &PeBuilder::pe64());
    write(
        &exe.parent().unwrap().join("EasyAntiCheat/Launcher.exe"),
        b"",
    );
    write(&exe.parent().unwrap().join("BEService_x64.exe"), b"");
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings.iter().any(|w| w.starts_with(
            "kernel anti-cheat found next to the game (beservice_x64.exe, easyanticheat)"
        )),
        "{warnings:#?}"
    );
}

#[test]
fn warns_about_32_bit_programs_on_a_runtime_without_wow64() {
    let fx = Fixture::new(&["msync"]);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(fx.dir.path(), "old", &PeBuilder::pe32());
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("lacks the `wow64` feature")),
        "{warnings:#?}"
    );

    let fx = Fixture::new(&["wow64"]);
    let bottle = fx.bottle("steam", |_| {});
    assert!(warnings_for(&fx, &bottle, &exe, &LaunchOptions::default()).is_empty());
}

#[test]
fn warns_about_direct3d_12_without_d3dmetal() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(
        fx.dir.path(),
        "dx12",
        &PeBuilder::pe64().import("d3d12.dll"),
    );
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings.iter().any(|w| w.starts_with("64-bit D3D12: only D3DMetal runs Direct3D 12 and WineD3D will fail; D3DMetal: D3DMetal is not installed; import it")),
        "{warnings:#?}"
    );

    let exe = game(
        fx.dir.path(),
        "dx12x86",
        &PeBuilder::pe32().import("d3d12.dll"),
    );
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("no backend on macOS runs 32-bit Direct3D 12")),
        "{warnings:#?}"
    );
}

#[test]
fn warns_about_modules_without_nx_compat_in_32_bit_games() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(
        fx.dir.path(),
        "nx",
        &PeBuilder::pe32().large_address_aware(true),
    );
    PeBuilder::pe32()
        .dll()
        .nx_compat(false)
        .write(&exe.parent().unwrap().join("ttv.dll"));
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings.iter().any(
            |w| w.starts_with("modules without NX_COMPAT (ttv.dll): ") && w.contains("Rosetta")
        ),
        "{warnings:#?}"
    );

    // 64-bit processes are not affected.
    let exe = game(fx.dir.path(), "nx64", &PeBuilder::pe64().nx_compat(false));
    assert!(warnings_for(&fx, &bottle, &exe, &LaunchOptions::default()).is_empty());
}

#[test]
fn suggests_large_address_awareness_where_the_runtime_honors_it() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let tool = game(
        fx.dir.path(),
        "tool",
        &PeBuilder::pe32().large_address_aware(false),
    );
    assert!(
        warnings_for(&fx, &bottle, &tool, &LaunchOptions::default()).is_empty(),
        "a program without a graphics API is not a game and gets no hint"
    );

    let exe = game(
        fx.dir.path(),
        "laa",
        &PeBuilder::pe32()
            .large_address_aware(false)
            .import("d3d9.dll"),
    );
    let warnings = warnings_for(&fx, &bottle, &exe, &LaunchOptions::default());
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("laa.exe is not large-address-aware")
                && w.contains("--env WINE_LARGE_ADDRESS_AWARE=1")),
        "{warnings:#?}"
    );

    let set = LaunchOptions {
        env: env(&[("WINE_LARGE_ADDRESS_AWARE", "1")]),
        ..LaunchOptions::default()
    };
    assert!(warnings_for(&fx, &bottle, &exe, &set).is_empty());

    let fx = Fixture::new(&["wow64"]);
    let bottle = fx.bottle("steam", |_| {});
    assert!(
        warnings_for(&fx, &bottle, &exe, &LaunchOptions::default()).is_empty(),
        "a runtime without the feature ignores the variable"
    );
}

#[test]
fn warns_about_bottle_wide_settings_it_cannot_change() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(fx.dir.path(), "plain", &PeBuilder::pe64());
    let retina = LaunchOptions {
        retina: true,
        ..LaunchOptions::default()
    };
    let warnings = warnings_for(&fx, &bottle, &exe, &retina);
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("Retina mode on was asked for")
                && w.contains("uncork bottle set steam performance.retina=true")),
        "{warnings:#?}"
    );

    let retina_bottle = fx.bottle("retina", |config| config.performance.retina = true);
    assert!(warnings_for(&fx, &retina_bottle, &exe, &retina).is_empty());

    let msync_off = profile(
        "schema = 1\nid = \"g\"\nname = \"G\"\n[exe]\npath = \"plain.exe\"\n[wine]\nmsync = false\nwindows_version = \"win7\"\n",
    );
    let warnings = launch::plan(
        ctx(&fx, &bottle, &[], Some(&msync_off)),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap()
    .warnings;
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("g wants msync off") && w.contains("uncork bottle kill steam")),
        "{warnings:#?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.starts_with("g wants Windows version win7")),
        "{warnings:#?}"
    );
}

// ----- execute -----

#[test]
fn execute_puts_the_backend_in_place_and_starts_the_command() {
    let fx = Fixture::new(CX_FEATURES);
    fx.dxmt("0.80");
    let components = fx.components();
    let mut bottle = fx.bottle("steam", |_| {});
    let exe = ron_like(fx.dir.path());
    let plan = launch::plan(
        ctx(&fx, &bottle, &components, None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();

    let mut child = launch::execute(&plan, &mut bottle, &fx.wine).unwrap();
    assert!(child.wait().unwrap().success());

    for cache in ["cache/dxmt", "cache/dxvk"] {
        assert!(bottle.path.join(cache).is_dir(), "{cache}");
    }
    for (dir, arch) in [("system32", "x86_64-windows"), ("syswow64", "i386-windows")] {
        let d3d11 = fs::read(bottle.drive_c().join("windows").join(dir).join("d3d11.dll")).unwrap();
        assert_eq!(
            uncork_pe::wine_marker(&d3d11),
            None,
            "{dir}: the marker is stripped"
        );
        assert!(d3d11.ends_with(format!("dxmt 0.80 {arch} d3d11").as_bytes()));
        let winemetal = fs::read(
            bottle
                .drive_c()
                .join("windows")
                .join(dir)
                .join("winemetal.dll"),
        )
        .unwrap();
        assert_eq!(
            winemetal,
            builtin_dll(&format!("dxmt 0.80 {arch} winemetal")),
            "{dir}"
        );
    }
    let active = bottle.state.active.as_ref().unwrap();
    assert_eq!(
        (active.backend, active.version.as_str()),
        (Backend::Dxmt, "0.80")
    );
    assert_eq!(
        Bottle::open(&bottle.path).unwrap().state,
        bottle.state,
        "state is saved"
    );

    let calls = fx.calls();
    assert_eq!(calls.len(), 1, "{calls:#?}");
    // The shell reports the physical directory (/var is a symlink on macOS).
    let cwd = fs::canonicalize(exe.parent().unwrap()).unwrap();
    assert!(
        calls[0].starts_with(&format!("wine {} | cwd={} ", exe.display(), cwd.display())),
        "{}",
        calls[0]
    );
    assert!(
        calls[0].contains("WINEDLLOVERRIDES=d3d10,d3d12,d3d12core=b;winemenubuilder.exe=d;d3d10core,d3d11,dxgi=n,b "),
        "{}",
        calls[0]
    );
    assert!(
        calls[0].contains("PATH=/usr/bin:/bin:/usr/sbin:/sbin"),
        "{}",
        calls[0]
    );
    assert!(plan.log.is_file(), "output goes to the log");
}

// ----- profile INI paths -----

#[test]
fn resolves_every_ini_base() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let users = bottle.drive_c().join("users");
    let install = fx.dir.path().join("Rise of Nations");
    let cases = [
        (
            r"%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini",
            users.join("crossover/AppData/Roaming/Microsoft Games/Rise of Nations/rise2.ini"),
        ),
        (
            "%LOCALAPPDATA%/Game/settings.ini",
            users.join("crossover/AppData/Local/Game/settings.ini"),
        ),
        (
            r"%USERPROFILE%\Documents\x.ini",
            users.join("crossover/Documents/x.ini"),
        ),
        (
            r"%InstallDir%\.\cfg\\game.ini",
            install.join("cfg/game.ini"),
        ),
    ];
    for (file, expected) in cases {
        assert_eq!(
            resolve_profile_path(&bottle, Some(&install), file),
            Some(expected),
            "{file}"
        );
    }
    for file in [
        r"%INSTALLDIR%\..\escape.ini",
        r"%APPDATA%\a\..\b.ini",
        r"%TEMP%\x.ini",
        r"C:\x.ini",
        r"%APPDATA%\",
        "%APPDATA%x.ini",
    ] {
        assert_eq!(
            resolve_profile_path(&bottle, Some(&install), file),
            None,
            "{file}"
        );
    }
    assert_eq!(
        resolve_profile_path(&bottle, None, r"%INSTALLDIR%\x.ini"),
        None
    );
}

#[test]
fn the_windows_user_comes_from_the_bottle_then_the_prefix_then_the_environment() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let users = bottle.drive_c().join("users");
    fs::create_dir_all(users.join("steamuser")).unwrap();
    assert_eq!(
        resolve_profile_path(&bottle, None, r"%USERPROFILE%\x.ini"),
        Some(users.join("steamuser/x.ini")),
        "the only user besides Public"
    );

    fs::create_dir_all(users.join("other")).unwrap();
    let expected = std::env::var("USER").ok().filter(|user| !user.is_empty());
    assert_eq!(
        resolve_profile_path(&bottle, None, r"%USERPROFILE%\x.ini"),
        expected.map(|user| users.join(user).join("x.ini")),
        "ambiguous: the current user"
    );

    let unsafe_user = fx.bottle("unsafe", |config| {
        config.env.insert("USER".to_owned(), "../root".to_owned());
    });
    assert_eq!(
        resolve_profile_path(&unsafe_user, None, r"%APPDATA%\x.ini"),
        None
    );
}

#[test]
fn ini_paths_follow_the_on_disk_spelling() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let actual = bottle
        .drive_c()
        .join("users/crossover/AppData/Roaming/Microsoft Games/Rise of Nations/Rise2.INI");
    write(&actual, b"");
    assert_eq!(
        resolve_profile_path(
            &bottle,
            None,
            r"%appdata%\microsoft games\RISE OF NATIONS\rise2.ini"
        ),
        Some(actual)
    );
}

#[test]
fn applies_profile_ini_edits_end_to_end() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |config| {
        config.env.insert("USER".to_owned(), "crossover".to_owned());
    });
    let install = fx.dir.path().join("game");
    let rise2 = bottle
        .drive_c()
        .join("users/crossover/AppData/Roaming/Microsoft Games/Rise of Nations/rise2.ini");
    write(
        &rise2,
        b"[RISE OF NATIONS]\r\nSkipIntroMovies=0\r\nVSync=1\r\n",
    );
    let game_ini = install.join("game.ini");
    write(&game_ini, b"[video]\nwidth=800\n");
    let game_profile = profile(&format!(
        "{RON_PROFILE}
[[ini]]
file = '%APPDATA%\\Microsoft Games\\Rise of Nations\\rise2.ini'
section = \"RISE OF NATIONS\"
key = \"SkipIntroMovies\"
value = \"1\"
[[ini]]
file = '%LOCALAPPDATA%\\Not Yet\\created.ini'
section = \"a\"
key = \"b\"
value = \"c\"
[[ini]]
file = '%INSTALLDIR%\\game.ini'
section = \"video\"
key = \"width\"
value = \"800\"
[[ini]]
file = '%APPDATA%/Microsoft Games/Rise of Nations/rise2.ini'
section = \"RISE OF NATIONS\"
key = \"Fullscreen\"
value = \"0\"
"
    ));

    let changed = apply_profile_ini(&game_profile, &bottle, Some(&install)).unwrap();
    assert_eq!(
        changed,
        std::slice::from_ref(&rise2),
        "game.ini already had its value"
    );
    assert_eq!(
        fs::read_to_string(&rise2).unwrap(),
        "[RISE OF NATIONS]\r\nSkipIntroMovies=1\r\nVSync=1\r\nFullscreen=0\r\n"
    );
    assert!(
        !bottle
            .drive_c()
            .join("users/crossover/AppData/Local/Not Yet")
            .exists()
    );
    assert_eq!(
        apply_profile_ini(&game_profile, &bottle, Some(&install)).unwrap(),
        Vec::<PathBuf>::new(),
        "idempotent"
    );

    let err = apply_profile_ini(&game_profile, &bottle, None).unwrap_err();
    assert!(
        matches!(&err, Error::Config { what: "profile", message, .. } if message.contains("%INSTALLDIR% needs")),
        "{err:?}"
    );
}

#[test]
fn plans_are_serializable_for_dry_runs() {
    let fx = Fixture::new(CX_FEATURES);
    let bottle = fx.bottle("steam", |_| {});
    let exe = game(fx.dir.path(), "plain", &PeBuilder::pe64());
    let plan = launch::plan(
        ctx(&fx, &bottle, &[], None),
        &target(&exe),
        &LaunchOptions::default(),
    )
    .unwrap();
    let json = serde_json::to_value(&plan).unwrap();
    assert_eq!(json["activation"]["backend"], "wined3d");
    assert_eq!(
        plan.command.args.first(),
        Some(&OsString::from(exe.as_os_str()))
    );
}
