//! Retina mode and DPI through the public API with a fake Wine: registry
//! changes made with the bottle stopped.

mod d_support;

use std::time::Duration;

use d_support::{CX_FEATURES, Fixture};
use uncork_core::bottle;
use uncork_core::steam;

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
