# ADR-003: Subprocess + Arrow IPC for Language Interop, Not PyO3

## Status
Accepted

## Metadata

- **Kind:** `decision`
- **Family:** `EXT`
- **Supersedes:** `none`

## Context
The `call` execution mode requires OxyMake (a Rust binary) to invoke
functions in Python/R/Julia and pass typed objects. Three approaches
were considered.

## Decision
Use subprocess + Apache Arrow IPC for language interop in v0.1.

OxyMake spawns a language-specific worker process and communicates via
Arrow IPC (zero-copy for DataFrames) or JSON for non-tabular objects.
Workers are reused across sequential `call`-mode jobs in the same
environment to amortize startup cost.

## Consequences
- No coupling to a specific Python ABI version
- Works with any language that has Arrow bindings (Python, R, Julia)
- Compatible with `uv`-managed isolated environments
- Worker startup adds ~100ms latency on first call (amortized by reuse)
- Cannot pass arbitrary Python objects — only Arrow-serializable types
- PyO3 embedding remains a possible Phase 3 optimization

## Alternatives Considered
- **PyO3** (embed Python in Rust): tight coupling to Python ABI, GIL complexity, breaks env isolation
- **gRPC**: heavier setup, more dependencies, overkill for local IPC
- **Unix domain sockets + custom protocol**: reinvents Arrow IPC poorly
