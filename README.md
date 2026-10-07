# Uncork

Uncork runs Windows games from Steam on Apple Silicon Macs. It is an open-source alternative to CrossOver for that one job, written in Rust as a command-line tool. Uncork does not reimplement Wine or any graphics layer. It installs pinned, checksum-verified builds of Wine (x86-64, run under Rosetta 2), DXMT, DXVK on MoltenVK and, if you import your own copy, Apple's D3DMetal; it creates a Wine prefix (a "bottle") with the Windows Steam client in it; and it starts each game with the translation layer and settings that suit it. The first target is Rise of Nations: Extended Edition, a 32-bit Direct3D 11 game.

Uncork is an independent project. It is not affiliated with or endorsed by CodeWeavers, Apple, Valve or Microsoft. Product names and trademarks belong to their owners; see [docs/LEGAL.md](docs/LEGAL.md).

## Status

Pre-alpha, milestone M0. The command-line surface, the core library and the component catalog exist and are covered by unit tests that use fake Wine scripts. Nothing has yet been validated end to end with a real Wine runtime and a real game; that is milestone M1. The plan is in [docs/ROADMAP.md](docs/ROADMAP.md).

| Area | State at M0 | Validated on a real game |
|---|---|---|
| CLI: `doctor`, `setup`, `play`, `run`, `inspect`, `runtime`, `bottle`, `steam`, `profile`, `winetricks` | Implemented, unit-tested | No (M1) |
| Pinned component catalog: download, SHA-256 verification, safe unpacking | Implemented | No (M1) |
| D3DMetal import from your own Game Porting Toolkit download | Implemented | No (M1) |
| Graphics backend choice from a PE scan of the game | Implemented | No (M1) |
| Per-process backend activation (three strategies, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)) | Implemented | No (M1) |
| Windows Steam client: install, start, direct and `-applaunch` game launch | Implemented | No (M1) |
| Game profiles | Two built-in, both `untested` | No |
| macOS Game Mode wrapper bundles | Experimental, off by default | No |
| Wine runtime built by Uncork's own CI from CrossOver 26.3 sources | Planned (M2) | |
| `uncork bench`, compatibility database and reports | Planned (M3) | |
| SwiftUI app over the Rust core | Planned (M4) | |
| Post-Rosetta ARM64 path | Planned (M5) | |

## Why

Performance comes from the stack Uncork assembles, not from the orchestrator. The orchestrator's job is to pick the right piece for each game and get out of the way.

| Layer | What Uncork uses | Why | Source |
|---|---|---|---|
| CPU | x86-64 Wine under Rosetta 2 with new-style WoW64: one `wine` loader, 64-bit prefixes, 32-bit games through a 32-bit code selector | One 64-bit Wine runs both 32- and 64-bit games, with no 32-bit macOS code. Wine 11.0 made new WoW64 the supported mode and removed the `wine64` loader | [Wine 11.0](https://github.com/wine-mirror/wine/blob/wine-11.0/ANNOUNCE.md), [signal_x86_64.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/ntdll/unix/signal_x86_64.c) |
| Synchronization | msync (Mach semaphores) on runtimes that have it | CPU-bound FFXIV on an M2 Max: 219 FPS with msync versus 93 with Wine's server-side sync | [wine-msync](https://github.com/marzent/wine-msync) |
| Direct3D 10/11 | DXMT: Direct3D 10/11 straight to Metal, 32-bit and 64-bit | The only Metal-native path for 32-bit Direct3D 11 games. D3DMetal is 64-bit only; MoltenVK has no geometry shaders | [DXMT](https://github.com/3Shain/dxmt/wiki/Device-System-Runtime-Specifications), [MoltenVK #1815](https://github.com/KhronosGroup/MoltenVK/pull/1815) |
| Direct3D 12 | Apple D3DMetal, imported from your own copy | The only Direct3D 12 path. Apple's license limits its distribution, so Uncork never ships it | [docs/LEGAL.md](docs/LEGAL.md#d3dmetal) |
| Backend selection | Per process, in one prefix | The Steam client stays on WineD3D while the game uses DXMT. DXMT cannot present across processes, which Steam's web UI needs | [dxmt#141](https://github.com/3Shain/dxmt/issues/141) |
| AVX | `ROSETTA_ADVERTISE_AVX=1` | Rosetta executes AVX/AVX2 but hides it from CPUID unless asked; some games refuse to start without it | [Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment) |
| Pixels | Retina mode off by default | With `RetinaMode` on, a game renders up to four times the pixels | [macdrv_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/winemac.drv/macdrv_main.c) |
| Overhead | Logging off (`WINEDEBUG=-all`, `DXMT_LOG_LEVEL=none`, `DXVK_LOG_LEVEL=none`); the loader is spawned directly, never through a shell | Shell wrappers such as `/bin/sh`, `env` and `nohup` are SIP-protected and drop `DYLD_*` variables | [Apple SIP](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html) |

Every knob, its default and the evidence for it are in [docs/PERFORMANCE.md](docs/PERFORMANCE.md).

## Quick start

You need an Apple Silicon Mac with macOS 14 or later (DXMT recommends 15 or later; development happens on macOS 27), Rosetta 2, the Xcode Command Line Tools and Rust 1.99 or newer.

```bash
xcode-select --install                                   # linker and SDK, once
softwareupdate --install-rosetta --agree-to-license      # if `uncork doctor` says Rosetta is missing
cargo install --locked --git https://github.com/tonianev/uncork uncork

uncork doctor                    # checks this Mac and says how to fix problems
uncork setup                     # installs Wine, DXMT and DXVK, creates the `steam` bottle, installs Steam
```

`uncork setup` downloads about 189 MB of pinned components from their publishers, then Valve's `SteamSetup.exe` from Valve. Steam then opens a window: sign in yourself (Uncork never sees your credentials) and let the first update finish. Under Rosetta the first start takes 15 to 25 minutes; see [docs/STEAM.md](docs/STEAM.md). Install the game from the Steam window, then:

```bash
uncork play rise-of-nations              # starts Steam if needed, then the game on DXMT
uncork play rise-of-nations --dry-run    # prints the exact command, environment and DLLs instead
uncork play rise-of-nations --hud        # with Apple's Metal performance HUD
```

Other useful commands: `uncork steam games` lists installed Steam games, `uncork inspect <game.exe>` says whether a program is 32- or 64-bit, which graphics API it uses and which backend Uncork would pick, and `uncork run <program.exe>` runs any Windows program in a bottle. To use D3DMetal for 64-bit Direct3D 12 games, download Apple's Game Porting Toolkit from [Apple Developer](https://developer.apple.com/games/game-porting-toolkit/), mount it, and run `uncork runtime import-gptk <mounted volume>`.

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
 wine (x86-64, under Rosetta 2)  ----  wineserver (one per bottle; msync when available)
   |
   +-- steam.exe, steamwebhelper.exe   WineD3D (Wine builtin) -> OpenGL -> Metal
   |                                   web UI rendered in software
   |
   +-- riseofnations.exe (32-bit)      d3d11.dll from DXMT -> Metal
       other games                     D3DMetal (64-bit, imported) | DXVK -> MoltenVK -> Metal | WineD3D
```

The game and Steam share one prefix and one wineserver, because Steamworks finds the running client through the prefix's registry ([steam_api](https://partner.steamgames.com/doc/api/steam_api)). The backend is chosen per process so the two can use different ones. [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) explains the crates, the data layout, the decision matrix and the launch environment.

## Supported games

The built-in profiles live in [profiles/](profiles/). Any other Windows program can still be run with `uncork run`; Uncork then picks a backend from a scan of the executable.

| Game | Steam app | Executable | Bits / API | Backend, then fallbacks | Status |
|---|---|---|---|---|---|
| Rise of Nations: Extended Edition | 287450 | `riseofnations.exe` | 32 / D3D11, geometry shaders | DXMT, WineD3D | untested |
| Age of Empires II: Definitive Edition | 813780 | `AoE2DE_s.exe` | 64 / D3D11 | DXMT, D3DMetal, DXVK, WineD3D | untested |

Status levels and how to report a result are in [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md). The profile format is in [profiles/README.md](profiles/README.md).

## Related projects

Uncork stands on other people's work and owes them credit. No code from the GPL, LGPL or AGPL projects below is copied into Uncork's crates.

| Project | What it is | Relationship to Uncork |
|---|---|---|
| [CrossOver](https://www.codeweavers.com/crossover) (CodeWeavers) | Commercial, supported Wine for macOS | CodeWeavers is a major Wine contributor, and DXMT's copyright line reads "Feifan He for CodeWeavers" ([LICENSE](https://github.com/3Shain/dxmt/blob/main/LICENSE)). Uncork's planned runtime builds from CrossOver's published LGPL Wine sources. If you want a supported product, buy CrossOver |
| [Wine](https://www.winehq.org/), [DXMT](https://github.com/3Shain/dxmt), [DXVK](https://github.com/doitsujin/dxvk), [MoltenVK](https://github.com/KhronosGroup/MoltenVK) | The translation layers | Uncork installs and configures them; fixes go upstream |
| [Gcenx](https://github.com/Gcenx) and [Sikarugir](https://github.com/Sikarugir-App/Sikarugir) | macOS Wine packaging, the Wineskin successor and its engines | Uncork's recommended phase-0 Wine runtime is a Sikarugir engine; the DXVK build is Gcenx's DXVK-macOS |
| [Whisky](https://github.com/Whisky-App/Whisky) | GPL-3.0 SwiftUI Wine wrapper, archived in 2025 | Its [maintenance notice](https://github.com/Whisky-App/whisky-book/blob/main/src/maintenance-notice.md) shaped Uncork's governance and runtime policy |
| [Highball](https://github.com/gauthierpiarrette/highball) | GPL-3.0 Swift app and CLI for Windows Steam games, with the CC0 [highball-db](https://github.com/gauthierpiarrette/highball-db) | The closest project in scope. Uncork plans to exchange compatibility data with highball-db rather than fork it |
| [dappermint/winecx-gptk](https://github.com/dappermint/winecx-gptk), [frankea/Whisky](https://github.com/frankea/Whisky) | CrossOver-derived Wine runtimes with msync fixes | The catalog pins a winecx-gptk runtime; their build recipe and fixes inform Uncork's own build (M2) |

Uncork differs from these in three ways: the core is a permissively licensed (MIT OR Apache-2.0) Rust library with a complete CLI; 32-bit WoW64 titles come first; and compatibility claims are to be measured with Apple's Metal HUD and recorded with their provenance (planned for M3).

## Documentation

| Document | Contents |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Crates, data layout, components, bottles, backend decision matrix, launch planning, Steam flow, doctor, testing |
| [docs/PERFORMANCE.md](docs/PERFORMANCE.md) | Every performance knob, its default and the evidence; how to benchmark |
| [docs/STEAM.md](docs/STEAM.md) | Steam in a bottle: install, first run, flags, launch modes, known issues |
| [docs/RUNTIME.md](docs/RUNTIME.md) | Where the Wine runtime comes from, feature tags, component layouts, updating a pin, the patch queue |
| [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) | Status levels, filing a game report, adding a profile |
| [profiles/README.md](profiles/README.md) | The game profile schema |
| [docs/LEGAL.md](docs/LEGAL.md) | What Uncork downloads and never redistributes, D3DMetal, Steam, LGPL, trademarks |
| [docs/ROADMAP.md](docs/ROADMAP.md) | Milestones M0 to M5, acceptance criteria, risks including the end of Rosetta |
| [docs/adr/](docs/adr/README.md) | Architecture decision records |

## Contributing

Read [CONTRIBUTING.md](CONTRIBUTING.md) for setup, `just ci` and the PR rules, [GOVERNANCE.md](GOVERNANCE.md) for who decides what, and [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md) for how AI tools are used here. The fastest way to help needs no Rust: test a game and file a report, or write a profile. Security problems go through [SECURITY.md](SECURITY.md). The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## License

Uncork's code, profiles and documentation are dual-licensed under MIT OR Apache-2.0, at your option. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). The components Uncork downloads (Wine, DXMT, DXVK, MoltenVK) and the ones it never distributes (D3DMetal, the Steam client) keep their own licenses; see [docs/LEGAL.md](docs/LEGAL.md) and [docs/adr/0001-license.md](docs/adr/0001-license.md).

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
