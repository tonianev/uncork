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
//!
//! # `plan.json`
//!
//! The JSON serialization of the plan's [`CommandSpec`] (`program`, `args`,
//! `env`, `env_clear`, `cwd`, `log`). Paths are strings; each argument is
//! serde's representation of an `OsString`, `{"Unix": [<bytes>]}`, so
//! non-UTF-8 arguments survive. [`read_plan`] parses it back.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;
use crate::launch::LaunchPlan;
use crate::paths::Layout;
use crate::process::CommandSpec;

/// File name of the bundle's executable (`CFBundleExecutable`).
const LAUNCHER: &str = "launch";

/// File name of the launch plan in `Contents/Resources`.
const PLAN_FILE: &str = "plan.json";

/// SHA-256 of the binary `Contents/MacOS/launch` was copied from. Signing
/// rewrites the copy, so comparing it with the source would always differ.
const LAUNCHER_STAMP: &str = "launcher.sha256";

/// Prefix of every wrapper's `CFBundleIdentifier`.
const BUNDLE_ID_PREFIX: &str = "dev.uncork.game.";

/// Apple's code-signing tool.
const CODESIGN: &str = "/usr/bin/codesign";

/// Longest bundle id [`prepare_bundle`] accepts.
const MAX_ID_LEN: usize = 64;

/// A value in the generated property list.
enum PlistValue<'a> {
    String(&'a str),
    True,
}

/// The `Info.plist` for a wrapper bundle. `name` is the display name,
/// `bundle_id` is `dev.uncork.game.<id>`.
///
/// Keys: `CFBundleDisplayName` and `CFBundleName` (= `name`),
/// `CFBundleExecutable` = `launch`, `CFBundleIdentifier` (= `bundle_id`),
/// `CFBundleInfoDictionaryVersion` = `6.0`, `CFBundlePackageType` = `APPL`,
/// `CFBundleShortVersionString` and `CFBundleVersion` (Uncork's version),
/// `GCSupportsGameMode` and `LSSupportsGameMode` = true,
/// `LSApplicationCategoryType` = `public.app-category.games`,
/// `LSMinimumSystemVersion` = `14.0`, `NSHighResolutionCapable` = true.
/// Text is XML-escaped; characters XML 1.0 cannot carry (most control
/// characters) are dropped.
#[must_use]
pub fn info_plist(name: &str, bundle_id: &str) -> String {
    let version = env!("CARGO_PKG_VERSION");
    let entries = [
        ("CFBundleDisplayName", PlistValue::String(name)),
        ("CFBundleExecutable", PlistValue::String(LAUNCHER)),
        ("CFBundleIdentifier", PlistValue::String(bundle_id)),
        ("CFBundleInfoDictionaryVersion", PlistValue::String("6.0")),
        ("CFBundleName", PlistValue::String(name)),
        ("CFBundlePackageType", PlistValue::String("APPL")),
        ("CFBundleShortVersionString", PlistValue::String(version)),
        ("CFBundleVersion", PlistValue::String(version)),
        ("GCSupportsGameMode", PlistValue::True),
        (
            "LSApplicationCategoryType",
            PlistValue::String("public.app-category.games"),
        ),
        ("LSMinimumSystemVersion", PlistValue::String("14.0")),
        ("LSSupportsGameMode", PlistValue::True),
        ("NSHighResolutionCapable", PlistValue::True),
    ];
    let mut plist = String::from(concat!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
        "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
        "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
        "<plist version=\"1.0\">\n<dict>\n",
    ));
    for (key, value) in entries {
        // Writing to a String cannot fail.
        let _ = writeln!(plist, "\t<key>{key}</key>");
        match value {
            PlistValue::String(text) => {
                let _ = writeln!(plist, "\t<string>{}</string>", xml_escape(text));
            }
            PlistValue::True => plist.push_str("\t<true/>\n"),
        }
    }
    plist.push_str("</dict>\n</plist>\n");
    plist
}

/// Escape `text` for XML character data and attribute values, dropping
/// characters XML 1.0 does not allow.
fn xml_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            '\t' | '\n' | '\r' => escaped.push(c),
            c if c < ' ' || c == '\u{fffe}' || c == '\u{ffff}' => {}
            c => escaped.push(c),
        }
    }
    escaped
}

/// Create or refresh `apps/<id>.app` for `plan`: write `Info.plist`, copy
/// `launcher` (normally `std::env::current_exe()`) to `Contents/MacOS/launch`
/// when it differs, write `plan.json`, and ad-hoc sign the bundle with
/// `codesign --force --sign - <bundle>`.
///
/// Details: `id` must be 1–64 characters of ASCII letters, digits, `.`, `_`
/// and `-`, not starting with `.` or `-` ([`crate::Error::InvalidName`]
/// otherwise); the bundle identifier is `dev.uncork.game.<id>` with `_`
/// written as `-`. Files are only rewritten when their content changes, and
/// the launcher is compared by the SHA-256 recorded in
/// `Contents/Resources/launcher.sha256` (signing rewrites the copy). The
/// copy is made executable (`0755`). The bundle is re-signed when anything
/// changed or it has no signature yet; `codesign`'s output goes to
/// `logs/gamemode-codesign.log`.
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
    prepare_bundle_with(layout, id, name, plan, launcher, true)
}

/// [`prepare_bundle`], signing only when `sign` is set (tests).
fn prepare_bundle_with(
    layout: &Layout,
    id: &str,
    name: &str,
    plan: &LaunchPlan,
    launcher: &Path,
    sign: bool,
) -> crate::Result<PathBuf> {
    validate_id(id)?;
    let bundle = layout.apps_dir().join(format!("{id}.app"));
    let contents = bundle.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    for dir in [&macos, &resources] {
        fs::create_dir_all(dir).map_err(|err| Error::io("cannot create directory", dir, err))?;
    }

    let mut changed = write_if_changed(
        &contents.join("Info.plist"),
        info_plist(name, &bundle_id(id)).as_bytes(),
    )?;
    changed |= copy_launcher(
        launcher,
        &macos.join(LAUNCHER),
        &resources.join(LAUNCHER_STAMP),
    )?;
    let plan_path = resources.join(PLAN_FILE);
    let mut plan_json = serde_json::to_vec_pretty(&plan.command).map_err(|err| {
        Error::io(
            "cannot serialize the launch plan for",
            &plan_path,
            io::Error::new(io::ErrorKind::InvalidData, err),
        )
    })?;
    plan_json.push(b'\n');
    changed |= write_if_changed(&plan_path, &plan_json)?;

    let signature = contents.join("_CodeSignature").join("CodeResources");
    if sign && (changed || !signature.is_file()) {
        let mut codesign = CommandSpec::new(CODESIGN)
            .args(["--force", "--sign", "-"])
            .arg(&bundle);
        codesign.log = Some(layout.logs_dir().join("gamemode-codesign.log"));
        crate::process::run(&codesign)?;
    }
    Ok(bundle)
}

/// `dev.uncork.game.<id>`, with `_` (not allowed in bundle identifiers) as `-`.
fn bundle_id(id: &str) -> String {
    format!("{BUNDLE_ID_PREFIX}{}", id.replace('_', "-"))
}

/// Ids are file names: see [`prepare_bundle`].
fn validate_id(id: &str) -> crate::Result<()> {
    let invalid = |reason| Error::InvalidName {
        what: "app bundle",
        name: id.to_owned(),
        reason,
    };
    if id.is_empty() {
        return Err(invalid("it is empty"));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return Err(invalid("use only letters, digits, '.', '_' and '-'"));
    }
    if id.len() > MAX_ID_LEN {
        return Err(invalid("it is longer than 64 characters"));
    }
    if id.starts_with(['.', '-']) {
        return Err(invalid("it must not start with '.' or '-'"));
    }
    Ok(())
}

/// Write `bytes` to `path` atomically unless it already holds them;
/// `true` if it wrote.
fn write_if_changed(path: &Path, bytes: &[u8]) -> crate::Result<bool> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(false),
        Ok(_) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(Error::io("cannot read", path, err)),
    }
    crate::config::write_atomic(path, bytes)?;
    Ok(true)
}

/// Copy `source` to `dest` (mode `0755`) unless `stamp` says this exact
/// source was copied before; `true` if it copied.
fn copy_launcher(source: &Path, dest: &Path, stamp: &Path) -> crate::Result<bool> {
    let source_sha256 = crate::download::sha256_file(source)?;
    let copied = fs::read_to_string(stamp).is_ok_and(|known| known.trim() == source_sha256);
    let executable = fs::Permissions::from_mode(0o755);
    if copied && dest.is_file() {
        fs::set_permissions(dest, executable)
            .map_err(|err| Error::io("cannot make executable", dest, err))?;
        return Ok(false);
    }
    let temp = dest.with_file_name(format!(".{LAUNCHER}.uncork-tmp"));
    let copy = fs::copy(source, &temp)
        .map_err(|err| Error::io("cannot copy the launcher to", &temp, err))
        .and_then(|_bytes| {
            fs::set_permissions(&temp, executable)
                .map_err(|err| Error::io("cannot make executable", &temp, err))
        })
        .and_then(|()| {
            fs::rename(&temp, dest).map_err(|err| Error::io("cannot replace", dest, err))
        });
    if let Err(err) = copy {
        if let Err(cleanup) = fs::remove_file(&temp)
            && cleanup.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!("cannot remove {}: {cleanup}", temp.display());
        }
        return Err(err);
    }
    crate::config::write_atomic(stamp, format!("{source_sha256}\n").as_bytes())?;
    Ok(true)
}

/// `Contents/Resources/plan.json` of `bundle`.
#[must_use]
pub fn plan_path(bundle: &Path) -> PathBuf {
    bundle.join("Contents").join("Resources").join(PLAN_FILE)
}

/// The on-disk form of a [`CommandSpec`] (which is only `Serialize`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanFile {
    program: PathBuf,
    args: Vec<OsString>,
    env: BTreeMap<String, String>,
    env_clear: bool,
    cwd: Option<PathBuf>,
    log: Option<PathBuf>,
}

/// Read a `plan.json` written by [`prepare_bundle`] (for the launcher's
/// `launch exec <plan.json>`).
///
/// # Errors
/// [`crate::Error::Io`] if it cannot be read, [`crate::Error::Config`] if
/// it is not a serialized [`CommandSpec`].
pub fn read_plan(path: &Path) -> crate::Result<CommandSpec> {
    let bytes = fs::read(path).map_err(|err| Error::io("cannot read", path, err))?;
    let file: PlanFile = serde_json::from_slice(&bytes).map_err(|err| Error::Config {
        what: "launch plan",
        path: path.to_path_buf(),
        message: err.to_string(),
    })?;
    Ok(CommandSpec {
        program: file.program,
        args: file.args,
        env: file.env,
        env_clear: file.env_clear,
        cwd: file.cwd,
        log: file.log,
    })
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

#[cfg(test)]
mod tests {
    use std::os::unix::ffi::OsStringExt as _;

    use super::*;
    use crate::graphics::{Activation, Backend, Strategy};

    fn plan() -> LaunchPlan {
        let mut command = CommandSpec::new("/rt/bin/wine")
            .arg("/games/RoN/riseofnations.exe")
            .arg(OsString::from_vec(b"bad-\xff-arg".to_vec()))
            .env("WINEPREFIX", "/b/steam");
        command.env_clear = true;
        command.cwd = Some(PathBuf::from("/games/RoN"));
        command.log = Some(PathBuf::from("/u/logs/steam-riseofnations-1.log"));
        LaunchPlan {
            command,
            activation: Activation {
                backend: Backend::Wined3d,
                version: None,
                strategy: Strategy::Builtin,
                copies: Vec::new(),
                env: BTreeMap::new(),
                overrides: BTreeMap::new(),
            },
            backend_reason: "test".to_owned(),
            log: PathBuf::from("/u/logs/steam-riseofnations-1.log"),
            warnings: Vec::new(),
            game_mode: None,
            frame_cap: None,
            display: None,
        }
    }

    fn launcher(dir: &Path, contents: &str) -> PathBuf {
        let path = dir.join("uncork-bin");
        fs::write(&path, contents).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        path
    }

    fn mtime(path: &Path) -> std::time::SystemTime {
        fs::metadata(path).unwrap().modified().unwrap()
    }

    #[test]
    fn escapes_xml() {
        assert_eq!(
            xml_escape("Tom & Jerry's <\"Game\">"),
            "Tom &amp; Jerry&apos;s &lt;&quot;Game&quot;&gt;"
        );
        assert_eq!(xml_escape("a\u{0}b\u{7}c\td\u{ffff}"), "abc\td");
        assert_eq!(xml_escape("Æon — 日本"), "Æon — 日本");
    }

    #[test]
    fn bundle_ids_are_valid_identifiers() {
        assert_eq!(
            bundle_id("rise-of-nations"),
            "dev.uncork.game.rise-of-nations"
        );
        assert_eq!(bundle_id("a_b.c"), "dev.uncork.game.a-b.c");
    }

    #[test]
    fn ids_must_be_file_names() {
        for id in ["", ".hidden", "-x", "a/b", "a b", "..", &"x".repeat(65)] {
            assert!(
                matches!(
                    validate_id(id),
                    Err(Error::InvalidName {
                        what: "app bundle",
                        ..
                    })
                ),
                "{id:?}"
            );
        }
        for id in ["rise-of-nations-extended-edition", "x", "A.b_c-1"] {
            assert!(validate_id(id).is_ok(), "{id}");
        }
    }

    #[test]
    fn prepares_the_bundle_layout() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::at(temp.path().join("home"));
        let source = launcher(temp.path(), "#!/bin/sh\nexit 0\n");
        let bundle =
            prepare_bundle_with(&layout, "ron", "Rise of Nations", &plan(), &source, false)
                .unwrap();
        assert_eq!(bundle, layout.apps_dir().join("ron.app"));
        let contents = bundle.join("Contents");
        let plist = fs::read_to_string(contents.join("Info.plist")).unwrap();
        assert_eq!(plist, info_plist("Rise of Nations", "dev.uncork.game.ron"));
        let copy = contents.join("MacOS").join("launch");
        assert_eq!(fs::read_to_string(&copy).unwrap(), "#!/bin/sh\nexit 0\n");
        assert_eq!(
            fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(read_plan(&plan_path(&bundle)).unwrap(), plan().command);
        assert_eq!(
            fs::read_to_string(contents.join("Resources").join(LAUNCHER_STAMP))
                .unwrap()
                .trim(),
            crate::download::sha256_file(&source).unwrap()
        );
    }

    #[test]
    fn refreshing_rewrites_only_what_changed() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::at(temp.path().join("home"));
        let source = launcher(temp.path(), "v1");
        let bundle = prepare_bundle_with(&layout, "g", "G", &plan(), &source, false).unwrap();
        let copy = bundle.join("Contents/MacOS/launch");
        let plist = bundle.join("Contents/Info.plist");
        // Simulate signing: the copy no longer equals its source.
        fs::write(&copy, "v1 + signature").unwrap();
        let (copy_time, plist_time) = (mtime(&copy), mtime(&plist));
        std::thread::sleep(std::time::Duration::from_millis(20));

        prepare_bundle_with(&layout, "g", "G", &plan(), &source, false).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "v1 + signature");
        assert_eq!(mtime(&copy), copy_time);
        assert_eq!(mtime(&plist), plist_time);

        fs::write(&source, "v2").unwrap();
        let mut changed = plan();
        changed.command.args.push(OsString::from("-window"));
        prepare_bundle_with(&layout, "g", "Renamed", &changed, &source, false).unwrap();
        assert_eq!(fs::read_to_string(&copy).unwrap(), "v2");
        assert!(
            fs::read_to_string(&plist)
                .unwrap()
                .contains("<string>Renamed</string>")
        );
        assert_eq!(read_plan(&plan_path(&bundle)).unwrap(), changed.command);
    }

    #[test]
    fn a_missing_launcher_is_an_io_error() {
        let temp = tempfile::tempdir().unwrap();
        let layout = Layout::at(temp.path().join("home"));
        let err = prepare_bundle_with(
            &layout,
            "g",
            "G",
            &plan(),
            &temp.path().join("missing"),
            false,
        )
        .unwrap_err();
        assert!(matches!(err, Error::Io { .. }), "{err:?}");
    }

    #[test]
    fn read_plan_rejects_other_json() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("plan.json");
        fs::write(&path, "{\"program\": \"/x\"}").unwrap();
        assert!(matches!(
            read_plan(&path),
            Err(Error::Config {
                what: "launch plan",
                ..
            })
        ));
        assert!(matches!(
            read_plan(&temp.path().join("absent.json")),
            Err(Error::Io { .. })
        ));
    }
}
