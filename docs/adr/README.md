# Architecture decision records

This directory holds Uncork's architecture decision records. An ADR is a short, numbered document that captures one decision that is expensive to reverse: what was decided, why, what else was considered and what follows from it. It is written when the decision is made, not after, and it is never edited to change the decision; a new ADR supersedes it instead. Scope changes go through [ROADMAP.md](../ROADMAP.md) and an ADR ([GOVERNANCE.md](../../GOVERNANCE.md)). Bug fixes, new profiles, catalog pin updates and features inside the agreed scope do not need one.

## Index

| Number | Title | Status | Date |
|---|---|---|---|
| [0001](0001-license.md) | Licensing of Uncork and of the components it uses | Accepted | 2026-10-07 |
| [0002](0002-orchestrate-not-reimplement.md) | Orchestrate Wine and the translation layers, do not reimplement them | Accepted | 2026-10-07 |
| [0003](0003-per-process-backends.md) | Choose the graphics backend per process | Accepted | 2026-10-07 |
| [0004](0004-runtime-supply.md) | Runtime supply: pinned upstream builds first, Uncork's own CI build next | Accepted | 2026-10-07 |

## Statuses

| Status | Meaning |
|---|---|
| Proposed | Written, not yet in force |
| Accepted | In force |
| Superseded by NNNN | Replaced; the text stays for history |
| Rejected | Considered and declined; kept so the question is not reopened without new facts |

## When to write one

Write an ADR for a decision that changes a public boundary or is costly to undo: a license, the runtime supply model, how backends are wired into Wine, the bottle or profile format, a new crate boundary, the GUI technology, the CPU backend, a change to a governance invariant. Do not write one for a profile, a catalog pin, a refactor inside a crate or a new optional setting.

## Naming

`NNNN-short-kebab-title.md`, four digits, zero-padded, in order of creation. Update the index table above in the same PR.

## Template

```markdown
# NNNN: Title

Status: Proposed | Accepted | Superseded by NNNN | Rejected
Date: YYYY-MM-DD (date the status was last changed)

## Context

What is the situation and what forces are at play. Facts, with dates and sources. Short.

## Decision

What was decided, in the imperative. One paragraph or a short list.

## Alternatives considered

| Alternative | Why not |
|---|---|
| | |

## Consequences

What becomes easier, what becomes harder, what must now be done, and what enforces the decision.
```
