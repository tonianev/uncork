# Steam in a bottle

This document describes how Uncork installs and runs the Windows Steam client inside a Wine prefix: why the Windows client is needed at all, what installation and the first start do, how the client gets a working Direct3D 11 device, which client flags Uncork passes or strips and why, how games are launched, how Uncork tells whether the client runs and how it stops it, the known problems, and what Uncork will never do with Steam. The constants it refers to live in `crates/uncork-steam/src/client.rs`; the orchestration lives in `crates/uncork-core/src/steam.rs`. Measurements marked 2026-10-07 were made on an M5 Max with macOS 27.0.1, Wine `winecx-gptk-4.7.3` (wine-11.17) and Steam client build 1788652215 ([README](../README.md#verified-on)).

## Why the Windows client

Steamworks games call `steam_api`, which needs a running Steam client for the same user. `SteamAPI_Init` fails without one, and SteamStub DRM starts Steam before the game ([Steamworks DRM](https://partner.steamgames.com/doc/features/drm), [steam_api](https://partner.steamgames.com/doc/api/steam_api)). Under Wine, "the same user" means the same prefix and the same wineserver: `steam_api` finds the client through `HKCU\Software\Valve\Steam\ActiveProcess` in the prefix's registry. So the Windows client runs in the bottle, next to the games.

The native macOS Steam client cannot take its place. Bridging Windows games to it needs a port of Proton's `lsteamclient`, which is under the Steamworks SDK license, and patching the client ([Proton lsteamclient](https://github.com/ValveSoftware/Proton/blob/proton_11.0/lsteamclient/LICENSE)).

## Installing

`uncork setup` does this for the `steam` bottle; `uncork steam install --bottle <name>` does it for another one. Both ask before downloading.

1. Download Valve's `SteamSetup.exe` into `$UNCORK_HOME/cache/downloads/`, from `cdn.akamai.steamstatic.com` (the host Valve's [download page](https://store.steampowered.com/about/) links), falling back to `cdn.fastly.steamstatic.com`.
2. Check it. The file last seen is 2,380,800 bytes, an NSIS 3 installer, SHA-256 `7d3654531c32d941b8cae81c4137fc542172bfa9635f169cb392f245a0a12bcb`, the same pin as [winetricks](https://github.com/Winetricks/winetricks/blob/master/src/winetricks). Valve replaces the file in place without changing its URL, so a different hash is a warning, not an error; the file must still parse as a 32-bit Windows GUI executable, or it is deleted and the install fails.
3. Run `wine SteamSetup.exe /S` (silent install), logging to `logs/<bottle>-steam-install.log`. It installs to `C:\Program Files (x86)\Steam` even though the client is 64-bit. Then stop the wineserver, which also ends any `steam.exe` the installer started on its own.
4. Put DXVK next to the web helper ([Graphics](#graphics-steam-runs-on-dxvk)) and start the client once with its window visible. Sign in yourself in Steam's own window, including Steam Guard. Uncork never sees, stores or types your credentials.

If `steam.exe` already exists in the bottle, the installer step is skipped. This path has unit tests but has not yet been run end to end on a clean Mac.

The alternative that has been run on hardware is to import a bottle that already has Steam: `uncork bottle import "$HOME/Library/Application Support/CrossOver/Bottles/Steam" --name steam` clones a CrossOver bottle with APFS clones (21 GB in about 6 s on 2026-10-07; the original is untouched). CrossOver's Windows user is `crossover`, so the import sets `USER=crossover` and `LOGNAME=crossover` in the bottle's `env`; Wine then keeps using that user's `AppData`, where Steam keeps its sign-in and web cache.

## The first start

The first start of a fresh install is slow and noisy. The figures below are third-party reports; Uncork has not measured a fresh install yet.

| What happens | Detail |
|---|---|
| Two-stage bootstrap | The installer's bootstrapper downloads the 32-bit client package, restarts, then migrates to the 64-bit client ([steam_client_win64](https://client-update.steamstatic.com/steam_client_win64)) |
| Duration | 15 to 25 minutes under Rosetta and WoW64 ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)) |
| Crashes | It can stop with "nested exception on signal stack". Start it again with `uncork steam start`; the update resumes where it stopped |
| Progress | `tail -f "$UNCORK_HOME/bottles/steam/drive_c/Program Files (x86)/Steam/logs/bootstrap_log.txt"` |
| After an update | The first start after a client update can quit shortly after launch ([steam-for-linux#13576](https://github.com/ValveSoftware/steam-for-linux/issues/13576)); start it again |

An installed, signed-in client measured on 2026-10-07: started from its own directory, it set its `ActiveProcess` `pid` about 18 s after start, ran its self-update and logged on with its saved session.

The Windows client has been 64-bit only since 2026-01-01, but it still loads a 32-bit `steamclient.dll` into 32-bit games, so a 32-bit game such as Rise of Nations needs a Wine runtime with the `wow64` feature.

## Graphics: Steam runs on DXVK

Steam's web UI is Chromium (CEF), and it needs a real Direct3D 11 device. Uncork gives it one with DXVK on MoltenVK, placed where only Steam finds it:

1. Before every start of the client, `steam::ensure_client_dxvk` copies DXVK's 64-bit `d3d11.dll` and `d3d10core.dll` from the `dxvk` component's `x86_64-windows/`, with Wine's builtin marker removed, into `C:\Program Files (x86)\Steam\bin\cef\cef.win64\`, the directory of `steamwebhelper.exe` (`client::CEF_DIR`). Identical files are not rewritten, and each file is replaced atomically. The DXVK used is the bottle's `graphics.dxvk` pin when `uncork play` starts the client, otherwise the newest installed.
2. The client starts with `WINEDLLOVERRIDES=d3d12,d3d9,dxgi=b;winemenubuilder.exe=d;d3d10core,d3d11=n,b` and `DXVK_LOG_LEVEL=none`. `n,b` lets the web helper load the app-local DXVK; `b` keeps the `dxgi`, `d3d9` and `d3d12` a game's backend left in `system32` or `syswow64` (DXMT's `dxgi.dll`, for example) out of every Steam process.
3. Without a DXVK component the client still starts, with a warning that its windows will be black; `uncork runtime install dxvk` fixes it.

App-local DLLs are found before `system32` (Windows' DLL search order starts in the application's directory), so the client gets DXVK while `system32` and `syswow64` stay free for a game's backend. On 2026-10-07 the app-local placement behaved the same as copying DXVK into `system32`, and Steam's GPU process held the app-local `d3d11.dll` while Rise of Nations ran on DXMT's DLLs from `syswow64` in the same bottle.

| Option | Result | Evidence |
|---|---|---|
| Software CEF (`-cef-disable-gpu -cef-disable-gpu-compositing`) with WineD3D | Every Steam window stays black: software-composited CEF output does not cross Wine's process boundary on macOS | Measured 2026-10-07 |
| WineD3D with GPU CEF | Not used: WineD3D's OpenGL path gives Chromium too low a Direct3D feature level | Uncork's notes in `uncork-core/src/steam.rs`; not re-measured for this document |
| DXMT | Not used: Chromium's GPU process presents into a window owned by another process, and DXMT cannot present across processes | [dxmt#141](https://github.com/3Shain/dxmt/issues/141) |
| D3DMetal | Not used: the client window stays black | [Highball#282](https://github.com/gauthierpiarrette/highball/issues/282), [frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163) |
| DXVK's `d3d11` with DXMT's `dxgi` | Swapchain creation fails (`0x887A0004`) | [frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163) |
| DXVK's `d3d11` and `d3d10core`, Wine's builtin `dxgi` | Client chrome, sign-in and the library list render; the embedded store page body stays black | Measured 2026-10-07; the configuration Uncork uses |

The web UI renders on the GPU through MoltenVK. `uncork play` starts the client with `-silent`, minimized to the tray.

## Client flags

Uncork checked flags against the strings of client build 1773426488 (March 2026). A flag being present in the binary does not prove the client honors it, so the defaults are few ([Steam client beta discussion](https://steamcommunity.com/groups/SteamClientBeta/discussions/3/3710433479207750727/)).

Passed when Uncork starts the client:

| Flag | When | Why |
|---|---|---|
| `-nofriendsui` | Every start | Skips the friends web view: less CEF work while gaming |
| `-silent` | When `uncork play` or `uncork steam launch` starts the client for a game, in either Steam launch mode | Starts minimized to the tray. `uncork setup`, `uncork steam install` and `uncork steam start` show the window |

Deliberately not passed:

| Flag | Why |
|---|---|
| `-cef-disable-gpu`, `-cef-disable-gpu-compositing` | Every Steam window stays black with them (measured 2026-10-07, client build 1788652215); see [Graphics](#graphics-steam-runs-on-dxvk) |

Stripped, with a warning, from any extra client argument list given to `client::client_args` (the CLI passes none today):

| Flag | Why stripped |
|---|---|
| `-noreactlogin`, `-no-browser` | Removed from the client by Valve in December 2022 |
| `-cef-force-32bit` | A 2020 CrossOver workaround, no longer in the client binaries. winetricks still prints the old advice on macOS |
| `-cef-in-process-gpu`, `-cef-single-process` | No longer in the client binaries |
| `-nochatui`, `-nooverlay` | No longer in the client binaries |
| `-nobootstrapupdate` | A misspelling from old guides; the real flag is `-nobootstrapperupdate` |
| `-noverifyfiles` | Present, but it skips the client's check of its own files; community recipes use it to keep modified Valve binaries running |

Never used: `steamwebhelper.exe` wrappers or renames, `Steam.cfg` update inhibitors (`BootStrapperInhibitAll` and similar), and `-noverifyfiles`. All of them modify or block Valve's client.

## Launch modes

A profile's `[launch] mode` chooses how a game starts; without a profile, `uncork play <appid>` uses `direct`. In every mode the profile's INI edits are applied first, and only to files that already exist.

| Mode | What Uncork does | When |
|---|---|---|
| `direct` | If the client is not running: put DXVK in place, start the client with `-silent -nofriendsui`, and poll every 2 s until it reports running (up to 180 s). Then put the game's backend DLLs in place and start the game executable directly in the same prefix, with its own environment and DLL overrides | Default. Works when the game does not need Steam to start it. Run on hardware: Rise of Nations, about 44 s from command to game with Steam starting first (2026-10-07) |
| `applaunch` | If the client runs, stop it ([Stopping](#detecting-and-stopping-the-client)). Put the game's backend DLLs and Steam's DXVK in place, then start `steam.exe -silent -nofriendsui -applaunch <appid> <profile args> <args>` with the game's environment and wait until it reports running; the client then starts the game | Games whose DRM must be started by Steam. Unit-tested only |
| `standalone` | Start the executable without Steam | DRM-free and non-Steam games. Unit-tested only |

`direct` gives the game its own backend while Steam keeps its DXVK. `applaunch` cannot: a second `steam.exe` only forwards its arguments to the running client, and the game inherits the client's environment, not the caller's ([Highball SteamRestart.swift](https://github.com/gauthierpiarrette/highball/blob/main/Sources/HighballKit/SteamRestart.swift)). That is why Uncork restarts the client with the game's environment, keeping `d3d10core,d3d11=n,b` so the web helper still loads its DXVK. The game is unaffected, because its backend comes from `system32`/`syswow64`. The client may not be: with a DXMT game the web helper also inherits `dxgi=n,b`, loads DXMT's `dxgi` next to DXVK's `d3d11`, and that pairing fails to create swapchains, so Steam's own windows can stay black until the client is restarted normally. This follows from the DLL pairing in the table above; it has not been run.

Without a profile, the executable is the largest `.exe` at the shallowest level (up to three below the install directory) that has one, skipping names of launchers, crash reporters and redistributable installers (`client::NON_GAME_EXE_PREFIXES`). A profile's `[exe] path` overrides that.

With `--game-mode` or `performance.game_mode = true`, `direct` and `standalone` launches go through a Game Mode app bundle ([PERFORMANCE.md](PERFORMANCE.md#game-mode)); in `applaunch` mode the client starts the game and no bundle is used.

## Detecting and stopping the client

Whether the client runs is read with `wine reg query HKCU\Software\Valve\Steam\ActiveProcess /v pid`, run with the bottle's environment so that a wineserver it starts gets the bottle's msync setting. A non-zero `pid` (printed as `    pid    REG_DWORD    0x274`) means running; `reg` exiting with status 1 ("Unable to find the specified registry key") means not running. Reading `user.reg` from disk is not reliable: Wine flushes it lazily.

The client sets the `pid` on start and clears it on a clean exit. A client that was killed or crashed leaves its `pid` behind, and Uncork would then treat Steam as running and start a game without it. So after Uncork kills a client, it writes the `pid` back to 0 with `wine reg add ... /t REG_DWORD /d 0 /f` (`steam::forget_client`).

`steam.exe -shutdown` is not a reliable way to stop the client: on 2026-10-07 it did not stop it within 60 s. Uncork therefore stops the client in steps (`steam::stop`, used by `applaunch`): send `-shutdown`, poll every 2 s for up to 180 s, then run `wineserver --kill`, which ends every process in the bottle, games included, and reset the `pid`. To stop Steam by hand, run `uncork bottle kill <bottle>`: it runs `wineserver --kill` at once and, when that stopped something and Steam is installed in the bottle, resets the `pid` (verified 2026-10-07). When nothing was running it says so and leaves the `pid` alone; a stale one left by a crash is then reset by hand:

```bash
( eval "$(uncork bottle env steam)" &&
  wine reg add 'HKCU\Software\Valve\Steam\ActiveProcess' /v pid /t REG_DWORD /d 0 /f )
```

### msync and Steam

The synchronization mode belongs to the wineserver, and a client whose `WINEMSYNC` differs from the server's exits. Steam, the games and Uncork's `wine reg` queries in one bottle therefore always share one setting ([ARCHITECTURE.md](ARCHITECTURE.md#environment)). With the catalog's runtime msync is on by default, and on 2026-10-07 the client ran with `WINEMSYNC=1`, rendered its chrome and library, and ran next to Rise of Nations. The 2-hour session that M1 asks for has not been run yet. A web-UI hang under msync was reported on one non-CrossOver Wine 10 engine ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)) and has not been seen on Uncork's runtime.

## Commands

| Command | Does |
|---|---|
| `uncork steam install [--bottle B] [--yes]` | Download Valve's installer, install Steam, put DXVK in place and start the client with its window visible |
| `uncork steam start [--bottle B]` | Put DXVK in place and start the client with its window visible; nothing if it already runs |
| `uncork steam games [--bottle B]` | List games installed in the bottle's Steam libraries, with size, state and matching profile |
| `uncork steam launch <appid> [launch options] [-- args]` | Launch an installed game by app id, with the profile that has that app id |
| `uncork play <game>` | Resolve a profile and launch through Steam as the profile says |
| `uncork bottle kill <bottle>` | Stop every Windows process in the bottle, Steam included, and reset Steam's `pid` when it stopped something |

The full option lists are in [CLI.md](CLI.md). Libraries are read from `steamapps/libraryfolders.vdf` and the `appmanifest_<appid>.acf` files; Windows paths in them are mapped into the prefix. A game counts as installed when its manifest's `StateFlags` has the fully-installed bit ([EAppState](https://github.com/OpenSteamClient/OpenSteamClient/blob/master/include/opensteamworks/EAppState.h)); a launch of a game that is not fully installed carries a warning.

## Known issues

| Issue | Symptom | What to do | Source |
|---|---|---|---|
| Store page | The embedded store page body stays black; the client chrome, sign-in and library list render | Use the library; open store pages in a Mac browser | Measured 2026-10-07 |
| Local network prompt | macOS asks whether the app that started Steam (your terminal or IDE) may "find devices on local networks" | Steam's LAN discovery asks for it; Uncork does not need it. Allow it if you use Steam's LAN features | Seen 2026-10-07 |
| `-shutdown` ignored | `steam.exe -shutdown` leaves the client running for over a minute | Uncork falls back to `wineserver --kill`; by hand, `uncork bottle kill <bottle>` | Measured 2026-10-07 |
| Stale running marker | After Steam crashed or was killed outside Uncork, `uncork play` believes Steam runs and starts the game without it | `uncork bottle kill <bottle>` while something still runs; otherwise reset the `pid` by hand ([above](#detecting-and-stopping-the-client)) | `steam::is_running`, `uncork bottle kill` |
| No DXVK | Every Steam window is black | `uncork runtime install dxvk` | `steam::ensure_client_dxvk` |
| Wine bug 60334 | Steam's UI stops responding after about 11 to 13 game launches in one session | Restart Steam between sessions: `uncork bottle kill steam` | [bug 60334](https://bugs.winehq.org/show_bug.cgi?id=60334) |
| Native Steam signed in to the same account | The bottle's client shows "NO CONNECTION" or the session is replaced | Quit the macOS Steam app, or sign it out, while playing through Uncork | [discussion](https://steamcommunity.com/discussions/forum/1/597413188027695764/) |
| First start after a client update | The client quits shortly after launch | Start it again | [steam-for-linux#13576](https://github.com/ValveSoftware/steam-for-linux/issues/13576) |
| Missing fonts | Store, Profile and Community pages show no text | `uncork winetricks corefonts` (downloads Microsoft's fonts at your request) | [bug 37110](https://bugs.winehq.org/show_bug.cgi?id=37110) |
| "Steam Client Service failed to start (GLE 126)" | A warning at start | Harmless for most games | [highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json) |
| Steam overlay | A game hangs at start or on the first frame on DXMT | Disable the overlay for that game: `gameoverlayrenderer = ""` under the profile's `[dll_overrides]` | [macos-wine-steam#8](https://github.com/ByMedion/macos-wine-steam/pull/8) |

## Never

These follow from the [Steam Subscriber Agreement](https://store.steampowered.com/subscriber_agreement/) (sections 2.G, 4.B and 4.C) and from keeping Steam working. [LEGAL.md](LEGAL.md#steam) has the details.

- Handle Steam credentials, or automate sign-in.
- Redistribute, mirror or bundle `SteamSetup.exe` or the client.
- Patch, rename, wrap or replace any Valve binary, including `steamwebhelper.exe`. The DXVK DLLs Uncork puts in the web helper's directory are additional files, not replacements of Valve's.
- Pass `-noverifyfiles`, or write `Steam.cfg` update inhibitors.
- Write Steam's `.vdf`/`.acf` files, especially while Steam runs.
- Run the Steam client on DXMT or D3DMetal.
