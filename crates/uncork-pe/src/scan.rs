//! [`crate::scan_game`]: inspect a game executable and the DLLs next to it.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};

use crate::{
    ApiEvidence, Bitness, EvidenceKind, GameScan, GraphicsApi, IGNORED_DLL_PREFIXES, MAX_DLL_BYTES,
    MAX_SIBLING_DLLS, PeError, PeInfo, api_for_dll, inspect_bytes, read_file, string_dll_refs,
};

/// Steamworks API DLLs (lowercase). Either one next to the executable or in
/// an import table means the game talks to a running Steam client.
const STEAM_API_DLLS: &[&str] = &["steam_api.dll", "steam_api64.dll"];

/// Lowercase file-name prefixes of kernel anti-cheat services and installers.
const ANTI_CHEAT_PREFIXES: &[&str] = &["easyanticheat", "beservice", "battleye", "vgk"];

pub(crate) fn scan_game(exe: &Path) -> Result<GameScan, PeError> {
    let bytes = read_file(exe)?;
    let info = inspect_bytes(&bytes).map_err(|err| err.in_file(exe))?;
    let Some(bitness) = info.bitness else {
        return Err(PeError::NotPe(format!(
            "{}: machine {} is not x86 or x86-64",
            exe.display(),
            info.machine.describe()
        )));
    };

    let exe_name = file_name(exe);
    let mut findings = Findings::default();
    findings.add_module(&exe_name, &info, &bytes);
    // Siblings are read one at a time; do not hold a large executable meanwhile.
    drop(bytes);

    let dir = containing_dir(exe);
    match list_dir(dir) {
        Ok(entries) if is_system_dir(&entries) => findings.skipped.push((
            dir.display().to_string(),
            "a Windows system directory: its DLLs are not the program's own and are not scanned"
                .to_owned(),
        )),
        Ok(entries) => findings.add_siblings(&entries, &exe_name, bitness),
        Err(err) => findings.skipped.push((
            dir.display().to_string(),
            format!("cannot list the executable's directory: {err}"),
        )),
    }

    Ok(findings.into_scan(exe, info))
}

/// What the scan has learned so far.
#[derive(Debug, Default)]
struct Findings {
    evidence: BTreeSet<ApiEvidence>,
    skipped: Vec<(String, String)>,
    uses_steamworks: bool,
    anti_cheat: BTreeSet<String>,
    non_nx_modules: Vec<String>,
}

impl Findings {
    /// Record what one inspected module (the executable or a sibling DLL)
    /// says about the game.
    fn add_module(&mut self, file: &str, info: &PeInfo, bytes: &[u8]) {
        for import in &info.imports {
            if let Some(api) = api_for_dll(import) {
                self.add_evidence(file, import.clone(), api, EvidenceKind::Import);
            }
            self.uses_steamworks |= is_steam_api(import);
        }
        for (dll, api) in string_dll_refs(bytes) {
            // The import table's own name strings are found by the string
            // scan too; they are not a separate `LoadLibrary` target.
            if info.imports.binary_search(&dll).is_err() {
                self.add_evidence(file, dll, api, EvidenceKind::String);
            }
        }
        if !info.nx_compat {
            self.non_nx_modules.push(file.to_owned());
        }
    }

    fn add_evidence(&mut self, file: &str, import: String, api: GraphicsApi, kind: EvidenceKind) {
        self.evidence.insert(ApiEvidence {
            file: file.to_owned(),
            import,
            api,
            kind,
        });
    }

    /// Record the directory entries next to the executable and inspect the
    /// DLLs among them. `entries` must be sorted by name.
    fn add_siblings(&mut self, entries: &[DirEntry], exe_name: &str, bitness: Bitness) {
        for entry in entries {
            let lowercase = entry.name.to_ascii_lowercase();
            self.uses_steamworks |= is_steam_api(&lowercase);
            if is_anti_cheat(&lowercase) {
                self.anti_cheat.insert(lowercase);
            }
        }

        let dlls: Vec<&DirEntry> = entries
            .iter()
            .filter(|entry| entry.is_candidate_dll(exe_name))
            .collect();
        let (inspected, over_limit) = dlls.split_at(dlls.len().min(MAX_SIBLING_DLLS));
        for dll in inspected {
            match inspect_sibling(dll, bitness) {
                Ok((info, bytes)) => self.add_module(&dll.name, &info, &bytes),
                Err(reason) => self.skipped.push((dll.name.clone(), reason)),
            }
        }
        // One entry, named after the first DLL left out, stands for all of them.
        if let Some(first) = over_limit.first() {
            self.skipped.push((
                first.name.clone(),
                format!(
                    "not inspected: only the first {MAX_SIBLING_DLLS} DLLs by file name are scanned \
                     ({} left out from here on)",
                    over_limit.len()
                ),
            ));
        }
    }

    fn into_scan(self, exe: &Path, info: PeInfo) -> GameScan {
        GameScan {
            exe: exe.to_path_buf(),
            info,
            apis: self.evidence.iter().map(|evidence| evidence.api).collect(),
            evidence: self.evidence.into_iter().collect(),
            skipped: self.skipped,
            uses_steamworks: self.uses_steamworks,
            anti_cheat: self.anti_cheat.into_iter().collect(),
            non_nx_modules: self.non_nx_modules,
        }
    }
}

/// Read and parse a sibling DLL, or say why it was skipped.
fn inspect_sibling(dll: &DirEntry, bitness: Bitness) -> Result<(PeInfo, Vec<u8>), String> {
    let bytes = read_at_most(&dll.path, MAX_DLL_BYTES)
        .map_err(|err| format!("cannot read: {err}"))?
        .ok_or_else(|| format!("larger than {} MiB", MAX_DLL_BYTES / (1024 * 1024)))?;
    let info = inspect_bytes(&bytes).map_err(|err| err.to_string())?;
    if info.bitness != Some(bitness) {
        return Err(format!(
            "{} module, but the executable is {}",
            info.machine.describe(),
            bitness_name(bitness)
        ));
    }
    Ok((info, bytes))
}

/// Read a file unless it is larger than `limit` bytes (`Ok(None)`). The
/// read itself is capped too, in case the file grows after the size check.
fn read_at_most(path: &Path, limit: u64) -> io::Result<Option<Vec<u8>>> {
    let file = File::open(path)?;
    if file.metadata()?.len() > limit {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1)).read_to_end(&mut bytes)?;
    Ok(u64::try_from(bytes.len())
        .is_ok_and(|len| len <= limit)
        .then_some(bytes))
}

fn bitness_name(bitness: Bitness) -> &'static str {
    match bitness {
        Bitness::X86 => "32-bit (x86)",
        Bitness::X64 => "64-bit (x86-64)",
    }
}

/// One entry of the executable's directory.
#[derive(Debug)]
struct DirEntry {
    name: String,
    path: PathBuf,
    /// A regular file, or a symlink to one.
    is_file: bool,
}

impl DirEntry {
    fn is_candidate_dll(&self, exe_name: &str) -> bool {
        let lowercase = self.name.to_ascii_lowercase();
        self.is_file
            && has_dll_extension(&self.name)
            && !self.name.eq_ignore_ascii_case(exe_name)
            && !IGNORED_DLL_PREFIXES
                .iter()
                .any(|prefix| lowercase.starts_with(prefix))
    }
}

/// The entries of `dir`, sorted by file name the way Windows compares them
/// (ignoring ASCII case), with the exact name breaking ties.
fn list_dir(dir: &Path) -> io::Result<Vec<DirEntry>> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        entries.push(DirEntry {
            name: entry.file_name().to_string_lossy().into_owned(),
            is_file: path.is_file(),
            path,
        });
    }
    entries.sort_by_cached_key(|entry| (entry.name.to_ascii_lowercase(), entry.name.clone()));
    Ok(entries)
}

/// The directory an executable path lives in; `.` for a bare file name.
fn containing_dir(exe: &Path) -> &Path {
    match exe.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// `true` for a Windows system directory (`system32`, `syswow64`): it holds
/// `ntdll.dll` and `kernel32.dll`, which no game ships. Scanning it would
/// attribute every graphics DLL Windows has to a program such as `cmd.exe`.
fn is_system_dir(entries: &[DirEntry]) -> bool {
    let has = |wanted: &str| {
        entries
            .iter()
            .any(|entry| entry.is_file && entry.name.eq_ignore_ascii_case(wanted))
    };
    has("ntdll.dll") && has("kernel32.dll")
}

fn has_dll_extension(name: &str) -> bool {
    Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("dll"))
}

fn is_steam_api(lowercase_name: &str) -> bool {
    STEAM_API_DLLS.contains(&lowercase_name)
}

fn is_anti_cheat(lowercase_name: &str) -> bool {
    ANTI_CHEAT_PREFIXES
        .iter()
        .any(|prefix| lowercase_name.starts_with(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dll_extension_matches_case_insensitively() {
        assert!(has_dll_extension("d3dgl.dll"));
        assert!(has_dll_extension("D3DGL.DLL"));
        assert!(has_dll_extension("archive.tar.Dll"));
        assert!(!has_dll_extension("d3dgl.dll.bak"));
        assert!(!has_dll_extension("d3dgl"));
        assert!(!has_dll_extension(".dll"));
        assert!(!has_dll_extension("readme.txt"));
    }

    #[test]
    fn steam_api_names_are_exact() {
        assert!(is_steam_api("steam_api.dll"));
        assert!(is_steam_api("steam_api64.dll"));
        assert!(!is_steam_api("steam_api.dll.bak"));
        assert!(!is_steam_api("steamapiupdater.dll"));
        assert!(!is_steam_api("steam_appid.txt"));
    }

    #[test]
    fn anti_cheat_matches_known_prefixes() {
        for name in [
            "easyanticheat",
            "easyanticheat_eos_setup.exe",
            "beservice_x64.exe",
            "battleye",
            "vgk.sys",
        ] {
            assert!(is_anti_cheat(name), "{name}");
        }
        for name in ["riseofnations.exe", "eac.dll", "battle.net.dll", "be.dll"] {
            assert!(!is_anti_cheat(name), "{name}");
        }
    }

    fn entry(name: &str, is_file: bool) -> DirEntry {
        DirEntry {
            name: name.to_owned(),
            path: PathBuf::from(name),
            is_file,
        }
    }

    #[test]
    fn system_directories_are_recognized_by_ntdll_and_kernel32() {
        let system = [
            entry("NTDLL.dll", true),
            entry("kernel32.dll", true),
            entry("d3d12.dll", true),
        ];
        assert!(is_system_dir(&system));
        let game = [entry("kernel32.dll", true), entry("d3d11.dll", true)];
        assert!(!is_system_dir(&game), "both markers are needed");
        let dirs = [entry("ntdll.dll", false), entry("kernel32.dll", false)];
        assert!(
            !is_system_dir(&dirs),
            "directories named like DLLs do not count"
        );
    }

    #[test]
    fn candidate_dlls_exclude_ignored_prefixes_directories_and_the_exe() {
        assert!(entry("d3dgl.dll", true).is_candidate_dll("game.exe"));
        assert!(entry("Unicows.DLL", true).is_candidate_dll("game.exe"));
        assert!(!entry("D3DX9_43.dll", true).is_candidate_dll("game.exe"));
        assert!(!entry("steam_api.dll", true).is_candidate_dll("game.exe"));
        assert!(!entry("api-ms-win-crt-runtime-l1-1-0.dll", true).is_candidate_dll("game.exe"));
        assert!(!entry("plugins.dll", false).is_candidate_dll("game.exe"));
        assert!(!entry("Launcher.dll", true).is_candidate_dll("launcher.DLL"));
        assert!(!entry("readme.txt", true).is_candidate_dll("game.exe"));
    }

    #[test]
    fn bare_file_names_live_in_the_current_directory() {
        assert_eq!(containing_dir(Path::new("game.exe")), Path::new("."));
        assert_eq!(
            containing_dir(Path::new("/games/x/game.exe")),
            Path::new("/games/x")
        );
        assert_eq!(containing_dir(Path::new("/")), Path::new("."));
    }

    #[test]
    fn file_name_falls_back_to_the_whole_path() {
        assert_eq!(file_name(Path::new("/games/x/Game.exe")), "Game.exe");
        assert_eq!(file_name(Path::new("/")), "/");
    }

    #[test]
    fn read_at_most_enforces_the_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("x.dll");
        fs::write(&path, [7; 10]).expect("write");
        assert_eq!(read_at_most(&path, 10).expect("read"), Some(vec![7; 10]));
        assert_eq!(read_at_most(&path, 9).expect("read"), None);
        assert_eq!(read_at_most(&path, 0).expect("read"), None);
        assert!(read_at_most(&dir.path().join("missing.dll"), 10).is_err());
    }

    #[test]
    fn directory_entries_sort_ignoring_case() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["b.dll", "Eulaxp1.dll", "a.DLL", "shaders", "Z.exe"] {
            fs::write(dir.path().join(name), b"").expect("write");
        }
        let entries = list_dir(dir.path()).expect("list");
        let names: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["a.DLL", "b.dll", "Eulaxp1.dll", "shaders", "Z.exe"]);
        assert!(entries.iter().all(|entry| entry.is_file));
    }

    #[test]
    fn directories_and_symlinks_are_classified_by_target() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir(dir.path().join("plugins.dll")).expect("mkdir");
        fs::write(dir.path().join("real.dll"), b"").expect("write");
        #[cfg(unix)]
        std::os::unix::fs::symlink("real.dll", dir.path().join("link.dll")).expect("symlink");

        let entries = list_dir(dir.path()).expect("list");
        let is_file = |name: &str| {
            entries
                .iter()
                .find(|entry| entry.name == name)
                .map(|entry| entry.is_file)
        };
        assert_eq!(is_file("plugins.dll"), Some(false));
        assert_eq!(is_file("real.dll"), Some(true));
        #[cfg(unix)]
        assert_eq!(is_file("link.dll"), Some(true));
    }

    #[test]
    fn listing_a_missing_directory_fails() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(list_dir(&dir.path().join("missing")).is_err());
    }
}
