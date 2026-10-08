# Performance

This document lists every setting Uncork uses to make games run fast on Apple Silicon: what each one does, its default, where it is set, and the evidence for it. It also records what has been measured so far, the pitfalls that silently cost performance, the open questions that still need measuring, and how to benchmark a change so the result can be trusted. Settings are only defaults when there is evidence for them; everything else is opt-in. Where a number comes from a third party it says so.

## Where settings come from

| Level | File or flag | Precedence |
|---|---|---|
| Bottle | `[performance]`, `env` and `dll_overrides` in `bottles/<name>/uncork.toml`; `uncork bottle set <name> performance.retina=true` | Lowest |
| Profile | `[wine]`, `[performance]`, `[env]` and `[dll_overrides]` in the game's profile ([profiles/README.md](../profiles/README.md)) | Overrides the bottle |
| Command line | `--hud`, `--metalfx`, `--retina`, `--game-mode`, `--wine-debug`, `--backend`, `-e KEY=VALUE` on `play`, `run` and `steam launch` ([CLI.md](CLI.md#uncork-play)). `steam start` accepts them too but applies only `--wine-debug` and `-e` to the client | Highest |

`uncork play <game> --dry-run` prints the resulting command and environment. Check it before and after changing a setting.

## Settings Uncork manages

| Setting | Default | Effect | Evidence |
|---|---|---|---|
| `performance.msync` | On. Effective only when the Wine runtime has the `msync` feature, which the catalog's runtime has | Sets `WINEMSYNC=1` for every process in the bottle, wineserver included | [msync](#msync) |
| `performance.avx` | On | Sets `ROSETTA_ADVERTISE_AVX=1` | [AVX under Rosetta](#avx-under-rosetta) |
| `performance.retina` | Off | Mac driver `RetinaMode` | [Retina mode](#retina-mode) |
| `performance.hud` | Off | Sets `MTL_HUD_ENABLED=1` | [Metal HUD](#metal-performance-hud) |
| `performance.metalfx` | Off | `DXMT_METALFX_SPATIAL_SWAPCHAIN=1` (DXMT) or `D3DM_ENABLE_METALFX=1` (D3DMetal) | [MetalFX](#metalfx) |
| `performance.game_mode` | Off (experimental, not recommended) | Launch through a games-category app bundle; `--game-mode` does it for one launch. Rise of Nations crashed at startup in 3 of 5 such launches | [Game Mode](#game-mode) |
| Logging | Off | `WINEDEBUG=-all`, `DXMT_LOG_LEVEL=none`, `DXVK_LOG_LEVEL=none`, `MVK_CONFIG_LOG_LEVEL=1`; `--wine-debug <channels>` turns Wine channels and backend logs on for one launch | [Logging](#logging) |
| Shader caches | Per bottle | `DXMT_SHADER_CACHE_PATH=<bottle>/cache/dxmt`, `DXVK_STATE_CACHE_PATH=<bottle>/cache/dxvk` | [Shader caches](#shader-caches) |
| DXVK pipeline compilation | Async | `DXVK_ASYNC=1`, `MVK_CONFIG_RESUME_LOST_DEVICE=1` | [DXVK](#dxvk-on-moltenvk) |
| WineD3D renderer | OpenGL | `WINE_D3D_CONFIG=renderer=gl` | [wined3d_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/wined3d/wined3d_main.c) |
| Cursor confinement, vertical sync, App Nap | Confine on, vsync allowed, App Nap off | Mac driver registry keys | [Mac driver keys](#mac-driver-keys) |
| Large address aware | Per profile (`WINE_LARGE_ADDRESS_AWARE=1` for Rise of Nations); a plan warning suggests it for a 32-bit program with a graphics API that lacks the flag | 4 GiB address space for a 32-bit game | [Large address aware](#large-address-aware) |

## Measured so far

First-hand numbers, 2026-10-07: MacBook Pro with Apple M5 Max, macOS 27.0.1 (26A434), Wine `winecx-gptk-4.7.3`, DXMT 0.80, DXVK-macOS 1.10.3, msync on, Retina off. They are single observations, not benchmarks run by [the procedure below](#how-to-benchmark).

| What | Result |
|---|---|
| Rise of Nations: Extended Edition, main menu, DXMT 0.80, windowed 1728x1117, Metal HUD | 120 FPS (the display's cap), GPU time 0.48 ms per frame, frame interval 8.33 ms; HUD reported "Composited" presentation and Game Mode off. Run by hand with the DLLs and overrides `uncork play` uses, with `Fullscreen=0` set in `rise2.ini` for the test |
| `uncork play rise-of-nations --hud`, Steam not running | About 44 s from the command to the game, Steam's start included |
| Signed-in Steam client, start to running (`ActiveProcess` `pid` set) | About 18 s, by hand |
| `uncork bottle create` | 13.8 s with Rosetta's translation cache warm; `wineboot -u` by hand on a fresh prefix took 29 s cold, and its wineserver exited at 33 s |
| `uncork runtime install all` from verified cached archives | 3.4 s for Wine, DXMT and DXVK |
| `uncork bottle import` of a 21 GB CrossOver bottle | About 6 s; APFS clones use almost no extra space until files change |
| `wine C:\windows\syswow64\cmd.exe /c ver` (32-bit) | 1.2 s, by hand |
| Fresh Steam install, first start to the sign-in window | About a minute, including the client's 336 MB download |

Not measured yet: a match or skirmish, the 99th-percentile frame time, WineD3D on the same scene, Game Mode (it needs full screen, which rendered cropped; see [Game Mode](#game-mode)), and a long session.

## msync

msync implements Windows synchronization objects with Mach semaphores instead of round trips to the wineserver. It was written by Marc-Aurel Zent at CodeWeavers and has shipped in CrossOver since 23.7.0 (November 2023). The CrossOver 26.3 Wine tree carries it in `server/msync.c` and `dlls/ntdll/unix/msync.c` and no longer carries esync ([winecx crossover-26.3.0](https://github.com/dappermint/winecx/tree/crossover-26.3.0/server)).

| Measurement (wine-msync README, M2 Max, CrossOver 23-based Wine) | Result |
|---|---|
| FFXIV, CPU-bound: msync with `__ulock_wait2` / msync / esync / server-side sync | 219 / 170 / 145 / 93 FPS |
| Contended wait benchmark, same four modes | 3.79 s / 5.89 s / 7.42 s / over 170 s |

Source: [marzent/wine-msync](https://github.com/marzent/wine-msync).

Rules:

- Every process of a wineserver must agree. A client whose `WINEMSYNC` differs from the server's logs an error and exits. Uncork therefore sets it in the base environment shared by Steam, games, tools and every other Wine command it runs in a bottle, `wineboot` and `regedit` included ([ARCHITECTURE.md](ARCHITECTURE.md#environment)). When those two ran without it, the first command after `uncork bottle create` exited with status 1 and no output (2026-10-07, fixed in commit b8f7255). A profile that overrides `wine.msync` needs the bottle's wineserver restarted (`uncork bottle kill <bottle>`) when the value differs from what is running.
- `WINEMSYNC_QLIMIT` sizes the server's message queue (default 50). There is no evidence for changing it.
- `WINEESYNC` is never set: esync is gone from the CrossOver 26 tree, and wine-staging dropped it at v10.16 ([wine-staging](https://github.com/wine-staging/wine-staging/tree/v11.18/patches)).
- Stock msync was slower than no msync on multi-object waits until fixes in the frankea/dappermint line: four-way multi-wait 366 to 84 ms, alertable wait 293 to 61 ms (their microbenchmarks, not independently reproduced; [frankea/Whisky v4.6.4-beta.1](https://github.com/frankea/Whisky/releases/tag/v4.6.4-beta.1)). The catalog's `winecx-gptk-4.7.3` comes from the dappermint line ([release](https://github.com/dappermint/winecx-gptk/releases/tag/runtime-v4.7.3)); Uncork's own runtime build (M2) is to carry the fixes as a patch ([RUNTIME.md](RUNTIME.md#patch-queue)).
- Uncork sets `WINEMSYNC=1` only when the Wine runtime is tagged `msync`. The catalog's only runtime, `winecx-gptk-4.7.3`, is: its `ntdll.so` contains `WINEMSYNC` and `WINEMSYNC_SPINS`, and Wine prints `msync: up and running.` at start. With the default `performance.msync = true`, every launch, the Steam client and Uncork's `wine reg` queries run with `WINEMSYNC=1` (checked in the `--dry-run` command on 2026-10-07).
- A Steam web UI hang under msync was reported on one non-CrossOver Wine 10 engine ([highball-db](https://github.com/gauthierpiarrette/highball-db/blob/main/recipes/launchers/steam.json)). On `winecx-gptk-4.7.3` with msync on, the client rendered its chrome and library and ran next to Rise of Nations (2026-10-07); a 2-hour session has not been run yet.

## AVX under Rosetta

Rosetta translates AVX and AVX2 but not AVX-512 ([Apple](https://developer.apple.com/documentation/apple-silicon/about-the-rosetta-translation-environment)). By default it does not advertise AVX in CPUID, so a game that checks CPUID before using AVX takes a slower path or refuses to start. `ROSETTA_ADVERTISE_AVX=1` changes only what CPUID reports; execution is the same either way. Apple's GPTK 4.0 beta 2 Read Me documents the variable as default off, for macOS 15 and later ([Read Me](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/Read%20Me.pdf)).

- The variable is read when a process starts and inherited by its children. A running Steam client keeps the old value, so restart Steam after changing it if games are started through `-applaunch`.
- Never infer AVX support from `sysctl hw.optional.avx*`: under Rosetta those read 0 whether or not AVX is advertised, although AVX and AVX2 execute.
- Turn it off per game (`[performance] avx = false`) if a game misbehaves with AVX advertised; no such game is known yet.

## Retina mode

With `RetinaMode` on, Wine's Mac driver gives Windows programs the full backing-pixel resolution, so a game renders up to four times the pixels (twice in each direction). The key is read only prefix-wide, because DPI must agree across processes ([macdrv_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/winemac.drv/macdrv_main.c)). Default off. Turn it on for a game whose UI scales and that has GPU headroom; leave it off for pixel-based UIs such as Rise of Nations'. Because it is prefix-wide it also changes the Steam client. For the same reason a launch never changes it: `uncork bottle set <bottle> performance.retina=true` rewrites the bottle's registry, while `--retina` or a profile's `retina` that differs from the bottle's only adds a warning to the plan.

Full screen on a high-resolution display is an open problem. On 2026-10-07 Rise of Nations in full screen on a 5K external display, with Retina mode off, rendered cropped and offset; windowed (`Fullscreen=0` in its `rise2.ini`) rendered correctly. Whether Retina mode, `CaptureDisplaysForFullscreen` or the game's resolution setting changes that has not been tested ([ROADMAP.md](ROADMAP.md#open-questions)). Play windowed for now.

## Metal performance HUD

`--hud` or `performance.hud` sets `MTL_HUD_ENABLED=1`, Apple's on-screen HUD with frame rate, frame time and GPU time. It works for every Metal-backed layer: DXMT, D3DMetal and MoltenVK. Apple documents further variables ([Monitoring your Metal app's graphics performance](https://developer.apple.com/documentation/xcode/monitoring-your-metal-apps-graphics-performance), [Customizing the Metal Performance HUD](https://developer.apple.com/documentation/xcode/customizing-metal-performance-hud)):

| Variable | Use |
|---|---|
| `MTL_HUD_LOG_ENABLED=1` | Log per-frame statistics as `metal-HUD:` lines; the basis of benchmarking |
| `MTL_HUD_ELEMENTS=device,rosetta,fps,frameinterval,gputime,memory,shaders` | Choose what the HUD shows; `rosetta` confirms the process is translated |
| `MTL_HUD_SCALE`, `MTL_HUD_OPACITY`, `MTL_HUD_ALIGNMENT` | Size, opacity and position |

Pass the extra variables with `-e`, for example `uncork play rise-of-nations --hud -e MTL_HUD_LOG_ENABLED=1`. The HUD itself costs a little; leave it off when not measuring.

## MetalFX

| Backend | Variable | Bitness | Notes | Source |
|---|---|---|---|---|
| DXMT | `DXMT_METALFX_SPATIAL_SWAPCHAIN=1` | 32 and 64 | Spatial upscale of the final swapchain image (factor set with `d3d11.metalSpatialUpscaleFactor` in `DXMT_CONFIG`). It also scales 2D UI, which can blur; opt-in | [DXMT CUSTOMIZATION.md](https://github.com/3Shain/dxmt/blob/main/docs/CUSTOMIZATION.md) |
| DXMT | `DXMT_ENABLE_NVEXT=1` | 64 only | Maps DLSS to the MetalFX temporal upscaler; set per game in a profile, never bottle-wide | [DXMT Vendor Extensions](https://github.com/3Shain/dxmt/wiki/Vendor-Extensions) |
| D3DMetal | `D3DM_ENABLE_METALFX=1` | 64 only | Converts DLSS to MetalFX on macOS 26 and later. It also needs GPTK's `nvngx` and `nvapi64` modules in the prefix, which Uncork does not install yet | [GPTK 4.0b2 Read Me](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/Read%20Me.pdf) |

`--metalfx` and `performance.metalfx` set the first and third rows. WineD3D and DXVK on MoltenVK have no MetalFX path.

## Frame pacing and backend knobs

These are not set by default; put them in a profile's `[env]` or pass them with `-e`.

| Variable | Effect | Source |
|---|---|---|
| `DXMT_CONFIG="d3d11.preferredMaxFrameRate=60;"` | Caps the frame rate inside DXMT with Metal-controlled pacing. The value must divide the display's refresh rate (60 or 120 on a 120 Hz display) | [DXMT CUSTOMIZATION.md](https://github.com/3Shain/dxmt/blob/main/docs/CUSTOMIZATION.md) |
| `D3DM_MAX_FPS=<n>` | Frame-rate cap in D3DMetal | [GPTK 4.0b2 Read Me](https://github.com/Sikarugir-App/Sikarugir/blob/main/D3DMetal/4.0/Read%20Me.pdf) |
| `D3DM_SUPPORT_DXR` | DirectX ray tracing in D3DMetal; default off on M1 and M2, on from M3 | same |
| `D3DM_MTL4=0` | On macOS 27, D3DMetal's Direct3D 12 path uses Metal 4 by default; `0` falls back to Metal 3 | same |
| `WINE_D3D_CONFIG` | WineD3D options such as `renderer=gl,csmt=0x1` | [wined3d_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/wined3d/wined3d_main.c) |

## DXVK on MoltenVK

Gcenx's DXVK-macOS 1.10.3 is frozen and carries the async pipeline patch; `DXVK_ASYNC=1` compiles pipelines in the background, trading a frame or two of missing effects for fewer compile stutters ([DXVK-macOS](https://github.com/Gcenx/DXVK-macOS)). `MVK_CONFIG_RESUME_LOST_DEVICE=1` lets MoltenVK continue after a lost device instead of ending the game ([MoltenVK configuration](https://github.com/KhronosGroup/MoltenVK/blob/v1.4.2/Docs/MoltenVK_Configuration_Parameters.md)). For games DXVK is a fallback only: MoltenVK has no geometry shaders, and upstream DXVK 2.x and 3.x need Vulkan features MoltenVK lacks. It is also what renders the Steam client's web UI ([STEAM.md](STEAM.md#graphics-steam-runs-on-dxvk)); the client gets `DXVK_LOG_LEVEL=none` but none of the game-side DXVK variables.

## Shader caches

| Backend | Cache | Notes |
|---|---|---|
| DXMT | `<bottle>/cache/dxmt` (`DXMT_SHADER_CACHE_PATH`) | DXMT's default is `$(getconf DARWIN_USER_CACHE_DIR)/dxmt/<exe>`; per bottle keeps caches with the prefix they belong to ([CUSTOMIZATION.md](https://github.com/3Shain/dxmt/blob/main/docs/CUSTOMIZATION.md)) |
| DXVK | `<bottle>/cache/dxvk` (`DXVK_STATE_CACHE_PATH`) | |
| D3DMetal | `$(getconf DARWIN_USER_CACHE_DIR)/d3dm/<game>/shaders.cache` | Not reused between GPTK 3 and 4 |

The first run after a game update or a backend update rebuilds shaders and stutters. Discard that run when benchmarking. Deleting a cache directory is safe; it is rebuilt.

## Logging

Logging is off in every launch: `WINEDEBUG=-all`, `DXMT_LOG_LEVEL=none`, `DXVK_LOG_LEVEL=none`, and `MVK_CONFIG_LOG_LEVEL=1` (MoltenVK errors only; it otherwise prints `[mvk-info]` device lines on every start). Wine's debug channels are printed by every process to its standard error and cost real frame time when enabled. `--wine-debug +loaddll` (or any channel list) turns Wine's channels on and sets the backends' log levels to `info` for that launch only; output goes to the launch log in `$UNCORK_HOME/logs/`.

## Mac driver keys

Uncork writes these under `HKEY_CURRENT_USER\Software\Wine\Mac Driver` when it creates a bottle. Several match Wine's own defaults; writing them makes a bottle's behavior independent of the Wine version ([macdrv_main.c](https://gitlab.winehq.org/wine/wine/-/blob/master/dlls/winemac.drv/macdrv_main.c)).

| Key | Uncork writes | Why |
|---|---|---|
| `RetinaMode` | `n` unless `performance.retina` | See [Retina mode](#retina-mode) |
| `UseConfinementCursorClipping`, `CursorClippingLocksWindows` | `y` | Keeps the cursor inside the window, which RTS edge scrolling needs |
| `AllowVerticalSync` | `y` | Lets games' vsync settings reach Metal |
| `EnableAppNap` | `n` | App Nap throttles a backgrounded Steam client |

`CaptureDisplaysForFullscreen` is left at Wine's default (`n`); whether `y` helps exclusive-fullscreen games is an open question.

## Large address aware

A 32-bit program without the `IMAGE_FILE_LARGE_ADDRESS_AWARE` flag gets 2 GiB of address space; with it, a 32-bit process under new WoW64 gets 4 GiB ([Proton virtual.c](https://github.com/ValveSoftware/wine/blob/proton_10.0/dlls/ntdll/unix/virtual.c)). `uncork inspect` shows the flag. Rise of Nations: Extended Edition's executable lacks it, so its profile sets `WINE_LARGE_ADDRESS_AWARE=1`.

When a 32-bit program lacks the flag, the runtime honors the variable and nothing sets it, the launch plan carries a warning that suggests it. The warning is limited to programs with a graphics API, so games get it and Windows tools such as `cmd.exe` or `winecfg` do not.

CrossOver-derived Wine honors that variable (`CROSSOVER HACK: bug 17634` in [virtual.c](https://github.com/dappermint/winecx/blob/crossover-26.3.0/dlls/ntdll/unix/virtual.c)); it is off unless set. Upstream Wine honors only the PE flag. The catalog marks runtimes that honor it with the `large-address-aware` feature; on others the variable does nothing.

## DEP and NX_COMPAT

This is the most expensive pitfall known for 32-bit games under Rosetta.

- Wine turns no-exec (DEP) off for the whole process when any loaded module lacks `IMAGE_DLLCHARACTERISTICS_NX_COMPAT`, and logs `disabling no-exec because of <module>` ([loader.c](https://github.com/wine-mirror/wine/blob/master/dlls/ntdll/loader.c)). For a WoW64 process that makes every readable mapping executable.
- Under Rosetta that is slow. First-touch page faults measured about 200 times costlier, and one game's boot went from 41.7 s to 16.9 s with the check disabled ([athei/wine-build#3](https://github.com/athei/wine-build/issues/3)). On macOS 26, 32-bit games on DXMT become slideshows when DEP is off, because Metal's placed buffers end up executable ([dxmt#161](https://github.com/3Shain/dxmt/issues/161)).
- `uncork inspect <exe>` reports the modules beside the game that lack `NX_COMPAT` (the scan's `non_nx_modules`). Run a game with `--wine-debug warn+module` and search the launch log for `disabling no-exec because of` to see which module triggered it at run time.
- Rise of Nations: Extended Edition ships 11 such DLLs beside `riseofnations.exe`, and every plan for it carries a warning naming them: `avutil-ttv-51.dll`, `Eulaxp1.dll`, `libmp3lame-ttv.dll`, `patchw32.dll`, `PidGenx.dll`, `pp_unicows.dll`, `rtp32cb.dll`, `SteamAPIUpdater.dll`, `swresample-ttv-0.dll`, `unicows.dll`, `UpdateDLLWrapper.dll`. Whether the game loads any of them, and so runs with DEP off, has not been checked with `warn+module` yet. The warning is printed for 32-bit programs only.
- Uncork's own runtime (M2) carries a patch that makes the main executable decide, as Windows does ([athei/wine 539aa62220](https://github.com/athei/wine/commit/539aa62220)), and every PE file Uncork ships must be `NX_COMPAT` ([RUNTIME.md](RUNTIME.md#patch-queue)).
- A `WINE_DISABLE_NX_COMPAT` escape hatch existed in one Gcenx build (11.6) and was removed; do not rely on it.

## Game Mode

macOS gives a game Game Mode (priority CPU and GPU access, lower Bluetooth latency) when an app whose bundle declares the games category is frontmost and full screen; it does not activate for binaries spawned from a terminal ([Apple Support](https://support.apple.com/en-us/105118), [macOS 26 release notes](https://developer.apple.com/documentation/macos-release-notes/macos-26-release-notes), [LSSupportsGameMode](https://developer.apple.com/documentation/bundleresources/information-property-list/lssupportsgamemode)). Neither Wine's loader nor CrossOver declares a games category ([wine_info.plist.in](https://gitlab.winehq.org/wine/wine/-/blob/master/loader/wine_info.plist.in)).

`--game-mode` on one launch, or `performance.game_mode = true` for a bottle (`uncork bottle set <bottle> performance.game_mode=true`), routes `direct` and `standalone` launches through a generated `apps/<id>.app` that LaunchServices opens with `/usr/bin/open -n -W`; its launcher `exec`s the Wine loader in place ([ARCHITECTURE.md](ARCHITECTURE.md#game-mode-experimental)). In `applaunch` mode the Steam client starts the game and no bundle is used. The manual run in [Measured so far](#measured-so-far) did not use a bundle, and the Metal HUD reported Game Mode off.

Results on 2026-10-07 (M5 Max, macOS 27.0.1, Rise of Nations on DXMT 0.80):

| What | Result |
|---|---|
| The bundle's launcher under LaunchServices | Works: a plan that ran `/usr/bin/env` through the bundle showed the plan's environment |
| Rise of Nations through the bundle | Crashed at startup in 3 of 5 launches, with an unhandled page fault reading address 0 in the game's own code. It never crashed in 6 or more direct launches. With Wine's `loaddll` and exception-handling debug channels on, it did not crash through the bundle either, so the crash depends on timing |
| Game Mode's effect | Not measurable: Game Mode needs full screen, and Rise of Nations in full screen rendered cropped and offset on a 5K external display with Retina mode off. Windowed works |

So Game Mode stays experimental, off by default and not recommended. Open questions, to answer on real hardware:

1. Why does Rise of Nations crash at startup when started through the bundle, and only then?
2. Does macOS keep Game Mode for the bundle after its launcher `exec`s the Wine loader?
3. Does Game Mode turn on with the Mac driver's window-level full screen, and does `CaptureDisplaysForFullscreen=y` change that? This needs full screen that renders correctly first ([Retina mode](#retina-mode)).
4. Is there a measurable frame-time difference on Rise of Nations with Game Mode on versus off?

## Not used, and why

| Option | Why not |
|---|---|
| esync (`WINEESYNC`) | Removed from the CrossOver 26 tree and from wine-staging; msync replaces it |
| rosettax87 | Replaces Rosetta's x87 handlers; needs the debugger entitlement or SIP debug restrictions disabled ([rosettax87_jit](https://github.com/Sikarugir-App/rosettax87_jit)). Uncork does not weaken system security |
| Global MetalFX or NVEXT | Spatial upscaling blurs 2D UI, and `DXMT_ENABLE_NVEXT=1` enables DXMT's NVAPI and NGX shims and writes NVIDIA registry keys ([CUSTOMIZATION.md](https://github.com/3Shain/dxmt/blob/main/docs/CUSTOMIZATION.md)), which changes how every program in the bottle behaves; per game only |
| Steam overlay in games | Its Direct3D 11 hook is reported to deadlock games under DXMT. Disable it per profile with `gameoverlayrenderer = ""` under `[dll_overrides]` if a game hangs ([macos-wine-steam#8](https://github.com/ByMedion/macos-wine-steam/pull/8)) |

Long sessions: Wine bug 59476 reports fragmentation of the reserved address range exhausting memory on macOS 26 ([bug 59476](https://bugs.winehq.org/show_bug.cgi?id=59476)); unconfirmed. If a game slows down after hours, record `vmmap -summary <pid>` region counts in your report.

## How to benchmark

A number without its conditions is noise. Change one thing at a time and record everything below.

1. Fix the conditions: on AC power, Low Power Mode off, other apps closed, the same display and refresh rate, the same in-game resolution and settings, Retina off unless that is what you test.
2. Record provenance: `uncork --version` (or the commit), `uncork runtime list`, `sw_vers -productVersion` and `sw_vers -buildVersion`, `sysctl -n machdep.cpu.brand_string`, the game's Steam build id, and the backend and its version.
3. Save the plan: `uncork play <game> --dry-run > plan.txt`.
4. Run with HUD logging:

   ```bash
   uncork play <game> --hud \
     -e MTL_HUD_LOG_ENABLED=1 \
     -e MTL_HUD_ELEMENTS=device,rosetta,fps,frameinterval,gputime,memory,shaders
   ```

5. Discard the first run (shader caches). Then run the same scene three times for at least 60 seconds each: a saved game, a replay or a fixed skirmish start.
6. Collect the `metal-HUD:` lines. Look in the launch log in `$UNCORK_HOME/logs/` first; if they are not there, capture the unified log while the game runs: `log stream --style compact --predicate 'eventMessage CONTAINS "metal-HUD"' > hud.log`.
7. Report the median FPS and the 99th-percentile frame interval per run, not a single average. An A/B result counts when the difference is larger than the spread between your three runs.

Useful A/B pairs: `--backend dxmt` versus `--backend wined3d`; msync on versus off (`uncork bottle set <bottle> performance.msync=false`, then `uncork bottle kill <bottle>` so Steam and the game restart together, on a runtime with `msync`); `--retina`; `--metalfx`. `uncork bench` (M3) will automate steps 2 to 7 and store results with their provenance.
