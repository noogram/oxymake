# Feasibility — OxyMake × itwinai Tutorial

**Formula:** `idea-to-plan`
**Step:** 2/3 — evaluate feasibility
**Captured:** 2026-05-27
**Companion:** [itwinai-tutorial-idea.md](itwinai-tutorial-idea.md)

## Summary verdict

**FEASIBLE** — green-lit for step 3 (plan).

All four open questions from step 1 resolve positively. No OxyMake or
itwinai source change is required. The tutorial is a pure composition
exercise using both tools' existing public CLIs.

Estimated effort: **3–5 working days** for a polished, runnable, tested
tutorial (env setup + 3 rules + README with positioning + 3 demo
commands + a smoke-test script).

## Resolution of the four open questions

### Q1 — itwinai install footprint

**Resolved: acceptable for laptop tutorial.**

itwinai `0.4.2` (`<itwinai>/pyproject.toml`) requires Python ≥3.10
and core deps that fit a standard dev laptop. The heaviest baseline
dependency is `ray[default,train,tune]>=2.43.0` (a few hundred MB). The
optional `[torch]` extra adds torch 2.6, lightning, torchmetrics,
torchvision, torchaudio — pushing total install to ~1–2 GB.

For the tutorial, we install only what we need:

```bash
pip install itwinai[torch]
```

The tutorial does NOT require GPU, NCCL, or HPC backends. A CPU MLP on
synthetic data (or MNIST) runs in seconds and validates the orchestration
contract without any CUDA dependency.

**Risk note:** itwinai pins `numpy<2.0.0`. This is fine in an isolated
venv but would conflict with a global Python env. The tutorial's setup
section MUST instruct users to create a dedicated venv (`python -m venv`
or `uv venv`).

### Q2 — Toy model choice

**Resolved: MNIST with a 2-layer MLP, ≤1 epoch.**

Justification:

- **MNIST** is the universally recognized "hello world" of ML. Readers
  recognize the dataset shape (28×28 grayscale, 10 classes) instantly,
  freeing all attention for the orchestration content of the tutorial.
- **2-layer MLP** trains in seconds on CPU. No GPU required. No data
  augmentation pipeline. The model is incidental — the orchestration is
  the point.
- **≤1 epoch** keeps the demo runs sub-30-seconds end-to-end, which
  matters for the cache-skip demo (a slow training step would make the
  cache-hit/cache-miss distinction harder to feel viscerally).

The `examples/intermediate-deletion/Oxymakefile.toml` precedent in this
repo already follows this discipline (toy linear DAG, sub-second
per-step). We extend it from `echo` rules to a real ML step.

**Alternative considered and rejected:** synthetic-data regression with a
linear model. Faster but loses the "this is real ML" pedagogical hook.
MNIST wins on readability.

### Q3 — Cache key fingerprinting contract

**Resolved: OxyMake's cache contract is sufficient as-is — no code change
needed.**

`crates/ox-cache/src/key.rs` (`compute_cache_key`) hashes together:

1. `rule_source_hash` — the shell command text.
2. `sorted(input_content_hashes)` — every file declared as `input` to the rule.
3. `params_hash` — declared params.
4. `env_hash` — environment spec.
5. Platform (OS + arch).

For the tutorial's `train` rule, declaring `train.yaml` as an `input`
makes the YAML config's content hash part of the cache key. Editing
`train.yaml` (any hyperparameter, any architecture change, any seed
change) changes its blake3 hash → changes the cache key → triggers
re-execution of `train` and downstream `eval`. The three reproducibility
invariants from the idea doc map cleanly onto this contract:

| Edit | Cache impact | Re-runs |
|------|--------------|---------|
| `eval.py` (rule source of `eval`) | `rule_source_hash` of `eval` changes | only `eval` |
| `train.yaml` (input of `train`) | content hash of input → cache key of `train` changes | `train` + `eval` |
| Raw data file (input of `prepare`) | content hash of input → cache key of `prepare` changes | `prepare` + `train` + `eval` |

✅ No OxyMake change required. The tutorial's reproducibility claims are
honest as long as the `Oxymakefile.toml` declares `train.yaml` as an
explicit `input` of the `train` rule.

**Subtle pitfall worth flagging in the tutorial README:** if the user
edits the *body* of a Python script that `train.yaml` references via
`_target_: data.MyClass`, the cache will NOT detect it (the YAML's hash
is unchanged, and the Python source is not declared as an OxyMake input).
This is the user's responsibility — itwinai loads Python code dynamically
and OxyMake cannot statically discover the dependency graph. The README
must say: *"declare any Python script referenced from train.yaml as an
input of the `train` rule, otherwise its edits will be silently cached."*
This is a load-bearing footnote.

### Q4 — Tutorial host location

**Resolved: `examples/itwinai-tutorial/` (sibling of `examples/demo/`
and `examples/intermediate-deletion/`).**

The existing `examples/` directory already hosts runnable
`Oxymakefile.toml` examples with their own READMEs. The itwinai tutorial
fits the same template. A pointer line from the future
`docs/positioning.md` (separate molecule from analysis §6.1) can mention
it.

No new top-level directory. No new doc tree. The tutorial is a peer of
the existing examples — that is the right granularity.

## Technical feasibility — orthogonality verified

The itwinai CLI surface relevant to the tutorial:

- Entry point: `itwinai = "itwinai.cli:app"` (`pyproject.toml`).
- Subcommand: `itwinai exec-pipeline` (`src/itwinai/cli.py`, decorated
  with `@hydra.main(version_base=None, config_path=os.getcwd(),
  config_name="config")`).
- Argument: `--config-name=<name>` (no `.yaml` suffix — Hydra convention),
  `-cp <path>` to specify config dir.

This is a black-box CLI invocation, exactly the kind of shell command
OxyMake rules already handle. No FFI, no Python embedding, no IPC. The
composition mechanism is `Oxymakefile.toml` declaring:

```toml
[rule.train]
input = ["data/mnist_prepared.npz", "config/train.yaml"]
output = ["checkpoints/model.pth", "metrics/train.log"]
shell = """
mkdir -p checkpoints metrics
itwinai exec-pipeline --config-name=train -cp config 2>&1 | tee metrics/train.log
"""
```

(Exact CLI flags to be validated in step 3 during the env smoke-test.)

## Alignment with project goals

| OxyMake invariant | Tutorial demonstrates? | Notes |
|---|---|---|
| DAG > sequential | ✅ explicit | 3-rule DAG with linear edges; could extend to fork-join in a follow-up |
| Content-addressable cache | ✅ central | The three-edit demo (eval / config / data) materializes the contract |
| Polyglot by rule | ✅ implicit | The shell rule wraps a Python CLI; OxyMake itself stays Rust |
| Daemon-free | ✅ implicit | `ox run` start-work-exit; itwinai's Ray cluster is irrelevant to this tutorial |
| Agent-friendly (JSON output) | partial | Not central to this tutorial; mention in passing |

The tutorial does NOT introduce or compromise any OxyMake invariant. It
demonstrates three of them concretely against a real adjacent peer.

## Effort estimate breakdown

| Task | Effort |
|------|--------|
| itwinai venv install + smoke-test (`itwinai exec-pipeline --help`) | 0.5 day |
| MNIST data loader + 2-layer MLP itwinai pipeline | 1 day |
| `Oxymakefile.toml` with 3 rules (`prepare`, `train`, `eval`) | 0.5 day |
| Three demonstration scripts (cold cache, edit-eval, edit-config) | 0.5 day |
| README: positioning paragraph + slot map + walkthrough | 1 day |
| Smoke-test in CI (optional — gate the example in `cargo test`) | 0.5–1 day |
| **Total** | **3–4.5 days** |

Buffer: round to **3–5 working days**.

## Risks and mitigations

1. **itwinai install fragility** — risk: `pip install itwinai[torch]` fails on macOS arm64 due to torch wheel mismatches. *Mitigation:* document tested platforms in the README (e.g., "tested on macOS arm64 + Linux x86_64 with Python 3.11"), and pin specific versions in a `requirements.txt`.
2. **itwinai API churn** — risk: itwinai 0.4.2 → 0.5.x changes the `exec-pipeline` CLI signature. *Mitigation:* pin itwinai version in `requirements.txt`; document the tested version explicitly. The orthogonality argument survives any specific itwinai version.
3. **Cache invariant pitfall (Python source not declared as input)** — risk: a user edits `data.py` referenced by `train.yaml`, gets a stale cached run, and concludes OxyMake is broken. *Mitigation:* load-bearing footnote in README (see Q3 resolution).
4. **Maintainability** — risk: the example bit-rots as OxyMake's rule syntax evolves. *Mitigation:* include a tiny smoke-test in the CI gate (or at minimum, a `make test` target in the example dir that runs the full pipeline).

## Recommendation for step 3

Proceed to step 3 (actionable plan) with the scope locked as documented
above. No ADR is required — the tutorial introduces no new architectural
decision; it is a composition example using existing public APIs of both
tools. The plan in step 3 should produce a concrete task list with
ordered, atomic deliverables.
