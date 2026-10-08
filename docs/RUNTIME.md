# Runtime supply

This document says where Uncork's Wine runtime and graphics components come from, today and after Uncork builds its own; what the runtime feature tags mean and how the launcher uses them; which files each component must contain; how to update a pinned component; and which patches Uncork's own Wine build carries and why. The decision behind the supply model is [ADR 0004](adr/0004-runtime-supply.md). The legal side is in [LEGAL.md](LEGAL.md).

## Phases

| Phase | Milestone | Wine runtime | Graphics components |
|---|---|---|---|
| 0 | Now | Pinned upstream builds, downloaded from their publishers | DXMT 0.80 and DXVK-macOS 1.10.3 from their releases; D3DMetal imported by the user |
| 1 | M2 | Uncork's own CI build from CrossOver 26.3's LGPL Wine sources plus a small patch queue | DXMT built from `main` (LGPL) with source; DXVK-macOS rebuilt with `NX_COMPAT`; D3DMetal still imported |

## Phase 0: pinned upstream builds

`runtime/catalog.toml` is compiled into the binary and lists every archive `uncork runtime install` may download. `uncork runtime available` prints it.

| Kind | Version | Size | License | Features | Recommended | Publisher |
|---|---|---|---|---|---|---|
| `wine` | `winecx-gptk-4.7.3` | 461,131,598 B | LGPL-2.1-or-later | `wow64`, `msync`, `dxmt`, `d3dmetal`, `large-address-aware` | Yes | [dappermint/winecx-gptk](https://github.com/dappermint/winecx-gptk/releases/tag/runtime-v4.7.3), source [dappermint/winecx](https://github.com/dappermint/winecx) |
| `dxmt` | `0.80` | 18,681,669 B | MIT | | Yes | [3Shain/dxmt v0.80](https://github.com/3Shain/dxmt/releases/tag/v0.80) |
| `dxvk` | `1.10.3-20230507` | 2,785,833 B | Zlib | | Yes | [Gcenx/DXVK-macOS](https://github.com/Gcenx/DXVK-macOS/releases/tag/v1.10.3-20230507-repack) |

Together the three are 482,599,100 B (482.6 MB); `uncork setup` adds Valve's `SteamSetup.exe`, about 2.4 MB, for 485.0 MB in all.

### The Wine runtime

`winecx-gptk-4.7.3` is CrossOver 26.3's Wine changes rebased onto Wine 11.17, with msync and its fixes ([release](https://github.com/dappermint/winecx-gptk/releases/tag/runtime-v4.7.3)). What was checked on the installed runtime, macOS 27.0.1 on an M5 Max, 2026-10-07:

| Fact | How it was checked |
|---|---|
| `bin/wine --version` prints `wine-11.17` | Ran it |
| The archive nests the tree under `Libraries/Wine`; `bin/wine`, `bin/wine64` and `bin/wineloader` are symlinks to `../lib/wine/x86_64-unix/wine`. Install moves the tree to the top of the component directory | Archive listing; `uncork runtime install` |
| The libraries it needs ship in its own `lib/` (the catalog note lists FreeType, GnuTLS, MoltenVK 1.4.2 and GStreamer) | Archive listing |
| `lib/wine/x86_64-unix/winemetal.so` is DXMT 0.80's Unix bridge, which `PrefixNative` activation of DXMT requires | Archive listing; DXMT 0.80 loaded with it in Rise of Nations |
| `ntdll.so` contains `WINEMSYNC`, `WINEMSYNC_SPINS`, `WINE_LARGE_ADDRESS_AWARE` and `CX_APPLEGPTK_LIBD3DSHARED_PATH`; Wine prints `msync: up and running.` at start | `strings`; stderr |
| `ntdll.so` has no `WINEDLLPATH_PREPEND` or `WINEDLLPATH_<RENDERER>` variables, only Wine's usual message that a non-builtin found in `WINEDLLPATH` is ignored. So it gets neither `renderer-dllpath` nor `dllpath-prepend`, and Uncork activates backends with `PrefixNative` | `strings` |
| Built for macOS 26.0 and later (`minos 26.0`, SDK 27.0), so it does not run on older macOS | `otool -l` on `lib/wine/x86_64-unix/wine` and `ntdll.so` |
| A new prefix takes 29 s (`wineboot -u`, cold Rosetta cache) and reports Windows 10.0.19045; 32-bit programs run (`wine C:\windows\syswow64\cmd.exe /c ver`, 1.2 s) | Ran it |
| MoltenVK prints `[mvk-info]` lines on every start unless `MVK_CONFIG_LOG_LEVEL=1`, which Uncork sets | stderr |
| The archive also carries DXMT and DXVK builds (`Libraries/DXMT`, `Libraries/DXVK`). They stay unused in the component directory: Uncork installs and pins its `dxmt` and `dxvk` components separately | Component directory listing |

What follows from having this one runtime:

- msync is on by default ([PERFORMANCE.md](PERFORMANCE.md#msync)), and `WINE_LARGE_ADDRESS_AWARE` from a profile takes effect.
- Every DXMT and DXVK launch uses `PrefixNative`: the backend's DLLs are copied into `system32` and `syswow64` and chosen per process with DLL overrides ([ARCHITECTURE.md](ARCHITECTURE.md#activation-strategies)). The Steam client is unaffected; it loads its DXVK from its own directory ([STEAM.md](STEAM.md#graphics-steam-runs-on-dxvk)).
- D3DMetal can be imported but not used: `PrefixNative` cannot activate it, so a D3DMetal launch is an error until a runtime with `renderer-dllpath` or `dllpath-prepend` is pinned. The phase-1 build adds `dllpath-prepend`.

Considered and not in the catalog:

| Runtime | Why not |
|---|---|
| Gcenx's Sikarugir engines ([Sikarugir-App/Engines](https://github.com/Sikarugir-App/Engines)), for example `sikarugir-11.0_1` | They honor `WINEDLLPATH_DXMT`, `_DXVK` and `_D3DMETAL` per process ([Sikarugir#283](https://github.com/Sikarugir-App/Sikarugir/issues/283)), which `RendererEnv` uses, but they need FreeType, GnuTLS and MoltenVK from the Sikarugir wrapper app, which Uncork does not supply. An earlier draft of the catalog recommended one; it was removed |
| Gcenx's [macOS_Wine_builds](https://github.com/Gcenx/macOS_Wine_builds) | Upstream WineHQ builds without msync and without the Metal glue DXMT and D3DMetal need; useful as an A/B baseline only |

DXMT 0.80 is the last MIT-licensed DXMT release; `main` moved to LGPL-2.1-or-later on 2026-04-25 ([LICENSE](https://github.com/3Shain/dxmt/blob/main/LICENSE)). Fixes that matter for Rise of Nations (a geometry-shader marshalling loop and zero-count draw handling, through commit [46911d7345](https://github.com/3Shain/dxmt/commit/46911d7345)) landed after 0.80, so phase 1 builds DXMT from `main`.

## Runtime features

A Wine component's feature tags say what it supports. The planner reads them; it never guesses from version numbers. Tags are copied from the catalog into `component.toml` at install time.

| Tag | Meaning | Used for |
|---|---|---|
| `wow64` | Runs 32-bit Windows programs (new-style WoW64: `--enable-archs=i386,x86_64`) | `doctor` warns without it; a plan for a 32-bit game on a runtime without it carries a warning |
| `msync` | Honors `WINEMSYNC=1` (Mach-semaphore synchronization) | `WINEMSYNC=1` is set only with this tag |
| `dxmt` | The Mac driver exports the Metal view API DXMT looks up (`macdrv_functions`; [winemetal_unix.c](https://github.com/3Shain/dxmt/blob/main/src/winemetal/unix/winemetal_unix.c)) | DXMT counts as available only with this tag, and, when its DLLs are copied into the prefix (`PrefixNative`), only with the runtime's own `lib/wine/x86_64-unix/winemetal.so` (`graphics::runtime_blocker`) |
| `d3dmetal` | Carries the CrossOver-derived glue D3DMetal needs (`__wine_unix_call` export, `CX_APPLEGPTK_LIBD3DSHARED_PATH`) | D3DMetal counts as available only with this tag and with `renderer-dllpath` or `dllpath-prepend`, which activating it needs, so on the catalog's runtime it is never chosen automatically (`graphics::runtime_blocker`) |
| `renderer-dllpath` | Honors `WINEDLLPATH_DXMT`, `WINEDLLPATH_DXVK`, `WINEDLLPATH_D3DMETAL` per process | `RendererEnv` activation. No runtime in the catalog has it |
| `dllpath-prepend` | Honors `WINEDLLPATH_PREPEND` per process | `DllPathPrepend` activation. No runtime in the catalog has it; Uncork's own runtime is to get it from [patch 1](#patch-queue) |
| `large-address-aware` | Honors `WINE_LARGE_ADDRESS_AWARE=1` | Profiles that set it take effect; a plan for a 32-bit program with a graphics API that lacks the PE flag suggests it |

A tag is a claim. Add one only after checking the binary, for example `strings lib/wine/x86_64-unix/ntdll.so | grep -c WINEMSYNC` for `msync`, `WINE_LARGE_ADDRESS_AWARE` for `large-address-aware`, `WINEDLLPATH_DXMT` for `renderer-dllpath`, and a 32-bit test program for `wow64`.

## Required component layouts

`component::validate_layout` refuses an install whose files are not where Uncork expects them; `component::required_paths` is the authoritative list and this table must match it. Paths are relative to the component directory, after `strip_prefix`.

| Kind | Required | Notes |
|---|---|---|
| `wine` | `bin/wine` or `bin/wine64`; `lib/wine/x86_64-windows/`; `lib/wine/x86_64-unix/` | A Wine tree nested up to three levels down (winecx-gptk's `Libraries/Wine`) is moved to the top before the check. `bin/wineserver` is used but not checked. `lib/wine/i386-windows/` is what `wow64` needs; the tag declares it and install does not check it. Wine 11 removed the `wine64` loader ([ANNOUNCE](https://github.com/wine-mirror/wine/blob/wine-11.0/ANNOUNCE.md)) |
| `dxmt` | `x86_64-windows/d3d11.dll`, `x86_64-windows/dxgi.dll`, `x86_64-windows/winemetal.dll`, `x86_64-unix/winemetal.so` | `i386-windows/` with the same DLLs for 32-bit games. `d3d10core.dll` is not checked at install, but a `PrefixNative` launch fails without it. The builtin variant, as in the release tarball ([guide](https://github.com/3Shain/dxmt/wiki/DXMT-Installation-Guide-for-Geeks)) |
| `dxvk` | `x86_64-windows/d3d11.dll` | The release's `x64/` and `x32/` are renamed to `x86_64-windows/` and `i386-windows/` before the check. `d3d10core.dll` is needed at launch and, with `d3d11.dll`, by the Steam client. No `dxgi.dll` and no `d3d9.dll` in the macOS fork |
| `d3dmetal` | `external/D3DMetal.framework/`, `wine/x86_64-windows/` | GPTK's `redist/lib` layout. `external/libd3dshared.dylib` is needed at launch. The `wine/x86_64-unix/*.so` files are symlinks to `../../external/libd3dshared.dylib` and must stay symlinks |

Graphics components use Wine's own architecture directory names (`x86_64-windows`, `i386-windows`, `x86_64-unix`), so the component directory itself is what `WINEDLLPATH_DXMT` or `WINEDLLPATH_PREPEND` points to. For D3DMetal it is the `wine/` subdirectory.

## Updating a pin

A pin is a promise that Uncork installs exactly one file. Changing one is a reviewed pull request, and the author does the verification themselves ([AI_CONTRIBUTIONS.md](../AI_CONTRIBUTIONS.md)).

1. Find the release on the publisher's own page and copy the exact HTTPS asset URL. Never use a mirror.
2. Download it into a new, empty directory:

   ```bash
   mkdir -p ~/pin-check && cd ~/pin-check
   curl -fL -o archive.tar.xz '<url>'
   ```

3. Compute the hash and the size:

   ```bash
   shasum -a 256 archive.tar.xz
   stat -f %z archive.tar.xz
   ```

4. List the archive and find its top-level directory, which becomes `strip_prefix`. Check that the layout matches [Required component layouts](#required-component-layouts):

   ```bash
   tar -tf archive.tar.xz | head -n 40
   ```

5. Record the license as an SPDX expression from the project's license file at that tag, and `source_code` as a URL to the exact tag or commit the binary was built from. For LGPL components that URL is the corresponding source.
6. For Wine, set only feature tags you verified ([Runtime features](#runtime-features)).
7. Edit `runtime/catalog.toml`. Keep at most one `recommended = true` per kind. Do not edit an existing entry's hash in place; a new upstream file is a new version.
8. Run `cargo test -p uncork-core`, which parses and validates the built-in catalog.
9. The catalog is compiled into the binary, so install the new pin with a build from your branch, in a sandbox, and run a game:

   ```bash
   export UNCORK_HOME="$HOME/UncorkDev/pin-check"
   cargo run -p uncork -- runtime install wine --version <version> --yes
   cargo run -p uncork -- doctor
   ```

10. Open the PR with the URL, hash, size, license, source link, how each feature tag was verified, and the game you tested.

If a publisher replaces a file in place (some publish every build under one rolling release tag), installs fail with a checksum mismatch until the pin is updated. That is intended: Uncork fails closed.

## Phase 1: Uncork's own CI build

Planned for M2. Uncork builds an x86-64 Wine from CrossOver's published LGPL sources plus its own patch queue, on GitHub's hosted arm64 macOS runners with Rosetta.

### Why the CrossOver tree

The base is `crossover-sources-26.3.0.tar.gz` (149,054,023 B, published 2026-07-21; [CodeWeavers source](https://www.codeweavers.com/crossover/source)). The directory listing returns 403, so the build fetches the exact file name and pins its SHA-256. Its Wine tree is Wine 11.0 with about 221 files changed ([dappermint/winecx@crossover-26.3.0](https://github.com/dappermint/winecx/tree/crossover-26.3.0)), and it carries pieces upstream Wine 11.19 does not:

| Piece | Location | Needed by |
|---|---|---|
| `macdrv_functions` export | `dlls/winemac.drv/d3dmetal.c` | DXMT and D3DMetal find the Mac driver's Metal views through it |
| `__wine_unix_call` export (CW HACK 22435) | `dlls/ntdll/loader.c` | D3DMetal's PE modules |
| `CX_APPLEGPTK_LIBD3DSHARED_PATH` (CW HACK 22434) | `dlls/ntdll/unix/loader.c` | Loading `libd3dshared.dylib` |
| msync | `server/msync.c`, `dlls/ntdll/unix/msync.c` | Fast synchronization; esync is gone |
| `WINE_LARGE_ADDRESS_AWARE` (CROSSOVER HACK 17634, off unless set) | `dlls/ntdll/unix/virtual.c` | 32-bit games such as Rise of Nations |
| `prepend_dll_path()` (CW Hack 24067), exported with no caller in the LGPL tree | `dlls/ntdll/unix/loader.c` | The `WINEDLLPATH_PREPEND` patch below |

The D3DMetal glue compiles only for x86-64. The rebase onto current WineHQ follows the approach of [dappermint/winecx](https://github.com/dappermint/winecx).

### Patch queue

All patches are LGPL-2.1-or-later and are sent upstream where upstream wants them.

| # | Patch | Why | Source |
|---|---|---|---|
| 1 | `WINEDLLPATH_PREPEND`: call `prepend_dll_path()` from an environment variable | Wine searches its own DLL directory before `WINEDLLPATH`, so plain `WINEDLLPATH` cannot override a builtin. This enables per-process `DllPathPrepend` activation | [loader.c, wine-11.19](https://github.com/wine-mirror/wine/blob/wine-11.19/dlls/ntdll/unix/loader.c), [cx loader.c](https://github.com/dappermint/winecx/blob/crossover-26.3.0/dlls/ntdll/unix/loader.c) |
| 2 | msync fixes: stale registrations and multi-object wait performance | Stock msync was slower than none on multi-object waits | [frankea/Whisky v4.6.4-beta.1](https://github.com/frankea/Whisky/releases/tag/v4.6.4-beta.1) |
| 3 | `steamwebhelper.exe` argument injection: append `--no-sandbox --in-process-gpu --disable-gpu`, skip `--type=crashpad-handler` | Would render Steam's web UI in software from the Wine side. Present in CrossOver 24.0.4's public source, absent from 25.1.0's. Not needed so far: on `winecx-gptk-4.7.3` the UI renders with app-local DXVK, and software CEF output stayed black (2026-10-07, [STEAM.md](STEAM.md#graphics-steam-runs-on-dxvk)). Ported only if Uncork's runtime needs it | [CX 24.0.4 process.c](https://github.com/PhoenicisOrg/winecx/blob/winecx-24.0.4/dlls/kernelbase/process.c) |
| 4 | No-exec decided by the main executable, as on Windows | One non-`NX_COMPAT` DLL otherwise turns DEP off for the whole process; under Rosetta that made first-touch page faults about 200 times costlier | [athei/wine 539aa62220](https://github.com/athei/wine/commit/539aa62220), [athei/wine-build#3](https://github.com/athei/wine-build/issues/3) |
| 5 | [MR 12257](https://gitlab.winehq.org/wine/wine/-/merge_requests/12257): top-down reservation at `0x7ff000000000` | Needed under Rosetta when the base is upstream Wine rather than the CrossOver tree | |
| 6 | [MR 11538](https://gitlab.winehq.org/wine/wine/-/merge_requests/11538): Vulkan portability enumeration | Only if MoltenVK is loaded through the Vulkan loader | |

### `runtime/build-wine.sh`

```text
runtime/build-wine.sh [--crossover-version 26.3.0] [--out DIR]
```

| Option | Meaning |
|---|---|
| `--crossover-version` | Which `crossover-sources-<version>.tar.gz` to build; default `26.3.0`. The tarball's SHA-256 is pinned in the script |
| `--out DIR` | Where the results are written |

Outputs:

- `uncork-wine-<version>-x86_64.tar.xz`: the runtime, laid out as in [Required component layouts](#required-component-layouts), ready to be pinned in the catalog. Versions should follow a `<base>-uncork<N>` pattern, which `compare_versions` orders correctly.
- `SOURCES.json`: the inputs the build used, each with its URL and SHA-256, at least the CrossOver source tarball and every patch applied. It is the index of the LGPL source offer published next to the binary.

`runtime/work/` and `runtime/out/` are git-ignored for local builds.

### Build recipe

Adapted from, and verified against, [winecx-gptk's build.yml](https://github.com/dappermint/winecx-gptk/blob/main/.github/workflows/build.yml).

| Step | Detail |
|---|---|
| Runner | arm64 `macos-26`, after `softwareupdate --install-rosetta --agree-to-license` |
| Wine tools | Built natively first (`configure --enable-archs=i386,x86_64 && make __tooldeps__`); cross-compiling without `--with-wine-tools` is a configure error ([configure.ac](https://github.com/wine-mirror/wine/blob/wine-11.19/configure.ac)) |
| Cross configure | `CC='clang -arch x86_64' --host=x86_64-apple-darwin24 --with-wine-tools=<tools> --enable-archs=i386,x86_64 --disable-tests --without-x --without-wayland --with-vulkan --with-coreaudio --with-sdl --with-gnutls --with-freetype --with-gstreamer --with-ffmpeg` |
| PE toolchain | mingw-w64 GCC, not llvm-mingw: CrossOver's PE DLLs are GCC 13.2.0, and a `kernelbase.dll` built with llvm-mingw broke Steam's login in dappermint's bisection |
| Deployment floor | `MACOSX_DEPLOYMENT_TARGET` pinned (15.0 recommended), `-Werror=unguarded-availability-new`, and `ac_cv_func_pipe2=no` because the macOS 27 SDK marks `pipe2` and `dup3` as 27.0-only |
| Relocation | `ac_cv_lib_soname_{freetype,gnutls,MoltenVK}` set to leaf names, install names rewritten to `@loader_path`, `-add_rpath @loader_path/../../`; never `DYLD_*` |
| Never | Building under `arch -x86_64` on macOS 27: thousands of short-lived translated tool processes were reported to overload the system's analytics daemon (a single report, in [build.yml](https://github.com/dappermint/winecx-gptk/blob/main/.github/workflows/build.yml)) |

CI gates for every runtime release:

- No absolute, non-system load commands in any Mach-O.
- Every library loads with `dlopen` with the build paths masked.
- A window opens: if the Mac driver fails, Wine silently falls back to no display driver.
- The i386 half is non-empty; otherwise every 32-bit program fails with `c0000135`.
- FFmpeg and GStreamer support were detected by configure.
- PE debug information is stripped.
- Every PE DLL shipped has `NX_COMPAT`.

### x86-64 dependencies

The x86-64 libraries Wine links against (FreeType, GnuTLS with Nettle, GMP and libtasn1, SDL2, libpng, an LGPL-only FFmpeg) are getting harder to obtain prebuilt: Homebrew moved Intel to Tier 3 in September 2026 ([Homebrew 5.0.0](https://brew.sh/2025/11/12/homebrew-5.0.0)), and Nixpkgs 26.05 is the last release with x86_64-darwin ([NixOS 26.05](https://nixos.org/blog/announcements/2026/nixos-2605/)). The plan is a small, pinned from-source x86-64 build of those libraries, plus the official universal GStreamer.framework and Khronos MoltenVK ([v1.4.2](https://github.com/KhronosGroup/MoltenVK/releases/tag/v1.4.2), the first release that knows the M5 GPU family).

### Release contents

Each phase-1 runtime release carries the binary archive, `SOURCES.json`, the exact source tarballs it lists, the patches and the build scripts, so the LGPL-2.1 section 6 obligation is met by the release itself ([LEGAL.md](LEGAL.md#lgpl-obligations-for-wine-builds)).
