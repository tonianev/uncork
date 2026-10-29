//! `docs/CLI.md` is every command's `--help`, verbatim. This test keeps it
//! that way: each `## uncork ...` heading names a command, and the first
//! fenced `text` block after it must equal that command's `--help` at
//! `COLUMNS=100`. Every command listed under `Commands:` in a section's
//! help must have a section of its own, so a new subcommand cannot ship
//! undocumented.
//!
//! When the help text changes, rewrite the document instead of editing it
//! by hand: `just cli-doc` runs this test with `UNCORK_REGEN_CLI_DOC=1`,
//! which replaces every block (and the version in the "Generated from"
//! line) and passes.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// The width the document is generated at; `Cli` caps help at 100 too.
const COLUMNS: &str = "100";

/// Where the document lives, relative to this crate.
fn doc_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/CLI.md")
}

/// One `## uncork ...` section: the command's words after `uncork`, and
/// the byte range of its fenced block's content.
struct Section {
    words: Vec<String>,
    block: std::ops::Range<usize>,
}

/// Every section in `text`, in order. A heading without a fenced block
/// after it (before the next heading) is an error in the document.
fn sections(text: &str) -> Vec<Section> {
    let mut found = Vec::new();
    let mut pos = 0;
    while let Some(start) = text[pos..].find("\n## uncork") {
        let heading_start = pos + start + 1;
        let heading_end = text[heading_start..]
            .find('\n')
            .map_or(text.len(), |i| heading_start + i);
        let heading = &text[heading_start..heading_end];
        let words: Vec<String> = heading
            .trim_start_matches("## ")
            .split_whitespace()
            .skip(1)
            .map(str::to_owned)
            .collect();
        let next_heading = text[heading_end..]
            .find("\n## ")
            .map_or(text.len(), |i| heading_end + i);
        let fence = "```text\n";
        let open = text[heading_end..next_heading]
            .find(fence)
            .unwrap_or_else(|| panic!("{heading}: no ```text block before the next heading"));
        let content_start = heading_end + open + fence.len();
        let close = text[content_start..next_heading]
            .find("\n```")
            .unwrap_or_else(|| panic!("{heading}: ```text block is not closed"));
        let content_end = content_start + close + 1;
        found.push(Section {
            words,
            block: content_start..content_end,
        });
        pos = content_end;
    }
    found
}

/// `uncork <words> --help` at the document's width, with a throwaway data
/// root so that help can never touch the real one. Trailing whitespace is
/// stripped from each line: clap pads some blank lines, editors strip
/// them, and neither should fail the test.
fn help(words: &[String]) -> String {
    let home = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_uncork"))
        .args(words)
        .arg("--help")
        .env_clear()
        .env("COLUMNS", COLUMNS)
        .env("HOME", home.path())
        .env("UNCORK_HOME", home.path().join("uncork"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "uncork {} --help failed: {}",
        words.join(" "),
        String::from_utf8_lossy(&output.stderr)
    );
    let raw = String::from_utf8(output.stdout).unwrap();
    raw.lines()
        .map(str::trim_end)
        .fold(String::new(), |mut s, line| {
            s.push_str(line);
            s.push('\n');
            s
        })
}

/// The names under `Commands:` in a help text (`help` left out): each is
/// a subcommand that needs its own section.
fn listed_commands(help: &str) -> Vec<String> {
    let Some(start) = help.find("\nCommands:\n") else {
        return Vec::new();
    };
    help[start + "\nCommands:\n".len()..]
        .lines()
        .take_while(|line| !line.is_empty())
        // A wrapped description continues with deeper indentation.
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .map(str::to_owned)
        .collect()
}

#[test]
fn cli_md_is_every_commands_help_verbatim() {
    let path = doc_path();
    let text = fs::read_to_string(&path).unwrap();
    let regenerate = std::env::var_os("UNCORK_REGEN_CLI_DOC").is_some();
    let found = sections(&text);
    assert!(
        found.iter().any(|s| s.words.is_empty()),
        "docs/CLI.md has no `## uncork` section"
    );

    let mut rebuilt = String::with_capacity(text.len());
    let mut last = 0;
    let mut stale = Vec::new();
    let mut missing = Vec::new();
    let mut have: Vec<&[String]> = found.iter().map(|s| s.words.as_slice()).collect();
    have.sort();
    for section in &found {
        let fresh = help(&section.words);
        for name in listed_commands(&fresh) {
            let mut words = section.words.clone();
            words.push(name);
            if have.binary_search(&words.as_slice()).is_err() {
                missing.push(format!("uncork {}", words.join(" ")));
            }
        }
        if text[section.block.clone()] != fresh {
            stale.push(format!("uncork {}", section.words.join(" ")));
        }
        rebuilt.push_str(&text[last..section.block.start]);
        rebuilt.push_str(&fresh);
        last = section.block.end;
    }
    rebuilt.push_str(&text[last..]);

    let generated = format!("Generated from `uncork {}`", env!("CARGO_PKG_VERSION"));
    let version_stale = !text.contains(&generated);
    if version_stale {
        let start = text.find("Generated from `uncork ").unwrap();
        let end = start + text[start..].find('`').unwrap() + 1;
        let end = end + text[end..].find('`').unwrap() + 1;
        rebuilt = rebuilt.replacen(&text[start..end], &generated, 1);
    }

    assert!(
        missing.is_empty(),
        "docs/CLI.md lacks a section for: {}\nAdd a `## <command>` heading with a ```text block (any content), then run `just cli-doc`.",
        missing.join(", ")
    );
    if regenerate {
        if rebuilt != text {
            fs::write(&path, rebuilt).unwrap();
            eprintln!("rewrote {}: {}", path.display(), stale.join(", "));
        }
        return;
    }
    assert!(
        stale.is_empty() && !version_stale,
        "docs/CLI.md is out of date for: {}{}\nRun `just cli-doc` to regenerate it from the binary.",
        stale.join(", "),
        if version_stale {
            " (and the version in the \"Generated from\" line)"
        } else {
            ""
        }
    );
}

#[test]
fn listed_commands_reads_the_commands_block() {
    let text = "Usage: uncork bottle <COMMAND>\n\nCommands:\n  list    List bottles\n  create  Create a bottle\n  set     Change a bottle setting, e.g. `graphics.backend=dxmt`, `performance.retina=true`,\n          `performance.max_fps=60`, `env.DXMT_LOG_LEVEL=info`\n  help    Print this message or the help of the given subcommand(s)\n\nOptions:\n";
    assert_eq!(listed_commands(text), ["list", "create", "set"]);
    assert!(listed_commands("Usage: uncork doctor\n\nOptions:\n").is_empty());
}

#[test]
fn sections_find_each_heading_and_its_block() {
    let text = "# Reference\n\n## uncork\n\nIntro.\n\n```text\nroot help\n```\n\n## uncork bottle set\n\n```text\nset help\n```\n";
    let found = sections(text);
    let words: Vec<_> = found.iter().map(|s| s.words.join(" ")).collect();
    assert_eq!(words, ["", "bottle set"]);
    assert_eq!(&text[found[0].block.clone()], "root help\n");
    assert_eq!(&text[found[1].block.clone()], "set help\n");
}
