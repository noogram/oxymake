# Architecture Notes

Frontier notes about OxyMake's structure and the boundary of its formal model.
These complement — they do not duplicate — the
[Architecture Decision Records](../adr/README.md).

- **[Crate Graph — How OxyMake Fits Together](../book/src/architecture/crate-graph.md)**
  — the hexagonal, `ox-core`-centered crate map: which crate does what, the
  exact inter-crate edges (verified against `cargo tree`), and the load-bearing
  rule that `ox-core` takes no `ox-*` dependency. Lives in the book so its
  Mermaid diagram renders on the docs site; this is the canonical source.
- **[Boundary — Substrate Axioms](boundary.md)** — the seven axioms about OS,
  filesystem, and SQLite behaviour that OxyMake assumes rather than proves; the
  frontier of the TLA+ model.
