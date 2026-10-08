//! `uncork doctor`.

use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use uncork_core::host::{self, Check, HostInfo, Status};

use super::Ctx;
use crate::output;

/// `--json` output.
#[derive(Debug, Serialize)]
struct DoctorView<'a> {
    /// Uncork's version.
    version: &'static str,
    /// The data root.
    data_root: &'a Path,
    /// Probed host facts.
    host: &'a HostInfo,
    /// Every check, in `host::evaluate` order.
    checks: &'a [Check],
    /// `false` when any check failed.
    ok: bool,
}

/// Probe the host, evaluate every check and print them; exit code 1 when
/// any check failed.
pub(super) fn run(ctx: &Ctx) -> anyhow::Result<ExitCode> {
    let host = HostInfo::probe(ctx.layout.root());
    let components = ctx.components()?;
    let bottles = uncork_core::bottle::list(&ctx.layout)?;
    let mut checks = host::evaluate(&host, &components, &bottles);
    // Each bottle's Retina mode and DPI, from its user.reg, after the
    // bottle-wine checks.
    let dpi: Vec<Check> = bottles
        .iter()
        .filter_map(|bottle| {
            let registry = uncork_core::bottle::DisplayRegistry::read(&bottle.path)?;
            host::bottle_dpi(bottle, &registry)
        })
        .collect();
    let at = checks
        .iter()
        .position(|check| check.id == "crossover")
        .unwrap_or(checks.len());
    checks.splice(at..at, dpi);
    let failed = checks.iter().filter(|c| c.status == Status::Fail).count();

    if ctx.json {
        output::print_json(&DoctorView {
            version: env!("CARGO_PKG_VERSION"),
            data_root: ctx.layout.root(),
            host: &host,
            checks: &checks,
            ok: failed == 0,
        })?;
    } else {
        println!("Uncork {}", env!("CARGO_PKG_VERSION"));
        println!("Data: {}", ctx.layout.root().display());
        println!();
        print!("{}", render_checks(&checks));
        println!();
        match failed {
            0 => println!("No problems that stop games from running."),
            1 => println!("1 problem must be fixed before games can run."),
            n => println!("{n} problems must be fixed before games can run."),
        }
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// `ok    summary` rows (status padded to four characters) with
/// `      fix: <command>` under the checks that have a fix.
fn render_checks(checks: &[Check]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!("{:<4}  {}\n", label(check.status), check.summary));
        if let Some(fix) = &check.fix {
            out.push_str(&format!("      fix: {fix}\n"));
        }
    }
    out
}

fn label(status: Status) -> &'static str {
    match status {
        Status::Ok => "ok",
        Status::Info => "info",
        Status::Warn => "warn",
        Status::Fail => "FAIL",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_are_aligned_with_fixes_underneath() {
        let host = HostInfo {
            os: "macos".to_owned(),
            apple_silicon: true,
            macos_version: Some("27.0.1".to_owned()),
            macos_build: Some("26A434".to_owned()),
            chip: Some("Apple M5 Max".to_owned()),
            memory_bytes: None,
            performance_cores: None,
            rosetta: host::Rosetta::Installed,
            free_bytes: None,
            crossover_installed: false,
        };
        let text = render_checks(&host::evaluate(&host, &[], &[]));
        assert!(
            text.contains("ok    macOS on Apple Silicon (Apple M5 Max)\n"),
            "{text}"
        );
        assert!(
            text.contains(
                "FAIL  no Wine runtime is installed\n      fix: uncork runtime install wine\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("info  no bottles yet\n      fix: uncork setup\n"),
            "{text}"
        );
        assert!(text.lines().all(|line| line == line.trim_end()), "{text}");
    }
}
