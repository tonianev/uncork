//! `uncork inspect <exe>`: what a Windows program is and which backend
//! Uncork would pick for it.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use anyhow::Context as _;
use serde::Serialize;
use uncork_core::catalog::Catalog;
use uncork_core::component::{ComponentKind, InstalledComponent};
use uncork_core::graphics::{self, Availability, Recommendation};
use uncork_pe::{ApiEvidence, Bitness, EvidenceKind, GameScan, GraphicsApi, Machine};

use super::{Ctx, key_values, newest, yes_no};
use crate::cli::InspectArgs;
use crate::output;

/// Which backends could be used now: installed components, judged against
/// the newest installed Wine's features (or, with no Wine installed yet,
/// the features of the Wine the catalog recommends).
fn availability(components: &[InstalledComponent]) -> Availability {
    let features: Vec<String> = match newest(components, ComponentKind::Wine) {
        Some(wine) => wine.meta.features.clone(),
        None => Catalog::builtin()
            .default_for(ComponentKind::Wine)
            .map(|entry| entry.features.clone())
            .unwrap_or_default(),
    };
    let wine_has = |feature: &str| features.iter().any(|f| f == feature);
    let installed = |kind: ComponentKind| newest(components, kind).is_some();
    Availability {
        dxmt: installed(ComponentKind::Dxmt) && wine_has("dxmt"),
        dxvk: installed(ComponentKind::Dxvk),
        d3dmetal: installed(ComponentKind::D3dmetal) && wine_has("d3dmetal"),
    }
}

/// `availability` in `--json` output.
#[derive(Debug, Serialize)]
struct AvailabilityView {
    dxmt: bool,
    dxvk: bool,
    d3dmetal: bool,
}

/// A sibling DLL the scan skipped.
#[derive(Debug, Serialize)]
struct SkippedView<'a> {
    file: &'a str,
    reason: &'a str,
}

/// `inspect --json`.
#[derive(Debug, Serialize)]
struct InspectView<'a> {
    exe: &'a Path,
    machine: Machine,
    bitness: Bitness,
    large_address_aware: bool,
    nx_compat: bool,
    subsystem: u16,
    imports: &'a [String],
    apis: &'a BTreeSet<GraphicsApi>,
    primary_api: Option<GraphicsApi>,
    evidence: &'a [ApiEvidence],
    uses_steamworks: bool,
    anti_cheat: &'a [String],
    non_nx_modules: &'a [String],
    skipped: Vec<SkippedView<'a>>,
    availability: AvailabilityView,
    recommendation: &'a Recommendation,
}

/// Scan the executable and the DLLs beside it, and recommend a backend.
pub(super) fn run(ctx: &Ctx, args: &InspectArgs) -> anyhow::Result<ExitCode> {
    let scan = uncork_pe::scan_game(&args.exe)
        .with_context(|| format!("cannot inspect {}", args.exe.display()))?;
    let components = ctx.components()?;
    let available = availability(&components);
    let recommendation = graphics::recommend(
        scan.primary_api(),
        scan.bitness(),
        &available,
        None,
        &[],
        false,
    );
    if ctx.json {
        output::print_json(&InspectView {
            exe: &scan.exe,
            machine: scan.info.machine,
            bitness: scan.bitness(),
            large_address_aware: scan.info.large_address_aware,
            nx_compat: scan.info.nx_compat,
            subsystem: scan.info.subsystem,
            imports: &scan.info.imports,
            apis: &scan.apis,
            primary_api: scan.primary_api(),
            evidence: &scan.evidence,
            uses_steamworks: scan.uses_steamworks,
            anti_cheat: &scan.anti_cheat,
            non_nx_modules: &scan.non_nx_modules,
            skipped: scan
                .skipped
                .iter()
                .map(|(file, reason)| SkippedView { file, reason })
                .collect(),
            availability: AvailabilityView {
                dxmt: available.dxmt,
                dxvk: available.dxvk,
                d3dmetal: available.d3dmetal,
            },
            recommendation: &recommendation,
        })?;
    } else {
        print!("{}", render(&scan, &recommendation));
    }
    Ok(ExitCode::SUCCESS)
}

fn api_label(api: GraphicsApi) -> &'static str {
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

fn render(scan: &GameScan, recommendation: &Recommendation) -> String {
    let bitness = scan.bitness();
    let architecture = match bitness {
        Bitness::X86 => "32-bit (PE32, x86)",
        Bitness::X64 => "64-bit (PE32+, x86-64)",
    };
    let large_address = match (bitness, scan.info.large_address_aware) {
        (_, true) => "yes".to_owned(),
        (Bitness::X86, false) => {
            "no: 2 GiB of address space (WINE_LARGE_ADDRESS_AWARE=1 lifts this on runtimes that honor it)"
                .to_owned()
        }
        (Bitness::X64, false) => "no".to_owned(),
    };
    let apis = if scan.evidence.is_empty() {
        "none detected".to_owned()
    } else {
        scan.evidence
            .iter()
            .map(|evidence| {
                let how = match evidence.kind {
                    EvidenceKind::Import => "imported by",
                    EvidenceKind::String => "named in",
                };
                format!(
                    "{}: {} {how} {}",
                    api_label(evidence.api),
                    evidence.import,
                    evidence.file
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    let list_or = |items: &[String], empty: &str| {
        if items.is_empty() {
            empty.to_owned()
        } else {
            items.join(", ")
        }
    };
    let mut rows = vec![
        ("File", scan.exe.display().to_string()),
        ("Architecture", architecture.to_owned()),
        ("Large address", large_address),
        ("NX compatible", yes_no(scan.info.nx_compat).to_owned()),
        ("Graphics APIs", apis),
        (
            "Steamworks",
            if scan.uses_steamworks {
                "yes: needs the Steam client running in the bottle".to_owned()
            } else {
                "no".to_owned()
            },
        ),
        (
            "Anti-cheat",
            if scan.anti_cheat.is_empty() {
                "none found".to_owned()
            } else {
                format!(
                    "{} (kernel anti-cheat does not run under Wine)",
                    scan.anti_cheat.join(", ")
                )
            },
        ),
        ("Non-NX modules", list_or(&scan.non_nx_modules, "none")),
    ];
    if !scan.skipped.is_empty() {
        rows.push((
            "Skipped",
            scan.skipped
                .iter()
                .map(|(file, reason)| format!("{file}: {reason}"))
                .collect::<Vec<_>>()
                .join("\n"),
        ));
    }
    rows.push((
        "Backend",
        format!("{}\n{}", recommendation.backend, recommendation.reason),
    ));
    key_values(&rows)
}
