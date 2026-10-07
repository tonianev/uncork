# Contributing to Uncork

This document covers how to set up a development machine, what to run before you push, the fastest way to contribute (game profiles and compatibility reports, no Rust needed), how to test with real games without touching your own setup, the pull request and commit rules, and the license terms you agree to by contributing. Who decides what is in [GOVERNANCE.md](GOVERNANCE.md). How AI tools are used is in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). The [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) applies in every project space.

## Prerequisites

| Tool | Why | Install |
|---|---|---|
| Xcode Command Line Tools | Linker and the macOS SDK. Full Xcode is not needed. | `xcode-select --install` |
| rustup | Installs the pinned toolchain from `rust-toolchain.toml` (Rust 1.99.0 with rustfmt and clippy) on your first cargo command. If you installed rustup through Homebrew (keg-only), add `/opt/homebrew/opt/rustup/bin` to `PATH`. | https://rustup.rs |
| just | Task runner for `just ci`. | `brew install just` |
| cargo-nextest, cargo-deny, typos-cli, cargo-machete, shellcheck | Optional locally (`just ci` skips a missing one with an install hint); CI runs them all. | `brew install cargo-nextest cargo-deny typos-cli shellcheck && cargo install cargo-machete --locked` |
| Rosetta 2 | Only for running real games. The test suite never starts Wine. | `softwareupdate --install-rosetta --agree-to-license` |

Running games needs an Apple Silicon Mac with macOS 26 or later, the oldest release the catalog's Wine runtime is built for. The test suite does not: it never starts Wine or downloads anything.

## Build and run

```bash
git clone https://github.com/tonianev/uncork.git
cd uncork
cargo build
cargo run -p uncork -- doctor
cargo test -p uncork-core
```

`cargo run -p uncork -- <command>` runs the CLI from the checkout. Point it at a scratch data directory with `UNCORK_HOME` (see "Testing with real games") so it never touches `~/Library/Application Support/Uncork`.

## Before you push: `just ci`

`just ci` runs the same gates as CI, in this order. CI is the source of truth; the local run saves you a round trip. Every compile uses the workspace's `ci` profile so one target directory is shared.

| Step | Command |
|---|---|
| Format | `cargo fmt --all -- --check` |
| Lint | `cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings` |
| Tests | `cargo nextest run --workspace --locked --cargo-profile ci --no-fail-fast` (falls back to `cargo test` when nextest is missing) |
| Doctests | `cargo test --doc --workspace --locked --profile ci` |
| Docs | `cargo doc --workspace --no-deps --locked --profile ci`, with `RUSTDOCFLAGS="-D warnings"` set by the justfile |
| Catalog pins | `scripts/check_catalog.sh` (offline lint of `runtime/catalog.toml`) |
| Labels | `scripts/sync_labels.sh --check` (`.github/labels.yml` matches the script) |
| Shell scripts | `shellcheck -x scripts/*.sh runtime/*.sh`, then `scripts/test_build_wine.sh` (offline tests of `runtime/build-wine.sh`) |
| Unused dependencies | `cargo machete` |
| Spelling | `typos` |
| Licenses, advisories, banned crates | `cargo deny --all-features check` (see `deny.toml`) |

`just catalog-verify` downloads every catalog archive and checks it against its pin; it is for maintainers and is not part of `just ci`.

The workspace lints are strict: `unsafe_code` is denied, `missing_docs` is a warning and every warning is an error in CI. A public item without a doc comment fails the build.

## The data-first path

Most of what makes a game run well is data, and most contributions need no Rust.

### Test a game and report it

Run a game with Uncork, then open a "Game report" issue from the [issue chooser](https://github.com/tonianev/uncork/issues/new/choose). [docs/COMPATIBILITY.md](docs/COMPATIBILITY.md) says what to include and how a report becomes an entry in the game's profile. Reports must be first-hand: a run you did yourself on your own Mac, never a result copied from ProtonDB, a forum or a model's guess ([AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md)).

### Write or fix a game profile

A profile is one TOML file in [profiles/](profiles/); the schema is in [profiles/README.md](profiles/README.md).

```bash
# 1. Learn the facts about the executable: bitness, graphics API, large-address-aware, NX.
uncork inspect "/path/to/Game/game.exe"
# 2. Try your profile without rebuilding: user profiles override built-ins with the same id.
mkdir -p "$UNCORK_HOME/profiles" && cp profiles/my-game.toml "$UNCORK_HOME/profiles/"
uncork profile show my-game
uncork play my-game --dry-run
# 3. Validate every built-in profile (the unit tests parse and check each file).
cargo test -p uncork-core
```

A profile PR states in its description which macOS version, chip and Uncork commit it was tested on. Comments in the TOML say why each non-default setting is there, with a source where one exists.

### Update a catalog pin

`runtime/catalog.toml` pins every downloadable component by URL, size and SHA-256. Changing a pin has its own procedure in [docs/RUNTIME.md](docs/RUNTIME.md#updating-a-pin): the author downloads the file and computes the hash themselves.

## Testing with real games

Never test a branch against your everyday data. Set `UNCORK_HOME` and everything Uncork writes (components, bottles, profiles, caches, logs) goes there instead of `~/Library/Application Support/Uncork`:

```bash
export UNCORK_HOME="$HOME/UncorkDev/my-branch"
cargo run -p uncork -- setup
cargo run -p uncork -- play rise-of-nations --dry-run   # read the plan before running it
cargo run -p uncork -- play rise-of-nations --wine-debug +loaddll
```

- Keep `UNCORK_HOME` on an APFS volume. Bottle import clones with APFS (`cp -c`), and exFAT has no symbolic links, which a Wine prefix's `dosdevices/` directory is made of.
- Every launch writes a log to `$UNCORK_HOME/logs/<bottle>-<program>-<unix seconds>.log`. `--wine-debug <channels>` turns on Wine debug channels and the backends' own logs for that launch.
- To reuse an existing Steam login across branches, import the bottle instead of signing in again: `uncork bottle import "$OLD_HOME/bottles/steam" --name steam` clones it. A CrossOver Steam bottle works the same way (`uncork bottle import "$HOME/Library/Application Support/CrossOver/Bottles/Steam" --name steam`); the clone runs with `USER=crossover` so Steam finds its sign-in, and CrossOver's copy is not touched.
- `uncork play <game> --dry-run` and `uncork steam start --dry-run` print the exact commands without starting anything.
- Stop a bottle before deleting it: `uncork bottle kill steam`, then `uncork bottle delete steam`.
- Steam's own logs under `drive_c/Program Files (x86)/Steam/logs/` contain your account name. Scrub them before attaching them to an issue.

## Code rules

These apply to every change under `crates/`.

1. The doc comments are the specification. A change in behavior changes the doc comment in the same commit, and the user-facing docs in `docs/` that describe it; a reviewer reads the diff of the docs first. A change to help text in `crates/uncork/src/cli.rs` regenerates [docs/CLI.md](docs/CLI.md).
2. No `unsafe` (denied workspace-wide). No `unwrap()` or `expect()` in library code except on an invariant documented next to it.
3. Errors carry context: the path, the command or the component involved and, where there is one, the command that fixes it. Libraries use `thiserror`; only the `uncork` binary uses `anyhow`.
4. No silent fallbacks. A backend the user asked for that is not installed or cannot run the game is an error naming the install command.
5. Never run a program through `/bin/sh`, `env`, `nohup` or `arch`: SIP strips `DYLD_*` variables from them. Describe it as a `CommandSpec` and spawn it directly.
6. Wine processes get a constructed environment ([docs/ARCHITECTURE.md](docs/ARCHITECTURE.md#launch-planning)). Do not inherit the user's shell environment.
7. Uncork is synchronous. `tokio` and `openssl-sys` are banned in `deny.toml`; HTTPS is `ureq` with rustls.
8. Tests are hermetic: a `tempfile` directory as the data root (`Layout::at` in unit tests, `UNCORK_HOME` on the child process in CLI tests; `std::env::set_var` is `unsafe` in Rust 2024 and therefore unavailable), fake `wine` and `wineserver` shell scripts where a program must run, no network, no real `~/Library`, no Wine. Use `proptest` where the contract states an invariant such as a round trip.
9. A new dependency needs a reason in the PR description, goes into `[workspace.dependencies]`, and must pass `cargo deny check`.
10. Do not copy code from GPL, LGPL or AGPL projects (Whisky, Highball, Silo, MetalSharp, Wine itself) into `crates/`. Reading them to learn how something works is fine; cite the source in a comment.

## Pull requests

| Kind of PR | Rule |
|---|---|
| Game profile (`profiles/`) | Welcome anytime. `cargo test -p uncork-core` passes; tested hardware and commit in the description. |
| Compatibility report | Welcome anytime, as a "Game report" issue or a `[[compat.reports]]` entry in a profile PR. First-hand only. |
| Docs | Welcome anytime. Run `typos`. Every non-obvious technical claim cites a primary source; a measurement says when, on what hardware and with which versions it was made. |
| Tests | Welcome anytime. |
| Catalog pin (`runtime/catalog.toml`) | Follow [docs/RUNTIME.md](docs/RUNTIME.md#updating-a-pin). The SHA-256 must be computed by the author from the downloaded file. |
| Code | Open an issue first so the change can be matched to a milestone in [docs/ROADMAP.md](docs/ROADMAP.md). The PR includes tests. |
| Architecture change | An ADR in [docs/adr/](docs/adr/README.md) before or with the change. |

The PR template has a checklist per kind, including the AI-assistance checkbox described in [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md). CI is the gate; the checklist is a guide. Expect a first response within 7 days.

## Commit style

- Imperative subject line of 72 characters or fewer: `Add Age of Mythology profile`, not `Added` or `Adds`.
- A body that says why, when the subject is not enough. Reference issues with `Closes #12`.
- No AI attribution trailers: no `Co-Authored-By` lines for tools and no "generated with" footers. Disclosure belongs in the PR template checkbox, not in the git history.

## License

Code, profiles, documentation and scripts are dual-licensed under MIT OR Apache-2.0. See [LICENSE-MIT](LICENSE-MIT) and [LICENSE-APACHE](LICENSE-APACHE). There is no CLA and no DCO.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.

The components Uncork downloads keep their own licenses. Patches to Wine or DXMT are derivative works of LGPL-2.1-or-later code and are contributed under that license; they belong to the runtime build described in [docs/RUNTIME.md](docs/RUNTIME.md), not to `crates/`. See [docs/adr/0001-license.md](docs/adr/0001-license.md).
