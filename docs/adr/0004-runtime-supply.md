# 0004: Runtime supply: pinned upstream builds first, Uncork's own CI build next

Status: Accepted
Date: 2026-10-07

This record decides where the Wine runtime and graphics components on a user's Mac come from: who builds them, who hosts them, how Uncork verifies them, and why D3DMetal is handled differently from everything else. The operational details are in [RUNTIME.md](../RUNTIME.md).

## Context

- Uncork needs an x86-64 Wine for macOS with new-style WoW64, the Mac driver glue DXMT and D3DMetal need, msync, large-address-aware support and a per-process way to load a backend. No single upstream build has all of these today ([RUNTIME.md](../RUNTIME.md#phase-0-pinned-upstream-builds)).
- Building Wine for macOS is a substantial project of its own: an x86-64 cross build on arm64 runners, a mingw-w64 PE toolchain, x86-64 dependencies whose prebuilt sources are drying up, and CI gates for relocation and the 32-bit half ([winecx-gptk build.yml](https://github.com/dappermint/winecx-gptk/blob/main/.github/workflows/build.yml), [Homebrew 5.0.0](https://brew.sh/2025/11/12/homebrew-5.0.0)).
- Good builds already exist: Gcenx's Sikarugir engines and dappermint's winecx-gptk, both published as GitHub releases with LGPL sources.
- Whoever distributes LGPL binaries must offer the corresponding source. One pinned runtime's repository has no LICENSE file; its source is the matching `dappermint/winecx` commit.
- Homebrew disabled the Wine casks that fail Gatekeeper on 2026-09-01 ([wine@devel cask](https://github.com/Homebrew/homebrew-cask/blob/main/Casks/w/wine@devel.rb)), so there is no package manager route either.
- D3DMetal's license permits distribution only for non-commercial purposes and grants a personal evaluation license ([License.pdf](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/License.pdf)). Third parties read that differently; some host it, some do not.
- Whisky froze on one Wine version (CrossOver 22.1.1, Wine 7.7) and new Steam clients stopped working on it ([Whisky](https://github.com/Whisky-App/Whisky)).

## Decision

1. Phase 0 (now): Uncork consumes pinned upstream builds. `runtime/catalog.toml` pins each archive by HTTPS URL, size and SHA-256, and records its license and source location. Uncork downloads from the publisher at install time, verifies the hash while streaming and refuses a mismatch. It never mirrors these files.
2. Phase 1 (M2): Uncork builds its own runtime in CI from CrossOver's published LGPL Wine sources (26.3.0 first) plus a short patch queue, and publishes each release with its complete corresponding source, patches, build scripts and a `SOURCES.json` index.
3. Every pin change is a reviewed pull request in which the author downloaded the file and computed the hash ([RUNTIME.md](../RUNTIME.md#updating-a-pin)). Pins fail closed.
4. Runtime capabilities are declared as feature tags in the catalog and read by the planner, so a new runtime is a data change.
5. D3DMetal is import-only: the user downloads Apple's Game Porting Toolkit from Apple and runs `uncork runtime import-gptk`. The catalog rejects `d3dmetal` entries.
6. Users are never stuck on one runtime: several versions can be installed side by side, and each bottle records the Wine version it uses.

## Alternatives considered

| Alternative | Why not |
|---|---|
| Mirror the upstream builds on Uncork's own releases | Moves the LGPL source obligation onto Uncork for builds it did not make, including one without a LICENSE file; costs hosting; adds nothing to integrity that the SHA-256 pin does not already give |
| Build our own runtime from day one | Delays the first end-to-end test (M1) by the length of a hard build project; existing builds are good enough to find the real problems first |
| Upstream WineHQ builds only (Gcenx's macOS_Wine_builds) | No msync, no Mac driver glue for DXMT and D3DMetal, no large-address-aware override; kept as an A/B baseline |
| "Bring your own CrossOver": drive the user's `CrossOver.app` | Ties Uncork to a commercial product's internal layout and license; acceptable at most as a developer-only provider that never patches or redistributes it, not as the supply |
| Homebrew casks | Disabled for Gatekeeper failures in September 2026 |
| Bundle D3DMetal under the non-commercial clause | Uncork would depend on one reading of Apple's license; importing the user's own copy needs no such reading |

## Consequences

- An install is reproducible: the same catalog gives the same bytes on every Mac, or an error.
- Upstream publishers can break a pin by replacing a file in place; Uncork then fails closed until a reviewed pin update lands. That risk ends for Wine when phase 1 ships.
- Phase 1 makes Uncork an LGPL distributor, with the obligations [LEGAL.md](../LEGAL.md#lgpl-obligations-for-wine-builds) lists, and gives it one runtime with msync, large-address-aware and per-process activation.
- Users who want D3DMetal take one extra step, once per GPTK version.
- `doctor` reports a bottle whose Wine version is not installed, so a pin change never silently re-pins a bottle.
