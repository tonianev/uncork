# Steam in a bottle

This document describes how Uncork installs and runs the Windows Steam client inside a Wine prefix: why the Windows client is needed at all, what installation and the first start do, how the client gets a working Direct3D 11 device, which client flags Uncork passes or strips and why, how games are launched, how Uncork tells whether the client runs and how it stops it, the known problems, and what Uncork will never do with Steam. The constants it refers to live in `crates/uncork-steam/src/client.rs`; the orchestration lives in `crates/uncork-core/src/steam.rs`. Measurements marked 2026-10-07 were made on an M5 Max with macOS 27.0.1, Wine `winecx-gptk-4.7.3` (wine-11.17) and Steam client build 1788652215, in a bottle imported from CrossOver and in a new bottle with a fresh install ([README](../README.md#verified-on)).

## Why the Windows client

Steamworks games call `steam_api`, which needs a running Steam client for the same user. `SteamAPI_Init` fails without one, and SteamStub DRM starts Steam before the game ([Steamworks DRM](https://partner.steamgames.com/doc/features/drm), [steam_api](https://partner.steamgames.com/doc/api/steam_api)). Under Wine, "the same user" means the same prefix and the same wineserver: `steam_api` finds the client through `HKCU\Software\Valve\Steam\ActiveProcess` in the prefix's registry. So the Windows client runs in the bottle, next to the games.

The native macOS Steam client cannot take its place. Bridging Windows games to it needs a port of Proton's `lsteamclient`, which is under the Steamworks SDK license, and patching the client ([Proton lsteamclient](https://github.com/ValveSoftware/Proton/blob/proton_11.0/lsteamclient/LICENSE)).

## Installing

`uncork setup` does this for the `steam` bottle; `uncork steam install --bottle <name>` does it for another one. Both ask before downloading. Both first finish a bottle that an earlier `bottle create` left incomplete, for example because `wineboot` hung (`bottle::finish_create`, [ARCHITECTURE.md](ARCHITECTURE.md#bottles)).

1. Download Valve's `SteamSetup.exe` into `$UNCORK_HOME/cache/downloads/`, from `cdn.akamai.steamstatic.com` (the host Valve's [download page](https://store.steampowered.com/about/) links), falling back to `cdn.fastly.steamstatic.com`. A `SteamSetup.exe` already there is reused when its SHA-256 is the pinned one, and downloaded again otherwise.
2. Check it. The file last seen is 2,380,800 bytes, an NSIS 3 installer, SHA-256 `7d3654531c32d941b8cae81c4137fc542172bfa9635f169cb392f245a0a12bcb`, the same pin as [winetricks](https://github.com/Winetricks/winetricks/blob/master/src/winetricks); on 2026-10-07 the download matched it. Valve replaces the file in place without changing its URL, so a different hash is a warning, not an error; the file must still parse as a 32-bit Windows GUI executable, or it is deleted and the install fails.
3. Run `wine SteamSetup.exe /S` (silent install), logging to `logs/<bottle>-steam-install.log`. It installs `Steam.exe` (capital S) to `C:\Program Files (x86)\Steam` even though the client is 64-bit; Uncork finds the executable ignoring letter case, so a case-sensitive volume works too. Then stop the wineserver, which also ends any client the installer started on its own.
4. Put DXVK next to the web helper ([Graphics](#graphics-steam-runs-on-dxvk)) and start the client once with its window visible. Sign in yourself in Steam's own window, including Steam Guard. Uncork never sees, stores or types your credentials.

If `Steam.exe` already exists in the bottle, in any letter case, the installer step is skipped. On 2026-10-07 `uncork bottle create fresh` followed by `uncork steam install --bottle fresh -y` ran this path end to end up to the sign-in window ([The first start](#the-first-start)). `uncork setup` from nothing on a clean Mac, and signing in to a fresh install, have not been run yet.

The alternative is to import a bottle that already has Steam: `uncork bottle import "$HOME/Library/Application Support/CrossOver/Bottles/Steam" --name steam` clones a CrossOver bottle with APFS clones (21 GB in about 6 s on 2026-10-07; the original's files are not touched). CrossOver's Windows user is `crossover`, so the import sets `USER=crossover` and `LOGNAME=crossover` in the bottle's `env`; Wine then keeps using that user's `AppData`, where Steam keeps its sign-in and web cache. A Steam `ActiveProcess` `pid` left in the copy's `user.reg` (a bottle copied while Steam ran) is reset to 0.

The clone also carries the original's saved Steam login, and two copies of one login do not stay signed in reliably. On 2026-10-07 the imported clone stopped signing in by itself after several copies of the bottle had used the same login: its connection log shows it connecting but never logging on with the saved session, and it showed the sign-in window. Probably Steam rotated the saved login, so the copies that did not take part in the rotation hold a login that no longer works; CrossOver's own Steam may ask to sign in again too. Import with `--move` when you will not go back to CrossOver; otherwise expect to sign in once more, and from then on use only one of the two copies.

## The first start

On 2026-10-07 the first start of a fresh install, in a new bottle on the development Mac, downloaded the 336 MB client and showed the full "Sign in to Steam" window, with its fields and QR code, on app-local DXVK within about a minute. Nobody signed in, so the update after sign-in was not timed. Others report much slower and noisier first starts; the figures below are theirs.

| What happens | Detail |
|---|---|
| Two-stage bootstrap | The installer's bootstrapper downloads the 32-bit client package, restarts, then migrates to the 64-bit client ([steam_client_win64](https://client-update.steamstatic.com/steam_client_win64)) |
| Duration | 15 to 25 minutes under Rosetta and WoW64 ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)); about a minute to the sign-in window in Uncork's one fresh install |
| Crashes | It can stop with "nested exception on signal stack". Start it again with `uncork steam start`; the update resumes where it stopped |
| Progress | `tail -f "$UNCORK_HOME/bottles/steam/drive_c/Program Files (x86)/Steam/logs/bootstrap_log.txt"` |
| After an update | The first start after a client update can quit shortly after launch ([steam-for-linux#13576](https://github.com/ValveSoftware/steam-for-linux/issues/13576)); start it again |

An installed, signed-in client measured on 2026-10-07: started from its own directory, it set its `ActiveProcess` `pid` about 18 s after start, ran its self-update and logged on with its saved session.

The Windows client has been 64-bit only since 2026-01-01, but it still loads a 32-bit `steamclient.dll` into 32-bit games, so a 32-bit game such as Rise of Nations needs a Wine runtime with the `wow64` feature.

## Graphics: Steam runs on DXVK

Steam's web UI is Chromium (CEF), and it needs a real Direct3D 11 device. Uncork gives it one with DXVK on MoltenVK, placed where only Steam finds it:

1. Before every start of the client, `steam::ensure_client_dxvk` copies DXVK's 64-bit `d3d11.dll` and `d3d10core.dll` from the `dxvk` component's `x86_64-windows/`, with Wine's builtin marker removed, into `C:\Program Files (x86)\Steam\bin\cef\cef.win64\`, the directory of `steamwebhelper.exe` (`client::CEF_DIR`). Identical files are not rewritten, and each file is replaced atomically. The DXVK used is the bottle's `graphics.dxvk` pin, else the newest installed, whichever command starts the client (`play`, `steam launch`, `steam start`, `steam install`, `setup`): they all go through `steam::prepare_client_dxvk`.
2. The client starts with `WINEDLLOVERRIDES=d3d12,d3d9,dxgi=b;winemenubuilder.exe=d;d3d10core,d3d11=n,b` and `DXVK_LOG_LEVEL=none`. `n,b` lets the web helper load the app-local DXVK; `b` keeps the `dxgi`, `d3d9` and `d3d12` a game's backend left in `system32` or `syswow64` (DXMT's `dxgi.dll`, for example) out of every Steam process.
3. Without that DXVK the client still starts, with a warning that its windows will be black and the fix: `uncork runtime install dxvk`, or, for a pinned version, `uncork runtime install dxvk --version <pin>` or unpinning it with `uncork bottle set <bottle> graphics.dxvk=`. `uncork doctor` warns when no DXVK is installed.

App-local DLLs are found before `system32` (Windows' DLL search order starts in the application's directory), so the client gets DXVK while `system32` and `syswow64` stay free for a game's backend. On 2026-10-07 the app-local placement behaved the same as copying DXVK into `system32`, and Steam's GPU process held the app-local `d3d11.dll` while Rise of Nations ran on DXMT's DLLs from `syswow64` in the same bottle. A fresh install's sign-in window rendered on it too.

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

A profile's `[launch] mode` chooses how a game starts; without a profile, `uncork play <appid>` uses `direct`. In every mode the profile's INI edits are applied first, and only to files that already exist. Before that, once the game is found and its launch planned, a running bottle whose main display has changed since its Wine session started is stopped (the client gets `-shutdown` and 15 s, as below), because Wine reads the displays only when its wineserver starts; the client then starts again with the game ([ARCHITECTURE.md](ARCHITECTURE.md#the-main-display)).

| Mode | What Uncork does | When |
|---|---|---|
| `direct` | If the client is not running: put DXVK in place, reset the `ActiveProcess` `ActiveUser` to 0, start the client with `-silent -nofriendsui`, and poll every 2 s until it reports running and an account has signed in (both within 180 s; [Waiting for the sign-in](#waiting-for-the-sign-in)). Then put the game's backend DLLs in place and start the game executable directly in the same prefix, with its own environment and DLL overrides, plus `SteamAppId` and `SteamGameId` set to the app id | Default. Works when the game does not need Steam to start it. Run on hardware: Rise of Nations, about 44 s from command to game with Steam starting first (2026-10-07) |
| `applaunch` | If the client runs, send it `-shutdown` and give it 15 s (`steam::SHUTDOWN_GRACE`) to exit, then stop the bottle with `wineserver --kill` and reset its `pid` ([Stopping](#detecting-and-stopping-the-client)). Put the game's backend DLLs and Steam's DXVK in place, then start `Steam.exe -silent -nofriendsui -applaunch <appid> <profile args> <args>` with the game's environment and wait until it reports running; the client then starts the game | Games whose DRM must be started by Steam. Unit-tested only |
| `standalone` | Start the executable without Steam | DRM-free and non-Steam games. Unit-tested only |

Steam sets `SteamAppId` and `SteamGameId` for the games it starts, and Steamworks reads them to know which app it is. A game started directly gets them from Uncork, so it does not need a `steam_appid.txt` and does not ask Steam to start it again with the client's environment. A value the bottle's `env`, the profile's `env` or `--env` sets wins; `--dry-run` shows them (`SteamAppId=287450` and `SteamGameId=287450` for Rise of Nations, checked 2026-10-07). In `applaunch` mode Steam sets them itself.

`direct` gives the game its own backend while Steam keeps its DXVK. `applaunch` cannot: a second `steam.exe` only forwards its arguments to the running client, and the game inherits the client's environment, not the caller's ([Highball SteamRestart.swift](https://github.com/gauthierpiarrette/highball/blob/main/Sources/HighballKit/SteamRestart.swift)). That is why Uncork restarts the client with the game's environment, keeping `d3d10core,d3d11=n,b` so the web helper still loads its DXVK. The game is unaffected, because its backend comes from `system32`/`syswow64`. The client may not be: with a DXMT game the web helper also inherits `dxgi=n,b`, loads DXMT's `dxgi` next to DXVK's `d3d11`, and that pairing fails to create swapchains, so Steam's own windows can stay black until the client is restarted normally. This follows from the DLL pairing in the table above; it has not been run.

Without a profile, the executable is the largest `.exe` at the shallowest level (up to three below the install directory) that has one, skipping names of launchers, crash reporters and redistributable installers (`client::NON_GAME_EXE_PREFIXES`). A profile's `[exe] path` overrides that. The profile is the one whose `[steam] appid` is the game's; when several profiles have that app id (a user profile next to a built-in with a different id, say), `uncork play <appid>` and `uncork steam launch <appid>` fail and name them, and `uncork play <profile id>` picks one.

What `--wait` waits for depends on the mode, because a running Steam client keeps the bottle's wineserver alive. In `direct` mode it waits for the game and not for the wineserver. In `applaunch` mode the process Uncork started is the Steam client, so `uncork play` labels its pid and log as Steam's, and `--wait` lasts until Steam exits. In `standalone` mode it waits for the game, then for the wineserver unless Steam runs in the bottle.

With `--game-mode` or `performance.game_mode = true`, `direct` and `standalone` launches go through a Game Mode app bundle ([PERFORMANCE.md](PERFORMANCE.md#game-mode)); in `applaunch` mode the client starts the game and no bundle is used. Game Mode is experimental and not recommended: through a bundle Rise of Nations crashed at startup in 3 of 5 launches on 2026-10-07.

## Detecting and stopping the client

Whether the client runs is read with `wine reg query HKCU\Software\Valve\Steam\ActiveProcess /v pid`, run with the bottle's environment so that a wineserver it starts gets the bottle's msync setting. A non-zero `pid` (printed as `    pid    REG_DWORD    0x274`) means running. `reg` exiting with status 1 after printing its own `reg:` message ("reg: Unable to find the specified registry key", or the same message in another language) means not running. A Wine client that exits with no such message, which is what a client whose `WINEMSYNC` differs from the running wineserver's does, is an error that shows its output and suggests `uncork bottle kill <bottle>`. Reading `user.reg` from disk is not reliable: Wine flushes it lazily.

The client sets the `pid` on start and clears it on a clean exit. A client that was killed, crashed or was force-quit, or that did not survive a restart of the Mac, leaves its `pid` behind, and Uncork would then treat Steam as running and start a game without it. Two things prevent that:

- Before the query, `steam::is_running` asks `wineserver -k0` whether a wineserver runs for the bottle. That is Wine's own lookup of the process holding the prefix's server lock, the one `wineserver --kill` uses, and it starts and stops nothing. Without a wineserver no client can be running, so a non-zero `pid` found then is stale: Uncork resets it and reports Steam as not running, and `uncork play` or `uncork steam start` starts the client.
- After Uncork kills a client, it writes the `pid` back to 0 with `wine reg add ... /t REG_DWORD /d 0 /f` (`steam::forget_client`).

`steam.exe -shutdown` is not a reliable way to stop the client: on 2026-10-07 it did not stop it within 60 s. Uncork therefore stops the client in steps (`steam::stop`): send `-shutdown`, poll every 2 s up to a timeout, then run `wineserver --kill`, which ends every process in the bottle, games included, and reset the `pid`. `applaunch` uses a 15 s timeout for this, so a client that ignores `-shutdown` costs 15 s rather than the 180 s start timeout. To stop Steam by hand, run `uncork bottle kill <bottle>`: it runs `wineserver --kill` at once and, when Steam is installed in the bottle, resets the `pid`, also when nothing was running (verified 2026-10-07).

The same steps stop a whole bottle when Uncork must restart it (`steam::stop_bottle`): before `uncork bottle set` changes Retina mode, the DPI or the Windows version, which Wine reads only when a wineserver starts, and before a game starts on a main display that changed while the bottle ran. After the client, `wineserver --kill` ends whatever is left, the `pid` is reset, and Uncork waits for the wineserver to exit.

### Waiting for the sign-in

A game started before Steam has logged on finds no Steam user. On 2026-10-08 Rise of Nations started that way 5 s after the client and 7 s before it logged on; it then quit and crashed in its exit code (`SteamAuthentication::CancelSteamAuthTicket`, a read of address 0). Before a `direct` launch Uncork therefore also waits for `HKCU\Software\Valve\Steam\ActiveProcess` `ActiveUser`, the signed-in account's 32-bit id, to be non-zero (`steam::wait_until_signed_in`). After 15 s it says it is still waiting and that a Steam window may be asking for a sign-in; if nobody signs in before the timeout, `uncork play` stops with an error that says to sign in in the Steam window. A client that records no `ActiveUser` at all is not waited for.

The value outlives the client. Polled once a second on 2026-10-08, a starting client set its `pid` at once but kept the previous session's `ActiveUser` for about 3 s, then set it to 0, logged on (`[Logged On` in `logs/connection_log.txt`) 4 s later, and wrote the id back within a second. Read in those first seconds, the old id looks like a sign-in. Uncork resets `ActiveUser` to 0 itself just before it starts the client, so only the new session's log-on counts.

### msync and Steam

The synchronization mode belongs to the wineserver, and a client whose `WINEMSYNC` differs from the server's exits with status 1 and no output. Steam, the games, the Steam installer and every Wine command Uncork runs in a bottle (`wineboot`, `regedit`, `reg`, `wineserver`) therefore share one setting ([ARCHITECTURE.md](ARCHITECTURE.md#environment)). On 2026-10-07 the first command after `uncork bottle create` failed that way because `wineboot` and `regedit` had run without the bottle's `WINEMSYNC`; since commit b8f7255 they run with it, and `steam install` right after `create` works. With the catalog's runtime msync is on by default, and on 2026-10-07 the client ran with `WINEMSYNC=1`, rendered its chrome and library, and ran next to Rise of Nations. The 2-hour session that M1 asks for has not been run yet. A web-UI hang under msync was reported on one non-CrossOver Wine 10 engine ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)) and has not been seen on Uncork's runtime.

## Commands

| Command | Does |
|---|---|
| `uncork steam install [--bottle B] [--yes]` | Finish an incomplete bottle, download Valve's installer (or reuse a verified cached one), install Steam, put DXVK in place and start the client with its window visible |
| `uncork steam start [--bottle B]` | Put the bottle's DXVK in place and start the client with its window visible; nothing if it already runs (a stale `pid` does not count) |
| `uncork steam games [--bottle B]` | List games installed in the bottle's Steam libraries, with size, state and matching profile |
| `uncork steam launch <appid> [launch options] [-- args]` | Launch an installed game by app id, with the profile that has that app id; several such profiles are an error that names them |
| `uncork play <game>` | Resolve a profile and launch through Steam as the profile says |
| `uncork bottle kill <bottle>` | Stop every Windows process in the bottle, Steam included, and reset Steam's `pid` |

The full option lists are in [CLI.md](CLI.md). Libraries are read from `steamapps/libraryfolders.vdf` and the `appmanifest_<appid>.acf` files; Windows paths in them are mapped into the prefix. A game counts as installed when its manifest's `StateFlags` has the fully-installed bit ([EAppState](https://github.com/OpenSteamClient/OpenSteamClient/blob/master/include/opensteamworks/EAppState.h)); a launch of a game that is not fully installed carries a warning.

## Known issues

| Issue | Symptom | What to do | Source |
|---|---|---|---|
| Store page | The embedded store page body stays black; the client chrome, sign-in and library list render | Use the library; open store pages in a Mac browser | Measured 2026-10-07 |
| Local network prompt | macOS asks whether the app that started Steam (your terminal or IDE) may "find devices on local networks" | Steam's LAN discovery asks for it; Uncork does not need it. Allow it if you use Steam's LAN features | Seen 2026-10-07 |
| `-shutdown` ignored | `steam.exe -shutdown` leaves the client running for over a minute | Uncork falls back to `wineserver --kill`; by hand, `uncork bottle kill <bottle>` | Measured 2026-10-07 |
| Steam crashed while a game still runs | Steam's `pid` is left behind while a wineserver still runs, so `uncork play` believes Steam runs and starts the next game without it. A `pid` left with no wineserver running is detected and reset | `uncork bottle kill <bottle>` | `steam::is_running`, `uncork bottle kill` |
| No DXVK | Every Steam window is black; `uncork doctor` warns | `uncork runtime install dxvk` | `steam::prepare_client_dxvk` |
| Steam asks to sign in again after `bottle import` | The clone, or the original, opens the sign-in window instead of signing in with the saved session; its connection log shows it connecting but never logging on. Seen when several copies of one bottle had used the same login, probably because Steam rotated the saved login | Import with `--move`, or sign in once more and from then on use only one copy ([Installing](#installing)) | Seen 2026-10-07 |
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
