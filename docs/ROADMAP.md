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

The repository, the crate boundaries and every public contract, written as documented signatures, then implemented against those docs with unit tests that use fakes. Implemented; it closes when the boxes below are ticked with evidence. On 2026-10-07, after the fixes through commit 7dcd07a, `just ci` passed every step locally on the development Mac (892 tests), `grep -rn 'todo!' crates/` found nothing, and every relative link in the documentation resolved; no hosted CI run is recorded yet.

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

The CLI does its job on a real Mac with the phase-0 runtime: from nothing to Rise of Nations: Extended Edition on DXMT. In progress.

Deliverables:

- Fixes to whatever the first real runs break: catalog pins (`strip_prefix`, layouts), command details, timeouts.
- First-hand compatibility reports for Rise of Nations: Extended Edition, recorded in its profile.
- Answers to the open questions marked M1 below, written into [STEAM.md](STEAM.md) and [PERFORMANCE.md](PERFORMANCE.md).

### Progress (2026-10-07 and 2026-10-08)

On the development Mac (MacBook Pro, M5 Max, macOS 27.0.1, Rosetta installed), in three sessions: the first with a Steam bottle imported from CrossOver, the second with a new bottle and a fresh Steam install, then a recheck with a release build that has the fixes listed below; the third, on 2026-10-08, on Rise of Nations' full screen on the built-in display. Details and numbers are in the [README](../README.md#verified-on).

| Done | Evidence |
|---|---|
| The CLI launches Rise of Nations end to end: `uncork play rise-of-nations --hud` started Steam silently, then the game on DXMT 0.80, in about 44 s | `lsof`: DXMT's `d3d11.dll` and `dxgi.dll` from `syswow64`, builtin `winemetal`, Apple's AGX driver; the profile's INI edit applied |
| The main menu renders on DXMT: 120 FPS at the display's cap, GPU time 0.48 ms per frame (by hand, same DLLs and overrides) | Metal HUD |
| Steam's UI works next to the game: client chrome, sign-in and library on app-local DXVK, with msync on | Screen; `lsof` of Steam's GPU process |
| `--dry-run` shows DXMT, the profile's environment, overrides and INI edit | [ARCHITECTURE.md](ARCHITECTURE.md#activation-strategies) |
| `doctor`, `runtime install` (from verified archives), `bottle import`, `bottle create`, `run` (32-bit), `steam games`, `bottle kill` | [README](../README.md#verified-on) |
| A fresh Steam install: `bottle create` then `steam install` downloaded Valve's installer (it matched the pinned SHA-256), installed `Steam.exe`, and the client's first start downloaded 336 MB and showed the full sign-in window on app-local DXVK within about a minute | Screen; [STEAM.md](STEAM.md#the-first-start) |
| After the fixes: `doctor` reports DXVK ok; `inspect` reports Rise of Nations' facts and DXMT; `play rise-of-nations --dry-run` sets `SteamAppId` and `SteamGameId`; a command right after `bottle create` works; `run` of `syswow64\cmd.exe` no longer warns about Direct3D 12 | [README](../README.md#verified-on) |
| Game Mode bundles start the game through LaunchServices, but Rise of Nations crashed at startup in 3 of 5 bundle launches; Game Mode stays off and is not recommended | [PERFORMANCE.md](PERFORMANCE.md#game-mode) |
| 2026-10-08: the cropped, offset full screen explained. The imported bottle had `RetinaMode` `n` next to CrossOver's 192 DPI, so the game saw a half-size screen. With a consistent pair (Retina on with 192 DPI, or off with 96) and a fresh Wine session, Rise of Nations ran as a borderless 1728x1117 window at (0,0) on the built-in display: 8 of 8 launches without a crash, 3 of them through Game Mode bundles, about 113 to 118 FPS at the main menu | [PERFORMANCE.md](PERFORMANCE.md#retina-mode-and-dpi) |
| 2026-10-08: Wine reads Retina mode, the DPI and the displays only when its wineserver starts; changing them while Steam ran gave stale window sizes and crashes until the wineserver restarted. DXMT runs uncapped after a game leaves exclusive full screen unless `DXMT_CONFIG` caps it | [ARCHITECTURE.md](ARCHITECTURE.md#the-main-display), [PERFORMANCE.md](PERFORMANCE.md#frame-cap) |

Changes made during the first session, now in `main`:

- The catalog's Wine runtime is `winecx-gptk-4.7.3` alone. The Sikarugir engine it used to recommend needs libraries from the Sikarugir app ([RUNTIME.md](RUNTIME.md#phase-0-pinned-upstream-builds)).
- Install moves a nested Wine tree (`Libraries/Wine`) to the top of the component directory.
- Steam's web UI runs on app-local DXVK instead of software CEF, which left every window black ([ADR 0005](adr/0005-steam-cef-on-app-local-dxvk.md)).
- Stopping Steam falls back from `-shutdown`, which did not stop the client within 60 s, to `wineserver --kill`, and the client's `ActiveProcess` `pid` is reset after a kill.
- Imported CrossOver bottles run with `USER=crossover` and `LOGNAME=crossover`, so Steam's sign-in and the games' settings are found.
- `MVK_CONFIG_LOG_LEVEL=1` keeps MoltenVK's device banner out of every log.
- Game Mode bundles are wired into launches (`--game-mode`, `performance.game_mode`), still experimental.

Changes made during and after the second session, now in `main` (commit b8f7255, then 4a6baec to 7dcd07a):

- Every Wine command in a bottle, `wineboot`, `regedit` and `wineserver` included, runs with the bottle's environment (`launch::in_bottle`). Before, the first command after `bottle create` exited with status 1 and no output, because the wineserver `create` left had no msync.
- `Steam.exe`, as Valve's installer names it, is found ignoring letter case, and a cached `SteamSetup.exe` with the pinned hash is reused instead of downloaded again.
- A Steam `pid` found while no wineserver runs (`wineserver -k0`) is stale and is cleared, so a crashed or force-quit client no longer makes `play` skip starting Steam. `reg` exiting with status 1 counts as "no key" only with its own `reg:` message. `bottle kill` clears the `pid` also when nothing was running, and `bottle import` clears one the source left.
- A bottle whose `create` failed is no longer taken for a ready one; `setup` and `steam install` finish it. `bottle set` updates the registry before it saves `uncork.toml`, so a failed change can be retried.
- `direct` mode sets `SteamAppId` and `SteamGameId`; `applaunch` gives a running client 15 s to obey `-shutdown` before `wineserver --kill`; `play --wait` no longer waits for the wineserver Steam keeps alive; every start of the client uses the bottle's DXVK pin.
- A backend counts as available only when the runtime can activate it, so automatic choice never picks D3DMetal on the catalog's runtime; `inspect` lists why it passed over the others.
- Plan warnings are printed on real launches; several matching profiles are an error that names them; `--json` shows command arguments as strings; Ctrl-D at a download prompt means no; `doctor` warns when DXVK is missing; Windows system directories are not scanned as a program's own DLLs, and the large-address-aware hint is limited to programs with a graphics API.

Changes made after the third session (2026-10-08), unit-tested with fakes and not yet run on real hardware:

- Retina mode and the DPI are written as a pair (96 without Retina mode, 192 with it); `bottle set` writes them with the bottle stopped; `bottle import` keeps a CrossOver bottle's Retina choice; `doctor` reports a bottle whose pair disagrees (`bottle-dpi`).
- Launches read the main display with `system_profiler`. Profile INI values can use `{display.width}`, `{display.height}` and `{display.refresh}`, and the Rise of Nations profile runs the game as a borderless window at the display's "looks like" size (`Fullscreen=2`).
- DXMT launches are capped at the main display's refresh rate (`performance.max_fps` overrides it); MetalFX stays off on DXMT in a Retina bottle.
- A running bottle whose main display has changed since it started is restarted before `play` and `run`, once the launch is planned and, on a terminal, after asking.
- After a launch Uncork says where to find a window that opened behind the terminal.

Left for M1:

- A fresh `uncork setup --yes` on a clean Mac with a fresh `UNCORK_HOME`: the components' network downloads, the first sign-in and the update after it, and `doctor` without failures there. Valve's installer and the client's first start up to the sign-in window have run in a new bottle on the development Mac.
- A 2-hour Steam session.
- A full match: the skirmish, tutorial, audio, lobby browser and alt-tab items below.
- The Metal HUD benchmark on DXMT and on WineD3D.
- Game Mode: why Rise of Nations crashes at startup through a bundle, then whether Game Mode turns on and what it changes ([open questions 9 and 15](#open-questions)).
- The borderless window on a 5K external display as the main display, and what the camera notch hides at the top centre during a match on the built-in display ([open questions 16 and 18](#open-questions)).
- Running the changes made after the third session on the development Mac: a bottle repaired with `bottle set`, a game launched after a display change, the frame cap on a 60 Hz external display.
- Whether a game started from the terminal the user is typing in comes to the front ([open question 19](#open-questions)).
- Steam asking to sign in again when several copies of a bottle share one login ([open question 17](#open-questions)).
- Setting the profile's `[compat]` status from those reports; until then it stays `untested`.

After M1: Uncork's own runtime (M2), the compatibility database (M3) and the GUI (M4).

Acceptance, on a clean Apple Silicon Mac running macOS 27 with a fresh `UNCORK_HOME`:

- [ ] `uncork doctor` reports no failures after following its own fixes.
- [ ] `uncork setup --yes` installs the recommended components, creates the `steam` bottle and installs Steam; you sign in in Steam's window; the client reaches its library view and stays usable for a 2-hour session. (Steam's installer and first start, up to the sign-in window, seen in a new bottle on the development Mac.)
- [ ] `uncork play rise-of-nations --dry-run` shows DXMT and the profile's environment, overrides and INI edit. (Seen on the development Mac.)
- [ ] `uncork play rise-of-nations` from a cold start starts Steam and the game, and the game reaches its main menu on DXMT. (Seen on the development Mac with an imported bottle.)
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
- [ ] Patches 1, 2 and 4 of the [patch queue](RUNTIME.md#patch-queue) are applied, each with a test or a recorded manual check; patch 3 only if Steam's UI needs it on this runtime.
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
- `doctor` reports `rosetta-sunset` on macOS 27 and later so users know (seen on macOS 27.0.1, 2026-10-07).

## Risks

| Risk | Facts | Mitigation |
|---|---|---|
| End of Rosetta | Above | M5; `CpuBackend` abstraction; test every beta with `game-test-tool` |
| ARM64 path gated by Apple | An undocumented entitlement; which developer accounts can get it is unclear | Paid membership, early request, track DXMT's ARM64 work and other ARM64 Wine efforts |
| Upstream pins disappear or change | The catalog has one Wine runtime, `winecx-gptk-4.7.3`, from one publisher, and its repository has no LICENSE file | Pins fail closed on a hash change; M2 removes the dependency on third-party Wine builds |
| No per-process backend loading in the phase-0 runtime | `winecx-gptk-4.7.3` has neither `renderer-dllpath` nor `dllpath-prepend`: game backends are copied into `system32`/`syswow64`, and D3DMetal cannot be used | Per-launch DLL overrides keep processes apart; the Steam client loads DXVK app-locally; M2 adds `WINEDLLPATH_PREPEND` |
| x86-64 dependency supply ends | Homebrew Intel at Tier 3 since September 2026; Nixpkgs 26.05 is the last with x86_64-darwin ([Homebrew](https://brew.sh/2025/11/12/homebrew-5.0.0), [NixOS](https://nixos.org/blog/announcements/2026/nixos-2605/)) | A small pinned from-source x86-64 dependency build in M2 |
| Steam client changes | Wine bug 60334, first-start crashes after updates; the client's web UI depends on a Direct3D 11 device from DXVK-macOS 1.10.3, a frozen fork; a saved login can stop working when several copies of a bottle use it | Track the issues in [STEAM.md](STEAM.md#known-issues); restart Steam between sessions; import bottles with `--move` |
| D3DMetal license | Personal evaluation use, non-commercial distribution only | Import only; never distributed ([LEGAL.md](LEGAL.md#d3dmetal)) |
| Single maintainer | Whisky was archived when its maintainer stepped away | [GOVERNANCE.md](../GOVERNANCE.md): a second maintainer within three months of v0.2.0 |
| Name | A Madrid Protocol filing for "UNCORK" in class 9 has unknown status | Formal clearance before a 1.0 release |

## Open questions

Each is answered by a hands-on test and recorded in the document named.

| # | Question | Milestone | Answer so far (2026-10-07) | Answer goes into |
|---|---|---|---|---|
| 1 | Does Steam's web UI render on the phase-0 runtime with software CEF, and does it survive `WINEMSYNC=1`? | M1 | Software CEF: no, every window is black. With app-local DXVK: chrome, sign-in and library render, with `WINEMSYNC=1`, and a fresh install's sign-in window renders too; the store page body stays black. A 2-hour session is still to run | [STEAM.md](STEAM.md#graphics-steam-runs-on-dxvk) |
| 2 | Does a direct launch of `riseofnations.exe` work end to end (Steamworks, lobbies, playtime recorded)? | M1 | Started directly while Steam runs, with the game's own `steam_api` loaded, it reaches the main menu without Steam's launcher. Lobbies and playtime not checked | [STEAM.md](STEAM.md#launch-modes), the profile |
| 3 | Does DXMT render all of Rise of Nations' geometry-shader permutations, and how does its frame rate compare with WineD3D? | M1 | The main menu renders on DXMT 0.80 at the display's 120 FPS cap. A match and WineD3D not run yet | the profile, [PERFORMANCE.md](PERFORMANCE.md#measured-so-far) |
| 4 | Does Rise of Nations load a non-`NX_COMPAT` DLL at run time, and does that collapse performance on macOS 27? | M1 | It ships 11 such DLLs, and Uncork warns about them; whether one loads is not checked yet | [PERFORMANCE.md](PERFORMANCE.md#dep-and-nx_compat) |
| 5 | Is Wine's builtin DirectMusic enough for the game's audio? | M1 | Wine logs that no default General MIDI DLS collection is installed (`err:dmloader:get_system_default_gm_path`), so MIDI music may be silent. Not listened for yet | the profile |
| 6 | What causes the "Setting up animals" tutorial hang: the C runtime, the UCRT or something else? | M1 | Open | the profile |
| 7 | Do the WMV intros and the menu background video play with GStreamer and gst-libav? | M1 | Open; the profile skips the intros (`SkipIntroMovies=1`, applied) | the profile |
| 8 | Does Rise of Nations exceed 2 GiB without large-address-awareness? | M1 | Open | [PERFORMANCE.md](PERFORMANCE.md#large-address-aware) |
| 9 | Does a games-category wrapper that `exec`s Wine get Game Mode, and with which `CaptureDisplaysForFullscreen` setting? | M1 | Open. The wrapper starts the game under LaunchServices with the plan's environment. Full screen rendered cropped on 2026-10-07 (question 16); the borderless window that renders correctly since 2026-10-08 has not been measured with Game Mode on and off | [PERFORMANCE.md](PERFORMANCE.md#game-mode) |
| 10 | In `applaunch` mode, do Steam's own processes still render with a game backend in their environment? | M1 | Not run. With a DXMT game the web helper would pair DXMT's `dxgi` with DXVK's `d3d11`, which fails to create swapchains, so black Steam windows are expected | [STEAM.md](STEAM.md#launch-modes) |
| 11 | Do GitHub's hosted arm64 runners expose a Metal device usable for DXMT smoke tests? | M2 | Open | [RUNTIME.md](RUNTIME.md) |
| 12 | Under `game-test-tool` and macOS 28 betas, do `wineserver`, `explorer.exe` and `steamwebhelper.exe` run, and do 32-bit code segments survive? | M5 | Open | this file |
| 13 | Is the ARM64 cross-architecture capability available to open-source projects distributing with Developer ID? | M5 | Open | this file |
| 14 | Why does Steam's embedded store page stay black while the rest of its web UI renders on DXVK? | M1 | Open | [STEAM.md](STEAM.md#known-issues) |
| 15 | Why does Rise of Nations crash at startup when LaunchServices starts it through a Game Mode bundle? | M1 | Crashed in 3 of 5 bundle launches on 2026-10-07 with an unhandled page fault reading address 0 in the game's code; never in 6 or more direct launches; not with Wine's `loaddll` and exception-handling debug channels on, so timing-sensitive. On 2026-10-08, with a consistent Retina mode and DPI and a fresh Wine session, 3 of 3 bundle launches started without a crash; whether the earlier crashes came from the pair that disagreed is not confirmed | [PERFORMANCE.md](PERFORMANCE.md#game-mode) |
| 16 | Why does Rise of Nations in full screen render cropped and offset on a 5K external display with Retina mode off, and does Retina mode, `CaptureDisplaysForFullscreen` or the game's resolution setting fix it? | M1 | Answered on the built-in display on 2026-10-08: the bottle's Retina mode and DPI disagreed (`RetinaMode` `n`, 192 DPI), so the game saw a half-size screen. With a consistent pair, a borderless window (`Fullscreen=2`) at the display's "looks like" size renders the whole picture. A 5K external display as the main display has not been tried with these settings | [PERFORMANCE.md](PERFORMANCE.md#retina-mode-and-dpi), [COMPATIBILITY.md](COMPATIBILITY.md#known-issues) |
| 17 | Does Steam invalidate a saved login when several copies of a bottle use it, and how should `bottle import` handle that? | M1 | An imported clone stopped signing in by itself after several copies had used one login: connected, never logged on with the saved session. Probably the saved login was rotated; not confirmed. For now the docs recommend `--move` or signing in again | [STEAM.md](STEAM.md#known-issues) |
| 18 | On a MacBook Pro with a notch, what does the 185x32-point strip behind the camera hide at the top centre of a borderless desktop-size window during a match? | M1 | Open | [COMPATIBILITY.md](COMPATIBILITY.md#known-issues) |
| 19 | Does a game started from the terminal the user is typing in come to the front? A process cannot bring itself or another app to the front from the background on current macOS (`NSRunningApplication.activate` and `lsappinfo setfront` return `permErr`) | M1 | Open; Uncork prints where to find the window after each launch | [CLI.md](CLI.md#uncork-play) |
