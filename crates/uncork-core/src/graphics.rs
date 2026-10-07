//! Graphics backends: which Direct3D translation layer a process uses, and
//! how Uncork makes Wine load it.
//!
//! # Backends
//!
//! | Backend | Translates | Bitness | Source |
//! |---|---|---|---|
//! | [`Backend::Dxmt`] | D3D10, D3D11 → Metal | 32 and 64 | `dxmt` component |
//! | [`Backend::Dxvk`] | D3D10, D3D11 → Vulkan → MoltenVK → Metal | 32 and 64 | `dxvk` component |
//! | [`Backend::D3dMetal`] | D3D11, D3D12 (D3D10 from GPTK 4) → Metal | 64 only | user-imported `d3dmetal` component |
//! | [`Backend::WineD3d`] | DirectDraw, D3D8, D3D9, D3D10, D3D11 → OpenGL | 32 and 64 | built into Wine |
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
use uncork_pe::{Bitness, GraphicsApi};

use crate::bottle::Bottle;
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
    pub const ALL: [Backend; 4] = [Backend::Dxmt, Backend::D3dmetal, Backend::Dxvk, Backend::Wined3d];

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
        let _ = s;
        todo!()
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
            s.parse().map(BackendChoice::Fixed).map_err(serde::de::Error::custom)
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
    let _ = (backend, api, bitness);
    todo!()
}

/// What is installed, for [`recommend`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Availability {
    /// A `dxmt` component is installed and the Wine runtime has the `dxmt` feature.
    pub dxmt: bool,
    /// A `dxvk` component is installed.
    pub dxvk: bool,
    /// A `d3dmetal` component is installed and the Wine runtime has the `d3dmetal` feature.
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
    let _ = (api, bitness, available, preferred, fallbacks, needs_geometry_shaders);
    todo!()
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
    let _ = (backend, wine);
    todo!()
}

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
    let _ = backend;
    todo!()
}

/// The directory to pass for `RendererEnv`/`DllPathPrepend`: the component
/// directory itself for DXMT and DXVK (it contains [`ARCH_DIRS`]); `wine/`
/// inside the component for D3DMetal (GPTK's `redist/lib/wine`).
#[must_use]
pub fn backend_dir(backend: Backend, component: &InstalledComponent) -> PathBuf {
    let _ = (backend, component);
    todo!()
}

/// One file to put into the prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DllCopy {
    /// Source file inside the component.
    pub source: PathBuf,
    /// Destination relative to the bottle, e.g. `drive_c/windows/syswow64/d3d11.dll`.
    pub dest: PathBuf,
    /// Remove Wine's builtin marker while copying (PrefixNative backend DLLs).
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
    let _ = (backend, component, wine, bottle, options);
    todo!()
}

/// Perform `activation.copies` (creating directories, skipping identical
/// files, stripping markers with [`uncork_pe::strip_builtin_marker`]) and
/// record them in the bottle state. Never deletes Wine placeholder DLLs.
///
/// # Errors
/// [`crate::Error::Io`] or [`crate::Error::Pe`].
pub fn apply_copies(activation: &Activation, bottle: &mut Bottle) -> crate::Result<()> {
    let _ = (activation, bottle);
    todo!()
}

/// Render overrides for `WINEDLLOVERRIDES`: entries grouped by value, groups
/// ordered by value, names sorted within a group, e.g.
/// `d3d10core,d3d11,dxgi=n,b;mscoree=d;winemenubuilder.exe=d`. An empty
/// value renders as `name=` (disabled).
#[must_use]
pub fn render_overrides(overrides: &BTreeMap<String, String>) -> String {
    let _ = overrides;
    todo!()
}

/// `true` if `dir` looks like a backend directory (has at least one of
/// [`ARCH_DIRS`]).
#[must_use]
pub fn is_backend_dir(dir: &Path) -> bool {
    ARCH_DIRS.iter().any(|arch| dir.join(arch).is_dir())
}
