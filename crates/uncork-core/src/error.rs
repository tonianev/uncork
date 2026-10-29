//! The core error type, with a stable kind and an exit code per variant.
//!
//! Every [`Error`] has a kebab-case [`Error::kind`] and an
//! [`Error::exit_code`]. The `uncork` binary exits with the code of the
//! first `Error` in a failure's chain and, with `--json`, prints the kind,
//! so a script or an agent can branch on the code alone and read the kind
//! for the detail. Codes group errors by what the caller does next, not by
//! where they came from:
//!
//! | Code | What to do | Kind (variant) |
//! |---|---|---|
//! | 1 | Read the message; nothing more specific is known | `io` ([`Error::Io`]) |
//! | 3 | Fix the argument: name something that exists, or does not exist yet | `not-found` ([`Error::NotFound`]), `already-exists` ([`Error::AlreadyExists`]) |
//! | 4 | Fix a file or a name: a config, profile, archive, executable or Steam library file is invalid | `invalid-config` ([`Error::Config`]), `invalid-name` ([`Error::InvalidName`]), `archive` ([`Error::Archive`]), `invalid-executable` ([`Error::Pe`]), `steam-library` ([`Error::Steam`]) |
//! | 5 | Check the network, or the catalog pin | `download` ([`Error::Download`]), `checksum` ([`Error::Checksum`]) |
//! | 6 | A program failed; read its log | `command-failed` ([`Error::Command`]) |
//! | 7 | This Mac or the backend cannot do it | `host` ([`Error::NoHome`], [`Error::Host`]), `unsupported` ([`Error::Unsupported`]) |
//! | 8 | Reinstall a component | `broken-component` ([`Error::BrokenComponent`]) |
//!
//! The binary also exits with 1 for a failure that has no `Error` of this
//! crate in its chain (kind `other`), and with 2, clap's code, for a usage
//! error; 0 and 2 are never returned here.

use std::path::PathBuf;

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in `uncork-core`. Messages are written for
/// the person at the terminal: they name the file, the command or the
/// component involved and, where there is one, the fix. Each variant has a
/// stable [`Error::kind`] and an [`Error::exit_code`] for scripts; the
/// table of both is at the top of `error.rs` and in `docs/CLI.md`.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Neither `UNCORK_HOME` nor `HOME` is set.
    #[error("cannot find a home directory: set HOME or UNCORK_HOME")]
    NoHome,

    /// A file-system operation failed.
    #[error("{action} {path}: {source}")]
    Io {
        /// What was being done, e.g. "cannot create directory".
        action: &'static str,
        /// The path involved.
        path: PathBuf,
        /// Underlying error.
        source: std::io::Error,
    },

    /// A TOML file could not be parsed or does not match its schema.
    #[error("invalid {what} {path}: {message}")]
    Config {
        /// What kind of file, e.g. "bottle config", "profile", "catalog".
        what: &'static str,
        /// The file.
        path: PathBuf,
        /// Parser message.
        message: String,
    },

    /// A name given by the user is not acceptable.
    #[error("invalid {what} name {name:?}: {reason}")]
    InvalidName {
        /// "bottle", "profile", ...
        what: &'static str,
        /// The rejected name.
        name: String,
        /// Why.
        reason: &'static str,
    },

    /// Something the user referred to does not exist.
    #[error("{what} {name:?} not found{hint}")]
    NotFound {
        /// "bottle", "component", "profile", "game", ...
        what: &'static str,
        /// What was looked up.
        name: String,
        /// Extra text starting with "; ", or empty.
        hint: String,
    },

    /// Something the user wants to create already exists.
    #[error("{what} {name:?} already exists at {path}")]
    AlreadyExists {
        /// "bottle", "component", ...
        what: &'static str,
        /// Its name.
        name: String,
        /// Where.
        path: PathBuf,
    },

    /// An HTTP download failed.
    #[error("download of {url} failed: {message}")]
    Download {
        /// The URL.
        url: String,
        /// What happened.
        message: String,
    },

    /// A download's SHA-256 does not match the pinned value.
    #[error("checksum mismatch for {url}: expected {expected}, got {actual}")]
    Checksum {
        /// The URL.
        url: String,
        /// Pinned hash.
        expected: String,
        /// Computed hash.
        actual: String,
    },

    /// An archive is corrupt or contains an unsafe entry (absolute path,
    /// `..`, a symlink escaping the destination).
    #[error("cannot extract {path}: {message}")]
    Archive {
        /// The archive.
        path: PathBuf,
        /// What was wrong.
        message: String,
    },

    /// An installed component does not have the files Uncork needs.
    #[error("{kind} component at {path} is incomplete: {message}")]
    BrokenComponent {
        /// Component kind.
        kind: crate::component::ComponentKind,
        /// Its directory.
        path: PathBuf,
        /// What is missing.
        message: String,
    },

    /// A program could not be started or exited unsuccessfully.
    #[error("{program} {status}{log_hint}")]
    Command {
        /// The program (file name only).
        program: String,
        /// `could not start: <io error>`, `exited with status N` or
        /// `was killed by signal N`.
        status: String,
        /// `" (log: <path>)"` when output was captured, else empty.
        log_hint: String,
    },

    /// The requested graphics backend cannot run this game.
    #[error("{0}")]
    Unsupported(String),

    /// The host cannot run Uncork (not macOS, not Apple Silicon, no Rosetta).
    #[error("{0}")]
    Host(String),

    /// PE inspection failed.
    #[error(transparent)]
    Pe(#[from] uncork_pe::PeError),

    /// Steam library data could not be read.
    #[error(transparent)]
    Steam(#[from] uncork_steam::library::LibraryError),
}

impl Error {
    /// Shorthand for [`Error::Io`].
    pub fn io(action: &'static str, path: impl Into<PathBuf>, source: std::io::Error) -> Error {
        Error::Io {
            action,
            path: path.into(),
            source,
        }
    }

    /// The error's kind: a stable kebab-case slug per variant (the table at
    /// the top of `error.rs`, and in `docs/CLI.md`), printed by
    /// `uncork --json` as `error.kind`. Renaming one breaks callers that
    /// branch on it.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Error::NoHome | Error::Host(_) => "host",
            Error::Io { .. } => "io",
            Error::Config { .. } => "invalid-config",
            Error::InvalidName { .. } => "invalid-name",
            Error::NotFound { .. } => "not-found",
            Error::AlreadyExists { .. } => "already-exists",
            Error::Download { .. } => "download",
            Error::Checksum { .. } => "checksum",
            Error::Archive { .. } => "archive",
            Error::BrokenComponent { .. } => "broken-component",
            Error::Command { .. } => "command-failed",
            Error::Unsupported(_) => "unsupported",
            Error::Pe(_) => "invalid-executable",
            Error::Steam(_) => "steam-library",
        }
    }

    /// The process exit code for this error, grouped by what the caller
    /// does next (the table at the top of `error.rs`, and in
    /// `docs/CLI.md`): never 0, and never 2, which clap uses for a usage
    /// error.
    #[must_use]
    pub fn exit_code(&self) -> u8 {
        match self {
            Error::Io { .. } => 1,
            Error::NotFound { .. } | Error::AlreadyExists { .. } => 3,
            Error::Config { .. }
            | Error::InvalidName { .. }
            | Error::Archive { .. }
            | Error::Pe(_)
            | Error::Steam(_) => 4,
            Error::Download { .. } | Error::Checksum { .. } => 5,
            Error::Command { .. } => 6,
            Error::NoHome | Error::Host(_) | Error::Unsupported(_) => 7,
            Error::BrokenComponent { .. } => 8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::component::ComponentKind;

    /// One error of every variant, with the kind and code the table in the
    /// module docs promises for it.
    fn every_variant() -> Vec<(Error, &'static str, u8)> {
        vec![
            (Error::NoHome, "host", 7),
            (
                Error::io("cannot read", "/x", std::io::Error::other("boom")),
                "io",
                1,
            ),
            (
                Error::Config {
                    what: "profile",
                    path: "/x.toml".into(),
                    message: "bad".into(),
                },
                "invalid-config",
                4,
            ),
            (
                Error::InvalidName {
                    what: "bottle",
                    name: "../x".into(),
                    reason: "slash",
                },
                "invalid-name",
                4,
            ),
            (
                Error::NotFound {
                    what: "bottle",
                    name: "steam".into(),
                    hint: String::new(),
                },
                "not-found",
                3,
            ),
            (
                Error::AlreadyExists {
                    what: "bottle",
                    name: "steam".into(),
                    path: "/b".into(),
                },
                "already-exists",
                3,
            ),
            (
                Error::Download {
                    url: "https://x".into(),
                    message: "timeout".into(),
                },
                "download",
                5,
            ),
            (
                Error::Checksum {
                    url: "https://x".into(),
                    expected: "aa".into(),
                    actual: "bb".into(),
                },
                "checksum",
                5,
            ),
            (
                Error::Archive {
                    path: "/x.tar".into(),
                    message: "escapes".into(),
                },
                "archive",
                4,
            ),
            (
                Error::BrokenComponent {
                    kind: ComponentKind::Wine,
                    path: "/c".into(),
                    message: "no bin/wine".into(),
                },
                "broken-component",
                8,
            ),
            (
                Error::Command {
                    program: "wine".into(),
                    status: "exited with status 1".into(),
                    log_hint: String::new(),
                },
                "command-failed",
                6,
            ),
            (Error::Unsupported("no".into()), "unsupported", 7),
            (Error::Host("not macOS".into()), "host", 7),
            (
                Error::Pe(uncork_pe::PeError::NotPe("x".into())),
                "invalid-executable",
                4,
            ),
            (
                Error::Steam(uncork_steam::library::LibraryError::Schema {
                    path: "/x.acf".into(),
                    message: "no appid".into(),
                }),
                "steam-library",
                4,
            ),
        ]
    }

    #[test]
    fn every_variant_has_its_kind_and_exit_code() {
        for (error, kind, code) in every_variant() {
            assert_eq!(error.kind(), kind, "{error}");
            assert_eq!(error.exit_code(), code, "{error}");
        }
    }

    #[test]
    fn exit_codes_leave_0_and_clap_s_2_alone() {
        for (error, _, _) in every_variant() {
            let code = error.exit_code();
            assert!(code == 1 || (3..=8).contains(&code), "{error}: {code}");
        }
    }

    #[test]
    fn kinds_are_kebab_case_slugs() {
        for (error, _, _) in every_variant() {
            let kind = error.kind();
            assert!(
                kind.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'),
                "{kind}"
            );
            assert!(!kind.starts_with('-') && !kind.ends_with('-'), "{kind}");
        }
    }
}
