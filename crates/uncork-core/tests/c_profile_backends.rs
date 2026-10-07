//! The built-in profiles and the backend decision table, through the public
//! API: what each first-target game ends up running on, for every
//! combination of installed backends.

use uncork_core::graphics::{Availability, Backend, recommend, supports};
use uncork_core::profile::{GameProfile, builtin, resolve};

fn every_availability() -> impl Iterator<Item = Availability> {
    (0..8u8).map(|bits| Availability {
        dxmt: bits & 1 != 0,
        dxvk: bits & 2 != 0,
        d3dmetal: bits & 4 != 0,
    })
}

fn profile(query: &str) -> GameProfile {
    resolve(&builtin(), query)
        .unwrap_or_else(|| panic!("no built-in profile for {query}"))
        .clone()
}

fn backend_for(profile: &GameProfile, available: Availability) -> Backend {
    let bitness = profile
        .exe
        .bitness
        .expect("built-in profiles declare their bitness");
    recommend(
        profile.exe.api,
        bitness,
        &available,
        profile.graphics.backend,
        &profile.graphics.fallbacks,
        profile.exe.geometry_shaders,
    )
    .backend
}

#[test]
fn rise_of_nations_runs_on_dxmt_or_wined3d_only() {
    let ron = profile("rise-of-nations-extended-edition");
    for available in every_availability() {
        let expected = if available.dxmt {
            Backend::Dxmt
        } else {
            Backend::Wined3d
        };
        assert_eq!(backend_for(&ron, available), expected, "{available:?}");
    }
}

#[test]
fn age_of_empires_2_falls_back_through_its_list() {
    let aoe2 = profile("813780");
    let available = |dxmt, dxvk, d3dmetal| Availability {
        dxmt,
        dxvk,
        d3dmetal,
    };
    assert_eq!(
        backend_for(&aoe2, available(true, true, true)),
        Backend::Dxmt
    );
    assert_eq!(
        backend_for(&aoe2, available(false, true, true)),
        Backend::D3dmetal
    );
    assert_eq!(
        backend_for(&aoe2, available(false, true, false)),
        Backend::Dxvk
    );
    assert_eq!(
        backend_for(&aoe2, available(false, false, false)),
        Backend::Wined3d
    );
}

#[test]
fn every_builtin_profile_gets_its_preferred_backend_when_everything_is_installed() {
    let everything = Availability {
        dxmt: true,
        dxvk: true,
        d3dmetal: true,
    };
    for profile in builtin() {
        let (Some(api), Some(bitness)) = (profile.exe.api, profile.exe.bitness) else {
            continue;
        };
        let chosen = backend_for(&profile, everything);
        assert!(supports(chosen, api, bitness), "{}: {chosen}", profile.id);
        if let Some(preferred) = profile.graphics.backend {
            assert_eq!(chosen, preferred, "{}", profile.id);
        }
    }
}

#[test]
fn no_builtin_profile_ever_lands_on_a_backend_that_cannot_run_it() {
    for profile in builtin() {
        let (Some(api), Some(bitness)) = (profile.exe.api, profile.exe.bitness) else {
            continue;
        };
        for available in every_availability() {
            let chosen = backend_for(&profile, available);
            assert!(
                supports(chosen, api, bitness),
                "{} with {available:?} chose {chosen}",
                profile.id
            );
            assert!(
                available.has(chosen),
                "{} with {available:?} chose {chosen}",
                profile.id
            );
        }
    }
}
