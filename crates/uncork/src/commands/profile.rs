//! `uncork profile ...`: game profiles.

use std::process::ExitCode;

use anyhow::bail;
use serde::Serialize;
use uncork_core::graphics::Backend;
use uncork_core::profile::{CompatStatus, GameProfile, Lookup};
use uncork_core::steam::LaunchMode;
use uncork_pe::{Bitness, GraphicsApi};

use super::{Ctx, key_values};
use crate::cli::ProfileCommand;
use crate::output;

/// Dispatch a `profile` subcommand.
pub(super) fn run(ctx: &Ctx, command: &ProfileCommand) -> anyhow::Result<ExitCode> {
    match command {
        ProfileCommand::List => list(ctx),
        ProfileCommand::Show { game } => show(ctx, game),
    }
}

/// Built-in and user profiles; invalid user profiles are reported as
/// warnings and left out.
pub(super) fn load_profiles(ctx: &Ctx) -> anyhow::Result<Vec<GameProfile>> {
    let (profiles, errors) = uncork_core::profile::load_all(&ctx.layout)?;
    for err in errors {
        eprintln!("warning: skipping a profile: {err}");
    }
    Ok(profiles)
}

/// One profile in `profile list --json`.
#[derive(Debug, Serialize)]
struct ProfileRow<'a> {
    id: &'a str,
    name: &'a str,
    appid: Option<u32>,
    status: CompatStatus,
    backend: Option<Backend>,
}

fn list(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let profiles = load_profiles(ctx)?;
    let rows: Vec<ProfileRow<'_>> = profiles
        .iter()
        .map(|profile| ProfileRow {
            id: &profile.id,
            name: &profile.name,
            appid: profile.steam.as_ref().map(|steam| steam.appid),
            status: profile.compat.status,
            backend: profile.graphics.backend,
        })
        .collect();
    if ctx.json {
        output::print_json(&rows)?;
        return Ok(ExitCode::SUCCESS);
    }
    let table: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.id.to_owned(),
                row.name.to_owned(),
                row.appid
                    .map_or_else(String::new, |appid| appid.to_string()),
                status_name(row.status).to_owned(),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(&["ID", "NAME", "APPID", "STATUS"], &table)
    );
    Ok(ExitCode::SUCCESS)
}

fn show(ctx: &Ctx, game: &str) -> anyhow::Result<ExitCode> {
    let profiles = load_profiles(ctx)?;
    let profile = match uncork_core::profile::lookup(&profiles, game) {
        Lookup::Found(profile) => profile,
        Lookup::Ambiguous(candidates) => bail!(
            "{game:?} matches several game profiles: {}; name one by its id",
            super::launch::profile_ids(&candidates)
        ),
        Lookup::NotFound => {
            bail!("no game profile matches {game:?}; `uncork profile list` shows them all")
        }
    };
    if ctx.json {
        output::print_json(profile)?;
    } else {
        print!("{}", render_profile(profile));
    }
    Ok(ExitCode::SUCCESS)
}

fn status_name(status: CompatStatus) -> &'static str {
    match status {
        CompatStatus::Untested => "untested",
        CompatStatus::Broken => "broken",
        CompatStatus::Runs => "runs",
        CompatStatus::Playable => "playable",
        CompatStatus::Perfect => "perfect",
    }
}

fn api_name(api: GraphicsApi) -> &'static str {
    match api {
        GraphicsApi::DirectDraw => "DirectDraw",
        GraphicsApi::OpenGl => "OpenGL",
        GraphicsApi::D3d8 => "D3D8",
        GraphicsApi::D3d9 => "D3D9",
        GraphicsApi::D3d10 => "D3D10",
        GraphicsApi::D3d11 => "D3D11",
        GraphicsApi::D3d12 => "D3D12",
        GraphicsApi::Vulkan => "Vulkan",
    }
}

/// `32-bit`/`64-bit`.
fn bitness_name(bitness: Bitness) -> &'static str {
    match bitness {
        Bitness::X86 => "32-bit",
        Bitness::X64 => "64-bit",
    }
}

/// A readable, TOML-like summary of a profile.
fn render_profile(profile: &GameProfile) -> String {
    let mut exe_facts: Vec<&str> = Vec::new();
    if let Some(bitness) = profile.exe.bitness {
        exe_facts.push(bitness_name(bitness));
    }
    if let Some(api) = profile.exe.api {
        exe_facts.push(api_name(api));
    }
    if profile.exe.geometry_shaders {
        exe_facts.push("geometry shaders");
    }
    let exe = if exe_facts.is_empty() {
        profile.exe.path.clone()
    } else {
        format!("{} ({})", profile.exe.path, exe_facts.join(", "))
    };
    let graphics = &profile.graphics;
    let backends = graphics
        .backend
        .iter()
        .chain(&graphics.fallbacks)
        .map(|backend| backend.as_str())
        .collect::<Vec<_>>();
    let mode = match profile.launch_mode() {
        LaunchMode::Direct => "direct (Steam running, game started directly)",
        LaunchMode::Applaunch => "applaunch (started by Steam)",
        LaunchMode::Standalone => "standalone (no Steam)",
    };
    let pairs = |map: &std::collections::BTreeMap<String, String>| {
        map.iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join("\n")
    };

    let mut rows: Vec<(&str, String)> =
        vec![("Name", profile.name.clone()), ("Id", profile.id.clone())];
    if let Some(steam) = &profile.steam {
        rows.push(("Steam app id", steam.appid.to_string()));
    }
    rows.push(("Executable", exe));
    rows.push((
        "Graphics",
        if backends.is_empty() {
            "automatic".to_owned()
        } else {
            backends.join(", then ")
        },
    ));
    rows.push(("Launch", mode.to_owned()));
    if !profile.launch.args.is_empty() {
        rows.push(("Arguments", profile.launch.args.join(" ")));
    }
    if let Some(version) = profile.wine.windows_version {
        rows.push(("Windows", version.winecfg_name().to_owned()));
    }
    if let Some(msync) = profile.wine.msync {
        rows.push(("msync", if msync { "on" } else { "off" }.to_owned()));
    }
    let performance = [
        ("retina", profile.performance.retina),
        ("metalfx", profile.performance.metalfx),
        ("avx", profile.performance.avx),
    ]
    .iter()
    .filter_map(|(name, value)| value.map(|on| format!("{name} {}", if on { "on" } else { "off" })))
    .collect::<Vec<_>>();
    if !performance.is_empty() {
        rows.push(("Performance", performance.join(", ")));
    }
    if !profile.env.is_empty() {
        rows.push(("Environment", pairs(&profile.env)));
    }
    if !profile.dll_overrides.is_empty() {
        rows.push(("DLL overrides", pairs(&profile.dll_overrides)));
    }
    if !profile.ini.is_empty() {
        let edits = profile
            .ini
            .iter()
            .map(|ini| {
                let reason = if ini.reason.is_empty() {
                    String::new()
                } else {
                    format!("\n  {}", ini.reason)
                };
                format!(
                    "{} [{}] {}={}{reason}",
                    ini.file, ini.section, ini.key, ini.value
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        rows.push(("INI", edits));
    }
    rows.push(("Status", status_name(profile.compat.status).to_owned()));
    if !profile.compat.notes.is_empty() {
        rows.push(("Notes", profile.compat.notes.clone()));
    }
    for report in &profile.compat.reports {
        rows.push((
            "Report",
            format!(
                "{} {} on {} macOS {}, {}: {}{}",
                report.date,
                report.uncork,
                report.chip,
                report.macos,
                report.backend,
                status_name(report.status),
                if report.notes.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", report.notes)
                }
            ),
        ));
    }
    key_values(&rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rise_of_nations_reads_well() {
        let profiles = uncork_core::profile::builtin();
        let profile = uncork_core::profile::resolve(&profiles, "287450").unwrap();
        let text = render_profile(profile);
        assert!(text.contains("Steam app id   287450\n"), "{text}");
        assert!(
            text.contains("riseofnations.exe (32-bit, D3D11, geometry shaders)"),
            "{text}"
        );
        assert!(text.contains("dxmt, then wined3d"), "{text}");
        assert!(text.contains("WINE_LARGE_ADDRESS_AWARE=1"), "{text}");
        assert!(text.contains("SkipIntroMovies=1"), "{text}");
        assert!(text.lines().all(|line| line == line.trim_end()), "{text}");
    }
}
