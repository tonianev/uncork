//! `uncork __manpage [DIR]`: the man pages, section 1, made from the same
//! command definition as `--help`: `uncork.1`, then `uncork-<command>.1`
//! and `uncork-<command>-<subcommand>.1` for every command `--help` lists.
//! The release build packages them as `man/man1/` in the archive.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Context as _;
use clap::CommandFactory as _;
use clap_mangen::Man;

use crate::cli::{Cli, ManpageArgs};

/// Write the pages into `args.dir` and say how many on stderr.
pub(super) fn run(args: &ManpageArgs) -> anyhow::Result<ExitCode> {
    let pages = write_pages(&args.dir)?;
    eprintln!("wrote {} man pages to {}", pages.len(), args.dir.display());
    Ok(ExitCode::SUCCESS)
}

/// Write one page per visible command into `dir` (created when missing)
/// and return their paths, `uncork.1` first.
fn write_pages(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    // `help` is clap's own subcommand; a page for it would say nothing.
    let mut cmd = Cli::command().disable_help_subcommand(true);
    cmd.build();
    let mut pages = Vec::new();
    write_tree(&cmd, dir, &mut pages)?;
    Ok(pages)
}

/// `cmd`'s page, then the pages of its visible subcommands, recursively.
fn write_tree(cmd: &clap::Command, dir: &Path, pages: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    let page = Man::new(cmd.clone())
        .generate_to(dir)
        .with_context(|| format!("cannot write a man page into {}", dir.display()))?;
    pages.push(page);
    for sub in cmd.get_subcommands().filter(|sub| !sub.is_hide_set()) {
        write_tree(sub, dir, pages)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(pages: &[PathBuf]) -> Vec<&str> {
        pages
            .iter()
            .map(|page| page.file_name().unwrap().to_str().unwrap())
            .collect()
    }

    #[test]
    fn a_page_per_visible_command_goes_into_the_directory() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("man").join("man1");
        let pages = write_pages(&out).unwrap();
        let names = names(&pages);
        assert_eq!(names[0], "uncork.1");
        for name in [
            "uncork-play.1",
            "uncork-doctor.1",
            "uncork-bottle-create.1",
            "uncork-completions.1",
        ] {
            assert!(names.contains(&name), "{name}: {names:?}");
            assert!(out.join(name).is_file(), "{name}");
        }
        for hidden in ["uncork-__exec.1", "uncork-__manpage.1", "uncork-help.1"] {
            assert!(!names.contains(&hidden), "{hidden}: {names:?}");
        }

        // roff writes a hyphen as `\-`: `--json` is `\-\-json` in the source.
        let root = std::fs::read_to_string(out.join("uncork.1")).unwrap();
        assert!(root.contains(".TH uncork 1"), "{root}");
        assert!(root.contains(r"\-\-json"), "{root}");
        assert!(root.contains("doctor"), "{root}");
        let play = std::fs::read_to_string(out.join("uncork-play.1")).unwrap();
        assert!(play.contains(r"\-\-dry\-run"), "{play}");
    }
}
