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
pub const INSTALLER_SHA256: &str =
    "7d3654531c32d941b8cae81c4137fc542172bfa9635f169cb392f245a0a12bcb";

/// Silent-install switch of the NSIS installer.
pub const INSTALLER_SILENT: &str = "/S";

/// Flags Uncork passes every time it starts the client.
///
/// - `-nofriendsui`: skip the friends web view (less CEF work while gaming).
///
/// Deliberately *not* passed: `-cef-disable-gpu` and
/// `-cef-disable-gpu-compositing`. Measured on 2026-10-07 (client build
/// 1788652215, Wine 11.17 with CrossOver 26.3's changes, macOS 27.0.1): with
/// them every Steam window stays black, because software-composited CEF
/// output does not cross Wine's process boundary on macOS. Steam's web UI
/// needs a real Direct3D 11 device instead, which Uncork provides by putting
/// DXVK next to `steamwebhelper.exe` (see `uncork_core::steam`).
pub const DEFAULT_CLIENT_ARGS: &[&str] = &["-nofriendsui"];

/// Directory of the 64-bit web helper, relative to the Steam directory.
/// Uncork places DXVK's `d3d11.dll` and `d3d10core.dll` here (app-local
/// DLLs win over `system32` for programs in this directory), so the Steam
/// UI gets a Direct3D 11 device while `system32` stays free for games.
pub const CEF_DIR: &str = "bin/cef/cef.win64";

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
///
/// A duplicate is a flag (an argument starting with `-`) equal to an earlier
/// one ignoring ASCII case, as for stripping; the first spelling is kept.
/// Other arguments are values of the flag before them and are never dropped.
/// Each stripped flag is reported once, as the caller spelled it.
///
/// # Examples
/// ```
/// use uncork_steam::client::client_args;
///
/// let extra = ["-NoFriendsUI", "-nooverlay", "-console"].map(String::from);
/// let (args, stripped) = client_args(&extra);
/// assert_eq!(args, ["-nofriendsui", "-console"]);
/// assert_eq!(stripped, ["-nooverlay"]);
/// ```
#[must_use]
pub fn client_args(extra: &[String]) -> (Vec<String>, Vec<String>) {
    build_client_args(&[], extra)
}

/// `["-silent", ...client_args(extra)]`: start minimized to the tray. A
/// `-silent` in `extra` is a duplicate and is dropped.
#[must_use]
pub fn silent_client_args(extra: &[String]) -> (Vec<String>, Vec<String>) {
    build_client_args(&["-silent"], extra)
}

/// `["-applaunch", "<appid>", game_args...]`, sent to a running client (a
/// second `steam.exe` forwards its arguments and exits).
#[must_use]
pub fn applaunch_args(appid: u32, game_args: &[String]) -> Vec<String> {
    let mut args = vec!["-applaunch".to_owned(), appid.to_string()];
    args.extend_from_slice(game_args);
    args
}

/// `leading`, [`DEFAULT_CLIENT_ARGS`] and `extra`, filtered as documented on
/// [`client_args`].
fn build_client_args(leading: &[&str], extra: &[String]) -> (Vec<String>, Vec<String>) {
    let candidates = leading
        .iter()
        .chain(DEFAULT_CLIENT_ARGS)
        .copied()
        .chain(extra.iter().map(String::as_str));
    let mut args: Vec<String> = Vec::new();
    let mut stripped: Vec<String> = Vec::new();
    for arg in candidates {
        if is_stripped(arg) {
            if !contains_ignore_case(&stripped, arg) {
                stripped.push(arg.to_owned());
            }
        } else if !(is_flag(arg) && contains_ignore_case(&args, arg)) {
            args.push(arg.to_owned());
        }
    }
    (args, stripped)
}

fn is_flag(arg: &str) -> bool {
    arg.starts_with('-')
}

fn is_stripped(arg: &str) -> bool {
    STRIPPED_CLIENT_ARGS
        .iter()
        .any(|flag| flag.eq_ignore_ascii_case(arg))
}

fn contains_ignore_case(list: &[String], arg: &str) -> bool {
    list.iter().any(|known| known.eq_ignore_ascii_case(arg))
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
///
/// The value may be hexadecimal (`0x...`, as `reg` prints a `REG_DWORD`) or
/// decimal; a `pid` line whose value is neither also yields `None`. Lines may
/// end in `\r\n`, as Wine's `reg` writes them.
#[must_use]
pub fn parse_active_pid(reg_query_output: &str) -> Option<u32> {
    reg_query_output
        .lines()
        .find_map(pid_value)
        .and_then(parse_dword)
}

/// The data field of a `pid    REG_<type>    <data>` line.
fn pid_value(line: &str) -> Option<&str> {
    let mut fields = line.split_whitespace();
    let name = fields.next()?;
    let kind = fields.next()?;
    let data = fields.next()?;
    (name.eq_ignore_ascii_case("pid") && kind.starts_with("REG_")).then_some(data)
}

fn parse_dword(data: &str) -> Option<u32> {
    let (digits, radix) = match data.strip_prefix("0x").or_else(|| data.strip_prefix("0X")) {
        Some(hex) => (hex, 16),
        None => (data, 10),
    };
    // `from_str_radix` alone would also accept a leading `+`.
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
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

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|&arg| arg.to_owned()).collect()
    }

    fn defaults_then(extra: &[&str]) -> Vec<String> {
        let mut args = strings(DEFAULT_CLIENT_ARGS);
        args.extend(strings(extra));
        args
    }

    #[test]
    fn no_extra_gives_the_defaults() {
        assert_eq!(client_args(&[]), (strings(DEFAULT_CLIENT_ARGS), vec![]));
    }

    #[test]
    fn extra_arguments_follow_the_defaults_in_order() {
        let extra = strings(&["-console", "-language", "german", "steam://open/games"]);
        let (args, stripped) = client_args(&extra);
        assert_eq!(
            args,
            defaults_then(&["-console", "-language", "german", "steam://open/games"])
        );
        assert!(stripped.is_empty());
    }

    #[test]
    fn every_stripped_flag_is_removed_and_reported() {
        let extra = strings(STRIPPED_CLIENT_ARGS);
        let (args, stripped) = client_args(&extra);
        assert_eq!(args, strings(DEFAULT_CLIENT_ARGS));
        assert_eq!(stripped, extra);
    }

    #[test]
    fn stripping_ignores_case_and_reports_the_user_spelling_once() {
        let extra = strings(&["-NoOverlay", "-console", "-NOOVERLAY", "-NoVerifyFiles"]);
        let (args, stripped) = client_args(&extra);
        assert_eq!(args, defaults_then(&["-console"]));
        assert_eq!(stripped, strings(&["-NoOverlay", "-NoVerifyFiles"]));
    }

    #[test]
    fn duplicate_flags_keep_the_first_occurrence() {
        let extra = strings(&["-console", "-NoFriendsUI", "-CONSOLE", "-console"]);
        let (args, _) = client_args(&extra);
        assert_eq!(args, defaults_then(&["-console"]));
    }

    #[test]
    fn repeated_values_of_different_flags_are_kept() {
        let extra = strings(&["-a", "1", "-b", "1"]);
        let (args, _) = client_args(&extra);
        assert_eq!(args, defaults_then(&["-a", "1", "-b", "1"]));
    }

    #[test]
    fn similar_but_valid_flags_are_not_stripped() {
        // The real flag is `-nobootstrapperupdate`; only the misspelling is dead.
        let extra = strings(&["-nobootstrapperupdate", "-nooverlayx", "nooverlay"]);
        let (args, stripped) = client_args(&extra);
        assert_eq!(
            args,
            defaults_then(&["-nobootstrapperupdate", "-nooverlayx", "nooverlay"])
        );
        assert!(stripped.is_empty());
    }

    #[test]
    fn silent_args_start_with_silent() {
        let extra = strings(&["-console", "-nochatui"]);
        let (args, stripped) = silent_client_args(&extra);
        let mut expected = strings(&["-silent"]);
        expected.extend(defaults_then(&["-console"]));
        assert_eq!(args, expected);
        assert_eq!(stripped, strings(&["-nochatui"]));
    }

    #[test]
    fn silent_args_drop_a_second_silent() {
        let (args, _) = silent_client_args(&strings(&["-SILENT"]));
        let mut expected = strings(&["-silent"]);
        expected.extend(strings(DEFAULT_CLIENT_ARGS));
        assert_eq!(args, expected);
    }

    #[test]
    fn applaunch_args_pass_game_args_verbatim() {
        assert_eq!(
            applaunch_args(287_450, &[]),
            strings(&["-applaunch", "287450"])
        );
        let game = strings(&["+connect", "1.2.3.4", "-nooverlay", "-nooverlay"]);
        assert_eq!(
            applaunch_args(287_450, &game),
            strings(&[
                "-applaunch",
                "287450",
                "+connect",
                "1.2.3.4",
                "-nooverlay",
                "-nooverlay"
            ])
        );
    }

    #[test]
    fn shutdown_args_are_a_single_flag() {
        assert_eq!(shutdown_args(), strings(&["-shutdown"]));
    }

    #[test]
    fn active_pid_from_wine_reg_output_with_crlf() {
        let output = "\r\nHKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\r\n    pid    REG_DWORD    0x1a2b\r\n\r\n";
        assert_eq!(parse_active_pid(output), Some(0x1a2b));
    }

    #[test]
    fn active_pid_zero_means_not_running() {
        let output = "HKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\n    pid    REG_DWORD    0x0\n";
        assert_eq!(parse_active_pid(output), Some(0));
    }

    #[test]
    fn active_pid_accepts_tabs_upper_case_and_decimal() {
        assert_eq!(parse_active_pid("\tPID\tREG_DWORD\t0X1A2B"), Some(0x1a2b));
        assert_eq!(parse_active_pid("    pid    REG_SZ    6699"), Some(6699));
        assert_eq!(parse_active_pid("pid REG_DWORD 0xffffffff"), Some(u32::MAX));
    }

    #[test]
    fn active_pid_ignores_other_values() {
        let output = "HKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\n    \
                      ActiveUser    REG_DWORD    0x0\n    SteamClientDll    REG_SZ    \
                      C:\\Program Files (x86)\\Steam\\steamclient.dll\n    pid    REG_DWORD    0x10\n";
        assert_eq!(parse_active_pid(output), Some(16));
    }

    #[test]
    fn active_pid_absent_or_unreadable_is_none() {
        for output in [
            "",
            "reg: Unable to find the specified registry key or value\r\n",
            "HKEY_CURRENT_USER\\Software\\Valve\\Steam\\ActiveProcess\n",
            "    pid    REG_DWORD",
            "    pid    REG_DWORD    0x",
            "    pid    REG_DWORD    0x+1",
            "    pid    REG_DWORD    +12",
            "    pid    REG_DWORD    0xg1",
            "    pid    REG_QWORD    0x100000000",
            "    pid    is    12",
            "    pidx    REG_DWORD    0x1",
        ] {
            assert_eq!(parse_active_pid(output), None, "output {output:?}");
        }
    }

    fn arg() -> impl Strategy<Value = String> {
        prop_oneof![
            prop::sample::select(
                STRIPPED_CLIENT_ARGS
                    .iter()
                    .chain(DEFAULT_CLIENT_ARGS)
                    .chain(&["-silent", "-console", "-applaunch", "value"])
                    .map(|&arg| arg.to_owned())
                    .collect::<Vec<_>>()
            ),
            "-?[a-zA-Z]{1,8}",
        ]
        .prop_flat_map(|arg| {
            // Randomize ASCII case to exercise case-insensitive matching.
            prop::collection::vec(any::<bool>(), arg.len()).prop_map(move |upper| {
                arg.chars()
                    .zip(upper)
                    .map(|(c, up)| if up { c.to_ascii_uppercase() } else { c })
                    .collect()
            })
        })
    }

    proptest! {
        #[test]
        fn client_args_invariants(extra in prop::collection::vec(arg(), 0..12)) {
            let (args, stripped) = client_args(&extra);
            prop_assert_eq!(&args[..DEFAULT_CLIENT_ARGS.len()], DEFAULT_CLIENT_ARGS);
            prop_assert!(args.iter().all(|arg| !is_stripped(arg)));
            prop_assert!(stripped.iter().all(|arg| is_stripped(arg)));
            for (i, arg) in args.iter().enumerate().filter(|(_, arg)| is_flag(arg)) {
                prop_assert!(!contains_ignore_case(&args[..i], arg), "duplicate {}", arg);
            }
            for arg in &extra {
                let kept = if is_stripped(arg) { &stripped } else { &args };
                prop_assert!(contains_ignore_case(kept, arg), "lost {}", arg);
            }
            let values = |list: &[String]| list.iter().filter(|arg| !is_flag(arg)).count();
            prop_assert_eq!(values(&args), values(&extra));
        }

        #[test]
        fn decimal_and_hex_pids_parse(pid in any::<u32>()) {
            let hex = format!("    pid    REG_DWORD    {pid:#x}\r\n");
            let decimal = format!("    pid    REG_DWORD    {pid}\r\n");
            prop_assert_eq!(parse_active_pid(&hex), Some(pid));
            prop_assert_eq!(parse_active_pid(&decimal), Some(pid));
        }
    }
}
