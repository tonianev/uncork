//! Facts about the Windows Steam client and argument lists for it.
//!
//! Sources for every constant are in `docs/STEAM.md`. Flags were checked
//! against the strings of client build 1773426488 (2026); a flag being
//! present in the binary does not prove the client honors it, so the
//! defaults are deliberately few.

/// Steam's directory relative to `drive_c`. The 64-bit client (the only one
/// since 2026-01-01) still installs under `Program Files (x86)`.
pub const STEAM_DIR: &str = "Program Files (x86)/Steam";

/// Valve's installer, primary URL first. The akamai host is the one Valve's
/// own download page links.
pub const INSTALLER_URLS: &[&str] = &[
    "https://cdn.akamai.steamstatic.com/client/installer/SteamSetup.exe",
    "https://cdn.fastly.steamstatic.com/client/installer/SteamSetup.exe",
];

/// SHA-256 of the installer last time it was checked (2026-10, matches
/// winetricks). Valve replaces the file in place, so a mismatch is a warning.
pub const INSTALLER_SHA256: &str = "7d3654531c32d941b8cae81c4137fc542172bfa9635f169cb392f245a0a12bcb";

/// Silent-install switch of the NSIS installer.
pub const INSTALLER_SILENT: &str = "/S";

/// Flags Uncork passes every time it starts the client.
///
/// - `-cef-disable-gpu`, `-cef-disable-gpu-compositing`: Steam's web UI
///   (CEF) renders in software. Its GPU process presents across processes,
///   which DXMT and D3DMetal cannot do and which costs nothing to avoid.
/// - `-nofriendsui`: skip the friends web view (less CEF work while gaming).
pub const DEFAULT_CLIENT_ARGS: &[&str] = &["-cef-disable-gpu", "-cef-disable-gpu-compositing", "-nofriendsui"];

/// Flags known to be dead in the 2026 client or harmful; Uncork strips them
/// from user-supplied argument lists and says so: `-noreactlogin`,
/// `-no-browser`, `-cef-force-32bit`, `-cef-in-process-gpu`,
/// `-cef-single-process`, `-nochatui`, `-nooverlay`, `-nobootstrapupdate`
/// (dead or misspelt), `-noverifyfiles` (harmful).
pub const STRIPPED_CLIENT_ARGS: &[&str] = &[
    "-noreactlogin",
    "-no-browser",
    "-cef-force-32bit",
    "-cef-in-process-gpu",
    "-cef-single-process",
    "-nochatui",
    "-nooverlay",
    "-nobootstrapupdate",
    "-noverifyfiles",
];

/// Arguments for starting the client: [`DEFAULT_CLIENT_ARGS`] then `extra`,
/// minus [`STRIPPED_CLIENT_ARGS`] (case-insensitive) and duplicates. Returns
/// the arguments and the stripped flags (for a warning).
#[must_use]
pub fn client_args(extra: &[String]) -> (Vec<String>, Vec<String>) {
    let _ = extra;
    todo!()
}

/// `["-silent", ...client_args(extra)]`: start minimized to the tray.
#[must_use]
pub fn silent_client_args(extra: &[String]) -> (Vec<String>, Vec<String>) {
    let _ = extra;
    todo!()
}

/// `["-applaunch", "<appid>", game_args...]`, sent to a running client (a
/// second `steam.exe` forwards its arguments and exits).
#[must_use]
pub fn applaunch_args(appid: u32, game_args: &[String]) -> Vec<String> {
    let _ = (appid, game_args);
    todo!()
}

/// `["-shutdown"]`: ask a running client to exit.
#[must_use]
pub fn shutdown_args() -> Vec<String> {
    vec!["-shutdown".to_owned()]
}

/// Registry key whose `pid` value is non-zero while the client runs.
pub const ACTIVE_PROCESS_KEY: &str = r"HKCU\Software\Valve\Steam\ActiveProcess";

/// Parse the output of `wine reg query <ACTIVE_PROCESS_KEY> /v pid`, e.g.
/// `    pid    REG_DWORD    0x1a2b`. Returns the pid (0 when Steam is not
/// running), or `None` if no `pid` line is present.
#[must_use]
pub fn parse_active_pid(reg_query_output: &str) -> Option<u32> {
    let _ = reg_query_output;
    todo!()
}

/// File-name prefixes (lowercase) of executables in a game directory that
/// are never the game itself: crash reporters, redistributable installers,
/// anti-cheat bootstrappers and launchers. Used when no profile names the exe.
pub const NON_GAME_EXE_PREFIXES: &[&str] = &[
    "unins",
    "vcredist",
    "vc_redist",
    "dxsetup",
    "dotnet",
    "crashreport",
    "crashpad",
    "crashhandler",
    "bssndrpt",
    "bugsplat",
    "unitycrashhandler",
    "easyanticheat",
    "beservice",
    "ue4prereq",
    "physx",
    "oalinst",
    "launcher",
    "setup",
    "install",
    "steamerrorreporter",
];
