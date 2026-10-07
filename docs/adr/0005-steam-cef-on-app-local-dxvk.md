# 0005: Give the Steam client's web UI app-local DXVK

Status: Accepted
Date: 2026-10-07

This record replaces decision 2 of [0003](0003-per-process-backends.md), which kept the Steam client on WineD3D with its web UI rendered in software. Hands-on testing showed that this leaves every Steam window black. It decides which Direct3D layer the client gets instead, and where those DLLs live so that they never meet a game's backend.

## Context

- Steam's web UI is Chromium (CEF). With `-cef-disable-gpu -cef-disable-gpu-compositing` and WineD3D, every Steam window stayed black: software-composited CEF output does not cross Wine's process boundary on macOS. Measured on 2026-10-07 with client build 1788652215, Wine `winecx-gptk-4.7.3` (wine-11.17) and macOS 27.0.1 on an M5 Max ([STEAM.md](../STEAM.md#graphics-steam-runs-on-dxvk)).
- With DXVK-macOS 1.10.3's `d3d11.dll` and `d3d10core.dll` (builtin marker removed, loaded native) and Wine's builtin `dxgi`, the client chrome, sign-in and library rendered. The embedded store page body stayed black. Same date and setup.
- DXMT cannot present across processes, which Chromium's GPU process needs ([dxmt#141](https://github.com/3Shain/dxmt/issues/141)); on D3DMetal the client stays black ([Highball#282](https://github.com/gauthierpiarrette/highball/issues/282)); DXMT's `dxgi` paired with DXVK's `d3d11` fails to create swapchains ([frankea/Whisky#163](https://github.com/frankea/Whisky/issues/163)).
- The catalog's only Wine runtime has no per-process DLL path, so a game's backend is copied into `system32` and `syswow64` (`PrefixNative`, [ARCHITECTURE.md](../ARCHITECTURE.md#activation-strategies)). A Steam DXVK placed there would collide with a game's DXMT.
- Windows, and Wine with it, searches a program's own directory before `system32`. DXVK copied into `Steam/bin/cef/cef.win64`, the web helper's directory, behaved the same as DXVK in `system32` (2026-10-07).

## Decision

1. The Steam client's web UI runs on DXVK on MoltenVK. Before every client start, Uncork copies DXVK's 64-bit `d3d11.dll` and `d3d10core.dll`, builtin marker removed, into `C:\Program Files (x86)\Steam\bin\cef\cef.win64\` (`steam::ensure_client_dxvk`).
2. The client starts with `d3d10core,d3d11=n,b` and `d3d12,d3d9,dxgi=b`, so it loads the app-local DXVK and never a `dxgi`, `d3d9` or `d3d12` a game's backend left in `system32` or `syswow64`.
3. Uncork never passes `-cef-disable-gpu` or `-cef-disable-gpu-compositing`.
4. Without a DXVK component the client still starts, with a warning that its windows will be black.

Decisions 1 and 3 to 7 of [0003](0003-per-process-backends.md) stand: backends are chosen per launch, and games keep `system32`/`syswow64`.

## Alternatives considered

| Alternative | Why not |
|---|---|
| WineD3D with software CEF (0003's decision) | Every window black (measured) |
| WineD3D with GPU CEF | WineD3D's OpenGL path gives Chromium too low a Direct3D feature level |
| DXVK copied into `system32` for the client | Works, but takes the slots `PrefixNative` needs for a game's DXMT, and DXMT's `dxgi` next to DXVK's `d3d11` breaks the client |
| DXMT or D3DMetal for the client | Cannot present across processes; black client |
| CrossOver 24's `steamwebhelper` argument injection (`--disable-gpu` and others) in Uncork's runtime | Software rendering again; kept in the patch queue only in case the M2 runtime needs it ([RUNTIME.md](../RUNTIME.md#patch-queue)) |

## Consequences

- Steam and a game on DXMT run side by side in one bottle (seen on 2026-10-07: the web helper held the app-local DXVK while Rise of Nations held DXMT's DLLs from `syswow64`).
- DXVK is no longer optional for a Steam bottle; `uncork setup` installs it. `doctor` still reports a missing DXVK as information only.
- Uncork adds two files to the Steam client's directory. Valve's own files are not modified ([LEGAL.md](../LEGAL.md#steam)). The DLLs are rewritten before each start, so a client update or file check that removes them is repaired on the next start.
- In `applaunch` mode the client inherits a game's environment. With a DXMT game the web helper then gets DXMT's `dxgi` next to DXVK's `d3d11`, and Steam's windows can stay black until the client is restarted normally ([STEAM.md](../STEAM.md#launch-modes)).
- The client depends on DXVK-macOS 1.10.3, a frozen fork. The store page body stays black with it; why is an open question ([ROADMAP.md](../ROADMAP.md#open-questions)).
