# ADR 0: Record Architecture Decisions

Date: 2026-11-21

## Status
Accepted

## Context
charlesnetfs needs a place to record architecture decisions so future contributors understand why the code looks the way it does. Without ADRs, decisions live only in chat logs and commit messages, which are hard to find and easy to lose.

## Decision
Use [Michael Nygard's minimal ADR format](https://cognitect.com/blog/2011/11/15/documenting-architecture-decisions): one markdown file per decision, numbered `0000`-`NNNN`, flat directory `docs/adr/`, no status subdirectories. Each ADR has Status, Context, Decision, Consequences. Keep under one page.

The first ADR (this one) is the bootstrap. Numbering starts at 0000. New ADRs take the next free number. Supersede rather than renumber.

## Consequences
- `docs/adr/` exists and is tracked in version control.
- Contributors add a file before making a significant architectural change.
- Readers see the rationale alongside the code, not just the outcome.
