//! Embed `profiles/*.toml` from the repository root into the binary.
//!
//! Generates `$OUT_DIR/builtin_profiles.rs`, an array expression of
//! `(file name, include_str!(absolute path))` pairs sorted by file name,
//! which `src/profile.rs` includes as `BUILTIN_FILES`.

use std::fmt::Write as _;
use std::path::PathBuf;

fn main() {
    let manifest_dir = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let profiles = manifest_dir.join("../../profiles");
    println!("cargo::rerun-if-changed={}", profiles.display());

    let mut files: Vec<PathBuf> = std::fs::read_dir(&profiles)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", profiles.display()))
        .map(|entry| entry.expect("readable profiles entry").path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "toml"))
        .collect();
    files.sort();

    let mut out = String::from("&[\n");
    for path in &files {
        let canonical = path.canonicalize().expect("profile path canonicalizes");
        let name = path.file_name().expect("file name").to_string_lossy();
        println!("cargo::rerun-if-changed={}", canonical.display());
        writeln!(out, "    ({name:?}, include_str!({:?})),", canonical.display().to_string())
            .expect("write to String");
    }
    out.push(']');

    let dest = PathBuf::from(std::env::var_os("OUT_DIR").expect("set by cargo")).join("builtin_profiles.rs");
    std::fs::write(&dest, out).unwrap_or_else(|e| panic!("cannot write {}: {e}", dest.display()));
}
