//! Find graphics DLL names embedded as strings, the arguments of
//! `LoadLibrary` calls that never show up in an import table.
//!
//! The scan is a single pass over the `.` bytes: every candidate ends in
//! `.dll`, so each dot is checked for a `.dll` extension in either encoding,
//! and the name in front of it is read backwards up to its delimiter. Every
//! byte is visited a bounded number of times, which matters for executables
//! of hundreds of megabytes.

use std::collections::BTreeSet;

use crate::{GRAPHICS_DLLS, GraphicsApi, api_for_dll};

/// The file extension every name in [`GRAPHICS_DLLS`] ends with.
const EXTENSION: &[u8] = b".dll";

/// Length of the longest name in [`GRAPHICS_DLLS`] without its extension.
/// A longer run of name characters in front of `.dll` cannot be a match.
const MAX_STEM_LEN: usize = {
    let mut longest = 0;
    let mut i = 0;
    while i < GRAPHICS_DLLS.len() {
        let stem = GRAPHICS_DLLS[i].0.len() - EXTENSION.len();
        if stem > longest {
            longest = stem;
        }
        i += 1;
    }
    longest
};

/// How a string is stored in the binary.
#[derive(Debug, Clone, Copy)]
enum Encoding {
    Ascii,
    Utf16Le,
}

impl Encoding {
    const ALL: [Self; 2] = [Self::Ascii, Self::Utf16Le];

    /// Bytes per code unit.
    fn unit(self) -> usize {
        match self {
            Self::Ascii => 1,
            Self::Utf16Le => 2,
        }
    }

    /// The byte value of the code unit at `at` if it is a single-byte
    /// character (for UTF-16LE: a unit whose high byte is zero).
    fn char_at(self, bytes: &[u8], at: usize) -> Option<u8> {
        match self {
            Self::Ascii => bytes.get(at).copied(),
            Self::Utf16Le => match bytes.get(at..)?.first_chunk() {
                Some(&[low, 0]) => Some(low),
                _ => None,
            },
        }
    }

    fn is_name_char_at(self, bytes: &[u8], at: usize) -> bool {
        self.char_at(bytes, at).is_some_and(is_name_char)
    }

    /// `text` (ASCII) is stored at `at`, compared case-insensitively.
    fn matches_at(self, bytes: &[u8], at: usize, text: &[u8]) -> bool {
        text.iter().enumerate().all(|(i, expected)| {
            self.char_at(bytes, at + i * self.unit())
                .is_some_and(|found| found.eq_ignore_ascii_case(expected))
        })
    }
}

/// Characters that can be part of a DLL name and so cannot delimit one.
/// Besides ASCII letters and digits this includes `_` and `-`, which occur in
/// the names themselves (`d3d10_1.dll`, `vulkan-1.dll`).
fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

/// See [`crate::string_dll_refs`].
pub(crate) fn graphics_dll_refs(bytes: &[u8]) -> Vec<(String, GraphicsApi)> {
    let mut found = BTreeSet::new();
    for (dot, _) in bytes.iter().enumerate().filter(|&(_, &byte)| byte == b'.') {
        for encoding in Encoding::ALL {
            if let Some(hit) = graphics_dll_ending_at(bytes, dot, encoding) {
                found.insert(hit);
            }
        }
    }
    found.into_iter().collect()
}

/// The graphics DLL whose `.dll` extension starts at `dot`, if the name is
/// delimited on both sides.
fn graphics_dll_ending_at(
    bytes: &[u8],
    dot: usize,
    encoding: Encoding,
) -> Option<(String, GraphicsApi)> {
    let unit = encoding.unit();
    if !encoding.matches_at(bytes, dot, EXTENSION) {
        return None;
    }
    if encoding.is_name_char_at(bytes, dot + EXTENSION.len() * unit) {
        return None;
    }

    let mut start = dot;
    let mut stem_len = 0;
    while stem_len <= MAX_STEM_LEN && start >= unit && encoding.is_name_char_at(bytes, start - unit)
    {
        start -= unit;
        stem_len += 1;
    }
    if stem_len == 0 || stem_len > MAX_STEM_LEN {
        return None;
    }

    let mut name: String = (0..stem_len)
        .filter_map(|i| encoding.char_at(bytes, start + i * unit))
        .map(|byte| char::from(byte.to_ascii_lowercase()))
        .collect();
    name.push_str(".dll");
    let api = api_for_dll(&name)?;
    Some((name, api))
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn utf16(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }

    fn refs(bytes: &[u8]) -> Vec<(String, GraphicsApi)> {
        graphics_dll_refs(bytes)
    }

    fn hit(name: &str, api: GraphicsApi) -> (String, GraphicsApi) {
        (name.to_owned(), api)
    }

    #[test]
    fn longest_stem_is_d3d10core() {
        assert_eq!(MAX_STEM_LEN, "d3d10core".len());
    }

    #[test]
    fn finds_an_ascii_name_between_nul_bytes() {
        assert_eq!(
            refs(b"\0\0d3d11.dll\0\0"),
            [hit("d3d11.dll", GraphicsApi::D3d11)]
        );
    }

    #[test]
    fn finds_a_utf16_name() {
        let mut bytes = vec![0xff, 0xfe];
        bytes.extend(utf16("opengl32.dll"));
        bytes.extend([0, 0]);
        assert_eq!(refs(&bytes), [hit("opengl32.dll", GraphicsApi::OpenGl)]);
    }

    #[test]
    fn finds_a_utf16_name_at_an_odd_offset() {
        let mut bytes = vec![0x41];
        bytes.extend(utf16("d3d9.dll"));
        assert_eq!(refs(&bytes), [hit("d3d9.dll", GraphicsApi::D3d9)]);
    }

    #[test]
    fn matching_is_case_insensitive_and_reports_lowercase() {
        assert_eq!(refs(b"D3D9.DLL"), [hit("d3d9.dll", GraphicsApi::D3d9)]);
        assert_eq!(
            refs(&utf16("Vulkan-1.Dll")),
            [hit("vulkan-1.dll", GraphicsApi::Vulkan)]
        );
    }

    #[test]
    fn names_at_the_buffer_edges_are_delimited() {
        assert_eq!(
            refs(b"ddraw.dll"),
            [hit("ddraw.dll", GraphicsApi::DirectDraw)]
        );
        assert_eq!(
            refs(&utf16("ddraw.dll")),
            [hit("ddraw.dll", GraphicsApi::DirectDraw)]
        );
    }

    #[test]
    fn path_separators_and_punctuation_delimit_names() {
        assert_eq!(
            refs(b"C:\\Windows\\System32\\d3d8.dll"),
            [hit("d3d8.dll", GraphicsApi::D3d8)]
        );
        assert_eq!(
            refs(b"lib/d3d12.dll"),
            [hit("d3d12.dll", GraphicsApi::D3d12)]
        );
        assert_eq!(
            refs(b"Failed to load \"d3d11.dll\"."),
            [hit("d3d11.dll", GraphicsApi::D3d11)]
        );
        assert_eq!(
            refs(b"cannot load d3d11.dll."),
            [hit("d3d11.dll", GraphicsApi::D3d11)]
        );
    }

    #[test]
    fn a_longer_name_ending_in_a_graphics_dll_does_not_count() {
        assert!(refs(b"myd3d9.dll").is_empty());
        assert!(refs(b"\0myd3d9.dll\0").is_empty());
        assert!(refs(&utf16("myd3d9.dll")).is_empty());
        assert!(refs(b"_d3d9.dll").is_empty());
        assert!(refs(b"my-d3d9.dll").is_empty());
        assert!(refs(b"xd3d10_1.dll").is_empty());
    }

    #[test]
    fn a_name_followed_by_more_name_characters_does_not_count() {
        assert!(refs(b"d3d9.dlls").is_empty());
        assert!(refs(b"d3d9.dll_old").is_empty());
        assert!(refs(&utf16("d3d9.dllx")).is_empty());
    }

    #[test]
    fn names_of_other_dlls_are_ignored() {
        assert!(refs(b"dxgi.dll kernel32.dll d3dgl.dll d3dx9_43.dll d3d9").is_empty());
    }

    #[test]
    fn utf16_units_with_a_high_byte_are_not_ascii() {
        // "d3d9.dll" with U+0164 in place of 'd' in the extension.
        let mut bytes = utf16("d3d9.");
        bytes.extend([b'd', 0x01, b'l', 0, b'l', 0]);
        assert!(refs(&bytes).is_empty());
    }

    #[test]
    fn a_non_ascii_utf16_character_delimits_a_name() {
        assert_eq!(
            refs(&utf16("\u{4e2d}d3d10.dll")),
            [hit("d3d10.dll", GraphicsApi::D3d10)]
        );
    }

    #[test]
    fn mixed_encodings_do_not_join_into_a_name() {
        // ASCII stem followed by a UTF-16 extension.
        let mut bytes = b"d3d9".to_vec();
        bytes.extend(utf16(".dll"));
        assert!(refs(&bytes).is_empty());
    }

    #[test]
    fn every_graphics_dll_is_recognised() {
        for &(name, api) in GRAPHICS_DLLS {
            assert_eq!(refs(name.as_bytes()), [hit(name, api)], "{name}");
            assert_eq!(refs(&utf16(name)), [hit(name, api)], "{name} (UTF-16)");
        }
    }

    #[test]
    fn results_are_sorted_and_deduplicated() {
        let mut bytes = b"vulkan-1.dll\0D3D11.dll\0d3d11.dll\0d3d10_1.dll\0".to_vec();
        bytes.extend(utf16("d3d11.dll\0d3d10core.dll"));
        assert_eq!(
            refs(&bytes),
            [
                hit("d3d10_1.dll", GraphicsApi::D3d10),
                hit("d3d10core.dll", GraphicsApi::D3d10),
                hit("d3d11.dll", GraphicsApi::D3d11),
                hit("vulkan-1.dll", GraphicsApi::Vulkan),
            ]
        );
    }

    #[test]
    fn a_long_run_of_name_characters_is_rejected() {
        let mut bytes = vec![b'a'; 10_000];
        bytes.extend_from_slice(b"d3d9.dll");
        assert!(refs(&bytes).is_empty());
    }

    #[test]
    fn dots_without_an_extension_are_harmless() {
        assert!(refs(b".").is_empty());
        assert!(refs(b"....dll").is_empty());
        assert!(refs(b"d3d9.").is_empty());
        assert!(refs(b"d3d9.dl").is_empty());
        assert!(refs(&[b'.', 0]).is_empty());
    }

    fn graphics_dll() -> impl Strategy<Value = (&'static str, GraphicsApi)> {
        proptest::sample::select(GRAPHICS_DLLS)
    }

    /// Bytes that cannot extend a DLL name: anything but a name character.
    fn delimiter() -> impl Strategy<Value = u8> {
        any::<u8>().prop_filter("not a name character", |&byte| !is_name_char(byte))
    }

    proptest! {
        #[test]
        fn never_panics(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            let _ = refs(&bytes);
        }

        #[test]
        fn every_result_names_a_graphics_dll(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
            for (name, api) in refs(&bytes) {
                prop_assert_eq!(api_for_dll(&name), Some(api));
            }
        }

        #[test]
        fn a_delimited_ascii_name_is_always_found(
            prefix in proptest::collection::vec(any::<u8>(), 0..64),
            before in delimiter(),
            (name, api) in graphics_dll(),
            uppercase in any::<bool>(),
            after in delimiter(),
            suffix in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            let mut bytes = prefix;
            bytes.push(before);
            let name_bytes = if uppercase { name.to_ascii_uppercase() } else { name.to_owned() };
            bytes.extend_from_slice(name_bytes.as_bytes());
            bytes.push(after);
            bytes.extend(suffix);
            prop_assert!(refs(&bytes).contains(&hit(name, api)));
        }

        #[test]
        fn a_delimited_utf16_name_is_always_found(
            prefix in proptest::collection::vec(any::<u8>(), 0..64),
            (name, api) in graphics_dll(),
            suffix in proptest::collection::vec(any::<u8>(), 0..64),
        ) {
            let mut bytes = prefix;
            bytes.extend([0, 0]);
            bytes.extend(utf16(name));
            bytes.extend([0, 0]);
            bytes.extend(suffix);
            prop_assert!(refs(&bytes).contains(&hit(name, api)));
        }

        #[test]
        fn a_name_with_a_leading_name_character_is_never_found(
            (name, _) in graphics_dll(),
            extra in "[a-zA-Z0-9_-]",
        ) {
            let text = format!("\0{extra}{name}\0");
            prop_assert!(refs(text.as_bytes()).is_empty());
            prop_assert!(refs(&utf16(&text)).is_empty());
        }
    }
}
