# Actionable Plan — OxyMake Positioning Doc (Step 3 of `idea-to-plan`)

> **Status:** Plan — Step 3 of `idea-to-plan`
> **Date:** 2026-05-27
> **Input:** [Step 1 capture](./oxymake-positioning-2d-map.md), [Step 2 feasibility](./oxymake-positioning-feasibility.md)
> **Output:** This plan + a queued follow-up molecule to write `docs/positioning.md`.

---

## 1. What this plan delivers

A **bounded, agent-executable writing task** that, when picked up, produces `docs/positioning.md` — the *outward-facing* positioning document.

This plan is **not** the deliverable. It is the bridge between *"the idea is captured and judged feasible"* (Steps 1–2) and *"a worker can pick it up and ship the doc"*. Per Step 2 §6, separating planning artefacts from the artefact itself keeps the layers clean.

---

## 2. The follow-up molecule (to be nucleated)

### 2.1 Identity

- **Suggested formula:** `task` (single-step, checklist-driven). Alternative: `feature` if a richer 3-step flow (design → implement → verify) feels warranted, but the work is doc-only — `task` is the right shape.
- **Suggested title:** *"Write OxyMake positioning doc — 2D map + itwinai prior-art citation"*
- **Suggested temperature:** `temp:warm` (clear, scoped, not blocking anything urgent — but ready to be promoted to `temp:hot` whenever the operator wants a polecat dispatched).
- **Dependencies:** none. Inputs are fully captured by Steps 1–2.

### 2.2 Nucleation command (recommended)

```bash
cs nucleate task \
  --kind task \
  --temperature warm \
  --var title="Write OxyMake positioning doc — 2D map + itwinai prior-art citation" \
  --var context="Source analysis: the internal itwinai analysis note §4.1 lever 1. Step 1 capture: docs/design/oxymake-positioning-2d-map.md. Step 2 feasibility: docs/design/oxymake-positioning-feasibility.md." \
  --var deliverable="docs/positioning.md"
```

(Exact flag names may vary by formula schema — operator runs `cs help nucleate` if needed.)

---

## 3. Acceptance criteria (`docs/positioning.md`)

The follow-up molecule is **done** when `docs/positioning.md` exists and meets all of:

- [ ] **A.** File at top-level of `docs/` (not in `docs/design/` — positioning is reader-facing).
- [ ] **B.** Contains a `last-revised: YYYY-MM-DD` frontmatter or header line.
- [ ] **C.** Contains the **2D map** as ASCII art (primary), positioning OxyMake in the *DAG × Orchestrator* quadrant. Optional Mermaid `quadrantChart` may appear below as decoration.
- [ ] **D.** Places **8–10 named tools** with one-line provenance each (primary source: paper DOI or repo URL). At minimum: itwinai, Snakemake, Airflow, Make. Recommended additional: Nextflow, Bazel, Prefect, DVC, Ray Tune, Pachyderm.
- [ ] **E.** Cites **itwinai (Bunino et al. 2026, JOSS, doi:10.21105/joss.09409)** as *positive prior-art validating the configurable-interface-above-N-backends pattern*. Citation must be neutral (no marketing language about itwinai).
- [ ] **F.** States OxyMake's quadrant in one short paragraph: *"DAG × Orchestrator, Rust-first, agent-friendly, content-addressable, daemon-free."*
- [ ] **G.** Includes a **"What OxyMake is not"** section (4–6 bullets).
- [ ] **H.** Includes a **"Where OxyMake calls neighbors"** mini-table (itwinai, Ray Tune, raw scripts as rule subprocesses).
- [ ] **I.** Includes a **differential invariants** mini-table (DAG > sequential, Rust > Python for orchestration core, agent-friendly JSON output, content-addressable cache, daemon-free).
- [ ] **J.** Cross-links to `docs/design/comparative-analysis-workflow-tools.md` (inward sibling) and to relevant ADRs (002, 005, 008).
- [ ] **K.** `docs/design/comparative-analysis-workflow-tools.md` gets a one-line "See also (outward, for slot positioning): docs/positioning.md" header (the bilateral cross-link).
- [ ] **L.** `README.md` gains a one-sentence link to `docs/positioning.md` in the intro paragraph. **No** embedded mini-map in the README (prevents divergence).
- [ ] **M.** Footer sentence: *"Positioning ages with the ecosystem. Re-read every 6 months ; PRs to correct placements welcome."*
- [ ] **N.** Doc is reviewable in one sitting (target: under 600 source lines, ~4–6 rendered pages).
- [ ] **O.** Humanizer pass applied before commit (no "remarkable", "vibrant", "delve", em-dash binge, rule-of-three pile-up, etc.).
- [ ] **P.** Quality gates pass: `cargo check --workspace` ✅, `cargo test --workspace` ✅, `cargo clippy --workspace -- -D warnings` ✅, `cargo fmt --all -- --check` ✅. (Doc-only change — gates should be trivially green; running them is the project's contract.)

---

## 4. Writer's checklist (suggested order of operations)

Pre-staged so a polecat picking up the molecule can move directly:

1. **Read** the two predecessors: `docs/design/oxymake-positioning-2d-map.md`, `docs/design/oxymake-positioning-feasibility.md`.
2. **Inventory** the 8–10 tools. Open the bunino-itwinai-2026-analysis.md §1 in a buffer.
3. **Draft the 2D map first** (the centerpiece). Start from the Step-1 §5 ASCII sketch ; refine spacing.
4. **Write the quadrant statement** for OxyMake (one paragraph). Anchor it in README.md §"Features".
5. **Write tool placements** (one paragraph or one row per tool). Cite the primary source inline.
6. **Write the citation block for itwinai**. Use Zotero key `4J2T464X`, DOI `10.21105/joss.09409`, JOSS 2026, CERN × FZJ. Quote no more than 1–2 short phrases.
7. **Write "What OxyMake is not"** (4–6 bullets, terse).
8. **Write "Where OxyMake calls neighbors"** (mini-table, 3–4 rows).
9. **Write differential invariants table** (5 rows: DAG vs sequential, Rust orchestration, agent-friendly JSON, content-addressable cache, daemon-free).
10. **Cross-link** bilaterally with `comparative-analysis-workflow-tools.md` and ADRs.
11. **Add README.md sentence + link.**
12. **Footer:** last-revised line + 6-month cadence sentence.
13. **Humanizer pass.**
14. **Quality gates** (`just test` or the four cargo commands).
15. **Commit + `gt done`.**

---

## 5. Definition of "in good taste"

Per OxyMake's stated discipline ("Rust où il pèse — pas où il étouffe"; agent-readable; content-addressable; daemon-free), the positioning doc should:

- **Read in 5 minutes**, not 30.
- **Cite, don't trash** — neighbors are credited, not dismissed.
- **Quote primary sources**, not blog posts.
- **State non-goals explicitly** — the "is not" section is load-bearing.
- **Avoid superlatives** — *"fastest", "best", "most"* are forbidden. *"X-microseconds-to-resolve-Y-jobs"* is allowed if benchmark-backed.
- **Date itself** — `last-revised` line ; 6-month re-read sentence ; PR-welcome stance.

---

## 6. Out of scope (deliberately deferred)

These ideas are noted here so they do **not** creep into the positioning doc:

| Item | Where it belongs |
|---|---|
| Tutorial *"Calling itwinai from an OxyMake DAG"* | Separate molecule (bunino-itwinai-2026-analysis.md §6 item 2). Pre-requisite: positioning doc lands first. |
| Domain-specific application design note (an internal verticale) | Separate molecule (bunino-itwinai-2026-analysis.md §6 item 3). `temp:warm` until that verticale activates. |
| Roadmap / milestones | `docs/design/roadmap-*` (already partially exists). |
| Benchmark figures vs. competitors | `benchmark/` artefacts. Positioning doc never embeds benchmark numbers — too prone to ageing. |
| `interlink-executor` integration | Roadmap item (bunino-itwinai-2026-analysis.md §1.4) — long-term. |

---

## 7. Risks (revisited)

No new risks since Step 2 §4. The five risks named there (misplaced tool, staleness, marketing-read, sibling-overlap, Mermaid rendering, itwinai-author-objection) remain mitigated by the acceptance criteria above.

---

## 8. Cosmon-ward note

The minor ergonomic surfaced in Step 2 §5 (`cs evolve --formula <relative-path>` requires absolute path from worktree subdirs) is **not** filed cosmon-ward today: it is a CLI ergonomic, not a broken invariant. If the operator wants to file it, the suggested form is a `temp:warm` cosmon-side `idea` molecule titled *"cs evolve --formula: resolve relative paths against galaxy root, not CWD"*.

---

## 9. Summary — what happens next

1. **This molecule completes** with `cs complete` after this commit lands.
2. **A follow-up `task` molecule is nucleated** (operator gesture, command suggested in §2.2 above) with title *"Write OxyMake positioning doc — 2D map + itwinai prior-art citation"*.
3. **A polecat dispatched on that follow-up** finds Steps 1–2–3 here, follows the writer's checklist (§4), meets acceptance criteria (§3), and commits `docs/positioning.md`.
4. **Two further follow-up molecules** (itwinai tutorial, parameter-sweep-as-DAG design note) become unblocked once positioning lands.

---

## 10. Cross-references

- **Step 1 capture:** [docs/design/oxymake-positioning-2d-map.md](./oxymake-positioning-2d-map.md)
- **Step 2 feasibility:** [docs/design/oxymake-positioning-feasibility.md](./oxymake-positioning-feasibility.md)
- **Source analysis:** the internal itwinai analysis note §4.1 lever 1, §6 (three proposed molecules)
- **Cited reference:** `[[bunino-itwinai-2026]]` — JOSS 2026, [doi:10.21105/joss.09409](https://doi.org/10.21105/joss.09409), Zotero key `4J2T464X`
- **Inward sibling:** `docs/design/comparative-analysis-workflow-tools.md` (573 lines, lesson extraction — 2026-03-31)
- **Relevant ADRs:** [ADR-002 TOML-not-DSL](../adr/002-toml-not-dsl.md), [ADR-005 daemon-free cooperative](../adr/005-daemon-free-cooperative.md), [ADR-008 executor-bridge](../adr/008-executor-bridge.md)
- **Boundary:** [docs/architecture/boundary.md](../architecture/boundary.md)
- **README:** [README.md](../../README.md)

— *End of Step 3. This planning step is complete after this commit.*
