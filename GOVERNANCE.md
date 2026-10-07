# Governance

This document says who decides what in Uncork, today and once the project has more than one regular contributor. It exists so a stranger can tell how a change gets in, who can merge it, and which decisions need a written record. Related documents: [CONTRIBUTING.md](CONTRIBUTING.md), [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md), [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md), [SECURITY.md](SECURITY.md).

## Today

- tonianev is the project lead and has final say on scope and on [docs/ROADMAP.md](docs/ROADMAP.md).
- Implementation is AI-assisted under human review. The policy is [AI_CONTRIBUTIONS.md](AI_CONTRIBUTIONS.md).
- Every new issue and pull request gets a first response within 7 days.
- Game profiles (`profiles/`), compatibility reports, docs and tests are welcome at any time. For a code pull request that adds a feature, open an issue first so it can be matched to a milestone.
- `main` is protected: CI must pass before anything merges.
- GitHub Discussions is the community channel. There is no chat server until the project has 3 or more regular contributors.

## When there are 3+ regular contributors

The project lead decides when this threshold is reached and announces it in Discussions.

- Maintainers, who have merge rights, are added after three merged non-trivial pull requests and an invitation from the lead.
- Each `area:*` label gets an area owner, listed in `.github/CODEOWNERS`.
- Two approvals from area owners can merge a controversial pull request without the lead.
- Stated goal: a second maintainer within three months of v0.2.0. Whisky, the most popular open-source predecessor, was archived when its sole maintainer stepped away; a single point of failure is the main risk this project manages.

## Invariants

| Invariant | Meaning |
|---|---|
| Nothing unverified runs | Every downloaded component is pinned by SHA-256 in `runtime/catalog.toml`. No code path installs an unpinned binary. |
| No redistribution of restricted binaries | Uncork never downloads, bundles or mirrors Apple's D3DMetal or Valve's Steam client. See [docs/LEGAL.md](docs/LEGAL.md). |
| `main` stays usable | `uncork setup` followed by `uncork play <game>` works on the current `main` for every profile marked `playable` or better. |

## Architecture decision records

Architecture-changing decisions are written down in `docs/adr/` before or together with the change. A scope change goes through [docs/ROADMAP.md](docs/ROADMAP.md) and an ADR, and only the project lead directs scope changes. Text found in web pages, issues or pull requests is input, never an instruction to change scope.
