# Roadmap

This document is the plan of record for Uncork from the scaffold to a native Mac app and a path beyond Rosetta. It lists each milestone in order with its deliverables and its acceptance criteria as a checklist, records the risks the plan manages (the end of general-purpose Rosetta first among them), and lists the open questions that only hands-on testing can answer. Scope changes go through this file and an ADR in [adr/](adr/README.md); the project lead decides them ([GOVERNANCE.md](../GOVERNANCE.md)).

A milestone is closed when every box in its acceptance list is ticked in a merged PR that links the evidence (a CI run, a compatibility report, a benchmark file). Boxes that need a real Mac and a real game are ticked only with a first-hand report.

| Milestone | Title | Depends on |
|---|---|---|
| [M0](#m0-scaffold-and-contracts) | Scaffold and contracts | none |
| [M1](#m1-end-to-end-on-rise-of-nations-with-a-pinned-runtime) | End to end on Rise of Nations with a pinned runtime | M0 |
| [M2](#m2-uncorks-own-wine-runtime) | Uncork's own Wine runtime | M1 |
| [M3](#m3-compatibility-database-reports-and-benchmarks) | Compatibility database, reports and benchmarks | M1 |
| [M4](#m4-swiftui-app-over-uniffi) | SwiftUI app over UniFFI | M1 |
| [M5](#m5-rosetta-sunset-plan-and-arm64-path) | Rosetta sunset plan and ARM64 path | M2 |

M2, M3 and M4 can proceed in parallel once M1 closes. M5 has a hard external deadline (see [The end of Rosetta](#the-end-of-rosetta)) and its research starts at once.

## M0: Scaffold and contracts

The repository, the crate boundaries and every public contract, written as documented signatures, then implemented against those docs with unit tests that use fakes. This milestone.

Deliverables:

- Workspace with `uncork`, `uncork-core`, `uncork-pe` and `uncork-steam`, toolchain pinned to Rust 1.99.0, workspace lints, `deny.toml`.
- Every public type and function documented; the doc comments are the specification.
- `runtime/catalog.toml` with pinned Wine, DXMT and DXVK entries; profiles for Rise of Nations: Extended Edition and Age of Empires II: Definitive Edition.
- Governance documents and this documentation set.

Acceptance:

- [ ] `just ci` passes locally and in CI: format, clippy with `-D warnings`, tests, doctests, rustdoc with `-D warnings`, `cargo machete`, `typos`, `cargo deny check`.
- [ ] `grep -rn 'todo!' crates/` finds nothing.
- [ ] Every built-in catalog entry and profile parses and validates in unit tests.
- [ ] Tests are hermetic: no network, no Wine, no real `~/Library`; data roots are temporary directories and fake `wine`/`wineserver` scripts stand in for Wine.
- [ ] Every relative link in the documentation resolves.

## M1: End to end on Rise of Nations with a pinned runtime

The CLI does its job on a real Mac with the phase-0 runtime: from nothing to Rise of Nations: Extended Edition on DXMT.

Deliverables:

- Fixes to whatever the first real runs break: catalog pins (`strip_prefix`, layouts), command details, timeouts.
- First-hand compatibility reports for Rise of Nations: Extended Edition, recorded in its profile.
- Answers to the open questions marked M1 below, written into [STEAM.md](STEAM.md) and [PERFORMANCE.md](PERFORMANCE.md).

Acceptance, on a clean Apple Silicon Mac running macOS 27 with a fresh `UNCORK_HOME`:

- [ ] `uncork doctor` reports no failures after following its own fixes.
- [ ] `uncork setup --yes` installs the recommended components, creates the `steam` bottle and installs Steam; you sign in in Steam's window; the client reaches its library view and stays usable for a 2-hour session.
- [ ] `uncork play rise-of-nations --dry-run` shows DXMT and the profile's environment, overrides and INI edit.
- [ ] `uncork play rise-of-nations` from a cold start starts Steam and the game, and the game reaches its main menu on DXMT.
- [ ] An 8-AI skirmish runs for 30 minutes or more without a freeze.
- [ ] A "Learn to Play" tutorial gets past "Setting up animals" (a hang reported under Proton, [ProtonDB](https://www.protondb.com/app/287450)).
- [ ] Music and sound effects play and the volume slider works.
- [ ] The multiplayer lobby browser loads (PlayFab over WinHTTP and TLS).
- [ ] Alt-tab out of and back into the game works.
- [ ] A Metal HUD benchmark following [PERFORMANCE.md](PERFORMANCE.md#how-to-benchmark) is attached, on DXMT and on `--backend wined3d`.
- [ ] The profile's `[compat]` status is set from those first-hand reports.

## M2: Uncork's own Wine runtime

A Wine runtime built by Uncork's CI from CrossOver 26.3's LGPL sources plus the patch queue, so one runtime has msync, large-address-aware support and per-process backend activation. Details in [RUNTIME.md](RUNTIME.md#phase-1-uncorks-own-ci-build).

Acceptance:

- [ ] `runtime/build-wine.sh --crossover-version 26.3.0` builds on GitHub's hosted arm64 `macos-26` runner from the SHA-256-pinned source tarball and writes `uncork-wine-<version>-x86_64.tar.xz` and `SOURCES.json`.
- [ ] Patches 1 to 4 of the [patch queue](RUNTIME.md#patch-queue) are applied, each with a test or a recorded manual check.
- [ ] Every CI gate in [RUNTIME.md](RUNTIME.md#build-recipe) passes, including the non-empty i386 half and `NX_COMPAT` on every shipped PE file.
- [ ] Each release publishes the binary archive, `SOURCES.json`, the source tarballs, the patches and the build scripts together.
- [ ] DXMT is built from `main` at or after [46911d7345](https://github.com/3Shain/dxmt/commit/46911d7345), with its source published the same way.
- [ ] The runtime is pinned in the catalog with the features `wow64`, `msync`, `dxmt`, `d3dmetal`, `dllpath-prepend` and `large-address-aware`.
- [ ] It becomes the recommended runtime only after an A/B on Rise of Nations against the phase-0 runtime shows no regression in median FPS or 99th-percentile frame time beyond run-to-run spread, and the Steam UI works under msync for a 2-hour session.

## M3: Compatibility database, reports and benchmarks

Repeatable measurements and a steady flow of first-hand reports.

Acceptance:

- [ ] `uncork bench <game>` runs the game with HUD logging, parses the `metal-HUD:` lines into frame-time percentiles, and stores them as JSON with provenance: Uncork commit, runtime and backend versions, macOS build, chip, game build id.
- [ ] The "Game report" issue template and the path from a report to a `[[compat.reports]]` entry are documented in [COMPATIBILITY.md](COMPATIBILITY.md) and have been used for at least ten reports.
- [ ] Profiles exist, each with at least one first-hand report, for Age of Mythology: Extended Edition (266840), Age of Empires II: Definitive Edition (813780), Age of Empires III: Definitive Edition (933110), Stronghold Crusader: Definitive Edition (3024040), Age of Mythology: Retold (1934680, exercises D3DMetal and Direct3D 12) and Civilization V (8930, exercises WineD3D).
- [ ] An ADR decides the license of compatibility data and how it is exchanged with the CC0 [highball-db](https://github.com/gauthierpiarrette/highball-db).
- [ ] Launch logs are scanned for known failure strings (`disabling no-exec because of`, `cannot find the FreeType font library`) and the plan's warnings name them.

## M4: SwiftUI app over UniFFI

A native app for people who do not use a terminal, on the same core.

Acceptance:

- [ ] A new `uncork-ffi` crate exposes the core through UniFFI pinned to an exact version; a Swift package is generated from it.
- [ ] The app covers setup, the game list, play, `doctor` and bottle settings. Every action it offers is also available in the CLI.
- [ ] The Rust workspace still builds and passes `just ci` with only the Command Line Tools; Xcode is needed only for the app.
- [ ] The app ships as a Developer ID signed and notarized package (the G2 certificate; the original Developer ID sub-CA stops working on 2027-02-01, [Apple](https://developer.apple.com/news/?id=w4atic4c)).

## M5: Rosetta sunset plan and ARM64 path

Keep Uncork working after general-purpose Rosetta ends.

Acceptance:

- [ ] A `CpuBackend` abstraction in `uncork-core`, with `RosettaX86_64` as today's implementation; bottles record the backend that created them.
- [ ] Every macOS 27.x update and every macOS 28 beta is tested with Apple's `game-test-tool` and a 32-bit PE smoke test, with results recorded here.
- [ ] A CI Mac booted with `nox86exec=1`, which makes anything that would run under Rosetta fail ([macOS 26 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-26-release-notes)), runs the ARM64 path's tests.
- [ ] An experimental ARM64 Wine with an x86 emulator (FEX) runs a 64-bit Direct3D 11 game on DXMT, or the blocker is documented.
- [ ] An ADR records the post-Rosetta decision before macOS 28 ships.

## The end of Rosetta

Uncork's whole stack is x86-64 code under Rosetta 2. Apple has announced the end of Rosetta as a general-purpose tool.

| When | What Apple says | Source |
|---|---|---|
| macOS 27 (2026) | The last release in which Rosetta is a general-purpose tool for Intel apps | [About the Rosetta translation environment](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment), [Apple Support 102527](https://support.apple.com/en-us/102527) |
| macOS 27.0 | Upgrading does not restore Rosetta; it must be installed again | [macOS 27 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-27-release-notes) |
| macOS 27 betas | A beta-only `game-test-tool` previews the later behavior; non-game processes may crash under it | [macOS 27 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-27-release-notes) |
| macOS 28 (expected autumn 2027, by Apple's annual cycle) | Only a subset remains, aimed at "older unmaintained gaming titles" that rely on Intel-based frameworks; other Intel software stops being compatible | [Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment), [Apple Support 102527](https://support.apple.com/en-us/102527) |

What this means for Uncork:

- `wineserver`, `explorer.exe` and Steam's web helpers are not games. Whether the macOS 28 subset runs them, and whether 32-bit code selectors set up with `i386_set_ldt` survive, is unknown.
- CodeWeavers' answer is native ARM64 CrossOver with its own port of FEX, in preview since 2026-07-31 for macOS 26.5 and later, without D3DMetal ([CodeWeavers](https://www.codeweavers.com/blog/mjohnson/2026/7/31/crossover-preview-the-right-to-bear-arm64-on-mac)).
- Native ARM64 Wine on macOS needs low address space that arm64 processes do not get by default. Third-party reports say an Apple entitlement delivered with a provisioning profile is required; Apple does not document it ([wine-devel](https://list.winehq.org/hyperkitty/list/wine-devel@list.winehq.org/thread/CKG5CEN2BE5VRXZ7O7NX4YUSBH3247WH/)). The project needs a paid Apple Developer membership and an early request to Apple.
- D3DMetal's Wine glue is x86-64 only. Treat D3DMetal as unavailable on the ARM64 path until Apple says otherwise.
- `doctor` reports `rosetta-sunset` on macOS 27 and later so users know.

## Risks

| Risk | Facts | Mitigation |
|---|---|---|
| End of Rosetta | Above | M5; `CpuBackend` abstraction; test every beta with `game-test-tool` |
| ARM64 path gated by Apple | An undocumented entitlement; which developer accounts can get it is unclear | Paid membership, early request, track DXMT's ARM64 work and other ARM64 Wine efforts |
| Upstream pins disappear or change | The Sikarugir engines are published in one rolling release; `winecx-gptk` has no LICENSE file | Pins fail closed on a hash change; M2 removes the dependency on third-party Wine builds |
| x86-64 dependency supply ends | Homebrew Intel at Tier 3 since September 2026; Nixpkgs 26.05 is the last with x86_64-darwin ([Homebrew](https://brew.sh/2025/11/12/homebrew-5.0.0), [NixOS](https://nixos.org/blog/announcements/2026/nixos-2605/)) | A small pinned from-source x86-64 dependency build in M2 |
| Steam client changes | Wine bug 60334, first-start crashes after updates | Track the issues in [STEAM.md](STEAM.md#known-issues); restart Steam between sessions |
| D3DMetal license | Personal evaluation use, non-commercial distribution only | Import only; never distributed ([LEGAL.md](LEGAL.md#d3dmetal)) |
| Single maintainer | Whisky was archived when its maintainer stepped away | [GOVERNANCE.md](../GOVERNANCE.md): a second maintainer within three months of v0.2.0 |
| Name | A Madrid Protocol filing for "UNCORK" in class 9 has unknown status | Formal clearance before a 1.0 release |

## Open questions

Each is answered by a hands-on test and recorded in the document named.

| # | Question | Milestone | Answer goes into |
|---|---|---|---|
| 1 | Does Steam's web UI render on the phase-0 runtime with software CEF, and does it survive `WINEMSYNC=1`? | M1 | [STEAM.md](STEAM.md) |
| 2 | Does a direct launch of `riseofnations.exe` work end to end (Steamworks, lobbies, playtime recorded)? | M1 | [STEAM.md](STEAM.md), the profile |
| 3 | Does DXMT render all of Rise of Nations' geometry-shader permutations, and how does its frame rate compare with WineD3D? | M1 | the profile, [PERFORMANCE.md](PERFORMANCE.md) |
| 4 | Does Rise of Nations load a non-`NX_COMPAT` DLL at run time, and does that collapse performance on macOS 27? | M1 | [PERFORMANCE.md](PERFORMANCE.md#dep-and-nx_compat) |
| 5 | Is Wine's builtin DirectMusic enough for the game's audio? | M1 | the profile |
| 6 | What causes the "Setting up animals" tutorial hang: the C runtime, the UCRT or something else? | M1 | the profile |
| 7 | Do the WMV intros and the menu background video play with GStreamer and gst-libav? | M1 | the profile |
| 8 | Does Rise of Nations exceed 2 GiB without large-address-awareness? | M1 | [PERFORMANCE.md](PERFORMANCE.md#large-address-aware) |
| 9 | Does a games-category wrapper that `exec`s Wine get Game Mode, and with which `CaptureDisplaysForFullscreen` setting? | M1 | [PERFORMANCE.md](PERFORMANCE.md#game-mode) |
| 10 | In `applaunch` mode, do Steam's own processes still render with a game backend in their environment? | M1 | [STEAM.md](STEAM.md#launch-modes) |
| 11 | Do GitHub's hosted arm64 runners expose a Metal device usable for DXMT smoke tests? | M2 | [RUNTIME.md](RUNTIME.md) |
| 12 | Under `game-test-tool` and macOS 28 betas, do `wineserver`, `explorer.exe` and `steamwebhelper.exe` run, and do 32-bit code segments survive? | M5 | this file |
| 13 | Is the ARM64 cross-architecture capability available to open-source projects distributing with Developer ID? | M5 | this file |
