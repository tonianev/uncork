# Game profiles

A game profile is one TOML file that tells Uncork how to run one game well: which executable renders it, which graphics backend to prefer, which Wine, performance, environment and DLL settings it needs, how to start it, which INI keys to enforce, and how well it runs. This document is the file format, field by field, with the validation rules and the semantics of INI edits and launch modes. How to test a game and add a profile is in [docs/COMPATIBILITY.md](../docs/COMPATIBILITY.md). The Rust definition is `uncork_core::profile`.

## Files and precedence

| Location | Role |
|---|---|
| `profiles/<id>.toml` in this repository | Built-in profiles, compiled into the `uncork` binary by `crates/uncork-core/build.rs` |
| `$UNCORK_HOME/profiles/<id>.toml` | Your own profiles. A user profile with the same `id` replaces the built-in one entirely; fields are not merged |

The file name is the profile id plus `.toml`. A user file that fails to parse is reported, not fatal; the other profiles still load.

`uncork play <query>` and `uncork profile show <query>` find a profile by, in order: exact id; Steam app id (when the query is all digits); exact name, ignoring case; a substring of an id or name, ignoring case. The first of these steps that matches anything decides. One match is used; several are an error that lists the candidate ids, so pick one by its id. `uncork play rise-of-nations` works because only one profile id contains `rise-of-nations`. Two profiles with the same `[steam] appid`, such as a user profile `rise-of-nations-wined3d` next to the built-in one, make `uncork play 287450` and `uncork steam launch 287450` ambiguous; `uncork play rise-of-nations-wined3d` picks one. Only when no profile matches does `uncork play <Steam app id>` start an installed game without a profile.

Settings layer from lowest to highest precedence: the bottle's `uncork.toml`, the profile, then command-line flags (`--backend`, `--hud`, `--metalfx`, `--retina`, `-e KEY=VALUE`). For environment variables and DLL overrides, later layers win per key.

## Example

```toml
schema = 1
id = "rise-of-nations-extended-edition"
name = "Rise of Nations: Extended Edition"

[steam]
appid = 287450

[exe]
path = "riseofnations.exe"        # relative to the install directory, '/' separators
bitness = "x86"
api = "d3d11"
geometry_shaders = true           # rules out DXVK on MoltenVK

[graphics]
backend = "dxmt"
fallbacks = ["wined3d"]

[wine]
windows_version = "win10"

[env]
WINE_LARGE_ADDRESS_AWARE = "1"    # 4 GiB address space on runtimes that honor it

[dll_overrides]
d3dcompiler_47 = "n,b"            # the game's Microsoft HLSL compiler, not Wine's

[[ini]]
file = '%APPDATA%\Microsoft Games\Rise of Nations\rise2.ini'
section = "RISE OF NATIONS"
key = "SkipIntroMovies"
value = "1"
reason = "The intro videos are WMV through DirectShow and stall without GStreamer WMV decoders."

[compat]
status = "untested"
notes = "Needs the Steam client running in the same bottle; uncork play starts it."
```

Every table rejects unknown keys, so a misspelt key is an error, not a silently ignored setting. Only `schema`, `id`, `name` and `[exe] path` are required.

## Fields

### Top level

| Key | Type | Required | Meaning |
|---|---|---|---|
| `schema` | integer | yes | Format version; must be `1` |
| `id` | string | yes | Kebab-case id, equal to the file name without `.toml` |
| `name` | string | yes | Display name |

### `[steam]`

| Key | Type | Required | Meaning |
|---|---|---|---|
| `appid` | integer | yes, if the table is present | Steam app id. Omit the whole table for non-Steam games |

### `[exe]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `path` | string | required | The executable that renders the game, relative to the install directory, with `/` separators. Often not Steam's launch target, which may be a launcher |
| `bitness` | `"x86"` or `"x64"` | detected | 32- or 64-bit. When omitted, Uncork reads it from the PE header |
| `api` | `"directdraw"`, `"opengl"`, `"d3d8"`, `"d3d9"`, `"d3d10"`, `"d3d11"`, `"d3d12"`, `"vulkan"` | detected | Primary graphics API. When omitted, Uncork scans the executable and the DLLs beside it |
| `geometry_shaders` | boolean | `false` | The game uses geometry shaders. Rules out DXVK, because MoltenVK has none |

`uncork inspect <exe>` prints the detected bitness and API with the evidence for them. Set `bitness` and `api` in the profile when detection is wrong or slow, for example when the renderer is loaded from a DLL by name at run time.

### `[graphics]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `backend` | `"dxmt"`, `"d3dmetal"`, `"dxvk"`, `"wined3d"` | automatic | Preferred backend |
| `fallbacks` | list of backends | `[]` | Tried in order when the preferred backend is not installed. After them, Uncork's own decision table applies ([docs/ARCHITECTURE.md](../docs/ARCHITECTURE.md#backend-decision-matrix)) |

### `[wine]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `windows_version` | `"win7"`, `"win81"`, `"win10"`, `"win11"` | the bottle's | Windows version the game needs |
| `msync` | boolean | the bottle's | Force msync on or off. msync is per wineserver, so a value different from the running bottle's needs a wineserver restart ([docs/PERFORMANCE.md](../docs/PERFORMANCE.md#msync)) |

### `[performance]`

Each key overrides the bottle's `[performance]` value for this game; omitted keys keep the bottle's.

| Key | Type | Meaning |
|---|---|---|
| `retina` | boolean | Ask for native Retina resolution. `RetinaMode` is a prefix-wide registry setting that a launch does not change, so a value that differs from the bottle's only produces a warning; `uncork bottle set <bottle> performance.retina=true` changes it |
| `metalfx` | boolean | Upscale with MetalFX where the backend supports it |
| `avx` | boolean | Let Rosetta advertise AVX/AVX2 (`ROSETTA_ADVERTISE_AVX`) |

### `[env]`

Extra environment variables for the game, as `NAME = "value"` strings. They apply after the bottle's and before `-e` on the command line. Names must match `^[A-Za-z_][A-Za-z0-9_]*$`. Use it for backend options such as `DXMT_CONFIG` or `D3DM_MAX_FPS` ([docs/PERFORMANCE.md](../docs/PERFORMANCE.md)).

### `[dll_overrides]`

Extra Wine DLL overrides for the game, as `name = "value"`, where `name` is the DLL without `.dll` (for example `d3dcompiler_47`, or `winemenubuilder.exe` for an executable). They apply after the backend's and the bottle's overrides.

| Value | Meaning |
|---|---|
| `"n"` | Native (the game's or Windows' DLL) only |
| `"b"` | Wine's builtin only |
| `"n,b"` | Native first, then builtin |
| `"b,n"` | Builtin first, then native |
| `"d"` or `""` | Disabled: the DLL is not loaded |

### `[launch]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `args` | list of strings | `[]` | Arguments always passed to the game, before any given after `--` on the command line |
| `mode` | `"direct"`, `"applaunch"`, `"standalone"` | `"direct"` with `[steam]`, else `"standalone"` | How to start the game; see [Launch modes](#launch-modes) |

### `[[ini]]`

Zero or more INI keys enforced before every launch; see [INI edits](#ini-edits).

| Key | Type | Required | Meaning |
|---|---|---|---|
| `file` | string | yes | The INI file, as a Windows-style path starting with a base: `%APPDATA%\`, `%LOCALAPPDATA%\`, `%USERPROFILE%\` or `%INSTALLDIR%\`. Both `\` and `/` are accepted |
| `section` | string | yes | Section name without brackets, matched ignoring case |
| `key` | string | yes | Key, matched ignoring case and surrounding whitespace |
| `value` | string | yes | The value to write |
| `reason` | string | no | Why; shown by `uncork profile show` |

### `[compat]`

| Key | Type | Default | Meaning |
|---|---|---|---|
| `status` | `"untested"`, `"broken"`, `"runs"`, `"playable"`, `"perfect"` | `"untested"` | Overall status: the best recent first-hand report ([docs/COMPATIBILITY.md](../docs/COMPATIBILITY.md#status-levels)) |
| `notes` | string | `""` | What players should know |

### `[[compat.reports]]`

Test results, newest first. Every key except `notes` is required.

| Key | Type | Meaning |
|---|---|---|
| `date` | string | `YYYY-MM-DD` |
| `uncork` | string | Uncork version or commit |
| `macos` | string | macOS version, for example `27.0.1` |
| `chip` | string | For example `M5 Max` |
| `backend` | backend name | The backend used |
| `status` | status name | The result |
| `notes` | string | Resolution, settings, FPS, what did not work |

## Validation

A profile with any of these problems is rejected (`cargo test -p uncork-core` checks every built-in profile):

- `schema` is not `1`.
- `id` is not kebab-case (`^[a-z0-9]+(-[a-z0-9]+)*$`) or is longer than 64 characters.
- `name` is empty.
- `[exe] path` is empty, absolute, contains `\` or contains `..`.
- An `[env]` name does not match `^[A-Za-z_][A-Za-z0-9_]*$`.
- A `[dll_overrides]` value is not one of `n`, `b`, `n,b`, `b,n`, `d` or empty.
- The preferred backend is repeated in `fallbacks`.
- A backend in `backend` or `fallbacks` cannot run the declared `bitness` and `api` (for example D3DMetal for a 32-bit game, or DXMT for Direct3D 9).
- DXVK appears in `backend` or `fallbacks` while `geometry_shaders = true`.
- `mode` is `direct` or `applaunch` without a `[steam]` table.
- An `[[ini]] file` does not start with a known base, or contains `..`.

## INI edits

Many games keep settings in an INI file that profiles need to control, such as Rise of Nations' `SkipIntroMovies`. Before each launch Uncork applies every `[[ini]]` entry:

- The base is resolved inside the bottle. `%APPDATA%`, `%LOCALAPPDATA%` and `%USERPROFILE%` are the prefix user's folders under `drive_c/users/<user>/`, where `<user>` is the bottle's `env.USER` if set (a bottle imported from CrossOver has `crossover`), else the one directory under `drive_c/users/` other than `Public`, else your macOS user name; `%INSTALLDIR%` is the game's install directory. Path components that exist on disk with different letter case are matched ignoring case.
- If the file does not exist yet, nothing happens: many games create their INI on first run, so the edit takes effect from the second launch.
- An existing key is rewritten in place, keeping its original spelling. A missing key is appended at the end of its section; a missing section is appended at the end of the file.
- Everything else is preserved byte for byte: line order, comments, unknown keys, CRLF or LF line endings, and the encoding (UTF-8 or ASCII, or UTF-16LE with a byte-order mark, written back the same way).
- The file is written atomically, and only when something changed.

An INI edit is enforced on every launch, so a value changed in the game's own options menu is reset next time. Use `[[ini]]` only for settings that must hold, and say why in `reason`.

## Launch modes

| Mode | Behavior | Use for |
|---|---|---|
| `direct` | Uncork makes sure the bottle's Steam client is running, then starts `[exe] path` directly in the same prefix with the game's own backend environment, plus `SteamAppId` and `SteamGameId` set to `[steam] appid` as Steam sets them for the games it starts (an `[env]` value for either wins) | Steam games that run when started directly. The default with `[steam]` |
| `applaunch` | Uncork restarts Steam with the game's environment and runs `steam.exe -applaunch <appid> <args>`; the game inherits Steam's environment | Steam games whose DRM must be started by Steam |
| `standalone` | Uncork starts the executable without Steam | DRM-free and non-Steam games. The default without `[steam]` |

Prefer `direct`: it is the only mode in which the game gets its own backend environment while the Steam client keeps its own (DXVK, loaded from its web helper's directory). Use `applaunch` when a direct start fails because the game insists on being launched by Steam. Details and trade-offs are in [docs/STEAM.md](../docs/STEAM.md#launch-modes).
