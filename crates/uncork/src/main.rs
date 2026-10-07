//! The `uncork` command.

mod cli;
mod commands;
mod output;

use clap::Parser;

fn main() -> std::process::ExitCode {
    let cli = cli::Cli::parse();
    output::init_logging(cli.verbose);
    match commands::dispatch(&cli) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            output::print_error(&err);
            std::process::ExitCode::FAILURE
        }
    }
}
