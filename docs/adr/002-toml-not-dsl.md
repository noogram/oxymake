# ADR-002: TOML Workflow Format, Not a Custom DSL

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context
Snakemake embeds workflow definitions in Python, making the Snakefile a
Turing-complete program. This breaks static analysis, IDE support, and
deterministic parsing. The workflow definition can have bugs in its own
logic, separate from the computation it orchestrates.

## Decision
OxyMake uses TOML as the workflow definition format. The Oxymakefile is
declarative data, not code. Individual rules execute code via `shell`,
`run`, `script`, or `call` modes, but the orchestration itself is static.

A minimal expression language (pure functions, no loops, no I/O except
`glob()` at parse time) handles computed config values.

## Consequences
- Workflow files are parseable in microseconds (no Python import time)
- Static analysis, schema validation, and LSP support are straightforward
- The workflow is not Turing-complete (by design)
- Complex config generation must happen outside the Oxymakefile (via scripts)
- Users familiar with Snakemake's Python flexibility may find TOML limiting

## Alternatives Considered
- **Python DSL** (Snakemake): powerful but breaks static analysis, couples to Python runtime
- **YAML** (Nextflow config, CWL): flexible but has the Norway problem, implicit type coercion
- **Custom DSL**: maximum expressiveness but requires writing a parser, no existing tooling
- **Starlark** (Bazel): good middle ground but adds a dependency and learning curve
