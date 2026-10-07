# 0001: Licensing of Uncork and of the components it uses

Status: Accepted
Date: 2026-10-07

This record fixes the license of everything in the Uncork repository before the first external contribution, because relicensing later would need every contributor's consent. It also fixes how Uncork relates to the licenses of the software it downloads and drives, which range from permissive to copyleft to proprietary.

## Context

Uncork sits between several bodies of software with different terms:

| Software | License | Source |
|---|---|---|
| Wine | LGPL-2.1-or-later | [WineHQ](https://gitlab.winehq.org/wine/wine) |
| DXMT | MIT up to v0.80; LGPL-2.1-or-later on `main` since 2026-04-25 | [LICENSE](https://github.com/3Shain/dxmt/blob/main/LICENSE) |
| DXVK-macOS | Zlib | [Gcenx/DXVK-macOS](https://github.com/Gcenx/DXVK-macOS) |
| MoltenVK | Apache-2.0 | [MoltenVK](https://github.com/KhronosGroup/MoltenVK) |
| D3DMetal | Apple's proprietary GPTK license (EA18380) | [License.pdf](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/License.pdf) |
| Steam client | Steam Subscriber Agreement | [SSA](https://store.steampowered.com/subscriber_agreement/) |
| Whisky, Highball, Mythic, Heroic | GPL-3.0 | their repositories |
| Silo | LGPL-2.1 | [Silo](https://github.com/mikaelhug/Silo) |
| MetalSharp | AGPL-3.0 | [MetalSharp](https://github.com/metalsharp/MetalSharp) |

Uncork runs these as separate programs and loads none of them into its own process: it downloads files, writes configuration and spawns the Wine loader. Its own code is therefore free to choose its license.

The Rust ecosystem's norm is `MIT OR Apache-2.0` with an implicit dual-license contribution clause and no CLA. A permissive core lets the planned SwiftUI app, other front ends and other projects reuse the library; every comparable macOS project is GPL or AGPL, so a permissive one fills a gap. Uncork's planned GUI binding, UniFFI, is MPL-2.0 (file-level copyleft), which a permissive core can depend on.

Much of the initial code and documentation is written with AI assistance under human review; [AI_CONTRIBUTIONS.md](../../AI_CONTRIBUTIONS.md) states the policy and the copyright position.

## Decision

1. Uncork's code, game profiles, documentation and scripts are licensed `MIT OR Apache-2.0`. `LICENSE-MIT` and `LICENSE-APACHE` sit at the repository root, and `Cargo.toml` declares `license = "MIT OR Apache-2.0"`.
2. There is no CLA and no DCO. `CONTRIBUTING.md` carries the implicit clause verbatim: "Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions."
3. Every component keeps its own license. Uncork records each catalog entry's SPDX license and source location, and never relicenses, modifies or wraps a component's terms.
4. No code from GPL, LGPL or AGPL projects is copied into `crates/`. Patches to Wine or DXMT are derivative works and are licensed LGPL-2.1-or-later; they live with the runtime build, not in `crates/`, and the scripts that apply them stay under the repository license.
5. Uncork never distributes D3DMetal or the Steam client ([0004](0004-runtime-supply.md), [LEGAL.md](../LEGAL.md)).
6. Rust dependencies must be under licenses on the allowlist in `deny.toml` (permissive licenses plus MPL-2.0); `cargo deny check` enforces it in CI.

## Alternatives considered

| Alternative | Why not |
|---|---|
| GPL-3.0, like Whisky and Highball | Blocks reuse of the core in permissive or proprietary front ends. The components Uncork drives are separate programs either way, so copyleft here would cover only the orchestrator |
| LGPL-2.1 to match Wine | Same reuse cost for a static Rust library, without a matching benefit |
| A CLA | Signing infrastructure and friction; the implicit clause is the accepted norm in Rust |
| DCO (`Signed-off-by`) | Reasonable if a corporate contributor asks; not needed now |
| CC0 for game profiles now | Exchanging data with the CC0 highball-db would favour it, but the compatibility database's form is not designed yet; decided in M3 with its own ADR ([ROADMAP.md](../ROADMAP.md#m3-compatibility-database-reports-and-benchmarks)) |

## Consequences

- Anyone can build on `uncork-core`, `uncork-pe` and `uncork-steam`, including in proprietary software.
- Contributors do nothing beyond opening a PR; the clause in `CONTRIBUTING.md` and the PR checkbox carry the agreement.
- Reviewers reject code pasted from copyleft projects; learning from them is fine and is cited in comments.
- Uncork's own runtime build (M2) must meet the LGPL's source obligations release by release ([RUNTIME.md](../RUNTIME.md#release-contents)).
- Only the project lead may change the license, and only through a new ADR that supersedes this one; once external contributions exist, that would need each contributor's consent.
