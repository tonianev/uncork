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
        return finish(result);
    }
    let cli = cli::Cli::parse();
    output::init_logging(cli.verbose);
    finish(commands::dispatch(&cli))
}

/// Turn a command's result into the process exit code, printing the error
/// chain on failure.
fn finish(result: anyhow::Result<ExitCode>) -> ExitCode {
    match result {
        Ok(code) => code,
        Err(err) => {
            output::print_error(&err);
            ExitCode::FAILURE
        }
    }
}
