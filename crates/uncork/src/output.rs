//! Terminal output: logging setup, errors, tables, JSON, progress, prompts.

use std::io::{BufRead, IsTerminal, Write as _};

/// Configure `tracing` from `-v` count; `RUST_LOG` wins when set.
/// 0 → warn, 1 → info, 2 → debug, 3+ → trace. Output to stderr, no timestamps.
pub fn init_logging(verbose: u8) {
    let level = match verbose {
        0 => "warn",
        1 => "info",
        2 => "debug",
        _ => "trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(level));
    // `try_init` fails only when a subscriber is already installed, which
    // leaves logging working; nothing to report.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .without_time()
        .with_target(verbose >= 2)
        .try_init();
}

/// Print `error: <message>` and each `caused by: <source>` line to stderr.
///
/// Many errors already include their source in their own message
/// (`cannot read <path>: <io error>`); a cause whose text was already
/// printed is skipped so it is not shown twice.
pub fn print_error(err: &anyhow::Error) {
    let mut stderr = std::io::stderr().lock();
    let mut printed = err.to_string();
    // Nothing useful can be done when stderr itself fails.
    let _ = writeln!(stderr, "error: {printed}");
    for cause in err.chain().skip(1) {
        let text = cause.to_string();
        if text.is_empty() || printed.contains(&text) {
            continue;
        }
        let _ = writeln!(stderr, "caused by: {text}");
        printed.push('\n');
        printed.push_str(&text);
    }
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
/// separated by two spaces; no trailing whitespace. Every line, the last
/// included, ends with a newline. Rows may be shorter than the header.
#[must_use]
pub fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let columns = rows
        .iter()
        .map(Vec::len)
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(0);
    let mut widths = vec![0; columns];
    let lines = std::iter::once(header.to_vec())
        .chain(
            rows.iter()
                .map(|row| row.iter().map(String::as_str).collect()),
        )
        .collect::<Vec<Vec<&str>>>();
    for line in &lines {
        for (width, cell) in widths.iter_mut().zip(line) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for line in &lines {
        let mut text = String::new();
        for (index, cell) in line.iter().enumerate() {
            if index > 0 {
                text.push_str("  ");
            }
            text.push_str(cell);
            let pad = widths[index].saturating_sub(cell.chars().count());
            text.extend(std::iter::repeat_n(' ', pad));
        }
        out.push_str(text.trim_end());
        out.push('\n');
    }
    out
}

/// Ask a yes/no question on stderr; `default_yes` decides the empty answer.
/// Returns `false` without asking when stdin is not a terminal, and when
/// input ends (Ctrl-D) instead of an answer.
#[must_use]
pub fn confirm(question: &str, default_yes: bool) -> bool {
    let stdin = std::io::stdin();
    if !stdin.is_terminal() {
        return false;
    }
    let hint = if default_yes { "[Y/n]" } else { "[y/N]" };
    let mut stderr = std::io::stderr().lock();
    let _ = write!(stderr, "{question} {hint} ");
    let _ = stderr.flush();
    drop(stderr);
    let answer = read_answer(&mut stdin.lock(), default_yes);
    if answer.is_none() {
        // Ctrl-D left the cursor after the question.
        eprintln!();
    }
    answer.unwrap_or(false)
}

/// One answer line from `input`: [`parse_answer`] of it, or `None` at the
/// end of input or on a read error (no answer, which never means yes).
fn read_answer(input: &mut impl BufRead, default_yes: bool) -> Option<bool> {
    let mut answer = String::new();
    match input.read_line(&mut answer) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(parse_answer(&answer, default_yes)),
    }
}

/// `y`/`yes` → true, `n`/`no` → false (any case), empty → `default_yes`,
/// anything else → false.
fn parse_answer(answer: &str, default_yes: bool) -> bool {
    match answer.trim().to_ascii_lowercase().as_str() {
        "" => default_yes,
        "y" | "yes" => true,
        _ => false,
    }
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
    /// Whether stderr is a terminal (decided at `start`).
    interactive: bool,
    /// Megabytes shown in the last redraw when the total is unknown.
    last_megabytes: Option<u64>,
    /// Something was drawn on the current line.
    drawn: bool,
}

impl TerminalProgress {
    /// Redraw the progress line if what it shows has changed.
    fn draw(&mut self) {
        let line = match self.total {
            Some(total) if total > 0 => {
                let percent =
                    u64::try_from(u128::from(self.done.min(total)) * 100 / u128::from(total))
                        .unwrap_or(100);
                if self.last_percent == Some(percent) {
                    return;
                }
                self.last_percent = Some(percent);
                format!(
                    "{}  {percent:>3}% {}",
                    self.label,
                    progress_amount(self.done.min(total), total)
                )
            }
            _ => {
                let megabytes = self.done / 1_000_000;
                if self.last_megabytes == Some(megabytes) {
                    return;
                }
                self.last_megabytes = Some(megabytes);
                format!("{}  {}", self.label, human_bytes(self.done))
            }
        };
        let mut stderr = std::io::stderr().lock();
        // \r returns to the start of the line, ESC[K clears what is left of
        // a longer previous line.
        let _ = write!(stderr, "\r{line}\x1b[K");
        let _ = stderr.flush();
        self.drawn = true;
    }
}

impl uncork_core::download::Progress for TerminalProgress {
    fn start(&mut self, label: &str, total: Option<u64>) {
        *self = TerminalProgress {
            label: label.to_owned(),
            total,
            interactive: std::io::stderr().is_terminal(),
            ..TerminalProgress::default()
        };
        if self.interactive {
            self.draw();
        } else {
            match total {
                Some(total) => eprintln!("downloading {label} ({})", human_bytes(total)),
                None => eprintln!("downloading {label}"),
            }
        }
    }

    fn advance(&mut self, bytes: u64) {
        self.done = self.done.saturating_add(bytes);
        if self.interactive {
            self.draw();
        }
    }

    fn finish(&mut self) {
        if self.interactive {
            if self.drawn {
                eprintln!();
            }
        } else {
            eprintln!("finished {} ({})", self.label, human_bytes(self.done));
        }
        self.drawn = false;
    }
}

/// Unit names for [`human_bytes`], powers of 1000.
const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];

/// Human-readable byte size: `512 B`, `1.5 KB`, `167.3 MB`, `2.1 GB` (base 1000).
#[must_use]
pub fn human_bytes(bytes: u64) -> String {
    match scale(bytes) {
        (0, _) => format!("{bytes} B"),
        (unit, divisor) => format!("{} {}", tenths(bytes, divisor), UNITS[unit]),
    }
}

/// `70.1/167.3 MB`: `done` in the unit `total` is shown in.
fn progress_amount(done: u64, total: u64) -> String {
    match scale(total) {
        (0, _) => format!("{done}/{total} B"),
        (unit, divisor) => format!(
            "{}/{} {}",
            tenths(done, divisor),
            tenths(total, divisor),
            UNITS[unit]
        ),
    }
}

/// The unit index and divisor `bytes` is shown with: the largest unit in
/// which it is at least 1 after rounding to one decimal.
fn scale(bytes: u64) -> (usize, u128) {
    if bytes < 1000 {
        return (0, 1);
    }
    let mut unit = 1;
    let mut divisor: u128 = 1000;
    while unit + 1 < UNITS.len() && rounded_tenths(bytes, divisor) >= 10_000 {
        unit += 1;
        divisor *= 1000;
    }
    (unit, divisor)
}

/// `bytes / divisor` in tenths, rounded half up.
fn rounded_tenths(bytes: u64, divisor: u128) -> u128 {
    (u128::from(bytes) * 10 + divisor / 2) / divisor
}

/// `bytes / divisor` with one decimal.
fn tenths(bytes: u64, divisor: u128) -> String {
    let tenths = rounded_tenths(bytes, divisor);
    format!("{}.{}", tenths / 10, tenths % 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_bytes_uses_base_1000_with_one_decimal() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(1500), "1.5 KB");
        assert_eq!(human_bytes(167_300_000), "167.3 MB");
        assert_eq!(human_bytes(461_131_598), "461.1 MB");
        assert_eq!(human_bytes(2_100_000_000), "2.1 GB");
        assert_eq!(human_bytes(2_380_800), "2.4 MB");
    }

    #[test]
    fn human_bytes_rounds_up_into_the_next_unit() {
        assert_eq!(human_bytes(999_949), "999.9 KB");
        assert_eq!(human_bytes(999_950), "1.0 MB");
        assert_eq!(human_bytes(999_999_999), "1.0 GB");
    }

    #[test]
    fn human_bytes_tops_out_at_terabytes() {
        assert_eq!(human_bytes(u64::MAX), "18446744.1 TB");
    }

    #[test]
    fn progress_amount_uses_the_total_unit() {
        assert_eq!(progress_amount(70_100_000, 167_300_000), "70.1/167.3 MB");
        assert_eq!(progress_amount(0, 2_380_800), "0.0/2.4 MB");
        assert_eq!(progress_amount(10, 900), "10/900 B");
    }

    #[test]
    fn table_aligns_columns_without_trailing_whitespace() {
        let rows = vec![
            vec![
                "wine".to_owned(),
                "winecx-gptk-4.7.3".to_owned(),
                "x".to_owned(),
            ],
            vec!["dxmt".to_owned(), "0.80".to_owned(), String::new()],
        ];
        let text = table(&["KIND", "VERSION", "NOTES"], &rows);
        assert_eq!(
            text,
            "KIND  VERSION            NOTES\n\
             wine  winecx-gptk-4.7.3  x\n\
             dxmt  0.80\n"
        );
    }

    #[test]
    fn table_handles_short_rows_and_no_rows() {
        assert_eq!(table(&["A", "B"], &[]), "A  B\n");
        let rows = vec![vec!["long value".to_owned()]];
        assert_eq!(table(&["A", "B"], &rows), "A           B\nlong value\n");
    }

    #[test]
    fn table_counts_characters_not_bytes() {
        let rows = vec![vec!["Café".to_owned(), "1".to_owned()]];
        assert_eq!(table(&["NAME", "N"], &rows), "NAME  N\nCafé  1\n");
    }

    #[test]
    fn answers() {
        assert!(parse_answer("\n", true));
        assert!(!parse_answer("\n", false));
        assert!(parse_answer("y\n", false));
        assert!(parse_answer(" YES \n", false));
        assert!(!parse_answer("n\n", true));
        assert!(!parse_answer("maybe\n", true));
    }

    #[test]
    fn the_end_of_input_is_no_answer() {
        let mut eof = std::io::Cursor::new(b"");
        assert_eq!(read_answer(&mut eof, true), None, "Ctrl-D is not yes");
        let mut empty = std::io::Cursor::new(b"\n");
        assert_eq!(read_answer(&mut empty, true), Some(true));
        let mut no = std::io::Cursor::new(b"n\n");
        assert_eq!(read_answer(&mut no, true), Some(false));
        let mut unterminated = std::io::Cursor::new(b"y");
        assert_eq!(read_answer(&mut unterminated, false), Some(true));
    }

    #[test]
    fn progress_tracks_bytes_without_a_terminal() {
        use uncork_core::download::Progress as _;
        let mut progress = TerminalProgress::default();
        progress.start("SteamSetup.exe", Some(2_000));
        progress.advance(500);
        progress.advance(1_500);
        assert_eq!(progress.done, 2_000);
        progress.finish();
        progress.start("again", None);
        assert_eq!(progress.done, 0, "start resets the counters");
    }
}
