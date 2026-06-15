# FAIR Alignment

A factual register of where OxyMake stands against the FAIR principles for
research software and reproducible computational workflows, and what we have
committed to ship next.

This page makes no claims the engine does not already back. The export work that
turns OxyMake's internal records into community-standard artefacts (RO-Crate,
W3C PROV) is named below as a **scoped v1.1 commitment**, not as a present
feature. The paper (`docs/paper/oxymake-paper.tex`) states the same scope; this
page is its repo-side companion for readers who evaluate against FAIR.

References: FAIR4RS (Chue Hong et al., *Sci Data* 2022); FAIR Computational
Workflows (Goble et al. 2020); Workflow RO-Crate (researchobject.org); W3C PROV;
CWL (commonwl.org).

## What already aligns

OxyMake's core was built around *"same inputs → same result, on any machine, at
any time"* — which is the **R** (Reproducible / Reusable) of FAIR. The alignment
below is a property of the engine as it ships, not of a future layer.

| OxyMake brick | FAIR principle served |
|---|---|
| BLAKE3 content-addressed fingerprint | stable artefact identity (**Findable**, versioned — not "Tuesday's file") |
| TOML spec readable without executing | transparency / inspectability (**Interoperable**) |
| `ox translate Snakefile` | interop with the existing community (**Interoperable**) |
| Typed `--json` event stream | raw material for a **provenance** trace |
| Determinism / re-execution | **Reusable** + bit-for-bit reproducibility |
| Clear MIT/Apache licence | **Reusable** (FAIR4RS licence clause) |

### Placement on the FAIR reproducibility ladder

The paper (Table `tab:fair-ladder`) frames reproducibility as a four-layer
ladder of *independent, composing* witnesses. OxyMake earns credit at **L2–L4**
and delegates **L1** to the substrate of the reader's choice (Guix, Nix, Docker)
— by design, and stated honestly rather than absorbed silently.

| Layer | Witness | OxyMake's standing |
|---|---|---|
| **L1 — Substrate** | same input hash ⇒ same binary | **Delegated** to Guix / Nix / Docker; OxyMake records the env hash and trusts the substrate |
| **L2 — Orchestration** | same plan hash ⇒ same DAG | **Native** — `ox.lock` |
| **L3 — Execution** | same DAG ⇒ same output hashes | **Native** — content-addressed cache |
| **L4 — Audit** | run trace decoupled from code | **Native record** (NDJSON + `state.db`); **RO-Crate** is the community packaging on top — see roadmap |

RO-Crate sits at L4 as a community *packaging* of the audit trail OxyMake
already records. The records exist today; emitting them in RO-Crate's
vocabulary is the v1.1 work below.

## The forward-compatibility verdict

Before deferring the export work to v1.1, we audited whether deferring it forces
a breaking change later — the one genuine one-way-door risk. The question was
narrow: can the surfaces frozen in v1 (the NDJSON event stream, the `state.db`
schema, and `ox.lock`) later **emit** valid W3C PROV and a Workflow RO-Crate
**without touching anything declared stable**?

**Verdict — `breaking-change-needed: NO`.** Every PROV / RO-Crate requirement
maps to a field that **already exists** in the durable `state.db` record
(`job_history` holds input/output hashes, wall-clock start/end, executor, host,
session, run linkage, and a free-form `artifact_provenance_json` bundle —
schema v8 added `reproducibility_class` and `artifact_provenance_json` for
exactly this). The two ingredients missing from the *live* event stream
(absolute wall-clock timestamps and output content hashes) can be added
**additively**: `STATUS.md` permits adding fields to stable surfaces and forbids
only removal/retype, and no frozen serialiser uses `deny_unknown_fields`. The
audit converted its load-bearing claim — *"additive = non-breaking"* — into
executable regression guards in `ox-lock` and `ox-report-json`, so CI goes red
if anyone later freezes those serialisers against additions.

In short: the v1 foundation is graftable. The v1.1 export work bolts on without
jackhammering the slab. (Full audit: molecule `task-20260614-a814`.)

## Roadmap v1.1 — scoped commitments

These are **commitments**, scoped and tracked, not aspirations. Each is filed as
a day-1 issue (`docs/issues-day1.md`) ready to become a GitHub issue at the
public flip. They are post-release because the operator's decision is to ship v1
on schedule and make export the **public activity of the v1.1 cycle** — none of
them blocks the release.

1. **RO-Crate export — `ox export --ro-crate`.** Emit a Workflow Run RO-Crate
   (workflow definition + declared inputs/outputs + provenance bundle) from
   `state.db` + `ox.lock`. The language the FAIR-computational-workflows
   community reads. Highest leverage; reads only existing surfaces.
2. **W3C PROV serialisation on `--json`.** Map the event/`state.db` record to
   PROV entities / activities / agents (`used`, `wasGeneratedBy`,
   `wasAssociatedWith`). Natural graft on the existing `--json` sink.
3. **CWL import/export interop.** We read Snakemake in today; CWL symmetry is
   the heavier interop step — scoped for the v1.1 cycle, after (1) and (2).
4. **Persistent identifiers (DOI/PID) — optional.** The *published-data*
   facet of **Findable**. The most optional of the four; arbitrated against
   release ambition rather than committed unconditionally.

See [`docs/issues-day1.md`](issues-day1.md) (FAIR roadmap entries) for the
issue-ready bodies.

## External validation

This alignment is the right artefact for an external FAIR / bioimage reviewer to
validate — standards expertise, open-source, and the bioimage-workflows world.
Such a review is a **validator of the doc + roadmap post-NDA**, not a release
blocker: the engine's L2–L4 standing and the forward-compat verdict hold
independently of it.
