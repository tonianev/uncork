//! `inspect_bytes` and `inspect` against images from the test PE builder.

mod common;

use common::{
    IMAGE_FILE_32BIT_MACHINE, IMAGE_FILE_EXECUTABLE_IMAGE, IMAGE_SUBSYSTEM_WINDOWS_CUI,
    IMAGE_SUBSYSTEM_WINDOWS_GUI, MACHINE_ARM64, PeBuilder, layout,
};
use proptest::prelude::*;
use uncork_pe::{
    Bitness, Machine, PeError, PeInfo, WINE_BUILTIN_SIGNATURE, WINE_PLACEHOLDER_SIGNATURE,
    WineMarker, inspect, inspect_bytes, strip_builtin_marker,
};

fn info(builder: &PeBuilder) -> PeInfo {
    inspect_bytes(&builder.build()).expect("builder output inspects")
}

fn malformed_reason(bytes: &[u8]) -> String {
    match inspect_bytes(bytes) {
        Err(PeError::Malformed(reason)) => reason,
        other => panic!("expected Malformed, got {other:?}"),
    }
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

#[test]
fn pe32_executable() {
    let info = info(
        &PeBuilder::pe32()
            .import("KERNEL32.dll")
            .import("USER32.dll")
            .import("d3d11.dll"),
    );
    assert_eq!(
        info,
        PeInfo {
            machine: Machine::X86,
            bitness: Some(Bitness::X86),
            is_dll: false,
            large_address_aware: false,
            nx_compat: true,
            subsystem: IMAGE_SUBSYSTEM_WINDOWS_GUI,
            wine_marker: None,
            imports: vec![
                "d3d11.dll".to_owned(),
                "kernel32.dll".to_owned(),
                "user32.dll".to_owned(),
            ],
        }
    );
}

#[test]
fn pe32_plus_dll() {
    let info = info(&PeBuilder::pe64().dll().import("d3d12.dll"));
    assert_eq!(info.machine, Machine::X86_64);
    assert_eq!(info.bitness, Some(Bitness::X64));
    assert!(info.is_dll);
    assert_eq!(info.imports, ["d3d12.dll"]);
}

#[test]
fn rise_of_nations_header_flags() {
    // riseofnations.exe: Characteristics 0x102, DllCharacteristics 0x8140.
    let info = info(
        &PeBuilder::pe32()
            .characteristics(IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_32BIT_MACHINE)
            .dll_characteristics(0x8140),
    );
    assert!(!info.large_address_aware);
    assert!(info.nx_compat);
    assert!(!info.is_dll);
}

#[test]
fn large_address_aware_flag() {
    assert!(info(&PeBuilder::pe32().large_address_aware(true)).large_address_aware);
    assert!(!info(&PeBuilder::pe32().large_address_aware(false)).large_address_aware);
    assert!(!info(&PeBuilder::pe64().large_address_aware(false)).large_address_aware);
}

#[test]
fn nx_compat_flag() {
    assert!(info(&PeBuilder::pe32().nx_compat(true)).nx_compat);
    assert!(!info(&PeBuilder::pe32().nx_compat(false)).nx_compat);
    assert!(!info(&PeBuilder::pe64().dll_characteristics(0)).nx_compat);
}

#[test]
fn subsystem_is_reported_raw() {
    assert_eq!(
        info(&PeBuilder::pe32().subsystem(IMAGE_SUBSYSTEM_WINDOWS_CUI)).subsystem,
        IMAGE_SUBSYSTEM_WINDOWS_CUI
    );
    assert_eq!(info(&PeBuilder::pe64().subsystem(10)).subsystem, 10);
}

#[test]
fn non_x86_machines_have_no_bitness() {
    let arm64 = info(&PeBuilder::pe64().machine(MACHINE_ARM64));
    assert_eq!(arm64.machine, Machine::Arm64);
    assert_eq!(arm64.bitness, None);

    let arm32 = info(&PeBuilder::pe32().machine(0x01c4));
    assert_eq!(arm32.machine, Machine::Other(0x01c4));
    assert_eq!(arm32.bitness, None);
}

#[test]
fn delay_load_imports_are_merged_lowercased_and_deduplicated() {
    let info = info(
        &PeBuilder::pe32()
            .import("KERNEL32.dll")
            .import("d3d11.dll")
            .delay_import("D3DCOMPILER_47.dll")
            .delay_import("kernel32.DLL")
            .delay_import("WINMM.dll"),
    );
    assert_eq!(
        info.imports,
        [
            "d3d11.dll",
            "d3dcompiler_47.dll",
            "kernel32.dll",
            "winmm.dll"
        ]
    );
}

#[test]
fn delay_load_only_images_report_their_imports() {
    for builder in [PeBuilder::pe32(), PeBuilder::pe64()] {
        let info = info(&builder.delay_import("d3d9.dll"));
        assert_eq!(info.imports, ["d3d9.dll"]);
    }
}

#[test]
fn legacy_delay_load_descriptors_are_read() {
    let info = info(
        &PeBuilder::pe32()
            .import("kernel32.dll")
            .delay_import("DDRAW.dll")
            .delay_import("dsound.dll")
            .legacy_delay_descriptors(),
    );
    assert_eq!(info.imports, ["ddraw.dll", "dsound.dll", "kernel32.dll"]);
}

#[test]
fn an_image_without_imports_reports_none() {
    assert!(info(&PeBuilder::pe32()).imports.is_empty());
    assert!(info(&PeBuilder::pe64()).imports.is_empty());
}

#[test]
fn wine_markers_are_reported() {
    let builtin = info(&PeBuilder::pe64().wine_signature(WINE_BUILTIN_SIGNATURE));
    assert_eq!(builtin.wine_marker, Some(WineMarker::Builtin));
    let placeholder = info(&PeBuilder::pe32().wine_signature(WINE_PLACEHOLDER_SIGNATURE));
    assert_eq!(placeholder.wine_marker, Some(WineMarker::Placeholder));
    assert_eq!(info(&PeBuilder::pe32()).wine_marker, None);
}

#[test]
fn stripping_the_builtin_marker_keeps_the_image_valid() {
    let builder = PeBuilder::pe64()
        .dll()
        .wine_signature(WINE_BUILTIN_SIGNATURE)
        .import("d3d11.dll");
    let mut bytes = builder.build();
    let before = inspect_bytes(&bytes).expect("inspects");

    assert!(strip_builtin_marker(&mut bytes));
    let after = inspect_bytes(&bytes).expect("still inspects");
    assert_eq!(after.wine_marker, None);
    assert_eq!(
        after,
        PeInfo {
            wine_marker: None,
            ..before
        }
    );
}

#[test]
fn import_directory_outside_every_section_is_malformed() {
    let builder = PeBuilder::pe32().import("d3d9.dll");
    let mut bytes = builder.build();
    put_u32(&mut bytes, layout::import_directory_entry(&builder), 0x9000);
    assert!(malformed_reason(&bytes).starts_with("import table: "));
}

#[test]
fn import_name_outside_the_section_is_malformed() {
    let builder = PeBuilder::pe32().import("d3d9.dll");
    let mut bytes = builder.build();
    // Name field of the first import descriptor.
    put_u32(&mut bytes, layout::SECTION_DATA + 12, 0x00de_0000);
    assert!(malformed_reason(&bytes).starts_with("import table: "));
}

#[test]
fn import_descriptors_without_a_terminator_are_malformed() {
    let builder = PeBuilder::pe32().import("d3d9.dll").data(&[0xaa; 8]);
    let mut bytes = builder.build();
    // Point the directory at the last eight bytes of the section: no room
    // for a single 20-byte descriptor, let alone the null one.
    let tail_rva = layout::SECTION_RVA + u32::try_from(builder.section_len() - 8).expect("small");
    put_u32(
        &mut bytes,
        layout::import_directory_entry(&builder),
        tail_rva,
    );
    assert!(malformed_reason(&bytes).starts_with("import table: "));
}

#[test]
fn delay_load_name_outside_the_section_is_malformed() {
    let builder = PeBuilder::pe64().delay_import("d3d9.dll");
    let mut bytes = builder.build();
    // DllNameRVA of the first delay-load descriptor.
    put_u32(&mut bytes, layout::SECTION_DATA + 4, 0x00de_0000);
    assert!(malformed_reason(&bytes).starts_with("delay-load import table: "));
}

#[test]
fn legacy_delay_load_name_below_the_image_base_is_malformed() {
    let builder = PeBuilder::pe32()
        .delay_import("d3d9.dll")
        .legacy_delay_descriptors();
    let mut bytes = builder.build();
    put_u32(&mut bytes, layout::SECTION_DATA + 4, 0x10);
    let reason = malformed_reason(&bytes);
    assert!(reason.contains("below the image base"), "{reason}");
}

#[test]
fn delay_load_directory_outside_every_section_is_malformed() {
    let builder = PeBuilder::pe32().delay_import("d3d9.dll");
    let mut bytes = builder.build();
    put_u32(
        &mut bytes,
        layout::delay_import_directory_entry(&builder),
        0x9000,
    );
    assert!(malformed_reason(&bytes).starts_with("delay-load import table: "));
}

#[test]
fn truncated_headers_are_not_pe() {
    let bytes = PeBuilder::pe32().import("d3d9.dll").build();
    for len in [0x84, 0x100, 0x190] {
        match inspect_bytes(&bytes[..len]) {
            Err(PeError::NotPe(_)) => {}
            other => panic!("truncated to {len:#x}: expected NotPe, got {other:?}"),
        }
    }
}

#[test]
fn missing_section_data_makes_imports_unreadable() {
    let with_imports = PeBuilder::pe32().import("d3d9.dll").build();
    malformed_reason(&with_imports[..layout::SECTION_DATA]);

    let without_imports = PeBuilder::pe32().build();
    let info = inspect_bytes(&without_imports[..layout::SECTION_DATA]).expect("headers only");
    assert!(info.imports.is_empty());
}

#[test]
fn inspect_reads_the_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("game.exe");
    let builder = PeBuilder::pe32().import("d3d9.dll");
    builder.write(&path);
    assert_eq!(
        inspect(&path).expect("inspects"),
        inspect_bytes(&builder.build()).expect("inspects")
    );
}

#[test]
fn inspect_reports_missing_files_with_their_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("missing.exe");
    match inspect(&path) {
        Err(PeError::Io {
            path: reported,
            source,
        }) => {
            assert_eq!(reported, path);
            assert_eq!(source.kind(), std::io::ErrorKind::NotFound);
        }
        other => panic!("expected Io, got {other:?}"),
    }
}

#[test]
fn inspect_names_the_file_in_parse_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("readme.dll");
    std::fs::write(&path, "not a program").expect("write");
    let err = inspect(&path).expect_err("not PE");
    assert!(matches!(err, PeError::NotPe(_)));
    let message = err.to_string();
    assert!(message.contains(&path.display().to_string()), "{message}");
    assert!(message.ends_with("missing MZ signature"), "{message}");
}

#[test]
fn inspect_of_a_directory_is_an_io_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(matches!(inspect(dir.path()), Err(PeError::Io { .. })));
}

fn dll_name() -> impl Strategy<Value = String> {
    "[A-Za-z0-9_-]{1,16}\\.(dll|DLL|drv)"
}

proptest! {
    #[test]
    fn inspect_bytes_never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let _ = inspect_bytes(&bytes);
    }

    #[test]
    fn inspect_bytes_never_panics_on_corrupted_images(
        is_64 in any::<bool>(),
        corruptions in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 1..16),
    ) {
        let builder = if is_64 { PeBuilder::pe64() } else { PeBuilder::pe32() };
        let mut bytes = builder
            .import("kernel32.dll")
            .import("d3d11.dll")
            .delay_import("d3dcompiler_47.dll")
            .data(b"opengl32.dll\0")
            .build();
        for (index, value) in corruptions {
            let at = index.index(bytes.len());
            bytes[at] = value;
        }
        let _ = inspect_bytes(&bytes);
    }

    #[test]
    fn inspect_bytes_never_panics_on_truncated_images(cut in 0usize..0x400) {
        let bytes = PeBuilder::pe64().import("d3d9.dll").delay_import("x.dll").build();
        let _ = inspect_bytes(&bytes[..cut.min(bytes.len())]);
    }

    #[test]
    fn header_fields_and_imports_round_trip(
        is_64 in any::<bool>(),
        is_dll in any::<bool>(),
        large_address_aware in any::<bool>(),
        nx_compat in any::<bool>(),
        subsystem in any::<u16>(),
        imports in proptest::collection::vec(dll_name(), 0..8),
        delay_imports in proptest::collection::vec(dll_name(), 0..8),
    ) {
        let mut builder = if is_64 { PeBuilder::pe64() } else { PeBuilder::pe32() }
            .large_address_aware(large_address_aware)
            .nx_compat(nx_compat)
            .subsystem(subsystem);
        if is_dll {
            builder = builder.dll();
        }
        for name in &imports {
            builder = builder.import(name);
        }
        for name in &delay_imports {
            builder = builder.delay_import(name);
        }

        let info = inspect_bytes(&builder.build()).expect("builder output inspects");

        let mut expected: Vec<String> = imports
            .iter()
            .chain(&delay_imports)
            .map(|name| name.to_ascii_lowercase())
            .collect();
        expected.sort();
        expected.dedup();
        prop_assert_eq!(info.imports, expected);
        prop_assert_eq!(info.bitness, Some(if is_64 { Bitness::X64 } else { Bitness::X86 }));
        prop_assert_eq!(info.is_dll, is_dll);
        prop_assert_eq!(info.large_address_aware, large_address_aware);
        prop_assert_eq!(info.nx_compat, nx_compat);
        prop_assert_eq!(info.subsystem, subsystem);
    }
}
