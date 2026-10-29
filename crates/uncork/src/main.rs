//! The `uncork` command.

mod cli;
mod commands;
mod output;

use std::process::ExitCode;

use clap::Parser;

fn main() -> ExitCode {
    // A copy of this binary inside a Game Mode app bundle is started by
    // LaunchServices without arguments; it runs the bundle's saved plan.
    if let Some(result) = commands::exec::bundle_launch() {
        return finish(result, false);
    }
    // A usage error ends here, in clap: text on stderr and exit code 2,
    // `--json` or not, since the flags are not known until parsing succeeds.
    let cli = cli::Cli::parse();
    output::init_logging(cli.verbose);
    finish(commands::dispatch(&cli), cli.json)
}

/// Turn a command's result into the process exit code. A failure is
/// classified ([`output::ErrorReport`]) and printed as `error:` and
/// `caused by:` lines on stderr, or with `json` as one `{"error": ...}`
/// object on stdout; the exit code is the report's either way
/// (`docs/CLI.md`, "Exit codes and JSON errors").
fn finish(result: anyhow::Result<ExitCode>, json: bool) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(err) => {
            let report = output::ErrorReport::new(&err);
            if json {
                output::print_json_error(&report);
            } else {
                output::print_error(&report);
            }
            ExitCode::from(report.exit_code)
        }
    }
}
