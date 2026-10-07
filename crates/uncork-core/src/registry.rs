//! Windows registry edits applied to a bottle with `wine regedit /S`.
//!
//! Uncork renders edits to a `REGEDIT4` file inside the bottle's
//! `drive_c/windows/temp/` and imports it with Wine's `regedit`, which works
//! headless and is the same mechanism winetricks uses.

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
#[must_use]
pub fn render(keys: &[RegKey]) -> String {
    let _ = keys;
    todo!()
}

/// `HKEY_CURRENT_USER\Software\Wine\Mac Driver`.
pub const MAC_DRIVER_KEY: &str = r"HKEY_CURRENT_USER\Software\Wine\Mac Driver";
