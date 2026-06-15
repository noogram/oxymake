# OxyMake — Agent Instructions

OxyMake is a Rust workspace for next-generation workflow orchestration.

This file is a **transport layer**: it points to where the canonical
instructions live, it does not restate them. `CLAUDE.md` is a symlink to this
file, so Claude Code and every other AGENTS.md-aware tool read the same source.

## Where the instructions live

- **Definition of Done, testing policy (TDD), coding conventions, CHANGELOG and
  public-surface discipline, shell-automation rules** →
  [`CONTRIBUTING.md`](CONTRIBUTING.md). This is the single source of truth for
  what "done" means and how to get there. Read it before touching code.
- **CLI usage** → `ox help` (or `man ox` / `man oxymake`), plus the reference
  chapter of the book under [`docs/book/src/reference/`](docs/book/src/reference/).
- **Operator handbook** → `ox guide` (or `ox help guide`) — a concise
  orientation that points to the canonical docs.
- **Concepts and guides** → the mdBook under
  [`docs/book/src/`](docs/book/src/), starting at `introduction.md`.
- **Architecture decisions** → [`docs/adr/`](docs/adr/) (index in
  `docs/adr/README.md`); exploratory notes in [`docs/design/`](docs/design/).
- **Agent / MCP access** → `ox serve --mcp` exposes the workflow engine over
  the Model Context Protocol (stdio), so an agent can drive `ox` directly.

Everything above works from a fresh clone with only the `ox` binary — no extra
tooling required.

## Cosmon (maintainer-only — external contributors can ignore)

The maintainer develops this project with **cosmon**, a private agent
orchestrator (`cs`) that is **not part of this repository**. If you are an
external contributor you do not have `cs` and do not need it: the public
onboarding chain above (CONTRIBUTING.md → `ox help` / `ox guide` → `ox serve
--mcp`) is complete on its own. The only cosmon artifact that ships publicly is
[`.cosmon/artifact-map.toml`](.cosmon/artifact-map.toml) — a curated CI input
documented in its own header (see ADR-017); the live `.cosmon/state/` is
local-only and never tracked.

## How this project was built

The pre-cosmon ("Gas Town") era and the later cosmon-driven phase are
chronicled in [`docs/MAKING-OF.md`](docs/MAKING-OF.md) — history, not a setup
requirement.
