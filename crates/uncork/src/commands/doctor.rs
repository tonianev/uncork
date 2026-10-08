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
    let count = |status: Status| checks.iter().filter(|c| c.status == status).count();
    let failed = count(Status::Fail);

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
        println!("{}", summary(failed, count(Status::Warn)));
    }
    Ok(if failed == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

/// The last line of the report. Warnings are named there too: some of
/// them break particular games (a bottle whose Retina mode and DPI
/// disagree crashes Rise of Nations), and a reader who skims to the end
/// should not miss them.
fn summary(failed: usize, warned: usize) -> String {
    let warnings = match warned {
        1 => "1 warning above".to_owned(),
        n => format!("{n} warnings above"),
    };
    match (failed, warned) {
        (0, 0) => "No problems that stop games from running.".to_owned(),
        (0, _) => format!(
            "No problems that stop games from running, but the {warnings} can still affect some games."
        ),
        (1, 0) => "1 problem must be fixed before games can run.".to_owned(),
        (n, 0) => format!("{n} problems must be fixed before games can run."),
        (1, _) => format!(
            "1 problem must be fixed before games can run, and the {warnings} can affect some games."
        ),
        (n, _) => format!(
            "{n} problems must be fixed before games can run, and the {warnings} can affect some games."
        ),
    }
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
    fn the_last_line_names_failures_and_warnings() {
        assert_eq!(summary(0, 0), "No problems that stop games from running.");
        assert_eq!(
            summary(0, 1),
            "No problems that stop games from running, but the 1 warning above can still affect some games."
        );
        assert_eq!(
            summary(0, 2),
            "No problems that stop games from running, but the 2 warnings above can still affect some games."
        );
        assert_eq!(
            summary(1, 0),
            "1 problem must be fixed before games can run."
        );
        assert_eq!(
            summary(2, 0),
            "2 problems must be fixed before games can run."
        );
        assert_eq!(
            summary(2, 3),
            "2 problems must be fixed before games can run, and the 3 warnings above can affect some games."
        );
    }

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
