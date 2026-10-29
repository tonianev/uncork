//! `uncork status`: what runs in each bottle, read without changing
//! anything.
//!
//! Everything here is read-only. Whether a wineserver runs comes from
//! `wineserver -k0` ([`uncork_core::steam::server_running`]), which starts
//! nothing; Steam's `ActiveProcess` values are read with `wine reg query`
//! only while a wineserver runs, because that query would otherwise start
//! one ([`uncork_core::steam::client_snapshot`]). A pid left by a client
//! that crashed is reported as not running and left alone (the next `play`
//! or `steam start` clears it). The main display is read with
//! `system_profiler`, as every launch reads it.

use std::path::Path;
use std::process::ExitCode;

use serde::Serialize;
use uncork_core::bottle::{self, Bottle};
use uncork_core::display::{self, Display};
use uncork_core::graphics::Backend;
use uncork_core::steam::{self, ClientSnapshot};
use uncork_steam::SteamInstall;

use super::{Ctx, bottle_wine, key_values, yes_no};
use crate::cli::StatusArgs;
use crate::output;

/// `--json` output: the main display now and the bottles asked about.
#[derive(Debug, Serialize)]
struct StatusView<'a> {
    /// The main display now; `None` when it is unknown.
    display: Option<DisplayView>,
    /// Every bottle, or the one named with `--bottle`.
    bottles: Vec<BottleStatus<'a>>,
}

/// The main display in `--json` output.
#[derive(Debug, Serialize)]
struct DisplayView {
    /// [`Display::signature`]: the name and "looks like" size, what a
    /// bottle's session display is compared with.
    signature: String,
    /// [`Display::describe`]: the signature with the refresh rate.
    description: String,
    /// The refresh rate in Hz, when reported.
    refresh_hz: Option<f64>,
}

/// One bottle in `--json` output.
#[derive(Debug, Serialize)]
struct BottleStatus<'a> {
    name: &'a str,
    /// The configured default bottle.
    default: bool,
    path: &'a Path,
    wine: WineView<'a>,
    /// A wineserver runs for the bottle; `None` when that could not be
    /// asked (the bottle's Wine is not installed, or `wineserver` failed).
    running: Option<bool>,
    steam: SteamStatus,
    /// The backend whose DLLs are in the prefix; `None` before the first
    /// DXMT or DXVK launch.
    backend: Option<BackendView<'a>>,
    /// The main display when the bottle's Wine session started
    /// (`uncork-state.toml`); `None` when unknown.
    session_display: Option<&'a str>,
    /// The session display differs from the main display now, so a
    /// running bottle is restarted before the next `play` or `run`;
    /// `None` when either display is unknown.
    display_changed: Option<bool>,
}

/// The bottle's Wine in `--json` output.
#[derive(Debug, Serialize)]
struct WineView<'a> {
    version: &'a str,
    installed: bool,
}

/// The Steam client in `--json` output. `running`, `pid`, `signed_in` and
/// `user` are `None` when they could not be read; `pid` and `user` are
/// given only while the client runs.
#[derive(Debug, Serialize)]
struct SteamStatus {
    /// `Steam.exe` is in the bottle.
    installed: bool,
    /// The client runs: a wineserver runs and `ActiveProcess` `pid` is
    /// non-zero.
    running: Option<bool>,
    /// The `ActiveProcess` pid (a Windows process id, as the client records
    /// it).
    pid: Option<u32>,
    /// An account is signed in (`ActiveUser` non-zero).
    signed_in: Option<bool>,
    /// The signed-in account's id (`ActiveUser`); 0 while nobody is.
    user: Option<u32>,
}

/// The active backend in `--json` output.
#[derive(Debug, Serialize)]
struct BackendView<'a> {
    name: Backend,
    version: &'a str,
}

/// `uncork status [--bottle B]`.
pub(super) fn run(ctx: &Ctx, args: &StatusArgs) -> anyhow::Result<ExitCode> {
    let bottles = match &args.bottle {
        Some(name) => vec![Bottle::open_named(&ctx.layout, name)?],
        None => bottle::list(&ctx.layout)?,
    };
    let default = ctx.config()?.default_bottle;
    let display = display::probe_main();
    let view = StatusView {
        display: display.as_ref().map(|display| DisplayView {
            signature: display.signature(),
            description: display.describe(),
            refresh_hz: display.refresh_hz,
        }),
        bottles: bottles
            .iter()
            .map(|bottle| bottle_status(ctx, bottle, &default, display.as_ref()))
            .collect(),
    };
    if ctx.json {
        output::print_json(&view)?;
        return Ok(ExitCode::SUCCESS);
    }
    print!("{}", render(&view));
    Ok(ExitCode::SUCCESS)
}

/// What is known about `bottle` without changing it.
fn bottle_status<'a>(
    ctx: &Ctx,
    bottle: &'a Bottle,
    default: &str,
    display: Option<&Display>,
) -> BottleStatus<'a> {
    let name = &bottle.config.name;
    let steam_installed = SteamInstall::find(bottle.prefix()).is_some();
    let wine = match bottle_wine(&ctx.layout, bottle) {
        Ok(wine) => Some(wine),
        Err(err) => {
            tracing::info!("{err:#}");
            None
        }
    };
    // Steam's registry values are read only while a wineserver runs, and
    // only when Steam is installed; `wineserver -k0` alone otherwise.
    let (running, snapshot) = match &wine {
        None => (None, None),
        Some(wine) if steam_installed => match steam::client_snapshot(bottle, wine) {
            Ok(snapshot) => (Some(snapshot.server), Some(snapshot)),
            Err(err) => {
                eprintln!("warning: cannot tell what runs in bottle {name}: {err}");
                (None, None)
            }
        },
        Some(wine) => match steam::server_running(bottle, wine) {
            Ok(server) => (Some(server), None),
            Err(err) => {
                eprintln!("warning: cannot tell whether bottle {name} runs: {err}");
                (None, None)
            }
        },
    };
    let steam = steam_status(steam_installed, snapshot.as_ref());
    let session_display = bottle
        .state
        .session_display
        .as_deref()
        .map(display::without_refresh);
    let display_changed = match (session_display, display) {
        (Some(session), Some(display)) => Some(session != display.signature()),
        _ => None,
    };
    BottleStatus {
        name,
        default: name == default,
        path: &bottle.path,
        wine: WineView {
            version: &bottle.config.wine,
            installed: wine.is_some(),
        },
        running,
        steam,
        backend: bottle.state.active.as_ref().map(|active| BackendView {
            name: active.backend,
            version: &active.version,
        }),
        session_display,
        display_changed,
    }
}

/// [`SteamStatus`] from what was read: nothing when Steam is not installed,
/// unknown when it is but nothing could be read.
fn steam_status(installed: bool, snapshot: Option<&ClientSnapshot>) -> SteamStatus {
    match snapshot {
        Some(snapshot) => {
            let running = snapshot.running();
            SteamStatus {
                installed,
                running: Some(running),
                pid: snapshot.pid.filter(|_| running),
                signed_in: Some(snapshot.signed_in()),
                user: snapshot.user.filter(|_| running),
            }
        }
        None if installed => SteamStatus {
            installed,
            running: None,
            pid: None,
            signed_in: None,
            user: None,
        },
        None => SteamStatus {
            installed,
            running: Some(false),
            pid: None,
            signed_in: Some(false),
            user: None,
        },
    }
}

/// The text report: the main display, then one block per bottle.
fn render(view: &StatusView<'_>) -> String {
    let mut out = key_values(&[(
        "Main display",
        view.display.as_ref().map_or_else(
            || "unknown".to_owned(),
            |display| display.description.clone(),
        ),
    )]);
    if view.bottles.is_empty() {
        out.push_str("\nNo bottles yet. Run `uncork setup`, or `uncork bottle create <name>`.\n");
        return out;
    }
    for bottle in &view.bottles {
        out.push('\n');
        out.push_str(&render_bottle(bottle));
    }
    out
}

/// One bottle's block.
fn render_bottle(bottle: &BottleStatus<'_>) -> String {
    let unknown = |value: Option<bool>| value.map_or("unknown", yes_no).to_owned();
    let steam = &bottle.steam;
    let steam_line = if !steam.installed {
        "not installed".to_owned()
    } else {
        match (steam.running, steam.signed_in) {
            (Some(true), signed_in) => {
                let pid = steam
                    .pid
                    .map_or_else(String::new, |pid| format!(" (pid {pid})"));
                let account = match (signed_in, steam.user) {
                    (Some(true), Some(user)) => format!("signed in (account {user})"),
                    (Some(true), None) => "signed in".to_owned(),
                    (Some(false), _) => "nobody signed in".to_owned(),
                    (None, _) => "sign-in unknown".to_owned(),
                };
                format!("running{pid}, {account}")
            }
            (Some(false), _) => "installed, not running".to_owned(),
            (None, _) => "installed, running: unknown".to_owned(),
        }
    };
    let session = match (bottle.session_display, bottle.display_changed) {
        (None, _) => "unknown".to_owned(),
        (Some(display), Some(true)) => format!(
            "{display} (differs from the main display now; a running bottle is restarted before the next launch)"
        ),
        (Some(display), Some(false)) => format!("{display} (the main display now)"),
        (Some(display), None) => display.to_owned(),
    };
    key_values(&[
        (
            "Bottle",
            if bottle.default {
                format!("{} (default)", bottle.name)
            } else {
                bottle.name.to_owned()
            },
        ),
        (
            "Wine",
            if bottle.wine.installed {
                bottle.wine.version.to_owned()
            } else {
                format!("{} (not installed)", bottle.wine.version)
            },
        ),
        ("Running", unknown(bottle.running)),
        ("Steam", steam_line),
        (
            "Backend",
            bottle.backend.as_ref().map_or_else(
                || "none (no backend DLLs in the prefix yet)".to_owned(),
                |backend| format!("{} {} (DLLs in the prefix)", backend.name, backend.version),
            ),
        ),
        ("Session display", session),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bottle<'a>(steam: SteamStatus, running: Option<bool>) -> BottleStatus<'a> {
        BottleStatus {
            name: "steam",
            default: true,
            path: Path::new("/u/bottles/steam"),
            wine: WineView {
                version: "winecx-gptk-4.7.3",
                installed: true,
            },
            running,
            steam,
            backend: Some(BackendView {
                name: Backend::Dxmt,
                version: "0.80",
            }),
            session_display: Some("Color LCD 1728x1117"),
            display_changed: Some(false),
        }
    }

    #[test]
    fn steam_status_from_a_snapshot() {
        let running = ClientSnapshot {
            server: true,
            pid: Some(0x274),
            user: Some(7),
        };
        let status = steam_status(true, Some(&running));
        assert_eq!(status.running, Some(true));
        assert_eq!(status.pid, Some(0x274));
        assert_eq!(status.signed_in, Some(true));
        assert_eq!(status.user, Some(7));

        let exited = ClientSnapshot {
            server: true,
            pid: Some(0),
            user: Some(7),
        };
        let status = steam_status(true, Some(&exited));
        assert_eq!(status.running, Some(false));
        assert_eq!(status.pid, None, "a pid is shown only while running");
        assert_eq!(status.user, None);

        let status = steam_status(true, None);
        assert_eq!((status.running, status.signed_in), (None, None));
        let status = steam_status(false, None);
        assert_eq!(
            (status.running, status.signed_in),
            (Some(false), Some(false))
        );
    }

    #[test]
    fn the_text_report_says_what_runs() {
        let running = steam_status(
            true,
            Some(&ClientSnapshot {
                server: true,
                pid: Some(628),
                user: Some(446_146_782),
            }),
        );
        let view = StatusView {
            display: Some(DisplayView {
                signature: "Color LCD 1728x1117".to_owned(),
                description: "Color LCD 1728x1117 @120Hz".to_owned(),
                refresh_hz: Some(120.0),
            }),
            bottles: vec![bottle(running, Some(true))],
        };
        let text = render(&view);
        assert!(
            text.starts_with(
                "Main display  Color LCD 1728x1117 @120Hz\n\nBottle           steam (default)\n"
            ),
            "{text}"
        );
        assert!(text.contains("\nRunning          yes\n"), "{text}");
        assert!(
            text.contains("\nSteam            running (pid 628), signed in (account 446146782)\n"),
            "{text}"
        );
        assert!(
            text.contains("\nBackend          dxmt 0.80 (DLLs in the prefix)\n"),
            "{text}"
        );
        assert!(
            text.ends_with("\nSession display  Color LCD 1728x1117 (the main display now)\n"),
            "{text}"
        );

        let mut stopped = bottle(steam_status(true, None), None);
        stopped.wine.installed = false;
        stopped.backend = None;
        stopped.display_changed = Some(true);
        let view = StatusView {
            display: None,
            bottles: vec![stopped],
        };
        let text = render(&view);
        assert!(text.starts_with("Main display  unknown\n"), "{text}");
        assert!(
            text.contains("\nWine             winecx-gptk-4.7.3 (not installed)\n"),
            "{text}"
        );
        assert!(text.contains("\nRunning          unknown\n"), "{text}");
        assert!(
            text.contains("\nSteam            installed, running: unknown\n"),
            "{text}"
        );
        assert!(
            text.contains("\nBackend          none (no backend DLLs in the prefix yet)\n"),
            "{text}"
        );
        assert!(
            text.contains("Color LCD 1728x1117 (differs from the main display now;"),
            "{text}"
        );

        let view = StatusView {
            display: None,
            bottles: Vec::new(),
        };
        assert!(render(&view).contains("No bottles yet."));
    }
}
