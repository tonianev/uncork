//! `uncork logs`: read a bottle's launch logs.
//!
//! Every launch writes `logs/<bottle>-<program>-<unix secs>.log`
//! ([`uncork_core::launch::launch_logs`]); the Steam client's logs have
//! program `steam`. The command shows the newest, or the newest of one
//! game, and never writes anything.

use std::fs;
use std::path::Path;
use std::process::ExitCode;

use anyhow::{Context as _, bail};
use serde::Serialize;
use uncork_core::launch::{self, LaunchLog};
use uncork_core::profile::Lookup;

use super::Ctx;
use crate::cli::LogsArgs;
use crate::output::{self, human_bytes};

/// `--json` output: one log and its last lines. `null` when there is no
/// log.
#[derive(Debug, Serialize)]
struct LogView<'a> {
    path: &'a Path,
    /// The program's file stem as the log name records it (`riseofnations`,
    /// `steam`).
    program: &'a str,
    bottle: &'a str,
    /// When the launch was planned, Unix seconds (from the name).
    started: u64,
    /// When the file was last written, Unix seconds.
    modified: Option<u64>,
    /// Size in bytes.
    size: u64,
    /// The last `-n` lines (every line with `-n 0`), without line endings.
    lines: Vec<String>,
}

/// `uncork logs [--bottle B] [--game G] [-n LINES] [--path]`.
pub(super) fn run(ctx: &Ctx, args: &LogsArgs) -> anyhow::Result<ExitCode> {
    let bottle = ctx.open_bottle(args.bottle.as_deref())?;
    let name = &bottle.config.name;
    let mut logs = launch::launch_logs(&bottle)?;
    let program = match &args.game {
        Some(game) => Some(program_for(ctx, game)?),
        None => None,
    };
    if let Some(program) = &program {
        logs.retain(|log| log.program.eq_ignore_ascii_case(program));
    }
    let Some(log) = logs.first() else {
        let which = program.map_or_else(String::new, |program| format!(" of {program}"));
        let message = format!(
            "No launch logs{which} in bottle {name} yet; each `uncork play` and `uncork run` writes one to {}.",
            launch::logs_dir_for(&bottle).display()
        );
        if ctx.json {
            output::print_json(&serde_json::Value::Null)?;
        } else if args.path {
            eprintln!("{message}");
        } else {
            println!("{message}");
        }
        return Ok(ExitCode::SUCCESS);
    };
    if args.path && !ctx.json {
        println!("{}", log.path.display());
        return Ok(ExitCode::SUCCESS);
    }
    let metadata =
        fs::metadata(&log.path).with_context(|| format!("cannot read {}", log.path.display()))?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|since| since.as_secs());
    let lines = if args.path {
        Vec::new()
    } else {
        let bytes =
            fs::read(&log.path).with_context(|| format!("cannot read {}", log.path.display()))?;
        tail(&String::from_utf8_lossy(&bytes), args.lines)
    };
    let view = LogView {
        path: &log.path,
        program: &log.program,
        bottle: name,
        started: log.started_unix,
        modified,
        size: metadata.len(),
        lines,
    };
    if ctx.json {
        output::print_json(&view)?;
        return Ok(ExitCode::SUCCESS);
    }
    eprintln!("{}", describe(log, &view));
    for line in &view.lines {
        println!("{line}");
    }
    Ok(ExitCode::SUCCESS)
}

/// The program name `--game` means: the file stem of the profile's
/// executable when `game` names a profile (as `uncork play` resolves it),
/// else `game` itself without a directory or extension; in the form log
/// names use ([`launch::sanitize_file_part`]).
fn program_for(ctx: &Ctx, game: &str) -> anyhow::Result<String> {
    let profiles = super::profile::load_profiles(ctx)?;
    let exe = match uncork_core::profile::lookup(&profiles, game) {
        Lookup::Found(profile) => profile.exe.path.clone(),
        Lookup::Ambiguous(candidates) => bail!(
            "{game:?} matches several game profiles: {}; name one by its id",
            super::launch::profile_ids(&candidates)
        ),
        Lookup::NotFound => game.to_owned(),
    };
    Ok(launch::sanitize_file_part(file_stem(&exe)))
}

/// `name` without a directory (`/` or `\`) or extension.
fn file_stem(name: &str) -> &str {
    let file = name.rsplit(['\\', '/']).next().unwrap_or(name);
    match file.rsplit_once('.') {
        Some((stem, extension)) if !stem.is_empty() && !extension.is_empty() => stem,
        _ => file,
    }
}

/// The last `count` lines of `text` (every line when `count` is 0), with
/// line endings (`\n`, `\r\n`) removed.
fn tail(text: &str, count: usize) -> Vec<String> {
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect();
    let skip = if count == 0 {
        0
    } else {
        lines.len().saturating_sub(count)
    };
    lines[skip..]
        .iter()
        .map(|line| (*line).to_owned())
        .collect()
}

/// The header line for text output: the path, program, bottle, size and
/// when the file was last written.
fn describe(log: &LaunchLog, view: &LogView<'_>) -> String {
    let written = view.modified.map_or_else(String::new, |modified| {
        format!(
            ", written {}",
            ago(uncork_core::unix_now().saturating_sub(modified))
        )
    });
    format!(
        "{} ({} in bottle {}, {}{written})",
        log.path.display(),
        view.program,
        view.bottle,
        human_bytes(view.size)
    )
}

/// `12 s ago`, `5 min ago`, `3 h ago`, `2 d ago`.
fn ago(seconds: u64) -> String {
    match seconds {
        0..60 => format!("{seconds} s ago"),
        60..3_600 => format!("{} min ago", seconds / 60),
        3_600..86_400 => format!("{} h ago", seconds / 3_600),
        _ => format!("{} d ago", seconds / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tails_keep_the_last_lines_without_line_endings() {
        assert_eq!(tail("a\r\nb\nc\n", 2), ["b", "c"]);
        assert_eq!(tail("a\r\nb\nc", 0), ["a", "b", "c"]);
        assert_eq!(tail("a\nb\n", 5), ["a", "b"]);
        assert!(tail("", 3).is_empty());
    }

    #[test]
    fn file_stems() {
        assert_eq!(file_stem("riseofnations.exe"), "riseofnations");
        assert_eq!(file_stem(r"bin\Game.EXE"), "Game");
        assert_eq!(file_stem("C:/Games/x/game"), "game");
        assert_eq!(file_stem("steam"), "steam");
        assert_eq!(file_stem(".hidden"), ".hidden");
    }

    #[test]
    fn ages() {
        assert_eq!(ago(0), "0 s ago");
        assert_eq!(ago(59), "59 s ago");
        assert_eq!(ago(60), "1 min ago");
        assert_eq!(ago(3_599), "59 min ago");
        assert_eq!(ago(7_200), "2 h ago");
        assert_eq!(ago(200_000), "2 d ago");
    }
}
