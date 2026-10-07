//! Command handlers. One function per subcommand; each loads what it needs
//! from `uncork-core`, does the work, and prints results (text or `--json`).

use crate::cli::{Cli, Command};

/// Run the parsed command.
///
/// # Errors
/// Anything the command reports; `main` prints it.
pub fn dispatch(cli: &Cli) -> anyhow::Result<()> {
    match &cli.command {
        Command::Doctor
        | Command::Setup(_)
        | Command::Play(_)
        | Command::Run(_)
        | Command::Inspect(_)
        | Command::Runtime(_)
        | Command::Bottle(_)
        | Command::Steam(_)
        | Command::Profile(_)
        | Command::Winetricks(_) => todo!(),
    }
}
