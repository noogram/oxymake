# OxyMake Positioning Doc — 2D Map (séquentiel-vs-DAG × glue-vs-orchestrator)

> **Status:** Idea — Step 1 of `idea-to-plan`
> **Date:** 2026-05-27
> **Author:** Polecat chrome (autonomous)
> **Source:** Cross-galaxies analysis the internal itwinai analysis note §4.1 lever 1
> **Cited reference:** Bunino et al. (2026), *itwinai: A Python Toolkit for Scalable Scientific Machine Learning on HPC Systems*, JOSS, [doi:10.21105/joss.09409](https://doi.org/10.21105/joss.09409). Zotero key `4J2T464X`. Wiki: `[[bunino-itwinai-2026]]`.

---

## 1. The idea in one sentence

> **Author a positioning document for OxyMake whose centerpiece is a 2D map plotting workflow tools on two orthogonal axes — *sequential vs DAG* and *glue vs orchestrator* — and that cites itwinai (Bunino et al. 2026, CERN × FZJ) as positive prior-art that validates the design pattern "stable configurable interface above N heterogeneous backends" while clarifying that OxyMake occupies a distinct quadrant (DAG × Orchestrator, Rust-first, agent-friendly).**

The doc is not a competitive teardown. It is a *map of the terrain* on which OxyMake stands — naming neighbors before naming differences, and citing the most credible peer (13 authors, CERN+FZJ, 6 years of ecosystem, EuroHPC grant 101058386) as a friendly validator of the *meta-pattern* OxyMake instantiates differently.

---

## 2. Context — why now

### 2.1 The trigger

A fresh cross-galaxies analysis of the JOSS 2026 itwinai paper (the internal itwinai analysis note) yielded a load-bearing observation (§4.1, "lever 1"):

> *"OxyMake ≠ remplaçant d'itwinai sur le slot ML-training-orchestration. itwinai a 6 ans, 8 use-cases scientifiques, le grant interTwin (101058386), JUWELS Booster. OxyMake ne doit pas attaquer ce périmètre. OxyMake = surcouche DAG amont qui peut appeler itwinai (ou Ray Tune nu, ou un script Python) comme step d'un DAG plus large."*

The analysis concluded with three concrete proposed molecules; this is the first (and most foundational): a **positioning doc** that does the work of *clarifying the slot* before any tutorial, benchmark, or feature pitch can land cleanly.

### 2.2 The gap

The existing comparative analysis (`docs/design/comparative-analysis-workflow-tools.md`, 573 lines) is **lesson-extraction-shaped** — it surveys 11 tools (Snakemake, Nextflow, Bazel, Ninja, DVC, Luigi, Airflow, Prefect, Pants, Buck2, GNU Make) to extract design lessons for OxyMake itself. It is *inward-facing*: what should OxyMake build given what others learned.

What is **missing** is the *outward-facing* sibling: a **positioning** doc that helps a reader (open-source visitor, prospective contributor, scientific collaborator, funder) locate OxyMake in one glance — *where it sits, what it is not, and what neighbors it can call*. itwinai's JOSS paper is conspicuously absent from the lesson-extraction doc (itwinai sits in a different family — ML-on-HPC glue, not generic workflow orchestration) yet it is exactly the *kind* of project that the positioning doc must place to make the slot legible.

### 2.3 The window

itwinai is a **fresh** publication (JOSS 2026), backed by CERN, FZJ, and EuroHPC. Citing it now (a) anchors OxyMake in the current state of the art, (b) demonstrates that OxyMake is read in the scientific-computing ecosystem, and (c) creates a tractable, credible bridge for future *"calling itwinai from an OxyMake DAG"* tutorials (the second proposed molecule).

---

## 3. Motivation — what changes if this doc exists

A positioning doc that lands the 2D map answers, in one page, four questions every visitor asks within 90 seconds of reading the README:

1. **"Where does this fit?"** — the 2D map locates OxyMake among named neighbors.
2. **"Is this another Bazel / Snakemake / Airflow?"** — quadrant placement answers no, here is why, here is what they have that this does not aim to have.
3. **"Can I use this *with* my existing stack?"** — itwinai cited as a *callable peer*, not a competitor, signals composability.
4. **"Why Rust?"** — the *agent-friendly Rust orchestrator* axis label makes the language choice load-bearing, not aesthetic.

Concrete downstream effects:

- **Adoption** — visitors who would have bounced ("yet another make replacement?") stay long enough to read the differentials.
- **Contributor recruitment** — clear non-goals (no ML training backend, no HPC glue, no notebook IDE) prevent the kind of contributor-time-burn that comes from PRs that pull the project off-slot.
- **Funding posture** — readable map of *"which adjacent projects exist and how OxyMake interfaces with them"* is exactly what grant reviewers and scientific-collaborator emails look for.
- **Agent-readability** — an autonomous worker (cosmon polecat, future user-agent) reading a 2D quadrant grid + slot statements parses positioning in milliseconds, vs. paragraphs of prose.

---

## 4. Initial scope

### 4.1 In scope

| Element | Form | Notes |
|---|---|---|
| **2D map** (centerpiece) | ASCII / Mermaid quadrant chart | x-axis: *sequential ←→ DAG* ; y-axis: *glue ←→ orchestrator* |
| **Tool placements** | ~8-12 named projects | itwinai, Snakemake, Nextflow, Pachyderm, Bazel, Airflow, Prefect, Luigi, DVC, Make, Ninja, Ray |
| **OxyMake quadrant statement** | One short paragraph | "DAG × Orchestrator, Rust-first, agent-friendly, content-addressable, daemon-free" |
| **Prior-art citation block** | Inline references with one-line glosses | itwinai cited as *validating the meta-pattern* (stable config interface above N backends), not as competitor |
| **"What OxyMake is not"** section | 4-6 bullet points | No ML training backend ; No HPC SLURM scheduler ; No notebook UI ; No experiment tracker (DVC-style) ; etc. |
| **"Where OxyMake calls neighbors"** section | Mini-table | OxyMake as DAG orchestrator can call itwinai pipelines as rules, Ray Tune as rules, raw scripts as rules. |
| **Differential invariants** | Mini-table | DAG > sequential ; Rust > Python for orchestration core ; agent-friendly JSON output ; content-addressable cache ; daemon-free |
| **Cross-references** | Footer | Link back to `comparative-analysis-workflow-tools.md` (the inward-facing sibling), to relevant ADRs (002 TOML-not-DSL, 005 daemon-free, 008 executor-bridge), to the source analysis. |

### 4.2 Out of scope (deliberately)

- **No competitive teardown.** The doc never says "X is bad". Neighbors are credited; positions are stated.
- **No benchmark numbers in this doc.** Performance claims belong in `benchmark/` artefacts ; a positioning doc that anchors itself to benchmark figures ages badly.
- **No roadmap.** That belongs in a separate doc (`docs/design/roadmap-*`).
- **No itwinai tutorial.** That is the *second* proposed molecule. This doc only opens the door.

### 4.3 Form factor

- **Length:** target 4–6 pages rendered (under 600 lines source). The doc must remain reviewable in one sitting.
- **Location:** `docs/positioning.md` at top level of `docs/`, *not* nested in `docs/design/` — positioning is reader-facing, not designer-facing.
- **Linked from:** `README.md` (a single sentence + link, no fork in the cover narrative), `docs/architecture/boundary.md`, `docs/design/comparative-analysis-workflow-tools.md` (cross-link the inward/outward pair).
- **Format:** Markdown, GitHub-renderable, Mermaid for the 2D quadrant chart (with classDef for visual grouping — *not* nested subgraphs, per the known Obsidian-Mermaid pitfalls).

---

## 5. The 2D map — preliminary placement (draft, refinable in Step 3)

```
                   ↑ orchestrator
                   │
   Airflow ▪       │       ▪ Bazel
   Prefect ▪       │       ▪ Buck2
   Luigi   ▪       │       ▪ Pants
                   │
                   │       ▪ Nextflow
                   │       ▪ Snakemake
                   │       ▪ Pachyderm
                   │       ▪ OxyMake ◀ (DAG × Orchestrator,
                   │                    Rust-first, agent-friendly)
─sequential────────┼──────────────────────────────── DAG →
                   │
                   │       ▪ Make (DAG, but glue-shaped)
                   │       ▪ Ninja
                   │
   itwinai ▪       │       ▪ DVC
   (Python         │       ▪ Ray Tune
    pipeline)      │
                   │
                   ↓ glue
```

(Final placement to be discussed in Step 2 — feasibility review may want to redraw axes or split into two charts.)

---

## 6. Open questions (for Step 2 — feasibility)

1. **Axis labels** — is *glue ↔ orchestrator* the right second axis, or should it be *embedded-DSL ↔ standalone-binary*, or *single-language ↔ polyglot*? The bunino-itwinai analysis privileges *glue/orchestrator*; defensible but worth challenging.
2. **Mermaid vs. ASCII vs. SVG** for the quadrant chart. Mermaid `quadrantChart` exists but is sparsely supported in GitHub render today (verify); ASCII guarantees rendering; SVG looks better but adds a build step.
3. **How many tools to place?** Too few = caricature; too many = noise. The lesson-doc covers 11; the positioning doc probably wants 8–10, with itwinai featured and one of {Bazel, Snakemake, Nextflow, Airflow} as the strongest per quadrant.
4. **Cross-link with `comparative-analysis-workflow-tools.md`** — leave the inward doc alone, or add a "see also (outward): positioning.md" header to make the pair discoverable?
5. **README integration** — one-line link, or a small embedded version of the 2D map in the README? (Risk: divergence between README map and positioning.md map.)
6. **Versioning** — does positioning.md need a date/version header? Positioning ages with the ecosystem; itwinai will release v2; OxyMake will publish v1.0. Suggest a `last-revised: YYYY-MM-DD` line + a 6-monthly review cadence noted in the doc footer.

---

## 7. Success criteria

A reader (assumed: technically literate but unfamiliar with the workflow-orchestration ecosystem) can, after 5 minutes with the positioning doc:

- Locate OxyMake among 3–4 named peers.
- State at least 2 things OxyMake does *not* aim to be.
- Name at least one adjacent project that OxyMake *can call from* its DAG.
- Articulate why Rust is part of the slot, not an aesthetic choice.

A funder / collaborator can, after the same 5 minutes:

- See OxyMake's relation to a EuroHPC-funded peer (itwinai) is *composable, not competitive*.
- Find the cross-reference path to the inward-facing comparative analysis if they want deeper substance.

---

## 8. Risks

- **Risk of misplacing a tool** — every workflow-system user has opinions on where their favorite tool sits. Mitigation: provenance per tool (one-line citation), and a footer inviting PRs to correct placements.
- **Risk of staleness** — the ecosystem shifts (Prefect v1→v2 broke users, itwinai will iterate, Buck2 may stabilize). Mitigation: explicit `last-revised` line + 6-month re-read cadence noted in the doc.
- **Risk of being read as marketing** — a positioning doc is by nature partial. Mitigation: cite primary sources for every claim (JOSS DOI for itwinai, Snakemake paper, etc.) and avoid superlatives.
- **Risk of overlap with `comparative-analysis-workflow-tools.md`** — the existing 573-line doc is exhaustive. Mitigation: positioning.md is *outward-facing and short* ; the lesson-extraction doc is *inward-facing and exhaustive*. They are a pair, not duplicates ; make this explicit in both files' headers.

---

## 9. Cross-references

- **Source analysis (load-bearing):** the internal itwinai analysis note §4.1 lever 1.
- **Cited reference:** `[[bunino-itwinai-2026]]` — JOSS 2026, [doi:10.21105/joss.09409](https://doi.org/10.21105/joss.09409).
- **Inward sibling (lesson extraction):** `docs/design/comparative-analysis-workflow-tools.md`.
- **Relevant ADRs:** ADR-002 (TOML-not-DSL), ADR-005 (daemon-free cooperative), ADR-008 (executor-bridge).
- **Boundary doc:** `docs/architecture/boundary.md`.
- **Itwinai repo (local clone, depth 50):** your local itwinai clone.

---

## 10. Next steps (within this molecule)

- **Step 2 — Evaluate feasibility:** review axis choice, tool inventory, Mermaid-vs-ASCII chart form ; check for dead links and missing ADRs ; estimate writing effort (target: 1–2 days for a v1 draft) ; surface any cosmon-ward feedback if a primitive is missing.
- **Step 3 — Create actionable plan:** convert this idea into either (a) a draft of `docs/positioning.md` itself (the deliverable) or (b) a follow-up task to write it (with checklist and acceptance criteria), depending on Step 2's effort estimate.

— *End of Step 1 capture. Step 2 (feasibility) follows immediately.*
