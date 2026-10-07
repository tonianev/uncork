# Steam in a bottle

This document describes how Uncork installs and runs the Windows Steam client inside a Wine prefix: why the Windows client is needed at all, what installation and the first start do and how long they take, which client flags Uncork passes or strips and why, how games are launched, the known problems, and what Uncork will never do with Steam. The constants it refers to live in `crates/uncork-steam/src/client.rs`; the orchestration lives in `crates/uncork-core/src/steam.rs`.

## Why the Windows client

Steamworks games call `steam_api`, which needs a running Steam client for the same user. `SteamAPI_Init` fails without one, and SteamStub DRM starts Steam before the game ([Steamworks DRM](https://partner.steamgames.com/doc/features/drm), [steam_api](https://partner.steamgames.com/doc/api/steam_api)). Under Wine, "the same user" means the same prefix and the same wineserver: `steam_api` finds the client through `HKCU\Software\Valve\Steam\ActiveProcess` in the prefix's registry. So the Windows client runs in the bottle, next to the games.

The native macOS Steam client cannot take its place. Bridging Windows games to it needs a port of Proton's `lsteamclient`, which is under the Steamworks SDK license, and patching the client ([Proton lsteamclient](https://github.com/ValveSoftware/Proton/blob/proton_11.0/lsteamclient/LICENSE)).

## Installing

`uncork setup` does this for the `steam` bottle; `uncork steam install --bottle <name>` does it for another one.

1. Download Valve's `SteamSetup.exe` into `$UNCORK_HOME/cache/downloads/`, from `cdn.akamai.steamstatic.com` (the host Valve's [download page](https://store.steampowered.com/about/) links), falling back to `cdn.fastly.steamstatic.com`.
2. Check it. The file last seen is 2,380,800 bytes, an NSIS 3 installer, SHA-256 `7d3654531c32d941b8cae81c4137fc542172bfa9635f169cb392f245a0a12bcb`, the same pin as [winetricks](https://github.com/Winetricks/winetricks/blob/master/src/winetricks). Valve replaces the file in place without changing its URL, so a different hash is a warning, not an error; the file must still parse as a 32-bit Windows GUI executable.
3. Run `wine SteamSetup.exe /S` (silent install). It installs to `C:\Program Files (x86)\Steam` even though the client is 64-bit. Then stop the wineserver, which also ends any `steam.exe` the installer started on its own.
4. Start the client once in the foreground. Sign in yourself in Steam's own window, including Steam Guard. Uncork never sees, stores or types your credentials.

If `steam.exe` already exists in the bottle, the installer step is skipped.

## The first start

The first start is slow and noisy. That is expected.

| What happens | Detail |
|---|---|
| Two-stage bootstrap | The installer's bootstrapper downloads the 32-bit client package, restarts, then migrates to the 64-bit client ([steam_client_win64](https://client-update.steamstatic.com/steam_client_win64)) |
| Duration | 15 to 25 minutes under Rosetta and WoW64 ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)) |
| Crashes | It can stop with "nested exception on signal stack". Start it again with `uncork steam start`; the update resumes where it stopped |
| Progress | `tail -f "$UNCORK_HOME/bottles/steam/drive_c/Program Files (x86)/Steam/logs/bootstrap_log.txt"` |
| After an update | The first start after a client update can quit shortly after launch ([steam-for-linux#13576](https://github.com/ValveSoftware/steam-for-linux/issues/13576)); start it again |

The Windows client has been 64-bit only since 2026-01-01, but it still loads a 32-bit `steamclient.dll` into 32-bit games, so a 32-bit game such as Rise of Nations needs a Wine runtime with the `wow64` feature.

## Graphics: Steam stays on WineD3D

Every Steam process (`steam.exe`, `steamwebhelper.exe`, `steamservice.exe`, the overlay UI) runs on Wine's own WineD3D, never on DXMT or D3DMetal. `steam::client_command` sets `d3d11,dxgi,d3d10core,d3d9,d3d12=b` so a backend DLL left in the prefix can never be loaded into the client. The reasons:

- Steam's web UI is Chromium (CEF). Its GPU process presents into a window owned by another process. DXMT cannot present across processes ([dxmt#141](https://github.com/3Shain/dxmt/issues/141)), and Wine's Mac driver does not implement cross-process child-window Metal swapchains either.
- On D3DMetal the client window stays black ([Highball#282](https://github.com/gauthierpiarrette/highball/issues/282), [frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163)).
- With DXMT's `dxgi` left behind under DXVK's `d3d11`, CEF fails to create its swapchain (`0x887A0004`).

The web UI renders in software instead (`-cef-disable-gpu -cef-disable-gpu-compositing`). That costs CPU while the Steam window is visible, so keep it minimized while playing; `uncork play` starts it with `-silent`.

## Client flags

Uncork checked flags against the strings of client build 1773426488 (March 2026). A flag being present in the binary does not prove the client honors it, so the defaults are few ([Steam client beta discussion](https://steamcommunity.com/groups/SteamClientBeta/discussions/3/3710433479207750727/)).

Passed every time the client starts:

| Flag | Why |
|---|---|
| `-cef-disable-gpu`, `-cef-disable-gpu-compositing` | Software rendering for the web UI; see above |
| `-nofriendsui` | Skips the friends web view: less CEF work while gaming |
| `-silent` (when Uncork starts Steam for a game) | Starts minimized to the tray |

Stripped, with a warning, from any extra client argument list that reaches Uncork:

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

A profile's `[launch] mode` chooses how a game starts. Steam games default to `direct`.

| Mode | What Uncork does | When |
|---|---|---|
| `direct` | Start Steam with `-silent` if it is not running and wait until it is (polling every 2 s); then start the game executable directly in the same prefix, with its own backend environment | Default. Works when the game does not need Steam to start it |
| `applaunch` | Stop Steam if its environment differs from the game's, start it with the game's environment, then run `steam.exe -applaunch <appid> <args>` | Games whose DRM must be started by Steam |
| `standalone` | Start the executable without Steam | DRM-free and non-Steam games |

`direct` gives the game its own per-process backend while Steam stays on WineD3D. `applaunch` cannot: a second `steam.exe` only forwards its arguments to the running client, and the game inherits the client's environment, not the caller's ([Highball SteamRestart.swift](https://github.com/gauthierpiarrette/highball/blob/main/Sources/HighballKit/SteamRestart.swift)). That is why Steam is restarted with the game's environment in that mode. Whether Steam's own processes still render correctly with a game backend in their environment is an open question for M1.

Whether Steam runs is read with `wine reg query HKCU\Software\Valve\Steam\ActiveProcess /v pid` (non-zero means running). Reading `user.reg` from disk is not reliable: Wine flushes it lazily.

### msync and Steam

The synchronization mode belongs to the wineserver, and a client whose `WINEMSYNC` differs from the server's exits. Steam and the games in one bottle therefore always share one setting ([ARCHITECTURE.md](ARCHITECTURE.md#environment)). CrossOver 26.1's own Steam bottle runs with msync and a working UI. A web-UI hang under msync was reported on one non-CrossOver Wine 10 engine and is treated as engine-specific until measured on Uncork's runtime.

## Commands

| Command | Does |
|---|---|
| `uncork steam install [--bottle B] [--yes]` | Download Valve's installer and install Steam |
| `uncork steam start [--bottle B]` | Start the client |
| `uncork steam games [--bottle B]` | List games installed in the bottle's Steam libraries |
| `uncork steam launch <appid> [launch options] [-- args]` | Launch an installed game by app id |
| `uncork play <game>` | Resolve a profile and launch through Steam as the profile says |
| `uncork bottle kill <bottle>` | Stop every Windows process in the bottle, Steam included |

Libraries are read from `steamapps/libraryfolders.vdf` and the `appmanifest_<appid>.acf` files; Windows paths in them are mapped into the prefix. A game counts as installed when its manifest's `StateFlags` has the fully-installed bit ([EAppState](https://github.com/OpenSteamClient/OpenSteamClient/blob/master/include/opensteamworks/EAppState.h)).

## Known issues

| Issue | Symptom | What to do | Source |
|---|---|---|---|
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
- Patch, rename, wrap or replace any Valve binary, including `steamwebhelper.exe`.
- Pass `-noverifyfiles`, or write `Steam.cfg` update inhibitors.
- Write Steam's `.vdf`/`.acf` files, especially while Steam runs.
- Run the Steam client on DXMT or D3DMetal.
