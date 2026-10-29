//! `uncork profile ...`: list, show, validate and scaffold game profiles,
//! and print their JSON schema.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use serde::Serialize;
use uncork_core::graphics::{self, Availability, Backend, Recommendation};
use uncork_core::profile::{CompatStatus, GameProfile, Lookup, Validation};
use uncork_core::steam::LaunchMode;
use uncork_pe::{Bitness, EvidenceKind, GameScan, GraphicsApi};

use super::{Ctx, key_values};
use crate::cli::ProfileCommand;
use crate::output;

/// Dispatch a `profile` subcommand.
pub(super) fn run(ctx: &Ctx, command: &ProfileCommand) -> anyhow::Result<ExitCode> {
    match command {
        ProfileCommand::List => list(ctx),
        ProfileCommand::Show { game } => show(ctx, game),
        ProfileCommand::Validate { paths } => validate(ctx, paths),
        ProfileCommand::New {
            target,
            bottle,
            id,
            out,
        } => new(ctx, target, bottle.as_deref(), id.as_deref(), out),
        ProfileCommand::Schema => {
            print!("{}", uncork_core::profile::schema_json());
            Ok(ExitCode::SUCCESS)
        }
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
            return Err(uncork_core::Error::NotFound {
                what: "game profile",
                name: game.to_owned(),
                hint: "; `uncork profile list` shows them all".to_owned(),
            }
            .into());
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
    status.as_str()
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

// ----- validate -----

/// `uncork profile validate --json`: the one value on stdout, with the
/// error the text form prints (`None` when every file is valid), so the
/// problems keep their structure and the exit code stays the error's.
#[derive(Debug, Serialize)]
struct ValidateView<'a> {
    profiles: &'a [Validation],
    error: Option<output::ErrorReport>,
}

/// `uncork profile validate <paths>`: every problem, then a summary. Any
/// problem is an `invalid-config` error naming the first file that has
/// one, so the exit status is 4.
fn validate(ctx: &Ctx, paths: &[PathBuf]) -> anyhow::Result<ExitCode> {
    let validations = uncork_core::profile::validate_paths(paths)?;
    let error = invalid_profiles(&validations);
    if ctx.json {
        let report = error.as_ref().map(output::ErrorReport::new);
        let code = report
            .as_ref()
            .map_or(ExitCode::SUCCESS, |report| ExitCode::from(report.exit_code));
        output::print_json(&ValidateView {
            profiles: &validations,
            error: report,
        })?;
        return Ok(code);
    }
    print!("{}", render_validations(&validations));
    match error {
        Some(error) => Err(error),
        None => Ok(ExitCode::SUCCESS),
    }
}

/// The error for the files with problems, if any: an `invalid-config`
/// naming the first such file and counting the problems.
fn invalid_profiles(validations: &[Validation]) -> Option<anyhow::Error> {
    let failed: Vec<&Validation> = validations.iter().filter(|v| !v.is_valid()).collect();
    let first = failed.first()?;
    let problems = count(failed.iter().map(|v| v.problems.len()).sum(), "problem");
    let message = if failed.len() == 1 {
        format!("{problems}, listed with the line")
    } else {
        format!("{problems} in {} files, listed with the line", failed.len())
    };
    Some(
        uncork_core::Error::Config {
            what: "profile",
            path: first.path.clone(),
            message,
        }
        .into(),
    )
}

/// One line per problem (`file:line: message`, or `file: message` when the
/// line is unknown), `file: ok` per valid file, then a summary line.
fn render_validations(validations: &[Validation]) -> String {
    let mut out = String::new();
    for validation in validations {
        let path = validation.path.display();
        if validation.is_valid() {
            let _ = writeln!(out, "{path}: ok");
        }
        for problem in &validation.problems {
            let _ = match problem.line {
                Some(line) => writeln!(out, "{path}:{line}: {}", problem.message),
                None => writeln!(out, "{path}: {}", problem.message),
            };
        }
    }
    let failed = validations.iter().filter(|v| !v.is_valid()).count();
    let problems: usize = validations.iter().map(|v| v.problems.len()).sum();
    let _ = if failed == 0 {
        writeln!(out, "{} valid", count(validations.len(), "profile"))
    } else {
        writeln!(
            out,
            "{} in {failed} of {} profiles",
            count(problems, "problem"),
            validations.len()
        )
    };
    out
}

/// `1 problem`, `2 problems`.
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}

// ----- new -----

/// What `uncork profile new` learned about the game before inspecting its
/// executable.
#[derive(Debug)]
struct GameFacts {
    /// Display name: Steam's, or the install directory's name.
    name: String,
    /// The Steam app id and build id, for a Steam game.
    steam: Option<(u32, Option<u64>)>,
    /// Steam's `StateFlags` when it does not list the game as fully installed.
    incomplete_state: Option<u32>,
    /// The executable, absolute.
    exe: PathBuf,
    /// `exe` relative to the install directory, `/`-separated.
    exe_path: String,
}

/// The scaffold as it is rendered (`--json` prints it as this object).
#[derive(Debug, Serialize)]
struct ScaffoldView<'a> {
    id: &'a str,
    name: &'a str,
    appid: Option<u32>,
    exe: &'a Path,
    toml: &'a str,
}

/// `uncork profile new <appid|exe>`.
fn new(
    ctx: &Ctx,
    target: &str,
    bottle: Option<&str>,
    id: Option<&str>,
    out: &str,
) -> anyhow::Result<ExitCode> {
    let facts = match parse_appid(target) {
        Some(appid) => steam_facts(ctx, appid, bottle)?,
        None => exe_facts(Path::new(target))?,
    };
    let scan = uncork_pe::scan_game(&facts.exe)
        .with_context(|| format!("cannot inspect {}", facts.exe.display()))?;
    let id = id.map_or_else(|| kebab_id(&facts.name), str::to_owned);
    let text = scaffold(&id, &facts, &scan);
    // The scaffold is a valid profile by construction; `--id` is the one
    // input the checks can reject.
    let validation = uncork_core::profile::validate_text(&text, Path::new(&format!("{id}.toml")));
    if !validation.is_valid() {
        let problems: Vec<String> = validation
            .problems
            .iter()
            .map(ToString::to_string)
            .collect();
        bail!(
            "the profile for {id:?} would not validate: {}",
            problems.join("; ")
        );
    }
    if let Some(flags) = facts.incomplete_state {
        eprintln!(
            "warning: Steam does not list {} as fully installed (StateFlags {flags}); its files may still change",
            facts.name
        );
    }
    if !scan.anti_cheat.is_empty() {
        eprintln!(
            "warning: anti-cheat files found ({}); kernel anti-cheat does not run under Wine",
            scan.anti_cheat.join(", ")
        );
    }
    let where_to_test = match facts.steam {
        Some(_) => format!("test with `uncork play {id}`"),
        None => format!(
            "test with `uncork run {}` (`uncork play` is for Steam games)",
            facts.exe.display()
        ),
    };
    if ctx.json {
        output::print_json(&ScaffoldView {
            id: &id,
            name: &facts.name,
            appid: facts.steam.map(|(appid, _)| appid),
            exe: &facts.exe,
            toml: &text,
        })?;
    } else if out == "-" {
        print!("{text}");
    } else {
        let path = Path::new(out);
        write_new_file(path, &text)?;
        eprintln!("Wrote {}.", path.display());
    }
    if out == "-" {
        eprintln!(
            "Next: save this as profiles/{id}.toml in a checkout, or as $UNCORK_HOME/profiles/{id}.toml for your own use; run `uncork profile validate` on the file; {where_to_test}; then fill [compat] from the checklist in docs/COMPATIBILITY.md."
        );
    } else {
        eprintln!(
            "Next: `uncork profile validate {out}`; copy the file to $UNCORK_HOME/profiles/ if it is not in a checkout's profiles/; {where_to_test}; then fill [compat] from the checklist in docs/COMPATIBILITY.md."
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// A Steam app id on the command line: digits only, in range.
fn parse_appid(target: &str) -> Option<u32> {
    let target = target.trim();
    if target.is_empty() || !target.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    target.parse().ok()
}

/// The game `appid` as installed in `bottle`'s Steam libraries, with the
/// executable `uncork play` would start.
fn steam_facts(ctx: &Ctx, appid: u32, bottle: Option<&str>) -> anyhow::Result<GameFacts> {
    let bottle = ctx.open_bottle(bottle)?;
    let app = uncork_core::steam::installed_game(&bottle, appid)?;
    let exe = uncork_core::steam::game_exe(&app, None)?;
    let exe_path = relative_exe_path(&exe, &app.install_path).ok_or_else(|| {
        anyhow::anyhow!(
            "{} is not under the install directory {}",
            exe.display(),
            app.install_path.display()
        )
    })?;
    let manifest = &app.manifest;
    Ok(GameFacts {
        name: manifest.name.clone(),
        steam: Some((manifest.appid, manifest.build_id)),
        incomplete_state: (!manifest.is_fully_installed()).then_some(manifest.state_flags),
        exe,
        exe_path,
    })
}

/// A Windows executable given by path: its directory is the install
/// directory and gives the name.
fn exe_facts(exe: &Path) -> anyhow::Result<GameFacts> {
    let exe =
        std::path::absolute(exe).with_context(|| format!("cannot resolve {}", exe.display()))?;
    if !exe.is_file() {
        bail!("{} does not exist", exe.display());
    }
    let stem = exe
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("game");
    let name = exe
        .parent()
        .and_then(Path::file_name)
        .and_then(|dir| dir.to_str())
        .filter(|dir| !dir.is_empty())
        .unwrap_or(stem)
        .to_owned();
    let exe_path = exe
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("game.exe")
        .to_owned();
    Ok(GameFacts {
        name,
        steam: None,
        incomplete_state: None,
        exe,
        exe_path,
    })
}

/// `exe` relative to `install_dir` with `/` separators, as `[exe] path`
/// wants it.
fn relative_exe_path(exe: &Path, install_dir: &Path) -> Option<String> {
    let relative = exe.strip_prefix(install_dir).ok()?;
    let parts: Vec<&str> = relative
        .components()
        .map(|part| part.as_os_str().to_str())
        .collect::<Option<_>>()?;
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// A profile id from a game name: ASCII letters and digits lowercased,
/// every other run of characters one `-`, at most 64 characters; `game`
/// when nothing is left.
fn kebab_id(name: &str) -> String {
    let mut id = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            id.push(c.to_ascii_lowercase());
        } else if !id.is_empty() && !id.ends_with('-') {
            id.push('-');
        }
    }
    id.truncate(64);
    let id = id.trim_end_matches('-');
    if id.is_empty() {
        "game".to_owned()
    } else {
        id.to_owned()
    }
}

/// A TOML basic string: JSON's escaping is a subset of TOML's (`\"`, `\\`,
/// `\n`, `\r`, `\t`, `\b`, `\f` and `\uXXXX`), so `serde_json` produces one.
fn toml_string(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| format!("{text:?}"))
}

/// `text` as `# ` comment lines wrapped at 78 columns.
fn comment(text: &str) -> String {
    let mut out = String::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && line.len() + 1 + word.len() > 76 {
            let _ = writeln!(out, "# {line}");
            line.clear();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        let _ = writeln!(out, "# {line}");
    }
    out
}

/// What the scan says about the executable, for the header comment.
fn describe_scan(scan: &GameScan) -> String {
    let bitness = match scan.bitness() {
        Bitness::X86 => "32-bit (PE32)",
        Bitness::X64 => "64-bit (PE32+)",
    };
    let large_address = if scan.info.large_address_aware {
        "large-address-aware"
    } else {
        "not large-address-aware"
    };
    let nx = if scan.info.nx_compat {
        "NX-compatible"
    } else {
        "not NX-compatible"
    };
    let apis = if scan.evidence.is_empty() {
        "no graphics API detected".to_owned()
    } else {
        scan.evidence
            .iter()
            .map(|evidence| {
                let how = match evidence.kind {
                    EvidenceKind::Import => "imported by",
                    EvidenceKind::String => "named in",
                };
                format!(
                    "{} ({} {how} {})",
                    api_name(evidence.api),
                    evidence.import,
                    evidence.file
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    let steamworks = if scan.uses_steamworks {
        "Steamworks"
    } else {
        "no Steamworks"
    };
    let anti_cheat = if scan.anti_cheat.is_empty() {
        "no anti-cheat files".to_owned()
    } else {
        format!(
            "anti-cheat files {} (kernel anti-cheat does not run under Wine)",
            scan.anti_cheat.join(", ")
        )
    };
    format!("{bitness}, {large_address}, {nx}; {apis}; {steamworks}; {anti_cheat}.")
}

/// The backend a profile should prefer for the scanned executable, judged
/// with every backend available: a profile describes the game, not what
/// is installed on this Mac.
fn recommend_for_profile(scan: &GameScan) -> Recommendation {
    let everything = Availability {
        dxmt: true,
        dxvk: true,
        d3dmetal: true,
    };
    graphics::recommend(
        scan.primary_api(),
        scan.bitness(),
        &everything,
        None,
        &[],
        false,
    )
}

/// The profile text for `id` from `facts` and the scan of its executable.
fn scaffold(id: &str, facts: &GameFacts, scan: &GameScan) -> String {
    let recommendation = recommend_for_profile(scan);
    let mut out = String::new();
    let title = match facts.steam {
        Some((appid, Some(build))) => format!("{} (Steam app {appid}, build {build}).", facts.name),
        Some((appid, None)) => format!("{} (Steam app {appid}).", facts.name),
        None => format!("{}.", facts.name),
    };
    out.push_str(&comment(&title));
    out.push_str(&comment(&format!(
        "Facts from `uncork profile new`, read from {} and the DLLs beside it: {} Check them, keep only the settings the game needs, and say in a comment why each one is there (profiles/README.md describes every field).",
        facts.exe_path,
        describe_scan(scan)
    )));
    if let Some(flags) = facts.incomplete_state {
        out.push_str(&comment(&format!(
            "Steam does not list the game as fully installed (StateFlags {flags}), so these facts may change once it is."
        )));
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "schema = 1");
    let _ = writeln!(out, "id = {}", toml_string(id));
    let _ = writeln!(out, "name = {}", toml_string(&facts.name));
    let _ = writeln!(out);
    match facts.steam {
        Some((appid, _)) => {
            let _ = writeln!(out, "[steam]");
            let _ = writeln!(out, "appid = {appid}");
        }
        None => {
            let _ = writeln!(out, "# [steam]");
            let _ = writeln!(
                out,
                "# appid = 123456   # when the game is on Steam: its app id (`uncork steam games`)"
            );
        }
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "[exe]");
    let _ = writeln!(out, "path = {}", toml_string(&facts.exe_path));
    let bitness = match scan.bitness() {
        Bitness::X86 => "x86",
        Bitness::X64 => "x64",
    };
    let _ = writeln!(out, "bitness = \"{bitness}\"");
    match scan.primary_api() {
        Some(api) => {
            let _ = writeln!(out, "api = \"{}\"", api_key(api));
        }
        None => {
            let _ = writeln!(
                out,
                "# api = \"d3d11\"   # nothing detected; set it by hand: directdraw, opengl, d3d8, d3d9, d3d10, d3d11, d3d12, vulkan"
            );
        }
    }
    let _ = writeln!(
        out,
        "# geometry_shaders = true   # when the game uses geometry shaders; rules out DXVK (MoltenVK has none)"
    );
    let _ = writeln!(out);
    let fallbacks = recommendation
        .order
        .iter()
        .filter(|backend| **backend != recommendation.backend)
        .map(|backend| format!("\"{backend}\""))
        .collect::<Vec<_>>()
        .join(", ");
    if scan.primary_api().is_some() {
        let _ = writeln!(out, "[graphics]");
        out.push_str(&comment(&recommendation.reason));
        let _ = writeln!(out, "backend = \"{}\"", recommendation.backend);
        let _ = writeln!(out, "fallbacks = [{fallbacks}]");
    } else {
        let _ = writeln!(out, "# [graphics]");
        out.push_str(&comment(&format!(
            "{} Set backend once [exe] api is known: dxmt, d3dmetal, dxvk or wined3d.",
            recommendation.reason
        )));
        let _ = writeln!(out, "# backend = \"dxmt\"");
        let _ = writeln!(out, "# fallbacks = [\"wined3d\"]");
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "# [wine]");
    let _ = writeln!(
        out,
        "# windows_version = \"win10\"   # only when the game needs a version other than the bottle's: win7, win81, win10, win11"
    );
    let _ = writeln!(out);
    let _ = writeln!(out, "# [env]");
    if scan.bitness() == Bitness::X86 && !scan.info.large_address_aware {
        let _ = writeln!(
            out,
            "# WINE_LARGE_ADDRESS_AWARE = \"1\"   # 4 GiB of address space for this 32-bit, not large-address-aware game, on runtimes that honor it"
        );
    } else {
        let _ = writeln!(
            out,
            "# DXMT_CONFIG = \"d3d11.preferredMaxFrameRate=60;\"   # backend options; see docs/PERFORMANCE.md"
        );
    }
    let _ = writeln!(out);
    let _ = writeln!(out, "# [dll_overrides]");
    let _ = writeln!(
        out,
        "# d3dcompiler_47 = \"n,b\"   # the game's own HLSL compiler before Wine's; \"n\" native only, \"b\" builtin only, \"d\" disabled"
    );
    let _ = writeln!(out);
    let _ = writeln!(
        out,
        "# An INI key enforced before every launch; the value may use {{display.width}}, {{display.height}} and {{display.refresh}}."
    );
    let _ = writeln!(out, "# [[ini]]");
    let _ = writeln!(out, "# file = '%APPDATA%\\Publisher\\Game\\settings.ini'");
    let _ = writeln!(out, "# section = \"Video\"");
    let _ = writeln!(out, "# key = \"Fullscreen\"");
    let _ = writeln!(out, "# value = \"0\"");
    let _ = writeln!(
        out,
        "# reason = \"Why the game needs it; shown by `uncork profile show`.\""
    );
    let _ = writeln!(out);
    out.push_str(&comment(
        "Set status only after a session through the checklist in docs/COMPATIBILITY.md, and add that session as the first [[compat.reports]] entry.",
    ));
    let _ = writeln!(out, "[compat]");
    let _ = writeln!(out, "status = \"untested\"");
    let _ = writeln!(out, "notes = \"\"");
    out
}

/// The TOML spelling of an API ([`GraphicsApi`]'s serde name).
fn api_key(api: GraphicsApi) -> &'static str {
    match api {
        GraphicsApi::DirectDraw => "directdraw",
        GraphicsApi::OpenGl => "opengl",
        GraphicsApi::D3d8 => "d3d8",
        GraphicsApi::D3d9 => "d3d9",
        GraphicsApi::D3d10 => "d3d10",
        GraphicsApi::D3d11 => "d3d11",
        GraphicsApi::D3d12 => "d3d12",
        GraphicsApi::Vulkan => "vulkan",
    }
}

/// Write `text` to `path`, which must not exist yet.
fn write_new_file(path: &Path, text: &str) -> anyhow::Result<()> {
    use std::io::Write as _;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => bail!(
            "{} already exists; choose another --out, or remove it first",
            path.display()
        ),
        Err(err) => return Err(err).with_context(|| format!("cannot write {}", path.display())),
    };
    file.write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))
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
