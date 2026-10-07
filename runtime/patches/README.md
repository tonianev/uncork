# Wine patch queue

`runtime/build-wine.sh` applies every `*.patch` file in this directory to the `sources/wine` tree of CodeWeavers' CrossOver source tarball, in file-name order, before it configures Wine. The queue is empty today. This file lists the patches we plan to carry and why.

## Rules

- **Format.** One change per file, named `NNNN-short-description.patch` (`0001-ntdll-honor-WINEDLLPATH_PREPEND.patch`), in `git format-patch` form with paths relative to the Wine tree, so `patch -p1` applies it. The script applies with no fuzz: a patch that does not apply exactly stops the build.
- **Header.** The `From:` line names the original author. The message says what the patch changes, why Uncork needs it, where it came from (`Origin:` with a link) and its upstream status (`Upstream:` with a merge request link, or `not submitted` and why).
- **License.** Patches modify Wine, so they are LGPL-2.1-or-later like Wine. Port a change from another Wine fork only when that fork's Wine changes are LGPL-2.1-or-later. Never copy code from GPL-only or AGPL projects.
- **Provenance.** Every build records the SHA-256 of each patch in `SOURCES.json` and ships the patch files in `share/doc/uncork-wine/patches/`, so a runtime can always be rebuilt from its exact sources (LGPL-2.1 section 6).
- **Upstream first.** Each patch is submitted to WineHQ or to the project it came from. A patch leaves this directory as soon as the base tree carries the change.

## Making a patch

```sh
# Fetch, verify and extract the source, apply the current queue, then stop.
# The source pin rules are the same as for a full build.
runtime/build-wine.sh --prepare-only
cd runtime/work/src-26.3.0/sources/wine
git init -q && git add -A && git commit -qm base
# Edit, then (NEXT is the next free number in runtime/patches):
git commit -qam "ntdll: Honor WINEDLLPATH_PREPEND."
git format-patch -1 --start-number NEXT -o ../../../../patches
```

Then run the full build, which starts again from a fresh tree, and check that the change does what it says on a real game.

## Planned queue

| Order | Change | Why Uncork needs it | Source and upstream |
|---|---|---|---|
| 1 | `WINEDLLPATH_PREPEND` | In both upstream Wine and the CrossOver tree, `set_dll_path()` puts Wine's own DLL directory before `WINEDLLPATH`, so `WINEDLLPATH` cannot override a builtin DLL per process. CrossOver exports `prepend_dll_path()` (CW Hack 24067) but nothing in the LGPL tree calls it. The patch calls it from an environment variable, which gives the runtime the `dllpath-prepend` feature in `runtime/catalog.toml`. | Upstream [`dlls/ntdll/unix/loader.c` at wine-11.19](https://github.com/wine-mirror/wine/blob/wine-11.19/dlls/ntdll/unix/loader.c) (`set_dll_path`); CrossOver's [`prepend_dll_path`](https://github.com/dappermint/winecx/blob/crossover-26.3.0/dlls/ntdll/unix/loader.c). Not upstream. |
| 2 | msync fixes | A stale-registration fix, and a multi-object wait fix: stock msync's multi-object wait is slower than no msync at all (366 ms, 84 ms after the fix). Every process runs with `WINEMSYNC=1`, so this is on every game's hot path. | [frankea/Whisky v4.6.4-beta.1](https://github.com/frankea/Whisky/releases/tag/v4.6.4-beta.1); [highball-engine](https://github.com/gauthierpiarrette/highball-engine) patch 0014. msync itself is [marzent/wine-msync](https://github.com/marzent/wine-msync), carried in the CrossOver tree. |
| 3 | `steamwebhelper` arguments | CrossOver 24.0.4 appended `--no-sandbox --in-process-gpu --disable-gpu` to `steamwebhelper.exe` (and skipped `--type=crashpad-handler`) in `hack_append_command_line`. The entry is absent from the CrossOver 25.1.0 public source, and how CrossOver 26 keeps Steam's web UI working is unknown. Port it only if Steam's UI fails without it on our runtime. | [CrossOver 24.0.4 `dlls/kernelbase/process.c`](https://github.com/PhoenicisOrg/winecx/blob/winecx-24.0.4/dlls/kernelbase/process.c). Not upstream; game-specific hacks rarely are. |
| 4 | No-exec decided by the main EXE | Windows decides DEP for a process from its main executable. Wine turns DEP off for the whole process when any loaded module lacks `NX_COMPAT`; under Rosetta that makes first-touch page faults about 200 times more expensive. | [athei/wine 539aa62220](https://github.com/athei/wine/commit/539aa62220); measurements in [athei/wine-build#3](https://github.com/athei/wine-build/issues/3). Not upstream. |
| 5 | [MR 12257](https://gitlab.winehq.org/wine/wine/-/merge_requests/12257) | Top-down address space reservation at `0x7ff000000000`, which Rosetta needs. Only on an upstream WineHQ base; check whether the CrossOver tree already has an equivalent before porting. | WineHQ merge request. |
| 6 | [MR 11538](https://gitlab.winehq.org/wine/wine/-/merge_requests/11538) | Vulkan portability enumeration. Only needed if MoltenVK is loaded through vulkan-loader; `build-wine.sh` points Wine at MoltenVK directly, so it is not needed today. | WineHQ merge request. |

D3DMetal, DXMT and DXVK need no Wine patch here: the CrossOver tree already exports what they need (`macdrv_functions` in `winemac.drv/d3dmetal.c`, `__wine_unix_call`, `CX_APPLEGPTK_LIBD3DSHARED_PATH`). The `WINE_LARGE_ADDRESS_AWARE` override is also in the tree and is off by default; Uncork turns it on per game.
