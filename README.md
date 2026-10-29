# Oche Core

Oche Core is a planned offline Rust service for automatic steel-tip dart scoring
with one to four USB cameras. Operators will configure and control a board from
the CLI or a terminal UI. A native API and a separately tested Autodarts
compatibility adapter will serve consuming applications.

**Status:** specification and repository setup only. There is no runnable service,
Rust workspace, verified compatibility profile, or measured scoring performance
yet.

## Project documents

- [Product and engineering roadmap](oche-core-roadmap.md): requirements,
  architecture, milestones, and release gates.
- [Discovery checklist](docs/discovery.md): evidence and decisions needed for M0.
- [Agent instructions](AGENTS.md): implementation and contribution conventions.

## Starting implementation

M0 establishes the hardware baseline, protocol contract, and replay corpus. The
independent parts of M1 can start alongside discovery: the Rust workspace, domain
types, validated configuration, persistent board identity, foreground service,
native control API, and CLI.

The compatibility adapter needs captured client behavior and sanitized fixtures.
Live scoring needs calibrated camera recordings with independent impact labels.
Synthetic events can exercise the service and client lifecycle, but do not
establish real-world accuracy or compatibility.

The development environment needs Rust, Cargo, rustfmt, and Clippy. Select and
record a Rust toolchain version and dependency versions during the initial
technical spike. This repository does not yet pin a toolchain or provide build
commands that can run successfully. Commit `Cargo.lock` when the workspace exists
because this project ships an application. Normal operation must work offline;
build dependencies and release artifacts must be prepared separately.

## Contribution workflow

Use [Conventional Commits](https://www.conventionalcommits.org/en/v1.0.0/):

```text
<type>[optional scope][!]: <short imperative description>
```

Examples: `chore: initialize repository`, `feat(domain): add visit state`, and
`fix(api): preserve event order after reset`. Keep commits focused and include
appropriate verification. See [AGENTS.md](AGENTS.md) for the engineering rules.
