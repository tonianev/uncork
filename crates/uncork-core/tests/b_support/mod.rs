//! Archive building for the extraction and installation tests.

#![allow(dead_code, reason = "each test crate uses a different subset")]

use std::fs;
use std::io::Write;
use std::path::Path;

use tar::{EntryType, Header};

/// Builds tar archives in memory, including ones no well-behaved tool would
/// write (absolute paths, `..`), by filling header fields directly.
pub struct TarBuilder {
    builder: tar::Builder<Vec<u8>>,
}

impl Default for TarBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl TarBuilder {
    pub fn new() -> TarBuilder {
        TarBuilder {
            builder: tar::Builder::new(Vec::new()),
        }
    }

    pub fn dir(self, path: &str) -> TarBuilder {
        self.dir_mode(path, 0o755)
    }

    pub fn dir_mode(self, path: &str, mode: u32) -> TarBuilder {
        self.raw(path.as_bytes(), EntryType::Directory, mode, b"", None)
    }

    pub fn file(self, path: &str, contents: &[u8]) -> TarBuilder {
        self.file_mode(path, contents, 0o644)
    }

    pub fn file_mode(mut self, path: &str, contents: &[u8], mode: u32) -> TarBuilder {
        // `append_data` writes a GNU long-name entry for long paths.
        let mut header = Header::new_gnu();
        header.set_entry_type(EntryType::Regular);
        header.set_size(contents.len() as u64);
        header.set_mode(mode);
        self.builder
            .append_data(&mut header, path, contents)
            .unwrap();
        self
    }

    pub fn symlink(self, path: &str, target: &str) -> TarBuilder {
        self.raw(
            path.as_bytes(),
            EntryType::Symlink,
            0o777,
            b"",
            Some(target),
        )
    }

    pub fn hard_link(self, path: &str, target: &str) -> TarBuilder {
        self.raw(path.as_bytes(), EntryType::Link, 0o644, b"", Some(target))
    }

    /// A pax global header, as `git archive` writes first.
    pub fn pax_global_header(self) -> TarBuilder {
        let record = b"17 comment=hello\n";
        self.raw(
            b"pax_global_header",
            EntryType::XGlobalHeader,
            0o644,
            record,
            None,
        )
    }

    /// An entry whose path bytes are written verbatim (at most 100 bytes).
    pub fn raw(
        mut self,
        path: &[u8],
        entry_type: EntryType,
        mode: u32,
        contents: &[u8],
        link: Option<&str>,
    ) -> TarBuilder {
        let mut header = Header::new_gnu();
        header.as_old_mut().name[..path.len()].copy_from_slice(path);
        header.set_entry_type(entry_type);
        header.set_size(contents.len() as u64);
        header.set_mode(mode);
        if let Some(link) = link {
            header.set_link_name(link).unwrap();
        }
        header.set_cksum();
        self.builder.append(&header, contents).unwrap();
        self
    }

    pub fn into_tar(self) -> Vec<u8> {
        self.builder.into_inner().unwrap()
    }

    pub fn write_tar_gz(self, path: &Path) {
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder.write_all(&self.into_tar()).unwrap();
        fs::write(path, encoder.finish().unwrap()).unwrap();
    }

    pub fn write_tar_xz(self, path: &Path) {
        let mut encoder = liblzma::write::XzEncoder::new(Vec::new(), 1);
        encoder.write_all(&self.into_tar()).unwrap();
        fs::write(path, encoder.finish().unwrap()).unwrap();
    }
}

/// Every path below `root`, relative and sorted, directories with a
/// trailing `/` and symlinks as `link -> target`.
pub fn tree(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().display().to_string();
        let meta = fs::symlink_metadata(&path).unwrap();
        if meta.file_type().is_symlink() {
            out.push(format!(
                "{relative} -> {}",
                fs::read_link(&path).unwrap().display()
            ));
        } else if meta.is_dir() {
            out.push(format!("{relative}/"));
            walk(root, &path, out);
        } else {
            out.push(relative);
        }
    }
}
