//! `uncork winetricks`: the user's own winetricks, pointed at a bottle.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context as _, anyhow};
use uncork_core::process::CommandSpec;

use super::{Ctx, bottle_wine};
use crate::cli::WinetricksArgs;

/// Where Homebrew installs winetricks, searched after `PATH`.
const FALLBACK_DIRS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// The first executable `winetricks` in `path` (a `PATH` value), then in
/// [`FALLBACK_DIRS`].
fn find_winetricks(path: Option<&OsString>) -> Option<PathBuf> {
    path.map(|path| std::env::split_paths(path).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|dir| dir.is_absolute())
        .chain(FALLBACK_DIRS.iter().map(PathBuf::from))
        .map(|dir| dir.join("winetricks"))
        .find(|candidate| uncork_core::process::is_executable(candidate))
}

/// Run winetricks with the bottle's Wine environment and the user's `PATH`
/// (winetricks needs `cabextract` and friends from it), stdio inherited.
pub(super) fn run(ctx: &Ctx, args: &WinetricksArgs) -> anyhow::Result<ExitCode> {
    let user_path = std::env::var_os("PATH");
    let winetricks = find_winetricks(user_path.as_ref()).ok_or_else(|| {
        anyhow!(
            "winetricks is not installed (looked in PATH, {}); install it with: brew install winetricks",
            FALLBACK_DIRS.join(", ")
        )
    })?;
    let bottle = ctx.open_bottle(args.bottle.as_deref())?;
    let wine = bottle_wine(&ctx.layout, &bottle)?;
    let mut spec = CommandSpec::new(&winetricks).args(&args.verbs);
    spec.env_clear = true;
    spec.env = uncork_core::launch::base_env(&bottle, &wine, None);
    spec.env.extend(bottle.config.env.clone());
    spec.env
        .insert("WINEPREFIX".to_owned(), path_text(bottle.prefix()));
    spec.env
        .insert("WINE".to_owned(), path_text(&wine.wine_bin()));
    spec.env
        .insert("WINESERVER".to_owned(), path_text(&wine.wineserver_bin()));
    if let Some(path) = user_path.as_ref().and_then(|path| path.to_str()) {
        spec.env.insert("PATH".to_owned(), path.to_owned());
    }
    eprintln!(
        "Running {} {} in bottle {}",
        winetricks.display(),
        args.verbs.join(" "),
        bottle.config.name
    );
    uncork_core::process::run(&spec).context("winetricks failed")?;
    Ok(ExitCode::SUCCESS)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt as _;

    use super::*;

    #[test]
    fn finds_the_first_executable_on_path() {
        let dir = tempfile::tempdir().unwrap();
        let (first, second) = (dir.path().join("a"), dir.path().join("b"));
        for (directory, mode) in [(&first, 0o644), (&second, 0o755)] {
            std::fs::create_dir(directory).unwrap();
            let file = directory.join("winetricks");
            std::fs::write(&file, "#!/bin/sh\n").unwrap();
            std::fs::set_permissions(&file, std::fs::Permissions::from_mode(mode)).unwrap();
        }
        let path = std::env::join_paths([&first, &second]).unwrap();
        assert_eq!(
            find_winetricks(Some(&path)),
            Some(second.join("winetricks")),
            "not executable in the first directory"
        );
    }

    #[test]
    fn relative_path_entries_are_ignored() {
        let path = OsString::from(".:bin");
        let found = find_winetricks(Some(&path));
        assert!(
            found
                .as_deref()
                .is_none_or(|found| FALLBACK_DIRS.iter().any(|dir| found.starts_with(dir))),
            "{found:?}"
        );
    }
}
