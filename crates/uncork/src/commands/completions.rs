//! `uncork completions <shell>`: a completion script for the shell, made
//! from the same command definition as `--help`, so it knows every command
//! and option. `clap_complete`'s static scripts have no notion of a hidden
//! subcommand, so `__exec` and `__manpage` complete too.

use std::io::Write as _;
use std::process::ExitCode;

use anyhow::Context as _;
use clap::CommandFactory as _;
use clap_complete::Shell;

use crate::cli::Cli;

/// Print the completion script for `shell` to stdout.
pub(super) fn run(shell: Shell) -> anyhow::Result<ExitCode> {
    std::io::stdout()
        .lock()
        .write_all(&script(shell))
        .context("cannot write the completion script to stdout")?;
    Ok(ExitCode::SUCCESS)
}

/// The completion script for `shell`.
fn script(shell: Shell) -> Vec<u8> {
    let mut buf = Vec::new();
    clap_complete::generate(shell, &mut Cli::command(), "uncork", &mut buf);
    buf
}

#[cfg(test)]
mod tests {
    use clap::ValueEnum as _;

    use super::*;

    #[test]
    fn the_zsh_script_completes_the_commands() {
        let script = String::from_utf8(script(Shell::Zsh)).unwrap();
        assert!(script.starts_with("#compdef uncork"), "{script}");
        // `_describe` entries, `'name:help'`.
        for command in ["play", "doctor", "bottle", "completions"] {
            assert!(
                script.contains(&format!("'{command}:")),
                "{command}: {script}"
            );
        }
        assert!(script.contains("--dry-run"), "{script}");
    }

    #[test]
    fn every_shell_gets_a_script() {
        for shell in Shell::value_variants() {
            let script = String::from_utf8(script(*shell)).unwrap();
            assert!(script.contains("uncork"), "{shell}: {script}");
        }
    }
}
