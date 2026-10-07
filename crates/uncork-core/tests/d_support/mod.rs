//! Fixtures for the launch, Steam and Game Mode tests: a data root in a
//! temporary directory, a fake Wine runtime (shell scripts that log what
//! they are asked to do, see `fake_wine.sh`), fake graphics components,
//! bottles and Steam libraries. Nothing here touches the real data root,
//! the network or a real Wine.

#![allow(dead_code, reason = "each test crate uses a different subset")]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use uncork_core::bottle::{Bottle, BottleConfig, BottleState};
use uncork_core::component::{self, ComponentKind, ComponentMeta, InstalledComponent, Source};
use uncork_core::launch::PlanContext;
use uncork_core::paths::Layout;
use uncork_core::profile::GameProfile;
use uncork_core::wine::WineRuntime;

/// The fake loader; `@STATE@` is replaced with the state directory.
pub const FAKE_WINE: &str = include_str!("fake_wine.sh");
/// The fake wineserver.
pub const FAKE_WINESERVER: &str = include_str!("fake_wineserver.sh");

/// Every feature of the CrossOver-derived runtime the live tests used.
pub const CX_FEATURES: &[&str] = &["wow64", "msync", "dxmt", "d3dmetal", "large-address-aware"];

/// Steam's directory below `drive_c`.
pub const STEAM_DIR: &str = "Program Files (x86)/Steam";

pub struct Fixture {
    pub dir: tempfile::TempDir,
    pub layout: Layout,
    /// Control files and `calls.log` of the fake runtime.
    pub state: PathBuf,
    pub wine: WineRuntime,
}

impl Fixture {
    /// A data root and a fake Wine runtime with `features`. The runtime has
    /// DXMT's Unix bridge (`lib/wine/x86_64-unix/winemetal.so`), as
    /// CrossOver-derived runtimes do.
    pub fn new(features: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::at(dir.path().join("uncork"));
        layout.ensure().unwrap();
        let state = dir.path().join("fake-state");
        fs::create_dir_all(&state).unwrap();
        let root = dir.path().join("runtime");
        for (name, script) in [("wine", FAKE_WINE), ("wineserver", FAKE_WINESERVER)] {
            let path = root.join("bin").join(name);
            write(
                &path,
                script
                    .replace("@STATE@", state.to_str().unwrap())
                    .as_bytes(),
            );
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        write(&root.join("lib/wine/x86_64-unix/winemetal.so"), b"bridge");
        fs::create_dir_all(root.join("lib/wine/x86_64-windows")).unwrap();
        let wine = WineRuntime {
            root,
            version: "11.17-fake".to_owned(),
            features: features.iter().map(|f| (*f).to_owned()).collect(),
        };
        Fixture {
            dir,
            layout,
            state,
            wine,
        }
    }

    /// A bottle `bottles/<name>` with a `drive_c`, written to disk.
    pub fn bottle(&self, name: &str, edit: impl FnOnce(&mut BottleConfig)) -> Bottle {
        let path = self.layout.bottle_dir(name);
        fs::create_dir_all(path.join("drive_c/windows/system32")).unwrap();
        fs::create_dir_all(path.join("drive_c/windows/syswow64")).unwrap();
        fs::create_dir_all(path.join("drive_c/users/Public")).unwrap();
        let mut config: BottleConfig = toml::from_str(&format!(
            "schema = 1\nname = \"{name}\"\nwine = \"{}\"\n",
            self.wine.version
        ))
        .unwrap();
        edit(&mut config);
        let bottle = Bottle {
            path,
            config,
            state: BottleState::default(),
        };
        bottle.save_config().unwrap();
        bottle
    }

    /// Register a component with these files under `components/`.
    pub fn component(
        &self,
        kind: ComponentKind,
        version: &str,
        files: &[(&str, Vec<u8>)],
    ) -> InstalledComponent {
        let path = self.layout.component_dir(kind, version);
        for (file, bytes) in files {
            write(&path.join(file), bytes);
        }
        let meta = ComponentMeta {
            schema: 1,
            kind,
            version: version.to_owned(),
            source: Source::Local {
                path: PathBuf::from("/fake"),
            },
            license: "test".to_owned(),
            source_code: None,
            features: Vec::new(),
            installed_unix: 0,
        };
        write(
            &path.join(component::META_FILE),
            toml::to_string(&meta).unwrap().as_bytes(),
        );
        InstalledComponent { meta, path }
    }

    /// DXMT with both architectures and its Unix bridge.
    pub fn dxmt(&self, version: &str) -> InstalledComponent {
        let mut files = Vec::new();
        for arch in ["x86_64-windows", "i386-windows"] {
            for dll in ["d3d11", "dxgi", "d3d10core", "winemetal"] {
                files.push((
                    format!("{arch}/{dll}.dll"),
                    builtin_dll(&format!("dxmt {version} {arch} {dll}")),
                ));
            }
        }
        files.push(("x86_64-unix/winemetal.so".to_owned(), b"so".to_vec()));
        let files: Vec<(&str, Vec<u8>)> =
            files.iter().map(|(f, b)| (f.as_str(), b.clone())).collect();
        self.component(ComponentKind::Dxmt, version, &files)
    }

    /// DXVK-macOS: `d3d11` and `d3d10core` for both architectures.
    pub fn dxvk(&self, version: &str) -> InstalledComponent {
        let mut files = Vec::new();
        for arch in ["x86_64-windows", "i386-windows"] {
            for dll in ["d3d11", "d3d10core"] {
                files.push((
                    format!("{arch}/{dll}.dll"),
                    builtin_dll(&format!("dxvk {version} {arch} {dll}")),
                ));
            }
        }
        let files: Vec<(&str, Vec<u8>)> =
            files.iter().map(|(f, b)| (f.as_str(), b.clone())).collect();
        self.component(ComponentKind::Dxvk, version, &files)
    }

    /// An imported D3DMetal.
    pub fn d3dmetal(&self, version: &str) -> InstalledComponent {
        self.component(
            ComponentKind::D3dmetal,
            version,
            &[
                ("external/libd3dshared.dylib", b"dylib".to_vec()),
                (
                    "external/D3DMetal.framework/Versions/A/D3DMetal",
                    b"fw".to_vec(),
                ),
                ("wine/x86_64-windows/d3d12.dll", builtin_dll("d3d12")),
                ("wine/x86_64-windows/d3d11.dll", builtin_dll("d3d11")),
                ("wine/x86_64-windows/dxgi.dll", builtin_dll("dxgi")),
            ],
        )
    }

    /// Every component on disk, as `PlanContext::components` wants them.
    pub fn components(&self) -> Vec<InstalledComponent> {
        component::list_installed(&self.layout).unwrap()
    }

    /// The fake runtime's call log, one line per call.
    pub fn calls(&self) -> Vec<String> {
        fs::read_to_string(self.state.join("calls.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// Create a control file of the fake runtime.
    pub fn flag(&self, name: &str) {
        fs::write(self.state.join(name), "").unwrap();
    }

    /// The `ActiveProcess` pid the fake `reg query` reports.
    pub fn set_pid(&self, value: &str) {
        fs::write(self.state.join("pid"), value).unwrap();
    }

    pub fn pid(&self) -> Option<String> {
        fs::read_to_string(self.state.join("pid")).ok()
    }

    /// Put a `steam.exe` into `bottle`; returns the Steam directory.
    pub fn install_steam(&self, bottle: &Bottle) -> PathBuf {
        let root = bottle.drive_c().join(STEAM_DIR);
        write(&root.join("steam.exe"), b"steam");
        root
    }

    /// Register Steam app `appid` (`StateFlags` = `state_flags`) installed in
    /// `steamapps/common/<installdir>` of the bottle's Steam directory, and
    /// return that directory.
    pub fn steam_game(
        &self,
        bottle: &Bottle,
        appid: u32,
        name: &str,
        installdir: &str,
        state_flags: u32,
    ) -> PathBuf {
        let steamapps = bottle.drive_c().join(STEAM_DIR).join("steamapps");
        write(
            &steamapps.join(format!("appmanifest_{appid}.acf")),
            format!(
                "\"AppState\"\n{{\n\t\"appid\"\t\t\"{appid}\"\n\t\"name\"\t\t\"{name}\"\n\t\"installdir\"\t\t\"{installdir}\"\n\t\"StateFlags\"\t\t\"{state_flags}\"\n}}\n"
            )
            .as_bytes(),
        );
        let dir = steamapps.join("common").join(installdir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}

/// A plan context over the fixture's runtime.
pub fn ctx<'a>(
    fx: &'a Fixture,
    bottle: &'a Bottle,
    components: &'a [InstalledComponent],
    profile: Option<&'a GameProfile>,
) -> PlanContext<'a> {
    PlanContext {
        layout: &fx.layout,
        bottle,
        wine: &fx.wine,
        components,
        profile,
    }
}

/// A Wine-built DLL: `MZ`, Wine's builtin marker at 0x40, then `tag`.
pub fn builtin_dll(tag: &str) -> Vec<u8> {
    let mut bytes = vec![0; 0x40];
    bytes[..2].copy_from_slice(b"MZ");
    bytes.extend_from_slice(uncork_pe::WINE_BUILTIN_SIGNATURE);
    bytes.extend_from_slice(tag.as_bytes());
    bytes
}

/// Parse a profile, checking it is valid.
pub fn profile(text: &str) -> GameProfile {
    let profile = GameProfile::parse(text, Path::new("test.toml")).unwrap();
    assert_eq!(profile.problems(), Vec::<String>::new(), "{text}");
    profile
}

/// Write `bytes` to `path`, creating its directory.
pub fn write(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}
