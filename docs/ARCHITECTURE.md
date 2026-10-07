# Architecture

This document is the map of the Uncork codebase. It says what each crate and module is for, where data lives on disk, how a component gets from its publisher to a bottle, how a graphics backend is chosen and wired into Wine for one process, how a launch is planned and executed, how the Steam client fits in, and how all of this is tested. Read it before your first code change. The doc comments in `crates/` are the specification; this file explains how the pieces fit and why. Decisions that are expensive to reverse are recorded in [adr/](adr/README.md).

## Bird's-eye view

Uncork is an orchestrator. It never translates an instruction or a draw call itself ([ADR 0002](adr/0002-orchestrate-not-reimplement.md)). Its work is:

1. Supply: download pinned, SHA-256-verified builds of Wine, DXMT and DXVK from their publishers, or import D3DMetal from the user's own copy of Apple's Game Porting Toolkit.
2. Bottles: create Wine prefixes with known registry settings, install the Windows Steam client into one.
3. Decide: inspect a game's PE files, look up its profile, and choose a graphics backend.
4. Launch: build one exact command with a constructed environment, put the backend's files in place, spawn the Wine loader directly, and log its output.

Steps 3 and 4 produce plain data (a `LaunchPlan`) before anything runs. `uncork play <game> --dry-run` prints that plan, and the unit tests assert on it.

## Invariants

| Invariant | Enforced by |
|---|---|
| Nothing unverified runs. Every downloaded component is pinned by SHA-256 in `runtime/catalog.toml`; a mismatch aborts the install. The one other download, Valve's `SteamSetup.exe`, changes in place upstream, so a hash difference is a warning and the file must still parse as a 32-bit Windows executable. | `catalog::Catalog::parse` validation, `download::download`, [SECURITY.md](../SECURITY.md) |
| Uncork never downloads, bundles or mirrors D3DMetal or the Steam client. | Catalog validation rejects `d3dmetal` entries; Steam's installer is fetched from Valve at run time. [LEGAL.md](LEGAL.md) |
| Wine processes get a constructed environment, not the user's shell environment. | `launch::ALLOWED_ENV`, `process::CommandSpec::env_clear` |
| Programs are spawned directly, never through `/bin/sh`, `env`, `nohup` or `arch`. | `process::spawn`; SIP purges `DYLD_*` when a protected binary starts ([Apple](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html)) |
| The Steam client always runs on WineD3D. | `steam::client_command` overrides every Direct3D DLL to builtin |
| No silent fallback. A fixed backend that is missing or cannot run the program is an error that names the install command. | `launch::plan` |
| No async runtime, no OpenSSL. | `deny.toml` bans `tokio` and `openssl-sys` |

## Crates

```text
 uncork (binary: CLI, clap)
   |  \
   |   +-----------------------+
   v                           v
 uncork-core  ------------->  uncork-pe      (PE inspection, no workspace deps)
   |
   +------------------------> uncork-steam   (Steam files and arguments, no workspace deps)
```

| Crate | Responsibility | Depends on |
|---|---|---|
| `crates/uncork` | The `uncork` binary. `cli.rs` describes the command-line surface (clap derive), `commands.rs` dispatches, `output.rs` prints tables, JSON, errors and download progress. The only crate that uses `anyhow`. | all three below |
| `crates/uncork-core` | The engine: data layout, catalog, downloads, components, Wine commands, bottles, registry, graphics backends, profiles, INI edits, launch planning, process spawning, Steam orchestration, Game Mode bundles, host checks. | `uncork-pe`, `uncork-steam` |
| `crates/uncork-pe` | Reads Windows PE files without running them: machine, bitness, large-address-aware and NX flags, imports, Wine's builtin marker; scans a game directory for the graphics API it uses. | none |
| `crates/uncork-steam` | Facts about the Windows Steam client with no Wine dependency: a KeyValues (`.vdf`/`.acf`) parser, library and app manifests, Windows-to-prefix path mapping, client argument lists. | none |

### `uncork-core` modules

| Module | Responsibility |
|---|---|
| `paths` | Where data lives (`$UNCORK_HOME`); see "Data on disk" |
| `config` | `config.toml` (default bottle) and the shared atomic TOML writer |
| `catalog` | The pinned component list compiled from `runtime/catalog.toml` |
| `download` | HTTPS downloads with streaming SHA-256, safe archive extraction |
| `component` | Installed components, their `component.toml`, layout validation, D3DMetal import |
| `wine` | A Wine runtime's binaries and the commands built from them |
| `bottle`, `registry` | Prefixes, their settings and state, registry defaults applied with `regedit` |
| `graphics` | Backend compatibility, recommendation, activation strategies, DLL overrides |
| `profile`, `ini` | Per-game profiles (built-in and user), INI keys enforced before launch |
| `launch`, `process` | Launch planning, the environment, `CommandSpec` and spawning |
| `steam` | Installing, starting and detecting the Steam client; planning Steam game launches |
| `gamemode` | macOS Game Mode wrapper bundles (experimental) |
| `host` | Host facts and the `doctor` checks |

## Data on disk

`paths::Layout` resolves every location. The root is `$UNCORK_HOME` when set and non-empty, otherwise `~/Library/Application Support/Uncork`. Nothing is created until `Layout::ensure` runs.

```text
$UNCORK_HOME
├── config.toml                       global settings: schema, default_bottle ("steam")
├── components/<kind>/<version>/      one unpacked component per kind and version
│   └── component.toml                kind, version, provenance, license, source code, features
├── bottles/<name>/                   one Wine prefix per bottle; WINEPREFIX points here
│   ├── uncork.toml                   bottle settings, user-editable
│   ├── uncork-state.toml             machine-written: backend DLLs in the prefix, Wine version
│   ├── cache/dxmt/  cache/dxvk/      shader caches set by the backend environment
│   └── drive_c/ dosdevices/ *.reg    Wine's own files
├── profiles/<id>.toml                user game profiles; replace built-ins with the same id
├── apps/<id>.app                     Game Mode wrapper bundles
├── cache/downloads/                  verified downloads, named <sha256[..16]>-<file name>
└── logs/                             one log per launch and per bottle creation
```

Keeping Uncork's two files at the prefix root, as CrossOver does with `cxbottle.conf`, makes moving a bottle a plain directory copy. `<kind>` is `wine`, `dxmt`, `dxvk` or `d3dmetal`; `<version>` is a directory name of 1 to 64 characters from `[A-Za-z0-9._+-]`, and versions sort "naturally" (`0.10` after `0.9`, `11.0-uncork2` after `11.0-uncork1`).

## Components and the catalog

A component is the unpacked upstream release of one layer. The four kinds:

| Kind | What it provides | Where it comes from |
|---|---|---|
| `wine` | An x86-64 Wine build for macOS with new-style WoW64 | The catalog |
| `dxmt` | Direct3D 10/11 to Metal | The catalog |
| `dxvk` | Direct3D 10/11 to Vulkan (Gcenx's macOS fork), run on MoltenVK | The catalog |
| `d3dmetal` | Apple's D3DMetal from the Game Porting Toolkit | `uncork runtime import-gptk <path>`, never the catalog |

The catalog, `runtime/catalog.toml`, is compiled into the binary. Each entry pins an HTTPS URL, the archive's size and SHA-256, the archive format, an optional top-level directory to strip, the SPDX license, where the source code lives, the homepage, feature tags and whether it is the recommended entry for its kind. Validation rejects a non-HTTPS URL, a malformed hash, an invalid version, a `d3dmetal` entry, a duplicate kind and version, more than one recommended entry per kind, and a zero size. The entries and the procedure for changing them are in [RUNTIME.md](RUNTIME.md).

`component::install_from_catalog` is the only way a downloaded binary reaches `components/`:

1. Download to `cache/downloads/<sha256[..16]>-<file name>`, hashing while streaming. A cached file that already verifies is reused without network access. The download is written to `<file>.part` and renamed only after the hash matches.
2. Extract into `components/<kind>/.staging-<version>`. Extraction rejects absolute paths, `..`, entries outside `strip_prefix`, symlinks that are absolute or resolve outside the destination, hard links and device files.
3. Check the layout (`component::validate_layout`; the required paths are listed in [RUNTIME.md](RUNTIME.md#required-component-layouts)).
4. Write `component.toml`, recording the source URL and verified hash, the license and the source-code location.
5. Rename the staging directory to `components/<kind>/<version>`. A failure at any step removes the staging directory.

D3DMetal is imported instead: `component::import_gptk` accepts the mounted GPTK volume, its `redist/` or its `redist/lib/`, reads the version from `D3DMetal.framework`'s `Info.plist` with `plutil`, and copies `external/` and `wine/` with `/bin/cp -R`, which preserves the `.so` symlinks D3DMetal's loader depends on (a dereferencing copy breaks it).

Feature tags describe what a Wine runtime can do; the planner reads them rather than guessing from version numbers. The tags (`wow64`, `msync`, `dxmt`, `d3dmetal`, `renderer-dllpath`, `dllpath-prepend`, `large-address-aware`) are defined in [RUNTIME.md](RUNTIME.md#runtime-features).

## Bottles

A bottle is a directory that is both the `WINEPREFIX` and the home of `uncork.toml` (`bottle::BottleConfig`) and `uncork-state.toml` (`bottle::BottleState`). `uncork.toml` holds the Wine version the bottle runs with (a bottle is never silently re-pinned), the Windows version, graphics settings (backend `auto` or fixed, optional DXMT/DXVK/D3DMetal version pins), performance switches, extra environment variables and DLL overrides. Unknown keys are errors, so typos surface. `uncork bottle set <name> key=value` edits it.

`bottle::create` runs these steps and logs them to `logs/bottle-<name>-create.log`:

1. Validate the name; refuse an existing directory.
2. Write `uncork.toml`.
3. Run `wine wineboot -u` with `WINEARCH` unset (new WoW64 prefixes are always 64-bit) and Mono and Gecko disabled, under a 300-second watchdog. On timeout, kill the wineserver and fail: `wineboot` can hang on macOS 26 and later ([Wine bug 59595](https://bugs.winehq.org/show_bug.cgi?id=59595)). Then wait for the wineserver to exit.
4. Import the registry defaults with `regedit /S`.
5. Record the Wine version in the state file.
6. Run `wine --version` once, so Rosetta translates the loader ahead of the first real launch.

A failed creation leaves the directory for inspection and the error names the log.

Registry defaults, all under `HKEY_CURRENT_USER\Software\Wine` (key names from [macdrv_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/winemac.drv/macdrv_main.c)):

| Key | Value | Why |
|---|---|---|
| `Mac Driver\RetinaMode` | `n` unless `performance.retina` | Prefix-wide by design; on, a game renders up to four times the pixels |
| `Mac Driver\UseConfinementCursorClipping`, `Mac Driver\CursorClippingLocksWindows` | `y` | Keeps the cursor in the window, which RTS edge scrolling needs |
| `Mac Driver\AllowVerticalSync` | `y` | |
| `Mac Driver\EnableAppNap` | `n` | App Nap throttles a backgrounded Steam client |
| `Version` | `win10` by default | What Steam and most games expect |

`uncork bottle import <path>` brings in an existing prefix (a CrossOver or Whisky bottle, or any `WINEPREFIX` with `drive_c` and `system.reg`). The default clones it with APFS (`/bin/cp -c -R`): instant, no extra space until files change, the original untouched. `--move` renames it instead.

## Graphics backends

| Backend | Translates | Bitness | Provided by |
|---|---|---|---|
| DXMT | Direct3D 10 and 11 to Metal | 32 and 64 | `dxmt` component |
| D3DMetal | Direct3D 11 and 12 to Metal; Direct3D 10 from GPTK 4 | 64 only | imported `d3dmetal` component |
| DXVK | Direct3D 10 and 11 to Vulkan, run on MoltenVK | 32 and 64 | `dxvk` component |
| WineD3D | DirectDraw, Direct3D 8, 9, 10 and 11 to OpenGL | 32 and 64 | built into Wine |

### Backend decision matrix

`graphics::recommend` takes the program's graphics API and bitness (from the profile, else from `uncork_pe::scan_game`), what is installed, an optional preferred backend and fallbacks from the profile or bottle, and whether the game uses geometry shaders. The preferred backend wins when it is installed and can run the program, then the fallbacks in order, then this table:

| API | 32-bit (PE32, i386) | 64-bit (PE32+, x86-64) |
|---|---|---|
| None detected, DirectDraw, Direct3D 8, Direct3D 9, OpenGL, Vulkan | WineD3D | WineD3D |
| Direct3D 10, Direct3D 11 | DXMT, WineD3D, DXVK | DXMT, D3DMetal, DXVK, WineD3D |
| Direct3D 12 | Unsupported; WineD3D, and the reason says it will fail | D3DMetal, WineD3D (fails without D3DMetal) |

"Installed" means the component exists and, for DXMT and D3DMetal, that the Wine runtime declares the matching feature tag. Each exclusion has a source:

| Rule | Reason | Source |
|---|---|---|
| D3DMetal never for 32-bit programs | It is x86-64 only and ships no i386 half | [utmapp/d3dmetal-native](https://github.com/utmapp/d3dmetal-native), [Sikarugir](https://github.com/Sikarugir-App/Sikarugir) |
| D3DMetal never for Direct3D 9 or older | It implements Direct3D 11, 12 and DXGI (plus Direct3D 10 from GPTK 4) | [GPTK 4.0b2 Read Me](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/Read%20Me.pdf), [frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163) |
| DXVK never when the game uses geometry shaders | MoltenVK 1.4.2 has no `geometryShader`; the work is an open PR | [MoltenVK #1815](https://github.com/KhronosGroup/MoltenVK/pull/1815) |
| DXVK never for Direct3D 9 | Gcenx's macOS fork ships only `d3d10core` and `d3d11`; its D3D9 cannot create a device on MoltenVK | [DXVK-macOS release](https://github.com/Gcenx/DXVK-macOS/releases/tag/v1.10.3-20230507-repack) |
| DXMT never for Direct3D 12 | DXMT's D3D12 work on `main` is marked "DO NOT USE" and is in no release | [dxmt#180](https://github.com/3Shain/dxmt/pull/180) |
| DXMT before DXVK for Direct3D 10/11 | Metal-native, no Vulkan layer in between; covers 32-bit games through WoW64 | [DXMT runtime specifications](https://github.com/3Shain/dxmt/wiki/Device-System-Runtime-Specifications) |
| WineD3D renders through OpenGL | Its automatic renderer is OpenGL; Uncork sets `WINE_D3D_CONFIG=renderer=gl` explicitly | [wined3d_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/wined3d/wined3d_main.c) |

The scan (`uncork_pe::scan_game`) reads the import and delay-load tables of the executable and of every `.dll` beside it, and searches them for graphics DLL names as ASCII and UTF-16 strings, because many games load their renderer at run time: Rise of Nations imports `d3d11.dll` from `d3dgl.dll`, not from `riseofnations.exe`. Redistributables (`d3dx9_*`, `d3dcompiler_*`, `msvcp*`, `steam_api*` and others) are skipped because they import graphics DLLs themselves. The scan also reports Steamworks use, kernel anti-cheat files (which do not run under Wine) and modules without `NX_COMPAT` (see [PERFORMANCE.md](PERFORMANCE.md#dep-and-nx_compat)). `uncork inspect <exe>` prints all of it.

### Activation strategies

Backends are chosen per process so the Steam client and a game run side by side in one prefix with different backends ([ADR 0003](adr/0003-per-process-backends.md)). How a backend is wired in depends on what the Wine runtime supports, declared by its feature tags. `graphics::strategy_for` picks the first that applies:

| Strategy | Runtime feature | Mechanism | Scope |
|---|---|---|---|
| `Builtin` | any | WineD3D is Wine's own; nothing to do | |
| `RendererEnv` | `renderer-dllpath` | Set `WINEDLLPATH_DXMT`, `WINEDLLPATH_DXVK` or `WINEDLLPATH_D3DMETAL` to the backend directory; Wine searches it before its own builtins | One process and its children |
| `DllPathPrepend` | `dllpath-prepend` | Set `WINEDLLPATH_PREPEND` to the backend directory | One process and its children |
| `PrefixNative` | neither | Copy the backend's DLLs, with Wine's builtin marker removed, into `system32` and `syswow64`, and set `<dlls>=n,b` for the process that should use them; every other process gets `<dlls>=b` | Per process via overrides; files are prefix-wide |

Plain `WINEDLLPATH` cannot do this job: Wine's loader searches its own DLL directory before `WINEDLLPATH`, so a builtin can never be overridden that way ([loader.c, wine-11.19](https://github.com/wine-mirror/wine/blob/wine-11.19/dlls/ntdll/unix/loader.c)). The Sikarugir 11.0 engine adds the `WINEDLLPATH_<RENDERER>` variables ([Sikarugir#283](https://github.com/Sikarugir-App/Sikarugir/issues/283)); `WINEDLLPATH_PREPEND` is a patch in Uncork's planned runtime ([RUNTIME.md](RUNTIME.md#patch-queue)).

`PrefixNative` is refused for D3DMetal: its PE files are forwarders to a Unix library, and removing the builtin marker loses the Unix half. In every strategy, DXMT's `winemetal.dll` is also copied, marker intact, into `system32` (from `x86_64-windows`) and `syswow64` (from `i386-windows`), as DXMT's installation guide requires ([DXMT guide](https://github.com/3Shain/dxmt/wiki/DXMT-Installation-Guide-for-Geeks)). Copies are skipped when the destination already has the same SHA-256 and are recorded in `uncork-state.toml`. Wine's placeholder DLLs are never deleted: a builtin whose placeholder is missing fails to load.

DLLs each backend replaces (the keys of its override entry):

| Backend | DLLs |
|---|---|
| DXMT | `d3d11`, `dxgi`, `d3d10core` |
| DXVK | `d3d11`, `d3d10core`. Wine's `dxgi` stays: DXMT's `dxgi` paired with DXVK's `d3d11` breaks swapchain creation |
| D3DMetal | `d3d11`, `dxgi`, `d3d12`, `d3d12core`, `d3d10`, `d3d10core` (those present in the component) |
| WineD3D | none |

In every strategy, the DLLs that other backends replace and this one does not are set to `b` for the process, so a native DLL left in `system32` by an earlier launch is never picked up by accident.

## Launch planning

`launch::plan` turns "run this program in that bottle" into a `LaunchPlan`: one `CommandSpec`, the backend `Activation` (files to copy, environment, overrides), the reason the backend was chosen, the log path and warnings. It reads the bottle and component directories and nothing else, so `--dry-run` and the tests see exactly what would run.

Backend selection, first match wins: `--backend` on the command line; the profile's `graphics.backend` and `fallbacks`; the bottle's `graphics.backend`; `auto`, which scans the executable and calls `recommend`. With `auto`, programs Wine resolves itself (`winecfg`, `regedit`, a `C:\` path) use WineD3D. The command is `<wine> <exe> <profile args> <args>`, run in the executable's directory, with output appended to `logs/<bottle>-<program>-<unix seconds>.log`.

Warnings in the plan include kernel anti-cheat files, a 32-bit game on a runtime without the `wow64` feature, and a Direct3D 12 game without D3DMetal.

### Environment

Wine processes never inherit the user's shell environment: a stray `DYLD_*`, `WINE*` or `MVK_*` variable from a shell profile is a classic cause of "works on my Mac". Layers, lowest precedence first:

| Layer | Variables |
|---|---|
| 1. Allowlist | `HOME`, `USER`, `LOGNAME`, `SHELL`, `TMPDIR`, `LANG`, `LC_ALL`, `LC_CTYPE`, `LC_MESSAGES`, `__CF_USER_TEXT_ENCODING` copied from the parent; `PATH=/usr/bin:/bin:/usr/sbin:/sbin` |
| 2. Wine basics | `WINEPREFIX=<bottle>`; `WINEDEBUG=-all`, or the channels from `--wine-debug` |
| 3. Bottle performance | `WINEMSYNC=1` when `performance.msync` is on and the runtime has `msync`; `ROSETTA_ADVERTISE_AVX=1` when `performance.avx` is on |
| 4. Backend activation | For example `WINEDLLPATH_DXMT`, `DXMT_LOG_LEVEL=none`, `DXMT_SHADER_CACHE_PATH=<bottle>/cache/dxmt`; `MTL_HUD_ENABLED=1` with `--hud` |
| 5. User layers | The bottle's `env`, then the profile's `env`, then `--env KEY=VALUE` |

`WINEDLLOVERRIDES` is layered the same way: `winemenubuilder.exe=d` (keeps Wine from creating menu entries and file associations on the Mac), then the activation's overrides, the bottle's `dll_overrides`, the profile's `dll_overrides`; later layers win per DLL. It renders grouped by value, for example `d3d10core,d3d11,dxgi=n,b;winemenubuilder.exe=d`.

Layers 1 to 3 (`launch::base_env`) are shared by game launches, the Steam client and bottle tools, so every process in a bottle agrees on one wineserver configuration. This matters for msync: every client of a wineserver must use the same `WINEMSYNC` setting, and a mismatched client exits ([wine-msync](https://github.com/marzent/wine-msync)).

`launch::execute` applies the activation's file copies and any registry change the launch needs (for example Retina mode), then spawns the command with `std::process::Command`, which uses `posix_spawn` on macOS.

## Profiles

A game profile is a TOML file with the settings that make one title run well: its Steam app id, the executable and its facts, the preferred backend and fallbacks, Wine and performance overrides, environment, DLL overrides, launch mode and arguments, INI keys to enforce, and its compatibility record. Built-in profiles in [profiles/](../profiles/) are compiled into the binary by `crates/uncork-core/build.rs`; a file in `$UNCORK_HOME/profiles/` with the same id replaces a built-in entirely. `uncork play <query>` resolves the query as an exact id, then a Steam app id, then an exact name (case-insensitive), then a unique substring of an id or name. The schema and validation rules are in [profiles/README.md](../profiles/README.md).

## Steam

Steamworks games need the Windows Steam client running in the same prefix and wineserver as the game; `steam_api` finds it through `HKCU\Software\Valve\Steam\ActiveProcess` ([steam_api](https://partner.steamgames.com/doc/api/steam_api)). [STEAM.md](STEAM.md) is the full description; the architecture in short:

- The client always runs on WineD3D (`d3d11,dxgi,d3d10core,d3d9,d3d12=b`) with its web UI rendered in software (`-cef-disable-gpu -cef-disable-gpu-compositing`). DXMT cannot present across processes, which Chromium's GPU process does ([dxmt#141](https://github.com/3Shain/dxmt/issues/141)); Wine's Mac driver does not implement cross-process child-window Metal swapchains either; and the client stays black on D3DMetal ([Highball#282](https://github.com/gauthierpiarrette/highball/issues/282)).
- Launch mode `direct` (the default for Steam games): make sure the client runs (`-silent`), then start the game executable directly in the same prefix with its own per-process backend environment.
- Launch mode `applaunch`, for games whose DRM needs Steam to start them: `steam.exe -applaunch <appid>`. A second `steam.exe` hands its arguments to the running client, and the game inherits the client's environment ([Highball SteamRestart.swift](https://github.com/gauthierpiarrette/highball/blob/main/Sources/HighballKit/SteamRestart.swift)), so Uncork restarts Steam with the game's environment first.
- Whether the client runs is read with `wine reg query` of the `ActiveProcess` `pid` value, not from `user.reg` on disk, which Wine flushes lazily.
- Library data comes from `steamapps/libraryfolders.vdf` and `appmanifest_<appid>.acf`, parsed by `uncork-steam`'s own KeyValues parser. Windows paths in those files are mapped into the prefix through `drive_c` and `dosdevices/`. Uncork reads these files and never writes them.

## Doctor

`uncork doctor` separates probing from judging. `host::HostInfo::probe` runs a few read-only commands (`sw_vers`, `sysctl`, `arch -x86_64 /usr/bin/true`, `df`) and never fails; unknown facts are `None`. `host::evaluate` is a pure function from those facts, the installed components and the bottles to a list of checks, so every check is unit-tested with hand-written inputs.

| Check | Fails or warns when | Fix it suggests |
|---|---|---|
| `platform` | Not macOS (fail); Intel Mac (warn, untested) | |
| `macos-version` | macOS older than 14 (fail) | Update macOS |
| `rosetta` | Rosetta missing (fail) | `softwareupdate --install-rosetta --agree-to-license` |
| `rosetta-sunset` | macOS 27 or later (info): the last release with general-purpose Rosetta | See [ROADMAP.md](ROADMAP.md#the-end-of-rosetta) |
| `disk-space` | Less than 20 GiB free (warn) | |
| `wine-installed` | No Wine component (fail) | `uncork runtime install wine` |
| `wine-32bit` | Newest Wine lacks `wow64` (warn: 32-bit games will not start) | |
| `dxmt-installed` | DXMT missing (warn: Direct3D 10/11 games fall back to OpenGL) | `uncork runtime install dxmt` |
| `dxvk-installed` | DXVK missing (info: optional fallback) | `uncork runtime install dxvk` |
| `d3dmetal` | Not imported (info: optional, 64-bit Direct3D 11/12) | `uncork runtime import-gptk <path>` |
| `bottles` | No bottles (info) | `uncork setup` |
| `bottle-wine` | A bottle's Wine version is not installed (warn, per bottle) | `uncork runtime install wine` or `uncork bottle set <name> wine=<v>` |
| `crossover` | CrossOver is installed (info: its bottles can be imported) | `uncork bottle import` |

Rosetta is detected by running an x86-64 binary, not with `pgrep oahd`: `oahd` starts on demand, so its absence proves nothing. Upgrading to macOS 27 does not restore Rosetta ([macOS 27 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-27-release-notes)), so a Mac that ran games before the upgrade fails this check after it.

## Game Mode (experimental)

macOS turns Game Mode on for a frontmost, full-screen app whose bundle declares the games category, and not for a binary spawned from a terminal ([macOS 26 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-26-release-notes), [Apple Support](https://support.apple.com/en-us/105118)). Wine's loader is a bare executable, so a Wine game never qualifies on its own. With `performance.game_mode = true`, Uncork writes `apps/<id>.app` with a games-category `Info.plist`, a copy of the `uncork` binary as its executable and the launch plan as `plan.json`, ad-hoc signs it, and opens it with `/usr/bin/open -n -W`. The launcher `exec`s the Wine loader in place. Whether Game Mode survives that `exec` is unmeasured; it is off by default until it is ([PERFORMANCE.md](PERFORMANCE.md#game-mode)).

## Errors

Library errors are one `thiserror` enum per crate (`uncork_core::Error`, `uncork_pe::PeError`, `uncork_steam::library::LibraryError`). Messages are written for the person at the terminal:

- I/O errors name the action and the path (`cannot create directory /...: Permission denied`).
- `NotFound` carries a hint naming the command that fixes it, for example that `uncork runtime install wine` installs a missing Wine.
- `Command` errors name the program and the log file that holds its output.
- `Unsupported` explains why a backend cannot run a program instead of falling back silently.

The CLI prints `error: <message>` and one `caused by:` line per source and exits non-zero. `unwrap` and `expect` appear in library code only on invariants documented next to them, for example that the compiled-in catalog and profiles are valid (unit tests guarantee it).

## Why synchronous

Uncork's work is a handful of sequential downloads and child processes it waits on. Blocking I/O on the calling thread is enough, keeps stack traces readable and keeps an async runtime out of the dependency graph. HTTPS is `ureq` with rustls. `deny.toml` bans `tokio` and `openssl-sys` so neither slips in through a dependency.

## Testing

Each layer is tested at the cheapest level that exercises it:

| Layer | Strategy |
|---|---|
| Pure functions (`recommend`, `supports`, `render_overrides`, `host::evaluate`, `client_args`, `ini::apply`, `registry::render`, `compare_versions`) | Unit tests with hand-built inputs, plus `proptest` where the contract states an invariant: KeyValues `parse(to_string(o)) == o`, INI edits preserving untouched bytes, version ordering |
| Plans (`launch::plan`, `steam::client_command`, Wine command builders) | Unit tests that build a bottle and components under a `tempfile` data root (`Layout::at`) and assert on the resulting `CommandSpec`, environment and overrides |
| Process execution, bottle creation, watchdogs | Fake `wine` and `wineserver` shell scripts written into a temporary component and made executable; they record their arguments and environment, exit with a chosen status or sleep past a timeout |
| Downloads and archives | Local archives built in the test, including hostile ones (`..`, absolute paths, escaping symlinks); no network |
| PE scanning | Small PE images built in memory |
| Built-in data | Every file in `runtime/catalog.toml` and `profiles/` is parsed and validated by unit tests |
| CLI | `assert_cmd` integration tests in `crates/uncork/tests/`, with `UNCORK_HOME` set on the child process to a temporary directory |

No test downloads anything, touches the real `~/Library`, or needs Wine or Rosetta. Real-game validation is manual and recorded as compatibility reports ([COMPATIBILITY.md](COMPATIBILITY.md)); `uncork bench` (M3) will make performance runs repeatable.

## Future work

### SwiftUI app (M4)

The planned graphical app is SwiftUI over the Rust core through [UniFFI](https://crates.io/crates/uniffi), in a new `uncork-ffi` crate. UniFFI is the most widely used Rust-to-Swift bridge (matrix-rust-sdk and Bitwarden ship it); it is pre-1.0 and breaks on minor versions, so it will be pinned exactly, and its MPL-2.0 license is already allowed by `deny.toml`. The CLI stays complete, so Rust contributors never need Xcode. Rejected: Tauri (a web view, not native), Slint (GPL-3.0 or Slint's own licenses, not MIT or Apache), gpui (unpublished since 2025, unstable API).

### CPU backend (M5)

Today every Wine process is x86-64 under Rosetta 2. Apple keeps general-purpose Rosetta only through macOS 27 ([Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)). The plan is a `CpuBackend` abstraction in `uncork-core` with `RosettaX86_64` as the only implementation now and an ARM64 Wine with an x86 emulator (FEX) as the second. Bottles will record which backend created them, because a prefix cannot be converted between architectures. The timeline and the open questions are in [ROADMAP.md](ROADMAP.md#m5-rosetta-sunset-plan-and-arm64-path).
