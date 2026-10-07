//! macOS Game Mode for Wine games (experimental).
//!
//! macOS turns Game Mode on for a frontmost full-screen app whose bundle
//! declares `LSApplicationCategoryType = public.app-category.games`, and only
//! when the app was launched through LaunchServices (not for binaries
//! spawned from a terminal; macOS 26 release notes). Wine's loader is a bare
//! executable with no bundle, so a Wine game never qualifies on its own.
//!
//! Uncork generates a small bundle per game under `$UNCORK_HOME/apps/`:
//!
//! ```text
//! <Game>.app/Contents/Info.plist        games category, LSSupportsGameMode
//! <Game>.app/Contents/MacOS/launch      shell-free launcher: a copy of the
//!                                       `uncork` binary, run as
//!                                       `launch exec <plan.json>`
//! <Game>.app/Contents/Resources/plan.json  the LaunchPlan command to exec
//! ```
//!
//! and opens it with `/usr/bin/open -n -W <Game>.app`. The launcher `exec`s
//! the Wine loader in place (same process), so Game Mode is keyed to the
//! bundle that LaunchServices started. Whether macOS keeps Game Mode after
//! the `exec` is an open question tracked in `docs/PERFORMANCE.md`; the
//! feature is off by default (`performance.game_mode = false`) until it is
//! measured.

use std::path::{Path, PathBuf};

use crate::launch::LaunchPlan;
use crate::paths::Layout;

/// The `Info.plist` for a wrapper bundle. `name` is the display name,
/// `bundle_id` is `dev.uncork.game.<id>`.
#[must_use]
pub fn info_plist(name: &str, bundle_id: &str) -> String {
    let _ = (name, bundle_id);
    todo!()
}

/// Create or refresh `apps/<id>.app` for `plan`: write `Info.plist`, copy
/// `launcher` (normally `std::env::current_exe()`) to `Contents/MacOS/launch`
/// when it differs, write `plan.json`, and ad-hoc sign the bundle with
/// `codesign --force --sign - <bundle>`.
///
/// # Errors
/// I/O or command errors.
pub fn prepare_bundle(
    layout: &Layout,
    id: &str,
    name: &str,
    plan: &LaunchPlan,
    launcher: &Path,
) -> crate::Result<PathBuf> {
    let _ = (layout, id, name, plan, launcher);
    todo!()
}

/// The command that opens `bundle` through LaunchServices and waits for it:
/// `/usr/bin/open -n -W <bundle>`.
#[must_use]
pub fn open_command(bundle: &Path) -> crate::process::CommandSpec {
    crate::process::CommandSpec::new("/usr/bin/open")
        .arg("-n")
        .arg("-W")
        .arg(bundle)
}
