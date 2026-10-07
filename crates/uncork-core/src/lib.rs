//! Uncork's engine: everything except the command line.
//!
//! Uncork runs Windows games on Apple Silicon Macs by orchestrating a Wine
//! runtime (x86-64, under Rosetta 2), Metal translation layers (DXMT, DXVK on
//! MoltenVK, Apple's D3DMetal) and the Windows Steam client. It does not
//! reimplement any of them. `docs/ARCHITECTURE.md` explains the design;
//! this crate's modules map onto it:
//!
//! | Module | Responsibility |
//! |---|---|
//! | [`paths`] | Where data lives (`$UNCORK_HOME`) |
//! | [`config`] | Global settings, TOML helpers |
//! | [`catalog`], [`download`], [`component`] | Pinned, verified Wine/DXMT/DXVK installs; D3DMetal import |
//! | [`wine`] | Wine runtime binaries and command builders |
//! | [`bottle`], [`registry`] | Prefixes and their settings |
//! | [`graphics`] | Backend choice and per-process activation |
//! | [`profile`], [`ini`] | Per-game settings |
//! | [`launch`], [`process`] | Launch plans and running them |
//! | [`steam`] | Steam client install, startup, game launch |
//! | [`gamemode`] | macOS Game Mode wrapper bundles (experimental) |
//! | [`host`] | Host facts and `doctor` checks |

pub mod bottle;
pub mod catalog;
pub mod component;
pub mod config;
pub mod download;
mod error;
pub mod gamemode;
pub mod graphics;
pub mod host;
pub mod ini;
pub mod launch;
pub mod paths;
pub mod process;
pub mod profile;
pub mod registry;
pub mod steam;
pub mod wine;

pub use error::{Error, Result};

/// Seconds since the Unix epoch (0 if the clock is before 1970).
#[must_use]
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
