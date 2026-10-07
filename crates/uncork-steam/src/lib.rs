//! The Windows Steam client, seen from macOS.
//!
//! Uncork installs the Windows Steam client into a Wine prefix (a "bottle")
//! and launches games through it, because Steamworks games need the client
//! running. This crate has no Wine dependency: it parses the client's text
//! `KeyValues` files ([`vdf`]), models its libraries ([`library`]), maps
//! Windows paths into the prefix ([`paths`]) and builds the argument lists
//! for installing and launching ([`client`]). Running those commands is
//! `uncork-core`'s job.

pub mod client;
pub mod library;
pub mod paths;
pub mod vdf;

pub use library::{AppManifest, InstalledApp, LibraryFolder, SteamInstall};
