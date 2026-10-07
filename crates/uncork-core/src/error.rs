//! The core error type.

use std::path::PathBuf;

/// Result alias for this crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in `uncork-core`. Messages are written for
/// the person at the terminal: they name the file, the command or the
/// component involved and, where there is one, the fix.
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
}
