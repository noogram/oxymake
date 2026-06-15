# Plan — OxyMake × itwinai Tutorial

**Formula:** `idea-to-plan`
**Step:** 3/3 — actionable plan
**Captured:** 2026-05-27
**Companions:** [idea](itwinai-tutorial-idea.md) · [feasibility](itwinai-tutorial-feasibility.md)

## Decision: tasks, not ADR

Per the feasibility assessment, this tutorial introduces no new
architectural decision. It is a composition example using public CLIs of
both OxyMake and itwinai. No ADR is required. The deliverable is a task
list to be nucleated as individual cosmon molecules (or grouped as a
single task molecule with sub-steps, depending on operator preference).

## Target deliverable

A self-contained, runnable tutorial at:

```
examples/itwinai-tutorial/
├── README.md                  # Walkthrough + positioning + slot map
├── Oxymakefile.toml           # 3 rules: prepare, train, eval
├── requirements.txt           # Pinned itwinai + torch versions
├── config/
│   └── train.yaml             # itwinai pipeline config (Hydra-compatible)
├── src/
│   ├── prepare.py             # Download/prepare MNIST → npz
│   ├── pipeline.py            # itwinai Pipeline components (MLP trainer)
│   └── eval.py                # Load checkpoint, compute test accuracy
├── scripts/
│   ├── demo-cold.sh           # Demo 1: full pipeline, cold cache
│   ├── demo-edit-eval.sh      # Demo 2: edit eval.py → only eval re-runs
│   └── demo-edit-config.sh    # Demo 3: edit train.yaml → train + eval re-run
└── tests/
    └── smoke.sh               # CI-callable smoke test (cold run, exit 0)
```

## Task breakdown (atomic, ordered)

Each task is a candidate cosmon molecule (`cs nucleate task --var
topic="..."`) or a self-contained PR. Total estimated effort:
**3–5 working days**.

### T1 — Scaffolding + env setup (0.5 day)

- Create directory `examples/itwinai-tutorial/`.
- Write `requirements.txt` pinning `itwinai==0.4.2` and `torch==2.6.*`.
- Write a minimal `README.md` skeleton (title, one-paragraph intro,
  TBD sections).
- Manual venv install on the dev machine (`python -m venv .venv && pip
  install -r requirements.txt`); verify `itwinai exec-pipeline --help`
  runs and returns 0.
- **Done when:** `itwinai --help` works from the example dir's venv;
  scaffold committed.

### T2 — MNIST prepare step (0.5 day)

- Write `src/prepare.py` — downloads MNIST (via `torchvision.datasets`)
  to a local `data/` cache, normalizes, splits train/test, and writes
  `data/mnist.npz` (numpy arrays for X_train, y_train, X_test, y_test).
- Pure-Python script, callable as `python src/prepare.py --output
  data/mnist.npz`. No itwinai dependency yet (this is the OxyMake `prepare`
  rule's payload).
- **Done when:** `python src/prepare.py --output data/mnist.npz` produces
  a valid `.npz` file in <30 s on a stock laptop.

### T3 — itwinai training pipeline (1 day)

- Write `src/pipeline.py` containing two itwinai `BaseComponent`
  subclasses:
  - `NpzDataLoader` — reads `data/mnist.npz`, yields tensors.
  - `MlpTrainer` — defines a 2-layer MLP (784 → 64 → 10), trains 1 epoch
    on CPU, writes `checkpoints/model.pth`.
- Write `config/train.yaml` — Hydra config that instantiates the
  `itwinai.pipeline.Pipeline` with the two components.
- Verify `itwinai exec-pipeline --config-name=train -cp config` runs end
  to end on the venv. Adjust component signatures and Hydra config until
  green.
- **Done when:** running `itwinai exec-pipeline` directly (without
  OxyMake) produces `checkpoints/model.pth` and prints a training-loss
  trajectory.

### T4 — Eval step (0.5 day)

- Write `src/eval.py` — loads `checkpoints/model.pth` and `data/mnist.npz`,
  computes test accuracy, writes `metrics/test_accuracy.txt`.
- Pure-Python script, callable as `python src/eval.py --checkpoint
  checkpoints/model.pth --data data/mnist.npz --output
  metrics/test_accuracy.txt`. No itwinai dependency in this step
  (deliberate — it shows that OxyMake rules are polyglot and not all of
  them need itwinai).
- **Done when:** running `eval.py` against a fresh checkpoint produces
  an accuracy in the expected range (>85 % after 1 MNIST epoch).

### T5 — `Oxymakefile.toml` orchestration (0.5 day)

Write the 3-rule manifest:

```toml
ox_version = "0.1"

[rule.all]
input = ["metrics/test_accuracy.txt"]

[rule.prepare]
input = ["src/prepare.py"]
output = ["data/mnist.npz"]
shell = """
mkdir -p data
python src/prepare.py --output {output}
"""

[rule.train]
input = [
  "data/mnist.npz",
  "src/pipeline.py",
  "config/train.yaml",
]
output = ["checkpoints/model.pth"]
shell = """
mkdir -p checkpoints
itwinai exec-pipeline --config-name=train -cp config
"""

[rule.eval]
input = [
  "checkpoints/model.pth",
  "data/mnist.npz",
  "src/eval.py",
]
output = ["metrics/test_accuracy.txt"]
shell = """
mkdir -p metrics
python src/eval.py \\
  --checkpoint {input[0]} \\
  --data {input[1]} \\
  --output {output}
"""
```

- Verify `ox run all` cold-builds the entire DAG; verify a second `ox
  run all` is a full cache hit (no rule executes).
- **Done when:** cold run completes; warm run produces 3 cache hits.

### T6 — Three demo scripts (0.5 day)

Write the three demo scripts under `scripts/`:

- `demo-cold.sh` — `ox clean && ox run all`. Expected: 3 rules execute.
- `demo-edit-eval.sh` — `touch src/eval.py && ox run all`. Expected:
  only `eval` re-executes; `prepare` and `train` are cache hits.
- `demo-edit-config.sh` — appends a no-op space to `config/train.yaml`,
  then `ox run all`. Expected: `train` and `eval` re-execute; `prepare`
  is a cache hit.

Each script ends with an `echo` line summarizing the expected outcome
(so the user can sanity-check against `ox run` output).

- **Done when:** running all three scripts in sequence on a fresh venv
  produces the three expected re-run patterns.

### T7 — README walkthrough + positioning (1 day)

Write the `README.md` with these sections:

1. **What this is** — one paragraph: an OxyMake DAG composing an itwinai
   training pipeline.
2. **Why** — the three leverages from the idea doc: orthogonality,
   content-addressable cache above itwinai execute, granular
   reproducibility.
3. **Slot map** — a small ASCII or Mermaid diagram showing
   (DAG-vs-sequential × glue-vs-orchestrator) with OxyMake, itwinai,
   Snakemake, and Nextflow positioned. Validates the positioning point.
4. **Prerequisites** — Python ≥3.10, `python -m venv && pip install -r
   requirements.txt`, tested platforms.
5. **Quick start** — `ox run all` and what to look at.
6. **The three demos** — pointer to `scripts/demo-*.sh` with expected
   behavior.
7. **The cache contract footnote** — *"any Python script referenced from
   `train.yaml` via `_target_` must be declared as an OxyMake input,
   otherwise edits will be silently cached."* (load-bearing per
   feasibility Q3.)
8. **Where to go next** — pointer to `OXYMAKE-THESIS.md` for the
   positioning argument; pointer to itwinai docs for users new to it.

- **Done when:** README reads cleanly end-to-end and the demos work
  exactly as documented.

### T8 — Smoke test (optional, 0.5–1 day)

- Write `tests/smoke.sh` — a CI-callable script that runs `prepare +
  train + eval` cold and exits 0 iff `metrics/test_accuracy.txt` exists
  and contains an accuracy >0.5 (defensive low threshold to avoid flaky
  CI).
- Optionally wire it into the workspace `cargo test` via a `#[ignore]`
  Rust integration test that shells out to the script and runs only
  when `OX_RUN_ITWINAI_TUTORIAL=1` is set (to avoid forcing all
  contributors to install itwinai).
- **Done when:** `OX_RUN_ITWINAI_TUTORIAL=1 cargo test -p ox-cli
  itwinai_tutorial -- --ignored` passes on the dev machine.

Marked optional because it requires a workspace decision about whether
itwinai becomes a tested integration (extra CI cost) or stays a
documented-but-untested example. Default: SKIP for v1; revisit if the
example bit-rots.

## Sequencing

```
T1 (scaffold) ─→ T2 (prepare) ─→ T3 (itwinai pipeline) ─→ T4 (eval)
                                                            │
                                                            ▼
                                            T5 (Oxymakefile) ─→ T6 (demos) ─→ T7 (README)
                                                                                  │
                                                                                  ▼
                                                                              T8 (optional smoke)
```

T2, T3, T4 can proceed serially (each builds on the previous). T5 needs
all three. T6 needs T5. T7 needs T6 to be runnable so the walkthrough
is verifiable.

## Cross-references

- **Source analysis:** an internal cross-project analysis note
  §4.1 lever 2 — the seed.
- **OxyMake cache contract:** `crates/ox-cache/src/key.rs`
  (`compute_cache_key`) — load-bearing for the reproducibility demos.
- **Companion molecules to nucleate later** (from analysis §6):
  - `idea-temp:warm` — OxyMake positioning doc / prior-art map vs
    itwinai+Snakemake+Nextflow+Pachyderm (§6.1). The slot map in T7
    is a preview; the standalone positioning doc is a separate
    deliverable.
  - `idea-temp:warm` — domain-specific application design note for an
    internal verticale (§6.3). Not blocked by this tutorial; orthogonal.

## Next action

Operator decision required: **how to nucleate the 7–8 tasks?**

Three reasonable shapes (operator picks one):

1. **One task molecule per T** (T1..T8) — fine-grained, each is a small
   PR. Maximum cosmon visibility, more nucleation overhead.
2. **One umbrella task molecule** with sub-steps (T1..T8 as a checklist
   in the briefing) — single PR, single review pass, less overhead.
3. **Two task molecules** — `tutorial-mechanics` (T1..T6, the runnable
   bits) and `tutorial-narrative` (T7..T8, the README + smoke test).
   Natural split between "make it work" and "explain why."

Default recommendation if no preference: **shape 2 (umbrella task)** —
the example is small enough that the overhead of splitting outweighs
the visibility gain, and the README depends on the mechanics being
runnable to be verifiable.

## Done means

This `idea-to-plan` molecule completes once this plan document is
committed. The next gesture (nucleating the implementation
molecule(s)) is the operator's, not this molecule's. The three docs in
`docs/design/itwinai-tutorial-*.md` are the persistent artifact of this
idea's evolution from raw note → captured idea → assessed feasibility →
actionable plan.
