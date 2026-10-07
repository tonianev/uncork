//! PE header and import-table parsing, on top of `object`.

use std::collections::BTreeSet;

use object::LittleEndian as LE;
use object::pe;
use object::read::pe::{ImageNtHeaders, ImageOptionalHeader, PeFile};

use crate::{Machine, PeError, PeInfo, wine_marker};

/// File offset of `e_lfanew` (the offset of the NT headers) in the DOS header.
const E_LFANEW_OFFSET: usize = 0x3c;
/// `PE\0\0`, the first four bytes of the NT headers.
const PE_SIGNATURE: &[u8] = b"PE\0\0";
/// Offset of the optional header's `Magic` from the start of the NT headers:
/// the PE signature plus the 20-byte COFF file header.
const OPTIONAL_MAGIC_OFFSET: usize = PE_SIGNATURE.len() + 20;

/// Parse the headers and import tables of a PE32 or PE32+ image.
pub(crate) fn parse(bytes: &[u8]) -> Result<PeInfo, PeError> {
    let nt_headers = nt_headers_offset(bytes)?;
    let magic = nt_headers
        .checked_add(OPTIONAL_MAGIC_OFFSET)
        .and_then(|at| read_u16(bytes, at))
        .ok_or_else(|| PeError::NotPe("truncated COFF header".to_owned()))?;
    match magic {
        pe::IMAGE_NT_OPTIONAL_HDR32_MAGIC => parse_as::<pe::ImageNtHeaders32>(bytes),
        pe::IMAGE_NT_OPTIONAL_HDR64_MAGIC => parse_as::<pe::ImageNtHeaders64>(bytes),
        other => Err(PeError::NotPe(format!(
            "unknown optional header magic {other:#06x}"
        ))),
    }
}

/// Check the `MZ` and `PE\0\0` signatures and return the NT headers' offset.
fn nt_headers_offset(bytes: &[u8]) -> Result<usize, PeError> {
    if !bytes.starts_with(b"MZ") {
        return Err(PeError::NotPe("missing MZ signature".to_owned()));
    }
    let e_lfanew = read_u32(bytes, E_LFANEW_OFFSET)
        .ok_or_else(|| PeError::NotPe("truncated DOS header".to_owned()))?;
    let signature_at = |start: usize| bytes.get(start..start.checked_add(PE_SIGNATURE.len())?);
    match usize::try_from(e_lfanew) {
        Ok(start) if signature_at(start) == Some(PE_SIGNATURE) => Ok(start),
        _ => Err(PeError::NotPe(format!(
            "missing PE\\0\\0 signature at offset {e_lfanew:#x}"
        ))),
    }
}

fn parse_as<Pe: ImageNtHeaders>(bytes: &[u8]) -> Result<PeInfo, PeError> {
    let file: PeFile<'_, Pe> =
        PeFile::parse(bytes).map_err(|err| PeError::NotPe(format!("invalid headers: {err}")))?;
    let file_header = file.nt_headers().file_header();
    let optional_header = file.nt_headers().optional_header();
    let characteristics = file_header.characteristics.get(LE);
    let machine = Machine::from_raw(file_header.machine.get(LE).0);
    Ok(PeInfo {
        machine,
        bitness: machine.bitness(),
        is_dll: characteristics.contains(pe::IMAGE_FILE_DLL),
        large_address_aware: characteristics.contains(pe::IMAGE_FILE_LARGE_ADDRESS_AWARE),
        nx_compat: optional_header
            .dll_characteristics()
            .contains(pe::IMAGE_DLLCHARACTERISTICS_NX_COMPAT),
        subsystem: optional_header.subsystem().0,
        wine_marker: wine_marker(bytes),
        imports: imported_dlls(&file)?,
    })
}

/// DLL names from the import and delay-load import tables, lowercased,
/// deduplicated and sorted.
fn imported_dlls<Pe: ImageNtHeaders>(file: &PeFile<'_, Pe>) -> Result<Vec<String>, PeError> {
    let mut names = BTreeSet::new();

    let import_error = malformed("import table");
    if let Some(table) = file.import_table().map_err(&import_error)? {
        for descriptor in table.descriptors().map_err(&import_error)? {
            let descriptor = descriptor.map_err(&import_error)?;
            let name = table.name(descriptor.name.get(LE)).map_err(&import_error)?;
            insert_dll_name(&mut names, name);
        }
    }

    // `object`'s own import iterators reject pre-VC7 delay-load descriptors,
    // so walk the descriptors directly.
    let delay_error = malformed("delay-load import table");
    if let Some(table) = file.delay_load_import_table().map_err(&delay_error)? {
        let image_base = file.nt_headers().optional_header().image_base();
        for descriptor in table.descriptors().map_err(&delay_error)? {
            let descriptor = descriptor.map_err(&delay_error)?;
            let name_rva = delay_load_name_rva(descriptor, image_base).ok_or_else(|| {
                PeError::Malformed(format!(
                    "delay-load import table: DLL name address {:#x} is below the image base {image_base:#x}",
                    descriptor.dll_name_rva.get(LE)
                ))
            })?;
            let name = table.name(name_rva).map_err(&delay_error)?;
            insert_dll_name(&mut names, name);
        }
    }

    Ok(names.into_iter().collect())
}

/// The RVA of a delay-load descriptor's DLL name. Descriptors written by
/// linkers older than Visual C++ 7 (without `IMAGE_DELAYLOAD_RVA_BASED`)
/// hold virtual addresses instead.
fn delay_load_name_rva(descriptor: &pe::ImageDelayloadDescriptor, image_base: u64) -> Option<u32> {
    let name = descriptor.dll_name_rva.get(LE);
    if descriptor.attributes.get(LE) & pe::IMAGE_DELAYLOAD_RVA_BASED != 0 {
        return Some(name);
    }
    u64::from(name)
        .checked_sub(image_base)
        .and_then(|rva| u32::try_from(rva).ok())
}

fn insert_dll_name(names: &mut BTreeSet<String>, name: &[u8]) {
    if !name.is_empty() {
        names.insert(String::from_utf8_lossy(name).to_ascii_lowercase());
    }
}

fn malformed(table: &'static str) -> impl Fn(object::read::Error) -> PeError {
    move |err| PeError::Malformed(format!("{table}: {err}"))
}

fn read_u16(bytes: &[u8], offset: usize) -> Option<u16> {
    bytes
        .get(offset..)?
        .first_chunk()
        .copied()
        .map(u16::from_le_bytes)
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    bytes
        .get(offset..)?
        .first_chunk()
        .copied()
        .map(u32::from_le_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 64-byte DOS header whose `e_lfanew` is `nt_offset`.
    fn dos_header(nt_offset: u32) -> Vec<u8> {
        let mut bytes = vec![0; 64];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[E_LFANEW_OFFSET..E_LFANEW_OFFSET + 4].copy_from_slice(&nt_offset.to_le_bytes());
        bytes
    }

    fn not_pe_reason(bytes: &[u8]) -> String {
        match parse(bytes) {
            Err(PeError::NotPe(reason)) => reason,
            other => panic!("expected NotPe, got {other:?}"),
        }
    }

    #[test]
    fn empty_input_is_not_pe() {
        assert_eq!(not_pe_reason(&[]), "missing MZ signature");
    }

    #[test]
    fn text_is_not_pe() {
        assert_eq!(not_pe_reason(b"hello, world"), "missing MZ signature");
    }

    #[test]
    fn bare_mz_is_a_truncated_dos_header() {
        assert_eq!(not_pe_reason(b"MZ"), "truncated DOS header");
        assert_eq!(
            not_pe_reason(&dos_header(0x40)[..0x3e]),
            "truncated DOS header"
        );
    }

    #[test]
    fn nt_headers_offset_past_the_end_is_reported_with_the_offset() {
        assert_eq!(
            not_pe_reason(&dos_header(0x1000)),
            "missing PE\\0\\0 signature at offset 0x1000"
        );
        assert_eq!(
            not_pe_reason(&dos_header(u32::MAX)),
            "missing PE\\0\\0 signature at offset 0xffffffff"
        );
    }

    #[test]
    fn wrong_nt_signature_is_not_pe() {
        let mut bytes = dos_header(0x40);
        bytes.extend_from_slice(b"NE\0\0");
        assert_eq!(
            not_pe_reason(&bytes),
            "missing PE\\0\\0 signature at offset 0x40"
        );
    }

    #[test]
    fn signature_without_coff_header_is_truncated() {
        let mut bytes = dos_header(0x40);
        bytes.extend_from_slice(PE_SIGNATURE);
        bytes.extend_from_slice(&[0; 10]);
        assert_eq!(not_pe_reason(&bytes), "truncated COFF header");
    }

    #[test]
    fn unknown_optional_header_magic_is_not_pe() {
        let mut bytes = dos_header(0x40);
        bytes.extend_from_slice(PE_SIGNATURE);
        bytes.extend_from_slice(&[0; 20]);
        bytes.extend_from_slice(&0x0107_u16.to_le_bytes()); // ROM image
        assert_eq!(
            not_pe_reason(&bytes),
            "unknown optional header magic 0x0107"
        );
    }

    #[test]
    fn truncated_optional_header_is_not_pe() {
        let mut bytes = dos_header(0x40);
        bytes.extend_from_slice(PE_SIGNATURE);
        bytes.extend_from_slice(&[0; 20]);
        bytes.extend_from_slice(&pe::IMAGE_NT_OPTIONAL_HDR32_MAGIC.to_le_bytes());
        assert!(not_pe_reason(&bytes).starts_with("invalid headers: "));
    }

    #[test]
    fn rva_based_delay_load_names_are_used_as_is() {
        let descriptor = delay_descriptor(pe::IMAGE_DELAYLOAD_RVA_BASED, 0x2010);
        assert_eq!(delay_load_name_rva(&descriptor, 0x40_0000), Some(0x2010));
    }

    #[test]
    fn legacy_delay_load_names_are_converted_from_virtual_addresses() {
        let descriptor = delay_descriptor(0, 0x40_2010);
        assert_eq!(delay_load_name_rva(&descriptor, 0x40_0000), Some(0x2010));
    }

    #[test]
    fn legacy_delay_load_names_below_the_image_base_are_rejected() {
        let descriptor = delay_descriptor(0, 0x2010);
        assert_eq!(delay_load_name_rva(&descriptor, 0x40_0000), None);
        assert_eq!(delay_load_name_rva(&descriptor, 0x1_4000_0000), None);
    }

    fn delay_descriptor(attributes: u32, dll_name: u32) -> pe::ImageDelayloadDescriptor {
        use object::U32;
        pe::ImageDelayloadDescriptor {
            attributes: U32::new(LE, attributes),
            dll_name_rva: U32::new(LE, dll_name),
            module_handle_rva: U32::new(LE, 0),
            import_address_table_rva: U32::new(LE, 0),
            import_name_table_rva: U32::new(LE, 0),
            bound_import_address_table_rva: U32::new(LE, 0),
            unload_information_table_rva: U32::new(LE, 0),
            time_date_stamp: U32::new(LE, 0),
        }
    }

    #[test]
    fn empty_dll_names_are_dropped_and_others_lowercased() {
        let mut names = BTreeSet::new();
        insert_dll_name(&mut names, b"");
        insert_dll_name(&mut names, b"KERNEL32.dll");
        insert_dll_name(&mut names, b"kernel32.DLL");
        insert_dll_name(&mut names, b"caf\xe9.dll");
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            ["caf\u{fffd}.dll", "kernel32.dll"]
        );
    }

    #[test]
    fn little_endian_reads_stop_at_the_end() {
        let bytes = [0x34, 0x12, 0x78, 0x56];
        assert_eq!(read_u16(&bytes, 0), Some(0x1234));
        assert_eq!(read_u32(&bytes, 0), Some(0x5678_1234));
        assert_eq!(read_u16(&bytes, 3), None);
        assert_eq!(read_u32(&bytes, 1), None);
        assert_eq!(read_u16(&bytes, usize::MAX), None);
    }
}
