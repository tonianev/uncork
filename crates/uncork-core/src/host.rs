//! Facts about the Mac Uncork runs on, and the `doctor` checks built on them.
//!
//! Probing ([`HostInfo::probe`]) runs a few read-only system commands.
//! Evaluation ([`evaluate`]) is a pure function over the probed facts, so
//! every check is unit-tested with hand-written [`HostInfo`] values.

use serde::Serialize;

use crate::bottle::Bottle;
use crate::component::InstalledComponent;

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
    #[must_use]
    pub fn probe(data_root: &std::path::Path) -> HostInfo {
        let _ = data_root;
        todo!()
    }

    /// Major macOS version (`27` for `27.0.1`).
    #[must_use]
    pub fn macos_major(&self) -> Option<u32> {
        self.macos_version.as_deref()?.split('.').next()?.parse().ok()
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
/// | `rosetta` | Ok: Installed or NotNeeded. Fail: Missing. Warn: Unknown | `softwareupdate --install-rosetta --agree-to-license` |
/// | `rosetta-sunset` | Info when major ≥ 27: "macOS 27 is the last release with general-purpose Rosetta; from macOS 28 Apple keeps only a subset for older games, and Wine-based tools may break" (`docs/ROADMAP.md`). Omitted below 27 | — |
/// | `disk-space` | Ok: free ≥ [`LOW_DISK_BYTES`]. Warn: below. Omitted when unknown | — |
/// | `wine-installed` | Ok: a wine component (names the newest). Fail: none | `uncork runtime install wine` |
/// | `wine-32bit` | Warn: newest wine lacks the `wow64` feature ("32-bit games will not start"). Omitted otherwise or without wine | — |
/// | `dxmt-installed` | Ok / Warn: missing ("D3D10/11 games fall back to OpenGL") | `uncork runtime install dxmt` |
/// | `dxvk-installed` | Ok / Info: missing (optional fallback) | `uncork runtime install dxvk` |
/// | `d3dmetal` | Ok: imported (version). Info: not imported ("optional, 64-bit D3D11/D3D12; import your own copy of Apple's Game Porting Toolkit") | `uncork runtime import-gptk <path>` |
/// | `bottles` | Ok: "N bottles". Info: none | `uncork setup` |
/// | `bottle-wine` | Warn, once per bottle whose `wine` version is not installed | `uncork runtime install wine` or `uncork bottle set <name> wine=<v>` |
/// | `crossover` | Info when CrossOver is installed: "bottles can be imported with `uncork bottle import`". Omitted otherwise | — |
#[must_use]
pub fn evaluate(host: &HostInfo, components: &[InstalledComponent], bottles: &[Bottle]) -> Vec<Check> {
    let _ = (host, components, bottles);
    todo!()
}
