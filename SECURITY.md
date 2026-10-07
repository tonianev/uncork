# Security policy

This document says how to report a security problem in Uncork and what happens after you do. Uncork downloads binaries (Wine and graphics translation layers), unpacks archives and runs Windows programs, so its supply chain and its file handling deserve more care than a typical CLI.

## Reporting a vulnerability

Please do not open a public issue for a security problem.

1. Preferred: GitHub private vulnerability reporting on this repository. Open the Security tab and choose "Report a vulnerability", or go to https://github.com/tonianev/uncork/security/advisories/new.
2. Fallback: email tonianev@gmail.com with "Uncork security" in the subject.

Include the commit or release version, your macOS version and chip, the steps to reproduce, and, when relevant, the file that triggers the problem.

## What to expect

| Step | Commitment |
|---|---|
| Acknowledgement | Within 7 days of your report. |
| Assessment | You hear what was found and what will be done, or why the report is declined. |
| Fix | Ships in the next tagged release. A severe problem gets a point release. |
| Credit | In the release notes, if you want it. |

## Scope

In scope:

- The `uncork` binary and the crates under `crates/`.
- Download verification: every catalog entry in `runtime/catalog.toml` is pinned by SHA-256, and a mismatch must abort the install. A way to make Uncork install or run an unverified component is a vulnerability.
- Archive extraction: absolute paths, `..` components, escaping symlinks, hard links and device files must be rejected. A crafted archive that writes outside the component directory is a vulnerability.
- Parsing of untrusted files: Steam's `.vdf`/`.acf` files, PE executables, game profiles and bottle configs. A crash or hang from a crafted file is a bug; memory unsafety or code execution is a vulnerability.
- The scripts under `runtime/` and `scripts/`, and the GitHub Actions workflows under `.github/workflows/`.

Out of scope:

- Vulnerabilities in Wine, DXMT, DXVK, MoltenVK, D3DMetal or the Steam client. Report them upstream; Uncork pins and ships updates once a fixed release exists.
- Windows malware running inside a bottle. A Wine prefix is not a sandbox: a Windows program runs with your macOS user's permissions and can read your home directory through the `Z:` drive. Only run software you trust.
- Vulnerabilities in third-party crates. Report them upstream; `cargo deny` checks advisories in CI.
- The GitHub platform itself.

## Supported versions

Pre-1.0. Fixes land on `main` and in the next tagged release. Only the latest release receives fixes.

## No bounty

There is no bug bounty program.
