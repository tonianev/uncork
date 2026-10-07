# Uncork task runner. `just` lists recipes; `just ci` mirrors .github/workflows/ci.yml.
# Optional tools (nextest, machete, typos, deny, shellcheck) are skipped with an install hint.

set shell := ["bash", "-euo", "pipefail", "-c"]

# rustup is keg-only in Homebrew; make cargo/rustc visible without a shell profile.
export PATH := "/opt/homebrew/opt/rustup/bin:" + env("PATH")
export CARGO_TERM_COLOR := "always"
export RUSTDOCFLAGS := "-D warnings"
export RUST_BACKTRACE := "1"

# List recipes.
default:
    @just --list --unsorted

# Everything CI runs (the check job, then the deny job), in the same order.
ci:
    #!/usr/bin/env bash
    set -euo pipefail
    have() { command -v "$1" >/dev/null 2>&1; }
    # cargo plugins may live in ~/.cargo/bin without being on PATH; ask cargo.
    have_cargo() { cargo "$1" --version >/dev/null 2>&1; }
    skip() { echo "skip: $1 not installed ($2)"; }
    step() { echo; echo "==> $*"; }

    step rustfmt
    cargo fmt --all -- --check

    step clippy
    cargo clippy --workspace --all-targets --locked --profile ci -- -D warnings

    step tests
    if have_cargo nextest; then
      cargo nextest run --workspace --locked --cargo-profile ci --no-fail-fast
    else
      echo "hint: cargo-nextest not installed (brew install cargo-nextest); falling back to cargo test"
      cargo test --workspace --locked --profile ci
    fi
    step doctests
    cargo test --doc --workspace --locked --profile ci

    step rustdoc
    cargo doc --workspace --no-deps --locked --profile ci

    step catalog pins
    scripts/check_catalog.sh
    step labels
    scripts/sync_labels.sh --check
    step shell scripts
    if have shellcheck; then shellcheck -x scripts/*.sh runtime/*.sh; else skip shellcheck "brew install shellcheck"; fi
    scripts/test_build_wine.sh

    step unused dependencies
    if have_cargo machete; then cargo machete; else skip cargo-machete "cargo install cargo-machete --locked"; fi
    step spelling
    if have typos; then typos; else skip typos "brew install typos-cli"; fi
    step licenses and advisories
    if have_cargo deny; then cargo deny --all-features check; else skip cargo-deny "brew install cargo-deny"; fi

    echo; echo "ci: all steps passed"

# Format the whole workspace.
fmt:
    cargo fmt --all

# Check formatting without changing files.
fmt-check:
    cargo fmt --all -- --check

# Run the CLI from source, e.g. `just run doctor` or `just run play "Rise of Nations"`.
[positional-arguments]
run *ARGS:
    cargo run -p uncork -- "$@"

# Install the `uncork` binary into ~/.cargo/bin.
install:
    cargo install --path crates/uncork --locked

# Parse and validate every game profile under profiles/.
profiles-check:
    cargo test -p uncork-core profile

# Lint runtime/catalog.toml offline (URLs, SHA-256 pins, sizes, licenses).
catalog-check:
    scripts/check_catalog.sh

# Maintainers: download every catalog archive and check it against its pin (large downloads).
[positional-arguments]
catalog-verify *ARGS:
    scripts/verify_catalog_pins.sh "$@"

# Create or update the GitHub labels from .github/labels.yml.
labels REPO="tonianev/uncork":
    scripts/sync_labels.sh {{ quote(REPO) }}

# Experimental: build Wine from CrossOver sources (see `just runtime-build --help`).
[positional-arguments]
runtime-build *ARGS:
    runtime/build-wine.sh "$@"
