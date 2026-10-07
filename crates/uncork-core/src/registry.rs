//! Windows registry edits applied to a bottle with `wine regedit /S`.
//!
//! Uncork renders edits to a `REGEDIT4` file inside the bottle's
//! `drive_c/windows/temp/` and imports it with Wine's `regedit`, which works
//! headless and is the same mechanism winetricks uses.

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
