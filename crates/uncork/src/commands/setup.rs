//! `uncork setup`: components, the Steam bottle and Steam in one go.

use std::process::ExitCode;

use anyhow::{Context as _, bail};
use uncork_core::bottle::{self, Bottle, CreateOptions};
use uncork_core::catalog::{Catalog, CatalogEntry};
use uncork_core::component::ComponentKind;
use uncork_core::host::{HostInfo, Rosetta};

use super::{Ctx, confirm_download, newest_wine};
use crate::cli::SetupArgs;
use crate::output::human_bytes;

/// Components `uncork setup` installs (catalog defaults).
pub(super) const SETUP_KINDS: [ComponentKind; 3] = [
    ComponentKind::Wine,
    ComponentKind::Dxmt,
    ComponentKind::Dxvk,
];

/// Run the whole setup. Every download is listed and confirmed once before
/// anything is fetched.
pub(super) fn run(ctx: &Ctx, args: &SetupArgs) -> anyhow::Result<ExitCode> {
    bottle::validate_name(&args.bottle)?;
    check_rosetta(&HostInfo::probe(ctx.layout.root()))?;

    let catalog = Catalog::builtin();
    let components = ctx.components()?;
    let missing = SETUP_KINDS
        .iter()
        .filter(|kind| super::newest(&components, **kind).is_none())
        .map(|kind| {
            catalog
                .default_for(*kind)
                .with_context(|| format!("the built-in catalog has no {kind} entry"))
        })
        .collect::<anyhow::Result<Vec<&CatalogEntry>>>()?;

    let existing = match Bottle::open_named(&ctx.layout, &args.bottle) {
        Ok(bottle) => Some(bottle),
        Err(uncork_core::Error::NotFound { .. }) => None,
        Err(err) => return Err(err.into()),
    };
    let needs_steam_download = !args.no_steam
        && existing
            .as_ref()
            .is_none_or(|bottle| uncork_steam::SteamInstall::find(bottle.prefix()).is_none());

    if let Some(what) = download_summary(&missing, needs_steam_download) {
        confirm_download(&what, args.yes)?;
    }

    for entry in &missing {
        super::runtime::install_entry(ctx, entry)?;
    }

    let mut bottle = match existing {
        Some(bottle) => {
            println!(
                "Bottle {} already exists ({}).",
                args.bottle,
                bottle.path.display()
            );
            bottle
        }
        None => {
            let wine = newest_wine(&ctx.layout)?;
            super::bottle::create_bottle(ctx, &args.bottle, &wine, &CreateOptions::default())?
        }
    };
    // A bottle an interrupted setup left half-made is finished first, with
    // the boot steps' watchdog and registry defaults.
    let wine = if args.no_steam && bottle.is_initialized() {
        None
    } else {
        let wine = super::bottle_wine(&ctx.layout, &bottle)?;
        super::bottle::ensure_initialized(ctx, &mut bottle, &wine)?;
        Some(wine)
    };

    let mut config = ctx.config()?;
    if config.default_bottle != args.bottle {
        config.default_bottle.clone_from(&args.bottle);
        config.save(&ctx.layout)?;
        println!("Default bottle is now {}.", args.bottle);
    }

    let Some(wine) = wine.filter(|_| !args.no_steam) else {
        println!("Components and bottle are ready. Install Steam later with: uncork steam install");
        return Ok(ExitCode::SUCCESS);
    };
    super::steam::install_and_start(ctx, &mut bottle, &wine)
}

/// Fail early, with the fix, when Rosetta 2 is missing; Wine is x86-64.
fn check_rosetta(host: &HostInfo) -> anyhow::Result<()> {
    match host.rosetta {
        Rosetta::Installed | Rosetta::NotNeeded => Ok(()),
        Rosetta::Missing => bail!(
            "Rosetta 2 is not installed, and Wine cannot run without it\n      fix: softwareupdate --install-rosetta --agree-to-license"
        ),
        Rosetta::Unknown => {
            eprintln!(
                "warning: cannot tell whether Rosetta 2 is installed; if Wine fails to start, run: softwareupdate --install-rosetta --agree-to-license"
            );
            Ok(())
        }
    }
}

/// Size of Valve's `SteamSetup.exe` last time it was checked (it changes in place).
pub(super) const STEAM_INSTALLER_BYTES: u64 = 2_380_800;

/// "466.1 MB (wine 1.0 461.1 MB, ..., Valve's Steam installer)", or `None`
/// when nothing needs downloading.
fn download_summary(entries: &[&CatalogEntry], steam_installer: bool) -> Option<String> {
    let mut parts: Vec<String> = entries
        .iter()
        .map(|entry| {
            format!(
                "{} {} {}",
                entry.kind,
                entry.version,
                human_bytes(entry.size)
            )
        })
        .collect();
    let mut total: u64 = entries.iter().map(|entry| entry.size).sum();
    if steam_installer {
        parts.push(format!(
            "Valve's Steam installer {}",
            human_bytes(STEAM_INSTALLER_BYTES)
        ));
        total += STEAM_INSTALLER_BYTES;
    }
    (!parts.is_empty()).then(|| format!("{} ({})", human_bytes(total), parts.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_lists_every_download_with_the_total() {
        let catalog = Catalog::builtin();
        let entries: Vec<&CatalogEntry> = SETUP_KINDS
            .iter()
            .filter_map(|kind| catalog.default_for(*kind))
            .collect();
        let summary = download_summary(&entries, true).unwrap();
        let total: u64 = entries.iter().map(|e| e.size).sum::<u64>() + STEAM_INSTALLER_BYTES;
        assert!(summary.starts_with(&human_bytes(total)), "{summary}");
        assert!(
            summary.contains("wine winecx-gptk-4.7.3 461.1 MB"),
            "{summary}"
        );
        assert!(summary.contains("dxmt 0.80"), "{summary}");
        assert!(
            summary.contains("Valve's Steam installer 2.4 MB"),
            "{summary}"
        );
    }

    #[test]
    fn nothing_to_download_needs_no_question() {
        assert_eq!(download_summary(&[], false), None);
        assert_eq!(
            download_summary(&[], true).as_deref(),
            Some("2.4 MB (Valve's Steam installer 2.4 MB)")
        );
    }

    #[test]
    fn missing_rosetta_is_an_error_with_the_fix() {
        let mut host = HostInfo {
            os: "macos".to_owned(),
            apple_silicon: true,
            macos_version: None,
            macos_build: None,
            chip: None,
            memory_bytes: None,
            performance_cores: None,
            rosetta: Rosetta::Missing,
            free_bytes: None,
            crossover_installed: false,
        };
        let err = check_rosetta(&host).unwrap_err().to_string();
        assert!(err.contains("softwareupdate --install-rosetta"), "{err}");
        host.rosetta = Rosetta::Installed;
        assert!(check_rosetta(&host).is_ok());
    }
}
