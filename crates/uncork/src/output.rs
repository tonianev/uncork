//! Terminal output: logging setup, errors, tables, JSON, progress, prompts.

use std::io::IsTerminal;

/// Configure `tracing` from `-v` count; `RUST_LOG` wins when set.
/// 0 → warn, 1 → info, 2 → debug, 3+ → trace. Output to stderr, no timestamps.
pub fn init_logging(verbose: u8) {
    let _ = verbose;
    todo!()
}

/// Print `error: <message>` and each `caused by: <source>` line to stderr.
pub fn print_error(err: &anyhow::Error) {
    let _ = err;
    todo!()
}

/// Print `value` as pretty JSON to stdout.
///
/// # Errors
/// Serialization failure.
pub fn print_json<T: serde::Serialize>(value: &T) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Render rows as an aligned plain-text table with a header row; columns
/// separated by two spaces; no trailing whitespace.
#[must_use]
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let _ = (header, rows);
    todo!()
}

/// Ask a yes/no question on stderr; `default_yes` decides the empty answer.
/// Returns `false` without asking when stdin is not a terminal.
#[must_use]
pub fn confirm(question: &str, default_yes: bool) -> bool {
    let _ = (question, default_yes, std::io::stdin().is_terminal());
    todo!()
}

/// A download progress reporter that prints a single updating line to
/// stderr when it is a terminal (`label  42% 70.1/167.3 MB`), and one line
/// at start and finish otherwise.
#[derive(Debug, Default)]
pub struct TerminalProgress {
    label: String,
    total: Option<u64>,
    done: u64,
    last_percent: Option<u64>,
}

impl uncork_core::download::Progress for TerminalProgress {
    fn start(&mut self, label: &str, total: Option<u64>) {
        let _ = (&self.label, self.total, self.done, self.last_percent, label, total);
        todo!()
    }

    fn advance(&mut self, bytes: u64) {
        let _ = bytes;
        todo!()
    }

    fn finish(&mut self) {
        todo!()
    }
}

/// Human-readable byte size: `512 B`, `1.5 KB`, `167.3 MB`, `2.1 GB` (base 1000).
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    let _ = bytes;
    todo!()
}
