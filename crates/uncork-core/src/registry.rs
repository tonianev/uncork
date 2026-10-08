//! Windows registry edits applied to a bottle with `wine regedit /S`, and
//! reading values from Wine's registry files offline.
//!
//! Uncork renders edits to a `REGEDIT4` file inside the bottle's
//! `drive_c/windows/temp/` and imports it with Wine's `regedit`, which works
//! headless and is the same mechanism winetricks uses. [`file_value`] reads
//! a value from the text of a prefix's `user.reg` or `system.reg` without
//! starting Wine (the files lag behind a running wineserver, which saves
//! them lazily).

use std::fmt::Write as _;

use serde::Serialize;

/// A registry value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum RegValue {
    /// `REG_SZ`.
    Sz(String),
    /// `REG_DWORD`.
    Dword(u32),
    /// Delete the value (`"name"=-`).
    Delete,
}

/// Values to set under one key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegKey {
    /// Full key path with hive, e.g. `HKEY_CURRENT_USER\Software\Wine\Mac Driver`.
    pub path: String,
    /// Values in the order they are written.
    pub values: Vec<(String, RegValue)>,
}

/// Render a `REGEDIT4` document: header line, blank line, then per key
/// `[path]` and one `"name"=...` line per value, keys separated by blank
/// lines, `\` and `"` escaped as `\\` and `\"` inside names and strings,
/// DWORDs as `dword:%08x`, deletions as `-`. Ends with a newline.
///
/// Line feeds and carriage returns inside names and strings are escaped as
/// `\n` and `\r`, as Wine's own `regedit` export does, so a value can never
/// break out of its line. Key paths are written verbatim.
#[must_use]
pub fn render(keys: &[RegKey]) -> String {
    let mut out = String::from("REGEDIT4\n\n");
    for (index, key) in keys.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push('[');
        out.push_str(&key.path);
        out.push_str("]\n");
        for (name, value) in &key.values {
            push_quoted(&mut out, name);
            out.push('=');
            match value {
                RegValue::Sz(text) => push_quoted(&mut out, text),
                RegValue::Dword(number) => {
                    // Writing to a String cannot fail.
                    let _ = write!(out, "dword:{number:08x}");
                }
                RegValue::Delete => out.push('-'),
            }
            out.push('\n');
        }
    }
    out
}

/// `HKEY_CURRENT_USER\Software\Wine\Mac Driver`.
pub const MAC_DRIVER_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Mac Driver";

/// A value as Wine's registry files (`user.reg`, `system.reg`) store it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum FileValue {
    /// `"name"="text"` or `"name"=str(2):"text"`, escapes resolved.
    Sz(String),
    /// `"name"=dword:0000002a`.
    Dword(u32),
    /// Any other type (`hex:`, `hex(7):`, ...), not decoded.
    Other,
}

/// Value `name` of key `key` in `text`, the contents of one of Wine's
/// registry files, or `None` when the key or the value is not there.
///
/// `key` is relative to the file's root and written with single
/// backslashes, for example `Software\Wine\Mac Driver` in `user.reg` (the
/// file itself doubles them: `[Software\\Wine\\Mac Driver] 1696000000`).
/// Key and value names are compared ignoring ASCII case, as Windows does.
/// Escapes in names and strings (`\\`, `\"`, `\n`, `\r`, `\t`, `\0`,
/// `\x<hex>`) are resolved. The default value (`@=`) is never matched, and
/// a value continued on the next line (long `hex:` data) is reported as
/// [`FileValue::Other`].
#[must_use]
pub fn file_value(text: &str, key: &str, name: &str) -> Option<FileValue> {
    let mut in_key = false;
    for line in text.lines() {
        let line = line.trim_start();
        if let Some(header) = line.strip_prefix('[') {
            in_key = parse_quoted_until(header, ']')
                .is_some_and(|(path, _)| path.eq_ignore_ascii_case(key));
            continue;
        }
        if !in_key {
            continue;
        }
        let Some(rest) = line.strip_prefix('"') else {
            continue;
        };
        let Some((value_name, rest)) = parse_quoted_until(rest, '"') else {
            continue;
        };
        if !value_name.eq_ignore_ascii_case(name) {
            continue;
        }
        let Some(data) = rest.strip_prefix('=') else {
            continue;
        };
        return Some(parse_file_data(data.trim_end()));
    }
    None
}

/// The data after `"name"=` in a registry file.
fn parse_file_data(data: &str) -> FileValue {
    let string = data
        .strip_prefix("str(2):")
        .or_else(|| data.strip_prefix("str(1):"))
        .unwrap_or(data);
    if let Some(quoted) = string.strip_prefix('"') {
        return match parse_quoted_until(quoted, '"') {
            Some((text, rest)) if rest.trim().is_empty() => FileValue::Sz(text),
            _ => FileValue::Other,
        };
    }
    match data.strip_prefix("dword:") {
        Some(hex) => u32::from_str_radix(hex.trim(), 16).map_or(FileValue::Other, FileValue::Dword),
        None => FileValue::Other,
    }
}

/// Read an escaped name or string up to the unescaped `end` character:
/// the unescaped text and what follows `end`. `None` when `end` never comes.
fn parse_quoted_until(text: &str, end: char) -> Option<(String, &str)> {
    let mut out = String::new();
    let mut chars = text.char_indices();
    while let Some((index, c)) = chars.next() {
        if c == end {
            return Some((out, &text[index + c.len_utf8()..]));
        }
        if c != '\\' {
            out.push(c);
            continue;
        }
        let (_, escaped) = chars.next()?;
        match escaped {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            '0' => out.push('\0'),
            'x' => {
                // Up to four hex digits, as Wine writes non-ASCII characters.
                let digits: String = chars
                    .clone()
                    .map(|(_, c)| c)
                    .take(4)
                    .take_while(char::is_ascii_hexdigit)
                    .collect();
                for _ in 0..digits.len() {
                    chars.next();
                }
                let code = u32::from_str_radix(&digits, 16).ok()?;
                out.push(char::from_u32(code).unwrap_or(char::REPLACEMENT_CHARACTER));
            }
            other => out.push(other),
        }
    }
    None
}

/// Append `text` as a double-quoted, escaped `.reg` string.
fn push_quoted(out: &mut String, text: &str) {
    out.push('"');
    for c in text.chars() {
        match c {
            '\\' => out.push_str(r"\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str(r"\n"),
            '\r' => out.push_str(r"\r"),
            _ => out.push(c),
        }
    }
    out.push('"');
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    fn key(path: &str, values: &[(&str, RegValue)]) -> RegKey {
        RegKey {
            path: path.to_owned(),
            values: values
                .iter()
                .map(|(name, value)| ((*name).to_owned(), value.clone()))
                .collect(),
        }
    }

    /// Inverse of `push_quoted` for one quoted string: what Wine's regedit
    /// does when it reads one back.
    fn unquote(quoted: &str) -> String {
        let inner = quoted
            .strip_prefix('"')
            .and_then(|q| q.strip_suffix('"'))
            .unwrap();
        let mut out = String::new();
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next().unwrap() {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    other => out.push(other),
                }
            } else {
                assert_ne!(c, '"', "unescaped quote in {quoted}");
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn empty_document_is_header_and_blank_line() {
        assert_eq!(render(&[]), "REGEDIT4\n\n");
    }

    #[test]
    fn renders_mac_driver_defaults() {
        let keys = [
            key(
                MAC_DRIVER_KEY,
                &[
                    ("RetinaMode", RegValue::Sz("n".to_owned())),
                    ("EnableAppNap", RegValue::Sz("n".to_owned())),
                ],
            ),
            key(
                r"HKEY_CURRENT_USER\Software\Wine",
                &[("Version", RegValue::Sz("win10".to_owned()))],
            ),
        ];
        assert_eq!(
            render(&keys),
            "REGEDIT4\n\
             \n\
             [HKEY_CURRENT_USER\\Software\\Wine\\Mac Driver]\n\
             \"RetinaMode\"=\"n\"\n\
             \"EnableAppNap\"=\"n\"\n\
             \n\
             [HKEY_CURRENT_USER\\Software\\Wine]\n\
             \"Version\"=\"win10\"\n"
        );
    }

    #[test]
    fn renders_dwords_as_eight_hex_digits() {
        let keys = [key(
            r"HKEY_CURRENT_USER\Software\Wine\AppDefaults\game.exe",
            &[
                ("Zero", RegValue::Dword(0)),
                ("One", RegValue::Dword(1)),
                ("Mixed", RegValue::Dword(0xDEAD_BEEF)),
                ("Max", RegValue::Dword(u32::MAX)),
            ],
        )];
        let text = render(&keys);
        assert!(text.contains("\"Zero\"=dword:00000000\n"));
        assert!(text.contains("\"One\"=dword:00000001\n"));
        assert!(text.contains("\"Mixed\"=dword:deadbeef\n"));
        assert!(text.contains("\"Max\"=dword:ffffffff\n"));
    }

    #[test]
    fn renders_deletions() {
        let keys = [key(MAC_DRIVER_KEY, &[("RetinaMode", RegValue::Delete)])];
        assert!(
            render(&keys)
                .ends_with("[HKEY_CURRENT_USER\\Software\\Wine\\Mac Driver]\n\"RetinaMode\"=-\n")
        );
    }

    #[test]
    fn escapes_names_and_strings_but_not_paths() {
        let keys = [key(
            r"HKEY_CURRENT_USER\Software\Wine\Fonts\Replacements",
            &[(
                r#"Odd "name" \ here"#,
                RegValue::Sz(r#"C:\a "b""#.to_owned()),
            )],
        )];
        assert_eq!(
            render(&keys),
            "REGEDIT4\n\n[HKEY_CURRENT_USER\\Software\\Wine\\Fonts\\Replacements]\n\
             \"Odd \\\"name\\\" \\\\ here\"=\"C:\\\\a \\\"b\\\"\"\n"
        );
    }

    #[test]
    fn line_breaks_cannot_inject_keys() {
        let keys = [key(
            MAC_DRIVER_KEY,
            &[(
                "x",
                RegValue::Sz("a\r\n[HKEY_LOCAL_MACHINE\\Evil]".to_owned()),
            )],
        )];
        let text = render(&keys);
        assert_eq!(text.lines().filter(|line| line.starts_with('[')).count(), 1);
        assert!(text.contains(r#""x"="a\r\n[HKEY_LOCAL_MACHINE\\Evil]""#));
    }

    #[test]
    fn key_without_values_is_just_its_header() {
        let keys = [
            key("HKEY_CURRENT_USER\\A", &[]),
            key("HKEY_CURRENT_USER\\B", &[]),
        ];
        assert_eq!(
            render(&keys),
            "REGEDIT4\n\n[HKEY_CURRENT_USER\\A]\n\n[HKEY_CURRENT_USER\\B]\n"
        );
    }

    /// The shape of a CrossOver bottle's `user.reg` with High Resolution
    /// Mode on.
    const USER_REG: &str = "WINE REGISTRY Version 2\n\
;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\
\n\
#arch=win64\n\
\n\
[Control Panel\\\\Desktop] 1759800000\n\
#time=1dc37f8a1b2c3d4\n\
\"ActiveWndTrackTimeout\"=dword:00000000\n\
\"LogPixels\"=dword:000000c0\n\
\"Wallpaper\"=\"\"\n\
\n\
[Software\\\\Wine\\\\Fonts] 1759800000\n\
\"LogPixels\"=dword:00000060\n\
\n\
[Software\\\\Wine\\\\Mac Driver] 1759800000\n\
#time=1dc37f8a1b2c3d4\n\
\"RetinaMode\"=\"y\"\n\
\"Odd \\\"name\\\"\"=str(2):\"%USERPROFILE%\\\\x\"\n\
\"Binary\"=hex:01,02,\\\n\
  03,04\n\
@=\"default\"\n\
\n\
[Software\\\\Valve\\\\Steam\\\\ActiveProcess] 1759800000\n\
\"pid\"=dword:00000274\n";

    #[test]
    fn reads_values_from_wine_registry_files() {
        let read = |key: &str, name: &str| file_value(USER_REG, key, name);
        assert_eq!(
            read(r"Control Panel\Desktop", "LogPixels"),
            Some(FileValue::Dword(192))
        );
        assert_eq!(
            read(r"Software\Wine\Fonts", "LogPixels"),
            Some(FileValue::Dword(96)),
            "the same name under another key"
        );
        assert_eq!(
            read(r"software\wine\MAC DRIVER", "retinamode"),
            Some(FileValue::Sz("y".to_owned())),
            "keys and names ignore case"
        );
        assert_eq!(
            read(r"Software\Wine\Mac Driver", r#"Odd "name""#),
            Some(FileValue::Sz(r"%USERPROFILE%\x".to_owned()))
        );
        assert_eq!(
            read(r"Software\Wine\Mac Driver", "Binary"),
            Some(FileValue::Other)
        );
        assert_eq!(
            read(r"Control Panel\Desktop", "Wallpaper"),
            Some(FileValue::Sz(String::new()))
        );
        assert_eq!(
            read(r"Software\Valve\Steam\ActiveProcess", "pid"),
            Some(FileValue::Dword(0x274))
        );
    }

    #[test]
    fn missing_keys_and_values_are_none() {
        for (key, name) in [
            (r"Control Panel\Desktop", "RetinaMode"),
            (r"Software\Wine\Mac Driver", "LogPixels"),
            (r"Software\Wine", "RetinaMode"),
            (r"Wine\Mac Driver", "RetinaMode"),
            (r"Software\Wine\Mac Driver", "@"),
            (r"Software\Wine\Mac Driver", ""),
        ] {
            assert_eq!(file_value(USER_REG, key, name), None, "{key} {name}");
        }
        assert_eq!(file_value("", r"Control Panel\Desktop", "LogPixels"), None);
        assert_eq!(
            file_value(
                "[Control Panel\\\\Desktop]\n\"LogPixels\"\n\"LogPixels\"=dword:zz\n",
                r"Control Panel\Desktop",
                "LogPixels"
            ),
            Some(FileValue::Other),
            "a line without '=' is skipped; bad data is Other"
        );
    }

    #[test]
    fn resolves_wines_escapes() {
        let text = "[A\\\\B] 1\n\"Name\"=\"caf\\xe9 \\x4e2d\\x6587 tab\\tend\\\\\"\n";
        assert_eq!(
            file_value(text, r"A\B", "name"),
            Some(FileValue::Sz("café 中文 tab\tend\\".to_owned()))
        );
        assert_eq!(
            file_value("[K] 1\n\"Name\"=\"unterminated\n", "K", "Name"),
            Some(FileValue::Other)
        );
    }

    proptest! {
        #[test]
        fn strings_survive_escaping(name in any::<String>(), value in any::<String>()) {
            let text = render(&[key("HKEY_CURRENT_USER\\T", &[(&name, RegValue::Sz(value.clone()))])]);
            let line = text.lines().nth(3).unwrap();
            prop_assert_eq!(text.lines().count(), 4);
            // Split at the `"="` between the two quoted strings: the first
            // unescaped quote after the name's opening quote ends the name.
            let mut escaped = false;
            let name_end = line
                .char_indices()
                .skip(1)
                .find(|&(_, c)| {
                    let end = c == '"' && !escaped;
                    escaped = c == '\\' && !escaped;
                    end
                })
                .map(|(index, _)| index)
                .unwrap();
            let (quoted_name, rest) = line.split_at(name_end + 1);
            prop_assert_eq!(unquote(quoted_name), name);
            prop_assert_eq!(unquote(rest.strip_prefix('=').unwrap()), value);
        }

        #[test]
        fn layout_has_one_line_per_value_and_blank_separators(
            keys in proptest::collection::vec(
                ("[A-Za-z\\\\ ]{1,12}", proptest::collection::vec(("[a-z]{1,4}", any::<u32>()), 0..4)),
                0..5,
            )
        ) {
            let keys: Vec<RegKey> = keys
                .into_iter()
                .map(|(path, values)| RegKey {
                    path,
                    values: values.into_iter().map(|(n, v)| (n, RegValue::Dword(v))).collect(),
                })
                .collect();
            let text = render(&keys);
            prop_assert!(text.starts_with("REGEDIT4\n\n"));
            prop_assert!(text.ends_with('\n'));
            let values: usize = keys.iter().map(|k| k.values.len()).sum();
            let separators = keys.len().saturating_sub(1);
            prop_assert_eq!(text.lines().count(), 2 + keys.len() + values + separators);
        }
    }
}
