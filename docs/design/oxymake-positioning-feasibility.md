# Feasibility — OxyMake Positioning Doc (Step 2 of `idea-to-plan`)

> **Status:** Feasibility assessment — Step 2 of `idea-to-plan`
> **Date:** 2026-05-27
> **Input:** [Step 1 capture](./oxymake-positioning-2d-map.md)
> **Output target:** Step 3 — actionable plan / draft

---

## 1. Scope recap (from Step 1)

Produce a positioning doc (`docs/positioning.md`, ~4–6 rendered pages) whose centerpiece is a 2D map (sequential ↔ DAG × glue ↔ orchestrator) that places OxyMake in the *DAG × Orchestrator, Rust-first, agent-friendly* quadrant, cites itwinai (Bunino et al. 2026, JOSS) as positive prior-art validating the *configurable interface above N backends* meta-pattern, and complements (does not duplicate) the existing inward-facing `docs/design/comparative-analysis-workflow-tools.md`.

---

## 2. Feasibility — five dimensions

### 2.1 Technical feasibility — **HIGH** ✅

The doc is pure Markdown. No code change, no build step beyond Markdown rendering.

**Constraints:**

- **2D quadrant chart** — three viable forms:
  1. **ASCII art** (as drafted in Step 1 §5) — guaranteed render anywhere, no build step, easy to PR-amend. **Recommended**.
  2. **Mermaid `quadrantChart`** — natively supported on GitHub since 2023, but spacing of labels can be finicky and dense placements collide.
  3. **SVG** — best aesthetic; requires checked-in binary asset + a regenerator script. Adds ~15 lines of infra for ~5% extra polish.

  **Verdict:** ASCII for the primary chart, Mermaid `quadrantChart` *optionally added below* for tools that render it richly. Both are sourceable in the same .md file.

- **Cross-references** — all paths cited in Step 1 exist and are reachable:
  - the internal itwinai analysis note ✅ (verified, 173 lines)
  - the internal itwinai summary note ✅ (verified, 131 lines)
  - `docs/design/comparative-analysis-workflow-tools.md` ✅ (573 lines)
  - `docs/architecture/boundary.md` ✅
  - ADR-002, ADR-005, ADR-008 ✅ (verified in `docs/adr/`)

**Risk:** zero technical blockers.

### 2.2 Effort estimate — **LOW-TO-MEDIUM** (1–2 days of writing)

| Sub-task | Estimated effort |
|---|---|
| Draft prose (sections: intro, 2D map narrative, OxyMake quadrant, "is not", "calls neighbors", differential invariants, footer) | 4–6 h |
| Inventory tool placements (~10 tools, one-line provenance per tool) | 1–2 h |
| Polish ASCII chart, verify alignment, add optional Mermaid version | 1 h |
| Cross-link from README + comparative-analysis-workflow-tools.md (mutual visibility) | 30 min |
| Self-review pass (humanizer pass — remove "remarkable", "vibrant", em-dash binge, etc.) | 30 min |
| **Total** | **~7–10 h of focused writing** = 1–2 working days |

Effort is well-below the threshold that would justify a multi-molecule decomposition. **One molecule (this one) covers it**, with Step 3 either producing the draft directly or filing a `write-positioning-doc` task.

### 2.3 Alignment with project goals — **HIGH** ✅

The README (lines 7–22) already markets OxyMake along axes that the positioning doc would make legible:

- *"Next-generation workflow orchestration"* → orchestrator axis
- *"file-based rules, backward-chaining DAG, wildcards"* → DAG axis
- *"polyglot execution"* → "calls neighbors" section
- *"first-class support for both human and AI agent users"* → agent-friendly slot statement
- *"same workflow on laptop, SLURM cluster, Ray cluster, or Kubernetes"* → executor-bridge differential

A positioning doc consolidates these scattered marketing strokes into one referenceable map. No goal drift; pure goal-articulation.

ADR alignment:
- **ADR-002 (TOML-not-DSL)** — supports *"declarative configuration"* axis labelling.
- **ADR-005 (daemon-free)** — load-bearing differential vs. Airflow/Prefect/Ray-Tune-as-cluster.
- **ADR-008 (executor-bridge)** — supports *"calls neighbors"* section (itwinai, Ray, raw scripts as rules).

### 2.4 Strategic alignment — **HIGH** ✅

Three strategic signals concur:

1. **The bunino-itwinai analysis** (the internal itwinai analysis note §6, three proposed molecules) explicitly recommends this doc as the **foundational** of three follow-ups. The two others (`calling-itwinai-from-an-oxymake-DAG` tutorial, `parameter-sweep-as-DAG` design note) **depend** on this doc landing first — without the positioning, the tutorial reads as competing rather than composing.
2. **Funding posture** — funders and scientific collaborators screen workflow tools by *slot legibility* before reading the README. A positioning doc shortens that screening from ~20 minutes (read README + try install) to ~5 minutes (read positioning + decide).
3. **Contributor recruitment** — explicit non-goals prevent contributor PRs that pull the project off-slot (e.g. "let's add an embedded notebook UI"). The "what OxyMake is not" section is a low-cost guard.

### 2.5 Reversibility — **HIGH** ✅

A positioning doc is fully reversible. If a future axis re-think (e.g. *"glue/orchestrator"* turns out to be the wrong cut and *"file-graph/event-graph"* is sharper) lands, the doc gets re-drafted. No code, no public API, no migration. Cheap to revise, cheap to delete.

---

## 3. Resolved Step-1 open questions

Carrying through each of the six open questions from Step 1 §6 with a current best-answer (refinable in Step 3 if needed):

| # | Question | Resolved answer |
|---|---|---|
| 1 | Axis labels — keep *glue ↔ orchestrator*? | **Yes, keep.** The bunino-itwinai analysis privileges this cut (§1.1, §4.1) and it maps cleanly to "I assemble things end-to-end" vs. "I glue two backends together". Alternative axes (*single-language ↔ polyglot*) would collapse interesting distinctions — Snakemake and OxyMake would coincide. Reject. |
| 2 | Mermaid vs ASCII vs SVG? | **ASCII primary, optional Mermaid quadrantChart secondary.** Per §2.1 above. SVG adds infra weight not yet justified. |
| 3 | How many tools? | **8–10 placed, with provenance.** Featured: itwinai (cited reference), Snakemake (closest-philosophical-cousin), Airflow (canonical orchestrator-not-DAG-shaped), Make (canonical DAG-but-glue). Additional placements (Nextflow, Bazel, Prefect, DVC, Ray Tune, Pachyderm) chosen to span the chart, not to be exhaustive. |
| 4 | Cross-link with comparative-analysis-workflow-tools.md? | **Yes, mutual visibility.** Add a header in each: positioning.md says "See also (inward, for design lessons): comparative-analysis-workflow-tools.md" ; the lesson doc says "See also (outward, for slot positioning): positioning.md". Cheap, prevents confusion. |
| 5 | README integration? | **One-sentence link to positioning.md from README's intro paragraph.** Do NOT embed a mini-version of the 2D map in the README — risk of divergence + dilution of the README's quick-start posture. |
| 6 | Versioning / staleness? | **Add `last-revised: YYYY-MM-DD` line + footer sentence: "Positioning ages with the ecosystem. Re-read every 6 months ; PRs to correct placements welcome."** Cheap, makes the contract explicit. |

---

## 4. Risks (revisited from Step 1 §8)

| Risk | Mitigation status |
|---|---|
| Misplacing a tool | Each placement gets a one-line citation (primary source: paper or repo README). PR-able. |
| Staleness | Explicit `last-revised` + 6-month cadence sentence. |
| Read as marketing | Cite primary sources for every claim ; no superlatives ; quote itwinai's JOSS paper neutrally. Run the humanizer pass before commit. |
| Overlap with comparative-analysis | Cross-link both ways with explicit "inward / outward" labels. |
| **NEW** — Mermaid `quadrantChart` not rendering well in GitHub on Mar/Apr 2026 | Resolved by choosing ASCII as primary. Mermaid version is optional decoration. |
| **NEW** — itwinai authors might prefer a different framing | Low risk; we are citing them positively as prior art validating a meta-pattern. If they object, easy revision. Out-bound (do not solicit pre-publication review — this is OxyMake's positioning, not a co-authored doc). |

No new blockers surfaced.

---

## 5. Cosmon-ward feedback

> *"When an application-site galaxy discovers a cosmon-level pathology, surface it back to cosmon as a typed molecule. Do not silently patch the application."* (CLAUDE.md, Cosmon-ward feedback flow.)

This molecule discovered **no cosmon-level pathology**. The `idea-to-plan` formula is well-matched to this kind of work (idea → feasibility → plan). The cosmon CLI worked correctly (the only friction was `--formula <relative-path>` needing an absolute path when running from a worktree subdirectory — minor, but worth noting in case the operator wants to file a polish task; not surfacing it as cosmon-ward today because it is a CLI ergonomic, not a broken invariant).

---

## 6. Decision — proceed to Step 3

**Verdict: PROCEED.**

All five feasibility dimensions are favorable. Effort is bounded (~1–2 days). The doc has clear acceptance criteria (Step 1 §7), an inventory of tools to place, an agreed primary chart form (ASCII), an agreed location (`docs/positioning.md`), and a clear bilateral cross-link plan with the inward-facing sibling.

Step 3 will produce the **actionable plan** — concretely, a checklist + writing task that delivers `docs/positioning.md` (the deliverable).

**One micro-deferral:** the actual *draft* of `docs/positioning.md` could be either (a) embedded in Step 3 as the deliverable artefact, or (b) deferred to a follow-up task. Recommendation in Step 3 below: **(b) follow-up task**, because Step 3 of an `idea-to-plan` formula is *"create actionable plan"* — not *"write the deliverable"*. Keeping the layers clean lets a future contributor (human or polecat) pick up the writing with full context, without confusing planning artefacts with the artefact itself.

---

## 7. Cross-references

- **Step 1 capture (input):** [docs/design/oxymake-positioning-2d-map.md](./oxymake-positioning-2d-map.md)
- **Source analysis:** the internal itwinai analysis note §4.1 lever 1
- **Cited reference:** `[[bunino-itwinai-2026]]` — JOSS 2026, [doi:10.21105/joss.09409](https://doi.org/10.21105/joss.09409)
- **Inward sibling:** `docs/design/comparative-analysis-workflow-tools.md` (573 lines, lesson extraction)
- **Relevant ADRs:** [ADR-002](../adr/002-toml-not-dsl.md), [ADR-005](../adr/005-daemon-free-cooperative.md), [ADR-008](../adr/008-executor-bridge.md)
- **Boundary:** [docs/architecture/boundary.md](../architecture/boundary.md)

— *End of Step 2. Step 3 (plan) follows immediately.*
