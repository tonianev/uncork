//! A Wine runtime: the `wine` component's binaries and the commands Uncork
//! builds from them.

use std::path::{Path, PathBuf};

use crate::component::{ComponentKind, InstalledComponent};
use crate::process::CommandSpec;

/// `WINEDLLOVERRIDES` for every Wine command: never let Wine create macOS
/// menu entries and `.desktop`-style launchers for programs it installs.
const DEFAULT_OVERRIDES: &str = "winemenubuilder.exe=d";

/// `WINEDLLOVERRIDES` while a prefix is created: also skip the Mono and Gecko
/// installers, which would otherwise open download dialogs.
const BOOT_INIT_OVERRIDES: &str = "mscoree,mshtml,winemenubuilder.exe=d";

/// A usable Wine installation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WineRuntime {
    /// Component directory (contains `bin/` and `lib/`).
    pub root: PathBuf,
    /// Component version.
    pub version: String,
    /// Feature tags from the component (`msync`, `wow64`, ...).
    pub features: Vec<String>,
}

impl WineRuntime {
    /// Wrap an installed `wine` component, checking the loader exists.
    ///
    /// # Errors
    /// [`crate::Error::BrokenComponent`] if `kind` is not Wine or no loader
    /// binary is found.
    pub fn from_component(component: &InstalledComponent) -> crate::Result<WineRuntime> {
        let broken = |message: String| crate::Error::BrokenComponent {
            kind: component.meta.kind,
            path: component.path.clone(),
            message,
        };
        if component.meta.kind != ComponentKind::Wine {
            return Err(broken(format!(
                "expected a {} component, not {}",
                ComponentKind::Wine,
                component.meta.kind
            )));
        }
        let runtime = WineRuntime {
            root: component.path.clone(),
            version: component.meta.version.clone(),
            features: component.meta.features.clone(),
        };
        if !runtime.wine_bin().is_file() {
            return Err(broken("no Wine loader (bin/wine or bin/wine64)".to_owned()));
        }
        Ok(runtime)
    }

    /// The loader: `bin/wine` if present (new-style WoW64 builds ship only
    /// this), else `bin/wine64`.
    #[must_use]
    pub fn wine_bin(&self) -> PathBuf {
        let bin = self.root.join("bin");
        let wine = bin.join("wine");
        if wine.is_file() {
            wine
        } else {
            bin.join("wine64")
        }
    }

    /// `bin/wineserver`.
    #[must_use]
    pub fn wineserver_bin(&self) -> PathBuf {
        self.root.join("bin").join("wineserver")
    }

    /// `true` if the component declares `feature`.
    #[must_use]
    pub fn has_feature(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// A command running `program` (an absolute path, e.g. the loader or
    /// `wineserver`) with only `WINEPREFIX=<prefix>`, `WINEDEBUG=-all` and
    /// `WINEDLLOVERRIDES=winemenubuilder.exe=d` set. Callers that launch
    /// programs add [`crate::launch::base_env`] on top. `env_clear` is set:
    /// the inherited environment is replaced by
    /// [`crate::launch::ALLOWED_ENV`] and [`crate::launch::WINE_PATH`].
    #[must_use]
    pub fn base_command(&self, program: &Path, prefix: &Path) -> CommandSpec {
        let mut spec = CommandSpec::new(program)
            .env("WINEPREFIX", prefix.to_string_lossy())
            .env("WINEDEBUG", "-all")
            .env("WINEDLLOVERRIDES", DEFAULT_OVERRIDES);
        spec.env_clear = true;
        spec
    }

    /// `wine wineboot -u` for a new prefix (creates it when missing). Also
    /// sets `WINEDLLOVERRIDES=mscoree,mshtml,winemenubuilder.exe=d` so Wine
    /// does not offer to download Mono and Gecko during creation.
    #[must_use]
    pub fn boot_init_command(&self, prefix: &Path) -> CommandSpec {
        self.wine_command(prefix)
            .args(["wineboot", "-u"])
            .env("WINEDLLOVERRIDES", BOOT_INIT_OVERRIDES)
    }

    /// `wine wineboot --update` (after a Wine version change).
    #[must_use]
    pub fn boot_update_command(&self, prefix: &Path) -> CommandSpec {
        self.wine_command(prefix).args(["wineboot", "--update"])
    }

    /// `wineserver --wait`: blocks until every process in the prefix exits.
    #[must_use]
    pub fn wait_command(&self, prefix: &Path) -> CommandSpec {
        self.base_command(&self.wineserver_bin(), prefix)
            .arg("--wait")
    }

    /// `wineserver --kill`.
    #[must_use]
    pub fn kill_command(&self, prefix: &Path) -> CommandSpec {
        self.base_command(&self.wineserver_bin(), prefix)
            .arg("--kill")
    }

    /// `wine regedit /S <C:\windows\temp\file.reg>`.
    #[must_use]
    pub fn regedit_import_command(&self, prefix: &Path, windows_path: &str) -> CommandSpec {
        self.wine_command(prefix)
            .args(["regedit", "/S", windows_path])
    }

    /// `wine winecfg /v <name>`.
    #[must_use]
    pub fn set_windows_version_command(
        &self,
        prefix: &Path,
        version: crate::bottle::WindowsVersion,
    ) -> CommandSpec {
        self.wine_command(prefix)
            .args(["winecfg", "/v", version.winecfg_name()])
    }

    /// `wine --version`, for `doctor` and `runtime list`.
    #[must_use]
    pub fn version_command(&self) -> CommandSpec {
        CommandSpec::new(self.wine_bin()).arg("--version")
    }

    /// [`WineRuntime::base_command`] for the loader.
    fn wine_command(&self, prefix: &Path) -> CommandSpec {
        self.base_command(&self.wine_bin(), prefix)
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;
    use crate::bottle::WindowsVersion;
    use crate::component::{ComponentMeta, Source};

    fn component(kind: ComponentKind, dir: &Path) -> InstalledComponent {
        InstalledComponent {
            meta: ComponentMeta {
                schema: 1,
                kind,
                version: "11.0-test".to_owned(),
                source: Source::Local {
                    path: dir.to_path_buf(),
                },
                license: "LGPL-2.1-or-later".to_owned(),
                source_code: None,
                features: vec!["msync".to_owned(), "wow64".to_owned()],
                installed_unix: 0,
            },
            path: dir.to_path_buf(),
        }
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"#!/bin/sh\n").unwrap();
    }

    fn runtime(root: &Path) -> WineRuntime {
        WineRuntime {
            root: root.to_path_buf(),
            version: "11.0".to_owned(),
            features: Vec::new(),
        }
    }

    fn args(spec: &CommandSpec) -> Vec<&str> {
        spec.args.iter().map(|a| a.to_str().unwrap()).collect()
    }

    #[test]
    fn from_component_accepts_wine_with_loader() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("bin/wine"));
        let wine =
            WineRuntime::from_component(&component(ComponentKind::Wine, dir.path())).unwrap();
        assert_eq!(wine.root, dir.path());
        assert_eq!(wine.version, "11.0-test");
        assert!(wine.has_feature("msync"));
        assert!(!wine.has_feature("dxmt"));
    }

    #[test]
    fn from_component_accepts_wine64_only_builds() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("bin/wine64"));
        let wine =
            WineRuntime::from_component(&component(ComponentKind::Wine, dir.path())).unwrap();
        assert_eq!(wine.wine_bin(), dir.path().join("bin/wine64"));
    }

    #[test]
    fn from_component_rejects_other_kinds() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("bin/wine"));
        let err =
            WineRuntime::from_component(&component(ComponentKind::Dxmt, dir.path())).unwrap_err();
        assert!(
            matches!(&err, crate::Error::BrokenComponent { kind: ComponentKind::Dxmt, path, .. } if path == dir.path()),
            "{err:?}"
        );
        assert!(
            err.to_string().contains("expected a wine component"),
            "{err}"
        );
    }

    #[test]
    fn from_component_rejects_missing_loader() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("bin/wineserver"));
        let err =
            WineRuntime::from_component(&component(ComponentKind::Wine, dir.path())).unwrap_err();
        assert!(
            matches!(
                err,
                crate::Error::BrokenComponent {
                    kind: ComponentKind::Wine,
                    ..
                }
            ),
            "{err:?}"
        );
    }

    #[test]
    fn from_component_rejects_loader_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("bin/wine")).unwrap();
        assert!(WineRuntime::from_component(&component(ComponentKind::Wine, dir.path())).is_err());
    }

    #[test]
    fn wine_bin_prefers_wine_over_wine64() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("bin/wine"));
        touch(&dir.path().join("bin/wine64"));
        assert_eq!(runtime(dir.path()).wine_bin(), dir.path().join("bin/wine"));
    }

    #[test]
    fn wine_bin_falls_back_to_wine64() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            runtime(dir.path()).wine_bin(),
            dir.path().join("bin/wine64")
        );
    }

    #[test]
    fn base_command_sets_exactly_the_wine_basics() {
        let wine = runtime(Path::new("/rt"));
        let spec = wine.base_command(Path::new("/rt/bin/wine"), Path::new("/b/steam"));
        assert_eq!(spec.program, Path::new("/rt/bin/wine"));
        assert!(spec.args.is_empty());
        assert!(spec.env_clear);
        assert_eq!(spec.cwd, None);
        assert_eq!(spec.log, None);
        let env: Vec<(&str, &str)> = spec
            .env
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect();
        assert_eq!(
            env,
            [
                ("WINEDEBUG", "-all"),
                ("WINEDLLOVERRIDES", "winemenubuilder.exe=d"),
                ("WINEPREFIX", "/b/steam"),
            ]
        );
    }

    #[test]
    fn boot_init_disables_mono_and_gecko() {
        let wine = runtime(Path::new("/rt"));
        let spec = wine.boot_init_command(Path::new("/b/x"));
        assert_eq!(spec.program, Path::new("/rt/bin/wine64"));
        assert_eq!(args(&spec), ["wineboot", "-u"]);
        assert_eq!(
            spec.env["WINEDLLOVERRIDES"],
            "mscoree,mshtml,winemenubuilder.exe=d"
        );
        assert_eq!(spec.env["WINEPREFIX"], "/b/x");
        assert!(spec.env_clear);
    }

    #[test]
    fn boot_update_runs_wineboot_update() {
        let spec = runtime(Path::new("/rt")).boot_update_command(Path::new("/b/x"));
        assert_eq!(args(&spec), ["wineboot", "--update"]);
        assert_eq!(spec.env["WINEDLLOVERRIDES"], "winemenubuilder.exe=d");
    }

    #[test]
    fn wineserver_commands_target_the_prefix() {
        let wine = runtime(Path::new("/rt"));
        let wait = wine.wait_command(Path::new("/b/x"));
        let kill = wine.kill_command(Path::new("/b/x"));
        for (spec, flag) in [(&wait, "--wait"), (&kill, "--kill")] {
            assert_eq!(spec.program, Path::new("/rt/bin/wineserver"));
            assert_eq!(spec.args, [OsString::from(flag)]);
            assert_eq!(spec.env["WINEPREFIX"], "/b/x");
            assert!(spec.env_clear);
        }
    }

    #[test]
    fn regedit_import_is_silent() {
        let spec = runtime(Path::new("/rt"))
            .regedit_import_command(Path::new("/b/x"), r"C:\windows\temp\a.reg");
        assert_eq!(args(&spec), ["regedit", "/S", r"C:\windows\temp\a.reg"]);
    }

    #[test]
    fn set_windows_version_uses_winecfg_names() {
        let wine = runtime(Path::new("/rt"));
        for (version, name) in [
            (WindowsVersion::Win7, "win7"),
            (WindowsVersion::Win81, "win81"),
            (WindowsVersion::Win10, "win10"),
            (WindowsVersion::Win11, "win11"),
        ] {
            let spec = wine.set_windows_version_command(Path::new("/b/x"), version);
            assert_eq!(args(&spec), ["winecfg", "/v", name]);
        }
    }

    #[test]
    fn version_command_needs_no_prefix() {
        let spec = runtime(Path::new("/rt")).version_command();
        assert_eq!(args(&spec), ["--version"]);
        assert!(spec.env.is_empty());
    }
}
