//! `uncork runtime ...`: installed and available components.

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, anyhow, bail};
use serde::Serialize;
use uncork_core::catalog::{Catalog, CatalogEntry};
use uncork_core::component::{self, ComponentKind, InstalledComponent, Source};

use super::Ctx;
use super::setup::SETUP_KINDS;
use crate::cli::RuntimeCommand;
use crate::output::{self, TerminalProgress, human_bytes};

/// Dispatch a `runtime` subcommand.
pub(super) fn run(ctx: &Ctx, command: &RuntimeCommand) -> anyhow::Result<ExitCode> {
    match command {
        RuntimeCommand::List => list(ctx),
        RuntimeCommand::Available => available(ctx),
        RuntimeCommand::Install {
            kinds,
            version,
            yes,
        } => install(ctx, kinds, version.as_deref(), *yes),
        RuntimeCommand::Remove { kind, version } => remove(ctx, kind, version),
        RuntimeCommand::ImportGptk { path } => import_gptk(ctx, path),
    }
}

/// One installed component in `runtime list --json`.
#[derive(Debug, Serialize)]
struct InstalledView<'a> {
    kind: ComponentKind,
    version: &'a str,
    license: &'a str,
    features: &'a [String],
    /// `catalog` or `local`.
    source: &'static str,
    /// Download URL or the directory it was copied from.
    origin: String,
    source_code: Option<&'a str>,
    installed_unix: u64,
    path: &'a Path,
}

impl<'a> InstalledView<'a> {
    fn new(component: &'a InstalledComponent) -> InstalledView<'a> {
        let meta = &component.meta;
        let (source, origin) = match &meta.source {
            Source::Catalog { url, .. } => ("catalog", url.clone()),
            Source::Local { path } => ("local", path.display().to_string()),
        };
        InstalledView {
            kind: meta.kind,
            version: &meta.version,
            license: &meta.license,
            features: &meta.features,
            source,
            origin,
            source_code: meta.source_code.as_deref(),
            installed_unix: meta.installed_unix,
            path: &component.path,
        }
    }
}

fn list(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let components = ctx.components()?;
    if ctx.json {
        let views: Vec<InstalledView<'_>> = components.iter().map(InstalledView::new).collect();
        output::print_json(&views)?;
        return Ok(ExitCode::SUCCESS);
    }
    if components.is_empty() {
        println!("No components installed. Run `uncork runtime install` (or `uncork setup`).");
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = components
        .iter()
        .map(|component| {
            let view = InstalledView::new(component);
            vec![
                view.kind.to_string(),
                view.version.to_owned(),
                view.license.to_owned(),
                view.source.to_owned(),
                view.features.join(","),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(&["KIND", "VERSION", "LICENSE", "SOURCE", "FEATURES"], &rows)
    );
    Ok(ExitCode::SUCCESS)
}

/// One catalog entry in `runtime available --json`.
#[derive(Debug, Serialize)]
struct AvailableView<'a> {
    kind: ComponentKind,
    version: &'a str,
    size: u64,
    license: &'a str,
    recommended: bool,
    installed: bool,
    features: &'a [String],
    url: &'a str,
    sha256: &'a str,
    homepage: &'a str,
    source_code: &'a str,
    notes: &'a str,
}

fn available(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let catalog = Catalog::builtin();
    let components = ctx.components()?;
    let installed = |entry: &CatalogEntry| {
        components
            .iter()
            .any(|c| c.meta.kind == entry.kind && c.meta.version == entry.version)
    };
    let entries: Vec<&CatalogEntry> = ComponentKind::ALL
        .iter()
        .flat_map(|kind| catalog.entries(*kind))
        .collect();
    if ctx.json {
        let views: Vec<AvailableView<'_>> = entries
            .iter()
            .map(|entry| AvailableView {
                kind: entry.kind,
                version: &entry.version,
                size: entry.size,
                license: &entry.license,
                recommended: entry.recommended,
                installed: installed(entry),
                features: &entry.features,
                url: &entry.url,
                sha256: &entry.sha256,
                homepage: &entry.homepage,
                source_code: &entry.source_code,
                notes: &entry.notes,
            })
            .collect();
        output::print_json(&views)?;
        return Ok(ExitCode::SUCCESS);
    }
    let rows: Vec<Vec<String>> = entries
        .iter()
        .map(|entry| {
            let status: Vec<&str> = [
                (entry.recommended, "recommended"),
                (installed(entry), "installed"),
            ]
            .iter()
            .filter_map(|(on, word)| on.then_some(*word))
            .collect();
            vec![
                entry.kind.to_string(),
                entry.version.clone(),
                human_bytes(entry.size),
                entry.license.clone(),
                status.join(", "),
                entry.notes.clone(),
            ]
        })
        .collect();
    print!(
        "{}",
        output::table(
            &["KIND", "VERSION", "SIZE", "LICENSE", "STATUS", "NOTES"],
            &rows
        )
    );
    println!();
    println!(
        "D3DMetal is not downloadable: import your own copy of Apple's Game Porting Toolkit with `uncork runtime import-gptk <path>`."
    );
    Ok(ExitCode::SUCCESS)
}

/// The kinds `runtime install` was asked for: none or `all` means every
/// kind `uncork setup` installs.
fn requested_kinds(kinds: &[String]) -> anyhow::Result<Vec<ComponentKind>> {
    if kinds.is_empty() || kinds.iter().any(|kind| kind.eq_ignore_ascii_case("all")) {
        return Ok(SETUP_KINDS.to_vec());
    }
    let mut parsed = Vec::new();
    for kind in kinds {
        let kind: ComponentKind = kind.parse().map_err(|message: String| anyhow!(message))?;
        if kind == ComponentKind::D3dmetal {
            bail!(
                "D3DMetal is never downloaded: import your own copy of Apple's Game Porting Toolkit with `uncork runtime import-gptk <path>`"
            );
        }
        if !parsed.contains(&kind) {
            parsed.push(kind);
        }
    }
    Ok(parsed)
}

/// The catalog entries to install for `kinds` (`version` only with one kind).
fn entries_for<'a>(
    catalog: &'a Catalog,
    kinds: &[ComponentKind],
    version: Option<&str>,
) -> anyhow::Result<Vec<&'a CatalogEntry>> {
    match (kinds, version) {
        ([kind], Some(version)) => {
            let entry = catalog.find(*kind, version).ok_or_else(|| {
                let known: Vec<&str> = catalog
                    .entries(*kind)
                    .iter()
                    .map(|entry| entry.version.as_str())
                    .collect();
                anyhow!(
                    "the catalog has no {kind} {version}; available: {}",
                    if known.is_empty() {
                        "none".to_owned()
                    } else {
                        known.join(", ")
                    }
                )
            })?;
            Ok(vec![entry])
        }
        (_, Some(_)) => bail!(
            "--version needs exactly one component kind, e.g. `uncork runtime install dxmt --version 0.80`"
        ),
        (kinds, None) => kinds
            .iter()
            .map(|kind| {
                catalog
                    .default_for(*kind)
                    .with_context(|| format!("the catalog has no {kind} entry"))
            })
            .collect(),
    }
}

fn install(
    ctx: &Ctx,
    kinds: &[String],
    version: Option<&str>,
    yes: bool,
) -> anyhow::Result<ExitCode> {
    let kinds = requested_kinds(kinds)?;
    let catalog = Catalog::builtin();
    let components = ctx.components()?;
    let mut wanted = Vec::new();
    for entry in entries_for(&catalog, &kinds, version)? {
        let installed = components
            .iter()
            .any(|c| c.meta.kind == entry.kind && c.meta.version == entry.version);
        if installed {
            println!("{} {} is already installed.", entry.kind, entry.version);
        } else {
            wanted.push(entry);
        }
    }
    if wanted.is_empty() {
        return Ok(ExitCode::SUCCESS);
    }
    let total: u64 = wanted.iter().map(|entry| entry.size).sum();
    let names: Vec<String> = wanted
        .iter()
        .map(|entry| format!("{} {}", entry.kind, entry.version))
        .collect();
    super::confirm_download(
        &format!("{} ({})", human_bytes(total), names.join(", ")),
        yes,
    )?;
    for entry in wanted {
        install_entry(ctx, entry)?;
    }
    Ok(ExitCode::SUCCESS)
}

/// Download, verify and install one catalog entry, with a progress bar.
pub(super) fn install_entry(ctx: &Ctx, entry: &CatalogEntry) -> anyhow::Result<()> {
    eprintln!(
        "Installing {} {} ({}, {})",
        entry.kind,
        entry.version,
        human_bytes(entry.size),
        entry.license
    );
    let installed =
        component::install_from_catalog(&ctx.layout, entry, &mut TerminalProgress::default())
            .with_context(|| format!("cannot install {} {}", entry.kind, entry.version))?;
    println!(
        "Installed {} {} in {}",
        entry.kind,
        entry.version,
        installed.path.display()
    );
    Ok(())
}

fn remove(ctx: &Ctx, kind: &str, version: &str) -> anyhow::Result<ExitCode> {
    let kind: ComponentKind = kind.parse().map_err(|message: String| anyhow!(message))?;
    component::remove(&ctx.layout, kind, version)?;
    println!("Removed {kind} {version}.");
    if kind == ComponentKind::Wine {
        let stranded: Vec<String> = uncork_core::bottle::list(&ctx.layout)?
            .into_iter()
            .filter(|bottle| bottle.config.wine == version)
            .map(|bottle| bottle.config.name)
            .collect();
        for name in stranded {
            eprintln!(
                "warning: bottle {name} uses Wine {version}; reinstall it or run `uncork bottle set {name} wine=<installed version>`"
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn import_gptk(ctx: &Ctx, path: &Path) -> anyhow::Result<ExitCode> {
    let installed = component::import_gptk(&ctx.layout, path)
        .with_context(|| format!("cannot import D3DMetal from {}", path.display()))?;
    println!(
        "Imported D3DMetal {} into {}",
        installed.meta.version,
        installed.path.display()
    );
    println!(
        "D3DMetal stays under Apple's Game Porting Toolkit license: Uncork uses your copy where it is and never redistributes it."
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(values: &[&str]) -> Vec<String> {
        values.iter().map(|&value| value.to_owned()).collect()
    }

    #[test]
    fn no_kinds_or_all_means_the_setup_set() {
        assert_eq!(requested_kinds(&[]).unwrap(), SETUP_KINDS.to_vec());
        assert_eq!(
            requested_kinds(&strings(&["dxmt", "ALL"])).unwrap(),
            SETUP_KINDS.to_vec()
        );
    }

    #[test]
    fn kinds_are_parsed_and_deduplicated() {
        assert_eq!(
            requested_kinds(&strings(&["DXMT", "wine", "dxmt"])).unwrap(),
            vec![ComponentKind::Dxmt, ComponentKind::Wine]
        );
        let err = requested_kinds(&strings(&["vulkan"])).unwrap_err();
        assert!(err.to_string().contains("unknown component kind"), "{err}");
        let err = requested_kinds(&strings(&["gptk"])).unwrap_err();
        assert!(err.to_string().contains("import-gptk"), "{err}");
    }

    #[test]
    fn version_needs_one_kind_and_a_catalog_entry() {
        let catalog = Catalog::builtin();
        let entries = entries_for(&catalog, &[ComponentKind::Dxmt], Some("0.80")).unwrap();
        assert_eq!(entries[0].version, "0.80");
        let err = entries_for(&catalog, &[ComponentKind::Dxmt], Some("9.9")).unwrap_err();
        assert!(err.to_string().contains("available: 0.80"), "{err}");
        let err = entries_for(&catalog, &SETUP_KINDS, Some("0.80")).unwrap_err();
        assert!(err.to_string().contains("exactly one"), "{err}");
        let defaults = entries_for(&catalog, &SETUP_KINDS, None).unwrap();
        assert_eq!(defaults.len(), 3);
    }
}
