//! A test-only writer for minimal but valid PE32 and PE32+ images, copied
//! from `crates/uncork-pe/tests/common` (test helpers cannot be shared
//! between crates without a crate of their own).
//!
//! Each image has the DOS header, NT headers and one `.idata` section holding
//! an import directory and a delay-load import directory (each DLL imports
//! one function by name), followed by any extra bytes the test wants to
//! embed. `crates/uncork-pe/tests/builder.rs` checks the output with `object`.

// Each integration test crate uses a different subset of the builder.
#![allow(dead_code)]

use std::path::Path;

use uncork_pe::Bitness;

pub const IMAGE_FILE_EXECUTABLE_IMAGE: u16 = 0x0002;
pub const IMAGE_FILE_LARGE_ADDRESS_AWARE: u16 = 0x0020;
pub const IMAGE_FILE_32BIT_MACHINE: u16 = 0x0100;
pub const IMAGE_FILE_DLL: u16 = 0x2000;
pub const IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE: u16 = 0x0040;
pub const IMAGE_DLLCHARACTERISTICS_NX_COMPAT: u16 = 0x0100;
pub const IMAGE_SUBSYSTEM_WINDOWS_GUI: u16 = 2;
pub const IMAGE_SUBSYSTEM_WINDOWS_CUI: u16 = 3;
pub const MACHINE_I386: u16 = 0x014c;
pub const MACHINE_AMD64: u16 = 0x8664;
pub const MACHINE_ARM64: u16 = 0xaa64;

/// File offset of the NT headers; leaves room for a Wine marker at 0x40.
const NT_HEADERS_OFFSET: usize = 0x80;
const FILE_ALIGNMENT: usize = 0x200;
const SECTION_ALIGNMENT: u32 = 0x1000;
const SECTION_RVA: u32 = SECTION_ALIGNMENT;
const COFF_HEADER_LEN: usize = 20;
const SECTION_HEADER_LEN: usize = 40;
const DATA_DIRECTORIES: usize = 16;
const IMPORT_DIRECTORY: usize = 1;
const DELAY_IMPORT_DIRECTORY: usize = 13;
const IMPORT_DESCRIPTOR_LEN: usize = 20;
const DELAY_DESCRIPTOR_LEN: usize = 32;

/// Builder for a PE image. Defaults to a GUI executable with `NX_COMPAT`.
#[derive(Debug, Clone)]
pub struct PeBuilder {
    bitness: Bitness,
    machine: u16,
    characteristics: u16,
    dll_characteristics: u16,
    subsystem: u16,
    imports: Vec<String>,
    delay_imports: Vec<String>,
    legacy_delay_descriptors: bool,
    wine_signature: Option<Vec<u8>>,
    extra: Vec<u8>,
}

impl PeBuilder {
    /// A PE32 (32-bit x86) executable.
    pub fn pe32() -> Self {
        Self::new(Bitness::X86)
    }

    /// A PE32+ (x86-64) executable.
    pub fn pe64() -> Self {
        Self::new(Bitness::X64)
    }

    pub fn new(bitness: Bitness) -> Self {
        let (machine, characteristics) = match bitness {
            Bitness::X86 => (
                MACHINE_I386,
                IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_32BIT_MACHINE,
            ),
            Bitness::X64 => (
                MACHINE_AMD64,
                IMAGE_FILE_EXECUTABLE_IMAGE | IMAGE_FILE_LARGE_ADDRESS_AWARE,
            ),
        };
        Self {
            bitness,
            machine,
            characteristics,
            dll_characteristics: IMAGE_DLLCHARACTERISTICS_DYNAMIC_BASE
                | IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
            subsystem: IMAGE_SUBSYSTEM_WINDOWS_GUI,
            imports: Vec::new(),
            delay_imports: Vec::new(),
            legacy_delay_descriptors: false,
            wine_signature: None,
            extra: Vec::new(),
        }
    }

    /// Override the COFF machine (keeps the optional-header format).
    pub fn machine(mut self, machine: u16) -> Self {
        self.machine = machine;
        self
    }

    /// Replace the COFF `Characteristics` field.
    pub fn characteristics(mut self, characteristics: u16) -> Self {
        self.characteristics = characteristics;
        self
    }

    /// Replace the optional header's `DllCharacteristics` field.
    pub fn dll_characteristics(mut self, dll_characteristics: u16) -> Self {
        self.dll_characteristics = dll_characteristics;
        self
    }

    /// Mark the image as a DLL.
    pub fn dll(mut self) -> Self {
        self.characteristics |= IMAGE_FILE_DLL;
        self
    }

    pub fn large_address_aware(mut self, on: bool) -> Self {
        set_flag(
            &mut self.characteristics,
            IMAGE_FILE_LARGE_ADDRESS_AWARE,
            on,
        );
        self
    }

    pub fn nx_compat(mut self, on: bool) -> Self {
        set_flag(
            &mut self.dll_characteristics,
            IMAGE_DLLCHARACTERISTICS_NX_COMPAT,
            on,
        );
        self
    }

    pub fn subsystem(mut self, subsystem: u16) -> Self {
        self.subsystem = subsystem;
        self
    }

    /// Add an import-table entry for `dll`.
    pub fn import(mut self, dll: &str) -> Self {
        self.imports.push(dll.to_owned());
        self
    }

    /// Add a delay-load import-table entry for `dll`.
    pub fn delay_import(mut self, dll: &str) -> Self {
        self.delay_imports.push(dll.to_owned());
        self
    }

    /// Write delay-load descriptors the way linkers before Visual C++ 7 did:
    /// without `IMAGE_DELAYLOAD_RVA_BASED`, holding virtual addresses.
    pub fn legacy_delay_descriptors(mut self) -> Self {
        self.legacy_delay_descriptors = true;
        self
    }

    /// Write `signature` (one of Wine's markers) at file offset 0x40.
    pub fn wine_signature(mut self, signature: &[u8]) -> Self {
        self.wine_signature = Some(signature.to_vec());
        self
    }

    /// Append raw bytes to the section, e.g. strings for a string scan.
    pub fn data(mut self, bytes: &[u8]) -> Self {
        self.extra.extend_from_slice(bytes);
        self
    }

    /// Write the image to `path`.
    pub fn write(&self, path: &Path) {
        std::fs::write(path, self.build())
            .unwrap_or_else(|err| panic!("cannot write {}: {err}", path.display()));
    }

    /// Length of the section's meaningful data (its virtual size).
    pub fn section_len(&self) -> usize {
        self.section().bytes.len()
    }

    pub fn image_base(&self) -> u64 {
        match self.bitness {
            Bitness::X86 => 0x40_0000,
            Bitness::X64 => 0x1_4000_0000,
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let section = self.section();
        let raw_size = align(section.bytes.len(), FILE_ALIGNMENT);
        let virtual_size = u32::try_from(section.bytes.len()).expect("section fits in u32");
        let size_of_image = SECTION_RVA + align_u32(virtual_size, SECTION_ALIGNMENT);

        let mut image = vec![0; FILE_ALIGNMENT];
        image[..2].copy_from_slice(b"MZ");
        put_u32(&mut image, 0x3c, NT_HEADERS_OFFSET as u32);
        if let Some(signature) = &self.wine_signature {
            image[0x40..0x40 + signature.len()].copy_from_slice(signature);
        }

        let mut headers = Vec::new();
        headers.extend_from_slice(b"PE\0\0");
        self.coff_header(&mut headers);
        self.optional_header(&mut headers, size_of_image, &section);
        section_header(&mut headers, virtual_size, raw_size);
        assert!(NT_HEADERS_OFFSET + headers.len() <= FILE_ALIGNMENT);
        image[NT_HEADERS_OFFSET..NT_HEADERS_OFFSET + headers.len()].copy_from_slice(&headers);

        image.extend_from_slice(&section.bytes);
        image.resize(FILE_ALIGNMENT + raw_size, 0);
        image
    }

    fn thunk_len(&self) -> usize {
        match self.bitness {
            Bitness::X86 => 4,
            Bitness::X64 => 8,
        }
    }

    fn optional_header_len(&self) -> usize {
        let fixed = match self.bitness {
            Bitness::X86 => 96,
            Bitness::X64 => 112,
        };
        fixed + DATA_DIRECTORIES * 8
    }

    fn coff_header(&self, out: &mut Vec<u8>) {
        let start = out.len();
        out.extend_from_slice(&self.machine.to_le_bytes());
        out.extend_from_slice(&1_u16.to_le_bytes()); // number of sections
        out.extend_from_slice(&[0; 12]); // timestamp, symbol table, symbol count
        let optional_len = u16::try_from(self.optional_header_len()).expect("small header");
        out.extend_from_slice(&optional_len.to_le_bytes());
        out.extend_from_slice(&self.characteristics.to_le_bytes());
        assert_eq!(out.len() - start, COFF_HEADER_LEN);
    }

    fn optional_header(&self, out: &mut Vec<u8>, size_of_image: u32, section: &Section) {
        let start = out.len();
        let is_64 = self.bitness == Bitness::X64;
        let magic: u16 = if is_64 { 0x20b } else { 0x10b };
        out.extend_from_slice(&magic.to_le_bytes());
        out.extend_from_slice(&[14, 0]); // linker version
        out.extend_from_slice(&[0; 12]); // code / data sizes
        out.extend_from_slice(&0_u32.to_le_bytes()); // entry point
        out.extend_from_slice(&SECTION_RVA.to_le_bytes()); // base of code
        if is_64 {
            out.extend_from_slice(&self.image_base().to_le_bytes());
        } else {
            out.extend_from_slice(&SECTION_RVA.to_le_bytes()); // base of data
            let base = u32::try_from(self.image_base()).expect("32-bit image base");
            out.extend_from_slice(&base.to_le_bytes());
        }
        out.extend_from_slice(&SECTION_ALIGNMENT.to_le_bytes());
        out.extend_from_slice(&(FILE_ALIGNMENT as u32).to_le_bytes());
        for version in [6_u16, 0, 0, 0, 6, 0] {
            // OS, image and subsystem versions
            out.extend_from_slice(&version.to_le_bytes());
        }
        out.extend_from_slice(&0_u32.to_le_bytes()); // Win32 version
        out.extend_from_slice(&size_of_image.to_le_bytes());
        out.extend_from_slice(&(FILE_ALIGNMENT as u32).to_le_bytes()); // size of headers
        out.extend_from_slice(&0_u32.to_le_bytes()); // checksum
        out.extend_from_slice(&self.subsystem.to_le_bytes());
        out.extend_from_slice(&self.dll_characteristics.to_le_bytes());
        for size in [0x10_0000_u64, 0x1000, 0x10_0000, 0x1000] {
            // stack and heap reserve / commit
            if is_64 {
                out.extend_from_slice(&size.to_le_bytes());
            } else {
                out.extend_from_slice(&(size as u32).to_le_bytes());
            }
        }
        out.extend_from_slice(&0_u32.to_le_bytes()); // loader flags
        out.extend_from_slice(&(DATA_DIRECTORIES as u32).to_le_bytes());
        for index in 0..DATA_DIRECTORIES {
            let (rva, size) = match index {
                IMPORT_DIRECTORY => section.imports,
                DELAY_IMPORT_DIRECTORY => section.delay_imports,
                _ => (0, 0),
            };
            out.extend_from_slice(&rva.to_le_bytes());
            out.extend_from_slice(&size.to_le_bytes());
        }
        assert_eq!(out.len() - start, self.optional_header_len());
    }

    /// Lay out the `.idata` section: descriptor arrays first, then for each
    /// DLL its name, hint/name entry and thunk arrays, then the extra bytes.
    fn section(&self) -> Section {
        let mut bytes = Vec::new();
        let import_len = descriptor_array_len(self.imports.len(), IMPORT_DESCRIPTOR_LEN);
        let delay_len = descriptor_array_len(self.delay_imports.len(), DELAY_DESCRIPTOR_LEN);
        bytes.resize(import_len + delay_len, 0);

        for (i, dll) in self.imports.iter().enumerate() {
            let entry = self.write_import_data(&mut bytes, dll, false);
            let descriptor = i * IMPORT_DESCRIPTOR_LEN;
            put_u32(&mut bytes, descriptor, entry.name_table); // OriginalFirstThunk
            put_u32(&mut bytes, descriptor + 12, entry.name);
            put_u32(&mut bytes, descriptor + 16, entry.address_table); // FirstThunk
        }

        for (i, dll) in self.delay_imports.iter().enumerate() {
            let legacy = self.legacy_delay_descriptors;
            let entry = self.write_import_data(&mut bytes, dll, legacy);
            let module_handle = append(&mut bytes, &vec![0; self.thunk_len()], 8);
            let descriptor = import_len + i * DELAY_DESCRIPTOR_LEN;
            put_u32(&mut bytes, descriptor, u32::from(!legacy)); // IMAGE_DELAYLOAD_RVA_BASED
            put_u32(&mut bytes, descriptor + 4, self.address(entry.name, legacy));
            put_u32(
                &mut bytes,
                descriptor + 8,
                self.address(module_handle, legacy),
            );
            put_u32(
                &mut bytes,
                descriptor + 12,
                self.address(entry.address_table, legacy),
            );
            put_u32(
                &mut bytes,
                descriptor + 16,
                self.address(entry.name_table, legacy),
            );
        }

        bytes.extend_from_slice(&self.extra);
        if bytes.is_empty() {
            bytes.resize(16, 0);
        }
        Section {
            bytes,
            imports: directory(0, import_len),
            delay_imports: directory(import_len, delay_len),
        }
    }

    /// Append one DLL's name, a `Function` hint/name entry and two thunk
    /// arrays pointing at it. Returns their RVAs.
    fn write_import_data(&self, bytes: &mut Vec<u8>, dll: &str, legacy: bool) -> ImportData {
        let mut name = dll.as_bytes().to_vec();
        name.push(0);
        let name = append(bytes, &name, 2);
        let hint_name = append(bytes, b"\x01\x00Function\0\0", 2);

        let thunk_value = u64::from(self.address(hint_name, legacy));
        let mut thunks = Vec::new();
        for value in [thunk_value, 0] {
            match self.bitness {
                Bitness::X86 => thunks.extend_from_slice(&(value as u32).to_le_bytes()),
                Bitness::X64 => thunks.extend_from_slice(&value.to_le_bytes()),
            }
        }
        ImportData {
            name,
            name_table: append(bytes, &thunks, 8),
            address_table: append(bytes, &thunks, 8),
        }
    }

    /// An RVA as written into the image: unchanged, or as a 32-bit virtual
    /// address for legacy delay-load data.
    fn address(&self, rva: u32, legacy: bool) -> u32 {
        if legacy {
            u32::try_from(self.image_base() + u64::from(rva))
                .expect("legacy delay-load descriptors need a 32-bit image")
        } else {
            rva
        }
    }
}

#[derive(Debug)]
struct Section {
    bytes: Vec<u8>,
    /// Import directory (RVA, size).
    imports: (u32, u32),
    /// Delay-load import directory (RVA, size).
    delay_imports: (u32, u32),
}

/// RVAs of one DLL's import data.
#[derive(Debug)]
struct ImportData {
    name: u32,
    name_table: u32,
    address_table: u32,
}

fn section_header(out: &mut Vec<u8>, virtual_size: u32, raw_size: usize) {
    let start = out.len();
    out.extend_from_slice(b".idata\0\0");
    out.extend_from_slice(&virtual_size.to_le_bytes());
    out.extend_from_slice(&SECTION_RVA.to_le_bytes());
    out.extend_from_slice(&(raw_size as u32).to_le_bytes());
    out.extend_from_slice(&(FILE_ALIGNMENT as u32).to_le_bytes()); // pointer to raw data
    out.extend_from_slice(&[0; 12]); // relocations and line numbers
    out.extend_from_slice(&0xc000_0040_u32.to_le_bytes()); // initialized data, read, write
    assert_eq!(out.len() - start, SECTION_HEADER_LEN);
}

/// Size of a descriptor array with its null terminator; nothing if empty.
fn descriptor_array_len(count: usize, descriptor_len: usize) -> usize {
    if count == 0 {
        0
    } else {
        (count + 1) * descriptor_len
    }
}

/// Data directory entry for `len` bytes at section offset `offset`.
fn directory(offset: usize, len: usize) -> (u32, u32) {
    if len == 0 {
        (0, 0)
    } else {
        (rva(offset), len as u32)
    }
}

/// Append `data` at the next multiple of `alignment` and return its RVA.
fn append(bytes: &mut Vec<u8>, data: &[u8], alignment: usize) -> u32 {
    bytes.resize(align(bytes.len(), alignment), 0);
    let offset = bytes.len();
    bytes.extend_from_slice(data);
    rva(offset)
}

fn rva(section_offset: usize) -> u32 {
    SECTION_RVA + u32::try_from(section_offset).expect("small section")
}

fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn set_flag(field: &mut u16, flag: u16, on: bool) {
    if on {
        *field |= flag;
    } else {
        *field &= !flag;
    }
}

fn align(value: usize, alignment: usize) -> usize {
    value.div_ceil(alignment) * alignment
}

fn align_u32(value: u32, alignment: u32) -> u32 {
    value.div_ceil(alignment) * alignment
}

/// Offsets into a built image, for tests that corrupt specific fields.
pub mod layout {
    /// File offset of the import directory's data-directory entry (RVA, size).
    pub fn import_directory_entry(builder: &super::PeBuilder) -> usize {
        data_directory_entry(builder, super::IMPORT_DIRECTORY)
    }

    /// File offset of the delay-load directory's data-directory entry.
    pub fn delay_import_directory_entry(builder: &super::PeBuilder) -> usize {
        data_directory_entry(builder, super::DELAY_IMPORT_DIRECTORY)
    }

    /// File offset of the section's raw data, where the import descriptors start.
    pub const SECTION_DATA: usize = super::FILE_ALIGNMENT;

    /// RVA of the section.
    pub const SECTION_RVA: u32 = super::SECTION_RVA;

    fn data_directory_entry(builder: &super::PeBuilder, index: usize) -> usize {
        let directories =
            super::NT_HEADERS_OFFSET + 4 + super::COFF_HEADER_LEN + builder.optional_header_len()
                - super::DATA_DIRECTORIES * 8;
        directories + index * 8
    }
}
