# Working on Uncork

A guide for anyone, person or coding agent, who changes this repository or
drives the `uncork` command from a script. It collects the facts that are
easy to get wrong and says where the longer explanations live. Policy for
AI-assisted changes is in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md); this
file is about doing the work.

## What Uncork is

A Rust command-line tool that runs Windows games on Apple Silicon Macs by
orchestrating a Wine runtime (x86-64 under Rosetta 2), a Metal translation
layer (DXMT, DXVK on MoltenVK, or Apple's D3DMetal) and the Windows Steam
client inside a "bottle" (a Wine prefix). It reimplements none of them
([ADR 0002](docs/adr/0002-orchestrate-not-reimplement.md)). Game support is
data: one TOML profile per game under `profiles/`.

## Commands you will use

| Do | Run |
|---|---|
| Build and run from source | `just run doctor`, `just run play rise-of-nations --dry-run` |
| Everything CI runs, in CI's order | `just ci` (fmt check, clippy with the strict config, nextest, doctests, rustdoc, catalog lint, typos, cargo-deny). Run it before every push |
| One crate's tests | `cargo nextest run -p uncork-core` (or `cargo test -p uncork-core`) |
| Install the binary into `~/.cargo/bin` | `just install` |
| Validate every game profile, the schema and the compat table | `just profiles-check` (`just profiles-schema` regenerates `profiles/schema.json`) |
| Rewrite `docs/CLI.md` after a help-text change | `just cli-doc` |
| Lint the component catalog offline | `just catalog-check` |

`rustup` is a keg-only Homebrew install on the maintainer's Mac; if `cargo`
is missing from `PATH`, add `$(brew --prefix rustup)/bin`.

## Repository map

| Path | What |
|---|---|
| `crates/uncork` | The CLI. `src/cli.rs` declares the whole surface with clap derive; `src/commands/` holds one handler module per command; `src/output.rs` prints text, JSON and errors |
| `crates/uncork-core` | The engine. `lib.rs` has a module table; the big ones are `launch` (plans), `steam` (client lifecycle, game launch, stopping a bottle), `bottle` and `registry` (prefixes and their settings), `graphics` (backend choice and per-process activation), `display` (the main display) |
| `crates/uncork-pe` | PE32/PE32+ inspection: bitness, imports, graphics API, flags such as NX_COMPAT and large-address-aware |
| `crates/uncork-steam` | Steam data formats and client facts: VDF/KeyValues, library folders, app manifests, client arguments, the `ActiveProcess` registry key |
| `profiles/` | Built-in game profiles and their format (`profiles/README.md`) |
| `runtime/catalog.toml` | The pinned components (URL, SHA-256, size, license) that `uncork runtime install` downloads; `runtime/build-wine.sh` builds Wine from CrossOver sources |
| `docs/` | `ARCHITECTURE.md` (design and invariants), `CLI.md` (every `--help`, verbatim), `STEAM.md`, `PERFORMANCE.md`, `COMPATIBILITY.md` (status levels and first-hand results), `RUNTIME.md`, `LEGAL.md`, `ROADMAP.md`, `adr/` |
| `scripts/` | Catalog checks and the GitHub label sync, all called from the `justfile` |

## Rules that are easy to break

The full list is in [CONTRIBUTING.md](CONTRIBUTING.md#code-rules) and the
invariants table in [ARCHITECTURE.md](docs/ARCHITECTURE.md#invariants).
These are the ones that have cost real time:

1. **Doc comments are the specification.** A behaviour change edits the doc
   comment and the page under `docs/` in the same commit. A change to any
   help text in `cli.rs` is followed by `just cli-doc`, which rewrites
   `docs/CLI.md` from the binary (each section is a command's `--help` at
   `COLUMNS=100`, unedited); CI fails when the two differ.
2. **One msync setting per wineserver.** `WINEMSYNC` belongs to the
   wineserver, and a Wine command whose setting differs exits 1 with no
   output. Every Wine command in a bottle, including `reg`, `regedit`,
   `wineboot` and `wineserver`, goes through `launch::in_bottle`; never
   build a Wine command by hand.
3. **Retina mode and DPI are a pair.** `HKCU\Software\Wine\Mac Driver`
   `RetinaMode` and `HKCU\Control Panel\Desktop` `LogPixels` must agree
   (`n` with 96, `y` with 192), and Wine reads them only when its wineserver
   starts. Change them only through `bottle::set`, which stops the bottle
   first. A mismatch gives a DPI-unaware game a half-size screen: cropped
   full screen and crashes in its renderer.
4. **Wine gets a constructed environment**, never the shell's
   (`launch::ALLOWED_ENV`, `launch::base_env`). Programs are spawned
   directly, never through `/bin/sh`, `env`, `nohup` or `arch`, because SIP
   strips `DYLD_*` from those.
5. **`wine reg query` starts a wineserver when none runs.** Anything that
   must be read-only asks `steam::server_running` (`wineserver -k0`) first.
6. **Never redistribute D3DMetal or the Steam client**, and the catalog
   rejects `d3dmetal` entries ([LEGAL.md](docs/LEGAL.md)).
7. **Compatibility claims are first-hand and dated.** A profile's
   `[compat] status` is set only after a session through the checklist in
   [COMPATIBILITY.md](docs/COMPATIBILITY.md); reaching the main menu does
   not make a game `playable`. Never copy a result from ProtonDB, a forum or
   a guess.
8. **No AI attribution in git history.** No `Co-Authored-By` for tools, no
   "generated with" footers. Disclosure is the checkbox in the PR template.
9. **No `unsafe`, no `unwrap()`/`expect()` in library code** (except on a
   documented invariant), `thiserror` in libraries and `anyhow` only in the
   binary, no `tokio`, no OpenSSL, no code copied from GPL/LGPL projects.

## Tests

Tests are hermetic: a `tempfile` data root (`Layout::at` in unit tests,
`UNCORK_HOME` on the child process in CLI tests), fake `wine` and
`wineserver` shell scripts (`crates/uncork-core/tests/d_support/`) that
record their calls and imitate the few Windows programs the code runs, no
network, no Rosetta, no real `~/Library`. `UNCORK_DISPLAYS_JSON` replaces
the Mac's display list. A code PR includes a test; a bug fix includes the
test that would have caught it, and it is worth checking that it fails
without the fix.

`std::env::set_var` is `unsafe` in Rust 2024 and therefore unavailable; set
variables on the child `Command` instead.

## Running real games while developing

Only a person at the Mac can judge a game. Rules for agents working on this
machine:

- Read-only commands are always fine: `uncork doctor`, `uncork status`,
  `uncork logs`, `uncork profile ...`, `uncork bottle info`, `uncork --json
  ... --dry-run`.
- `uncork play`, `uncork run`, `uncork steam start|launch`, `uncork bottle
  kill|set|delete|import` start or stop Wine and Steam in the user's bottle.
  Run them only when asked to test, and say what you ran.
- Never take a full-screen capture while the user may be working; capture
  the game window by id (`screencapture -l <window id>`).
- While the screen is locked, macOS reports a full-screen Wine window at
  about 90% of its size, centred, and `screencapture` fails. That is not a
  bug in the game or in Uncork; `WINEDEBUG=+macdrv` shows the real frame.
- Steam keeps the last session's signed-in account in the registry for a
  few seconds after it starts. Uncork clears it before starting the client;
  a game started before `[Logged On` appears in
  `Steam/logs/connection_log.txt` crashes in its Steamworks exit code.

## Where state lives

`$UNCORK_HOME`, by default `~/Library/Application Support/Uncork`: global
`config.toml`, `components/<kind>/<version>/`, `bottles/<name>/` (the Wine
prefix, with Uncork's `uncork.toml` and `uncork-state.toml` at its root),
user `profiles/`, `cache/downloads/`, `logs/`. The full tree is in
[ARCHITECTURE.md](docs/ARCHITECTURE.md#data-on-disk). Nothing outside it is
written except what Wine writes inside a prefix.

## Machine-readable use

`--json` is a global flag. Where it applies, stdout is exactly one JSON
value and nothing else; progress and warnings go to stderr through
`tracing` (`-v` adds more). It covers `doctor`, `status`, `logs`,
`inspect`, every `list`/`info`/`show`/`games`, every `--dry-run`, a real
launch (`play`, `run`, `steam launch` print the pid, log path, bottle,
backend and mode, plus the exit status with `--wait`), and every failure
(`{"error": {"kind", "message", "causes", "exit_code"}}` with a distinct
exit code per kind of failure). Without a terminal on stdin nothing is
downloaded unless `-y`/`--yes` is given, and questions take their default.
The reference, with the exit-code table and the launch object, is in
[CLI.md](docs/CLI.md).

A typical agent loop: `uncork --json status` to see what runs, `uncork
--json play <game>` to start it, `uncork --json logs --game <game>` when
it fails, and `uncork --json doctor` when the install itself is in doubt.

## Adding a game

1. `uncork profile new <appid>` scaffolds a profile from the installed game
   (its executable, bitness, graphics API, the recommended backend).
2. Edit it (`profiles/README.md` is the format; `.taplo.toml` points
   editors at `profiles/schema.json`), then `uncork profile validate <file>`
   (exit 4 and one `file:line: message` per problem).
3. Test with `uncork play <id> --hud`, read `uncork logs --game <id>`,
   iterate.
4. Fill `[compat]` from the checklist in
   [COMPATIBILITY.md](docs/COMPATIBILITY.md) and add the row to its
   "Current results" table; CI checks that the two agree.

## Debugging a launch

`uncork play <game> --dry-run --json` shows the plan: backend, DLLs copied
into the prefix, overrides, environment, INI edits. `uncork logs` shows
the newest launch log of the default bottle (`--game <id>` for one game,
`--bottle` for another bottle, `--path` for just the file); Wine's
`winedbg` backtraces land there. `uncork status` says what runs in each
bottle without starting anything. For a
display or window problem add `--env WINEDEBUG=+macdrv,+display`; for a
backend problem `--env DXMT_LOG_LEVEL=info` or `DXVK_LOG_LEVEL=info`. The
game's own files are under
`drive_c/users/<user>/AppData/Roaming/...` in the bottle.
