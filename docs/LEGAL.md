# Legal

This document says which third-party software Uncork downloads, which it never redistributes, and why; summarizes the licenses and terms that shape those rules (Apple's Game Porting Toolkit license, the Steam Subscriber Agreement, the LGPL, Microsoft's redistributable terms); and states how Uncork uses other companies' trademarks. It records the project's policy, not legal advice. If you think Uncork gets something here wrong, open an issue or write to the address in [SECURITY.md](../SECURITY.md).

## Uncork's own license

Uncork's code, game profiles and documentation are dual-licensed under MIT OR Apache-2.0 ([LICENSE-MIT](../LICENSE-MIT), [LICENSE-APACHE](../LICENSE-APACHE), [ADR 0001](adr/0001-license.md)). That license covers only what is in this repository. Every component below keeps its own license.

## What Uncork downloads and what it never redistributes

| Component | License or terms | How it reaches your Mac | Uncork redistributes it? |
|---|---|---|---|
| Wine runtime, phase 0 (`sikarugir-11.0_1`, `winecx-gptk-4.7.3`) | LGPL-2.1-or-later | `uncork runtime install` downloads the exact archive pinned in `runtime/catalog.toml` from the publisher's GitHub release and verifies its SHA-256 | No |
| Wine runtime, phase 1 (planned, M2) | LGPL-2.1-or-later | Uncork's own CI build, published with its complete corresponding source | Yes, with source ([below](#lgpl-obligations-for-wine-builds)) |
| DXMT 0.80 | MIT (DXMT `main` is LGPL-2.1-or-later since 2026-04-25) | Downloaded from the 3Shain/dxmt release | No |
| DXVK-macOS 1.10.3 | Zlib | Downloaded from the Gcenx/DXVK-macOS release | No |
| MoltenVK | Apache-2.0 | Inside the Wine runtime archives that include it | No |
| D3DMetal (Apple Game Porting Toolkit) | Apple's GPTK license | You download GPTK from Apple and import it with `uncork runtime import-gptk` | Never |
| `SteamSetup.exe` and the Steam client | Steam Subscriber Agreement | Uncork downloads Valve's installer from Valve's CDN when you install Steam; the client then updates itself from Valve | Never |
| Visual C++ and DirectX redistributables, Microsoft core fonts, MSXML | Microsoft license terms | Installed by Steam with the game (Steamworks Common Redistributables), or by `uncork winetricks` at your request from Microsoft's servers | Never |
| Games | Your Steam license | Steam | Never |

Uncork does not mirror any of these files. Fetching each from its publisher keeps the publisher's license terms and source offer attached to the file you receive, and the SHA-256 pin gives you the same integrity a mirror would ([ADR 0004](adr/0004-runtime-supply.md)). Each installed component's `component.toml` records its license and where its source code is published.

## D3DMetal

D3DMetal is Apple's Direct3D 11/12-to-Metal layer, part of the Game Porting Toolkit. It is proprietary and ships under Apple's license agreement EA18380, the same text in GPTK 3.0 and in GPTK 4.0 beta 2 ([License.pdf](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/License.pdf)). In summary:

- Section 2A grants a limited, personal, non-transferable license to install, use and test the software for developing, testing or evaluating video games for Apple-branded products.
- Distribution is permitted only for non-commercial purposes.
- Modification, decompilation and reverse engineering are not permitted, and use is limited to Apple-branded hardware.

Uncork's policy follows from that:

- Uncork never downloads, bundles, mirrors or links to a copy of D3DMetal hosted by anyone but Apple. The catalog rejects `d3dmetal` entries.
- You download the Game Porting Toolkit yourself from [Apple Developer](https://developer.apple.com/games/game-porting-toolkit/), signing in with your Apple Account, and accept Apple's license there.
- `uncork runtime import-gptk <path>` copies D3DMetal from your mounted GPTK image into `$UNCORK_HOME/components/d3dmetal/<version>/` without modifying it. It is used from there by reference; it is never copied into a bottle.
- Uncork does not patch, wrap or decompile D3DMetal. Besides the path Wine needs to find it (`CX_APPLEGPTK_LIBD3DSHARED_PATH`), it sets only environment variables Apple documents in the GPTK Read Me.

Third-party practice differs (some projects host D3DMetal under the non-commercial clause); Uncork does not, so that its own distribution never depends on that reading.

## Steam

The Steam Subscriber Agreement ([SSA](https://store.steampowered.com/subscriber_agreement/), updated 2026-09-10) licenses the Steam client for personal, non-commercial use (section 2.A), forbids reverse engineering or modifying it without Valve's consent (2.G), and forbids unauthorized third-party software that tampers with Steam processes and automation that interacts with Steam (4.B, 4.C). Uncork's rules:

| Uncork does | Uncork never does |
|---|---|
| Download `SteamSetup.exe` from Valve's own CDN at install time | Redistribute or mirror the installer or the client |
| Run Valve's installer and client unmodified, with command-line flags the client accepts | Patch, rename, wrap or replace any Valve binary, including `steamwebhelper.exe` |
| Open the client's own window so you sign in yourself | See, store or type your Steam credentials, or automate sign-in |
| Read `libraryfolders.vdf` and `appmanifest_*.acf` to find installed games | Write Steam's files, or edit `Steam.cfg` to block updates |
| Start a game directly in the bottle while your client runs, or ask the client to start it with `-applaunch` | Bridge games to the native macOS Steam client (that needs Valve's Steamworks-SDK-licensed `lsteamclient` and patching the client) |

## LGPL obligations for Wine builds

Wine and current DXMT are licensed under the GNU Lesser General Public License, version 2.1 or later. Whoever distributes the binaries must make the corresponding source available.

- Phase 0 (today): Uncork distributes no Wine binaries. `uncork runtime install` downloads them from their publishers, whose releases carry the source obligation. The catalog records each entry's `source_code` location, and `component.toml` keeps it on your disk.
- Phase 1 (planned, M2): Uncork will publish its own runtime builds. Each release will include, next to the binary archive, the exact upstream source tarballs it was built from (with their SHA-256), every patch applied, the build scripts and a `SOURCES.json` manifest tying them together, satisfying LGPL-2.1 section 6 ([RUNTIME.md](RUNTIME.md#phase-1-uncorks-own-ci-build)).
- Patches to Wine or DXMT are derivative works and are licensed LGPL-2.1-or-later, whatever the license of the scripts that apply them.
- If Uncork ever redistributes a third party's build, it must also host or point to that build's exact corresponding source. One runtime Uncork pins today, `winecx-gptk`, has no LICENSE file in its repository; its source is the matching `dappermint/winecx` commit.

## Microsoft redistributables and fonts

Many games need Microsoft's Visual C++ runtime, DirectX components, MSXML or the core fonts. Uncork ships none of them.

- Steam installs a game's declared redistributables itself from Valve's Steamworks Common Redistributables when it installs or first starts the game.
- `uncork winetricks <verbs>` runs your own installed copy of [winetricks](https://github.com/Winetricks/winetricks) in a bottle. winetricks downloads the files from Microsoft and the license terms are between you and Microsoft. Uncork never runs it without your command.
- Steam's web UI needs Arial and Times New Roman ([Wine bug 37110](https://bugs.winehq.org/show_bug.cgi?id=37110)). The plan is to supply metric-compatible fonts under the SIL Open Font License with registry font replacements; until then, `uncork winetricks corefonts` installs Microsoft's fonts at your request.

## Trademarks and nominative use

Microsoft, Windows, DirectX, Rise of Nations and Age of Empires are trademarks of the Microsoft group of companies. Steam and Steamworks are trademarks of Valve Corporation. Apple, macOS and Metal are trademarks of Apple Inc. CrossOver is a trademark of CodeWeavers, Inc. Other names may be trademarks of their respective owners.

Uncork uses these names only to say what it works with: which games a profile is for, which operating system and graphics APIs it translates between. It uses no logos, does not suggest endorsement, and names games in profiles and documentation only as far as needed to identify them. Uncork is not affiliated with or endorsed by CodeWeavers, Apple, Valve or Microsoft.

## Code provenance

No code from GPL, LGPL or AGPL projects (Whisky, Highball, Silo, MetalSharp, Wine) is copied into Uncork's crates, which keeps them under MIT OR Apache-2.0. Learning from those projects is fine; a comment cites what was learned and where. Rust dependencies are checked by `cargo deny` against the license allowlist in `deny.toml`.
