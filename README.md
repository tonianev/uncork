# Uncork

Uncork runs Windows games from Steam on Apple Silicon Macs. It is an open-source alternative to CrossOver for that one job, written in Rust as a command-line tool. Uncork does not reimplement Wine or any graphics layer. It installs pinned, checksum-verified builds of Wine (x86-64, run under Rosetta 2), DXMT and DXVK on MoltenVK, and imports Apple's D3DMetal from your own copy; it creates a Wine prefix (a "bottle") with the Windows Steam client in it, or imports one you already have; and it starts each game with the translation layer and settings that suit it. The first target is Rise of Nations: Extended Edition, a 32-bit Direct3D 11 game.

Uncork is an independent project. It is not affiliated with or endorsed by CodeWeavers, Apple, Valve or Microsoft. Product names and trademarks belong to their owners; see [docs/LEGAL.md](docs/LEGAL.md).

## Status

Pre-alpha, between milestones M0 and M1. On one Mac, `uncork play rise-of-nations` starts the Windows Steam client and the game, and the game reaches its main menu on DXMT. A full match has not been played yet, and setup from nothing on a clean Mac has not been run yet. The plan and what is left are in [docs/ROADMAP.md](docs/ROADMAP.md#m1-end-to-end-on-rise-of-nations-with-a-pinned-runtime).

| State | Area |
|---|---|
| Implemented, run on real hardware ([Verified on](#verified-on)) | `doctor`; `runtime install` from SHA-256-verified archives; `bottle import` of a CrossOver bottle (APFS clone, its Windows user kept); `bottle create` and `run` with real Wine, 32-bit program included; `steam games`; `play` in `direct` mode: Steam started silently on app-local DXVK, then the game on DXMT 0.80 with the profile's overrides and INI edit; `bottle kill` including Steam's running marker; msync on |
| Implemented and unit-tested, not yet run on real hardware | `setup` and `steam install` on a clean Mac (network downloads, Valve's installer, first sign-in); `applaunch` and `standalone` launch modes; DXVK and WineD3D as game backends; `--backend`, `--metalfx`, `--retina`; D3DMetal import (`runtime import-gptk`); Game Mode app bundles (`--game-mode`, experimental); `winetricks`; `bottle set`, `env`, `tool`, `delete` |
| Planned | Wine runtime built by Uncork's own CI from CrossOver 26.3 sources (M2); `uncork bench`, a compatibility database and reports (M3); a SwiftUI app over the Rust core (M4); a post-Rosetta ARM64 path (M5) |

The unit and integration tests use fake `wine` and `wineserver` scripts; they never start Wine or download anything ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#testing)).

## Verified on

Hands-on results from 2026-10-07, through the `uncork` CLI itself unless noted. The game's profile stays `untested` until a full match has been played ([docs/COMPATIBILITY.md](docs/COMPATIBILITY.md#current-results)).

| | |
|---|---|
| Mac | MacBook Pro, Apple M5 Max |
| macOS | 27.0.1 (26A434), Rosetta 2 installed |
| Uncork | 0.1.0, `main` of 2026-10-07 |
| Components | Wine `winecx-gptk-4.7.3` (`wine --version`: wine-11.17), DXMT 0.80, DXVK-macOS 1.10.3-20230507 |
| Steam | Windows client build 1788652215 (64-bit), in a bottle imported from CrossOver |

| Step | Result |
|---|---|
| `uncork doctor` | Reported Apple M5 Max, macOS 27.0.1, Rosetta present, the `rosetta-sunset` notice and the installed CrossOver |
| `uncork runtime install all -y` | Installed the three components in 3.4 s from the download cache (the 461 MB, 19 MB and 3 MB archives had been downloaded and SHA-256-verified earlier) |
| `uncork bottle import "<CrossOver>/Bottles/Steam" --name steam` | Cloned the 21 GB CrossOver bottle with APFS clones in about 6 s, original untouched; set `USER=crossover` and `LOGNAME=crossover` for the bottle |
| `uncork steam games` | Listed 228980 (Steamworks Common Redistributables), 287450 (Rise of Nations: Extended Edition, 2.7 GB, profile matched) and 813780 (Age of Empires II: Definitive Edition, 16.5 GB, profile matched) |
| `uncork play rise-of-nations --hud` | Started Steam (`-silent`) and then the game, about 44 s in total. The game ran on DXMT 0.80: `lsof` showed Uncork's `syswow64/d3d11.dll` and `dxgi.dll`, the game's own `d3dcompiler_47.dll`, builtin `winemetal.dll` with `winemetal.so`, and Apple's AGX Metal driver. Steam's GPU process had the app-local DXVK `d3d11.dll` from `Steam/bin/cef/cef.win64` loaded at the same time. `SkipIntroMovies=1` was written to `rise2.ini` |
| Same configuration, run by hand earlier that day | Main menu rendered, windowed; Metal HUD: 120 FPS (the display's cap), GPU time 0.48 ms per frame, frame interval 8.33 ms; Game Mode off |
| `uncork bottle create smoke-test` | 13.8 s with Rosetta's translation cache warm; `wineboot -u` took 29 s cold when run by hand |
| `uncork run --bottle smoke-test --wait 'C:\windows\syswow64\cmd.exe' -- /c 'echo hello & ver'` | The 32-bit `cmd.exe` printed to the per-launch log |
| `uncork bottle kill steam` | Stopped every process in the bottle and reset Steam's `ActiveProcess` `pid` to 0 |
| Steam's window | Client chrome, sign-in and the library list render; the embedded store page body stays black ([docs/STEAM.md](docs/STEAM.md#known-issues)) |

Not verified yet: a full match, audio, videos, multiplayer, alt-tab and full screen; Game Mode through an app bundle (the display was asleep during that test); `uncork setup` with a fresh Steam install on a clean Mac.

## Why

Performance comes from the stack Uncork assembles, not from the orchestrator. The orchestrator's job is to pick the right piece for each game and get out of the way.

| Layer | What Uncork uses | Why | Source |
|---|---|---|---|
| CPU | x86-64 Wine under Rosetta 2 with new-style WoW64: one `wine` loader, 64-bit prefixes, 32-bit games through a 32-bit code selector | One 64-bit Wine runs both 32- and 64-bit games, with no 32-bit macOS code. Wine 11.0 made new WoW64 the supported mode and removed the `wine64` loader | [Wine 11.0](https://github.com/wine-mirror/wine/blob/wine-11.0/ANNOUNCE.md), [signal_x86_64.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/ntdll/unix/signal_x86_64.c) |
| Synchronization | msync (Mach semaphores), on by default; the catalog's Wine runtime has it | CPU-bound FFXIV on an M2 Max: 219 FPS with msync versus 93 with Wine's server-side sync | [wine-msync](https://github.com/marzent/wine-msync) |
| Direct3D 10/11 | DXMT: Direct3D 10/11 straight to Metal, 32-bit and 64-bit | The only Metal-native path for 32-bit Direct3D 11 games. D3DMetal is 64-bit only; MoltenVK has no geometry shaders | [DXMT](https://github.com/3Shain/dxmt/wiki/Device-System-Runtime-Specifications), [MoltenVK #1815](https://github.com/KhronosGroup/MoltenVK/pull/1815) |
| Direct3D 12 | Apple D3DMetal, imported from your own copy | The only Direct3D 12 path. Apple's license limits its distribution, so Uncork never ships it. Activating it needs a Wine runtime that loads backends per process, which the catalog does not have yet ([docs/RUNTIME.md](docs/RUNTIME.md#runtime-features)) | [docs/LEGAL.md](docs/LEGAL.md#d3dmetal) |
| Backend selection | Per process, in one prefix | The game gets DXMT from `system32`/`syswow64`; Steam's web UI gets DXVK from its own directory. DXMT cannot present across processes, which Steam's web UI needs, and with software rendering every Steam window stays black | [dxmt#141](https://github.com/3Shain/dxmt/issues/141), [docs/STEAM.md](docs/STEAM.md#graphics-steam-runs-on-dxvk) |
| AVX | `ROSETTA_ADVERTISE_AVX=1` | Rosetta executes AVX/AVX2 but hides it from CPUID unless asked; some games refuse to start without it | [Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment) |
| Pixels | Retina mode off by default | With `RetinaMode` on, a game renders up to four times the pixels | [macdrv_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/winemac.drv/macdrv_main.c) |
| Overhead | Logging off (`WINEDEBUG=-all`, `DXMT_LOG_LEVEL=none`, `DXVK_LOG_LEVEL=none`, `MVK_CONFIG_LOG_LEVEL=1`); the loader is spawned directly, never through a shell | Shell wrappers such as `/bin/sh`, `env` and `nohup` are SIP-protected and drop `DYLD_*` variables | [Apple SIP](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html) |

Every knob, its default and the evidence for it are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## Quick start

You need an Apple Silicon Mac with macOS 26 or later, Rosetta 2, the Xcode Command Line Tools and Rust 1.99 or newer. The catalog's Wine runtime is built for macOS 26.0 and later (its Mach-O `minos`); `uncork doctor` itself accepts macOS 14, so it does not catch an older macOS. Uncork has only been run on macOS 27.

Install Uncork, either with Cargo:

```bash
xcode-select --install                                   # linker and SDK, once
cargo install --git https://github.com/tonianev/uncork --locked uncork
```

or from a checkout:

```bash
git clone https://github.com/tonianev/uncork.git && cd uncork
cargo install --path crates/uncork --locked
```

Then check the Mac:

```bash
uncork doctor                    # checks this Mac and says how to fix problems
softwareupdate --install-rosetta --agree-to-license      # only if doctor says Rosetta is missing
```

Then either set everything up from nothing:

```bash
uncork setup                     # installs Wine, DXMT and DXVK, creates the `steam` bottle, installs Steam
```

`uncork setup` asks once, then downloads 485.0 MB: the pinned components from their publishers (Wine 461.1 MB, DXMT 18.7 MB, DXVK 2.8 MB) and Valve's `SteamSetup.exe` (about 2.4 MB) from Valve. Steam then opens a window: sign in yourself (Uncork never sees your credentials) and let the first update finish. That first start has been reported to take 15 to 25 minutes under Rosetta; Uncork has not measured it yet ([docs/STEAM.md](docs/STEAM.md#the-first-start)). Install the game from the Steam window.

Or, if CrossOver already has a Steam bottle with your sign-in and games, clone it instead (the original is not touched):

```bash
uncork runtime install --yes     # Wine, DXMT and DXVK, 482.6 MB
uncork bottle import "$HOME/Library/Application Support/CrossOver/Bottles/Steam" --name steam
```

Then play:

```bash
uncork play rise-of-nations              # starts Steam if needed, then the game on DXMT
uncork play rise-of-nations --dry-run    # prints the backend, DLLs, INI edits and the exact command instead
uncork play rise-of-nations --hud        # with Apple's Metal performance HUD
uncork bottle kill steam                 # stops Steam and everything else in the bottle
```

macOS may ask whether the app that started Steam (your terminal) may "find devices on local networks"; that is Steam's LAN discovery, and either answer is fine for single-player.

Other useful commands: `uncork steam games` lists installed Steam games, `uncork inspect <game.exe>` says whether a program is 32- or 64-bit, which graphics API it uses and which backend Uncork would pick, and `uncork run <program.exe>` runs any Windows program in a bottle. Every command and flag is in [docs/CLI.md](docs/CLI.md).

Everything Uncork writes lives under `~/Library/Application Support/Uncork`, or under `$UNCORK_HOME` when that is set.

## How it works

```text
 uncork play rise-of-nations
   |  profile (profiles/*.toml), bottle settings (uncork.toml), installed components
   v
 uncork-core: plan the launch  ------>  LaunchPlan: one exact command, its environment,
   |  (PE scan picks the backend         the DLLs to put in place, the log file
   |   when no profile says)             (`--dry-run` prints this and stops)
   |  posix_spawn, no shell
   v
 wine (x86-64, under Rosetta 2)  ----  wineserver (one per bottle; msync on)
   |
   +-- steam.exe, steamwebhelper.exe   DXVK d3d11 from Steam/bin/cef/cef.win64 (app-local)
   |                                   -> MoltenVK -> Metal; Wine's own dxgi
   |
   +-- riseofnations.exe (32-bit)      DXMT d3d11, dxgi from syswow64 -> winemetal -> Metal
       other games                     DXMT | DXVK -> MoltenVK -> Metal | WineD3D -> OpenGL
```

The game and Steam share one prefix and one wineserver, because Steamworks finds the running client through the prefix's registry ([steam_api](https://partner.steamgames.com/doc/api/steam_api)). The backend is chosen per process so the two can use different ones: with the catalog's runtime, the game's backend DLLs are copied into the prefix and selected with per-process DLL overrides, and Steam's DXVK sits in its own directory, where Windows' search order finds it first. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the crates, the data layout, the decision matrix and the launch environment.

## Supported games

The built-in profiles live in [profiles/](profiles/). Any other Windows program can still be run with `uncork run`; Uncork then picks a backend from a scan of the executable.

| Game | Steam app | Executable | Bits / API | Backend, then fallbacks | Status |
|---|---|---|---|---|---|
| Rise of Nations: Extended Edition | 287450 | `riseofnations.exe` | 32 / D3D11, geometry shaders | DXMT, WineD3D | `untested`: main menu reached on DXMT; no match played yet |
| Age of Empires II: Definitive Edition | 813780 | `AoE2DE_s.exe` | 64 / D3D11 | DXMT, D3DMetal, DXVK, WineD3D | `untested`: not launched yet |

Status levels and how to report a result are in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md). The profile format is in [profiles/README.md](profiles/README.md).

## Related projects and credits

Uncork stands on other people's work and owes them credit. No code from the GPL, LGPL or AGPL projects below is copied into Uncork's crates.

| Project | What it is | Relationship to Uncork |
|---|---|---|
| [CrossOver](https://www.codeweavers.com/crossover) (CodeWeavers) | Commercial, supported Wine for macOS | CodeWeavers is a major Wine contributor and publishes CrossOver's Wine changes as open source ([CrossOver source](https://www.codeweavers.com/crossover/source)); DXMT's copyright line reads "Feifan He for CodeWeavers" ([LICENSE](https://github.com/3Shain/dxmt/blob/main/LICENSE)). The runtime Uncork installs and the one it plans to build both come from those sources. `uncork bottle import` clones CrossOver bottles. If you want a supported product, buy CrossOver |
| [dappermint/winecx-gptk](https://github.com/dappermint/winecx-gptk) | CrossOver 26.3's macOS changes rebased onto Wine 11.17, with msync and its fixes | The only Wine runtime in Uncork's catalog. Its build recipe informs Uncork's own build (M2) |
| [Wine](https://www.winehq.org/), [DXMT](https://github.com/3Shain/dxmt) (3Shain), [DXVK](https://github.com/doitsujin/dxvk), [MoltenVK](https://github.com/KhronosGroup/MoltenVK) | The translation layers | Uncork installs and configures them; fixes go upstream |
| [Gcenx](https://github.com/Gcenx) | macOS Wine packaging, including [DXVK-macOS](https://github.com/Gcenx/DXVK-macOS) and the Sikarugir engines | The DXVK in the catalog is Gcenx's DXVK-macOS; it renders Steam's web UI and is a game fallback |
| [Sikarugir](https://github.com/Sikarugir-App/Sikarugir) | The Wineskin successor and its Wine engines | Its per-renderer `WINEDLLPATH_*` variables are the model for Uncork's `RendererEnv` activation. Its engines are not in the catalog: they need FreeType, GnuTLS and MoltenVK from the Sikarugir app ([docs/RUNTIME.md](docs/RUNTIME.md#phase-0-pinned-upstream-builds)) |
| [frankea/Whisky](https://github.com/frankea/Whisky) | A Whisky fork whose runtime carries msync fixes | Its msync fixes are in Uncork's planned patch queue ([docs/RUNTIME.md](docs/RUNTIME.md#patch-queue)) |
| [Whisky](https://github.com/Whisky-App/Whisky) | GPL-3.0 SwiftUI Wine wrapper, archived in 2025 | Its [maintenance notice](https://github.com/Whisky-App/whisky-book/blob/main/src/maintenance-notice.md) shaped Uncork's governance and runtime policy |
| [Highball](https://github.com/gauthierpiarrette/highball) | GPL-3.0 Swift app and CLI for Windows Steam games, with the CC0 [highball-db](https://github.com/gauthierpiarrette/highball-db) | The closest project in scope. Uncork plans to exchange compatibility data with highball-db rather than fork it |

Uncork differs from these in three ways: the core is a permissively licensed (MIT OR Apache-2.0) Rust library with a complete CLI; 32-bit WoW64 titles come first; and compatibility claims are to be measured with Apple's Metal HUD and recorded with their provenance (planned for M3).

## Documentation

| Document | Contents |
|---|---|
| [docs/CLI.md](docs/CLI.md) | Every command and flag, from the binary's own help |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crates, data layout, components, bottles, backend decision matrix, launch planning, Steam flow, doctor, testing |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | Every performance knob, its default and the evidence; measured numbers; how to benchmark |
| [docs/STEAM.md](docs/STEAM.md) | Steam in a bottle: install, first run, graphics, flags, launch modes, stopping, known issues |
| [docs/RUNTIME.md](docs/RUNTIME.md) | Where the Wine runtime comes from, feature tags, component layouts, updating a pin, the patch queue |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Status levels, current results, filing a game report, adding a profile |
| [profiles/README.md](profiles/README.md) | The game profile schema |
| [docs/LEGAL.md](docs/LEGAL.md) | What Uncork downloads and never redistributes, D3DMetal, Steam, LGPL, trademarks |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Milestones M0 to M5, progress, acceptance criteria, risks including the end of Rosetta |
| [docs/adr/](docs/adr/README.md) | Architecture decision records |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup, `just ci` and the PR rules, [GOVERNANCE.md](GOVERNANCE.md) for who decides what, and [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md) for how AI tools are used here. The fastest way to help needs no Rust: test a game and file a report, or write a profile. Security problems go through [SECURITY.md](SECURITY.md). The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## License

Uncork's code, profiles and documentation are dual-licensed under MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). The components Uncork downloads (Wine, DXMT, DXVK, MoltenVK) and the ones it never distributes (D3DMetal, the Steam client) keep their own licenses; see [docs/LEGAL.md](docs/LEGAL.md) and [docs/adr/0001-license.md](docs/adr/0001-license.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
