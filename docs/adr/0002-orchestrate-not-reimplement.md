# 0002: Orchestrate Wine and the translation layers, do not reimplement them

Status: Accepted
Date: 2026-10-07

This record decides what Uncork is: a Rust program that selects, installs, configures and launches existing translation layers, not a new translation layer. It also records why the orchestrator is written in Rust and why the choice costs no frame rate.

## Context

Running a Windows game on an Apple Silicon Mac takes four translations: x86 machine code to ARM (Rosetta 2), the Windows API to macOS (Wine), Direct3D to Metal (DXMT, D3DMetal, or DXVK on MoltenVK) and, for Steam games, a Windows Steam client in the same environment. Each of these is a large, specialized project:

- Wine has been developed since 1993. Wine 11.0 (January 2026) completed new-style WoW64, which runs 32-bit programs from a 64-bit build ([ANNOUNCE](https://github.com/wine-mirror/wine/blob/wine-11.0/ANNOUNCE.md)).
- DXMT translates Direct3D 10 and 11 to Metal for 32- and 64-bit games and is actively developed; its copyright line reads "Feifan He for CodeWeavers" ([3Shain/dxmt](https://github.com/3Shain/dxmt), [LICENSE](https://github.com/3Shain/dxmt/blob/main/LICENSE)).
- D3DMetal is Apple's own and is closed.
- msync, the fast synchronization primitive for macOS, exists only as a Wine patch set ([wine-msync](https://github.com/marzent/wine-msync)).

What these projects do not provide is the glue: choosing the right Wine build and backend per game, verifying downloads, creating prefixes with known settings, keeping the Steam client on a backend that works while a game uses another, and launching without the environment mistakes that make "works on my Mac" bugs. Every existing macOS project (CrossOver, Whisky, Sikarugir, Highball) is such glue, and the ones that faded did so for maintenance reasons, not because the approach failed ([Whisky's maintenance notice](https://github.com/Whisky-App/whisky-book/blob/main/src/maintenance-notice.md)).

Performance is set by the translation layers and their configuration, not by the launcher. The large wins are configuration: msync over server-side synchronization (219 versus 93 FPS on one CPU-bound benchmark, [wine-msync](https://github.com/marzent/wine-msync)), DXMT rather than an OpenGL path for Direct3D 11, not disabling DEP for a 32-bit process under Rosetta ([athei/wine-build#3](https://github.com/athei/wine-build/issues/3)), Retina off, logging off. An orchestrator that gets those right and then gets out of the way delivers the stack's full speed.

## Decision

1. Uncork does not translate instructions, system calls or graphics calls. It pins, installs, configures and launches Wine, DXMT, DXVK on MoltenVK and the user's D3DMetal. Fixes to those layers go upstream; where a fix cannot wait, it is a small patch in Uncork's runtime build ([RUNTIME.md](../RUNTIME.md#patch-queue)), sent upstream too.
2. The orchestrator is a Rust workspace: a library (`uncork-core`, `uncork-pe`, `uncork-steam`) and a complete CLI (`uncork`). Any GUI is a thin layer over the library ([ROADMAP.md](../ROADMAP.md#m4-swiftui-app-over-uniffi)).
3. The orchestrator adds no work to a running game. It plans the launch as data, spawns the Wine loader directly with `posix_spawn`, and then only waits or exits. No shell, no wrapper process, no hook, no logging unless asked.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Write a new Direct3D-to-Metal layer | DXMT already does this for Direct3D 10/11 and is maintained; a second one would split scarce effort and lag it for years |
| Fork CrossOver | Its game database and per-game backend logic are proprietary; only the Wine tree is LGPL. Uncork builds from that tree instead ([0004](0004-runtime-supply.md)) |
| A Swift app, as Whisky and Highball are | Ties the core to Xcode and to macOS for every contributor and every CI job; no headless CLI for scripting and testing; GPL in both precedents |
| Shell scripts | Untestable; and `/bin/sh` is SIP-protected, so `DYLD_*` variables are stripped from every program it starts ([Apple](https://developer.apple.com/library/archive/documentation/Security/Conceptual/System_Integrity_Protection_Guide/RuntimeProtections/RuntimeProtections.html)) |
| Python or another interpreted language | A runtime to install and pin on every Mac; weaker typing for the plan data that must be exactly right |

## Consequences

- Uncork's speed and compatibility depend on upstream projects. The catalog pins exact builds, so an upstream change reaches users only through a reviewed pin update.
- Uncork's own code stays small, and much of its value is data: catalog pins, profiles and compatibility reports that need no Rust to contribute.
- Rust gives one static binary installable with `cargo install`, `Result`-based errors that carry context, launch plans that are plain typed data (unit-testable, printable with `--dry-run`), and a path to Swift through UniFFI.
- Every fix Uncork depends on is sent upstream rather than carried forever, and Uncork links to the projects it relies on. Whisky's maintenance notice explains why a wrapper that only takes from those projects harms them.
