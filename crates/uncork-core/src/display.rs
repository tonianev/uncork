//! The Mac's displays, as `system_profiler SPDisplaysDataType -json` reports
//! them.
//!
//! Games under Wine use the main display (the one with the menu bar). Its
//! "looks like" size in points is the resolution a program that is not
//! DPI-aware should use (Rise of Nations' `Windowed Width` and
//! `Windowed Height`), and its refresh rate is the frame cap DXMT paces to
//! ([`crate::launch::plan`]). Parsing ([`parse`]) is a pure function over
//! the JSON; probing ([`probe`]) runs `/usr/sbin/system_profiler` (about a
//! quarter of a second) and never fails: an error only leaves the display
//! unknown, which launches handle.
//!
//! The shapes `system_profiler` uses vary between Macs and macOS versions:
//!
//! ```json
//! {"SPDisplaysDataType": [{"_name": "Apple M5 Max", "spdisplays_ndrvs": [{
//!   "_name": "Color LCD",
//!   "_spdisplays_pixels": "3456 x 2234",
//!   "_spdisplays_resolution": "1728 x 1117 @ 120.00Hz",
//!   "spdisplays_main": "spdisplays_yes",
//!   "spdisplays_connection_type": "spdisplays_internal",
//!   "spdisplays_display_type": "spdisplays_built-in-liquid-retina-xdr"
//! }]}]}
//! ```
//!
//! External displays may give no refresh rate (`"1920 x 1080 (1080p FHD -
//! Full High Definition)"`), several GPUs each list their own displays, and
//! mirrored displays are listed separately.

use std::io::Read as _;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use serde_json::Value;

use crate::process::CommandSpec;

/// The program that reports the displays.
pub const SYSTEM_PROFILER: &str = "/usr/sbin/system_profiler";

/// How long [`probe`] waits for [`SYSTEM_PROFILER`] (it normally answers in
/// about a quarter of a second).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// When set to a file, [`probe`] reads `system_profiler SPDisplaysDataType
/// -json` output from it instead of asking macOS: for tests, and for
/// reproducing the display setup of a bug report.
pub const DISPLAYS_JSON_ENV: &str = "UNCORK_DISPLAYS_JSON";

/// One display.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Display {
    /// The name macOS gives it (`Color LCD` for a built-in panel).
    pub name: String,
    /// The "looks like" size in points: what macOS lays the desktop out in.
    pub points: (u32, u32),
    /// The panel's size in pixels, when reported.
    pub pixels: Option<(u32, u32)>,
    /// The refresh rate in Hz, when reported.
    pub refresh_hz: Option<f64>,
    /// The main display: the one with the menu bar, where Wine puts games.
    pub main: bool,
    /// Built into the Mac.
    pub built_in: bool,
}

impl Display {
    /// The refresh rate rounded to whole Hz (`59.94` → 60).
    #[must_use]
    pub fn refresh_rounded(&self) -> Option<u32> {
        self.refresh_hz
            .filter(|hz| hz.is_finite() && *hz >= 1.0 && *hz < f64::from(u16::MAX))
            .map(|hz| {
                // In range by the filter above, so the cast cannot truncate.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let rounded = hz.round() as u32;
                rounded
            })
    }

    /// What identifies the main display for a running bottle:
    /// `<name> <width>x<height>` (`Color LCD 1728x1117`). A different
    /// signature means Wine's view of the main display is out of date: the
    /// display was replaced or its "looks like" size changed. The refresh
    /// rate is left out: only the frame cap depends on it, and every launch
    /// reads it anew, so switching a display from 120 to 60 Hz does not
    /// restart the bottle (and end a game in progress).
    #[must_use]
    pub fn signature(&self) -> String {
        let (width, height) = self.points;
        format!("{} {width}x{height}", self.name)
    }

    /// The display for messages: its [`Self::signature`] and refresh rate,
    /// as precise as reported (`Color LCD 1728x1117 @120Hz`,
    /// `TV 1920x1080 @59.94Hz`); without the rate when it is unknown.
    #[must_use]
    pub fn describe(&self) -> String {
        let signature = self.signature();
        match self.refresh_hz.filter(|hz| hz.is_finite() && *hz > 0.0) {
            Some(hz) if (hz - hz.round()).abs() < 0.005 => {
                format!("{signature} @{}Hz", hz.round())
            }
            Some(hz) => format!("{signature} @{hz:.2}Hz"),
            None => signature,
        }
    }
}

/// `recorded`, a session display from `uncork-state.toml`, without the
/// ` @<Hz>Hz` that signatures had before the refresh rate was left out of
/// them ([`Display::signature`]), so a bottle that was running then is not
/// restarted for nothing.
#[must_use]
pub fn without_refresh(recorded: &str) -> &str {
    match recorded.rsplit_once(" @") {
        Some((signature, rate))
            if rate
                .strip_suffix("Hz")
                .is_some_and(|hz| !hz.is_empty() && hz.bytes().all(|b| b.is_ascii_digit())) =>
        {
            signature
        }
        _ => recorded,
    }
}

/// Every display in `json`, the output of `system_profiler
/// SPDisplaysDataType -json`, in the order listed (GPU by GPU). Entries
/// without a readable "looks like" size, and displays macOS reports as
/// offline, are left out. Malformed JSON gives an empty list.
///
/// Per display: `_name`; the size from `_spdisplays_resolution`
/// (`"1728 x 1117 @ 120.00Hz"`), else `spdisplays_resolution`
/// (`"spdisplays_1728x1117Retina"`); the refresh rate after its `@`; the
/// pixels from `_spdisplays_pixels`, else `spdisplays_pixelresolution`;
/// `main` when `spdisplays_main` is `spdisplays_yes`; built in when
/// `spdisplays_connection_type` is `spdisplays_internal` or
/// `spdisplays_display_type` mentions `built-in`.
#[must_use]
pub fn parse(json: &str) -> Vec<Display> {
    let root: Value = match serde_json::from_str(json) {
        Ok(root) => root,
        Err(err) => {
            tracing::debug!("cannot parse system_profiler's display list: {err}");
            return Vec::new();
        }
    };
    let gpus = root
        .get("SPDisplaysDataType")
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice);
    gpus.iter()
        .filter_map(|gpu| gpu.get("spdisplays_ndrvs").and_then(Value::as_array))
        .flatten()
        .filter_map(parse_display)
        .collect()
}

/// One entry of `spdisplays_ndrvs`.
fn parse_display(entry: &Value) -> Option<Display> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str);
    if text("spdisplays_online") == Some("spdisplays_no") {
        return None;
    }
    let resolution = text("_spdisplays_resolution").or_else(|| text("spdisplays_resolution"))?;
    let (size, refresh) = match resolution.split_once('@') {
        Some((size, refresh)) => (size, parse_refresh(refresh)),
        None => (resolution, None),
    };
    let points = parse_size(size)?;
    let pixels = text("_spdisplays_pixels")
        .or_else(|| text("spdisplays_pixelresolution"))
        .and_then(parse_size);
    let built_in = text("spdisplays_connection_type") == Some("spdisplays_internal")
        || text("spdisplays_display_type").is_some_and(|kind| {
            let kind = kind.to_ascii_lowercase();
            kind.contains("built-in") || kind.contains("builtin")
        });
    Some(Display {
        name: text("_name").unwrap_or("display").trim().to_owned(),
        points,
        pixels,
        refresh_hz: refresh,
        main: text("spdisplays_main") == Some("spdisplays_yes"),
        built_in,
    })
}

/// The first `<width> x <height>` in `text` (spaces around `x` optional,
/// a `spdisplays_` prefix and text after the numbers ignored).
fn parse_size(text: &str) -> Option<(u32, u32)> {
    let text = text.trim();
    let text = text.strip_prefix("spdisplays_").unwrap_or(text);
    let digits = |s: &str| -> Option<(u32, usize)> {
        let len = s.bytes().take_while(u8::is_ascii_digit).count();
        Some((s.get(..len)?.parse().ok()?, len))
    };
    let (width, len) = digits(text)?;
    let rest = text[len..]
        .trim_start()
        .strip_prefix(['x', 'X'])?
        .trim_start();
    let (height, _) = digits(rest)?;
    (width > 0 && height > 0).then_some((width, height))
}

/// `120.00Hz` or ` 60 Hz` → the rate.
fn parse_refresh(text: &str) -> Option<f64> {
    let text = text.trim();
    let number = text
        .strip_suffix("Hz")
        .or_else(|| text.strip_suffix("hz"))
        .unwrap_or(text)
        .trim();
    number
        .parse::<f64>()
        .ok()
        .filter(|hz| hz.is_finite() && *hz > 0.0)
}

/// The main display: the one macOS marks as main, else the only display.
/// `None` when there are several and none is marked, or none at all.
#[must_use]
pub fn main_display(displays: &[Display]) -> Option<&Display> {
    let only = match displays {
        [only] => Some(only),
        _ => None,
    };
    displays.iter().find(|display| display.main).or(only)
}

/// Every display, asked from macOS with `system_profiler SPDisplaysDataType
/// -json` (or read from the file [`DISPLAYS_JSON_ENV`] names). Never fails:
/// a missing program, an error exit, a timeout ([`PROBE_TIMEOUT`]) or
/// unreadable output is logged at debug level and gives an empty list.
#[must_use]
pub fn probe() -> Vec<Display> {
    let file = std::env::var_os(DISPLAYS_JSON_ENV).filter(|path| !path.is_empty());
    probe_from(file.as_deref().map(Path::new))
}

/// [`probe`], reading `file` instead of running [`SYSTEM_PROFILER`] when
/// one is given.
fn probe_from(file: Option<&Path>) -> Vec<Display> {
    let Some(file) = file else {
        return system_profiler(PROBE_TIMEOUT).map_or_else(Vec::new, |json| parse(&json));
    };
    match std::fs::read_to_string(file) {
        Ok(json) => parse(&json),
        Err(err) => {
            tracing::debug!(
                "cannot read {} (named by {DISPLAYS_JSON_ENV}): {err}",
                file.display()
            );
            Vec::new()
        }
    }
}

/// [`main_display`] of [`probe`].
#[must_use]
pub fn probe_main() -> Option<Display> {
    let displays = probe();
    let main = main_display(&displays).cloned();
    if main.is_none() {
        tracing::debug!("no main display found among {} displays", displays.len());
    }
    main
}

/// Run `system_profiler SPDisplaysDataType -json`; its stdout, or `None`.
fn system_profiler(timeout: Duration) -> Option<String> {
    let spec = CommandSpec::new(SYSTEM_PROFILER).args(["SPDisplaysDataType", "-json"]);
    let mut command = spec.to_command().ok()?;
    let mut child = match command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            tracing::debug!("cannot run {SYSTEM_PROFILER}: {err}");
            return None;
        }
    };
    let mut stdout = child.stdout.take()?;
    let (sender, receiver) = mpsc::channel();
    // Read on another thread, so a hung program can be killed on time.
    std::thread::spawn(move || {
        let mut text = String::new();
        let read = stdout.read_to_string(&mut text).map(|_| text);
        // The receiver is gone only after a timeout; nothing to report then.
        let _ = sender.send(read);
    });
    let read = receiver.recv_timeout(timeout);
    if read.is_err() {
        tracing::debug!("{SYSTEM_PROFILER} did not answer within {timeout:?}");
        if let Err(err) = child.kill() {
            tracing::debug!("cannot stop {SYSTEM_PROFILER}: {err}");
        }
    }
    let status = child.wait();
    match (read, status) {
        (Ok(Ok(text)), Ok(status)) if status.success() => Some(text),
        (Ok(Err(err)), _) => {
            tracing::debug!("cannot read {SYSTEM_PROFILER}'s output: {err}");
            None
        }
        (_, Ok(status)) => {
            tracing::debug!("{SYSTEM_PROFILER} failed: {status}");
            None
        }
        (_, Err(err)) => {
            tracing::debug!("cannot wait for {SYSTEM_PROFILER}: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The built-in display of a 16-inch `MacBook` Pro (M5 Max, macOS 27.0.1),
    /// as `system_profiler` printed it on 2026-10-08 (identifiers removed).
    const BUILT_IN_ONLY: &str = r#"{
  "SPDisplaysDataType" : [
    {
      "_name" : "Apple M5 Max",
      "spdisplays_mtlgpufamilysupport" : "spdisplays_metal4",
      "spdisplays_ndrvs" : [
        {
          "_name" : "Color LCD",
          "_spdisplays_displayID" : "1",
          "_spdisplays_pixels" : "3456 x 2234",
          "_spdisplays_resolution" : "1728 x 1117 @ 120.00Hz",
          "spdisplays_ambient_brightness" : "spdisplays_yes",
          "spdisplays_connection_type" : "spdisplays_internal",
          "spdisplays_display_type" : "spdisplays_built-in-liquid-retina-xdr",
          "spdisplays_main" : "spdisplays_yes",
          "spdisplays_mirror" : "spdisplays_off",
          "spdisplays_online" : "spdisplays_yes",
          "spdisplays_pixelresolution" : "spdisplays_3456x2234Retina"
        }
      ],
      "spdisplays_vendor" : "sppci_vendor_Apple",
      "sppci_bus" : "spdisplays_builtin",
      "sppci_cores" : "40",
      "sppci_device_type" : "spdisplays_gpu",
      "sppci_model" : "Apple M5 Max"
    }
  ]
}"#;

    /// The built-in display plus a 5K external display that is the main one.
    const EXTERNAL_5K_MAIN: &str = r#"{
  "SPDisplaysDataType" : [
    {
      "_name" : "Apple M5 Max",
      "spdisplays_ndrvs" : [
        {
          "_name" : "Color LCD",
          "_spdisplays_pixels" : "3456 x 2234",
          "_spdisplays_resolution" : "1728 x 1117 @ 120.00Hz",
          "spdisplays_connection_type" : "spdisplays_internal",
          "spdisplays_display_type" : "spdisplays_built-in-liquid-retina-xdr",
          "spdisplays_mirror" : "spdisplays_off",
          "spdisplays_online" : "spdisplays_yes"
        },
        {
          "_name" : "LG UltraFine",
          "_spdisplays_pixels" : "5120 x 2880",
          "_spdisplays_resolution" : "2560 x 1440 @ 60.00Hz",
          "spdisplays_main" : "spdisplays_yes",
          "spdisplays_mirror" : "spdisplays_off",
          "spdisplays_online" : "spdisplays_yes",
          "spdisplays_pixelresolution" : "spdisplays_5120x2880Retina"
        }
      ]
    }
  ]
}"#;

    fn built_in() -> Display {
        Display {
            name: "Color LCD".to_owned(),
            points: (1728, 1117),
            pixels: Some((3456, 2234)),
            refresh_hz: Some(120.0),
            main: true,
            built_in: true,
        }
    }

    fn only(json: &str) -> Display {
        let displays = parse(json);
        assert_eq!(displays.len(), 1, "{displays:#?}");
        displays.into_iter().next().unwrap()
    }

    #[test]
    fn reads_the_built_in_display() {
        let displays = parse(BUILT_IN_ONLY);
        assert_eq!(displays, [built_in()]);
        let main = main_display(&displays).unwrap();
        assert_eq!(main.signature(), "Color LCD 1728x1117");
        assert_eq!(main.describe(), "Color LCD 1728x1117 @120Hz");
        assert_eq!(main.refresh_rounded(), Some(120));
    }

    #[test]
    fn the_main_display_is_the_one_macos_marks() {
        let displays = parse(EXTERNAL_5K_MAIN);
        assert_eq!(displays.len(), 2);
        assert_eq!(
            displays[0],
            Display {
                main: false,
                ..built_in()
            }
        );
        let main = main_display(&displays).unwrap();
        assert_eq!(
            *main,
            Display {
                name: "LG UltraFine".to_owned(),
                points: (2560, 1440),
                pixels: Some((5120, 2880)),
                refresh_hz: Some(60.0),
                main: true,
                built_in: false,
            }
        );
        assert_eq!(main.signature(), "LG UltraFine 2560x1440");
        assert_eq!(main.describe(), "LG UltraFine 2560x1440 @60Hz");
    }

    #[test]
    fn rounds_fractional_refresh_rates() {
        let display = only(
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_name":"TV",
            "_spdisplays_resolution":"1920 x 1080 @ 59.94Hz","spdisplays_main":"spdisplays_yes"}]}]}"#,
        );
        assert_eq!(display.refresh_hz, Some(59.94));
        assert_eq!(display.refresh_rounded(), Some(60));
        assert_eq!(display.pixels, None);
        assert!(!display.built_in);
        assert_eq!(display.signature(), "TV 1920x1080");
        assert_eq!(display.describe(), "TV 1920x1080 @59.94Hz");
    }

    #[test]
    fn a_missing_refresh_rate_is_unknown() {
        let display = only(
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_name":"DELL U2720Q",
            "_spdisplays_resolution":"3840 x 2160 (2160p/4K UHD 1 - Ultra High Definition)",
            "_spdisplays_pixels":"3840 x 2160","spdisplays_main":"spdisplays_yes"}]}]}"#,
        );
        assert_eq!(display.points, (3840, 2160));
        assert_eq!(display.refresh_hz, None);
        assert_eq!(display.refresh_rounded(), None);
        assert_eq!(display.signature(), "DELL U2720Q 3840x2160");
        assert_eq!(display.describe(), "DELL U2720Q 3840x2160");

        let named_mode = only(
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_name":"TV",
            "_spdisplays_resolution":"3840 x 2160 (2160p/4K UHD 1 - Ultra High Definition) @ 60.00Hz"}]}]}"#,
        );
        assert_eq!(named_mode.points, (3840, 2160));
        assert_eq!(named_mode.refresh_hz, Some(60.0));
    }

    #[test]
    fn mirrored_displays_are_listed_and_the_main_one_wins() {
        let displays = parse(
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[
            {"_name":"Color LCD","_spdisplays_resolution":"1728 x 1117 @ 120.00Hz",
             "spdisplays_connection_type":"spdisplays_internal","spdisplays_mirror":"spdisplays_on"},
            {"_name":"Projector","_spdisplays_resolution":"1920 x 1080 @ 60.00Hz",
             "spdisplays_main":"spdisplays_yes","spdisplays_mirror":"spdisplays_on"}]}]}"#,
        );
        assert_eq!(displays.len(), 2);
        assert_eq!(main_display(&displays).unwrap().name, "Projector");
    }

    #[test]
    fn displays_of_every_gpu_are_read() {
        let displays = parse(
            r#"{"SPDisplaysDataType":[
            {"_name":"Intel UHD Graphics 630","spdisplays_ndrvs":[{"_name":"Color LCD",
             "_spdisplays_resolution":"1792 x 1120 @ 60.00Hz","spdisplays_display_type":"spdisplays_built-in_retinaLCD"}]},
            {"_name":"AMD Radeon Pro 5500M","spdisplays_ndrvs":[{"_name":"Studio Display",
             "_spdisplays_resolution":"2560 x 1440 @ 60.00Hz","spdisplays_main":"spdisplays_yes"}]},
            {"_name":"eGPU without displays"}]}"#,
        );
        assert_eq!(displays.len(), 2);
        assert!(displays[0].built_in, "{:?}", displays[0]);
        assert_eq!(main_display(&displays).unwrap().name, "Studio Display");
    }

    #[test]
    fn older_shapes_and_odd_entries() {
        let displays = parse(
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[
            {"_name":"Old","spdisplays_resolution":"spdisplays_1440x900Retina","spdisplays_pixelresolution":"spdisplays_2880x1800Retina"},
            {"_name":"Asleep","_spdisplays_resolution":"1920 x 1080 @ 60.00Hz","spdisplays_online":"spdisplays_no"},
            {"_name":"No size","_spdisplays_resolution":"unknown"},
            {"_spdisplays_resolution":"800x600@75Hz"}]}]}"#,
        );
        assert_eq!(displays.len(), 2, "{displays:#?}");
        assert_eq!(displays[0].points, (1440, 900));
        assert_eq!(displays[0].pixels, Some((2880, 1800)));
        assert_eq!(displays[1].name, "display");
        assert_eq!(displays[1].points, (800, 600));
        assert_eq!(displays[1].refresh_hz, Some(75.0));
        assert_eq!(
            main_display(&displays),
            None,
            "several displays and none is marked"
        );
        assert_eq!(main_display(&displays[1..]).unwrap().points, (800, 600));
    }

    #[test]
    fn garbage_gives_no_displays() {
        for json in [
            "",
            "not json",
            "[]",
            "{}",
            r#"{"SPDisplaysDataType":{}}"#,
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":"x"}]}"#,
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_spdisplays_resolution":"0 x 0"}]}]}"#,
            r#"{"SPDisplaysDataType":[{"spdisplays_ndrvs":[{"_spdisplays_resolution":42}]}]}"#,
        ] {
            assert_eq!(parse(json), [], "{json}");
        }
        assert_eq!(main_display(&[]), None);
    }

    #[test]
    fn sizes_and_refresh_rates() {
        assert_eq!(parse_size("1728 x 1117"), Some((1728, 1117)));
        assert_eq!(parse_size("2560x1440"), Some((2560, 1440)));
        assert_eq!(parse_size("spdisplays_3456x2234Retina"), Some((3456, 2234)));
        assert_eq!(parse_size(" 800 X 600 (SVGA)"), Some((800, 600)));
        assert_eq!(parse_size("x 600"), None);
        assert_eq!(parse_size("800 by 600"), None);
        assert_eq!(parse_size("99999999999 x 1"), None);
        assert_eq!(parse_refresh(" 120.00Hz"), Some(120.0));
        assert_eq!(parse_refresh("60 Hz"), Some(60.0));
        assert_eq!(parse_refresh("fast"), None);
        assert_eq!(parse_refresh("-60Hz"), None);
        let odd = Display {
            refresh_hz: Some(f64::NAN),
            ..built_in()
        };
        assert_eq!(odd.refresh_rounded(), None);
        assert_eq!(odd.describe(), "Color LCD 1728x1117");
    }

    #[test]
    fn the_signature_leaves_out_the_refresh_rate() {
        let at_60 = Display {
            refresh_hz: Some(60.0),
            ..built_in()
        };
        assert_eq!(at_60.signature(), built_in().signature());
        assert_ne!(at_60.describe(), built_in().describe());
        let scaled = Display {
            points: (1512, 982),
            ..built_in()
        };
        assert_ne!(scaled.signature(), built_in().signature());

        // Signatures recorded with a refresh rate compare without it.
        assert_eq!(
            without_refresh("Color LCD 1728x1117 @120Hz"),
            "Color LCD 1728x1117"
        );
        for kept in [
            "Color LCD 1728x1117",
            "Odd @ name 800x600",
            "TV 1920x1080 @Hz",
            "TV 1920x1080 @59.94Hz",
        ] {
            assert_eq!(without_refresh(kept), kept);
        }
    }

    #[test]
    fn the_probe_can_read_a_file_instead() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("displays.json");
        std::fs::write(&file, EXTERNAL_5K_MAIN).unwrap();
        let displays = probe_from(Some(&file));
        assert_eq!(
            main_display(&displays).unwrap().describe(),
            "LG UltraFine 2560x1440 @60Hz"
        );
        assert_eq!(probe_from(Some(&dir.path().join("missing.json"))), []);
        std::fs::write(&file, "garbage").unwrap();
        assert_eq!(probe_from(Some(&file)), []);
    }
}
