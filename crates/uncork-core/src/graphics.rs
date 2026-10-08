//! Graphics backends: which Direct3D translation layer a process uses, and
//! how Uncork makes Wine load it.
//!
//! # Backends
//!
//! | Backend | Translates | Bitness | Source |
//! |---|---|---|---|
//! | [`Backend::Dxmt`] | D3D10, D3D11 → Metal | 32 and 64 | `dxmt` component |
//! | [`Backend::Dxvk`] | D3D10, D3D11 → Vulkan → MoltenVK → Metal | 32 and 64 | `dxvk` component |
//! | [`Backend::D3dmetal`] | D3D11, D3D12 (D3D10 from GPTK 4) → Metal | 64 only | user-imported `d3dmetal` component |
//! | [`Backend::Wined3d`] | DirectDraw, D3D8, D3D9, D3D10, D3D11 → OpenGL | 32 and 64 | built into Wine |
//!
//! The decision table ([`recommend`]) is in `docs/ARCHITECTURE.md`
//! ("Backend decision matrix") with sources. In short: DXMT for D3D10/11 at
//! either bitness, D3DMetal only for 64-bit D3D12, WineD3D for D3D9 and
//! older, DXVK as a fallback that cannot run games that need geometry
//! shaders (MoltenVK has none).
//!
//! # Activation strategies
//!
//! Backends are chosen **per process** so the Steam client (which must stay
//! on WineD3D; DXMT cannot present across processes) and a game can run in
//! the same prefix at the same time. How depends on what the Wine runtime
//! supports, declared as component features in the catalog:
//!
//! - [`Strategy::RendererEnv`] (feature `renderer-dllpath`, Gcenx's
//!   Sikarugir engines): set `WINEDLLPATH_DXMT`, `WINEDLLPATH_DXVK` or
//!   `WINEDLLPATH_D3DMETAL` to the backend's directory. Wine searches it
//!   before its own builtins for that process only.
//! - [`Strategy::DllPathPrepend`] (feature `dllpath-prepend`): set
//!   `WINEDLLPATH_PREPEND` to the backend's directory.
//! - [`Strategy::PrefixNative`] (any other runtime): copy the backend's DLLs,
//!   with Wine's builtin marker removed, into `system32`/`syswow64`, and set
//!   `WINEDLLOVERRIDES=<dlls>=n,b` for the processes that should use them.
//!   Every other process gets `<dlls>=b` explicitly so a native DLL left in
//!   `system32` never leaks into the Steam client.
//!
//! In every strategy DXMT's `winemetal.dll` is also copied (marker intact)
//! into `system32` and `syswow64`, as DXMT's installation guide requires.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uncork_pe::{Bitness, GraphicsApi};

use crate::bottle::{ActiveBackend, Bottle, InstalledDll};
use crate::component::{ComponentKind, InstalledComponent};
use crate::wine::WineRuntime;

/// A Direct3D translation layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    /// Apple D3DMetal (Game Porting Toolkit).
    D3dmetal,
    /// DXMT.
    Dxmt,
    /// DXVK on MoltenVK.
    Dxvk,
    /// Wine's built-in WineD3D.
    Wined3d,
}

impl Backend {
    /// Every backend, in display order.
    pub const ALL: [Backend; 4] = [
        Backend::Dxmt,
        Backend::D3dmetal,
        Backend::Dxvk,
        Backend::Wined3d,
    ];

    /// Lowercase name used in TOML and the CLI.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::D3dmetal => "d3dmetal",
            Backend::Dxmt => "dxmt",
            Backend::Dxvk => "dxvk",
            Backend::Wined3d => "wined3d",
        }
    }

    /// The component that provides it (`None` for WineD3D).
    #[must_use]
    pub fn component_kind(self) -> Option<ComponentKind> {
        match self {
            Backend::D3dmetal => Some(ComponentKind::D3dmetal),
            Backend::Dxmt => Some(ComponentKind::Dxmt),
            Backend::Dxvk => Some(ComponentKind::Dxvk),
            Backend::Wined3d => None,
        }
    }
}

impl fmt::Display for Backend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Backend {
    type Err = String;

    /// Case-insensitive; also accepts `d3d-metal`, `gptk`, `wine-d3d`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "d3dmetal" | "d3d-metal" | "gptk" => Ok(Backend::D3dmetal),
            "dxmt" => Ok(Backend::Dxmt),
            "dxvk" => Ok(Backend::Dxvk),
            "wined3d" | "wine-d3d" => Ok(Backend::Wined3d),
            _ => Err(format!(
                "unknown graphics backend {s:?} (expected dxmt, d3dmetal, dxvk or wined3d)"
            )),
        }
    }
}

impl Backend {
    /// The project's own spelling, for messages.
    fn display_name(self) -> &'static str {
        match self {
            Backend::D3dmetal => "D3DMetal",
            Backend::Dxmt => "DXMT",
            Backend::Dxvk => "DXVK",
            Backend::Wined3d => "WineD3D",
        }
    }
}

/// A bottle-level or command-line choice: a fixed backend or automatic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BackendChoice {
    /// Pick per program with [`recommend`].
    #[default]
    Auto,
    /// Always this backend.
    Fixed(Backend),
}

impl Serialize for BackendChoice {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            BackendChoice::Auto => serializer.serialize_str("auto"),
            BackendChoice::Fixed(b) => serializer.serialize_str(b.as_str()),
        }
    }
}

impl<'de> Deserialize<'de> for BackendChoice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        if s.eq_ignore_ascii_case("auto") {
            Ok(BackendChoice::Auto)
        } else {
            s.parse()
                .map(BackendChoice::Fixed)
                .map_err(serde::de::Error::custom)
        }
    }
}

/// Can `backend` run a program with this API and bitness at all?
///
/// - D3DMetal: 64-bit only; D3D10 (GPTK ≥ 4 only — treated as supported),
///   D3D11, D3D12.
/// - DXMT: D3D10, D3D11; 32 and 64-bit.
/// - DXVK: D3D10, D3D11 (the macOS fork's D3D9 cannot create a device on MoltenVK).
/// - WineD3D: DirectDraw, D3D8, D3D9, D3D10, D3D11.
/// - OpenGL and Vulkan programs do not use a Direct3D backend: every backend
///   "supports" them (the choice is irrelevant) and [`recommend`] returns WineD3D.
#[must_use]
pub fn supports(backend: Backend, api: GraphicsApi, bitness: Bitness) -> bool {
    use GraphicsApi as Api;
    match (backend, api) {
        (Backend::D3dmetal, Api::D3d10 | Api::D3d11 | Api::D3d12) => bitness == Bitness::X64,
        (_, Api::OpenGl | Api::Vulkan)
        | (Backend::Dxmt | Backend::Dxvk, Api::D3d10 | Api::D3d11)
        | (Backend::Wined3d, Api::DirectDraw | Api::D3d8 | Api::D3d9 | Api::D3d10 | Api::D3d11) => {
            true
        }
        _ => false,
    }
}

/// What is installed, for [`recommend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Availability {
    /// A `dxmt` component is installed and the Wine runtime can load it
    /// ([`runtime_blocker`]: the `dxmt` feature, and `winemetal.so` when
    /// the DLLs are copied into the prefix).
    pub dxmt: bool,
    /// A `dxvk` component is installed.
    pub dxvk: bool,
    /// A `d3dmetal` component is installed and the Wine runtime can load it
    /// ([`runtime_blocker`]: the `d3dmetal` feature and per-process loading).
    pub d3dmetal: bool,
}

impl Availability {
    /// `true` if `backend` can be used (WineD3D always can).
    #[must_use]
    pub fn has(&self, backend: Backend) -> bool {
        match backend {
            Backend::Dxmt => self.dxmt,
            Backend::Dxvk => self.dxvk,
            Backend::D3dmetal => self.d3dmetal,
            Backend::Wined3d => true,
        }
    }
}

/// Result of [`recommend`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Recommendation {
    /// Backend to use.
    pub backend: Backend,
    /// The full preference order that was considered, best first, including
    /// unavailable entries (for `uncork inspect`).
    pub order: Vec<Backend>,
    /// One sentence explaining the choice, e.g. "32-bit D3D11: DXMT (D3DMetal is 64-bit only)".
    pub reason: String,
}

/// Choose a backend.
///
/// Preference order (first available and [`supports`]-compatible wins):
///
/// | API | 32-bit | 64-bit |
/// |---|---|---|
/// | none detected, OpenGL, Vulkan, DirectDraw, D3D8, D3D9 | WineD3D | WineD3D |
/// | D3D10, D3D11 | DXMT, WineD3D, DXVK | DXMT, D3DMetal, DXVK, WineD3D |
/// | D3D12 | (unsupported: WineD3D, reason says it will fail) | D3DMetal, WineD3D |
///
/// `preferred` (from a profile or the bottle) is tried first when it is
/// available and compatible; then `fallbacks` in order; then the table.
/// `needs_geometry_shaders` removes DXVK from consideration.
#[must_use]
pub fn recommend(
    api: Option<GraphicsApi>,
    bitness: Bitness,
    available: &Availability,
    preferred: Option<Backend>,
    fallbacks: &[Backend],
    needs_geometry_shaders: bool,
) -> Recommendation {
    let program = describe_program(api, bitness);
    if let Some(api @ (GraphicsApi::OpenGl | GraphicsApi::Vulkan)) = api {
        return Recommendation {
            backend: Backend::Wined3d,
            order: vec![Backend::Wined3d],
            reason: format!(
                "{program}: WineD3D ({} needs no Direct3D translation)",
                api_name(api)
            ),
        };
    }

    let candidates = rank(
        api,
        bitness,
        *available,
        preferred,
        fallbacks,
        needs_geometry_shaders,
    );
    let chosen = candidates
        .iter()
        .position(|(_, verdict)| *verdict == Verdict::Usable);
    // With nothing usable, WineD3D is the last resort even though it cannot
    // run the program (32-bit D3D12, or 64-bit D3D12 without D3DMetal).
    let backend = chosen.map_or(Backend::Wined3d, |index| candidates[index].0);

    let mut order: Vec<Backend> = candidates
        .iter()
        .filter(|(_, verdict)| !matches!(verdict, Verdict::Excluded(_)))
        .map(|(candidate, _)| *candidate)
        .collect();
    if !order.contains(&backend) {
        order.push(backend);
    }

    let origin = match chosen {
        Some(_) if preferred == Some(backend) => Some("preferred".to_owned()),
        Some(_) if fallbacks.contains(&backend) => Some("fallback".to_owned()),
        _ => None,
    };
    let passed_over = candidates[..chosen.unwrap_or(candidates.len())]
        .iter()
        .filter(|(candidate, _)| *candidate != backend)
        .filter_map(|(candidate, verdict)| verdict.note(*candidate));
    let mut notes: Vec<String> = Vec::new();
    for note in origin
        .into_iter()
        .chain(passed_over)
        .chain(table_note(api, bitness))
    {
        if !notes.contains(&note) {
            notes.push(note);
        }
    }

    let outcome = if chosen.is_some() {
        ""
    } else {
        ", which will fail"
    };
    let details = if notes.is_empty() {
        String::new()
    } else {
        format!(" ({})", notes.join("; "))
    };
    Recommendation {
        backend,
        order,
        reason: format!("{program}: {}{outcome}{details}", backend.display_name()),
    }
}

/// Every backend worth considering, best first and without repeats: the
/// preference, the fallbacks, then the table row; each with its verdict.
fn rank(
    api: Option<GraphicsApi>,
    bitness: Bitness,
    available: Availability,
    preferred: Option<Backend>,
    fallbacks: &[Backend],
    needs_geometry_shaders: bool,
) -> Vec<(Backend, Verdict)> {
    let mut ranked: Vec<(Backend, Verdict)> = Vec::new();
    let table = table_order(api, bitness);
    for backend in preferred
        .into_iter()
        .chain(fallbacks.iter().copied())
        .chain(table.iter().copied())
    {
        if ranked.iter().all(|(seen, _)| *seen != backend) {
            let verdict = assess(backend, api, bitness, available, needs_geometry_shaders);
            ranked.push((backend, verdict));
        }
    }
    ranked
}

/// How [`recommend`] judges one candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Verdict {
    /// Installed and able to run the program.
    Usable,
    /// Able to run the program but not installed.
    Unavailable,
    /// Cannot run the program; the text says why.
    Excluded(String),
}

impl Verdict {
    /// Why `backend` was passed over, if it was.
    fn note(&self, backend: Backend) -> Option<String> {
        match self {
            Verdict::Usable => None,
            Verdict::Unavailable => Some(format!("{} not available", backend.display_name())),
            Verdict::Excluded(why) => Some(why.clone()),
        }
    }
}

fn assess(
    backend: Backend,
    api: Option<GraphicsApi>,
    bitness: Bitness,
    available: Availability,
    needs_geometry_shaders: bool,
) -> Verdict {
    if needs_geometry_shaders && backend == Backend::Dxvk {
        return Verdict::Excluded("DXVK cannot run games that need geometry shaders".to_owned());
    }
    if backend == Backend::D3dmetal && bitness == Bitness::X86 {
        return Verdict::Excluded(D3DMETAL_64_BIT_ONLY.to_owned());
    }
    // With no API detected only bitness can rule a backend out; a profile's
    // preference is trusted.
    if let Some(api) = api
        && !supports(backend, api, bitness)
    {
        return Verdict::Excluded(format!(
            "{} cannot run {}",
            backend.display_name(),
            api_name(api)
        ));
    }
    if available.has(backend) {
        Verdict::Usable
    } else {
        Verdict::Unavailable
    }
}

const D3DMETAL_64_BIT_ONLY: &str = "D3DMetal is 64-bit only";

/// The preference table from [`recommend`]'s documentation.
fn table_order(api: Option<GraphicsApi>, bitness: Bitness) -> &'static [Backend] {
    use GraphicsApi as Api;
    match (api, bitness) {
        (Some(Api::D3d10 | Api::D3d11), Bitness::X86) => {
            &[Backend::Dxmt, Backend::Wined3d, Backend::Dxvk]
        }
        (Some(Api::D3d10 | Api::D3d11), Bitness::X64) => &[
            Backend::Dxmt,
            Backend::D3dmetal,
            Backend::Dxvk,
            Backend::Wined3d,
        ],
        (Some(Api::D3d12), Bitness::X64) => &[Backend::D3dmetal, Backend::Wined3d],
        _ => &[Backend::Wined3d],
    }
}

/// What the table row itself implies, for the reason sentence.
fn table_note(api: Option<GraphicsApi>, bitness: Bitness) -> Option<String> {
    use GraphicsApi as Api;
    match (api?, bitness) {
        (Api::D3d10 | Api::D3d11, Bitness::X86) => Some(D3DMETAL_64_BIT_ONLY.to_owned()),
        (Api::D3d10 | Api::D3d11 | Api::OpenGl | Api::Vulkan, _) => None,
        (Api::D3d12, Bitness::X86) => {
            Some("D3DMetal, the only D3D12 backend, is 64-bit only".to_owned())
        }
        (Api::D3d12, Bitness::X64) => Some("only D3DMetal runs D3D12".to_owned()),
        (legacy @ (Api::DirectDraw | Api::D3d8 | Api::D3d9), _) => {
            Some(format!("only WineD3D runs {}", api_name(legacy)))
        }
    }
}

/// "32-bit D3D11" or "64-bit, no Direct3D detected".
pub(crate) fn describe_program(api: Option<GraphicsApi>, bitness: Bitness) -> String {
    let bits = match bitness {
        Bitness::X86 => "32-bit",
        Bitness::X64 => "64-bit",
    };
    match api {
        Some(api) => format!("{bits} {}", api_name(api)),
        None => format!("{bits}, no Direct3D detected"),
    }
}

pub(crate) fn api_name(api: GraphicsApi) -> &'static str {
    match api {
        GraphicsApi::DirectDraw => "DirectDraw",
        GraphicsApi::OpenGl => "OpenGL",
        GraphicsApi::D3d8 => "D3D8",
        GraphicsApi::D3d9 => "D3D9",
        GraphicsApi::D3d10 => "D3D10",
        GraphicsApi::D3d11 => "D3D11",
        GraphicsApi::D3d12 => "D3D12",
        GraphicsApi::Vulkan => "Vulkan",
    }
}

/// How a backend is wired into Wine for one process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Strategy {
    /// Set `var` (e.g. `WINEDLLPATH_DXMT`) to the backend directory.
    RendererEnv {
        /// Variable name.
        var: String,
    },
    /// Set `WINEDLLPATH_PREPEND` to the backend directory.
    DllPathPrepend,
    /// Copy marker-stripped DLLs into the prefix and use `n,b` overrides.
    PrefixNative,
    /// Nothing to do (WineD3D).
    Builtin,
}

/// Pick the strategy for `backend` on `wine`:
/// WineD3D → `Builtin`; runtime feature `renderer-dllpath` →
/// `RendererEnv { var: "WINEDLLPATH_<DXMT|DXVK|D3DMETAL>" }`; feature
/// `dllpath-prepend` → `DllPathPrepend`; otherwise `PrefixNative`
/// (not allowed for D3DMetal: returns [`crate::Error::Unsupported`] because
/// stripping the marker loses its Unix half).
///
/// # Errors
/// [`crate::Error::Unsupported`] as above.
pub fn strategy_for(backend: Backend, wine: &WineRuntime) -> crate::Result<Strategy> {
    if backend == Backend::Wined3d {
        Ok(Strategy::Builtin)
    } else if wine.has_feature(FEATURE_RENDERER_DLLPATH) {
        Ok(Strategy::RendererEnv {
            var: format!("WINEDLLPATH_{}", backend.as_str().to_ascii_uppercase()),
        })
    } else if wine.has_feature(FEATURE_DLLPATH_PREPEND) {
        Ok(Strategy::DllPathPrepend)
    } else if backend == Backend::D3dmetal {
        Err(crate::Error::Unsupported(format!(
            "D3DMetal needs a Wine runtime that can load it per process (feature \
             {FEATURE_RENDERER_DLLPATH} or {FEATURE_DLLPATH_PREPEND}); Wine {} has neither, and copying \
             D3DMetal into the prefix would lose its Unix half",
            wine.version
        )))
    } else {
        Ok(Strategy::PrefixNative)
    }
}

/// The runtime feature `backend` needs besides its component: `dxmt`
/// (winemac exports the Metal view API DXMT needs) for DXMT, `d3dmetal`
/// (CrossOver-derived glue) for D3DMetal, none for DXVK and WineD3D.
#[must_use]
pub fn required_feature(backend: Backend) -> Option<&'static str> {
    match backend {
        Backend::Dxmt => Some("dxmt"),
        Backend::D3dmetal => Some("d3dmetal"),
        Backend::Dxvk | Backend::Wined3d => None,
    }
}

/// Why `wine` cannot run `backend` even with the backend's component
/// installed, with the fix; `None` when it can. Everything [`activation`]
/// checks about the runtime:
///
/// - [`runtime_feature_blocker`]: the [`required_feature`], and a way to
///   load the backend ([`strategy_for`]; D3DMetal needs per-process
///   loading);
/// - DXMT copied into the prefix ([`Strategy::PrefixNative`]) needs the
///   runtime's own `lib/wine/x86_64-unix/winemetal.so`.
#[must_use]
pub fn runtime_blocker(backend: Backend, wine: &WineRuntime) -> Option<String> {
    runtime_feature_blocker(backend, wine).or_else(|| {
        let native = matches!(strategy_for(backend, wine), Ok(Strategy::PrefixNative));
        (backend == Backend::Dxmt && native)
            .then(|| require_winemetal_bridge(wine).err())
            .flatten()
            .map(|err| err.to_string())
    })
}

/// The part of [`runtime_blocker`] that only needs the runtime's declared
/// features (for judging a runtime that is not installed yet).
#[must_use]
pub fn runtime_feature_blocker(backend: Backend, wine: &WineRuntime) -> Option<String> {
    let name = backend.display_name();
    if let Some(feature) = required_feature(backend)
        && !wine.has_feature(feature)
    {
        return Some(format!(
            "Wine {} cannot load {name} (the runtime lacks the `{feature}` feature); use a Wine runtime that has it (`uncork runtime available` lists them)",
            wine.version
        ));
    }
    strategy_for(backend, wine).err().map(|err| {
        format!("{err}; use a Wine runtime that has one (`uncork runtime available` lists them)")
    })
}

/// Runtime feature: honors `WINEDLLPATH_DXMT`/`_DXVK`/`_D3DMETAL`.
const FEATURE_RENDERER_DLLPATH: &str = "renderer-dllpath";
/// Runtime feature: honors `WINEDLLPATH_PREPEND`.
const FEATURE_DLLPATH_PREPEND: &str = "dllpath-prepend";

/// Where Wine expects each architecture inside a backend directory (the
/// same layout as Wine's own `lib/wine`).
pub const ARCH_DIRS: [&str; 3] = ["x86_64-windows", "i386-windows", "x86_64-unix"];

/// The DLL names a backend replaces (lowercase, without `.dll`). These are
/// the keys of its `WINEDLLOVERRIDES` entry.
///
/// - DXMT: `d3d11`, `dxgi`, `d3d10core`
/// - DXVK: `d3d11`, `d3d10core` (Wine's own `dxgi` stays; a native DXMT
///   `dxgi` breaks DXVK)
/// - D3DMetal: `d3d11`, `dxgi`, `d3d12`, `d3d12core`, `d3d10`, `d3d10core`
///   (only those present in the component are used)
/// - WineD3D: none
#[must_use]
pub fn replaced_dlls(backend: Backend) -> &'static [&'static str] {
    match backend {
        Backend::Dxmt => &["d3d11", "dxgi", "d3d10core"],
        Backend::Dxvk => &["d3d11", "d3d10core"],
        Backend::D3dmetal => &["d3d11", "dxgi", "d3d12", "d3d12core", "d3d10", "d3d10core"],
        Backend::Wined3d => &[],
    }
}

/// The directory to pass for `RendererEnv`/`DllPathPrepend`: the component
/// directory itself for DXMT and DXVK (it contains [`ARCH_DIRS`]); `wine/`
/// inside the component for D3DMetal (GPTK's `redist/lib/wine`).
#[must_use]
pub fn backend_dir(backend: Backend, component: &InstalledComponent) -> PathBuf {
    match backend {
        Backend::D3dmetal => component.path.join("wine"),
        Backend::Dxmt | Backend::Dxvk | Backend::Wined3d => component.path.clone(),
    }
}

/// One file to put into the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DllCopy {
    /// Source file inside the component.
    pub source: PathBuf,
    /// Destination relative to the bottle, e.g. `drive_c/windows/syswow64/d3d11.dll`.
    pub dest: PathBuf,
    /// Remove Wine's builtin marker while copying ([`Strategy::PrefixNative`] backend DLLs).
    pub strip_builtin_marker: bool,
}

/// Everything needed to run one process with `backend`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Activation {
    /// Backend.
    pub backend: Backend,
    /// Component version used (`None` for WineD3D).
    pub version: Option<String>,
    /// Strategy chosen.
    pub strategy: Strategy,
    /// Files to copy into the prefix before launch (idempotent; skipped
    /// when the destination already has the same SHA-256).
    pub copies: Vec<DllCopy>,
    /// Environment variables for the process.
    pub env: BTreeMap<String, String>,
    /// DLL overrides for the process (`name` → `n,b` / `b`).
    pub overrides: BTreeMap<String, String>,
}

/// Options that change a backend's environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BackendOptions {
    /// Upscale with MetalFX: `DXMT_METALFX_SPATIAL_SWAPCHAIN=1` (DXMT) or
    /// `D3DM_ENABLE_METALFX=1` (D3DMetal).
    pub metalfx: bool,
    /// Show the Metal HUD (`MTL_HUD_ENABLED=1`).
    pub hud: bool,
    /// Keep backend logging on (`DXMT_LOG_LEVEL=info`, `DXVK_LOG_LEVEL=info`)
    /// instead of `none`.
    pub debug_logs: bool,
}

/// Build the activation for `backend` in `bottle`.
///
/// Environment per backend (always, unless noted):
///
/// - DXMT: `DXMT_LOG_LEVEL=none`, `DXMT_SHADER_CACHE_PATH=<bottle>/cache/dxmt`,
///   plus `DXMT_METALFX_SPATIAL_SWAPCHAIN=1` with MetalFX. `winemetal.dll`
///   copies into `system32` (from `x86_64-windows`) and `syswow64` (from
///   `i386-windows`).
/// - DXVK: `DXVK_LOG_LEVEL=none`, `DXVK_ASYNC=1`,
///   `DXVK_STATE_CACHE_PATH=<bottle>/cache/dxvk`, `MVK_CONFIG_RESUME_LOST_DEVICE=1`.
/// - D3DMetal: `CX_APPLEGPTK_LIBD3DSHARED_PATH=<component>/external/libd3dshared.dylib`,
///   plus `D3DM_ENABLE_METALFX=1` with MetalFX.
/// - WineD3D: `WINE_D3D_CONFIG=renderer=gl`.
/// - Any backend with `hud`: `MTL_HUD_ENABLED=1`.
///
/// Overrides: with `PrefixNative`, `replaced_dlls(backend)` = `n,b`; in every
/// strategy, the DLLs *other* backends replace (and this one does not) are
/// set to `b`, so leftovers in `system32` are never loaded.
///
/// # Errors
/// [`crate::Error::Unsupported`] from [`strategy_for`];
/// [`crate::Error::BrokenComponent`] if a required file is missing from the component.
pub fn activation(
    backend: Backend,
    component: Option<&InstalledComponent>,
    wine: &WineRuntime,
    bottle: &Bottle,
    options: BackendOptions,
) -> crate::Result<Activation> {
    let strategy = strategy_for(backend, wine)?;
    let component = match backend.component_kind() {
        Some(kind) => Some(checked_component(backend, kind, component)?),
        None => None,
    };
    let mut copies = Vec::new();
    let mut env = backend_env(backend, bottle, options);
    let mut overrides = foreign_overrides(backend);

    if let Some(component) = component {
        match backend {
            Backend::Dxmt => copies.extend(winemetal_copies(component)?),
            Backend::D3dmetal => {
                env.insert(
                    "CX_APPLEGPTK_LIBD3DSHARED_PATH".to_owned(),
                    path_value(&libd3dshared(component)?),
                );
            }
            Backend::Dxvk | Backend::Wined3d => {}
        }
        match &strategy {
            Strategy::RendererEnv { var } => {
                env.insert(
                    var.clone(),
                    path_value(&checked_backend_dir(backend, component)?),
                );
            }
            Strategy::DllPathPrepend => {
                env.insert(
                    "WINEDLLPATH_PREPEND".to_owned(),
                    path_value(&checked_backend_dir(backend, component)?),
                );
            }
            Strategy::PrefixNative => {
                if backend == Backend::Dxmt {
                    require_winemetal_bridge(wine)?;
                }
                copies.extend(native_copies(backend, component)?);
                overrides.extend(
                    replaced_dlls(backend)
                        .iter()
                        .map(|dll| ((*dll).to_owned(), "n,b".to_owned())),
                );
            }
            Strategy::Builtin => {}
        }
    }

    Ok(Activation {
        backend,
        version: component.map(|component| component.meta.version.clone()),
        strategy,
        copies,
        env,
        overrides,
    })
}

/// `component`, checked to be present and of the kind `backend` needs.
fn checked_component(
    backend: Backend,
    kind: ComponentKind,
    component: Option<&InstalledComponent>,
) -> crate::Result<&InstalledComponent> {
    let Some(component) = component else {
        let fix = match backend {
            Backend::D3dmetal => {
                "import it from Apple's Game Porting Toolkit with `uncork runtime import-gptk`"
                    .to_owned()
            }
            _ => format!("install it with `uncork runtime install {backend}`"),
        };
        return Err(crate::Error::Unsupported(format!(
            "{} is not installed; {fix}",
            backend.display_name()
        )));
    };
    if component.meta.kind != kind {
        return Err(broken(component, format!("expected a {kind} component")));
    }
    Ok(component)
}

fn broken(component: &InstalledComponent, message: impl Into<String>) -> crate::Error {
    crate::Error::BrokenComponent {
        kind: component.meta.kind,
        path: component.path.clone(),
        message: message.into(),
    }
}

/// [`backend_dir`], checked to have Wine's architecture layout.
fn checked_backend_dir(backend: Backend, component: &InstalledComponent) -> crate::Result<PathBuf> {
    let dir = backend_dir(backend, component);
    if is_backend_dir(&dir) {
        Ok(dir)
    } else {
        Err(broken(
            component,
            format!("{} has none of {}", dir.display(), ARCH_DIRS.join(", ")),
        ))
    }
}

/// Environment of `backend` itself (the strategy adds its DLL path).
fn backend_env(
    backend: Backend,
    bottle: &Bottle,
    options: BackendOptions,
) -> BTreeMap<String, String> {
    let log_level = if options.debug_logs { "info" } else { "none" };
    let cache_dir = |name: &str| path_value(&bottle.path.join("cache").join(name));
    let mut env = BTreeMap::new();
    let mut set = |key: &str, value: String| {
        env.insert(key.to_owned(), value);
    };
    match backend {
        Backend::Dxmt => {
            set("DXMT_LOG_LEVEL", log_level.to_owned());
            set("DXMT_SHADER_CACHE_PATH", cache_dir("dxmt"));
            if options.metalfx {
                set("DXMT_METALFX_SPATIAL_SWAPCHAIN", "1".to_owned());
            }
        }
        Backend::Dxvk => {
            set("DXVK_LOG_LEVEL", log_level.to_owned());
            set("DXVK_ASYNC", "1".to_owned());
            set("DXVK_STATE_CACHE_PATH", cache_dir("dxvk"));
            set("MVK_CONFIG_RESUME_LOST_DEVICE", "1".to_owned());
        }
        Backend::D3dmetal => {
            if options.metalfx {
                set("D3DM_ENABLE_METALFX", "1".to_owned());
            }
        }
        Backend::Wined3d => set("WINE_D3D_CONFIG", "renderer=gl".to_owned()),
    }
    if options.hud {
        set("MTL_HUD_ENABLED", "1".to_owned());
    }
    env
}

/// `b` for every DLL another backend replaces and `backend` does not, so a
/// native copy left in `system32` by an earlier activation is never loaded.
fn foreign_overrides(backend: Backend) -> BTreeMap<String, String> {
    let own = replaced_dlls(backend);
    Backend::ALL
        .iter()
        .flat_map(|other| replaced_dlls(*other))
        .filter(|dll| !own.contains(dll))
        .map(|dll| ((*dll).to_owned(), "b".to_owned()))
        .collect()
}

/// Where each Windows architecture of a backend directory goes in a prefix.
const PREFIX_DIRS: [(&str, &str); 2] =
    [("x86_64-windows", "system32"), ("i386-windows", "syswow64")];

/// Copies of `file` from every Windows architecture of `dir` that has it.
fn arch_copies(dir: &Path, file: &str, strip_builtin_marker: bool) -> Vec<DllCopy> {
    PREFIX_DIRS
        .iter()
        .filter_map(|(arch, system_dir)| {
            let source = dir.join(arch).join(file);
            source.is_file().then(|| DllCopy {
                source,
                dest: Path::new("drive_c")
                    .join("windows")
                    .join(system_dir)
                    .join(file),
                strip_builtin_marker,
            })
        })
        .collect()
}

/// DXMT's `winemetal.dll`, marker intact. The 64-bit one is required; the
/// 32-bit one only exists in builds with 32-bit support.
fn winemetal_copies(component: &InstalledComponent) -> crate::Result<Vec<DllCopy>> {
    const WINEMETAL: &str = "winemetal.dll";
    if !component
        .path
        .join("x86_64-windows")
        .join(WINEMETAL)
        .is_file()
    {
        return Err(broken(
            component,
            format!("x86_64-windows/{WINEMETAL} is missing"),
        ));
    }
    Ok(arch_copies(&component.path, WINEMETAL, false))
}

/// D3DMetal's `external/libd3dshared.dylib`, which its Wine-side DLLs load.
fn libd3dshared(component: &InstalledComponent) -> crate::Result<PathBuf> {
    let path = component.path.join("external").join("libd3dshared.dylib");
    if path.is_file() {
        Ok(path)
    } else {
        Err(broken(component, "external/libd3dshared.dylib is missing"))
    }
}

/// Marker-stripped copies of every DLL `backend` replaces.
fn native_copies(backend: Backend, component: &InstalledComponent) -> crate::Result<Vec<DllCopy>> {
    let dir = backend_dir(backend, component);
    let mut copies = Vec::new();
    for dll in replaced_dlls(backend) {
        let file = format!("{dll}.dll");
        let found = arch_copies(&dir, &file, true);
        if found.is_empty() {
            return Err(broken(
                component,
                format!("{file} is in neither x86_64-windows nor i386-windows"),
            ));
        }
        copies.extend(found);
    }
    Ok(copies)
}

/// With [`Strategy::PrefixNative`], DXMT's Unix half (`winemetal.so`) is
/// not on any per-process path, so it must already be in the runtime's own
/// `lib/wine/x86_64-unix` (CrossOver-derived runtimes ship it there).
fn require_winemetal_bridge(wine: &WineRuntime) -> crate::Result<()> {
    let bridge = wine
        .root
        .join("lib")
        .join("wine")
        .join("x86_64-unix")
        .join("winemetal.so");
    if bridge.is_file() {
        Ok(())
    } else {
        Err(crate::Error::Unsupported(format!(
            "Wine {} has no DXMT bridge ({} is missing); use a Wine runtime with the `dxmt` feature",
            wine.version,
            bridge.display()
        )))
    }
}

/// A path as an environment value.
fn path_value(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Perform `activation.copies` (creating directories, skipping identical
/// files, stripping markers with [`uncork_pe::strip_builtin_marker`]) and
/// record them in the bottle state. Never deletes Wine placeholder DLLs.
///
/// # Errors
/// [`crate::Error::Io`] or [`crate::Error::Pe`].
pub fn apply_copies(activation: &Activation, bottle: &mut Bottle) -> crate::Result<()> {
    if activation.copies.is_empty() {
        return Ok(());
    }
    let dlls = activation
        .copies
        .iter()
        .map(|copy| install_dll(copy, &bottle.path))
        .collect::<crate::Result<Vec<_>>>()?;
    let active = ActiveBackend {
        backend: activation.backend,
        version: activation.version.clone().unwrap_or_default(),
        dlls,
    };
    if bottle.state.active.as_ref() != Some(&active) {
        bottle.state.active = Some(active);
        bottle.save_state()?;
    }
    Ok(())
}

/// Copy one DLL into the bottle unless an identical file is already there.
fn install_dll(copy: &DllCopy, bottle_dir: &Path) -> crate::Result<InstalledDll> {
    let mut bytes = std::fs::read(&copy.source)
        .map_err(|err| crate::Error::io("cannot read", &copy.source, err))?;
    if copy.strip_builtin_marker && !uncork_pe::strip_builtin_marker(&mut bytes) {
        tracing::debug!(
            "{} has no builtin marker; copying it unchanged",
            copy.source.display()
        );
    }
    let sha256 = sha256_hex(&bytes);
    let dest = bottle_dir.join(&copy.dest);
    if file_sha256(&dest)?.as_deref() != Some(sha256.as_str()) {
        write_file_atomic(&dest, &bytes)?;
    }
    Ok(InstalledDll {
        path: copy.dest.clone(),
        sha256,
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

/// SHA-256 of the file at `path`, or `None` if there is none.
fn file_sha256(path: &Path) -> crate::Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(crate::Error::io("cannot read", path, err)),
    }
}

/// Write through a temporary sibling and rename, so a running Wine never
/// sees a half-written DLL.
fn write_file_atomic(dest: &Path, bytes: &[u8]) -> crate::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|err| crate::Error::io("cannot create directory", dir, err))?;
    }
    let file_name = dest.file_name().unwrap_or_default().to_string_lossy();
    let temp = dest.with_file_name(format!(".{file_name}.uncork-tmp"));
    std::fs::write(&temp, bytes).map_err(|err| crate::Error::io("cannot write", &temp, err))?;
    std::fs::rename(&temp, dest).map_err(|err| {
        if let Err(cleanup) = std::fs::remove_file(&temp) {
            tracing::warn!("cannot remove {}: {cleanup}", temp.display());
        }
        crate::Error::io("cannot replace", dest, err)
    })
}

/// Render overrides for `WINEDLLOVERRIDES`: entries grouped by value, groups
/// ordered by value, names sorted within a group, e.g.
/// `mscoree,winemenubuilder.exe=d;d3d10core,d3d11,dxgi=n,b`. An empty
/// value renders as `name=` (disabled).
#[must_use]
pub fn render_overrides(overrides: &BTreeMap<String, String>) -> String {
    let mut groups: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (name, value) in overrides {
        groups
            .entry(value.as_str())
            .or_default()
            .push(name.as_str());
    }
    groups
        .iter()
        .map(|(value, names)| format!("{}={value}", names.join(",")))
        .collect::<Vec<_>>()
        .join(";")
}

/// `true` if `dir` looks like a backend directory (has at least one of
/// [`ARCH_DIRS`]).
#[must_use]
pub fn is_backend_dir(dir: &Path) -> bool {
    ARCH_DIRS.iter().any(|arch| dir.join(arch).is_dir())
}

#[cfg(test)]
mod tests {
    use proptest::prelude::{any, prop, prop_assert, prop_assert_eq, proptest};

    use super::*;
    use crate::bottle::{BottleConfig, BottleState, CONFIG_FILE, STATE_FILE};
    use crate::component::{ComponentMeta, Source};

    use super::Backend::{D3dmetal, Dxmt, Dxvk, Wined3d};
    use uncork_pe::Bitness::{X64, X86};
    use uncork_pe::GraphicsApi as Api;

    const EVERYTHING: Availability = Availability {
        dxmt: true,
        dxvk: true,
        d3dmetal: true,
    };
    const NOTHING: Availability = Availability {
        dxmt: false,
        dxvk: false,
        d3dmetal: false,
    };
    const ALL_APIS: [GraphicsApi; 8] = [
        Api::DirectDraw,
        Api::OpenGl,
        Api::D3d8,
        Api::D3d9,
        Api::D3d10,
        Api::D3d11,
        Api::D3d12,
        Api::Vulkan,
    ];

    fn only(backends: &[Backend]) -> Availability {
        Availability {
            dxmt: backends.contains(&Dxmt),
            dxvk: backends.contains(&Dxvk),
            d3dmetal: backends.contains(&D3dmetal),
        }
    }

    // ----- names -----

    #[test]
    fn backend_names_parse_case_insensitively() {
        for (text, backend) in [
            ("dxmt", Dxmt),
            ("DXMT", Dxmt),
            ("dxvk", Dxvk),
            ("DxVk", Dxvk),
            ("d3dmetal", D3dmetal),
            ("D3DMetal", D3dmetal),
            ("d3d-metal", D3dmetal),
            ("GPTK", D3dmetal),
            ("wined3d", Wined3d),
            ("WineD3D", Wined3d),
            ("wine-d3d", Wined3d),
        ] {
            assert_eq!(text.parse::<Backend>(), Ok(backend), "{text}");
        }
    }

    #[test]
    fn backend_names_round_trip() {
        for backend in Backend::ALL {
            assert_eq!(backend.as_str().parse::<Backend>(), Ok(backend));
            assert_eq!(backend.to_string(), backend.as_str());
        }
    }

    #[test]
    fn unknown_backend_names_are_rejected() {
        for text in ["", "metal", "dxmt ", "vulkan", "auto"] {
            let err = text.parse::<Backend>().unwrap_err();
            assert!(
                err.contains("expected dxmt, d3dmetal, dxvk or wined3d"),
                "{err}"
            );
        }
    }

    #[test]
    fn backend_choice_reads_auto_and_aliases() {
        #[derive(serde::Deserialize, serde::Serialize)]
        struct Doc {
            backend: BackendChoice,
        }
        let parse = |text: &str| {
            toml::from_str::<Doc>(&format!("backend = {text:?}")).map(|doc| doc.backend)
        };
        assert_eq!(parse("auto").unwrap(), BackendChoice::Auto);
        assert_eq!(parse("AUTO").unwrap(), BackendChoice::Auto);
        assert_eq!(parse("gptk").unwrap(), BackendChoice::Fixed(D3dmetal));
        assert!(parse("opengl").is_err());
        let doc = Doc {
            backend: BackendChoice::Fixed(Dxmt),
        };
        assert_eq!(toml::to_string(&doc).unwrap().trim(), r#"backend = "dxmt""#);
    }

    // ----- supports -----

    #[test]
    fn supports_matches_the_documented_matrix() {
        // (backend, api, 32-bit, 64-bit)
        let matrix = [
            (D3dmetal, Api::DirectDraw, false, false),
            (D3dmetal, Api::D3d8, false, false),
            (D3dmetal, Api::D3d9, false, false),
            (D3dmetal, Api::D3d10, false, true),
            (D3dmetal, Api::D3d11, false, true),
            (D3dmetal, Api::D3d12, false, true),
            (Dxmt, Api::DirectDraw, false, false),
            (Dxmt, Api::D3d8, false, false),
            (Dxmt, Api::D3d9, false, false),
            (Dxmt, Api::D3d10, true, true),
            (Dxmt, Api::D3d11, true, true),
            (Dxmt, Api::D3d12, false, false),
            (Dxvk, Api::DirectDraw, false, false),
            (Dxvk, Api::D3d8, false, false),
            (Dxvk, Api::D3d9, false, false),
            (Dxvk, Api::D3d10, true, true),
            (Dxvk, Api::D3d11, true, true),
            (Dxvk, Api::D3d12, false, false),
            (Wined3d, Api::DirectDraw, true, true),
            (Wined3d, Api::D3d8, true, true),
            (Wined3d, Api::D3d9, true, true),
            (Wined3d, Api::D3d10, true, true),
            (Wined3d, Api::D3d11, true, true),
            (Wined3d, Api::D3d12, false, false),
        ];
        for (backend, api, x86, x64) in matrix {
            assert_eq!(supports(backend, api, X86), x86, "{backend} {api:?} 32-bit");
            assert_eq!(supports(backend, api, X64), x64, "{backend} {api:?} 64-bit");
        }
    }

    #[test]
    fn every_backend_supports_opengl_and_vulkan() {
        for backend in Backend::ALL {
            for api in [Api::OpenGl, Api::Vulkan] {
                for bitness in [X86, X64] {
                    assert!(
                        supports(backend, api, bitness),
                        "{backend} {api:?} {bitness:?}"
                    );
                }
            }
        }
    }

    // ----- recommend -----

    struct Case {
        api: Option<GraphicsApi>,
        bitness: Bitness,
        available: Availability,
        preferred: Option<Backend>,
        fallbacks: &'static [Backend],
        geometry_shaders: bool,
        expect: Backend,
        why: &'static str,
    }

    fn auto(
        api: Option<GraphicsApi>,
        bitness: Bitness,
        available: Availability,
        expect: Backend,
        why: &'static str,
    ) -> Case {
        Case {
            api,
            bitness,
            available,
            preferred: None,
            fallbacks: &[],
            geometry_shaders: false,
            expect,
            why,
        }
    }

    fn run(case: &Case) -> Recommendation {
        recommend(
            case.api,
            case.bitness,
            &case.available,
            case.preferred,
            case.fallbacks,
            case.geometry_shaders,
        )
    }

    #[test]
    fn decision_matrix() {
        let cases = [
            auto(
                Some(Api::D3d11),
                X86,
                EVERYTHING,
                Dxmt,
                "32-bit D3D11 runs on DXMT",
            ),
            auto(
                Some(Api::D3d10),
                X86,
                EVERYTHING,
                Dxmt,
                "32-bit D3D10 runs on DXMT",
            ),
            auto(
                Some(Api::D3d11),
                X86,
                only(&[Dxvk, D3dmetal]),
                Wined3d,
                "32-bit D3D11 without DXMT: WineD3D before DXVK",
            ),
            auto(
                Some(Api::D3d10),
                X86,
                only(&[Dxvk]),
                Wined3d,
                "32-bit D3D10 without DXMT: WineD3D before DXVK",
            ),
            auto(
                Some(Api::D3d11),
                X86,
                NOTHING,
                Wined3d,
                "32-bit D3D11 with nothing installed",
            ),
            auto(
                Some(Api::D3d11),
                X64,
                EVERYTHING,
                Dxmt,
                "64-bit D3D11 prefers DXMT",
            ),
            auto(
                Some(Api::D3d11),
                X64,
                only(&[Dxvk, D3dmetal]),
                D3dmetal,
                "64-bit D3D11 without DXMT: D3DMetal",
            ),
            auto(
                Some(Api::D3d10),
                X64,
                only(&[D3dmetal]),
                D3dmetal,
                "64-bit D3D10 on D3DMetal",
            ),
            auto(
                Some(Api::D3d11),
                X64,
                only(&[Dxvk]),
                Dxvk,
                "64-bit D3D11 with only DXVK",
            ),
            auto(
                Some(Api::D3d11),
                X64,
                NOTHING,
                Wined3d,
                "64-bit D3D11 with nothing installed",
            ),
            auto(
                Some(Api::D3d12),
                X64,
                EVERYTHING,
                D3dmetal,
                "64-bit D3D12 needs D3DMetal",
            ),
            auto(
                Some(Api::D3d12),
                X64,
                only(&[Dxmt, Dxvk]),
                Wined3d,
                "64-bit D3D12 without D3DMetal",
            ),
            auto(
                Some(Api::D3d12),
                X86,
                EVERYTHING,
                Wined3d,
                "32-bit D3D12 has no backend",
            ),
            auto(
                Some(Api::D3d9),
                X86,
                EVERYTHING,
                Wined3d,
                "D3D9 is WineD3D only",
            ),
            auto(
                Some(Api::D3d9),
                X64,
                EVERYTHING,
                Wined3d,
                "D3D9 is WineD3D only",
            ),
            auto(
                Some(Api::D3d8),
                X86,
                EVERYTHING,
                Wined3d,
                "D3D8 is WineD3D only",
            ),
            auto(
                Some(Api::DirectDraw),
                X86,
                EVERYTHING,
                Wined3d,
                "DirectDraw is WineD3D only",
            ),
            auto(None, X64, EVERYTHING, Wined3d, "no Direct3D detected"),
            auto(
                Some(Api::OpenGl),
                X64,
                EVERYTHING,
                Wined3d,
                "OpenGL needs no backend",
            ),
            auto(
                Some(Api::Vulkan),
                X86,
                EVERYTHING,
                Wined3d,
                "Vulkan needs no backend",
            ),
            Case {
                geometry_shaders: true,
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    only(&[Dxvk]),
                    Wined3d,
                    "geometry shaders rule DXVK out",
                )
            },
            Case {
                geometry_shaders: true,
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    EVERYTHING,
                    Dxmt,
                    "geometry shaders do not affect DXMT",
                )
            },
            Case {
                preferred: Some(D3dmetal),
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    EVERYTHING,
                    D3dmetal,
                    "preferred beats the table",
                )
            },
            Case {
                preferred: Some(D3dmetal),
                ..auto(
                    Some(Api::D3d11),
                    X86,
                    EVERYTHING,
                    Dxmt,
                    "D3DMetal is never used for 32-bit",
                )
            },
            Case {
                preferred: Some(D3dmetal),
                fallbacks: &[D3dmetal],
                ..auto(
                    Some(Api::D3d11),
                    X86,
                    EVERYTHING,
                    Dxmt,
                    "not even as a fallback",
                )
            },
            Case {
                preferred: Some(D3dmetal),
                ..auto(None, X86, EVERYTHING, Wined3d, "nor with no API detected")
            },
            Case {
                preferred: Some(Dxmt),
                fallbacks: &[Dxvk],
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    only(&[Dxvk, D3dmetal]),
                    Dxvk,
                    "fallbacks come before the table",
                )
            },
            Case {
                fallbacks: &[D3dmetal],
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    EVERYTHING,
                    D3dmetal,
                    "fallbacks without a preference",
                )
            },
            Case {
                preferred: Some(Dxmt),
                fallbacks: &[Dxvk, Wined3d],
                geometry_shaders: true,
                ..auto(
                    Some(Api::D3d11),
                    X86,
                    only(&[Dxvk]),
                    Wined3d,
                    "an excluded fallback is skipped",
                )
            },
            Case {
                preferred: Some(Dxvk),
                geometry_shaders: true,
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    EVERYTHING,
                    Dxmt,
                    "an excluded preference is skipped",
                )
            },
            Case {
                preferred: Some(Wined3d),
                ..auto(
                    Some(Api::D3d11),
                    X64,
                    EVERYTHING,
                    Wined3d,
                    "WineD3D can be preferred",
                )
            },
            Case {
                preferred: Some(Dxmt),
                ..auto(
                    Some(Api::D3d11),
                    X86,
                    only(&[Dxvk]),
                    Wined3d,
                    "an unavailable preference falls back to the table",
                )
            },
            Case {
                preferred: Some(Dxvk),
                ..auto(
                    Some(Api::D3d9),
                    X64,
                    EVERYTHING,
                    Wined3d,
                    "an incompatible preference is skipped",
                )
            },
            Case {
                preferred: Some(Dxmt),
                ..auto(
                    None,
                    X64,
                    EVERYTHING,
                    Dxmt,
                    "a preference is trusted when no API was detected",
                )
            },
            Case {
                preferred: Some(Dxmt),
                ..auto(
                    Some(Api::D3d12),
                    X64,
                    EVERYTHING,
                    D3dmetal,
                    "DXMT cannot run D3D12",
                )
            },
            Case {
                preferred: Some(Dxmt),
                ..auto(
                    Some(Api::OpenGl),
                    X64,
                    EVERYTHING,
                    Wined3d,
                    "OpenGL ignores preferences",
                )
            },
        ];
        for case in &cases {
            let got = run(case);
            assert_eq!(got.backend, case.expect, "{}: {}", case.why, got.reason);
        }
    }

    #[test]
    fn reasons_are_one_explanatory_sentence() {
        let reason = |case: Case| run(&case).reason;
        assert_eq!(
            reason(auto(Some(Api::D3d11), X86, EVERYTHING, Dxmt, "")),
            "32-bit D3D11: DXMT (D3DMetal is 64-bit only)"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d11), X64, EVERYTHING, Dxmt, "")),
            "64-bit D3D11: DXMT"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d11), X64, only(&[D3dmetal]), D3dmetal, "")),
            "64-bit D3D11: D3DMetal (DXMT not available)"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d12), X64, EVERYTHING, D3dmetal, "")),
            "64-bit D3D12: D3DMetal (only D3DMetal runs D3D12)"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d12), X64, NOTHING, Wined3d, "")),
            "64-bit D3D12: WineD3D, which will fail (D3DMetal not available; only D3DMetal runs D3D12)"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d12), X86, EVERYTHING, Wined3d, "")),
            "32-bit D3D12: WineD3D, which will fail (D3DMetal, the only D3D12 backend, is 64-bit only)"
        );
        assert_eq!(
            reason(auto(Some(Api::D3d9), X64, EVERYTHING, Wined3d, "")),
            "64-bit D3D9: WineD3D (only WineD3D runs D3D9)"
        );
        assert_eq!(
            reason(auto(Some(Api::OpenGl), X64, EVERYTHING, Wined3d, "")),
            "64-bit OpenGL: WineD3D (OpenGL needs no Direct3D translation)"
        );
        assert_eq!(
            reason(auto(None, X86, EVERYTHING, Wined3d, "")),
            "32-bit, no Direct3D detected: WineD3D"
        );
        assert_eq!(
            reason(Case {
                geometry_shaders: true,
                ..auto(Some(Api::D3d11), X64, only(&[Dxvk]), Wined3d, "")
            }),
            "64-bit D3D11: WineD3D (DXMT not available; D3DMetal not available; \
             DXVK cannot run games that need geometry shaders)"
        );
        // Rise of Nations' profile on a machine without DXMT.
        assert_eq!(
            reason(Case {
                preferred: Some(Dxmt),
                fallbacks: &[Wined3d],
                geometry_shaders: true,
                ..auto(Some(Api::D3d11), X86, NOTHING, Wined3d, "")
            }),
            "32-bit D3D11: WineD3D (fallback; DXMT not available; D3DMetal is 64-bit only)"
        );
        assert_eq!(
            reason(Case {
                preferred: Some(D3dmetal),
                ..auto(Some(Api::D3d11), X64, EVERYTHING, D3dmetal, "")
            }),
            "64-bit D3D11: D3DMetal (preferred)"
        );
    }

    #[test]
    fn order_lists_every_candidate_best_first() {
        let order = |case: Case| run(&case).order;
        assert_eq!(
            order(auto(Some(Api::D3d11), X64, NOTHING, Wined3d, "")),
            [Dxmt, D3dmetal, Dxvk, Wined3d],
            "unavailable entries are listed"
        );
        assert_eq!(
            order(auto(Some(Api::D3d11), X86, NOTHING, Wined3d, "")),
            [Dxmt, Wined3d, Dxvk]
        );
        assert_eq!(
            order(auto(Some(Api::D3d12), X64, NOTHING, Wined3d, "")),
            [D3dmetal, Wined3d]
        );
        assert_eq!(
            order(auto(Some(Api::D3d12), X86, EVERYTHING, Wined3d, "")),
            [Wined3d]
        );
        assert_eq!(
            order(auto(Some(Api::D3d9), X64, EVERYTHING, Wined3d, "")),
            [Wined3d]
        );
        assert_eq!(
            order(Case {
                geometry_shaders: true,
                ..auto(Some(Api::D3d11), X64, EVERYTHING, Dxmt, "")
            }),
            [Dxmt, D3dmetal, Wined3d],
            "DXVK is removed from consideration"
        );
        assert_eq!(
            order(Case {
                preferred: Some(Dxvk),
                fallbacks: &[Dxvk, Wined3d],
                ..auto(Some(Api::D3d11), X64, EVERYTHING, Dxvk, "")
            }),
            [Dxvk, Wined3d, Dxmt, D3dmetal],
            "preference, then fallbacks, then the table, without duplicates"
        );
        assert_eq!(
            order(Case {
                preferred: Some(D3dmetal),
                ..auto(Some(Api::D3d11), X86, EVERYTHING, Dxmt, "")
            }),
            [Dxmt, Wined3d, Dxvk],
            "incompatible entries are not candidates"
        );
    }

    fn any_backend() -> impl proptest::strategy::Strategy<Value = Backend> {
        prop::sample::select(Backend::ALL.to_vec())
    }

    proptest! {
        #[test]
        fn recommendations_are_always_runnable(
            api in prop::option::of(prop::sample::select(ALL_APIS.to_vec())),
            bitness in prop::sample::select(vec![X86, X64]),
            (dxmt, dxvk, d3dmetal) in any::<(bool, bool, bool)>(),
            preferred in prop::option::of(any_backend()),
            fallbacks in prop::collection::vec(any_backend(), 0..5),
            geometry_shaders in any::<bool>(),
        ) {
            let available = Availability { dxmt, dxvk, d3dmetal };
            let got = recommend(api, bitness, &available, preferred, &fallbacks, geometry_shaders);

            prop_assert!(got.order.contains(&got.backend));
            let mut deduplicated = got.order.clone();
            deduplicated.sort();
            deduplicated.dedup();
            prop_assert_eq!(deduplicated.len(), got.order.len(), "duplicates in {:?}", got.order);
            prop_assert!(available.has(got.backend));
            if bitness == X86 {
                prop_assert!(!got.order.contains(&D3dmetal));
            }
            if geometry_shaders {
                prop_assert!(!got.order.contains(&Dxvk));
            }
            if let Some(api) = api {
                let runs = supports(got.backend, api, bitness);
                prop_assert!(runs || got.backend == Wined3d);
                prop_assert_eq!(got.reason.contains("will fail"), !runs);
            }
            prop_assert!(got.reason.starts_with(&describe_program(api, bitness)));
        }
    }

    // ----- strategies -----

    fn wine(features: &[&str]) -> WineRuntime {
        WineRuntime {
            root: PathBuf::from("/runtime"),
            version: "11.0".to_owned(),
            features: features.iter().map(|f| (*f).to_owned()).collect(),
        }
    }

    #[test]
    fn wined3d_is_always_builtin() {
        for features in [&[][..], &["renderer-dllpath"], &["dllpath-prepend"]] {
            assert_eq!(
                strategy_for(Wined3d, &wine(features)).unwrap(),
                Strategy::Builtin
            );
        }
    }

    #[test]
    fn renderer_dllpath_runtimes_get_a_variable_per_backend() {
        let runtime = wine(&["msync", "renderer-dllpath", "dllpath-prepend"]);
        for (backend, var) in [
            (Dxmt, "WINEDLLPATH_DXMT"),
            (Dxvk, "WINEDLLPATH_DXVK"),
            (D3dmetal, "WINEDLLPATH_D3DMETAL"),
        ] {
            assert_eq!(
                strategy_for(backend, &runtime).unwrap(),
                Strategy::RendererEnv {
                    var: var.to_owned()
                }
            );
        }
    }

    #[test]
    fn dllpath_prepend_runtimes_prepend() {
        let runtime = wine(&["dllpath-prepend"]);
        for backend in [Dxmt, Dxvk, D3dmetal] {
            assert_eq!(
                strategy_for(backend, &runtime).unwrap(),
                Strategy::DllPathPrepend
            );
        }
    }

    #[test]
    fn other_runtimes_copy_into_the_prefix() {
        let runtime = wine(&["msync", "wow64"]);
        assert_eq!(
            strategy_for(Dxmt, &runtime).unwrap(),
            Strategy::PrefixNative
        );
        assert_eq!(
            strategy_for(Dxvk, &runtime).unwrap(),
            Strategy::PrefixNative
        );
    }

    #[test]
    fn d3dmetal_is_never_copied_into_the_prefix() {
        let err = strategy_for(D3dmetal, &wine(&["msync"])).unwrap_err();
        let crate::Error::Unsupported(message) = err else {
            panic!("expected Unsupported, got {err:?}");
        };
        assert!(
            message.contains("renderer-dllpath") && message.contains("dllpath-prepend"),
            "{message}"
        );
        assert!(message.contains("Wine 11.0"), "{message}");
    }

    #[test]
    fn replaced_dlls_per_backend() {
        assert_eq!(replaced_dlls(Dxmt), ["d3d11", "dxgi", "d3d10core"]);
        assert_eq!(replaced_dlls(Dxvk), ["d3d11", "d3d10core"]);
        assert_eq!(
            replaced_dlls(D3dmetal),
            ["d3d11", "dxgi", "d3d12", "d3d12core", "d3d10", "d3d10core"]
        );
        assert!(replaced_dlls(Wined3d).is_empty());
    }

    // ----- activation -----

    const DXMT_FILES: &[&str] = &[
        "x86_64-windows/d3d11.dll",
        "x86_64-windows/dxgi.dll",
        "x86_64-windows/d3d10core.dll",
        "x86_64-windows/winemetal.dll",
        "i386-windows/d3d11.dll",
        "i386-windows/dxgi.dll",
        "i386-windows/d3d10core.dll",
        "i386-windows/winemetal.dll",
        "x86_64-unix/winemetal.so",
    ];
    const DXVK_FILES: &[&str] = &[
        "x86_64-windows/d3d11.dll",
        "x86_64-windows/d3d10core.dll",
        "x86_64-windows/dxgi.dll",
        "i386-windows/d3d11.dll",
        "i386-windows/d3d10core.dll",
        "i386-windows/dxgi.dll",
    ];
    const D3DMETAL_FILES: &[&str] = &[
        "external/libd3dshared.dylib",
        "wine/x86_64-windows/d3d11.dll",
        "wine/x86_64-windows/dxgi.dll",
        "wine/x86_64-windows/d3d12.dll",
        "wine/x86_64-unix/d3d11.so",
    ];

    /// A component directory under `root/<kind>` holding `files`, each
    /// containing its own relative path.
    fn component(root: &Path, kind: ComponentKind, files: &[&str]) -> InstalledComponent {
        let dir = root.join(kind.as_str());
        std::fs::create_dir_all(&dir).unwrap();
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, file).unwrap();
        }
        InstalledComponent {
            meta: ComponentMeta {
                schema: 1,
                kind,
                version: "1.0".to_owned(),
                source: Source::Local { path: dir.clone() },
                license: "Zlib".to_owned(),
                source_code: None,
                features: Vec::new(),
                installed_unix: 0,
            },
            path: dir,
        }
    }

    fn bottle(path: &Path) -> Bottle {
        Bottle {
            path: path.to_path_buf(),
            config: BottleConfig {
                schema: 1,
                name: "test".to_owned(),
                wine: "11.0".to_owned(),
                windows_version: crate::bottle::WindowsVersion::Win10,
                graphics: crate::bottle::GraphicsConfig::default(),
                performance: crate::bottle::PerformanceConfig::default(),
                env: BTreeMap::new(),
                dll_overrides: BTreeMap::new(),
                imported_from: None,
            },
            state: BottleState::default(),
        }
    }

    fn copy(component: &InstalledComponent, source: &str, dest: &str, strip: bool) -> DllCopy {
        DllCopy {
            source: component.path.join(source),
            dest: PathBuf::from(dest),
            strip_builtin_marker: strip,
        }
    }

    fn overrides(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    fn winemetal_copies_of(dxmt: &InstalledComponent) -> Vec<DllCopy> {
        vec![
            copy(
                dxmt,
                "x86_64-windows/winemetal.dll",
                "drive_c/windows/system32/winemetal.dll",
                false,
            ),
            copy(
                dxmt,
                "i386-windows/winemetal.dll",
                "drive_c/windows/syswow64/winemetal.dll",
                false,
            ),
        ]
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        bottle: Bottle,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let bottle = bottle(&root.join("bottles/test"));
        Fixture {
            _dir: dir,
            root,
            bottle,
        }
    }

    #[test]
    fn dxmt_with_renderer_env() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let got = activation(
            Dxmt,
            Some(&dxmt),
            &wine(&["renderer-dllpath"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();

        assert_eq!(got.backend, Dxmt);
        assert_eq!(got.version.as_deref(), Some("1.0"));
        assert_eq!(
            got.strategy,
            Strategy::RendererEnv {
                var: "WINEDLLPATH_DXMT".to_owned()
            }
        );
        assert_eq!(got.copies, winemetal_copies_of(&dxmt));
        assert_eq!(
            got.env,
            overrides(&[
                ("DXMT_LOG_LEVEL", "none"),
                (
                    "DXMT_SHADER_CACHE_PATH",
                    f.bottle.path.join("cache/dxmt").to_str().unwrap()
                ),
                ("WINEDLLPATH_DXMT", dxmt.path.to_str().unwrap()),
            ])
        );
        assert_eq!(
            got.overrides,
            overrides(&[("d3d10", "b"), ("d3d12", "b"), ("d3d12core", "b")])
        );
    }

    #[test]
    fn dxmt_with_dllpath_prepend() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let got = activation(
            Dxmt,
            Some(&dxmt),
            &wine(&["dllpath-prepend"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();

        assert_eq!(got.strategy, Strategy::DllPathPrepend);
        assert_eq!(got.env["WINEDLLPATH_PREPEND"], dxmt.path.to_str().unwrap());
        assert!(!got.env.contains_key("WINEDLLPATH_DXMT"));
        assert_eq!(got.copies, winemetal_copies_of(&dxmt));
        assert_eq!(
            got.overrides,
            overrides(&[("d3d10", "b"), ("d3d12", "b"), ("d3d12core", "b")])
        );
    }

    /// A runtime directory under `root` whose `lib/wine/x86_64-unix` has
    /// DXMT's bridge, as CrossOver-derived runtimes do.
    fn wine_with_bridge(root: &Path) -> WineRuntime {
        let unix = root.join("runtime/lib/wine/x86_64-unix");
        std::fs::create_dir_all(&unix).unwrap();
        std::fs::write(unix.join("winemetal.so"), "bridge").unwrap();
        WineRuntime {
            root: root.join("runtime"),
            ..wine(&[])
        }
    }

    #[test]
    fn dxmt_in_the_prefix_needs_the_runtime_bridge() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let err = activation(
            Dxmt,
            Some(&dxmt),
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, crate::Error::Unsupported(_)), "{err:?}");
        assert!(err.to_string().contains("winemetal.so"), "{err}");
    }

    #[test]
    fn dxmt_in_the_prefix() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let got = activation(
            Dxmt,
            Some(&dxmt),
            &wine_with_bridge(&f.root),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();

        assert_eq!(got.strategy, Strategy::PrefixNative);
        let mut expected = winemetal_copies_of(&dxmt);
        for dll in ["d3d11", "dxgi", "d3d10core"] {
            expected.push(copy(
                &dxmt,
                &format!("x86_64-windows/{dll}.dll"),
                &format!("drive_c/windows/system32/{dll}.dll"),
                true,
            ));
            expected.push(copy(
                &dxmt,
                &format!("i386-windows/{dll}.dll"),
                &format!("drive_c/windows/syswow64/{dll}.dll"),
                true,
            ));
        }
        assert_eq!(got.copies, expected);
        assert_eq!(
            got.overrides,
            overrides(&[
                ("d3d10", "b"),
                ("d3d10core", "n,b"),
                ("d3d11", "n,b"),
                ("d3d12", "b"),
                ("d3d12core", "b"),
                ("dxgi", "n,b"),
            ])
        );
        assert!(
            !got.env.keys().any(|key| key.starts_with("WINEDLLPATH")),
            "{:?}",
            got.env
        );
    }

    #[test]
    fn dxmt_without_32_bit_files_copies_64_bit_only() {
        let f = fixture();
        let files: Vec<&str> = DXMT_FILES
            .iter()
            .copied()
            .filter(|f| !f.starts_with("i386"))
            .collect();
        let dxmt = component(&f.root, ComponentKind::Dxmt, &files);
        let got = activation(
            Dxmt,
            Some(&dxmt),
            &wine_with_bridge(&f.root),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();
        assert_eq!(got.copies.len(), 4);
        assert!(
            got.copies
                .iter()
                .all(|c| c.dest.starts_with("drive_c/windows/system32")),
            "{:?}",
            got.copies
        );
    }

    #[test]
    fn dxmt_needs_its_64_bit_winemetal() {
        let f = fixture();
        let files: Vec<&str> = DXMT_FILES
            .iter()
            .copied()
            .filter(|f| *f != "x86_64-windows/winemetal.dll")
            .collect();
        let dxmt = component(&f.root, ComponentKind::Dxmt, &files);
        let err = activation(
            Dxmt,
            Some(&dxmt),
            &wine(&["renderer-dllpath"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, crate::Error::BrokenComponent { message, .. } if message.contains("winemetal.dll")),
            "{err:?}"
        );
    }

    #[test]
    fn dxvk_in_the_prefix_keeps_wines_dxgi() {
        let f = fixture();
        let dxvk = component(&f.root, ComponentKind::Dxvk, DXVK_FILES);
        let got = activation(
            Dxvk,
            Some(&dxvk),
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();

        assert_eq!(got.strategy, Strategy::PrefixNative);
        let copied: Vec<&Path> = got.copies.iter().map(|c| c.dest.as_path()).collect();
        assert_eq!(
            copied,
            [
                Path::new("drive_c/windows/system32/d3d11.dll"),
                Path::new("drive_c/windows/syswow64/d3d11.dll"),
                Path::new("drive_c/windows/system32/d3d10core.dll"),
                Path::new("drive_c/windows/syswow64/d3d10core.dll"),
            ]
        );
        assert!(got.copies.iter().all(|c| c.strip_builtin_marker));
        assert_eq!(
            got.overrides,
            overrides(&[
                ("d3d10", "b"),
                ("d3d10core", "n,b"),
                ("d3d11", "n,b"),
                ("d3d12", "b"),
                ("d3d12core", "b"),
                ("dxgi", "b"),
            ])
        );
        let cache = f.bottle.path.join("cache/dxvk");
        assert_eq!(
            got.env,
            overrides(&[
                ("DXVK_ASYNC", "1"),
                ("DXVK_LOG_LEVEL", "none"),
                ("DXVK_STATE_CACHE_PATH", cache.to_str().unwrap()),
                ("MVK_CONFIG_RESUME_LOST_DEVICE", "1"),
            ])
        );
    }

    #[test]
    fn dxvk_with_renderer_env_copies_nothing() {
        let f = fixture();
        let dxvk = component(&f.root, ComponentKind::Dxvk, DXVK_FILES);
        let got = activation(
            Dxvk,
            Some(&dxvk),
            &wine(&["renderer-dllpath"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();
        assert!(got.copies.is_empty());
        assert_eq!(got.env["WINEDLLPATH_DXVK"], dxvk.path.to_str().unwrap());
        assert_eq!(got.overrides["dxgi"], "b");
        assert!(!got.overrides.contains_key("d3d11"));
    }

    #[test]
    fn prefix_native_needs_every_replaced_dll() {
        let f = fixture();
        let dxvk = component(
            &f.root,
            ComponentKind::Dxvk,
            &["x86_64-windows/d3d11.dll", "i386-windows/d3d11.dll"],
        );
        let err = activation(
            Dxvk,
            Some(&dxvk),
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, crate::Error::BrokenComponent { kind: ComponentKind::Dxvk, message, .. }
                if message.contains("d3d10core.dll")),
            "{err:?}"
        );
    }

    #[test]
    fn path_strategies_need_an_architecture_directory() {
        let f = fixture();
        let dxvk = component(&f.root, ComponentKind::Dxvk, &["README.md"]);
        for features in [&["renderer-dllpath"][..], &["dllpath-prepend"]] {
            let err = activation(
                Dxvk,
                Some(&dxvk),
                &wine(features),
                &f.bottle,
                BackendOptions::default(),
            )
            .unwrap_err();
            assert!(
                matches!(err, crate::Error::BrokenComponent { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn d3dmetal_with_renderer_env() {
        let f = fixture();
        let gptk = component(&f.root, ComponentKind::D3dmetal, D3DMETAL_FILES);
        let got = activation(
            D3dmetal,
            Some(&gptk),
            &wine(&["renderer-dllpath", "d3dmetal"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();

        assert_eq!(
            got.strategy,
            Strategy::RendererEnv {
                var: "WINEDLLPATH_D3DMETAL".to_owned()
            }
        );
        assert!(got.copies.is_empty());
        let shared = gptk.path.join("external/libd3dshared.dylib");
        let wine_dir = gptk.path.join("wine");
        assert_eq!(
            got.env,
            overrides(&[
                ("CX_APPLEGPTK_LIBD3DSHARED_PATH", shared.to_str().unwrap()),
                ("WINEDLLPATH_D3DMETAL", wine_dir.to_str().unwrap()),
            ])
        );
        assert!(
            got.overrides.is_empty(),
            "D3DMetal replaces every DLL the others do: {:?}",
            got.overrides
        );
    }

    #[test]
    fn d3dmetal_with_dllpath_prepend() {
        let f = fixture();
        let gptk = component(&f.root, ComponentKind::D3dmetal, D3DMETAL_FILES);
        let got = activation(
            D3dmetal,
            Some(&gptk),
            &wine(&["dllpath-prepend"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();
        assert_eq!(got.strategy, Strategy::DllPathPrepend);
        assert_eq!(
            got.env["WINEDLLPATH_PREPEND"],
            gptk.path.join("wine").to_str().unwrap()
        );
    }

    #[test]
    fn d3dmetal_needs_libd3dshared() {
        let f = fixture();
        let gptk = component(
            &f.root,
            ComponentKind::D3dmetal,
            &["wine/x86_64-windows/d3d11.dll"],
        );
        let err = activation(
            D3dmetal,
            Some(&gptk),
            &wine(&["renderer-dllpath"]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, crate::Error::BrokenComponent { message, .. } if message.contains("libd3dshared.dylib")),
            "{err:?}"
        );
    }

    #[test]
    fn d3dmetal_without_per_process_paths_is_unsupported() {
        let f = fixture();
        let gptk = component(&f.root, ComponentKind::D3dmetal, D3DMETAL_FILES);
        let err = activation(
            D3dmetal,
            Some(&gptk),
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(matches!(err, crate::Error::Unsupported(_)), "{err:?}");
    }

    #[test]
    fn wined3d_pins_every_direct3d_dll_to_builtin() {
        let f = fixture();
        let got = activation(
            Wined3d,
            None,
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap();
        assert_eq!(got.strategy, Strategy::Builtin);
        assert_eq!(got.version, None);
        assert!(got.copies.is_empty());
        assert_eq!(got.env, overrides(&[("WINE_D3D_CONFIG", "renderer=gl")]));
        assert_eq!(
            got.overrides,
            overrides(&[
                ("d3d10", "b"),
                ("d3d10core", "b"),
                ("d3d11", "b"),
                ("d3d12", "b"),
                ("d3d12core", "b"),
                ("dxgi", "b"),
            ])
        );
    }

    #[test]
    fn missing_components_name_the_fix() {
        let f = fixture();
        let runtime = wine(&["renderer-dllpath"]);
        for (backend, fix) in [
            (Dxmt, "uncork runtime install dxmt"),
            (Dxvk, "uncork runtime install dxvk"),
            (D3dmetal, "uncork runtime import-gptk"),
        ] {
            let err = activation(
                backend,
                None,
                &runtime,
                &f.bottle,
                BackendOptions::default(),
            )
            .unwrap_err();
            let crate::Error::Unsupported(message) = err else {
                panic!("expected Unsupported for {backend}, got {err:?}");
            };
            assert!(message.contains(fix), "{message}");
        }
    }

    #[test]
    fn components_of_the_wrong_kind_are_rejected() {
        let f = fixture();
        let dxvk = component(&f.root, ComponentKind::Dxvk, DXVK_FILES);
        let err = activation(
            Dxmt,
            Some(&dxvk),
            &wine(&[]),
            &f.bottle,
            BackendOptions::default(),
        )
        .unwrap_err();
        assert!(
            matches!(&err, crate::Error::BrokenComponent { kind: ComponentKind::Dxvk, message, .. }
                if message == "expected a dxmt component"),
            "{err:?}"
        );
    }

    #[test]
    fn options_add_their_variables() {
        let f = fixture();
        let runtime = wine(&["renderer-dllpath"]);
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let dxvk = component(&f.root, ComponentKind::Dxvk, DXVK_FILES);
        let gptk = component(&f.root, ComponentKind::D3dmetal, D3DMETAL_FILES);
        let all_on = BackendOptions {
            metalfx: true,
            hud: true,
            debug_logs: true,
        };
        let env = |backend, component| {
            activation(backend, component, &runtime, &f.bottle, all_on)
                .unwrap()
                .env
        };

        let dxmt_env = env(Dxmt, Some(&dxmt));
        assert_eq!(dxmt_env["DXMT_METALFX_SPATIAL_SWAPCHAIN"], "1");
        assert_eq!(dxmt_env["DXMT_LOG_LEVEL"], "info");
        assert_eq!(dxmt_env["MTL_HUD_ENABLED"], "1");

        let dxvk_env = env(Dxvk, Some(&dxvk));
        assert_eq!(dxvk_env["DXVK_LOG_LEVEL"], "info");
        assert_eq!(dxvk_env["MTL_HUD_ENABLED"], "1");
        assert!(!dxvk_env.keys().any(|key| key.contains("METALFX")));

        let gptk_env = env(D3dmetal, Some(&gptk));
        assert_eq!(gptk_env["D3DM_ENABLE_METALFX"], "1");
        assert_eq!(gptk_env["MTL_HUD_ENABLED"], "1");

        let wined3d_env = env(Wined3d, None);
        assert_eq!(wined3d_env["MTL_HUD_ENABLED"], "1");
        assert!(!wined3d_env.keys().any(|key| key.contains("METALFX")));
    }

    #[test]
    fn backend_dir_per_backend() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let gptk = component(&f.root, ComponentKind::D3dmetal, D3DMETAL_FILES);
        assert_eq!(backend_dir(Dxmt, &dxmt), dxmt.path);
        assert_eq!(backend_dir(D3dmetal, &gptk), gptk.path.join("wine"));
        assert!(is_backend_dir(&backend_dir(Dxmt, &dxmt)));
        assert!(is_backend_dir(&backend_dir(D3dmetal, &gptk)));
        assert!(!is_backend_dir(&gptk.path));
    }

    // ----- copies -----

    #[test]
    fn sha256_hex_matches_the_standard_vector() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn install_dll_copies_and_creates_directories() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let winemetal = copy(
            &dxmt,
            "i386-windows/winemetal.dll",
            "drive_c/windows/syswow64/winemetal.dll",
            false,
        );

        let installed = install_dll(&winemetal, &f.bottle.path).unwrap();

        let dest = f.bottle.path.join("drive_c/windows/syswow64/winemetal.dll");
        assert_eq!(std::fs::read(&dest).unwrap(), b"i386-windows/winemetal.dll");
        assert_eq!(
            installed.path,
            Path::new("drive_c/windows/syswow64/winemetal.dll")
        );
        assert_eq!(installed.sha256, sha256_hex(b"i386-windows/winemetal.dll"));
        let leftovers: Vec<_> = std::fs::read_dir(dest.parent().unwrap()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temporary file is left behind");
    }

    #[test]
    fn install_dll_skips_identical_files_and_replaces_others() {
        use std::os::unix::fs::MetadataExt;

        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let winemetal = copy(
            &dxmt,
            "x86_64-windows/winemetal.dll",
            "drive_c/windows/system32/winemetal.dll",
            false,
        );
        let dest = f.bottle.path.join(&winemetal.dest);

        install_dll(&winemetal, &f.bottle.path).unwrap();
        let first = std::fs::metadata(&dest).unwrap().ino();
        install_dll(&winemetal, &f.bottle.path).unwrap();
        assert_eq!(
            std::fs::metadata(&dest).unwrap().ino(),
            first,
            "identical file was rewritten"
        );

        std::fs::write(&winemetal.source, b"a newer build").unwrap();
        let installed = install_dll(&winemetal, &f.bottle.path).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"a newer build");
        assert_eq!(installed.sha256, sha256_hex(b"a newer build"));
    }

    #[test]
    fn install_dll_replaces_a_wine_placeholder_and_nothing_else() {
        let f = fixture();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let system32 = f.bottle.system32();
        std::fs::create_dir_all(&system32).unwrap();
        std::fs::write(system32.join("winemetal.dll"), b"Wine placeholder DLL").unwrap();
        std::fs::write(system32.join("kernel32.dll"), b"untouched").unwrap();

        let winemetal = copy(
            &dxmt,
            "x86_64-windows/winemetal.dll",
            "drive_c/windows/system32/winemetal.dll",
            false,
        );
        install_dll(&winemetal, &f.bottle.path).unwrap();

        assert_eq!(
            std::fs::read(system32.join("winemetal.dll")).unwrap(),
            b"x86_64-windows/winemetal.dll"
        );
        assert_eq!(
            std::fs::read(system32.join("kernel32.dll")).unwrap(),
            b"untouched"
        );
    }

    #[test]
    fn install_dll_reports_a_missing_source() {
        let f = fixture();
        let missing = DllCopy {
            source: f.root.join("nowhere.dll"),
            dest: PathBuf::from("drive_c/windows/system32/nowhere.dll"),
            strip_builtin_marker: false,
        };
        let err = install_dll(&missing, &f.bottle.path).unwrap_err();
        assert!(
            matches!(&err, crate::Error::Io { path, .. } if path == &f.root.join("nowhere.dll")),
            "{err:?}"
        );
    }

    #[test]
    fn apply_copies_without_copies_changes_nothing() {
        let f = fixture();
        let mut bottle = f.bottle.clone();
        let activation = activation(
            Wined3d,
            None,
            &wine(&[]),
            &bottle,
            BackendOptions::default(),
        )
        .unwrap();
        apply_copies(&activation, &mut bottle).unwrap();
        assert_eq!(bottle.state, BottleState::default());
        assert!(!bottle.path.exists());
    }

    /// A PE-like file with Wine's builtin marker at 0x40.
    fn builtin_dll(body: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0u8; 0x40];
        bytes[..2].copy_from_slice(b"MZ");
        bytes.extend_from_slice(uncork_pe::WINE_BUILTIN_SIGNATURE);
        bytes.extend_from_slice(body);
        bytes
    }

    #[test]
    fn apply_copies_installs_strips_and_records() {
        let f = fixture();
        let mut bottle = f.bottle.clone();
        std::fs::create_dir_all(&bottle.path).unwrap();
        let dxvk = component(&f.root, ComponentKind::Dxvk, DXVK_FILES);
        std::fs::write(
            dxvk.path.join("x86_64-windows/d3d11.dll"),
            builtin_dll(b"d3d11 x64"),
        )
        .unwrap();
        let activation = activation(
            Dxvk,
            Some(&dxvk),
            &wine(&[]),
            &bottle,
            BackendOptions::default(),
        )
        .unwrap();

        apply_copies(&activation, &mut bottle).unwrap();

        let d3d11 = std::fs::read(bottle.system32().join("d3d11.dll")).unwrap();
        assert_eq!(&d3d11[..0x40], &builtin_dll(b"")[..0x40]);
        assert!(
            d3d11[0x40..0x50].iter().all(|b| *b == 0),
            "marker not stripped: {d3d11:?}"
        );
        assert!(d3d11.ends_with(b"d3d11 x64"));

        let active = bottle.state.active.clone().expect("recorded");
        assert_eq!(active.backend, Dxvk);
        assert_eq!(active.version, "1.0");
        assert_eq!(active.dlls.len(), 4);
        assert_eq!(
            active.dlls[0].path,
            Path::new("drive_c/windows/system32/d3d11.dll")
        );
        assert_eq!(active.dlls[0].sha256, sha256_hex(&d3d11));

        let saved: BottleState =
            toml::from_str(&std::fs::read_to_string(bottle.path.join(STATE_FILE)).unwrap())
                .unwrap();
        assert_eq!(saved, bottle.state);
        assert!(
            !bottle.path.join(CONFIG_FILE).exists(),
            "only the state is written"
        );
    }

    #[test]
    fn apply_copies_is_idempotent() {
        let f = fixture();
        let mut bottle = f.bottle.clone();
        std::fs::create_dir_all(&bottle.path).unwrap();
        let dxmt = component(&f.root, ComponentKind::Dxmt, DXMT_FILES);
        let activation = activation(
            Dxmt,
            Some(&dxmt),
            &wine(&["renderer-dllpath"]),
            &bottle,
            BackendOptions::default(),
        )
        .unwrap();

        apply_copies(&activation, &mut bottle).unwrap();
        let state_file = bottle.path.join(STATE_FILE);
        std::fs::remove_file(&state_file).unwrap();
        apply_copies(&activation, &mut bottle).unwrap();
        assert!(!state_file.exists(), "an unchanged state is not rewritten");
        assert_eq!(bottle.state.active.as_ref().map(|a| a.dlls.len()), Some(2));
    }

    // ----- overrides -----

    #[test]
    fn render_overrides_groups_by_value() {
        let rendered = render_overrides(&overrides(&[
            ("dxgi", "n,b"),
            ("d3d11", "n,b"),
            ("winemenubuilder.exe", "d"),
            ("d3d10core", "n,b"),
            ("mscoree", "d"),
        ]));
        assert_eq!(
            rendered,
            "mscoree,winemenubuilder.exe=d;d3d10core,d3d11,dxgi=n,b"
        );
    }

    #[test]
    fn render_overrides_orders_groups_by_value() {
        let rendered = render_overrides(&overrides(&[
            ("f", "n,b"),
            ("e", "n"),
            ("d", "d"),
            ("c", "b,n"),
            ("b", "b"),
            ("a", ""),
        ]));
        assert_eq!(rendered, "a=;b=b;c=b,n;d=d;e=n;f=n,b");
    }

    #[test]
    fn render_overrides_of_nothing_is_empty() {
        assert_eq!(render_overrides(&BTreeMap::new()), "");
        assert_eq!(render_overrides(&overrides(&[("d3d12", "")])), "d3d12=");
    }

    /// Wine's reading of `WINEDLLOVERRIDES`.
    fn parse_overrides(text: &str) -> BTreeMap<String, String> {
        let mut parsed = BTreeMap::new();
        for group in text.split(';').filter(|group| !group.is_empty()) {
            let (names, value) = group.split_once('=').unwrap();
            for name in names.split(',') {
                parsed.insert(name.to_owned(), value.to_owned());
            }
        }
        parsed
    }

    proptest! {
        #[test]
        fn render_overrides_round_trips(
            entries in prop::collection::btree_map(
                "[a-z0-9_.]{1,12}",
                prop::sample::select(vec!["n", "b", "n,b", "b,n", "d", ""]),
                0..12,
            )
        ) {
            let entries: BTreeMap<String, String> =
                entries.into_iter().map(|(k, v)| (k, v.to_owned())).collect();
            let rendered = render_overrides(&entries);
            prop_assert_eq!(parse_overrides(&rendered), entries);
        }
    }
}
