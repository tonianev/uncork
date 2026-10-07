//! Command handlers. One module per command group; each loads what it needs
//! from `uncork-core`, does the work, and prints results (text or `--json`).
//!
//! Results go to stdout; progress, notes and warnings go to stderr, so
//! `--json` output and `uncork bottle env` stay machine-readable.

mod bottle;
mod doctor;
pub mod exec;
mod inspect;
mod launch;
mod profile;
mod runtime;
mod setup;
mod steam;
mod winetricks;

use std::io::IsTerminal as _;
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use uncork_core::bottle::Bottle;
use uncork_core::component::{self, ComponentKind, InstalledComponent};
use uncork_core::config::Config;
use uncork_core::paths::Layout;
use uncork_core::wine::WineRuntime;

use crate::cli::{Cli, Command};
use crate::output;

/// Run the parsed command and return the process exit code.
///
/// # Errors
/// Anything the command reports; `main` prints it.
pub fn dispatch(cli: &Cli) -> anyhow::Result<ExitCode> {
    let ctx = || Ctx::new(cli.json);
    match &cli.command {
        Command::Doctor => doctor::run(&ctx()?),
        Command::Setup(args) => setup::run(&ctx()?, args),
        Command::Play(args) => launch::play(&ctx()?, args),
        Command::Run(args) => launch::run(&ctx()?, args),
        Command::Inspect(args) => inspect::run(&ctx()?, args),
        Command::Runtime(command) => runtime::run(&ctx()?, command),
        Command::Bottle(command) => bottle::run(&ctx()?, command),
        Command::Steam(command) => steam::run(&ctx()?, command),
        Command::Profile(command) => profile::run(&ctx()?, command),
        Command::Winetricks(args) => winetricks::run(&ctx()?, args),
        Command::Exec(args) => exec::run(&args.plan),
    }
}

/// What every command needs: where the data lives and how to print.
#[derive(Debug)]
pub(crate) struct Ctx {
    /// Data directories (`$UNCORK_HOME` or `~/Library/Application Support/Uncork`).
    pub layout: Layout,
    /// `--json` was given.
    pub json: bool,
}

impl Ctx {
    fn new(json: bool) -> anyhow::Result<Ctx> {
        let layout = Layout::discover().context("cannot find Uncork's data directory")?;
        Ok(Ctx { layout, json })
    }

    /// The global settings (defaults when `config.toml` does not exist).
    pub fn config(&self) -> anyhow::Result<Config> {
        Ok(Config::load(&self.layout)?)
    }

    /// `requested`, or the configured default bottle.
    pub fn bottle_name(&self, requested: Option<&str>) -> anyhow::Result<String> {
        match requested {
            Some(name) => Ok(name.to_owned()),
            None => Ok(self.config()?.default_bottle),
        }
    }

    /// Open `requested`, or the default bottle.
    pub fn open_bottle(&self, requested: Option<&str>) -> anyhow::Result<Bottle> {
        let name = self.bottle_name(requested)?;
        let bottle = Bottle::open_named(&self.layout, &name).map_err(|err| match err {
            // The default bottle is missing on a fresh install: say how to make it.
            uncork_core::Error::NotFound { .. } if requested.is_none() => anyhow::anyhow!(
                "the default bottle {name:?} does not exist yet; run `uncork setup`, or pass --bottle <name> (`uncork bottle list` shows your bottles)"
            ),
            other => other.into(),
        })?;
        Ok(bottle)
    }

    /// Every installed component, newest first within a kind.
    pub fn components(&self) -> anyhow::Result<Vec<InstalledComponent>> {
        Ok(component::list_installed(&self.layout)?)
    }
}

/// The newest installed component of `kind` in a
/// [`component::list_installed`] listing.
pub(crate) fn newest(
    components: &[InstalledComponent],
    kind: ComponentKind,
) -> Option<&InstalledComponent> {
    components.iter().find(|c| c.meta.kind == kind)
}

/// The Wine runtime a bottle is pinned to.
pub(crate) fn bottle_wine(layout: &Layout, bottle: &Bottle) -> anyhow::Result<WineRuntime> {
    let version = &bottle.config.wine;
    let component = component::find_installed(layout, ComponentKind::Wine, Some(version))
        .with_context(|| {
            let name = &bottle.config.name;
            format!(
                "bottle {name} needs Wine {version}; install it, or switch the bottle to an installed version with `uncork bottle set {name} wine=<version>`"
            )
        })?;
    Ok(WineRuntime::from_component(&component)?)
}

/// [`bottle_wine`], or `None` (logged) when it is not installed.
pub(crate) fn bottle_wine_if_installed(layout: &Layout, bottle: &Bottle) -> Option<WineRuntime> {
    bottle_wine(layout, bottle)
        .inspect_err(|err| tracing::info!("{err:#}"))
        .ok()
}

/// The newest installed Wine runtime.
pub(crate) fn newest_wine(layout: &Layout) -> anyhow::Result<WineRuntime> {
    let component = component::find_installed(layout, ComponentKind::Wine, None)?;
    Ok(WineRuntime::from_component(&component)?)
}

/// Get the user's go-ahead for downloading `what` (e.g. "461.1 MB (wine
/// winecx-gptk-4.7.3)"): `yes` skips the question; without a terminal to
/// ask on, the answer must come from `--yes`.
///
/// # Errors
/// When the user declines, or when there is no terminal and `yes` is false.
pub(crate) fn confirm_download(what: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!(
            "downloading {what} needs your confirmation; run again with --yes to download without asking"
        );
    }
    if output::confirm(&format!("Download {what}?"), true) {
        Ok(())
    } else {
        bail!("cancelled; nothing was downloaded")
    }
}

/// Like [`confirm_download`] for a destructive action, defaulting to no.
///
/// # Errors
/// When the user declines, or when there is no terminal and `yes` is false.
pub(crate) fn confirm_action(question: &str, yes: bool) -> anyhow::Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("{question} Run again with --yes to confirm.");
    }
    if output::confirm(question, false) {
        Ok(())
    } else {
        bail!("cancelled")
    }
}

/// `^[A-Za-z_][A-Za-z0-9_]*$`: a name the shell and Wine accept as an
/// environment variable.
pub(crate) fn is_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// POSIX single quoting: `'...'` with each `'` written as `'\''`.
pub(crate) fn shell_quote(word: &str) -> String {
    format!("'{}'", word.replace('\'', r"'\''"))
}

/// `yes`/`no` for text output.
pub(crate) fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

/// Aligned `label  value` lines (the label column is as wide as the longest
/// label); a value with several lines continues under the value column.
pub(crate) fn key_values(rows: &[(&str, String)]) -> String {
    let width = rows
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut out = String::new();
    for (label, value) in rows {
        let mut lines = value.lines();
        let first = lines.next().unwrap_or("");
        let line = format!("{label:<width$}  {first}");
        out.push_str(line.trim_end());
        out.push('\n');
        for line in lines {
            let line = format!("{:<width$}  {line}", "");
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_names() {
        for name in ["A", "_", "WINEDEBUG", "a_1"] {
            assert!(is_env_name(name), "{name}");
        }
        for name in ["", "1A", "A-B", "A B", "A=B", "É"] {
            assert!(!is_env_name(name), "{name}");
        }
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(shell_quote("plain"), "'plain'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
    }

    #[test]
    fn key_value_alignment() {
        let text = key_values(&[
            ("Name", "steam".to_owned()),
            ("Environment", "A=1\nB=2".to_owned()),
            ("Empty", String::new()),
        ]);
        assert_eq!(
            text,
            "Name         steam\nEnvironment  A=1\n             B=2\nEmpty\n"
        );
    }
}
