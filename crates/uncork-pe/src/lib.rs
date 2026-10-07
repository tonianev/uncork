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

mod image;
mod scan;
mod strings;

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

impl PeError {
    /// Name the file a parse error came from, so the message stands on its
    /// own (`Io` errors already carry their path).
    fn in_file(self, path: &Path) -> Self {
        match self {
            Self::NotPe(reason) => Self::NotPe(format!("{}: {reason}", path.display())),
            Self::Malformed(reason) => Self::Malformed(format!("{}: {reason}", path.display())),
            io @ Self::Io { .. } => io,
        }
    }
}

/// Read a whole file, attaching its path to any error.
fn read_file(path: &Path) -> Result<Vec<u8>, PeError> {
    std::fs::read(path).map_err(|source| PeError::Io {
        path: path.to_path_buf(),
        source,
    })
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

impl Machine {
    fn from_raw(raw: u16) -> Self {
        match object::pe::Machine(raw) {
            object::pe::IMAGE_FILE_MACHINE_I386 => Self::X86,
            object::pe::IMAGE_FILE_MACHINE_AMD64 => Self::X86_64,
            object::pe::IMAGE_FILE_MACHINE_ARM64 => Self::Arm64,
            _ => Self::Other(raw),
        }
    }

    fn raw(self) -> u16 {
        match self {
            Self::X86 => object::pe::IMAGE_FILE_MACHINE_I386.0,
            Self::X86_64 => object::pe::IMAGE_FILE_MACHINE_AMD64.0,
            Self::Arm64 => object::pe::IMAGE_FILE_MACHINE_ARM64.0,
            Self::Other(raw) => raw,
        }
    }

    fn bitness(self) -> Option<Bitness> {
        match self {
            Self::X86 => Some(Bitness::X86),
            Self::X86_64 => Some(Bitness::X64),
            Self::Arm64 | Self::Other(_) => None,
        }
    }

    /// Human-readable name with the raw value, for error messages.
    fn describe(self) -> String {
        let name = match self {
            Self::X86 => "x86",
            Self::X86_64 => "x86-64",
            Self::Arm64 => "ARM64",
            Self::Other(_) => "unknown machine",
        };
        format!("{name} ({:#06x})", self.raw())
    }
}

/// Pointer width of a Windows program, which decides whether Wine runs it
/// through its 32-bit (`WoW64`) or 64-bit path and which DLL directory
/// (`syswow64` or `system32`) a graphics backend must be installed into.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize,
)]
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
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum GraphicsApi {
    /// `ddraw.dll` (`DirectDraw` / Direct3D 7 and earlier).
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

impl GraphicsApi {
    /// Every Direct3D version, `DirectDraw` included, as opposed to the
    /// cross-platform APIs that many Direct3D games also load.
    fn is_direct3d(self) -> bool {
        !matches!(self, Self::OpenGl | Self::Vulkan)
    }
}

/// The DLL file names (lowercase) that identify each [`GraphicsApi`].
const GRAPHICS_DLLS: &[(&str, GraphicsApi)] = &[
    ("ddraw.dll", GraphicsApi::DirectDraw),
    ("opengl32.dll", GraphicsApi::OpenGl),
    ("d3d8.dll", GraphicsApi::D3d8),
    ("d3d9.dll", GraphicsApi::D3d9),
    ("d3d10.dll", GraphicsApi::D3d10),
    ("d3d10_1.dll", GraphicsApi::D3d10),
    ("d3d10core.dll", GraphicsApi::D3d10),
    ("d3d11.dll", GraphicsApi::D3d11),
    ("d3d12.dll", GraphicsApi::D3d12),
    ("vulkan-1.dll", GraphicsApi::Vulkan),
];

/// The API a DLL file name indicates (case-insensitive, exact name).
fn api_for_dll(name: &str) -> Option<GraphicsApi> {
    GRAPHICS_DLLS
        .iter()
        .find(|(dll, _)| dll.eq_ignore_ascii_case(name))
        .map(|&(_, api)| api)
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
    /// `WoW64` a single non-NX module can disable no-exec for the whole
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
    let padding = bytes.get(WINE_MARKER_OFFSET..)?;
    if padding.starts_with(WINE_BUILTIN_SIGNATURE) {
        Some(WineMarker::Builtin)
    } else if padding.starts_with(WINE_PLACEHOLDER_SIGNATURE) {
        Some(WineMarker::Placeholder)
    } else {
        None
    }
}

/// Turn a Wine builtin DLL into one Wine treats as native by zeroing the
/// builtin signature at [`WINE_MARKER_OFFSET`] (the bytes are DOS-stub
/// padding; nothing else reads them). Returns `true` if a marker was
/// removed, `false` if there was none. Placeholders are left alone.
pub fn strip_builtin_marker(bytes: &mut [u8]) -> bool {
    let marker_range = WINE_MARKER_OFFSET..WINE_MARKER_OFFSET + WINE_BUILTIN_SIGNATURE.len();
    match bytes.get_mut(marker_range) {
        Some(signature) if *signature == WINE_BUILTIN_SIGNATURE[..] => {
            signature.fill(0);
            true
        }
        _ => false,
    }
}

/// Parse a PE image from memory.
///
/// # Errors
/// [`PeError::NotPe`] when the signatures are missing, [`PeError::Malformed`]
/// when an import table cannot be read.
pub fn inspect_bytes(bytes: &[u8]) -> Result<PeInfo, PeError> {
    image::parse(bytes)
}

/// Read and parse a PE file.
///
/// # Errors
/// [`PeError::Io`] if the file cannot be read, otherwise as [`inspect_bytes`].
pub fn inspect(path: &Path) -> Result<PeInfo, PeError> {
    let bytes = read_file(path)?;
    inspect_bytes(&bytes).map_err(|err| err.in_file(path))
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
    imports
        .into_iter()
        .filter_map(|name| api_for_dll(name.as_ref()))
        .collect()
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
        self.info
            .bitness
            .expect("scan_game only accepts x86/x86-64 images")
    }

    /// The newest Direct3D API detected, or `None` when no Direct3D import
    /// was found. Uses the [`GraphicsApi`] order but ignores `OpenGl` and
    /// `Vulkan` unless nothing else was found, in which case it returns the
    /// greatest of those.
    #[must_use]
    pub fn primary_api(&self) -> Option<GraphicsApi> {
        // Without any Direct3D API, every detected API is OpenGL or Vulkan,
        // so the set's maximum is the greatest of those.
        self.apis
            .iter()
            .rfind(|api| api.is_direct3d())
            .or_else(|| self.apis.last())
            .copied()
    }
}

/// Graphics DLL names that appear as strings in `bytes`, as ASCII or
/// UTF-16LE, matched case-insensitively and only when delimited by a
/// non-identifier byte on both sides (so `myd3d9.dll` does not count as
/// `d3d9.dll`). Returns `(lowercase dll name, api)` pairs, sorted, deduplicated.
#[must_use]
pub fn string_dll_refs(bytes: &[u8]) -> Vec<(String, GraphicsApi)> {
    strings::graphics_dll_refs(bytes)
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
    scan::scan_game(exe)
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// `len` bytes of DOS stub with `signature` written at the marker offset.
    fn with_signature(signature: &[u8], len: usize) -> Vec<u8> {
        let mut bytes = vec![0xcc; len];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[WINE_MARKER_OFFSET..WINE_MARKER_OFFSET + signature.len()].copy_from_slice(signature);
        bytes
    }

    #[test]
    fn detects_the_builtin_marker() {
        let bytes = with_signature(WINE_BUILTIN_SIGNATURE, 0x200);
        assert_eq!(wine_marker(&bytes), Some(WineMarker::Builtin));
    }

    #[test]
    fn detects_the_placeholder_marker() {
        let bytes = with_signature(WINE_PLACEHOLDER_SIGNATURE, 0x200);
        assert_eq!(wine_marker(&bytes), Some(WineMarker::Placeholder));
    }

    #[test]
    fn markers_may_end_exactly_at_the_end_of_the_buffer() {
        let builtin = with_signature(WINE_BUILTIN_SIGNATURE, 0x50);
        assert_eq!(wine_marker(&builtin), Some(WineMarker::Builtin));
        let placeholder = with_signature(WINE_PLACEHOLDER_SIGNATURE, 0x54);
        assert_eq!(wine_marker(&placeholder), Some(WineMarker::Placeholder));
    }

    #[test]
    fn truncated_markers_do_not_count() {
        let builtin = with_signature(WINE_BUILTIN_SIGNATURE, 0x50);
        assert_eq!(wine_marker(&builtin[..0x4f]), None);
        let placeholder = with_signature(WINE_PLACEHOLDER_SIGNATURE, 0x54);
        assert_eq!(wine_marker(&placeholder[..0x53]), None);
        assert_eq!(wine_marker(&[]), None);
        assert_eq!(wine_marker(b"MZ"), None);
    }

    #[test]
    fn markers_elsewhere_do_not_count() {
        let mut bytes = vec![0; 0x200];
        bytes[0x41..0x51].copy_from_slice(WINE_BUILTIN_SIGNATURE);
        assert_eq!(wine_marker(&bytes), None);
        bytes[0x40] = b'w';
        assert_eq!(wine_marker(&bytes), None);
    }

    #[test]
    fn marker_matching_is_case_sensitive() {
        let bytes = with_signature(b"wine builtin dll", 0x200);
        assert_eq!(wine_marker(&bytes), None);
    }

    #[test]
    fn strip_zeroes_only_the_builtin_signature() {
        let original = with_signature(WINE_BUILTIN_SIGNATURE, 0x200);
        let mut bytes = original.clone();
        assert!(strip_builtin_marker(&mut bytes));
        assert_eq!(wine_marker(&bytes), None);
        assert_eq!(bytes[0x40..0x50], [0; 16]);
        assert_eq!(bytes[..0x40], original[..0x40]);
        assert_eq!(bytes[0x50..], original[0x50..]);
    }

    #[test]
    fn strip_is_idempotent() {
        let mut bytes = with_signature(WINE_BUILTIN_SIGNATURE, 0x200);
        assert!(strip_builtin_marker(&mut bytes));
        let stripped = bytes.clone();
        assert!(!strip_builtin_marker(&mut bytes));
        assert_eq!(bytes, stripped);
    }

    #[test]
    fn strip_leaves_placeholders_alone() {
        let original = with_signature(WINE_PLACEHOLDER_SIGNATURE, 0x200);
        let mut bytes = original.clone();
        assert!(!strip_builtin_marker(&mut bytes));
        assert_eq!(bytes, original);
    }

    #[test]
    fn strip_handles_buffers_too_short_for_a_marker() {
        let mut empty: [u8; 0] = [];
        assert!(!strip_builtin_marker(&mut empty));
        let mut exact = with_signature(WINE_BUILTIN_SIGNATURE, 0x50);
        let mut truncated = exact[..0x4f].to_vec();
        assert!(!strip_builtin_marker(&mut truncated));
        assert!(strip_builtin_marker(&mut exact));
    }

    #[test]
    fn imports_map_to_their_apis() {
        let cases = [
            ("ddraw.dll", GraphicsApi::DirectDraw),
            ("opengl32.dll", GraphicsApi::OpenGl),
            ("d3d8.dll", GraphicsApi::D3d8),
            ("d3d9.dll", GraphicsApi::D3d9),
            ("d3d10.dll", GraphicsApi::D3d10),
            ("d3d10_1.dll", GraphicsApi::D3d10),
            ("d3d10core.dll", GraphicsApi::D3d10),
            ("d3d11.dll", GraphicsApi::D3d11),
            ("d3d12.dll", GraphicsApi::D3d12),
            ("vulkan-1.dll", GraphicsApi::Vulkan),
        ];
        for (dll, api) in cases {
            assert_eq!(apis_from_imports([dll]), BTreeSet::from([api]), "{dll}");
        }
    }

    #[test]
    fn import_matching_ignores_case() {
        assert_eq!(
            apis_from_imports(["D3D11.DLL", "OpenGL32.dll"]),
            BTreeSet::from([GraphicsApi::OpenGl, GraphicsApi::D3d11])
        );
    }

    #[test]
    fn dxgi_alone_adds_nothing() {
        assert!(apis_from_imports(["dxgi.dll"]).is_empty());
        assert_eq!(
            apis_from_imports(["dxgi.dll", "d3d12.dll"]),
            BTreeSet::from([GraphicsApi::D3d12])
        );
    }

    #[test]
    fn import_matching_needs_the_exact_file_name() {
        let near_misses = [
            "d3d9",
            "d3d9.dll ",
            "myd3d9.dll",
            "d3d9.dll.bak",
            "d3dx9_43.dll",
            "d3dcompiler_47.dll",
            "d3d12core.dll",
            "kernel32.dll",
            "",
        ];
        assert!(apis_from_imports(near_misses).is_empty());
    }

    #[test]
    fn imports_accept_owned_strings() {
        let imports = vec!["kernel32.dll".to_owned(), "d3d9.dll".to_owned()];
        assert_eq!(
            apis_from_imports(&imports),
            BTreeSet::from([GraphicsApi::D3d9])
        );
    }

    fn scan_with(apis: &[GraphicsApi]) -> GameScan {
        GameScan {
            exe: PathBuf::from("game.exe"),
            info: PeInfo {
                machine: Machine::X86,
                bitness: Some(Bitness::X86),
                is_dll: false,
                large_address_aware: false,
                nx_compat: true,
                subsystem: 2,
                wine_marker: None,
                imports: Vec::new(),
            },
            apis: apis.iter().copied().collect(),
            evidence: Vec::new(),
            skipped: Vec::new(),
            uses_steamworks: false,
            anti_cheat: Vec::new(),
            non_nx_modules: Vec::new(),
        }
    }

    #[test]
    fn primary_api_is_the_newest_direct3d() {
        use GraphicsApi::*;
        assert_eq!(scan_with(&[D3d9, D3d11]).primary_api(), Some(D3d11));
        assert_eq!(scan_with(&[D3d8, D3d9, D3d10]).primary_api(), Some(D3d10));
        assert_eq!(scan_with(&[DirectDraw, D3d12]).primary_api(), Some(D3d12));
        assert_eq!(scan_with(&[DirectDraw]).primary_api(), Some(DirectDraw));
    }

    #[test]
    fn primary_api_prefers_direct3d_over_opengl_and_vulkan() {
        use GraphicsApi::*;
        assert_eq!(scan_with(&[OpenGl, D3d9]).primary_api(), Some(D3d9));
        assert_eq!(scan_with(&[D3d11, Vulkan]).primary_api(), Some(D3d11));
        assert_eq!(
            scan_with(&[DirectDraw, OpenGl, Vulkan]).primary_api(),
            Some(DirectDraw)
        );
    }

    #[test]
    fn primary_api_falls_back_to_opengl_or_vulkan() {
        use GraphicsApi::*;
        assert_eq!(scan_with(&[OpenGl]).primary_api(), Some(OpenGl));
        assert_eq!(scan_with(&[Vulkan]).primary_api(), Some(Vulkan));
        assert_eq!(scan_with(&[OpenGl, Vulkan]).primary_api(), Some(Vulkan));
    }

    #[test]
    fn primary_api_is_none_without_any_api() {
        assert_eq!(scan_with(&[]).primary_api(), None);
    }

    #[test]
    fn scan_bitness_comes_from_the_executable() {
        assert_eq!(scan_with(&[]).bitness(), Bitness::X86);
    }

    #[test]
    fn machines_round_trip_through_their_raw_values() {
        for raw in [0x014c, 0x8664, 0xaa64, 0x01c4, 0xa641, 0] {
            assert_eq!(Machine::from_raw(raw).raw(), raw);
        }
        assert_eq!(Machine::from_raw(0x014c), Machine::X86);
        assert_eq!(Machine::from_raw(0x8664), Machine::X86_64);
        assert_eq!(Machine::from_raw(0xaa64), Machine::Arm64);
        assert_eq!(Machine::from_raw(0x01c4), Machine::Other(0x01c4));
    }

    #[test]
    fn only_x86_machines_have_a_bitness() {
        assert_eq!(Machine::X86.bitness(), Some(Bitness::X86));
        assert_eq!(Machine::X86_64.bitness(), Some(Bitness::X64));
        assert_eq!(Machine::Arm64.bitness(), None);
        assert_eq!(Machine::Other(0x01c4).bitness(), None);
    }

    #[test]
    fn machine_descriptions_name_the_raw_value() {
        assert_eq!(Machine::X86.describe(), "x86 (0x014c)");
        assert_eq!(Machine::X86_64.describe(), "x86-64 (0x8664)");
        assert_eq!(Machine::Arm64.describe(), "ARM64 (0xaa64)");
        assert_eq!(
            Machine::Other(0x01c4).describe(),
            "unknown machine (0x01c4)"
        );
    }

    #[test]
    fn parse_errors_gain_the_file_path() {
        let path = Path::new("/games/x/game.exe");
        let not_pe = PeError::NotPe("missing MZ signature".to_owned()).in_file(path);
        assert_eq!(
            not_pe.to_string(),
            "not a PE image: /games/x/game.exe: missing MZ signature"
        );
        let malformed = PeError::Malformed("import table: bad".to_owned()).in_file(path);
        assert_eq!(
            malformed.to_string(),
            "malformed PE image: /games/x/game.exe: import table: bad"
        );
    }

    #[test]
    fn io_errors_keep_their_own_path() {
        let path = Path::new("/games/x/game.exe");
        let io = PeError::Io {
            path: path.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        }
        .in_file(Path::new("/elsewhere"));
        assert!(matches!(io, PeError::Io { path: p, .. } if p == path));
    }

    #[test]
    fn read_file_errors_carry_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("missing.exe");
        match read_file(&missing) {
            Err(PeError::Io { path, source }) => {
                assert_eq!(path, missing);
                assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
            }
            other => panic!("expected an I/O error, got {other:?}"),
        }
    }

    proptest! {
        #[test]
        fn wine_marker_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..256)) {
            let _ = wine_marker(&bytes);
        }

        #[test]
        fn strip_removes_exactly_the_builtin_marker(
            mut bytes in proptest::collection::vec(any::<u8>(), 0..256),
            plant in any::<bool>(),
        ) {
            if plant && bytes.len() >= 0x50 {
                bytes[0x40..0x50].copy_from_slice(WINE_BUILTIN_SIGNATURE);
            }
            let original = bytes.clone();
            let marker = wine_marker(&bytes);
            let stripped = strip_builtin_marker(&mut bytes);

            prop_assert_eq!(stripped, marker == Some(WineMarker::Builtin));
            prop_assert_ne!(wine_marker(&bytes), Some(WineMarker::Builtin));
            if stripped {
                prop_assert_eq!(&bytes[..0x40], &original[..0x40]);
                prop_assert_eq!(&bytes[0x40..0x50], &[0; 16]);
                prop_assert_eq!(&bytes[0x50..], &original[0x50..]);
            } else {
                prop_assert_eq!(&bytes, &original);
            }
        }
    }
}
