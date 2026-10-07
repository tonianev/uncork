//! Minimal edits to Windows INI files that games keep their settings in.
//!
//! Profiles can set keys (for example Rise of Nations' `SkipIntroMovies=1`
//! in `%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini`). Edits preserve
//! everything else byte for byte: line order, comments, unknown keys, the
//! file's line endings (CRLF or LF) and its encoding (UTF-8/ASCII, or UTF-16LE
//! with BOM, which is written back as UTF-16LE with BOM).
//!
//! [`apply_to_file`] reads the same encodings Wine's profile API does: UTF-8
//! (with or without BOM), UTF-16LE or UTF-16BE with BOM, and otherwise a
//! legacy 8-bit ("ANSI") code page, whose bytes are carried through
//! unchanged.

use std::fs;
use std::io;
use std::path::Path;

use crate::Error;

/// One `key=value` to enforce in `[section]`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IniSet {
    /// Section name without brackets, matched case-insensitively.
    pub section: String,
    /// Key, matched case-insensitively and with surrounding whitespace trimmed.
    pub key: String,
    /// Value to write.
    pub value: String,
}

/// Apply `sets` to INI text. Existing keys are rewritten in place (keeping
/// the original key spelling); missing keys are appended at the end of
/// their section; missing sections are appended at the end of the text.
/// Returns the new text and whether anything changed.
///
/// Details: a key is rewritten wherever it occurs in a section of that name;
/// a missing key goes after the last non-blank line of the first such
/// section; values are compared and written without surrounding whitespace,
/// which INI readers strip anyway. New lines use the text's first line
/// ending (CRLF if it has none), and a text that did not end with a line
/// break still does not. Sets that cannot be written as one INI line (a line
/// break anywhere, an empty key, a key containing `=` or starting with `;`,
/// `#` or `[`, a section containing `]`) are skipped with a warning.
#[must_use]
pub fn apply(text: &str, sets: &[IniSet]) -> (String, bool) {
    // A UTF-8 byte-order mark decodes to U+FEFF; keep it out of the way of
    // the first line's parsing.
    let (bom, body) = match text.strip_prefix(BOM) {
        Some(body) => (BOM, body),
        None => ("", text),
    };
    let mut document = Document::parse(body);
    for set in sets {
        match Edit::new(set) {
            Some(edit) => document.apply(&edit),
            None => tracing::warn!(
                section = %set.section,
                key = %set.key,
                "skipping an INI setting that cannot be written as one INI line"
            ),
        }
    }
    let new_text = format!("{bom}{}", document.render());
    let changed = new_text != text;
    (new_text, changed)
}

/// Apply `sets` to the file at `path` if it exists (games create their INI
/// on first run; a missing file is not an error and returns `Ok(false)`).
/// Writes atomically only when something changed.
///
/// A symlink is written through, not replaced, and the file keeps its
/// permissions.
///
/// # Errors
/// [`crate::Error::Io`], including for files in no supported encoding (NUL
/// bytes without a UTF-16 BOM, malformed UTF-16) and for new values an
/// 8-bit file cannot hold.
pub fn apply_to_file(path: &Path, sets: &[IniSet]) -> crate::Result<bool> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(Error::io("cannot read", path, e)),
    };
    let unsupported = |message: &'static str| {
        Error::io(
            "cannot edit INI file",
            path,
            io::Error::new(io::ErrorKind::InvalidData, message),
        )
    };
    let (encoding, text) = Encoding::decode(&bytes).map_err(unsupported)?;
    let (new_text, changed) = apply(&text, sets);
    if !changed {
        return Ok(false);
    }
    let new_bytes = encoding.encode(&new_text).map_err(unsupported)?;
    let target = fs::canonicalize(path).map_err(|e| Error::io("cannot resolve", path, e))?;
    crate::config::write_atomic(&target, &new_bytes)?;
    Ok(true)
}

/// A byte-order mark, as text.
const BOM: &str = "\u{FEFF}";
const CRLF: &str = "\r\n";
const LF: &str = "\n";

/// A validated [`IniSet`], trimmed.
struct Edit<'a> {
    section: &'a str,
    key: &'a str,
    value: &'a str,
}

impl<'a> Edit<'a> {
    /// `None` when the set cannot be written without changing the meaning of
    /// the surrounding lines.
    fn new(set: &'a IniSet) -> Option<Edit<'a>> {
        let breaks_line = |s: &str| s.contains(['\r', '\n']);
        let section = set.section.trim();
        let key = set.key.trim();
        let value = set.value.trim();
        let valid = !breaks_line(section)
            && !section.contains(']')
            && !key.is_empty()
            && !breaks_line(key)
            && !key.contains('=')
            && !key.starts_with([';', '#', '['])
            && !breaks_line(value);
        valid.then_some(Edit {
            section,
            key,
            value,
        })
    }
}

/// What a line means to an INI reader.
enum LineKind<'a> {
    /// `[name]`, name trimmed.
    Section(&'a str),
    /// `key=value`, key trimmed.
    Entry(&'a str),
    /// Comment, blank line or anything else, kept as is.
    Other,
}

fn classify(content: &str) -> LineKind<'_> {
    let line = content.trim_start();
    if let Some(rest) = line.strip_prefix('[') {
        return match rest.find(']') {
            Some(end) => LineKind::Section(rest[..end].trim()),
            None => LineKind::Other,
        };
    }
    if line.starts_with([';', '#']) {
        return LineKind::Other;
    }
    match line.split_once('=') {
        Some((key, _)) if !key.trim().is_empty() => LineKind::Entry(key.trim()),
        _ => LineKind::Other,
    }
}

/// Case-insensitive comparison, as Windows compares section and key names.
fn same_name(a: &str, b: &str) -> bool {
    a.chars()
        .flat_map(char::to_lowercase)
        .eq(b.chars().flat_map(char::to_lowercase))
}

/// One line without its terminator, plus the terminator.
struct Line {
    content: String,
    ending: &'static str,
}

impl Line {
    fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }
}

/// INI text as lines. Every line carries an ending while editing; whether
/// the text ended with one is remembered separately and restored on render.
struct Document {
    lines: Vec<Line>,
    /// Ending for lines this module adds.
    eol: &'static str,
    final_newline: bool,
}

impl Document {
    fn parse(text: &str) -> Document {
        let eol = match text.find('\n') {
            Some(index) if !text[..index].ends_with('\r') => LF,
            _ => CRLF,
        };
        let mut final_newline = true;
        let lines = text
            .split_inclusive('\n')
            .map(|piece| {
                let (content, ending) = if let Some(content) = piece.strip_suffix(CRLF) {
                    (content, CRLF)
                } else if let Some(content) = piece.strip_suffix(LF) {
                    (content, LF)
                } else {
                    final_newline = false;
                    (piece, eol)
                };
                Line {
                    content: content.to_owned(),
                    ending,
                }
            })
            .collect();
        Document {
            lines,
            eol,
            final_newline,
        }
    }

    fn render(&self) -> String {
        let mut out: String = self
            .lines
            .iter()
            .flat_map(|line| [line.content.as_str(), line.ending])
            .collect();
        if !self.final_newline
            && let Some(last) = self.lines.last()
        {
            out.truncate(out.len() - last.ending.len());
        }
        out
    }

    fn apply(&mut self, edit: &Edit<'_>) {
        if !self.rewrite_existing(edit) {
            self.insert(edit);
        }
    }

    /// Set the value wherever the key occurs in a section with the edit's
    /// name. Returns whether it occurs at all.
    fn rewrite_existing(&mut self, edit: &Edit<'_>) -> bool {
        let mut found = false;
        let mut in_section = false;
        for line in &mut self.lines {
            let is_target = match classify(&line.content) {
                LineKind::Section(name) => {
                    in_section = same_name(name, edit.section);
                    false
                }
                LineKind::Entry(key) => in_section && same_name(key, edit.key),
                LineKind::Other => false,
            };
            if is_target {
                found = true;
                if let Some(rewritten) = with_value(&line.content, edit.value) {
                    line.content = rewritten;
                }
            }
        }
        found
    }

    /// Add the key to the first section with the edit's name, or append that
    /// section to the document.
    fn insert(&mut self, edit: &Edit<'_>) {
        let entry = Line {
            content: format!("{}={}", edit.key, edit.value),
            ending: self.eol,
        };
        if let Some(index) = self.section_end(edit.section) {
            self.lines.insert(index, entry);
            return;
        }
        if self.lines.last().is_some_and(|line| !line.is_blank()) {
            self.lines.push(Line {
                content: String::new(),
                ending: self.eol,
            });
        }
        self.lines.push(Line {
            content: format!("[{}]", edit.section),
            ending: self.eol,
        });
        self.lines.push(entry);
    }

    /// Index just past the last non-blank line of the first section named
    /// `name`, so trailing blank lines stay between it and the next section.
    fn section_end(&self, name: &str) -> Option<usize> {
        let header = self.lines.iter().position(|line| {
            matches!(classify(&line.content), LineKind::Section(found) if same_name(found, name))
        })?;
        let mut end = header + 1;
        for (index, line) in self.lines.iter().enumerate().skip(header + 1) {
            if matches!(classify(&line.content), LineKind::Section(_)) {
                break;
            }
            if !line.is_blank() {
                end = index + 1;
            }
        }
        Some(end)
    }
}

/// `content` (an entry line) with its value replaced, keeping everything up
/// to and including `=` and the whitespace after it. `None` if the value is
/// already `value`.
fn with_value(content: &str, value: &str) -> Option<String> {
    let (head, current) = content.split_once('=')?;
    if current.trim() == value {
        return None;
    }
    let gap = &current[..current.len() - current.trim_start().len()];
    Some(format!("{head}={gap}{value}"))
}

/// How a file's bytes map to text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Encoding {
    /// UTF-8; a BOM, if any, is part of the text as U+FEFF.
    Utf8,
    /// UTF-16 with a BOM, which is part of the text as U+FEFF.
    Utf16 { big_endian: bool },
    /// Not UTF-8: each byte is the character with the same value
    /// (ISO 8859-1), so every byte survives the round trip.
    Ansi,
}

impl Encoding {
    fn decode(bytes: &[u8]) -> Result<(Encoding, String), &'static str> {
        if bytes.starts_with(&[0xFF, 0xFE]) {
            return decode_utf16(bytes, u16::from_le_bytes)
                .map(|text| (Encoding::Utf16 { big_endian: false }, text));
        }
        if bytes.starts_with(&[0xFE, 0xFF]) {
            return decode_utf16(bytes, u16::from_be_bytes)
                .map(|text| (Encoding::Utf16 { big_endian: true }, text));
        }
        if bytes.contains(&0) {
            return Err("it contains NUL bytes (UTF-16 without a byte-order mark?)");
        }
        match String::from_utf8(bytes.to_vec()) {
            Ok(text) => Ok((Encoding::Utf8, text)),
            Err(_) if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) => {
                Err("it starts with a UTF-8 byte-order mark but is not valid UTF-8")
            }
            Err(_) => Ok((
                Encoding::Ansi,
                bytes.iter().copied().map(char::from).collect(),
            )),
        }
    }

    fn encode(self, text: &str) -> Result<Vec<u8>, &'static str> {
        match self {
            Encoding::Utf8 => Ok(text.as_bytes().to_vec()),
            Encoding::Utf16 { big_endian } => Ok(text
                .encode_utf16()
                .flat_map(|unit| {
                    if big_endian {
                        unit.to_be_bytes()
                    } else {
                        unit.to_le_bytes()
                    }
                })
                .collect()),
            Encoding::Ansi => text
                .chars()
                .map(|c| {
                    u8::try_from(c).map_err(
                        |_| "a new value has characters the file's 8-bit encoding cannot hold",
                    )
                })
                .collect(),
        }
    }
}

fn decode_utf16(bytes: &[u8], unit: fn([u8; 2]) -> u16) -> Result<String, &'static str> {
    let (pairs, rest) = bytes.as_chunks::<2>();
    if !rest.is_empty() {
        return Err("it has a UTF-16 byte-order mark but an odd number of bytes");
    }
    let units: Vec<u16> = pairs.iter().copied().map(unit).collect();
    String::from_utf16(&units)
        .map_err(|_| "it has a UTF-16 byte-order mark but is not valid UTF-16")
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn set(section: &str, key: &str, value: &str) -> IniSet {
        IniSet {
            section: section.to_owned(),
            key: key.to_owned(),
            value: value.to_owned(),
        }
    }

    fn applied(text: &str, sets: &[IniSet]) -> String {
        let (out, changed) = apply(text, sets);
        assert_eq!(changed, out != text);
        out
    }

    /// What a Windows-style reader sees: the first entry for `key` in a
    /// section named `section`, value trimmed.
    fn lookup(text: &str, section: &str, key: &str) -> Option<String> {
        let mut in_section = false;
        for line in text.trim_start_matches(BOM).split('\n') {
            match classify(line) {
                LineKind::Section(name) => in_section = same_name(name, section),
                LineKind::Entry(found) if in_section && same_name(found, key) => {
                    return Some(line.split_once('=')?.1.trim().to_owned());
                }
                _ => {}
            }
        }
        None
    }

    const RISE2: &str = "[RISE OF NATIONS]\r\n\
                         ; written by the game\r\n\
                         Fullscreen=1\r\n\
                         SkipIntroMovies = 0\r\n\
                         VSync=1\r\n\
                         \r\n\
                         [Network]\r\n\
                         Port=34987\r\n";

    #[test]
    fn rewrites_in_place_keeping_spelling_spacing_and_endings() {
        let out = applied(RISE2, &[set("rise of nations", "skipintromovies", "1")]);
        assert_eq!(
            out,
            RISE2.replace("SkipIntroMovies = 0", "SkipIntroMovies = 1")
        );
    }

    #[test]
    fn unchanged_value_reports_no_change() {
        let sets = [set("RISE OF NATIONS", "VSync", "1")];
        assert_eq!(apply(RISE2, &sets), (RISE2.to_owned(), false));
        // Surrounding whitespace is not part of a value.
        let sets = [set("RISE OF NATIONS", " VSync ", " 1 ")];
        assert_eq!(apply(RISE2, &sets), (RISE2.to_owned(), false));
        assert!(!apply("[A]\nk=1   \n", &[set("A", "k", "1")]).1);
    }

    #[test]
    fn appends_missing_key_before_trailing_blank_lines() {
        let out = applied(
            RISE2,
            &[set("Rise of Nations", "IgnoreMinimizeOnTabOut", "1")],
        );
        assert_eq!(
            out,
            RISE2.replace("VSync=1\r\n", "VSync=1\r\nIgnoreMinimizeOnTabOut=1\r\n")
        );
    }

    #[test]
    fn appends_after_trailing_comments_of_the_section() {
        let text = "[A]\nx=1\n; note\n\n\n[B]\n";
        assert_eq!(
            applied(text, &[set("A", "y", "2")]),
            "[A]\nx=1\n; note\ny=2\n\n\n[B]\n"
        );
    }

    #[test]
    fn appends_into_an_empty_section_right_after_its_header() {
        assert_eq!(
            applied("[A]\n\n[B]\nz=0\n", &[set("a", "k", "v")]),
            "[A]\nk=v\n\n[B]\nz=0\n"
        );
    }

    #[test]
    fn appends_missing_section_at_the_end() {
        let out = applied(
            RISE2,
            &[
                set("Video", "Width", "1920"),
                set("video", "Height", "1080"),
            ],
        );
        assert_eq!(
            out,
            format!("{RISE2}\r\n[Video]\r\nWidth=1920\r\nHeight=1080\r\n")
        );
    }

    #[test]
    fn missing_section_after_trailing_blank_line_adds_no_extra_blank() {
        assert_eq!(
            applied("[A]\nx=1\n\n", &[set("B", "k", "v")]),
            "[A]\nx=1\n\n[B]\nk=v\n"
        );
    }

    #[test]
    fn empty_text_gets_a_crlf_section() {
        assert_eq!(applied("", &[set("S", "k", "v")]), "[S]\r\nk=v\r\n");
    }

    #[test]
    fn missing_final_newline_stays_missing() {
        assert_eq!(applied("[A]\nx=1", &[set("A", "x", "2")]), "[A]\nx=2");
        assert_eq!(applied("[A]\nx=1", &[set("A", "y", "2")]), "[A]\nx=1\ny=2");
        assert_eq!(
            applied("[A]\nx=1", &[set("B", "y", "2")]),
            "[A]\nx=1\n\n[B]\ny=2"
        );
        assert_eq!(applied("[A]", &[set("A", "y", "2")]), "[A]\r\ny=2");
    }

    #[test]
    fn new_lines_follow_the_first_line_ending() {
        assert_eq!(
            applied("[A]\nx=1\r\n", &[set("A", "y", "2")]),
            "[A]\nx=1\r\ny=2\n"
        );
        assert_eq!(
            applied("[A]\r\nx=1\n", &[set("A", "y", "2")]),
            "[A]\r\nx=1\ny=2\r\n"
        );
    }

    #[test]
    fn rewrites_every_occurrence_in_every_matching_section() {
        let text = "[A]\nk=1\nK=1\n[B]\nk=1\n[a]\nk = 1\n";
        assert_eq!(
            applied(text, &[set("A", "k", "2")]),
            "[A]\nk=2\nK=2\n[B]\nk=1\n[a]\nk = 2\n"
        );
    }

    #[test]
    fn missing_key_goes_into_the_first_matching_section() {
        assert_eq!(
            applied("[A]\nx=1\n[B]\n[a]\ny=1\n", &[set("A", "z", "3")]),
            "[A]\nx=1\nz=3\n[B]\n[a]\ny=1\n"
        );
    }

    #[test]
    fn keys_before_any_section_are_not_in_a_section() {
        assert_eq!(applied("k=1\n", &[set("", "k", "2")]), "k=1\n\n[]\nk=2\n");
    }

    #[test]
    fn comments_and_lookalikes_are_left_alone() {
        let text = "[A]\n;k=1\n# k=1\nk\n=1\n[k=1\n";
        let out = applied(text, &[set("A", "k", "2")]);
        assert_eq!(out, "[A]\n;k=1\n# k=1\nk\n=1\n[k=1\nk=2\n");
    }

    #[test]
    fn section_headers_tolerate_whitespace_and_trailing_text() {
        let text = "  [ Video ]  ; display\nw=1\n";
        assert_eq!(
            applied(text, &[set("video", "w", "2")]),
            "  [ Video ]  ; display\nw=2\n"
        );
    }

    #[test]
    fn values_may_contain_equals_and_semicolons() {
        let text = "[A]\nurl=a=b\n";
        let out = applied(text, &[set("A", "url", "c=d ; e")]);
        assert_eq!(out, "[A]\nurl=c=d ; e\n");
        assert_eq!(lookup(&out, "a", "URL").as_deref(), Some("c=d ; e"));
        assert!(!apply(&out, &[set("A", "url", "c=d ; e")]).1);
    }

    #[test]
    fn whitespace_only_value_keeps_its_gap() {
        assert_eq!(
            applied("[A]\nk=  \n", &[set("A", "k", "v")]),
            "[A]\nk=  v\n"
        );
        assert_eq!(applied("[A]\nk=old\n", &[set("A", "k", "")]), "[A]\nk=\n");
    }

    #[test]
    fn matching_is_unicode_case_insensitive() {
        let text = "[Ünïcode]\nSchlüssel=1\n";
        assert_eq!(
            applied(text, &[set("üNÏCODE", "SCHLÜSSEL", "2")]),
            "[Ünïcode]\nSchlüssel=2\n"
        );
    }

    #[test]
    fn unrepresentable_sets_are_skipped() {
        let text = "[A]\nk=1\n";
        for bad in [
            set("A", "k", "two\nlines"),
            set("A", "", "1"),
            set("A", "  ", "1"),
            set("A", "a=b", "1"),
            set("A", ";k", "1"),
            set("A", "#k", "1"),
            set("A", "[k", "1"),
            set("A]", "k", "1"),
            set("A\r", "k\nx", "1"),
        ] {
            assert_eq!(
                apply(text, std::slice::from_ref(&bad)),
                (text.to_owned(), false),
                "{bad:?}"
            );
        }
        // Valid sets alongside are still applied.
        assert_eq!(
            applied(text, &[set("A", "", "1"), set("A", "k", "2")]),
            "[A]\nk=2\n"
        );
    }

    #[test]
    fn surrounding_line_breaks_in_values_are_trimmed_not_rejected() {
        assert_eq!(
            applied("[A]\nk=1\n", &[set("A", "k", "2\r\n")]),
            "[A]\nk=2\n"
        );
    }

    #[test]
    fn later_sets_win() {
        let sets = [set("A", "k", "1"), set("a", "K", "2")];
        let out = applied("", &sets);
        assert_eq!(out, "[A]\r\nk=2\r\n");
        assert!(!apply(&out, &sets).1);
    }

    #[test]
    fn utf8_bom_is_preserved_and_does_not_hide_the_first_header() {
        let text = "\u{FEFF}[A]\r\nk=1\r\n";
        assert_eq!(
            applied(text, &[set("A", "k", "2")]),
            "\u{FEFF}[A]\r\nk=2\r\n"
        );
        assert_eq!(
            applied("\u{FEFF}", &[set("A", "k", "2")]),
            "\u{FEFF}[A]\r\nk=2\r\n"
        );
    }

    #[test]
    fn no_sets_means_no_change() {
        for text in ["", "\n", "x", "[A]\r\n\r\n", "\u{FEFF}garbage\r"] {
            assert_eq!(apply(text, &[]), (text.to_owned(), false));
        }
    }

    #[test]
    fn decodes_and_reencodes_every_supported_encoding() {
        let utf16le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("[A]\r\n".encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let utf16be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("[A]\r\n".encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let cases: [(&[u8], Encoding); 5] = [
            (b"[A]\r\n", Encoding::Utf8),
            (b"\xEF\xBB\xBF[A]\r\n", Encoding::Utf8),
            (&utf16le, Encoding::Utf16 { big_endian: false }),
            (&utf16be, Encoding::Utf16 { big_endian: true }),
            (b"[A]\r\n; caf\xE9 \x80\xFF\r\n", Encoding::Ansi),
        ];
        for (bytes, expected) in cases {
            let (encoding, text) = Encoding::decode(bytes).unwrap();
            assert_eq!(encoding, expected);
            assert!(
                text.trim_start_matches(BOM).starts_with("[A]\r\n"),
                "{text:?}"
            );
            assert_eq!(encoding.encode(&text).unwrap(), bytes);
        }
    }

    #[test]
    fn rejects_undecodable_bytes() {
        assert!(Encoding::decode(b"[A]\0\r\n").is_err());
        assert!(Encoding::decode(b"\xEF\xBB\xBF\xFF").is_err());
        assert!(
            Encoding::decode(&[0xFF, 0xFE, b'[']).is_err(),
            "odd UTF-16 length"
        );
        // An unpaired high surrogate.
        assert!(Encoding::decode(&[0xFF, 0xFE, 0x00, 0xD8]).is_err());
    }

    #[test]
    fn ansi_cannot_hold_wide_characters() {
        assert_eq!(Encoding::Ansi.encode("é").unwrap(), [0xE9]);
        assert!(Encoding::Ansi.encode("€").is_err());
    }

    /// One line of a plausible INI file, built from small name pools so that
    /// sets often hit existing sections and keys.
    fn line() -> impl Strategy<Value = String> {
        let name = || prop::sample::select(vec!["Video", "video", "SOUND", "Net", "a b", "Ünï"]);
        let key = || prop::sample::select(vec!["Width", "width", "VSync", "k", "Skip Intro", "ß"]);
        prop_oneof![
            (name(), "[ \t]{0,2}", "[ \t]{0,2}").prop_map(|(n, a, b)| format!("{a}[{b}{n}{b}]")),
            (key(), "[ \t]{0,2}", "[ \t]{0,2}", "[ -~]{0,8}")
                .prop_map(|(k, a, b, v)| format!("{a}{k}{b}={b}{v}")),
            "[;#][ -~]{0,10}",
            "[ \t]{0,3}",
            "[ -~]{0,12}",
            "\\PC{0,8}",
        ]
    }

    fn text() -> impl Strategy<Value = String> {
        (
            any::<bool>(),
            prop::collection::vec((line(), prop::sample::select(vec!["\n", "\r\n"])), 0..12),
            any::<bool>(),
        )
            .prop_map(|(bom, lines, final_newline)| {
                let mut text = if bom { BOM.to_string() } else { String::new() };
                for (line, ending) in lines {
                    text.push_str(&line);
                    text.push_str(ending);
                }
                if !final_newline && text.ends_with('\n') {
                    text.pop();
                    if text.ends_with('\r') {
                        text.pop();
                    }
                }
                text
            })
    }

    fn sets() -> impl Strategy<Value = Vec<IniSet>> {
        let section =
            prop::sample::select(vec!["Video", "VIDEO", "Sound", "New", " a b ", "ünÏ", "x]"]);
        let key = prop::sample::select(vec![
            "Width",
            "WIDTH",
            "vsync",
            "K",
            "skip intro",
            "SS",
            "",
            "a=b",
        ]);
        let value = "[ -~]{0,6}|\\PC{0,4}|.*\n.*";
        prop::collection::vec(
            (section, key, value).prop_map(|(s, k, v)| set(s, k, &v)),
            0..6,
        )
    }

    proptest! {
        #[test]
        fn applying_twice_changes_nothing_more(text in text(), sets in sets()) {
            let (once, changed) = apply(&text, &sets);
            prop_assert_eq!(changed, once != text);
            let (twice, changed_again) = apply(&once, &sets);
            prop_assert_eq!(&twice, &once);
            prop_assert!(!changed_again);
        }

        #[test]
        fn every_valid_set_is_visible_afterwards(text in text(), sets in sets()) {
            let (out, _) = apply(&text, &sets);
            let mut expected: Vec<(&IniSet, &str)> = Vec::new();
            for set in &sets {
                if let Some(edit) = Edit::new(set) {
                    expected.retain(|(earlier, _)| {
                        !(same_name(earlier.section.trim(), edit.section) && same_name(earlier.key.trim(), edit.key))
                    });
                    expected.push((set, edit.value));
                }
            }
            for (set, value) in expected {
                let found = lookup(&out, set.section.trim(), set.key.trim());
                prop_assert_eq!(found.as_deref(), Some(value));
            }
        }

        #[test]
        fn arbitrary_text_never_panics_and_is_idempotent(text in "(\\PC|\r|\n|\\[|\\]|=|;){0,40}", sets in sets()) {
            let (once, _) = apply(&text, &sets);
            prop_assert_eq!(apply(&once, &sets), (once.clone(), false));
        }

        #[test]
        fn without_sets_text_is_untouched(text in "(\\PC|\r|\n){0,40}") {
            prop_assert_eq!(apply(&text, &[]), (text.clone(), false));
        }

        #[test]
        fn ansi_bytes_round_trip(bytes in prop::collection::vec(1u8.., 0..64)) {
            let (encoding, text) = Encoding::decode(&bytes).unwrap();
            prop_assert_eq!(encoding.encode(&text).unwrap(), bytes);
        }
    }
}
