//! Shared helpers for the CLI integration tests: a throwaway data root
//! (`UNCORK_HOME`), fake components, and a fake Wine made of shell scripts
//! that records every call. Nothing here touches the real
//! `~/Library/Application Support/Uncork`, the network or a real Wine.

#![allow(dead_code, reason = "each test crate uses a different subset")]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use assert_cmd::Command;

/// Version of the fake Wine component [`Home::install_fake_wine`] installs.
pub const FAKE_WINE: &str = "fake-1";

/// `system_profiler SPDisplaysDataType -json` output with only a 16-inch
/// `MacBook` Pro's built-in display (1728x1117 points, 120 Hz), the main
/// display every CLI test sees unless it calls [`Home::set_displays`].
pub const BUILT_IN_DISPLAY: &str = r#"{"SPDisplaysDataType":[{"_name":"Apple M5 Max","spdisplays_ndrvs":[{
  "_name":"Color LCD","_spdisplays_pixels":"3456 x 2234","_spdisplays_resolution":"1728 x 1117 @ 120.00Hz",
  "spdisplays_connection_type":"spdisplays_internal","spdisplays_main":"spdisplays_yes"}]}]}"#;

/// A 5K external display at 60 Hz as the main display, next to the
/// built-in one.
pub const EXTERNAL_5K_DISPLAY: &str = r#"{"SPDisplaysDataType":[{"_name":"Apple M5 Max","spdisplays_ndrvs":[
 {"_name":"Color LCD","_spdisplays_resolution":"1728 x 1117 @ 120.00Hz","spdisplays_connection_type":"spdisplays_internal"},
 {"_name":"LG UltraFine","_spdisplays_pixels":"5120 x 2880","_spdisplays_resolution":"2560 x 1440 @ 60.00Hz","spdisplays_main":"spdisplays_yes"}]}]}"#;

/// A temporary directory holding an Uncork data root and a home directory.
pub struct Home {
    dir: tempfile::TempDir,
}

impl Default for Home {
    fn default() -> Self {
        Self::new()
    }
}

impl Home {
    pub fn new() -> Home {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("home")).unwrap();
        let home = Home { dir };
        home.set_displays(BUILT_IN_DISPLAY);
        home
    }

    /// The displays `uncork` sees from now on, as `system_profiler` JSON
    /// (the file `UNCORK_DISPLAYS_JSON` names).
    pub fn set_displays(&self, json: &str) {
        fs::write(self.displays_file(), json).unwrap();
    }

    fn displays_file(&self) -> PathBuf {
        self.path().join("displays.json")
    }

    /// The marker file of the fake wineserver: present while one "runs".
    pub fn server_file(&self) -> PathBuf {
        self.path().join("wineserver-running")
    }

    /// Make the fake Wine act as if a wineserver ran for every prefix.
    pub fn start_fake_wineserver(&self) {
        fs::write(self.server_file(), "").unwrap();
    }

    /// The temporary directory (for test files outside the data root).
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// The data root, `UNCORK_HOME`.
    pub fn root(&self) -> PathBuf {
        self.dir.path().join("uncork")
    }

    /// `bottles/<name>`.
    pub fn bottle(&self, name: &str) -> PathBuf {
        self.root().join("bottles").join(name)
    }

    /// The `uncork` binary with `UNCORK_HOME`, `HOME` and `USER` pointing at
    /// test values, the displays from [`Home::set_displays`] and `RUST_LOG`
    /// removed.
    pub fn uncork(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_uncork"));
        cmd.env("UNCORK_HOME", self.root())
            .env("HOME", self.path().join("home"))
            .env("USER", "tester")
            .env("LOGNAME", "tester")
            .env("UNCORK_DISPLAYS_JSON", self.displays_file())
            .env_remove("RUST_LOG");
        cmd
    }

    /// Run `uncork <args>` and return its stdout, failing the test when it fails.
    pub fn stdout(&self, args: &[&str]) -> String {
        let output = self.uncork().args(args).output().unwrap();
        assert!(
            output.status.success(),
            "uncork {args:?} failed: {}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }

    /// Run `uncork <args>`, expect exit code 1, and return its stderr.
    pub fn stderr_of_failure(&self, args: &[&str]) -> String {
        let output = self.uncork().args(args).output().unwrap();
        assert_eq!(
            output.status.code(),
            Some(1),
            "uncork {args:?}: {}\nstdout: {}\nstderr: {}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stderr).unwrap()
    }

    /// Run `uncork --json <args>` and parse its output.
    pub fn json(&self, args: &[&str]) -> serde_json::Value {
        let mut all = vec!["--json"];
        all.extend_from_slice(args);
        let text = self.stdout(&all);
        serde_json::from_str(&text).unwrap_or_else(|err| panic!("{err}: {text}"))
    }

    /// Write `components/<kind>/<version>/component.toml` (with `features`)
    /// and empty files at `files` (relative to the component directory).
    pub fn install_component(
        &self,
        kind: &str,
        version: &str,
        features: &[&str],
        files: &[&str],
    ) -> PathBuf {
        let dir = self.root().join("components").join(kind).join(version);
        fs::create_dir_all(&dir).unwrap();
        let features = features
            .iter()
            .map(|feature| format!("{feature:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let meta = format!(
            "schema = 1\nkind = {kind:?}\nversion = {version:?}\nlicense = \"MIT\"\n\
             features = [{features}]\ninstalled_unix = 0\n\n[source]\ntype = \"local\"\npath = \"/fake\"\n"
        );
        fs::write(dir.join("component.toml"), meta).unwrap();
        for file in files {
            let path = dir.join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fake").unwrap();
        }
        dir
    }

    /// Install a Wine component whose `bin/wine` and `bin/wineserver` are
    /// shell scripts appending one line per call to [`Home::calls`].
    /// `wine wineboot` lays out a minimal prefix (`system.reg`,
    /// `drive_c/users/$USER`); `wine reg` fails like a missing key. Every
    /// `wine` call but `wine --version` "starts a wineserver" (creates
    /// [`Home::server_file`]); `wineserver -k0` reports it, `--wait` and
    /// `--kill` end it.
    pub fn install_fake_wine(&self) -> PathBuf {
        let dir = self.install_component(
            "wine",
            FAKE_WINE,
            &["wow64", "msync", "dxmt"],
            &["lib/wine/x86_64-unix/winemetal.so"],
        );
        for sub in ["lib/wine/x86_64-windows", "lib/wine/x86_64-unix", "bin"] {
            fs::create_dir_all(dir.join(sub)).unwrap();
        }
        let calls = self.calls_file();
        let calls = calls.to_str().unwrap();
        let server = self.server_file();
        let server = server.to_str().unwrap();
        let wine = format!(
            r#"#!/bin/sh
printf 'wine %s | WINEPREFIX=%s USER=%s\n' "$*" "$WINEPREFIX" "$USER" >> '{calls}'
# Like Wine, everything but `wine --version` starts a wineserver.
[ "$1" = --version ] || : > '{server}'
case "$1" in
    wineboot)
        mkdir -p "$WINEPREFIX/drive_c/windows/system32" "$WINEPREFIX/drive_c/windows/syswow64" \
            "$WINEPREFIX/drive_c/users/$USER" "$WINEPREFIX/dosdevices"
        printf 'WINE REGISTRY Version 2\n' > "$WINEPREFIX/system.reg"
        printf 'WINE REGISTRY Version 2\n' > "$WINEPREFIX/user.reg"
        ;;
    --version) echo 'wine-11.0 (fake)' ;;
    reg) echo 'reg: Unable to find the specified registry key' >&2; exit 1 ;;
esac
exit 0
"#
        );
        let wineserver = format!(
            r#"#!/bin/sh
printf 'wineserver %s | WINEPREFIX=%s\n' "$*" "$WINEPREFIX" >> '{calls}'
case "$1" in
    -k0) [ -f '{server}' ] && exit 0; exit 1 ;;
    --wait|--kill) rm -f '{server}' ;;
esac
exit 0
"#
        );
        write_script(&dir.join("bin/wine"), &wine);
        write_script(&dir.join("bin/wineserver"), &wineserver);
        dir
    }

    /// Replace the fake Wine's `bin/wine` and `bin/wineserver` with ones
    /// that act out a Steam client that crashed: `reg query` reports its
    /// pid (0x274, or 0 once `reg add` cleared it), but no wineserver runs
    /// (`wineserver -k0` and `--kill` exit 1). Every call is still recorded.
    pub fn fake_crashed_steam(&self) {
        let dir = self.root().join("components/wine").join(FAKE_WINE);
        let calls = self.calls_file();
        let calls = calls.to_str().unwrap();
        let pid = self.path().join("steam-pid");
        let pid = pid.to_str().unwrap();
        fs::write(pid, "0x274").unwrap();
        let wine = format!(
            r#"#!/bin/sh
printf 'wine %s | WINEPREFIX=%s USER=%s\n' "$*" "$WINEPREFIX" "$USER" >> '{calls}'
case "$1 $2" in
    "reg query") printf '    pid    REG_DWORD    %s\r\n' "$(cat '{pid}')"; exit 0 ;;
    "reg add") printf '0x0' > '{pid}'; exit 0 ;;
esac
exit 0
"#
        );
        let wineserver = format!(
            "#!/bin/sh\nprintf 'wineserver %s | WINEPREFIX=%s\\n' \"$*\" \"$WINEPREFIX\" >> '{calls}'\nexit 1\n"
        );
        write_script(&dir.join("bin/wine"), &wine);
        write_script(&dir.join("bin/wineserver"), &wineserver);
    }

    /// Replace the fake Wine's `bin/wine` and `bin/wineserver` with ones
    /// that act out a running Steam client: `reg query` reports pid 0x274
    /// and `wineserver -k0` finds a wineserver. Every call is recorded.
    pub fn fake_running_steam(&self) {
        let dir = self.root().join("components/wine").join(FAKE_WINE);
        let calls = self.calls_file();
        let calls = calls.to_str().unwrap();
        let wine = format!(
            r#"#!/bin/sh
printf 'wine %s | WINEPREFIX=%s USER=%s\n' "$*" "$WINEPREFIX" "$USER" >> '{calls}'
case "$1 $2" in
    "reg query") printf '    pid    REG_DWORD    0x274\r\n'; exit 0 ;;
esac
exit 0
"#
        );
        let wineserver = format!(
            "#!/bin/sh\nprintf 'wineserver %s | WINEPREFIX=%s\\n' \"$*\" \"$WINEPREFIX\" >> '{calls}'\nexit 0\n"
        );
        write_script(&dir.join("bin/wine"), &wine);
        write_script(&dir.join("bin/wineserver"), &wineserver);
    }

    fn calls_file(&self) -> PathBuf {
        self.path().join("calls.log")
    }

    /// Every call the fake Wine recorded, in order.
    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.calls_file())
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Write a bottle directly (no Wine involved): `uncork.toml` naming
    /// `wine`, a `system.reg` and a state recording `prefix_wine`, so it
    /// counts as initialized.
    pub fn write_bottle(&self, name: &str, wine: &str) -> PathBuf {
        let dir = self.write_half_made_bottle(name, wine);
        fs::write(dir.join("system.reg"), "WINE REGISTRY Version 2\n").unwrap();
        fs::write(
            dir.join("uncork-state.toml"),
            format!("schema = 1\nprefix_wine = {wine:?}\n"),
        )
        .unwrap();
        dir
    }

    /// A bottle as a failed `bottle create` leaves it: `uncork.toml` only.
    pub fn write_half_made_bottle(&self, name: &str, wine: &str) -> PathBuf {
        let dir = self.bottle(name);
        fs::create_dir_all(dir.join("drive_c/users/tester")).unwrap();
        fs::write(
            dir.join("uncork.toml"),
            format!("schema = 1\nname = {name:?}\nwine = {wine:?}\n"),
        )
        .unwrap();
        dir
    }

    /// Put a Steam client and one app manifest into `bottle`.
    pub fn install_fake_steam(&self, bottle: &str, apps: &[(u32, &str, u32)]) -> PathBuf {
        let steam = self
            .bottle(bottle)
            .join("drive_c/Program Files (x86)/Steam");
        fs::create_dir_all(steam.join("steamapps/common")).unwrap();
        fs::write(steam.join("steam.exe"), b"MZ fake").unwrap();
        for (appid, name, state_flags) in apps {
            let installdir = name.replace(':', "");
            fs::create_dir_all(steam.join("steamapps/common").join(&installdir)).unwrap();
            fs::write(
                steam.join(format!("steamapps/appmanifest_{appid}.acf")),
                format!(
                    "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\
                     \t\"installdir\"\t\t\"{installdir}\"\n\t\"StateFlags\"\t\t\"{state_flags}\"\n\
                     \t\"SizeOnDisk\"\t\t\"1500000000\"\n\t\"buildid\"\t\t\"14787288\"\n}}\n"
                ),
            )
            .unwrap();
        }
        steam
    }
}

/// Write an executable shell script.
pub fn write_script(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}
