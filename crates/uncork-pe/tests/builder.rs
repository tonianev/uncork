//! The test PE builder must emit images that `object` itself accepts, or the
//! other integration tests would only prove that two bugs agree.

mod common;

use common::{
    IMAGE_DLLCHARACTERISTICS_NX_COMPAT, IMAGE_FILE_DLL, IMAGE_FILE_LARGE_ADDRESS_AWARE,
    IMAGE_SUBSYSTEM_WINDOWS_CUI, PeBuilder,
};
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile32, PeFile64};
use object::{
    Architecture, FileKind, ImportLibraryFlags, LittleEndian as LE, NameOrOrdinal, Object,
    ObjectKind, ObjectSection,
};

/// `(library, delay-loaded)` pairs in table order.
fn libraries<'data>(file: &impl Object<'data>) -> Vec<(String, bool)> {
    file.import_libraries()
        .expect("import libraries")
        .map(|library| {
            let library = library.expect("import library");
            let delay = matches!(library.flags(), ImportLibraryFlags::Pe { delay: true });
            (String::from_utf8_lossy(library.name()).into_owned(), delay)
        })
        .collect()
}

/// `(library, function)` pairs, which also walks every thunk array.
fn imported_functions<'data>(file: &impl Object<'data>) -> Vec<(String, String)> {
    file.imports()
        .expect("imports")
        .map(|import| {
            let import = import.expect("import");
            let NameOrOrdinal::Name(name) = import.name() else {
                panic!("the builder imports by name");
            };
            (
                String::from_utf8_lossy(import.library()).into_owned(),
                String::from_utf8_lossy(name).into_owned(),
            )
        })
        .collect()
}

fn owned(pairs: &[(&str, bool)]) -> Vec<(String, bool)> {
    pairs
        .iter()
        .map(|&(name, delay)| (name.to_owned(), delay))
        .collect()
}

#[test]
fn pe32_image_parses_with_object() {
    let bytes = PeBuilder::pe32()
        .import("KERNEL32.dll")
        .import("d3d11.dll")
        .delay_import("D3DCOMPILER_47.dll")
        .build();

    assert_eq!(FileKind::parse(&*bytes).expect("file kind"), FileKind::Pe32);
    let file = PeFile32::parse(&*bytes).expect("PE32 parses");
    assert_eq!(file.architecture(), Architecture::I386);
    assert!(!file.is_64());
    assert_eq!(file.kind(), ObjectKind::Executable);
    assert_eq!(
        libraries(&file),
        owned(&[
            ("KERNEL32.dll", false),
            ("d3d11.dll", false),
            ("D3DCOMPILER_47.dll", true),
        ])
    );
    let functions = imported_functions(&file);
    assert_eq!(functions.len(), 3);
    assert!(functions.iter().all(|(_, function)| function == "Function"));
}

#[test]
fn pe64_image_parses_with_object() {
    let bytes = PeBuilder::pe64()
        .import("d3d12.dll")
        .delay_import("dxgi.dll")
        .delay_import("steam_api64.dll")
        .build();

    assert_eq!(FileKind::parse(&*bytes).expect("file kind"), FileKind::Pe64);
    assert!(PeFile32::parse(&*bytes).is_err());
    let file = PeFile64::parse(&*bytes).expect("PE32+ parses");
    assert_eq!(file.architecture(), Architecture::X86_64);
    assert!(file.is_64());
    assert_eq!(
        libraries(&file),
        owned(&[
            ("d3d12.dll", false),
            ("dxgi.dll", true),
            ("steam_api64.dll", true),
        ])
    );
    assert_eq!(imported_functions(&file).len(), 3);
}

#[test]
fn header_fields_are_written_where_object_reads_them() {
    let bytes = PeBuilder::pe32()
        .dll()
        .large_address_aware(true)
        .nx_compat(false)
        .subsystem(IMAGE_SUBSYSTEM_WINDOWS_CUI)
        .build();
    let file = PeFile32::parse(&*bytes).expect("parses");
    let characteristics = file.nt_headers().file_header().characteristics.get(LE).0;
    let optional = file.nt_headers().optional_header();

    assert_eq!(file.kind(), ObjectKind::Dynamic);
    assert_ne!(characteristics & IMAGE_FILE_DLL, 0);
    assert_ne!(characteristics & IMAGE_FILE_LARGE_ADDRESS_AWARE, 0);
    assert_eq!(
        optional.dll_characteristics().0 & IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
        0
    );
    assert_eq!(optional.subsystem().0, IMAGE_SUBSYSTEM_WINDOWS_CUI);
    assert_eq!(optional.image_base(), PeBuilder::pe32().image_base());
}

#[test]
fn an_image_without_imports_has_no_import_tables() {
    let bytes = PeBuilder::pe64().build();
    let file = PeFile64::parse(&*bytes).expect("parses");
    assert!(file.import_table().expect("import table").is_none());
    assert!(
        file.delay_load_import_table()
            .expect("delay-load table")
            .is_none()
    );
}

#[test]
fn extra_data_lands_in_the_section() {
    let bytes = PeBuilder::pe32()
        .import("user32.dll")
        .data(b"GraphicsDLL=d3dgl.dll\0")
        .build();
    let file = PeFile32::parse(&*bytes).expect("parses");
    let section = file.sections().next().expect("one section");
    assert_eq!(section.name().expect("name"), ".idata");
    let data = section.data().expect("section data");
    assert!(data.ends_with(b"GraphicsDLL=d3dgl.dll\0"));
}

#[test]
fn a_wine_marker_does_not_disturb_the_headers() {
    let bytes = PeBuilder::pe64()
        .wine_signature(b"Wine builtin DLL")
        .import("ntdll.dll")
        .build();
    assert_eq!(&bytes[0x40..0x50], b"Wine builtin DLL");
    let file = PeFile64::parse(&*bytes).expect("parses");
    assert_eq!(libraries(&file), owned(&[("ntdll.dll", false)]));
}

#[test]
fn object_rejects_legacy_delay_load_descriptors() {
    // This is why `uncork-pe` walks the delay-load descriptors itself.
    let bytes = PeBuilder::pe32()
        .delay_import("d3d9.dll")
        .legacy_delay_descriptors()
        .build();
    let file = PeFile32::parse(&*bytes).expect("headers parse");
    let first = file
        .import_libraries()
        .expect("iterator")
        .next()
        .expect("one entry");
    assert!(first.is_err());
}
