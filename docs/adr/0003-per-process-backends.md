# 0003: Choose the graphics backend per process

Status: Accepted; decision 2 superseded by [0005](0005-steam-cef-on-app-local-dxvk.md)
Date: 2026-10-07

Hands-on testing showed that the Steam client does not work on WineD3D with its web UI in software: every window stays black. [0005](0005-steam-cef-on-app-local-dxvk.md) gives the client app-local DXVK instead. The rest of this record stands. With the catalog's current Wine runtime every DXMT and DXVK launch uses `PrefixNative` ([RUNTIME.md](../RUNTIME.md#phase-0-pinned-upstream-builds)).

This record decides that Uncork chooses the Direct3D translation layer for each process it starts, not for a whole bottle, and how that choice is wired into Wine depending on what the Wine runtime supports.

## Context

Two facts collide in every Steam bottle.

The Steam client cannot run on the fast backends. Its web UI is Chromium, whose GPU process presents into a window owned by another process. DXMT cannot present across processes ([dxmt#141](https://github.com/3Shain/dxmt/issues/141)); Wine's Mac driver does not implement cross-process child-window Metal swapchains; on D3DMetal the client stays black ([Highball#282](https://github.com/gauthierpiarrette/highball/issues/282)); and DXMT's `dxgi` left behind under DXVK's `d3d11` breaks the client's swapchain with `0x887A0004` ([frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163)). The client works on Wine's own WineD3D with its web UI in software.

The game needs a fast backend and must share the client's prefix. Steamworks finds the client through the prefix's registry and needs the same wineserver ([steam_api](https://partner.steamgames.com/doc/api/steam_api)). Rise of Nations is a 32-bit Direct3D 11 game; the only Metal-native path for it is DXMT.

So two processes in one prefix must load different `d3d11.dll` and `dxgi.dll` files at the same time. Wine's obvious tools do not do this:

- `WINEDLLPATH` cannot override a builtin: Wine's loader searches its own DLL directory first ([loader.c, wine-11.19](https://github.com/wine-mirror/wine/blob/wine-11.19/dlls/ntdll/unix/loader.c)).
- `WINEDLLOVERRIDES` applies to a process and its children alike, and Steam's helpers are its children.
- Copying a backend into `system32` makes it prefix-wide unless every other process is explicitly told to ignore it.

Runtimes differ in what they offer. Gcenx's Sikarugir 11.0 engine honors `WINEDLLPATH_DXMT`, `WINEDLLPATH_DXVK` and `WINEDLLPATH_D3DMETAL`, searched before the builtins for that process only ([Sikarugir#283](https://github.com/Sikarugir-App/Sikarugir/issues/283)). CrossOver's Wine has `prepend_dll_path()`, which its proprietary compatibility layer calls per process and which nothing in the LGPL tree calls ([cx loader.c](https://github.com/dappermint/winecx/blob/crossover-26.3.0/dlls/ntdll/unix/loader.c)). Sikarugir's older CrossOver-based engines honor `WINEDLLPATH_PREPEND`, and Highball uses it for per-launch overlays ([Highball](https://github.com/gauthierpiarrette/highball)). Other runtimes offer neither.

## Decision

1. The backend is a property of a launch, not of a bottle. A bottle's `graphics.backend` and a profile's `graphics.backend` are defaults for the processes Uncork starts.
2. The Steam client and its helpers always run on WineD3D, with `d3d11,dxgi,d3d10core,d3d9,d3d12=b`.
3. Each Wine runtime declares how it can be wired in through catalog feature tags, and `graphics::strategy_for` picks the first that applies: `RendererEnv` (`renderer-dllpath`), then `DllPathPrepend` (`dllpath-prepend`), then `PrefixNative` (copy marker-stripped DLLs into `system32`/`syswow64` and set `n,b` for the game only). WineD3D needs nothing (`Builtin`).
4. Every launch sets the DLLs other backends replace to `b`, so a native DLL left in the prefix by an earlier launch never loads by accident.
5. `PrefixNative` is refused for D3DMetal: its PE files forward to a Unix library, and removing the builtin marker loses that half.
6. DXMT's `winemetal.dll` is placed in `system32` and `syswow64` with its marker intact in every strategy, as DXMT's guide requires ([guide](https://github.com/3Shain/dxmt/wiki/DXMT-Installation-Guide-for-Geeks)).
7. Uncork's own runtime (M2) carries a `WINEDLLPATH_PREPEND` patch so that the preferred runtime always supports a per-process strategy ([RUNTIME.md](../RUNTIME.md#patch-queue)).

## Alternatives considered

| Alternative | Why not |
|---|---|
| One backend per bottle, with Steam in a separate bottle | Steamworks needs the client in the same prefix and wineserver as the game |
| Install the backend in `system32` for the whole bottle | Steam's client then loads it and breaks; CrossOver itself selects backends per process |
| Per-executable `AppDefaults\<exe>\DllOverrides` in the registry | Works only with marker-stripped native copies, cannot express D3DMetal (which must stay builtin), and turns a launch setting into persistent prefix state that outlives the launch |
| Always use `PrefixNative` | Prefix-wide files and state for every launch, and no D3DMetal |
| Patch only, require Uncork's own runtime | Leaves phase 0 without per-process selection; the Sikarugir engine already supports it |

## Consequences

- Steam and a game run side by side in one bottle with different backends, in `direct` launch mode. In `applaunch` mode the game inherits the Steam client's environment, so Uncork restarts Steam with the game's environment first; whether Steam's helpers still render then is an open question ([ROADMAP.md](../ROADMAP.md#open-questions)).
- Catalog feature tags are load-bearing. A wrong `renderer-dllpath` or `dllpath-prepend` tag silently gives a game Wine's builtins; pins must verify tags ([RUNTIME.md](../RUNTIME.md#updating-a-pin)).
- `PrefixNative` writes into the prefix; Uncork records every file it copied in `uncork-state.toml`, skips identical files and never deletes Wine's placeholders.
- `uncork play --dry-run` shows the strategy, the variables and the overrides of a launch, which makes backend problems reportable.
