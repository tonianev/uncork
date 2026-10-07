//! Global settings in `config.toml`.
//!
//! ```toml
//! schema = 1
//! default_bottle = "steam"     # used when --bottle is omitted
//! ```

use serde::{Deserialize, Serialize};

use crate::paths::Layout;

/// Name of the bottle Uncork creates for Steam and uses by default.
pub const DEFAULT_BOTTLE: &str = "steam";

/// Parsed `config.toml`. Unknown keys are rejected so typos surface.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Schema version, currently 1.
    pub schema: u32,
    /// Bottle used when a command does not name one.
    #[serde(default = "default_bottle")]
    pub default_bottle: String,
}

fn default_bottle() -> String {
    DEFAULT_BOTTLE.to_owned()
}

impl Default for Config {
    fn default() -> Config {
        Config {
            schema: 1,
            default_bottle: default_bottle(),
        }
    }
}

impl Config {
    /// Read `config.toml`; a missing file yields [`Config::default`].
    ///
    /// # Errors
    /// [`crate::Error::Config`] for invalid TOML, [`crate::Error::Io`] otherwise.
    pub fn load(layout: &Layout) -> crate::Result<Config> {
        let _ = layout;
        todo!()
    }

    /// Write atomically (temp file in the same directory, then rename).
    ///
    /// # Errors
    /// [`crate::Error::Io`].
    pub fn save(&self, layout: &Layout) -> crate::Result<()> {
        let _ = layout;
        todo!()
    }
}

/// Serialize `value` to TOML and write it atomically to `path` (temp file
/// `.<name>.tmp` in the same directory, fsync, rename). Shared by every
/// module that writes TOML.
///
/// # Errors
/// [`crate::Error::Io`].
pub fn write_toml_atomic<T: Serialize>(path: &std::path::Path, value: &T) -> crate::Result<()> {
    let _ = (path, value);
    todo!()
}

/// Read and deserialize a TOML file. `what` names the file kind in errors.
///
/// # Errors
/// [`crate::Error::Io`] or [`crate::Error::Config`].
pub fn read_toml<T: serde::de::DeserializeOwned>(
    path: &std::path::Path,
    what: &'static str,
) -> crate::Result<T> {
    let _ = (path, what);
    todo!()
}
