//! Inspect Windows PE executables without running them.
//!
//! Uncork uses this crate to answer three questions about a game before it
//! launches it: is it 32-bit or 64-bit, which graphics API does it render
//! with, and is it large-address-aware. The answers drive the graphics
//! backend choice in `uncork-core` (see `docs/ARCHITECTURE.md`).
//!
//! Many games keep their renderer in a DLL next to the executable (Rise of
//! Nations: Extended Edition imports `d3d11.dll` from `d3dgl.dll`, not from
//! `riseofnations.exe`), so [`scan_game`] inspects the executable *and* the
//! DLLs in its directory.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Errors from reading or parsing a PE file.
#[derive(Debug, thiserror::Error)]
pub enum PeError {
    /// The file could not be read.
    #[error("cannot read {path}: {source}")]
    Io {
        /// File that failed.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },
    /// The bytes are not a PE image (missing `MZ`/`PE\0\0` signatures or a truncated header).
    #[error("not a PE image: {0}")]
    NotPe(String),
    /// The PE headers parse but a table is malformed.
    #[error("malformed PE image: {0}")]
    Malformed(String),
}

/// CPU architecture from the COFF `Machine` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Machine {
    /// `IMAGE_FILE_MACHINE_I386` (0x014c): 32-bit x86.
    X86,
    /// `IMAGE_FILE_MACHINE_AMD64` (0x8664): 64-bit x86.
    X86_64,
    /// `IMAGE_FILE_MACHINE_ARM64` (0xaa64).
    Arm64,
    /// Any other machine value.
    Other(u16),
}

/// Pointer width of a Windows program, which decides whether Wine runs it
/// through its 32-bit (WoW64) or 64-bit path and which DLL directory
/// (`syswow64` or `system32`) a graphics backend must be installed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Bitness {
    /// 32-bit (PE32, `i386-windows`).
    X86,
    /// 64-bit (PE32+, `x86_64-windows`).
    X64,
}

/// A graphics API, detected from imported DLL names.
///
/// The derived order is oldest to newest; [`GameScan::primary_api`] returns the
/// greatest detected value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsApi {
    /// `ddraw.dll` (DirectDraw / Direct3D 7 and earlier).
    DirectDraw,
    /// `opengl32.dll`.
    OpenGl,
    /// `d3d8.dll`.
    D3d8,
    /// `d3d9.dll`.
    D3d9,
    /// `d3d10.dll`, `d3d10_1.dll`, `d3d10core.dll`.
    D3d10,
    /// `d3d11.dll`.
    D3d11,
    /// `d3d12.dll`.
    D3d12,
    /// `vulkan-1.dll`.
    Vulkan,
}

/// What [`inspect`] learns from one PE file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PeInfo {
    /// COFF machine.
    pub machine: Machine,
    /// `Some` for x86 and x86-64 images, `None` for other machines.
    pub bitness: Option<Bitness>,
    /// `IMAGE_FILE_DLL` is set.
    pub is_dll: bool,
    /// `IMAGE_FILE_LARGE_ADDRESS_AWARE` (0x0020) is set. Matters for 32-bit
    /// games: without it they get a 2 GiB address space.
    pub large_address_aware: bool,
    /// `IMAGE_DLLCHARACTERISTICS_NX_COMPAT` (0x0100) is set. Under Wine's
    /// WoW64 a single non-NX module can disable no-exec for the whole
    /// process, which is very slow under Rosetta.
    pub nx_compat: bool,
    /// Optional-header subsystem value (2 = Windows GUI, 3 = console).
    pub subsystem: u16,
    /// Wine's marker at file offset 0x40, if present.
    pub wine_marker: Option<WineMarker>,
    /// Imported DLL names from the import table *and* the delay-load import
    /// table, lowercased, deduplicated and sorted.
    pub imports: Vec<String>,
}

/// Wine writes one of two 16-byte ASCII signatures right after the
/// 64-byte DOS header (file offset 0x40) of the PE files it builds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum WineMarker {
    /// `Wine builtin DLL`: a real Wine-built module, loaded as builtin.
    Builtin,
    /// `Wine placeholder DLL`: a stub in `system32` that tells the loader to
    /// use the builtin from Wine's library directory.
    Placeholder,
}

/// Offset of Wine's marker in a PE file.
pub const WINE_MARKER_OFFSET: usize = 0x40;
/// `b"Wine builtin DLL"` (16 bytes).
pub const WINE_BUILTIN_SIGNATURE: &[u8; 16] = b"Wine builtin DLL";
/// `b"Wine placeholder DLL"` (20 bytes).
pub const WINE_PLACEHOLDER_SIGNATURE: &[u8; 20] = b"Wine placeholder DLL";

/// Which Wine marker, if any, `bytes` carries at [`WINE_MARKER_OFFSET`].
#[must_use]
pub fn wine_marker(bytes: &[u8]) -> Option<WineMarker> {
    let _ = bytes;
    todo!()
}

/// Turn a Wine builtin DLL into one Wine treats as native by zeroing the
/// builtin signature at [`WINE_MARKER_OFFSET`] (the bytes are DOS-stub
/// padding; nothing else reads them). Returns `true` if a marker was
/// removed, `false` if there was none. Placeholders are left alone.
pub fn strip_builtin_marker(bytes: &mut [u8]) -> bool {
    let _ = bytes;
    todo!()
}

/// Parse a PE image from memory.
///
/// # Errors
/// [`PeError::NotPe`] when the signatures are missing, [`PeError::Malformed`]
/// when an import table cannot be read.
pub fn inspect_bytes(bytes: &[u8]) -> Result<PeInfo, PeError> {
    let _ = bytes;
    todo!()
}

/// Read and parse a PE file.
///
/// # Errors
/// [`PeError::Io`] if the file cannot be read, otherwise as [`inspect_bytes`].
pub fn inspect(path: &Path) -> Result<PeInfo, PeError> {
    let _ = path;
    todo!()
}

/// Map imported DLL names to the graphics APIs they indicate.
///
/// Matching is case-insensitive on the exact file name: `ddraw.dll`,
/// `opengl32.dll`, `d3d8.dll`, `d3d9.dll`, `d3d10.dll`, `d3d10_1.dll`,
/// `d3d10core.dll`, `d3d11.dll`, `d3d12.dll`, `vulkan-1.dll`. `dxgi.dll` alone
/// adds nothing (it is shared by D3D10, 11 and 12).
pub fn apis_from_imports<I, S>(imports: I) -> BTreeSet<GraphicsApi>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let _ = imports.into_iter().map(|s| s.as_ref().len()).count();
    todo!()
}

/// DLL file names (lowercase) that are redistributables or helpers rather than
/// a game's renderer. [`scan_game`] skips them because they import graphics
/// DLLs themselves (for example `d3dx9_43.dll` imports `d3d9.dll`) and would
/// produce false positives.
pub const IGNORED_DLL_PREFIXES: &[&str] = &[
    "d3dx9_",
    "d3dx10",
    "d3dx11_",
    "d3dcompiler_",
    "d3dcsx",
    "xinput",
    "x3daudio",
    "xaudio",
    "xapofx",
    "steam_api",
    "dxvk",
    "openvr_api",
    "nvapi",
    "amd_ags",
    "galaxy",
    "eossdk",
    "discord",
    "msvcp",
    "msvcr",
    "vcruntime",
    "api-ms-win-",
    "ucrtbase",
    "concrt",
    "vccorlib",
    "mfc",
];

/// One piece of evidence for a detected API: which file imported which DLL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct ApiEvidence {
    /// File name (not full path) of the inspected PE that has the import.
    pub file: String,
    /// The imported DLL, lowercase.
    pub import: String,
    /// The API it indicates.
    pub api: GraphicsApi,
    /// Import table or string reference.
    pub kind: EvidenceKind,
}

/// How a piece of [`ApiEvidence`] was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EvidenceKind {
    /// Import or delay-load import table.
    Import,
    /// A DLL name found as an ASCII or UTF-16LE string (a `LoadLibrary` target).
    String,
}

/// Result of [`scan_game`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GameScan {
    /// The executable that was scanned.
    pub exe: PathBuf,
    /// Its PE information.
    pub info: PeInfo,
    /// Every API detected in the executable or a sibling DLL.
    pub apis: BTreeSet<GraphicsApi>,
    /// Where each API was seen, sorted.
    pub evidence: Vec<ApiEvidence>,
    /// Sibling DLLs that failed to parse or had a different bitness and were
    /// skipped, with the reason. Never fatal.
    pub skipped: Vec<(String, String)>,
    /// `steam_api.dll` or `steam_api64.dll` is imported or present next to
    /// the executable: the game expects a running Steam client.
    pub uses_steamworks: bool,
    /// Anti-cheat files found next to the executable (`EasyAntiCheat*`,
    /// `BEService*`, `BattlEye*`, `vgk*`, file names lowercase). Kernel
    /// anti-cheat does not run under Wine; Uncork warns.
    pub anti_cheat: Vec<String>,
    /// File names of inspected modules without `NX_COMPAT` (see [`PeInfo::nx_compat`]).
    pub non_nx_modules: Vec<String>,
}

impl GameScan {
    /// Bitness of the executable.
    ///
    /// # Panics
    /// Never for scans produced by [`scan_game`], which rejects non-x86 images.
    #[must_use]
    pub fn bitness(&self) -> Bitness {
        self.info.bitness.expect("scan_game only accepts x86/x86-64 images")
    }

    /// The newest Direct3D API detected, or `None` when no Direct3D import
    /// was found. Uses the [`GraphicsApi`] order but ignores `OpenGl` and
    /// `Vulkan` unless nothing else was found, in which case it returns the
    /// greatest of those.
    #[must_use]
    pub fn primary_api(&self) -> Option<GraphicsApi> {
        todo!()
    }
}

/// Graphics DLL names that appear as strings in `bytes`, as ASCII or
/// UTF-16LE, matched case-insensitively and only when delimited by a
/// non-identifier byte on both sides (so `myd3d9.dll` does not count as
/// `d3d9.dll`). Returns `(lowercase dll name, api)` pairs, sorted, deduplicated.
#[must_use]
pub fn string_dll_refs(bytes: &[u8]) -> Vec<(String, GraphicsApi)> {
    let _ = bytes;
    todo!()
}

/// Maximum number of sibling DLLs [`scan_game`] inspects. Directories with
/// more DLLs are truncated in file-name order and a `skipped` entry says so.
pub const MAX_SIBLING_DLLS: usize = 256;

/// Largest sibling DLL [`scan_game`] reads, in bytes. Bigger files are skipped.
pub const MAX_DLL_BYTES: u64 = 128 * 1024 * 1024;

/// Inspect a game executable and the `.dll` files in the same directory
/// (non-recursive, case-insensitive extension match, sorted by file name,
/// [`IGNORED_DLL_PREFIXES`] skipped, DLLs whose bitness differs from the
/// executable skipped).
///
/// Evidence comes from import tables of every inspected module and, for the
/// executable and each inspected DLL, from [`string_dll_refs`] (renderers
/// loaded with `LoadLibrary`). Directory entries (files and subdirectories,
/// case-insensitive) also feed `uses_steamworks` and `anti_cheat`.
///
/// # Errors
/// Fails only if the executable itself cannot be read or parsed, or if it is
/// not an x86/x86-64 image ([`PeError::NotPe`] with a message naming the
/// machine).
pub fn scan_game(exe: &Path) -> Result<GameScan, PeError> {
    let _ = exe;
    todo!()
}
