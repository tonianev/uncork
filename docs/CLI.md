# Command-line reference

This document is the reference for every `uncork` command and subcommand. Each section is the command's own `--help` output, unedited, so it matches the binary exactly; a short note under a heading adds what the help text cannot say. Concepts are explained elsewhere: launches and backends in [ARCHITECTURE.md](ARCHITECTURE.md), Steam in [STEAM.md](STEAM.md), performance flags in [PERFORMANCE.md](PERFORMANCE.md), the profile format in [profiles/README.md](../profiles/README.md).

Generated from `uncork 0.1.0` with `COLUMNS=100`. After changing `crates/uncork/src/cli.rs`, regenerate it by running each command below with `--help` (the commands that launch something share one set of options, `LaunchFlags`).

Conventions that apply to every command:

- `--json` and `-v`/`--verbose` are global and may follow any subcommand.
- `-b`/`--bottle` defaults to `default_bottle` in `$UNCORK_HOME/config.toml`, which `uncork setup` sets (normally `steam`).
- Arguments after `--` go to the Windows program unchanged (`uncork play rise-of-nations -- -windowed`).
- Everything Uncork writes lives under `$UNCORK_HOME`, by default `~/Library/Application Support/Uncork`.
- Exit status: 0 on success; 1 when the command fails, or with `--wait` when the program exits unsuccessfully; 2 for a usage error. Errors print `error:` and one `caused by:` line per further cause.

## Commands

| Command | Does |
|---|---|
| [`uncork doctor`](#uncork-doctor) | Check this Mac and the installation |
| [`uncork setup`](#uncork-setup) | Install the recommended components, create the `steam` bottle, install Steam |
| [`uncork play`](#uncork-play) | Play a game by profile id, Steam app id or name |
| [`uncork run`](#uncork-run) | Run any Windows program in a bottle |
| [`uncork inspect`](#uncork-inspect) | Bitness, graphics API and backend choice for an `.exe` |
| [`uncork runtime`](#uncork-runtime) | Install, list and remove Wine, DXMT, DXVK; import D3DMetal |
| [`uncork bottle`](#uncork-bottle) | Create, import, configure, stop and delete bottles |
| [`uncork steam`](#uncork-steam) | Install and start the Windows Steam client; list and launch its games |
| [`uncork profile`](#uncork-profile) | List and show game profiles |
| [`uncork winetricks`](#uncork-winetricks) | Run winetricks verbs in a bottle |

## uncork

```text
Run Windows games on Apple Silicon Macs.

Uncork sets up Wine, a Metal translation layer (DXMT, DXVK or Apple's D3DMetal) and the Windows
Steam client in a "bottle", then launches games with per-game settings tuned for speed.

First time? Run `uncork setup`, then `uncork play rise-of-nations`.

Usage: uncork [OPTIONS] <COMMAND>

Commands:
  doctor      Check this Mac and the Uncork installation, and say how to fix problems
  setup       One-shot setup: install the recommended components, create the `steam` bottle and
              install Steam into it
  play        Find a game profile and play it (through Steam when the game is on Steam)
  run         Run a Windows program in a bottle
  inspect     Show what a Windows executable is: 32/64-bit, graphics API, recommended backend
  runtime     Manage Wine and graphics components
  bottle      Manage bottles (Wine prefixes)
  steam       Install and use the Windows Steam client in a bottle
  profile     List and show game profiles
  winetricks  Run winetricks verbs in a bottle (needs `brew install winetricks`)
  help        Print this message or the help of the given subcommand(s)

Options:
      --json
          Print machine-readable JSON instead of text (doctor, list, info, show, inspect, games and
          --dry-run output)

  -v, --verbose...
          More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## uncork doctor

```text
Check this Mac and the Uncork installation, and say how to fix problems

Usage: uncork doctor [OPTIONS]

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork setup

```text
One-shot setup: install the recommended components, create the `steam` bottle and install Steam into
it

Usage: uncork setup [OPTIONS]

Options:
      --bottle <BOTTLE>  Name of the bottle to create for Steam [default: steam]
      --json             Print machine-readable JSON instead of text (doctor, list, info, show,
                         inspect, games and --dry-run output)
      --no-steam         Do not install Steam (components and bottle only)
  -v, --verbose...       More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -y, --yes              Answer yes to downloads
  -h, --help             Print help
  -V, --version          Print version
```

## uncork play

Resolves the game as a profile id, a Steam app id, an exact name or a unique part of a name. A Steam game starts in the launch mode its profile names ([STEAM.md](STEAM.md#launch-modes)); without a profile, an installed Steam app id is started directly. `--dry-run` prints the backend, the reason, warnings, DLL copies, INI edits, the log file and the exact command; with `--json` it prints the plan as JSON.

```text
Find a game profile and play it (through Steam when the game is on Steam)

Usage: uncork play [OPTIONS] <GAME> [-- <ARGS>...]

Arguments:
  <GAME>
          Profile id, Steam app id, or part of a game's name

  [ARGS]...
          Arguments passed to the game after the profile's own

Options:
  -b, --bottle <BOTTLE>
          Bottle to use (default: `default_bottle` from config, normally `steam`)

      --json
          Print machine-readable JSON instead of text (doctor, list, info, show, inspect, games and
          --dry-run output)

      --backend <BACKEND>
          Graphics backend for this launch

          Possible values:
          - auto:     Pick per game from its API and bitness
          - d3dmetal: Apple D3DMetal (64-bit D3D11/D3D12; import with `uncork runtime import-gptk`)
          - dxmt:     DXMT (D3D10/11 to Metal, 32- and 64-bit)
          - dxvk:     DXVK on MoltenVK (D3D9/10/11)
          - wined3d:  Wine's built-in WineD3D (OpenGL), the compatibility fallback

  -v, --verbose...
          More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides

      --hud
          Show Apple's Metal performance HUD (FPS, frame time, GPU)

      --metalfx
          Upscale with MetalFX where the backend supports it

      --retina
          Render at native Retina resolution

      --wine-debug <CHANNELS>
          Enable Wine debug channels (e.g. `+loaddll,+d3d`); output goes to the launch log

  -e, --env <KEY=VALUE>
          Extra environment variable for this launch (repeatable)

      --game-mode
          Start through a macOS Game Mode app bundle (experimental; see docs/PERFORMANCE.md)

      --dry-run
          Print the launch plan (command, environment, DLLs to install) and exit

      --wait
          Wait for the program and its wineserver to exit before returning

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## uncork run

A Windows path (`C:\...`) is mapped into the bottle's `drive_c`; either way the file must exist. No profile applies: the backend comes from `--backend`, else the bottle's `graphics.backend`, else a scan of the executable.

```text
Run a Windows program in a bottle

Usage: uncork run [OPTIONS] <EXE> [-- <ARGS>...]

Arguments:
  <EXE>
          Windows executable: a macOS path, or a Windows path (`C:\...`) inside the bottle

  [ARGS]...
          Arguments passed to the program

Options:
  -b, --bottle <BOTTLE>
          Bottle to use (default: `default_bottle` from config, normally `steam`)

      --json
          Print machine-readable JSON instead of text (doctor, list, info, show, inspect, games and
          --dry-run output)

      --backend <BACKEND>
          Graphics backend for this launch

          Possible values:
          - auto:     Pick per game from its API and bitness
          - d3dmetal: Apple D3DMetal (64-bit D3D11/D3D12; import with `uncork runtime import-gptk`)
          - dxmt:     DXMT (D3D10/11 to Metal, 32- and 64-bit)
          - dxvk:     DXVK on MoltenVK (D3D9/10/11)
          - wined3d:  Wine's built-in WineD3D (OpenGL), the compatibility fallback

  -v, --verbose...
          More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides

      --hud
          Show Apple's Metal performance HUD (FPS, frame time, GPU)

      --metalfx
          Upscale with MetalFX where the backend supports it

      --retina
          Render at native Retina resolution

      --wine-debug <CHANNELS>
          Enable Wine debug channels (e.g. `+loaddll,+d3d`); output goes to the launch log

  -e, --env <KEY=VALUE>
          Extra environment variable for this launch (repeatable)

      --game-mode
          Start through a macOS Game Mode app bundle (experimental; see docs/PERFORMANCE.md)

      --dry-run
          Print the launch plan (command, environment, DLLs to install) and exit

      --wait
          Wait for the program and its wineserver to exit before returning

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## uncork inspect

```text
Show what a Windows executable is: 32/64-bit, graphics API, recommended backend

Usage: uncork inspect [OPTIONS] <EXE>

Arguments:
  <EXE>  Path to a Windows `.exe`

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork runtime

```text
Manage Wine and graphics components

Usage: uncork runtime [OPTIONS] <COMMAND>

Commands:
  list         List installed components
  available    List components available to install (the pinned catalog)
  install      Install components: `wine`, `dxmt`, `dxvk`, or `all` (default: all recommended)
  remove       Remove an installed component version
  import-gptk  Import D3DMetal from your copy of Apple's Game Porting Toolkit (mounted .dmg or its
               redist folder)
  help         Print this message or the help of the given subcommand(s)

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork runtime list

```text
List installed components

Usage: uncork runtime list [OPTIONS]

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork runtime available

```text
List components available to install (the pinned catalog)

Usage: uncork runtime available [OPTIONS]

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork runtime install

With no kinds, installs the recommended entry of every kind. `uncork runtime available` lists the catalog ([RUNTIME.md](RUNTIME.md#phase-0-pinned-upstream-builds)).

```text
Install components: `wine`, `dxmt`, `dxvk`, or `all` (default: all recommended)

Usage: uncork runtime install [OPTIONS] [KINDS]...

Arguments:
  [KINDS]...  Component kinds

Options:
      --json               Print machine-readable JSON instead of text (doctor, list, info, show,
                           inspect, games and --dry-run output)
      --version <VERSION>  Exact version (only with a single kind)
  -v, --verbose...         More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -y, --yes                Answer yes to downloads
  -h, --help               Print help
```

## uncork runtime remove

```text
Remove an installed component version

Usage: uncork runtime remove [OPTIONS] <KIND> <VERSION>

Arguments:
  <KIND>     Component kind
  <VERSION>  Version

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
```

## uncork runtime import-gptk

```text
Import D3DMetal from your copy of Apple's Game Porting Toolkit (mounted .dmg or its redist folder)

Usage: uncork runtime import-gptk [OPTIONS] <PATH>

Arguments:
  <PATH>  Path to the mounted GPTK volume, its `redist`, or `redist/lib`

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle

```text
Manage bottles (Wine prefixes)

Usage: uncork bottle [OPTIONS] <COMMAND>

Commands:
  list    List bottles
  create  Create a bottle
  info    Show a bottle's settings and state
  set     Change a bottle setting, e.g. `graphics.backend=dxmt`, `performance.retina=true`,
          `env.DXMT_LOG_LEVEL=info`
  delete  Delete a bottle and everything installed in it
  import  Import an existing Wine prefix (CrossOver or Whisky bottle, plain WINEPREFIX)
  kill    Stop every Windows process in a bottle
  env     Print `export` lines to use a bottle with Wine tools by hand
  tool    Open a Wine tool: winecfg, regedit, taskmgr, explorer, cmd, control
  help    Print this message or the help of the given subcommand(s)

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle list

```text
List bottles

Usage: uncork bottle list [OPTIONS]

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle create

```text
Create a bottle

Usage: uncork bottle create [OPTIONS] <NAME>

Arguments:
  <NAME>  Bottle name

Options:
      --json               Print machine-readable JSON instead of text (doctor, list, info, show,
                           inspect, games and --dry-run output)
      --wine <WINE>        Wine component version (default: newest installed)
  -v, --verbose...         More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
      --windows <WINDOWS>  Windows version: win7, win81, win10, win11 [default: win10]
  -h, --help               Print help
  -V, --version            Print version
```

## uncork bottle info

```text
Show a bottle's settings and state

Usage: uncork bottle info [OPTIONS] <NAME>

Arguments:
  <NAME>  Bottle name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle set

```text
Change a bottle setting, e.g. `graphics.backend=dxmt`, `performance.retina=true`,
`env.DXMT_LOG_LEVEL=info`

Usage: uncork bottle set [OPTIONS] <NAME> <SETTINGS>...

Arguments:
  <NAME>         Bottle name
  <SETTINGS>...  `key=value` pairs

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle delete

```text
Delete a bottle and everything installed in it

Usage: uncork bottle delete [OPTIONS] <NAME>

Arguments:
  <NAME>  Bottle name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -y, --yes         Do not ask for confirmation
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle import

Clones with APFS (`/bin/cp -c -R`) by default, falling back to a plain copy on volumes without clones. When the prefix has exactly one Windows user other than `Public` and it is not your macOS user name (CrossOver bottles use `crossover`), the bottle's `env` gets `USER` and `LOGNAME` set to it, so Wine keeps using that profile's AppData.

```text
Import an existing Wine prefix (CrossOver or Whisky bottle, plain WINEPREFIX)

Usage: uncork bottle import [OPTIONS] <PATH>

Arguments:
  <PATH>  The prefix directory (contains `drive_c` and `system.reg`)

Options:
      --json         Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                     games and --dry-run output)
      --name <NAME>  Name for the new bottle (default: the directory name)
      --move         Move instead of cloning (cloning is instant on APFS and leaves the original
                     untouched)
  -v, --verbose...   More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help         Print help
  -V, --version      Print version
```

## uncork bottle kill

Runs `wineserver --kill` for the bottle, then, when Steam is installed in it, resets `HKCU\Software\Valve\Steam\ActiveProcess\pid` to 0 so the next launch does not mistake a killed client for a running one.

```text
Stop every Windows process in a bottle

Usage: uncork bottle kill [OPTIONS] <NAME>

Arguments:
  <NAME>  Bottle name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle env

```text
Print `export` lines to use a bottle with Wine tools by hand

Usage: uncork bottle env [OPTIONS] <NAME>

Arguments:
  <NAME>  Bottle name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork bottle tool

Wine tools always run on WineD3D.

```text
Open a Wine tool: winecfg, regedit, taskmgr, explorer, cmd, control

Usage: uncork bottle tool [OPTIONS] <NAME> <TOOL>

Arguments:
  <NAME>  Bottle name
  <TOOL>  Tool name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork steam

```text
Install and use the Windows Steam client in a bottle

Usage: uncork steam [OPTIONS] <COMMAND>

Commands:
  install  Download Valve's installer and install Steam into a bottle
  start    Start the Steam client
  games    List games installed in a bottle's Steam libraries
  launch   Launch a Steam game by app id
  help     Print this message or the help of the given subcommand(s)

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork steam install

```text
Download Valve's installer and install Steam into a bottle

Usage: uncork steam install [OPTIONS]

Options:
  -b, --bottle <BOTTLE>  Bottle (default: `steam`)
      --json             Print machine-readable JSON instead of text (doctor, list, info, show,
                         inspect, games and --dry-run output)
  -v, --verbose...       More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -y, --yes              Answer yes to the download
  -h, --help             Print help
  -V, --version          Print version
```

## uncork steam start

Starts the client with its window visible. `--backend`, `--hud`, `--metalfx` and `--retina` apply to games, not to the client, which always gets DXVK app-locally ([STEAM.md](STEAM.md#graphics-steam-runs-on-dxvk)). `--env` and `--wine-debug` do apply.

```text
Start the Steam client

Usage: uncork steam start [OPTIONS]

Options:
  -b, --bottle <BOTTLE>
          Bottle to use (default: `default_bottle` from config, normally `steam`)

      --json
          Print machine-readable JSON instead of text (doctor, list, info, show, inspect, games and
          --dry-run output)

      --backend <BACKEND>
          Graphics backend for this launch

          Possible values:
          - auto:     Pick per game from its API and bitness
          - d3dmetal: Apple D3DMetal (64-bit D3D11/D3D12; import with `uncork runtime import-gptk`)
          - dxmt:     DXMT (D3D10/11 to Metal, 32- and 64-bit)
          - dxvk:     DXVK on MoltenVK (D3D9/10/11)
          - wined3d:  Wine's built-in WineD3D (OpenGL), the compatibility fallback

  -v, --verbose...
          More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides

      --hud
          Show Apple's Metal performance HUD (FPS, frame time, GPU)

      --metalfx
          Upscale with MetalFX where the backend supports it

      --retina
          Render at native Retina resolution

      --wine-debug <CHANNELS>
          Enable Wine debug channels (e.g. `+loaddll,+d3d`); output goes to the launch log

  -e, --env <KEY=VALUE>
          Extra environment variable for this launch (repeatable)

      --game-mode
          Start through a macOS Game Mode app bundle (experimental; see docs/PERFORMANCE.md)

      --dry-run
          Print the launch plan (command, environment, DLLs to install) and exit

      --wait
          Wait for the program and its wineserver to exit before returning

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## uncork steam games

```text
List games installed in a bottle's Steam libraries

Usage: uncork steam games [OPTIONS]

Options:
  -b, --bottle <BOTTLE>  Bottle (default: `steam`)
      --json             Print machine-readable JSON instead of text (doctor, list, info, show,
                         inspect, games and --dry-run output)
  -v, --verbose...       More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help             Print help
  -V, --version          Print version
```

## uncork steam launch

The same flow as `uncork play <appid>`, with the profile whose `[steam] appid` matches, if any.

```text
Launch a Steam game by app id

Usage: uncork steam launch [OPTIONS] <APPID> [-- <ARGS>...]

Arguments:
  <APPID>
          Steam app id

  [ARGS]...
          Arguments passed to the game

Options:
  -b, --bottle <BOTTLE>
          Bottle to use (default: `default_bottle` from config, normally `steam`)

      --json
          Print machine-readable JSON instead of text (doctor, list, info, show, inspect, games and
          --dry-run output)

      --backend <BACKEND>
          Graphics backend for this launch

          Possible values:
          - auto:     Pick per game from its API and bitness
          - d3dmetal: Apple D3DMetal (64-bit D3D11/D3D12; import with `uncork runtime import-gptk`)
          - dxmt:     DXMT (D3D10/11 to Metal, 32- and 64-bit)
          - dxvk:     DXVK on MoltenVK (D3D9/10/11)
          - wined3d:  Wine's built-in WineD3D (OpenGL), the compatibility fallback

  -v, --verbose...
          More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides

      --hud
          Show Apple's Metal performance HUD (FPS, frame time, GPU)

      --metalfx
          Upscale with MetalFX where the backend supports it

      --retina
          Render at native Retina resolution

      --wine-debug <CHANNELS>
          Enable Wine debug channels (e.g. `+loaddll,+d3d`); output goes to the launch log

  -e, --env <KEY=VALUE>
          Extra environment variable for this launch (repeatable)

      --game-mode
          Start through a macOS Game Mode app bundle (experimental; see docs/PERFORMANCE.md)

      --dry-run
          Print the launch plan (command, environment, DLLs to install) and exit

      --wait
          Wait for the program and its wineserver to exit before returning

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
```

## uncork profile

```text
List and show game profiles

Usage: uncork profile [OPTIONS] <COMMAND>

Commands:
  list  List profiles with their compatibility status
  show  Show one profile
  help  Print this message or the help of the given subcommand(s)

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork profile list

```text
List profiles with their compatibility status

Usage: uncork profile list [OPTIONS]

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork profile show

```text
Show one profile

Usage: uncork profile show [OPTIONS] <GAME>

Arguments:
  <GAME>  Profile id, Steam app id or part of the name

Options:
      --json        Print machine-readable JSON instead of text (doctor, list, info, show, inspect,
                    games and --dry-run output)
  -v, --verbose...  More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help        Print help
  -V, --version     Print version
```

## uncork winetricks

```text
Run winetricks verbs in a bottle (needs `brew install winetricks`)

Usage: uncork winetricks [OPTIONS] <VERBS>...

Arguments:
  <VERBS>...  Verbs, e.g. `corefonts vcrun2022`

Options:
  -b, --bottle <BOTTLE>  Bottle (default: `steam`)
      --json             Print machine-readable JSON instead of text (doctor, list, info, show,
                         inspect, games and --dry-run output)
  -v, --verbose...       More log output (-v info, -vv debug, -vvv trace). `RUST_LOG` overrides
  -h, --help             Print help
  -V, --version          Print version
```
