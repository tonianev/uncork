<!-- Thanks for the PR. CI is the gate; this checklist is a guide. Delete the tiers that do not apply. -->

## What and why

<!-- One paragraph. Link the issue: "Closes #123". -->

## Tier

Tick the tier that matches the change and complete its items.

### Profile PR (files under `profiles/`)

- [ ] `just profiles-check` passes
- [ ] The file name equals the profile's `id`
- [ ] Every non-default setting (`env`, `dll_overrides`, `[[ini]]`) says why in a comment or `reason`
- [ ] Every `[[compat.reports]]` entry is a run I did myself, on my own Mac, with my own copy of the game

### Docs PR (files under `docs/`, `README.md`, `CONTRIBUTING.md`)

- [ ] `typos` passes
- [ ] Links are relative paths and resolve
- [ ] Claims about Wine, Apple or Valve behavior link a primary source

### Code PR (files under `crates/`, `scripts/`, `runtime/build-wine.sh`, `runtime/patches/`, `.github/`)

- [ ] `just ci` passes locally
- [ ] A test covers the change
- [ ] No `unsafe`, and no `unwrap()` or `expect()` in library code except on a documented invariant
- [ ] A new dependency goes in `[workspace.dependencies]`, passes `cargo deny check`, and this PR says why it is needed
- [ ] Shell scripts pass `shellcheck`; Wine patches follow `runtime/patches/README.md`

### Catalog-pin PR (`runtime/catalog.toml`)

- [ ] I downloaded the archive myself and hashed it myself (`shasum -a 256`, `wc -c`); the SHA-256 and size are not copied from a release page or another project
- [ ] `scripts/check_catalog.sh` passes
- [ ] `license` and `source_code` are right for this exact version, and its source code is published where `source_code` says
- [ ] I installed it with `uncork runtime install` and ran a game with it (name the game and backend under "How to verify")
- [ ] The component may be redistributed and downloaded by Uncork (never D3DMetal or anything else under a no-redistribution license)

## Licensing and disclosure

- [ ] I agree that, unless I explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by me, as defined in the Apache-2.0 license, shall be dual licensed as MIT OR Apache-2.0, without any additional terms or conditions (CONTRIBUTING.md). Wine patches under `runtime/patches/` are LGPL-2.1-or-later, like Wine.
- [ ] No code in this PR is copied from a GPL, LGPL or AGPL project into Uncork's own code.
- [ ] AI assistance: I have disclosed any AI-assisted parts of this change per AI_CONTRIBUTIONS.md, and I have reviewed and tested them myself.

## How to verify

<!-- Commands a reviewer runs and what they should see. For a profile or catalog change, the Mac, macOS version and result of your own run. -->
