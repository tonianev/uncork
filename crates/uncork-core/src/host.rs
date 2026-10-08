//! Facts about the Mac Uncork runs on, and the `doctor` checks built on them.
//!
//! Probing ([`HostInfo::probe`]) runs a few read-only system commands.
//! Evaluation ([`evaluate`]) is a pure function over the probed facts, so
//! every check is unit-tested with hand-written [`HostInfo`] values.

use std::ffi::OsStr;
use std::io;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::str::FromStr;

use serde::Serialize;

use crate::bottle::{Bottle, DisplayRegistry};
use crate::component::{ComponentKind, InstalledComponent};

/// Whether Rosetta 2 can run x86-64 code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Rosetta {
    /// `arch -x86_64 /usr/bin/true` succeeds.
    Installed,
    /// Apple Silicon without Rosetta.
    Missing,
    /// Intel Mac: not needed.
    NotNeeded,
    /// Could not tell.
    Unknown,
}

/// Probed host facts. Every field is optional where probing can fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostInfo {
    /// `std::env::consts::OS` (`macos` on supported hosts).
    pub os: String,
    /// `true` on Apple Silicon (`sysctl -n hw.optional.arm64` = 1). On an
    /// x86-64 process running under Rosetta this is still `true`.
    pub apple_silicon: bool,
    /// `sw_vers -productVersion`, e.g. `27.0.1`.
    pub macos_version: Option<String>,
    /// `sw_vers -buildVersion`, e.g. `26A434`.
    pub macos_build: Option<String>,
    /// `sysctl -n machdep.cpu.brand_string`, e.g. `Apple M5 Max`.
    pub chip: Option<String>,
    /// `sysctl -n hw.memsize`.
    pub memory_bytes: Option<u64>,
    /// `sysctl -n hw.perflevel0.physicalcpu` (performance cores), falling
    /// back to `hw.physicalcpu`.
    pub performance_cores: Option<u32>,
    /// Rosetta status.
    pub rosetta: Rosetta,
    /// Free bytes on the volume holding the Uncork data root.
    pub free_bytes: Option<u64>,
    /// `/Applications/CrossOver.app` exists (bottles can be imported).
    pub crossover_installed: bool,
}

impl HostInfo {
    /// Probe the running system. Never fails; unknown facts are `None`.
    /// `data_root` is used for the free-space query (`df -k`); it may not
    /// exist yet, in which case its nearest existing ancestor is used.
    ///
    /// Rosetta counts as installed when
    /// `/Library/Apple/usr/libexec/oah/libRosettaRuntime` exists and
    /// `arch -x86_64 /usr/bin/true` succeeds, and as missing when that file
    /// is absent or the spawn fails with `EBADARCH` ("Bad CPU type in
    /// executable").
    #[must_use]
    pub fn probe(data_root: &Path) -> HostInfo {
        let os = std::env::consts::OS.to_owned();
        let arm64 = arm64_hardware();
        let rosetta = match (os == "macos", arm64) {
            (true, Some(true)) => probe_rosetta(),
            (true, Some(false)) => Rosetta::NotNeeded,
            _ => Rosetta::Unknown,
        };
        HostInfo {
            apple_silicon: arm64 == Some(true),
            macos_version: command_stdout(SW_VERS, &["-productVersion"]),
            macos_build: command_stdout(SW_VERS, &["-buildVersion"]),
            chip: command_stdout(SYSCTL, &["-n", "machdep.cpu.brand_string"]),
            memory_bytes: sysctl_number("hw.memsize"),
            performance_cores: sysctl_number("hw.perflevel0.physicalcpu")
                .or_else(|| sysctl_number("hw.physicalcpu")),
            rosetta,
            free_bytes: free_bytes(data_root),
            crossover_installed: Path::new(CROSSOVER_APP).exists(),
            os,
        }
    }

    /// Major macOS version (`27` for `27.0.1`).
    #[must_use]
    pub fn macos_major(&self) -> Option<u32> {
        self.macos_version
            .as_deref()?
            .split('.')
            .next()?
            .parse()
            .ok()
    }
}

/// Severity of a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    /// Fine.
    Ok,
    /// Informational.
    Info,
    /// Works, but something should be fixed or known.
    Warn,
    /// Uncork cannot run games until this is fixed.
    Fail,
}

/// One `doctor` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    /// Stable id, kebab-case (e.g. `rosetta`, `wine-installed`).
    pub id: &'static str,
    /// Severity.
    pub status: Status,
    /// One line for the user.
    pub summary: String,
    /// Command or action that fixes it.
    pub fix: Option<String>,
}

impl Check {
    fn new(id: &'static str, status: Status, summary: impl Into<String>) -> Check {
        Check {
            id,
            status,
            summary: summary.into(),
            fix: None,
        }
    }

    fn with_fix(mut self, fix: impl Into<String>) -> Check {
        self.fix = Some(fix.into());
        self
    }
}

/// Lowest macOS major version Uncork supports.
pub const MIN_MACOS_MAJOR: u32 = 14;

/// Free space below which `doctor` warns, in bytes (a Steam bottle with one
/// large game needs tens of GB).
pub const LOW_DISK_BYTES: u64 = 20 * 1024 * 1024 * 1024;

/// Evaluate every check, in this fixed order:
///
/// | id | Status and condition | Fix |
/// |---|---|---|
/// | `platform` | Ok: macOS on Apple Silicon. Warn: Intel Mac ("untested"). Fail: not macOS | — |
/// | `macos-version` | Ok: major ≥ [`MIN_MACOS_MAJOR`]. Fail: older. Warn: unknown | "Update macOS" |
/// | `rosetta` | Ok: `Installed` or `NotNeeded`. Fail: `Missing`. Warn: `Unknown` | `softwareupdate --install-rosetta --agree-to-license` |
/// | `rosetta-sunset` | Info when major ≥ 27: "macOS 27 is the last release with general-purpose Rosetta; from macOS 28 Apple keeps only a subset for older games, and Wine-based tools may break" (`docs/ROADMAP.md`). Omitted below 27 | — |
/// | `disk-space` | Ok: free ≥ [`LOW_DISK_BYTES`]. Warn: below. Omitted when unknown | — |
/// | `wine-installed` | Ok: a wine component (names the newest). Fail: none | `uncork runtime install wine` |
/// | `wine-32bit` | Warn: newest wine lacks the `wow64` feature ("32-bit games will not start"). Omitted otherwise or without wine | — |
/// | `dxmt-installed` | Ok / Warn: missing ("D3D10/11 games fall back to OpenGL") | `uncork runtime install dxmt` |
/// | `dxvk-installed` | Ok / Warn: missing ("the Steam client's windows will be black without it": its web UI needs Direct3D 11, which Uncork gives it with DXVK) | `uncork runtime install dxvk` |
/// | `d3dmetal` | Ok: imported (version). Info: not imported ("optional, 64-bit D3D11/D3D12; import your own copy of Apple's Game Porting Toolkit") | `uncork runtime import-gptk <path>` |
/// | `bottles` | Ok: "N bottles". Info: none | `uncork setup` |
/// | `bottle-wine` | Warn, once per bottle whose `wine` version is not installed | `uncork runtime install wine` or `uncork bottle set <name> wine=<v>` |
/// | `crossover` | Info when `CrossOver.app` is installed: "bottles can be imported with `uncork bottle import`". Omitted otherwise | — |
///
/// The `bottle-dpi` checks need each bottle's `user.reg` and come from
/// [`bottle_dpi`]; `doctor` puts them after the `bottle-wine` checks.
///
/// A check carries its row's fix whenever its status is not Ok.
/// `components` is expected in [`crate::component::list_installed`] order
/// (newest first within a kind), so "newest" is the first of a kind. In a
/// `bottle-wine` fix, `<v>` is the newest installed Wine version, or the
/// placeholder `<version>` when there is none.
#[must_use]
pub fn evaluate(
    host: &HostInfo,
    components: &[InstalledComponent],
    bottles: &[Bottle],
) -> Vec<Check> {
    let newest = |kind: ComponentKind| components.iter().find(|c| c.meta.kind == kind);
    let wine = newest(ComponentKind::Wine);

    let mut checks = vec![platform(host), macos_version(host), rosetta(host)];
    checks.extend(rosetta_sunset(host));
    checks.extend(disk_space(host));
    checks.push(wine_installed(components, wine));
    checks.extend(wine_32bit(wine));
    checks.push(dxmt_installed(newest(ComponentKind::Dxmt)));
    checks.push(dxvk_installed(newest(ComponentKind::Dxvk)));
    checks.push(d3dmetal(newest(ComponentKind::D3dmetal)));
    checks.push(bottle_count(bottles));
    checks.extend(bottle_wine(bottles, components, wine));
    checks.extend(crossover(host));
    checks
}

/// The last macOS major release with general-purpose Rosetta; from here on
/// `doctor` mentions that it is ending.
const ROSETTA_SUNSET_MAJOR: u32 = 27;

const ROSETTA_SUNSET: &str = "macOS 27 is the last release with general-purpose Rosetta; \
     from macOS 28 Apple keeps only a subset for older games, and Wine-based tools may break";

const INSTALL_ROSETTA: &str = "softwareupdate --install-rosetta --agree-to-license";
const INSTALL_WINE: &str = "uncork runtime install wine";

fn platform(host: &HostInfo) -> Check {
    const ID: &str = "platform";
    if host.os != "macos" {
        return Check::new(
            ID,
            Status::Fail,
            format!("Uncork runs only on macOS, not {}", host.os),
        );
    }
    if !host.apple_silicon {
        return Check::new(
            ID,
            Status::Warn,
            "Intel Mac: untested; Uncork targets Apple Silicon",
        );
    }
    let chip = host
        .chip
        .as_deref()
        .map_or_else(String::new, |chip| format!(" ({chip})"));
    Check::new(ID, Status::Ok, format!("macOS on Apple Silicon{chip}"))
}

fn macos_version(host: &HostInfo) -> Check {
    const ID: &str = "macos-version";
    const FIX: &str = "Update macOS";
    let (Some(major), Some(version)) = (host.macos_major(), host.macos_version.as_deref()) else {
        return Check::new(ID, Status::Warn, "cannot determine the macOS version").with_fix(FIX);
    };
    let build = host
        .macos_build
        .as_deref()
        .map_or_else(String::new, |build| format!(" ({build})"));
    let described = format!("macOS {version}{build}");
    if major >= MIN_MACOS_MAJOR {
        Check::new(ID, Status::Ok, described)
    } else {
        Check::new(
            ID,
            Status::Fail,
            format!(
                "{described} is older than macOS {MIN_MACOS_MAJOR}, the oldest Uncork supports"
            ),
        )
        .with_fix(FIX)
    }
}

fn rosetta(host: &HostInfo) -> Check {
    const ID: &str = "rosetta";
    match host.rosetta {
        Rosetta::Installed => Check::new(ID, Status::Ok, "Rosetta 2 is installed"),
        Rosetta::NotNeeded => Check::new(ID, Status::Ok, "Rosetta 2 is not needed on an Intel Mac"),
        Rosetta::Missing => Check::new(
            ID,
            Status::Fail,
            "Rosetta 2 is not installed; Wine cannot run without it",
        )
        .with_fix(INSTALL_ROSETTA),
        Rosetta::Unknown => Check::new(
            ID,
            Status::Warn,
            "cannot tell whether Rosetta 2 is installed",
        )
        .with_fix(INSTALL_ROSETTA),
    }
}

fn rosetta_sunset(host: &HostInfo) -> Option<Check> {
    host.macos_major()
        .filter(|&major| major >= ROSETTA_SUNSET_MAJOR)
        .map(|_| Check::new("rosetta-sunset", Status::Info, ROSETTA_SUNSET))
}

fn disk_space(host: &HostInfo) -> Option<Check> {
    const ID: &str = "disk-space";
    let free = host.free_bytes?;
    let amount = format_gib(free);
    Some(if free >= LOW_DISK_BYTES {
        Check::new(ID, Status::Ok, format!("{amount} free"))
    } else {
        Check::new(
            ID,
            Status::Warn,
            format!("only {amount} free; a Steam bottle with one large game needs tens of GB"),
        )
    })
}

/// Bytes as GiB with one decimal, rounded down so that a value below a
/// threshold never prints as the threshold.
fn format_gib(bytes: u64) -> String {
    const GIB: u128 = 1024 * 1024 * 1024;
    let tenths = u128::from(bytes) * 10 / GIB;
    format!("{}.{} GiB", tenths / 10, tenths % 10)
}

fn wine_installed(components: &[InstalledComponent], wine: Option<&InstalledComponent>) -> Check {
    const ID: &str = "wine-installed";
    let Some(wine) = wine else {
        return Check::new(ID, Status::Fail, "no Wine runtime is installed").with_fix(INSTALL_WINE);
    };
    let count = components
        .iter()
        .filter(|c| c.meta.kind == ComponentKind::Wine)
        .count();
    let others = if count > 1 {
        format!(" (newest of {count})")
    } else {
        String::new()
    };
    Check::new(
        ID,
        Status::Ok,
        format!("Wine {} is installed{others}", wine.meta.version),
    )
}

fn wine_32bit(wine: Option<&InstalledComponent>) -> Option<Check> {
    let wine = wine.filter(|wine| !wine.has_feature("wow64"))?;
    Some(Check::new(
        "wine-32bit",
        Status::Warn,
        format!(
            "Wine {} lacks the wow64 feature: 32-bit games will not start",
            wine.meta.version
        ),
    ))
}

fn dxmt_installed(dxmt: Option<&InstalledComponent>) -> Check {
    const ID: &str = "dxmt-installed";
    match dxmt {
        Some(dxmt) => Check::new(
            ID,
            Status::Ok,
            format!("DXMT {} is installed", dxmt.meta.version),
        ),
        None => Check::new(
            ID,
            Status::Warn,
            "DXMT is not installed: D3D10/11 games fall back to OpenGL",
        )
        .with_fix("uncork runtime install dxmt"),
    }
}

fn dxvk_installed(dxvk: Option<&InstalledComponent>) -> Check {
    const ID: &str = "dxvk-installed";
    match dxvk {
        Some(dxvk) => Check::new(
            ID,
            Status::Ok,
            format!("DXVK {} is installed", dxvk.meta.version),
        ),
        None => Check::new(
            ID,
            Status::Warn,
            "DXVK is not installed: the Steam client's windows will be black without it (its web UI needs Direct3D 11)",
        )
        .with_fix("uncork runtime install dxvk"),
    }
}

fn d3dmetal(d3dmetal: Option<&InstalledComponent>) -> Check {
    const ID: &str = "d3dmetal";
    match d3dmetal {
        Some(d3dmetal) => Check::new(
            ID,
            Status::Ok,
            format!("D3DMetal {} is imported", d3dmetal.meta.version),
        ),
        None => Check::new(
            ID,
            Status::Info,
            "D3DMetal is not imported (optional, 64-bit D3D11/D3D12; \
             import your own copy of Apple's Game Porting Toolkit)",
        )
        .with_fix("uncork runtime import-gptk <path>"),
    }
}

fn bottle_count(bottles: &[Bottle]) -> Check {
    const ID: &str = "bottles";
    match bottles.len() {
        0 => Check::new(ID, Status::Info, "no bottles yet").with_fix("uncork setup"),
        1 => Check::new(ID, Status::Ok, "1 bottle"),
        count => Check::new(ID, Status::Ok, format!("{count} bottles")),
    }
}

fn bottle_wine(
    bottles: &[Bottle],
    components: &[InstalledComponent],
    newest_wine: Option<&InstalledComponent>,
) -> Vec<Check> {
    let installed = |version: &str| {
        components
            .iter()
            .any(|c| c.meta.kind == ComponentKind::Wine && c.meta.version == version)
    };
    let suggested = newest_wine.map_or("<version>", |wine| wine.meta.version.as_str());
    bottles
        .iter()
        .filter(|bottle| !installed(&bottle.config.wine))
        .map(|bottle| {
            let name = &bottle.config.name;
            Check::new(
                "bottle-wine",
                Status::Warn,
                format!(
                    "bottle {name:?} uses Wine {}, which is not installed",
                    bottle.config.wine
                ),
            )
            .with_fix(format!(
                "{INSTALL_WINE} or uncork bottle set {name} wine={suggested}"
            ))
        })
        .collect()
}

/// The `bottle-dpi` check of one bottle, from its registry as `user.reg`
/// has it ([`DisplayRegistry::read`]): Warn when Retina mode and the DPI
/// disagree ([`DisplayRegistry::disagrees`]: programs that are not
/// DPI-aware then see a screen of the wrong size, and full-screen games
/// render cropped and offset or crash), or when they differ from the
/// bottle's `performance.retina` ([`DisplayRegistry::differs_from`]).
/// The fix, `uncork bottle set <name> performance.retina=<its
/// performance.retina>`, rewrites the pair to match `uncork.toml`. `None`
/// when the registry matches.
#[must_use]
pub fn bottle_dpi(bottle: &Bottle, registry: &DisplayRegistry) -> Option<Check> {
    let name = &bottle.config.name;
    let retina = bottle.config.performance.retina;
    let summary = if registry.disagrees() {
        format!(
            "bottle {name:?}: Retina mode and DPI disagree ({}), so games that are not DPI-aware see a screen of the wrong size",
            registry.describe()
        )
    } else if registry.differs_from(retina) {
        format!(
            "bottle {name:?}: its registry ({}) does not match performance.retina = {retina} in uncork.toml",
            registry.describe()
        )
    } else {
        return None;
    };
    Some(
        Check::new("bottle-dpi", Status::Warn, summary).with_fix(format!(
            "uncork bottle set {name} performance.retina={retina}"
        )),
    )
}

fn crossover(host: &HostInfo) -> Option<Check> {
    host.crossover_installed.then(|| {
        Check::new(
            "crossover",
            Status::Info,
            "CrossOver is installed: bottles can be imported with `uncork bottle import`",
        )
    })
}

// Probing. Absolute paths: these are system tools, and a user's PATH must
// not be able to substitute them.

const SYSCTL: &str = "/usr/sbin/sysctl";
const SW_VERS: &str = "/usr/bin/sw_vers";
const ARCH: &str = "/usr/bin/arch";
const DF: &str = "/bin/df";
const ROSETTA_RUNTIME: &str = "/Library/Apple/usr/libexec/oah/libRosettaRuntime";
const CROSSOVER_APP: &str = "/Applications/CrossOver.app";

/// `EBADARCH` from `<sys/errno.h>`: no slice of the binary can run here.
const EBADARCH: i32 = 86;
/// `strerror(EBADARCH)`, which `arch` prints when its spawn fails that way.
const BAD_CPU_TYPE: &str = "Bad CPU type in executable";

/// Run to completion with stdin empty and stderr discarded (`sysctl`
/// complains about OIDs that older or Intel Macs lack). `None` if the
/// program cannot start.
fn quiet_output<S: AsRef<OsStr>>(program: &str, args: &[S]) -> Option<Output> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
}

/// Trimmed stdout of a successful run, if it is not empty.
fn command_stdout(program: &str, args: &[&str]) -> Option<String> {
    let output = quiet_output(program, args)?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn sysctl_number<T: FromStr>(name: &str) -> Option<T> {
    command_stdout(SYSCTL, &["-n", name])?.parse().ok()
}

/// `Some(true)` on Apple Silicon. `Some(false)` when `sysctl` runs but does
/// not report arm64 (Intel Macs lack the OID). `None` when it cannot run.
fn arm64_hardware() -> Option<bool> {
    let output = quiet_output(SYSCTL, &["-n", "hw.optional.arm64"])?;
    Some(output.status.success() && output.stdout.trim_ascii() == b"1")
}

fn probe_rosetta() -> Rosetta {
    // A missing runtime file is decisive; there is nothing to try.
    if !Path::new(ROSETTA_RUNTIME).exists() {
        return Rosetta::Missing;
    }
    let outcome = Command::new(ARCH)
        .args(["-x86_64", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    rosetta_from_x86_64_run(&outcome)
}

/// Rosetta status from the outcome of `arch -x86_64 /usr/bin/true`, given
/// that the Rosetta runtime file exists.
fn rosetta_from_x86_64_run(outcome: &io::Result<Output>) -> Rosetta {
    match outcome {
        Ok(output) if output.status.success() => Rosetta::Installed,
        Ok(output) if String::from_utf8_lossy(&output.stderr).contains(BAD_CPU_TYPE) => {
            Rosetta::Missing
        }
        Err(error) if error.raw_os_error() == Some(EBADARCH) => Rosetta::Missing,
        Ok(_) | Err(_) => Rosetta::Unknown,
    }
}

/// Free bytes on the volume holding `path` or, if it does not exist yet,
/// its nearest existing ancestor.
fn free_bytes(path: &Path) -> Option<u64> {
    let absolute = std::path::absolute(path).ok()?;
    let existing = absolute.ancestors().find(|dir| dir.exists())?;
    let output = quiet_output(
        DF,
        &[OsStr::new("-P"), OsStr::new("-k"), existing.as_os_str()],
    )?;
    if !output.status.success() {
        return None;
    }
    parse_df_available_kib(&String::from_utf8_lossy(&output.stdout))?.checked_mul(1024)
}

/// The "Available" column of `df -P -k` output, in KiB. The data line is
/// `<filesystem> <blocks> <used> <available> <capacity>% <mount point>`,
/// where both ends may contain spaces, so the columns are found as four
/// adjacent fields shaped like numbers and a percentage.
fn parse_df_available_kib(output: &str) -> Option<u64> {
    let fields: Vec<&str> = output.lines().nth(1)?.split_whitespace().collect();
    fields.windows(4).find_map(|window| {
        let [blocks, used, available, capacity] = window else {
            return None;
        };
        let numeric = blocks.parse::<u64>().is_ok() && used.parse::<u64>().is_ok();
        if !numeric || !capacity.ends_with('%') {
            return None;
        }
        available.parse().ok()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::os::unix::process::ExitStatusExt as _;
    use std::path::PathBuf;
    use std::process::ExitStatus;

    use super::*;
    use crate::bottle::{
        BottleConfig, BottleState, GraphicsConfig, PerformanceConfig, WindowsVersion,
    };
    use crate::component::{ComponentMeta, Source};

    const GIB: u64 = 1024 * 1024 * 1024;

    fn mac() -> HostInfo {
        HostInfo {
            os: "macos".to_owned(),
            apple_silicon: true,
            macos_version: Some("27.0.1".to_owned()),
            macos_build: Some("26A434".to_owned()),
            chip: Some("Apple M5 Max".to_owned()),
            memory_bytes: Some(128 * GIB),
            performance_cores: Some(12),
            rosetta: Rosetta::Installed,
            free_bytes: Some(500 * GIB),
            crossover_installed: false,
        }
    }

    fn component(kind: ComponentKind, version: &str, features: &[&str]) -> InstalledComponent {
        InstalledComponent {
            meta: ComponentMeta {
                schema: 1,
                kind,
                version: version.to_owned(),
                source: Source::Local {
                    path: PathBuf::from("/src"),
                },
                license: "LGPL-2.1-or-later".to_owned(),
                source_code: None,
                features: features.iter().map(|&f| f.to_owned()).collect(),
                installed_unix: crate::unix_now(),
            },
            path: PathBuf::from(format!("/u/components/{kind}/{version}")),
        }
    }

    fn bottle(name: &str, wine: &str) -> Bottle {
        Bottle {
            path: PathBuf::from(format!("/u/bottles/{name}")),
            config: BottleConfig {
                schema: 1,
                name: name.to_owned(),
                wine: wine.to_owned(),
                windows_version: WindowsVersion::default(),
                graphics: GraphicsConfig::default(),
                performance: PerformanceConfig::default(),
                env: BTreeMap::new(),
                dll_overrides: BTreeMap::new(),
                imported_from: None,
            },
            state: BottleState::default(),
        }
    }

    /// Wine 11.17 (`wow64`), DXMT, DXVK and `D3DMetal`.
    fn full_set() -> Vec<InstalledComponent> {
        vec![
            component(ComponentKind::Wine, "11.17", &["wow64", "msync"]),
            component(ComponentKind::Dxmt, "0.80", &[]),
            component(ComponentKind::Dxvk, "1.10.3", &[]),
            component(ComponentKind::D3dmetal, "3.0", &[]),
        ]
    }

    fn ids(checks: &[Check]) -> Vec<&'static str> {
        checks.iter().map(|check| check.id).collect()
    }

    fn only<'a>(checks: &'a [Check], id: &str) -> &'a Check {
        let matching: Vec<&Check> = checks.iter().filter(|c| c.id == id).collect();
        assert_eq!(matching.len(), 1, "expected one {id} check in {checks:#?}");
        matching[0]
    }

    fn check(host: &HostInfo, id: &str) -> Check {
        only(
            &evaluate(host, &full_set(), &[bottle("steam", "11.17")]),
            id,
        )
        .clone()
    }

    fn absent(host: &HostInfo, components: &[InstalledComponent], bottles: &[Bottle], id: &str) {
        let checks = evaluate(host, components, bottles);
        assert!(
            !checks.iter().any(|c| c.id == id),
            "unexpected {id} in {checks:#?}"
        );
    }

    #[test]
    fn healthy_mac_passes_in_the_documented_order() {
        let checks = evaluate(&mac(), &full_set(), &[bottle("steam", "11.17")]);
        assert_eq!(
            ids(&checks),
            [
                "platform",
                "macos-version",
                "rosetta",
                "rosetta-sunset",
                "disk-space",
                "wine-installed",
                "dxmt-installed",
                "dxvk-installed",
                "d3dmetal",
                "bottles",
            ]
        );
        for check in &checks {
            let expected = if check.id == "rosetta-sunset" {
                Status::Info
            } else {
                Status::Ok
            };
            assert_eq!(check.status, expected, "{check:?}");
            assert_eq!(check.fix, None, "{check:?}");
        }
    }

    #[test]
    fn every_optional_row_appears_in_its_place() {
        let host = HostInfo {
            crossover_installed: true,
            ..mac()
        };
        let components = [component(ComponentKind::Wine, "10.0", &[])];
        let bottles = [bottle("a", "9.0"), bottle("b", "10.0"), bottle("c", "8.0")];
        let checks = evaluate(&host, &components, &bottles);
        assert_eq!(
            ids(&checks),
            [
                "platform",
                "macos-version",
                "rosetta",
                "rosetta-sunset",
                "disk-space",
                "wine-installed",
                "wine-32bit",
                "dxmt-installed",
                "dxvk-installed",
                "d3dmetal",
                "bottles",
                "bottle-wine",
                "bottle-wine",
                "crossover",
            ]
        );
    }

    #[test]
    fn fixes_accompany_exactly_the_non_ok_checks() {
        let hosts = [
            mac(),
            HostInfo {
                os: "linux".to_owned(),
                apple_silicon: false,
                macos_version: None,
                rosetta: Rosetta::Unknown,
                free_bytes: Some(GIB),
                crossover_installed: true,
                ..mac()
            },
        ];
        let component_sets = [full_set(), Vec::new()];
        for host in &hosts {
            for components in &component_sets {
                for check in evaluate(host, components, &[bottle("x", "1.0")]) {
                    let has_fix_column = !matches!(
                        check.id,
                        "platform" | "rosetta-sunset" | "disk-space" | "wine-32bit" | "crossover"
                    );
                    assert_eq!(
                        check.fix.is_some(),
                        has_fix_column && check.status != Status::Ok,
                        "{check:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn platform_row() {
        let ok = check(&mac(), "platform");
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.summary, "macOS on Apple Silicon (Apple M5 Max)");
        let no_chip = check(
            &HostInfo {
                chip: None,
                ..mac()
            },
            "platform",
        );
        assert_eq!(no_chip.summary, "macOS on Apple Silicon");

        let intel = check(
            &HostInfo {
                apple_silicon: false,
                rosetta: Rosetta::NotNeeded,
                ..mac()
            },
            "platform",
        );
        assert_eq!(intel.status, Status::Warn);
        assert!(intel.summary.contains("untested"), "{intel:?}");

        let linux = check(
            &HostInfo {
                os: "linux".to_owned(),
                ..mac()
            },
            "platform",
        );
        assert_eq!(linux.status, Status::Fail);
        assert!(linux.summary.contains("linux"), "{linux:?}");
        assert_eq!(linux.fix, None);
    }

    #[test]
    fn macos_version_row() {
        let with = |version: Option<&str>| {
            check(
                &HostInfo {
                    macos_version: version.map(str::to_owned),
                    ..mac()
                },
                "macos-version",
            )
        };
        let current = with(Some("27.0.1"));
        assert_eq!(current.status, Status::Ok);
        assert_eq!(current.summary, "macOS 27.0.1 (26A434)");
        assert_eq!(with(Some("14.0")).status, Status::Ok);
        assert_eq!(with(Some("14")).status, Status::Ok);

        let old = with(Some("13.6.1"));
        assert_eq!(old.status, Status::Fail);
        assert!(old.summary.contains("13.6.1"), "{old:?}");
        assert_eq!(old.fix.as_deref(), Some("Update macOS"));

        for unknown in [None, Some("unknown"), Some("")] {
            let check = with(unknown);
            assert_eq!(check.status, Status::Warn, "{unknown:?}");
            assert_eq!(check.fix.as_deref(), Some("Update macOS"));
        }
    }

    #[test]
    fn rosetta_row() {
        let with = |rosetta| check(&HostInfo { rosetta, ..mac() }, "rosetta");
        assert_eq!(with(Rosetta::Installed).status, Status::Ok);
        assert_eq!(with(Rosetta::NotNeeded).status, Status::Ok);
        let missing = with(Rosetta::Missing);
        assert_eq!(missing.status, Status::Fail);
        assert_eq!(
            missing.fix.as_deref(),
            Some("softwareupdate --install-rosetta --agree-to-license")
        );
        let unknown = with(Rosetta::Unknown);
        assert_eq!(unknown.status, Status::Warn);
        assert_eq!(unknown.fix, missing.fix);
    }

    #[test]
    fn rosetta_sunset_row() {
        let with = |version: Option<&str>| HostInfo {
            macos_version: version.map(str::to_owned),
            ..mac()
        };
        let sunset = check(&with(Some("27.0")), "rosetta-sunset");
        assert_eq!(sunset.status, Status::Info);
        assert_eq!(
            sunset.summary,
            "macOS 27 is the last release with general-purpose Rosetta; from macOS 28 Apple keeps \
             only a subset for older games, and Wine-based tools may break"
        );
        assert_eq!(sunset.fix, None);
        assert_eq!(
            check(&with(Some("28.1")), "rosetta-sunset").status,
            Status::Info
        );
        for version in [Some("26.4"), Some("14.0"), None] {
            absent(&with(version), &full_set(), &[], "rosetta-sunset");
        }
    }

    #[test]
    fn disk_space_row() {
        let with = |free_bytes| HostInfo {
            free_bytes,
            ..mac()
        };
        let enough = check(&with(Some(LOW_DISK_BYTES)), "disk-space");
        assert_eq!(enough.status, Status::Ok);
        assert_eq!(enough.summary, "20.0 GiB free");
        let low = check(&with(Some(LOW_DISK_BYTES - 1)), "disk-space");
        assert_eq!(low.status, Status::Warn);
        assert!(low.summary.starts_with("only 19.9 GiB free"), "{low:?}");
        assert_eq!(low.fix, None);
        assert_eq!(check(&with(Some(0)), "disk-space").status, Status::Warn);
        absent(&with(None), &full_set(), &[], "disk-space");
    }

    #[test]
    fn formats_gib_rounding_down() {
        assert_eq!(format_gib(0), "0.0 GiB");
        assert_eq!(format_gib(GIB + GIB / 2), "1.5 GiB");
        assert_eq!(format_gib(2 * GIB - 1), "1.9 GiB");
        assert_eq!(format_gib(u64::MAX), "17179869183.9 GiB");
    }

    #[test]
    fn wine_installed_row_names_the_newest() {
        let components = [
            component(ComponentKind::Wine, "11.17", &["wow64"]),
            component(ComponentKind::Wine, "11.0", &["wow64"]),
        ];
        let checks = evaluate(&mac(), &components, &[]);
        let wine = only(&checks, "wine-installed");
        assert_eq!(wine.status, Status::Ok);
        assert_eq!(wine.summary, "Wine 11.17 is installed (newest of 2)");

        let checks = evaluate(&mac(), &components[1..], &[]);
        assert_eq!(
            only(&checks, "wine-installed").summary,
            "Wine 11.0 is installed"
        );

        let checks = evaluate(&mac(), &[component(ComponentKind::Dxmt, "0.80", &[])], &[]);
        let none = only(&checks, "wine-installed");
        assert_eq!(none.status, Status::Fail);
        assert_eq!(none.fix.as_deref(), Some("uncork runtime install wine"));
    }

    #[test]
    fn wine_32bit_row_looks_only_at_the_newest() {
        let lacking = [component(ComponentKind::Wine, "11.17", &["msync"])];
        let checks = evaluate(&mac(), &lacking, &[]);
        let warn = only(&checks, "wine-32bit");
        assert_eq!(warn.status, Status::Warn);
        assert!(warn.summary.contains("11.17"), "{warn:?}");
        assert!(
            warn.summary.contains("32-bit games will not start"),
            "{warn:?}"
        );
        assert_eq!(warn.fix, None);

        let newest_has_it = [
            component(ComponentKind::Wine, "11.17", &["wow64"]),
            component(ComponentKind::Wine, "9.0", &[]),
        ];
        absent(&mac(), &newest_has_it, &[], "wine-32bit");
        absent(&mac(), &[], &[], "wine-32bit");
    }

    #[test]
    fn dxmt_row() {
        let ok = check(&mac(), "dxmt-installed");
        assert_eq!(
            (ok.status, ok.summary.as_str()),
            (Status::Ok, "DXMT 0.80 is installed")
        );
        let checks = evaluate(&mac(), &[], &[]);
        let missing = only(&checks, "dxmt-installed");
        assert_eq!(missing.status, Status::Warn);
        assert!(
            missing
                .summary
                .contains("D3D10/11 games fall back to OpenGL")
        );
        assert_eq!(missing.fix.as_deref(), Some("uncork runtime install dxmt"));
    }

    #[test]
    fn dxvk_row() {
        assert_eq!(check(&mac(), "dxvk-installed").status, Status::Ok);
        let checks = evaluate(&mac(), &[], &[]);
        let missing = only(&checks, "dxvk-installed");
        assert_eq!(missing.status, Status::Warn);
        assert!(
            missing
                .summary
                .contains("the Steam client's windows will be black without it")
        );
        assert_eq!(missing.fix.as_deref(), Some("uncork runtime install dxvk"));
    }

    #[test]
    fn d3dmetal_row() {
        let ok = check(&mac(), "d3dmetal");
        assert_eq!(
            (ok.status, ok.summary.as_str()),
            (Status::Ok, "D3DMetal 3.0 is imported")
        );
        let checks = evaluate(&mac(), &[], &[]);
        let missing = only(&checks, "d3dmetal");
        assert_eq!(missing.status, Status::Info);
        assert!(missing.summary.contains(
            "optional, 64-bit D3D11/D3D12; import your own copy of Apple's Game Porting Toolkit"
        ));
        assert_eq!(
            missing.fix.as_deref(),
            Some("uncork runtime import-gptk <path>")
        );
    }

    #[test]
    fn bottles_row() {
        let count =
            |bottles: &[Bottle]| only(&evaluate(&mac(), &full_set(), bottles), "bottles").clone();
        let none = count(&[]);
        assert_eq!(none.status, Status::Info);
        assert_eq!(none.fix.as_deref(), Some("uncork setup"));
        assert_eq!(count(&[bottle("a", "11.17")]).summary, "1 bottle");
        let two = count(&[bottle("a", "11.17"), bottle("b", "11.17")]);
        assert_eq!(
            (two.status, two.summary.as_str()),
            (Status::Ok, "2 bottles")
        );
    }

    #[test]
    fn bottle_wine_row_warns_once_per_bottle_with_a_missing_runtime() {
        let bottles = [bottle("steam", "11.17"), bottle("old", "9.0")];
        let checks = evaluate(&mac(), &full_set(), &bottles);
        let warn = only(&checks, "bottle-wine");
        assert_eq!(warn.status, Status::Warn);
        assert!(
            warn.summary.contains("\"old\"") && warn.summary.contains("9.0"),
            "{warn:?}"
        );
        assert_eq!(
            warn.fix.as_deref(),
            Some("uncork runtime install wine or uncork bottle set old wine=11.17")
        );

        // A version of another kind does not count as an installed Wine.
        let dxmt_only = [component(ComponentKind::Dxmt, "9.0", &[])];
        let checks = evaluate(&mac(), &dxmt_only, &bottles);
        let warnings: Vec<&Check> = checks.iter().filter(|c| c.id == "bottle-wine").collect();
        assert_eq!(warnings.len(), 2);
        assert_eq!(
            warnings[0].fix.as_deref(),
            Some("uncork runtime install wine or uncork bottle set steam wine=<version>")
        );
        absent(
            &mac(),
            &full_set(),
            &[bottle("steam", "11.17")],
            "bottle-wine",
        );
    }

    #[test]
    fn bottle_dpi_row_names_the_fix_that_matches_the_config() {
        let pair = |retina_mode, log_pixels| DisplayRegistry {
            retina_mode,
            log_pixels,
        };
        let steam = bottle("steam", "11.17");
        let mut retina = bottle("hires", "11.17");
        retina.config.performance.retina = true;

        // Seen on 2026-10-08: Uncork's RetinaMode n next to CrossOver's 192.
        let check = bottle_dpi(&steam, &pair(Some(false), Some(192))).unwrap();
        assert_eq!((check.id, check.status), ("bottle-dpi", Status::Warn));
        assert_eq!(
            check.summary,
            "bottle \"steam\": Retina mode and DPI disagree (RetinaMode n, LogPixels 192), so games that are not DPI-aware see a screen of the wrong size"
        );
        assert_eq!(
            check.fix.as_deref(),
            Some("uncork bottle set steam performance.retina=false")
        );
        let check = bottle_dpi(&retina, &pair(None, Some(192))).unwrap();
        assert!(check.summary.contains("disagree"), "{check:?}");
        assert_eq!(
            check.fix.as_deref(),
            Some("uncork bottle set hires performance.retina=true")
        );

        // A consistent pair that is not the bottle's.
        let check = bottle_dpi(&steam, &pair(Some(true), Some(192))).unwrap();
        assert_eq!(
            check.summary,
            "bottle \"steam\": its registry (RetinaMode y, LogPixels 192) does not match performance.retina = false in uncork.toml"
        );

        for (bottle, registry) in [
            (&steam, pair(Some(false), Some(96))),
            (&steam, pair(None, None)),
            (&steam, pair(Some(false), None)),
            (&retina, pair(Some(true), Some(192))),
            (&retina, pair(Some(true), None)),
        ] {
            assert_eq!(bottle_dpi(bottle, &registry), None, "{registry:?}");
        }
    }

    #[test]
    fn crossover_row() {
        let host = HostInfo {
            crossover_installed: true,
            ..mac()
        };
        let checks = evaluate(&host, &full_set(), &[]);
        let last = checks.last().unwrap();
        assert_eq!(last.id, "crossover");
        assert_eq!(last.status, Status::Info);
        assert!(
            last.summary
                .contains("bottles can be imported with `uncork bottle import`")
        );
        assert_eq!(last.fix, None);
        absent(&mac(), &full_set(), &[], "crossover");
    }

    #[test]
    fn statuses_order_by_severity_and_serialize_lowercase() {
        assert!(
            Status::Ok < Status::Info && Status::Info < Status::Warn && Status::Warn < Status::Fail
        );
        assert_eq!(serde_json::to_string(&Status::Warn).unwrap(), "\"warn\"");
        assert_eq!(
            serde_json::to_string(&Rosetta::NotNeeded).unwrap(),
            "\"not-needed\""
        );
    }

    #[test]
    fn macos_major_parses_the_first_component() {
        let with = |v: &str| {
            HostInfo {
                macos_version: Some(v.to_owned()),
                ..mac()
            }
            .macos_major()
        };
        assert_eq!(with("27.0.1"), Some(27));
        assert_eq!(with("14"), Some(14));
        assert_eq!(with("x.1"), None);
    }

    fn finished(code: i32, stderr: &str) -> io::Result<Output> {
        Ok(Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: Vec::new(),
            stderr: stderr.as_bytes().to_vec(),
        })
    }

    #[test]
    fn classifies_the_x86_64_trial_run() {
        assert_eq!(
            rosetta_from_x86_64_run(&finished(0, "")),
            Rosetta::Installed
        );
        assert_eq!(
            rosetta_from_x86_64_run(&finished(
                1,
                "arch: posix_spawnp: /usr/bin/true: Bad CPU type in executable\n"
            )),
            Rosetta::Missing
        );
        assert_eq!(
            rosetta_from_x86_64_run(&Err(io::Error::from_raw_os_error(EBADARCH))),
            Rosetta::Missing
        );
        assert_eq!(
            rosetta_from_x86_64_run(&finished(1, "arch: /usr/bin/true isn't executable\n")),
            Rosetta::Unknown
        );
        assert_eq!(
            rosetta_from_x86_64_run(&Err(io::Error::from_raw_os_error(2))),
            Rosetta::Unknown
        );
    }

    #[test]
    fn parses_df_output() {
        let typical = "Filesystem   1024-blocks       Used Available Capacity  Mounted on\n\
                       /dev/disk3s5  1948404040 1770483716 151119376    93%    /System/Volumes/Data\n";
        assert_eq!(parse_df_available_kib(typical), Some(151_119_376));

        let spaces = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                      map auto_home 0 0 0 100% /System/Volumes/Data/home\n";
        assert_eq!(parse_df_available_kib(spaces), Some(0));

        let mount_with_spaces = "Filesystem 1024-blocks Used Available Capacity Mounted on\n\
                                 /dev/disk5s1 1000 400 600 40% /Volumes/My Games 2\n";
        assert_eq!(parse_df_available_kib(mount_with_spaces), Some(600));

        for garbage in [
            "",
            "Filesystem 1024-blocks Used Available Capacity Mounted on\n",
            "a\nb c d e f\n",
        ] {
            assert_eq!(parse_df_available_kib(garbage), None, "{garbage:?}");
        }
    }

    #[test]
    fn probe_never_panics_and_reports_this_os() {
        let temp = tempfile::tempdir().unwrap();
        let host = HostInfo::probe(&temp.path().join("not").join("created"));
        assert_eq!(host.os, std::env::consts::OS);
        assert!(
            !temp.path().join("not").exists(),
            "probing must not create the data root"
        );
        if cfg!(target_os = "macos") {
            assert!(host.macos_major().is_some(), "{host:?}");
            assert!(host.free_bytes.is_some(), "{host:?}");
            assert_eq!(
                host.apple_silicon,
                host.rosetta != Rosetta::NotNeeded,
                "{host:?}"
            );
        } else {
            assert_eq!(host.rosetta, Rosetta::Unknown);
        }
    }

    #[test]
    fn free_bytes_falls_back_to_an_existing_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let direct = free_bytes(temp.path());
        let nested = free_bytes(&temp.path().join("a").join("b"));
        if cfg!(target_os = "macos") {
            assert!(direct.is_some());
            assert!(nested.is_some());
        }
    }
}
