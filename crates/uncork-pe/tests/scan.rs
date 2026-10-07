//! `scan_game` against fake game directories built from the test PE builder.

mod common;

use std::fs;
use std::path::{Path, PathBuf};

use common::{IMAGE_FILE_32BIT_MACHINE, IMAGE_FILE_EXECUTABLE_IMAGE, MACHINE_ARM64, PeBuilder};
use tempfile::TempDir;
use uncork_pe::{
    ApiEvidence, Bitness, EvidenceKind, GameScan, GraphicsApi, MAX_DLL_BYTES, MAX_SIBLING_DLLS,
    PeError, inspect, scan_game,
};

/// A temporary game directory.
struct GameDir {
    dir: TempDir,
}

impl GameDir {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("tempdir"),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(name)
    }

    fn pe(&self, name: &str, builder: &PeBuilder) -> PathBuf {
        let path = self.path(name);
        builder.write(&path);
        path
    }

    fn file(&self, name: &str, contents: &[u8]) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, contents).expect("write file");
        path
    }

    fn subdir(&self, name: &str) {
        fs::create_dir(self.path(name)).expect("create dir");
    }

    fn scan(&self, exe: &str) -> GameScan {
        scan_game(&self.path(exe)).expect("scan succeeds")
    }
}

fn evidence(file: &str, import: &str, api: GraphicsApi, kind: EvidenceKind) -> ApiEvidence {
    ApiEvidence {
        file: file.to_owned(),
        import: import.to_owned(),
        api,
        kind,
    }
}

fn skip_reason<'a>(scan: &'a GameScan, file: &str) -> &'a str {
    scan.skipped
        .iter()
        .find(|(name, _)| name == file)
        .map(|(_, reason)| reason.as_str())
        .unwrap_or_else(|| panic!("{file} not in skipped: {:?}", scan.skipped))
}

fn dll32() -> PeBuilder {
    PeBuilder::pe32().dll()
}

fn dll64() -> PeBuilder {
    PeBuilder::pe64().dll()
}

/// The layout of Rise of Nations: Extended Edition, reduced to what matters.
#[test]
fn rise_of_nations_layout() {
    let game = GameDir::new();
    let exe = game.pe(
        "riseofnations.exe",
        &PeBuilder::pe32()
            .characteristics(IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_32BIT_MACHINE)
            .dll_characteristics(0x8140)
            .import("KERNEL32.dll")
            .import("MSVCP140.dll")
            .data(b"GraphicsDLL\0d3dgl.dll\0"),
    );
    game.pe(
        "d3dgl.dll",
        &dll32()
            .import("d3d11.dll")
            .import("D3DCOMPILER_47.dll")
            .import("KERNEL32.dll"),
    );
    // Redistributables that import graphics DLLs themselves.
    game.pe("d3dx9_43.dll", &dll32().import("d3d9.dll"));
    game.pe("D3DCompiler_47.dll", &dll32().import("d3d11.dll"));
    // Steamworks, present but never inspected.
    game.file("steam_api.dll", b"MZ not really");
    game.file("steam_appid.txt", b"287450");
    // An old bundled library without NX_COMPAT.
    game.pe(
        "unicows.dll",
        &dll32().nx_compat(false).import("kernel32.dll"),
    );
    // A 64-bit helper next to the 32-bit game.
    game.pe("ttv64.dll", &dll64().import("d3d12.dll"));
    // Something named .dll that is not a PE image.
    game.file("readme.dll", b"This is not a DLL.\n");
    game.subdir("shaders");

    let scan = scan_game(&exe).expect("scan succeeds");

    assert_eq!(scan.exe, exe);
    assert_eq!(scan.info, inspect(&exe).expect("inspect"));
    assert_eq!(scan.bitness(), Bitness::X86);
    assert!(!scan.info.large_address_aware);
    assert!(scan.info.nx_compat);
    assert_eq!(
        scan.apis.iter().copied().collect::<Vec<_>>(),
        [GraphicsApi::D3d11]
    );
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d11));
    assert_eq!(
        scan.evidence,
        [evidence(
            "d3dgl.dll",
            "d3d11.dll",
            GraphicsApi::D3d11,
            EvidenceKind::Import
        )]
    );
    assert!(scan.uses_steamworks);
    assert!(scan.anti_cheat.is_empty());
    assert_eq!(scan.non_nx_modules, ["unicows.dll"]);

    let skipped: Vec<&str> = scan.skipped.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(skipped, ["readme.dll", "ttv64.dll"]);
    assert_eq!(
        skip_reason(&scan, "readme.dll"),
        "not a PE image: missing MZ signature"
    );
    assert_eq!(
        skip_reason(&scan, "ttv64.dll"),
        "x86-64 (0x8664) module, but the executable is 32-bit (x86)"
    );
}

#[test]
fn ignored_redistributables_never_add_apis() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32().import("kernel32.dll"));
    game.pe("d3dx9_43.dll", &dll32().import("d3d9.dll"));
    game.pe("d3dx10_43.dll", &dll32().import("d3d10.dll"));
    game.pe("XInput1_3.dll", &dll32().import("d3d9.dll"));
    game.pe("dxvk_d3d11.dll", &dll32().import("d3d11.dll"));
    game.pe("msvcr100.dll", &dll32().nx_compat(false));

    let scan = game.scan("game.exe");
    assert!(scan.apis.is_empty());
    assert!(scan.evidence.is_empty());
    assert!(scan.skipped.is_empty());
    assert!(scan.non_nx_modules.is_empty());
    assert_eq!(scan.primary_api(), None);
}

#[test]
fn executable_imports_are_evidence() {
    let game = GameDir::new();
    game.pe(
        "Game.exe",
        &PeBuilder::pe64()
            .import("d3d11.dll")
            .import("dxgi.dll")
            .delay_import("d3d12.dll"),
    );
    let scan = game.scan("Game.exe");
    assert_eq!(
        scan.evidence,
        [
            evidence(
                "Game.exe",
                "d3d11.dll",
                GraphicsApi::D3d11,
                EvidenceKind::Import
            ),
            evidence(
                "Game.exe",
                "d3d12.dll",
                GraphicsApi::D3d12,
                EvidenceKind::Import
            ),
        ]
    );
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d12));
}

#[test]
fn load_library_strings_are_evidence() {
    let game = GameDir::new();
    let mut utf16: Vec<u8> = "OpenGL32.dll"
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    utf16.extend([0, 0]);
    game.pe(
        "game.exe",
        &PeBuilder::pe32()
            .import("kernel32.dll")
            .data(&utf16)
            .data(b"\0myd3d9.dll\0"),
    );
    game.pe("renderer.dll", &dll32().data(b"\0vulkan-1.dll\0"));

    let scan = game.scan("game.exe");
    assert_eq!(
        scan.evidence,
        [
            evidence(
                "game.exe",
                "opengl32.dll",
                GraphicsApi::OpenGl,
                EvidenceKind::String
            ),
            evidence(
                "renderer.dll",
                "vulkan-1.dll",
                GraphicsApi::Vulkan,
                EvidenceKind::String
            ),
        ]
    );
    assert_eq!(scan.primary_api(), Some(GraphicsApi::Vulkan));
}

#[test]
fn imported_names_are_not_counted_again_as_strings() {
    let game = GameDir::new();
    // The import table stores "d3d9.dll" as a string; the data adds a
    // separate "ddraw.dll" LoadLibrary target.
    game.pe(
        "game.exe",
        &PeBuilder::pe32()
            .import("D3D9.dll")
            .data(b"\0ddraw.dll\0d3d9.dll\0"),
    );
    let scan = game.scan("game.exe");
    assert_eq!(
        scan.evidence,
        [
            evidence(
                "game.exe",
                "d3d9.dll",
                GraphicsApi::D3d9,
                EvidenceKind::Import
            ),
            evidence(
                "game.exe",
                "ddraw.dll",
                GraphicsApi::DirectDraw,
                EvidenceKind::String
            ),
        ]
    );
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d9));
}

#[test]
fn evidence_is_sorted_by_file_then_import() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32().import("opengl32.dll"));
    game.pe(
        "b_render.dll",
        &dll32().import("d3d9.dll").import("d3d8.dll"),
    );
    game.pe("a_render.dll", &dll32().import("ddraw.dll"));
    let scan = game.scan("game.exe");
    let order: Vec<(&str, &str)> = scan
        .evidence
        .iter()
        .map(|evidence| (evidence.file.as_str(), evidence.import.as_str()))
        .collect();
    assert_eq!(
        order,
        [
            ("a_render.dll", "ddraw.dll"),
            ("b_render.dll", "d3d8.dll"),
            ("b_render.dll", "d3d9.dll"),
            ("game.exe", "opengl32.dll"),
        ]
    );
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d9));
}

#[test]
fn sibling_dlls_of_another_architecture_are_skipped() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe64().import("kernel32.dll"));
    game.pe("render32.dll", &dll32().import("d3d9.dll"));
    game.pe(
        "render_arm.dll",
        &dll64().machine(MACHINE_ARM64).import("d3d12.dll"),
    );
    game.pe("render64.dll", &dll64().import("d3d11.dll"));

    let scan = game.scan("game.exe");
    assert_eq!(scan.bitness(), Bitness::X64);
    assert_eq!(
        scan.apis.iter().copied().collect::<Vec<_>>(),
        [GraphicsApi::D3d11]
    );
    assert_eq!(
        skip_reason(&scan, "render32.dll"),
        "x86 (0x014c) module, but the executable is 64-bit (x86-64)"
    );
    assert_eq!(
        skip_reason(&scan, "render_arm.dll"),
        "ARM64 (0xaa64) module, but the executable is 64-bit (x86-64)"
    );
}

#[test]
fn unparsable_dlls_are_skipped_with_the_reason() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    game.file("empty.dll", b"");
    let truncated = dll32().import("d3d9.dll").build();
    game.file("truncated.dll", &truncated[..0x100]);

    let scan = game.scan("game.exe");
    assert_eq!(
        skip_reason(&scan, "empty.dll"),
        "not a PE image: missing MZ signature"
    );
    assert!(skip_reason(&scan, "truncated.dll").starts_with("not a PE image: invalid headers"));
    assert!(scan.apis.is_empty());
}

#[test]
fn dll_extension_matches_case_insensitively_and_only_for_files() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    game.pe("RENDER.DLL", &dll32().import("d3d9.dll"));
    game.pe("render.dll.bak", &dll32().import("d3d11.dll"));
    game.subdir("plugins.dll");

    let scan = game.scan("game.exe");
    assert_eq!(
        scan.evidence,
        [evidence(
            "RENDER.DLL",
            "d3d9.dll",
            GraphicsApi::D3d9,
            EvidenceKind::Import
        )]
    );
    assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
}

#[test]
fn an_executable_named_dll_is_inspected_once() {
    let game = GameDir::new();
    game.pe(
        "launcher.dll",
        &PeBuilder::pe32().nx_compat(false).import("d3d9.dll"),
    );
    let scan = game.scan("launcher.dll");
    assert_eq!(scan.evidence.len(), 1);
    assert_eq!(scan.non_nx_modules, ["launcher.dll"]);
}

#[test]
fn steamworks_from_a_sibling_file() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe64());
    game.file("Steam_API64.DLL", b"");
    assert!(game.scan("game.exe").uses_steamworks);
}

#[test]
fn steamworks_from_an_import() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe64().import("steam_api64.dll"));
    assert!(game.scan("game.exe").uses_steamworks);
}

#[test]
fn steamworks_from_a_sibling_dll_import() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    game.pe("online.dll", &dll32().delay_import("steam_api.dll"));
    assert!(game.scan("game.exe").uses_steamworks);
}

#[test]
fn no_steamworks_without_the_dll() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32().import("kernel32.dll"));
    game.file("steam_appid.txt", b"287450");
    game.file("SteamAPIUpdater.dll", b"");
    assert!(!game.scan("game.exe").uses_steamworks);
}

#[test]
fn anti_cheat_files_and_directories_are_listed_lowercase() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe64());
    game.subdir("EasyAntiCheat");
    game.file("EasyAntiCheat_EOS_Setup.exe", b"");
    game.file("BEService_x64.exe", b"");
    game.subdir("BattlEye");
    game.file("vgk.sys", b"");
    game.file("eac_readme.txt", b"");

    let scan = game.scan("game.exe");
    assert_eq!(
        scan.anti_cheat,
        [
            "battleye",
            "beservice_x64.exe",
            "easyanticheat",
            "easyanticheat_eos_setup.exe",
            "vgk.sys",
        ]
    );
}

#[test]
fn modules_without_nx_compat_are_listed_executable_first() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32().nx_compat(false));
    game.pe("pp_unicows.dll", &dll32().nx_compat(false));
    game.pe("PidGenx.dll", &dll32().nx_compat(false));
    game.pe("avutil-ttv-51.dll", &dll32().nx_compat(false));
    game.pe("dssl.dll", &dll32());

    let scan = game.scan("game.exe");
    // Sibling order ignores case, as Windows does.
    assert_eq!(
        scan.non_nx_modules,
        [
            "game.exe",
            "avutil-ttv-51.dll",
            "PidGenx.dll",
            "pp_unicows.dll"
        ]
    );
}

#[test]
fn only_the_first_dlls_by_name_are_inspected() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    let plain = dll32().import("kernel32.dll").build();
    for i in 0..MAX_SIBLING_DLLS {
        game.file(&format!("mod{i:03}.dll"), &plain);
    }
    // Sorted after the first MAX_SIBLING_DLLS, so never inspected.
    game.pe("zz_late.dll", &dll32().import("d3d9.dll"));
    game.pe("zz_later.dll", &dll32().import("d3d12.dll"));
    // Ignored DLLs do not count towards the limit.
    game.pe("xinput1_3.dll", &dll32());

    let scan = game.scan("game.exe");
    assert!(scan.apis.is_empty(), "{:?}", scan.apis);
    assert_eq!(scan.skipped.len(), 1, "{:?}", scan.skipped);
    assert_eq!(
        skip_reason(&scan, "zz_late.dll"),
        format!(
            "not inspected: only the first {MAX_SIBLING_DLLS} DLLs by file name are scanned \
             (2 left out from here on)"
        )
    );
}

#[test]
fn exactly_the_limit_is_not_truncated() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    let plain = dll32().build();
    for i in 0..MAX_SIBLING_DLLS - 1 {
        game.file(&format!("mod{i:03}.dll"), &plain);
    }
    game.pe("zz_render.dll", &dll32().import("d3d9.dll"));

    let scan = game.scan("game.exe");
    assert!(scan.skipped.is_empty(), "{:?}", scan.skipped);
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d9));
}

#[test]
fn oversized_dlls_are_skipped_unread() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    let huge = fs::File::create(game.path("huge.dll")).expect("create");
    // Sparse, so the test writes nothing.
    huge.set_len(MAX_DLL_BYTES + 1).expect("set_len");

    let scan = game.scan("game.exe");
    assert_eq!(skip_reason(&scan, "huge.dll"), "larger than 128 MiB");
}

#[test]
fn missing_executable_is_an_io_error() {
    let game = GameDir::new();
    let exe = game.path("missing.exe");
    match scan_game(&exe) {
        Err(PeError::Io { path, .. }) => assert_eq!(path, exe),
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn non_pe_executable_is_rejected_with_its_path() {
    let game = GameDir::new();
    let exe = game.file("game.exe", b"#!/bin/sh\necho hi\n");
    let err = scan_game(&exe).expect_err("not PE");
    assert!(matches!(err, PeError::NotPe(_)));
    assert!(err.to_string().contains(&exe.display().to_string()));
}

#[test]
fn non_x86_executable_is_rejected_naming_the_machine() {
    let game = GameDir::new();
    let exe = game.pe("game.exe", &PeBuilder::pe64().machine(MACHINE_ARM64));
    match scan_game(&exe) {
        Err(PeError::NotPe(message)) => {
            assert!(message.contains("ARM64 (0xaa64)"), "{message}");
            assert!(message.contains(&exe.display().to_string()), "{message}");
        }
        other => panic!("expected NotPe, got {other:?}"),
    }
}

#[test]
fn malformed_executable_imports_are_fatal() {
    let game = GameDir::new();
    let builder = PeBuilder::pe32().import("d3d9.dll");
    let mut bytes = builder.build();
    let entry = common::layout::import_directory_entry(&builder);
    bytes[entry..entry + 4].copy_from_slice(&0x9000_u32.to_le_bytes());
    let exe = game.file("game.exe", &bytes);
    assert!(matches!(scan_game(&exe), Err(PeError::Malformed(_))));
}

#[test]
fn malformed_sibling_imports_are_skipped() {
    let game = GameDir::new();
    game.pe("game.exe", &PeBuilder::pe32());
    let builder = dll32().import("d3d9.dll");
    let mut bytes = builder.build();
    let entry = common::layout::import_directory_entry(&builder);
    bytes[entry..entry + 4].copy_from_slice(&0x9000_u32.to_le_bytes());
    game.file("broken.dll", &bytes);

    let scan = game.scan("game.exe");
    assert!(skip_reason(&scan, "broken.dll").starts_with("malformed PE image: import table: "));
}

#[cfg(unix)]
#[test]
fn an_unlistable_directory_is_reported_not_fatal() {
    use std::os::unix::fs::PermissionsExt;

    let game = GameDir::new();
    let exe = game.pe("game.exe", &PeBuilder::pe32().import("d3d9.dll"));
    let dir = game.dir.path();
    // Search permission only: the executable can be opened, the listing not.
    fs::set_permissions(dir, fs::Permissions::from_mode(0o300)).expect("chmod");
    let listable = fs::read_dir(dir).is_ok();
    let result = scan_game(&exe);
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700)).expect("restore");

    if listable {
        // Running as root: permissions are not enforced, nothing to test.
        return;
    }
    let scan = result.expect("scan succeeds");
    assert_eq!(scan.primary_api(), Some(GraphicsApi::D3d9));
    assert_eq!(scan.skipped.len(), 1);
    let (name, reason) = &scan.skipped[0];
    assert_eq!(Path::new(name), dir);
    assert!(
        reason.starts_with("cannot list the executable's directory: "),
        "{reason}"
    );
}
