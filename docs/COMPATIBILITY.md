# Compatibility

This document explains how Uncork records how well a game runs: the five status levels, the current results and known issues, what a test report must contain and how to file one, how a report becomes part of a game's profile, and how to write a new profile. Reports are the most useful contribution a player can make and need no Rust. The profile file format itself is in [profiles/README.md](../profiles/README.md).

## Status levels

Every profile has a `[compat] status`. The levels are ordered from worst to best:

| Status | Meaning | Typical evidence |
|---|---|---|
| `untested` | Nobody has reported a result with Uncork yet. The default. | |
| `broken` | Does not start, or is unplayable. | Crash at launch, black screen, hang before the main menu |
| `runs` | Starts and is playable, with notable problems listed in `notes`. | Missing audio, broken videos, a mode that does not work, frequent stutter |
| `playable` | Playable start to finish with minor issues. | A cosmetic glitch, a launcher that misdraws, a workaround needed once |
| `perfect` | Indistinguishable from Windows. | Everything in the checklist below works |

The overall status is the best of the recent first-hand reports, where recent means made on current Uncork, runtime and macOS versions; a better result on outdated versions does not hold the status up, and `notes` says what changed. A status describes Uncork's default setup for that game; a result that needs extra steps says so in `notes`.

## Current results

First-hand results so far. Both built-in profiles are `untested`: reaching the main menu does not show that a game is playable, so a status is set only after a session through the checklist below.

| Game | Status | Run on | Verified | Not tested yet |
|---|---|---|---|---|
| Rise of Nations: Extended Edition (287450) | `untested` | 2026-10-07; MacBook Pro M5 Max, macOS 27.0.1; Wine `winecx-gptk-4.7.3`, DXMT 0.80, Steam client 1788652215 in a bottle imported from CrossOver | `uncork play rise-of-nations --hud` started Steam, then the game on DXMT 0.80 (its `d3d11.dll` and `dxgi.dll` loaded from `syswow64`, the game's own `d3dcompiler_47.dll`) in about 44 s; `SkipIntroMovies=1` was applied. In a run by hand with the same DLLs and overrides, the main menu rendered windowed at 1728x1117: 120 FPS (the display's cap), GPU time 0.48 ms per frame. `uncork inspect` reports a 32-bit, NX-compatible executable that is not large-address-aware, Direct3D 11 through `d3dgl.dll`, Steamworks, 11 DLLs without `NX_COMPAT` (which Uncork warns about) and DXMT as the backend; `--dry-run` sets `SteamAppId` and `SteamGameId` to 287450. Wine logs that no General MIDI DLS collection is installed, so DirectMusic music may be silent. Full screen rendered cropped and offset on a 5K external display, and through a Game Mode bundle the game crashed at startup in 3 of 5 launches ([Known issues](#known-issues)) | A match or skirmish, save and load, music and sound, videos, the multiplayer lobby list, alt-tab, full screen on other displays, WineD3D for comparison |
| Age of Empires II: Definitive Edition (813780) | `untested` | 2026-10-07, same Mac and bottle | `uncork steam games` found it installed (16.5 GB) and matched its profile | Everything; it has not been launched |

The full record of that day, with the commands, is in the [README](../README.md#verified-on).

## Known issues

Problems seen in first-hand runs that a game's `[compat] notes` also carry. Each stays here until a fix is verified.

| Game | Issue | Workaround | Seen |
|---|---|---|---|
| Rise of Nations: Extended Edition | In full screen, the picture is cropped and offset on a 5K external display with Retina mode off | Play windowed: set `Fullscreen=0` under `[RISE OF NATIONS]` in `%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini`, inside the bottle under `drive_c/users/<user>/AppData/Roaming/` (`<user>` is `crossover` in a bottle imported from CrossOver). To have Uncork enforce it on every launch, copy [the built-in profile](../profiles/rise-of-nations-extended-edition.toml) to `$UNCORK_HOME/profiles/` and add an `[[ini]]` entry for that key ([profiles/README.md](../profiles/README.md#ini-edits)) | 2026-10-07, M5 Max, macOS 27.0.1 |
| Rise of Nations: Extended Edition | Started through a Game Mode bundle (`--game-mode`), the game crashed at startup in 3 of 5 launches, with an unhandled page fault reading address 0 in its own code; never when started directly | Leave Game Mode off, the default ([PERFORMANCE.md](PERFORMANCE.md#game-mode)) | 2026-10-07, M5 Max, macOS 27.0.1 |

## What to test

Not every game has every feature; skip what does not apply and say so.

- [ ] Reaches the main menu with the profile's default backend (`uncork play <game> --dry-run` shows which).
- [ ] A typical session of 30 minutes or more without a crash or freeze.
- [ ] Save and load.
- [ ] Music and sound effects; the volume controls work.
- [ ] Intro and in-game videos.
- [ ] Multiplayer: the lobby list loads; a match starts.
- [ ] Alt-tab out and back; switching between full screen and windowed.
- [ ] Mouse capture and edge scrolling; keyboard shortcuts.
- [ ] Frame rate with the Metal HUD (`--hud`), at a stated resolution and quality.

## Filing a report

Open a "Game report" issue from the [issue chooser](https://github.com/tonianev/uncork/issues/new/choose) and include the following. The commands collect it:

| Field | How to get it |
|---|---|
| Game, Steam app id, game build | `uncork steam games` for the app id; `buildid` in the game's `steamapps/appmanifest_<appid>.acf` |
| Uncork version or commit | `uncork --version` |
| macOS version and build | `sw_vers -productVersion` and `sw_vers -buildVersion` |
| Chip | `sysctl -n machdep.cpu.brand_string` |
| Installed components | `uncork runtime list` |
| Backend used | `uncork play <game> --dry-run` (also attach the full output) |
| Host checks | `uncork doctor` |
| Status you would give it | One of the five levels above |
| What works, what does not | The checklist above, with resolution, settings and FPS |
| Workarounds | Anything you changed: `--backend`, `-e`, bottle settings, winetricks verbs |
| Logs, if it fails | The launch log from `$UNCORK_HOME/logs/`, ideally rerun with `--wine-debug +loaddll` |

Rules for reports:

- First-hand only: a run you did on your own Mac. Results copied from ProtonDB, forums, CrossOver's database or a language model's guess are not reports ([AI_CONTRIBUTIONS.md](../AI_CONTRIBUTIONS.md)).
- Scrub personal data. Steam's logs and some launch logs contain your account name and home directory path.
- One game per report. A new Uncork or macOS version is worth a new report even if nothing changed.

## From report to profile

A maintainer, or you in a pull request, adds the report to the game's profile as a `[[compat.reports]]` entry, newest first, and updates `status` and `notes` if the report changes them:

```toml
[compat]
status = "runs"
notes = "What players should know: workarounds, features that do not work."

[[compat.reports]]
date = "2026-11-02"           # YYYY-MM-DD
uncork = "0.1.0 (a1b2c3d)"    # version or commit
macos = "27.0.1"
chip = "M5 Max"
backend = "dxmt"
status = "runs"
notes = "Resolution, settings, FPS with the HUD, what did not work."
```

Every field except `notes` is required. `backend` is one of `dxmt`, `d3dmetal`, `dxvk`, `wined3d`; `status` is one of the five levels.

## Adding a profile

A profile is worth adding when a game needs something Uncork's automatic choice does not give it: a specific backend, an override, an INI setting, a launch mode, or a large-address-aware flag. A game that runs perfectly with the defaults still benefits from a profile, because it carries the compatibility record.

1. Find the executable that renders the game. Steam's launch target is often a launcher; the game is usually the largest `.exe` in the install directory.
2. Inspect it:

   ```bash
   uncork inspect "$UNCORK_HOME/bottles/steam/drive_c/Program Files (x86)/Steam/steamapps/common/<Game>/<game>.exe"
   ```

   This prints the bitness, the graphics APIs found in the executable and the DLLs beside it (with evidence), the large-address-aware and NX flags, Steamworks and anti-cheat files, and the backend Uncork would choose.
3. Create `profiles/<id>.toml`, where `<id>` is the game's name in kebab-case. Start from the fields [profiles/README.md](../profiles/README.md) describes; set only what differs from the defaults, and comment why, with a source where one exists.
4. Test it as a user profile before opening a PR: copy it to `$UNCORK_HOME/profiles/`, then run `uncork profile show <id>` and `uncork play <id> --dry-run`, then play.
5. Run `cargo test -p uncork-core`; the tests validate every built-in profile.
6. Open a PR with the profile, set `status` from your own test, and add your report as the first `[[compat.reports]]` entry.

Anti-cheat: games with kernel-level anti-cheat do not run under Wine. `uncork inspect` reports the anti-cheat files it finds (Easy Anti-Cheat, BattlEye, Vanguard). Such a game can still get a profile marked `broken`, so nobody spends an evening on it.
