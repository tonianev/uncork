# AI contributions

This document states how AI tools are used in Uncork and what is expected of a pull request written with AI help. It exists so contributors know where the code came from and so reviewers apply one standard to every change.

## How this codebase was made

Uncork is AI-assisted. The research, the design and the first implementation were produced with Claude under the direction of the project lead, who reviews, runs and edits what the agents produce before it merges. In short, the maintainer licenses whatever copyright exists under MIT OR Apache-2.0, and to the extent portions are not copyrightable, recipients may treat them as public domain, which is strictly more permissive.

## AI-assisted pull requests

AI-assisted pull requests are welcome when all of the following hold:

1. A human author ran the change locally and understands it. You should be able to answer a review question about any line in the diff.
2. CI passes, and a code pull request includes a test, exactly as for any other code PR. See [CONTRIBUTING.md](CONTRIBUTING.md).
3. Compatibility claims are first-hand. A game profile's `[[compat.reports]]` entry records a run you did yourself on your own Mac, never a result copied from ProtonDB, a forum or a model's guess.
4. You tick the AI-assistance checkbox in the pull request template, [.github/PULL_REQUEST_TEMPLATE.md](.github/PULL_REQUEST_TEMPLATE.md). Disclosure is normal here and does not count against the PR.

Review applies the same standard to every PR, whoever or whatever typed it. A PR whose author cannot explain it is closed, not finished by the reviewer.

## Not accepted

- Unreviewed output: a PR that pastes agent output without running it, or whose description does not match its diff.
- Catalog pins (`runtime/catalog.toml`) whose SHA-256 was not computed from the downloaded file by the author.
- Text copied from sources with an incompatible license.

## Commit messages

Commit messages and PR descriptions carry no AI attribution trailers. The commit style is in [CONTRIBUTING.md](CONTRIBUTING.md).
