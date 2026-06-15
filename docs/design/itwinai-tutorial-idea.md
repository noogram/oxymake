# Idea — OxyMake × itwinai Tutorial

**Formula:** `idea-to-plan`
**Captured:** 2026-05-27
**Status:** Step 1/3 — capture

## One-sentence pitch

A 3-rule OxyMake example (`prepare → train → eval`) where the `train` rule
shells out to `itwinai exec-pipeline --config train.yaml`, demonstrating the
**orthogonality** between OxyMake (DAG orchestration, content-addressable
cache) and itwinai (sequential ML-training pipeline configured by YAML).

## Source

Idea: design an example tutorial that calls itwinai from an OxyMake DAG,
prompted by a 2026-05-27 reading of the itwinai paper (Bunino et al., 2026 —
see Context below).

## Context

itwinai (Bunino et al., 2026, CERN×FZJ — JOSS) consolidates 6 years of
AI-on-HPC scientific glue behind a single configurable YAML interface. Its
`Pipeline` primitive is **sequential** (`step.execute(*args)` chained); any
advanced workflow (fork-join, parameter sweep) has to be wired in raw Python.

OxyMake occupies the **adjacent and orthogonal slot**: DAG orchestration
amont, polyglot rules, content-addressable cache, daemon-free `ox run`.
The two tools do not compete — they compose. A DAG of OxyMake rules can
contain a node that invokes `itwinai exec-pipeline` as a black-box training
backend.

This tutorial materializes the orthogonality so that:

1. **OxyMake users** see how to plug a state-of-the-art ML-training pipeline
   into their DAG without reinventing training-loop machinery.
2. **itwinai users** discover OxyMake as the missing upstream layer when
   their workflow stops being sequential (parameter sweeps, fork-join over
   data splits, conditional re-runs).

## Motivation — three concrete leverages

### 1. Orthogonality (slot clarification)

Today, OxyMake's positioning (DAG over rules, polyglot, content-addressable)
is asserted but not demonstrated against a peer. A tutorial that
*explicitly* composes the two systems makes the slot boundary legible
without polemic. The tutorial is the proof; no marketing copy needed.

### 2. Content-addressable cache **above** itwinai execute

itwinai re-runs from scratch on every invocation; its reproducibility model
is *"re-run with the same config"*. OxyMake's cache hashes the inputs
(config file + dataset fingerprint + rule signature) and skips the rule
when nothing changed. The tutorial shows that wrapping `itwinai
exec-pipeline` in an OxyMake rule **adds skip-if-unchanged semantics
itwinai does not natively offer** — a strict capability gain for itwinai
users.

### 3. Granular reproducibility

In the canonical `prepare → train → eval` triplet:

- Changing the `eval` script must NOT re-trigger `train`.
- Changing a hyperparameter in `train.yaml` MUST re-trigger `train` AND
  `eval`, but NOT `prepare`.
- Changing the raw data MUST re-trigger all three.

These three reproducibility invariants are what OxyMake's content-addressable
cache buys for free, on top of itwinai's training pipeline. Demonstrating
them on a real itwinai pipeline turns a positioning claim into a
measurable artifact.

## Initial scope (subject to feasibility assessment in step 2)

**In scope:**

- A self-contained example directory under `examples/itwinai-tutorial/`.
- 3 OxyMake rules: `prepare`, `train`, `eval`.
- A minimal itwinai `train.yaml` (toy model — MNIST or a 2-layer MLP on
  synthetic data; the point is the orchestration, not the science).
- A `README.md` explaining the orthogonality, with three demonstration
  commands:
  1. `ox run all` — full pipeline, cold cache.
  2. `ox run eval` (after touching `eval.py`) — only `eval` re-runs.
  3. `ox run all` (after editing `train.yaml`) — `train` + `eval` re-run, `prepare` skipped.
- A short *Prior art / positioning* paragraph in the README citing itwinai
  with a 1-paragraph slot map (DAG-vs-sequential × glue-vs-orchestrator).

**Out of scope (deferred):**

- Distributed/HPC execution (itwinai's `--strategy=ddp` etc.) — local CPU only.
- `interlink-executor` integration — tagged in roadmap, not built.
- A second tutorial that swaps itwinai for Ray Tune nu (parallel idea —
  separate molecule).
- Any benchmark numbers vs. running itwinai standalone (separate molecule
  if pursued).

## Anti-scope

- This is **not** a wrapper, a plugin, or a library integration. No Rust
  code is added; no Cargo dependency is taken. The tutorial relies on
  OxyMake's existing shell-rule mechanism (`itwinai exec-pipeline` is
  invoked as a subprocess, like any other CLI).
- This is **not** a recommendation for itwinai over Ray Tune / DDP-nu /
  PyTorch Lightning. itwinai is chosen because (a) it occupies the slot
  most cleanly, (b) it has the CERN/FZJ caution, (c) its CLI contract is
  stable. The pattern transposes to any subprocess-CLI training tool.

## Open questions (to resolve in steps 2 & 3)

1. **itwinai install footprint** — does the env install fit in a reasonable
   tutorial setup (`pip install itwinai` + lightweight torch)? If the env
   is heavy (>1 GB or GPU-required), the tutorial loses its laptop-first
   audience. Step 2 must verify.
2. **Toy model choice** — MNIST is overused but universally understood.
   Synthetic-data MLP is faster but less recognizable. To decide in step 3.
3. **Cache key design** — does OxyMake's current cache key generation
   correctly fingerprint a YAML config file passed as an argument to a
   shell rule? Or does it only hash declared `inputs`? Step 2 must check
   the cache contract and confirm the tutorial's reproducibility claims
   are honestly achievable.
4. **Tutorial host** — does it live under `examples/`, under `docs/`, or
   as a standalone repo? Default: `examples/itwinai-tutorial/` with a
   pointer from `docs/positioning.md` (when that doc lands — separate
   molecule §6.1 of the source analysis).

## Next steps

- Step 2 (evaluate feasibility) — verify itwinai install footprint, cache
  fingerprinting contract, and toy-model choice.
- Step 3 (actionable plan) — produce a concrete task list (or an ADR if
  the cache contract turns out to need adjustment).
