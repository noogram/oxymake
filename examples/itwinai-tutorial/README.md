# OxyMake × itwinai — MNIST tutorial

A three-rule OxyMake DAG that orchestrates an
[itwinai](https://github.com/interTwin-eu/itwinai) training pipeline.
The point of the example is **not** the model (a 1-epoch MLP on MNIST)
but the *composition*: OxyMake supplies the cache and the DAG, itwinai
supplies the in-process ML pipeline. They cooperate through a single
shell command per rule. No FFI, no Python embedding.

## What this is

```
prepare ─→ train ─→ eval
   |        |         |
prepare.py  itwinai    eval.py
            exec-pipeline
            (Hydra: train.yaml)
```

Three rules, declared in `Oxymakefile.toml`:

| Rule      | Payload                              | Output                       |
|-----------|--------------------------------------|------------------------------|
| `prepare` | `python src/prepare.py …`            | `data/mnist.npz`             |
| `train`   | `itwinai exec-pipeline …`            | `checkpoints/model.pth`      |
| `eval`    | `python src/eval.py …`               | `metrics/test_accuracy.txt`  |

## Why

The tutorial demonstrates three OxyMake invariants against a real ML
peer:

1. **Orthogonality** — OxyMake (DAG/cache) and itwinai (ML pipeline)
   compose by `Oxymakefile.toml` declaring the latter's CLI as the shell
   payload of one rule. Neither tool knows about the other.
2. **Content-addressable cache above `itwinai exec-pipeline`** — itwinai
   runs deterministically inside a single Python process; OxyMake caches
   *across* runs. The combination gives "don't re-train if nothing
   relevant changed."
3. **Granular reproducibility** — three demo edits (eval.py, train.yaml,
   data) trigger three different downstream re-run patterns, materialized
   by `scripts/demo-*.sh`.

## Slot map

A 2×2 mental model of related tools — DAG vs sequential on one axis,
glue (workflow runner) vs orchestrator (pipeline framework) on the other.

```
                    ┌─────────────────────┬─────────────────────┐
                    │  GLUE (workflow)    │  ORCHESTRATOR (ML)  │
   ┌────────────────┼─────────────────────┼─────────────────────┤
   │  DAG           │  OxyMake, Nextflow, │  itwinai (Pipeline  │
   │                │  Snakemake          │  is sequential —    │
   │                │                     │  see ↓ ↓)           │
   ├────────────────┼─────────────────────┼─────────────────────┤
   │  SEQUENTIAL    │  bash, justfile     │  itwinai, ZenML,    │
   │                │                     │  MLflow recipes     │
   └────────────────┴─────────────────────┴─────────────────────┘
```

OxyMake sits in **DAG × glue**: a Rust workflow runner with a
content-addressable cache. itwinai sits in **sequential × orchestrator**:
a Python ML pipeline framework whose `Pipeline.steps` is a list. The two
are complementary, not competitive. This tutorial sits in the joint.

## Prerequisites

- Python ≥ 3.10 (3.11 recommended)
- Linux x86_64 or macOS arm64 (other platforms may need torch wheel work)
- A working OxyMake binary on `$PATH` (`cargo install --path crates/ox-cli`
  from the workspace root, or `cargo build --release` + add `target/release`
  to `$PATH`)

itwinai pins `numpy<2.0.0` — install into a **dedicated venv**, not a
global Python env, or you will collide with newer numpy.

```bash
python -m venv .venv
source .venv/bin/activate
pip install -r requirements.txt
itwinai --help     # should print the itwinai CLI subcommands
ox --version       # should print an oxymake version
```

## Quick start

```bash
ox run all
```

That single command:

1. Downloads MNIST (first run only) and writes `data/mnist.npz`.
2. Calls `itwinai exec-pipeline` with `config/train.yaml`, training a
   2-layer MLP for one CPU epoch and writing `checkpoints/model.pth`.
3. Runs `src/eval.py` to compute test accuracy → `metrics/test_accuracy.txt`.

Total wall time on a stock laptop: under a minute (MNIST download +
1 epoch + eval). Re-running `ox run all` immediately should be 3 cache
hits and finish in milliseconds.

## The three demos

Three small scripts under `scripts/` illustrate how the cache reacts to
the three independent change axes:

| Script                       | Edit                          | Expected re-runs            |
|------------------------------|-------------------------------|-----------------------------|
| `scripts/demo-cold.sh`       | `ox clean && ox run all`      | prepare + train + eval      |
| `scripts/demo-edit-eval.sh`  | append newline to `src/eval.py` | eval only                 |
| `scripts/demo-edit-config.sh`| append comment to `config/train.yaml` | train + eval        |

Each script ends with an `echo` line restating what should have happened,
so you can compare against the live `ox run` output.

## Cache footnote (load-bearing)

OxyMake's cache hashes the **shell command text**, the **content of every
declared input file**, declared **params**, **env spec**, and the
**platform**. It does NOT statically discover Python imports.

This matters because `config/train.yaml` references Python classes via
Hydra's `_target_:` strings:

```yaml
- _target_: src.pipeline.MlpTrainer
```

If you edit the *body* of `src/pipeline.py`, the YAML's hash is
unchanged. OxyMake will hit the cache and you will get a stale `train`
result. To prevent this, `src/pipeline.py` is declared as an explicit
input of the `train` rule in `Oxymakefile.toml`:

```toml
[rule.train]
input = [
  "data/mnist.npz",
  "src/pipeline.py",
  "config/train.yaml",
]
```

**Rule of thumb:** any Python module referenced from `train.yaml` via
`_target_:` MUST appear as an OxyMake input of the rule that runs the
config. Otherwise edits to that Python file will be silently cached.

This is the price of orthogonality. OxyMake cannot read Python code;
declaring inputs is your responsibility.

## Where to go next

- **Positioning argument** — the repo's `docs/positioning.md` (and the
  longer thesis it points to) explains why OxyMake exists alongside
  Snakemake, Nextflow, and itwinai rather than instead of them.
- **itwinai docs** — https://itwinai.readthedocs.io for the broader
  pipeline framework: distributed training backends, hyperparameter
  search, logger integrations. None of those are exercised here.
- **Other OxyMake examples** — `examples/demo/` (word-frequency pipeline)
  and `examples/intermediate-deletion/` (cache behavior under deleted
  intermediates).

## File map

```
examples/itwinai-tutorial/
├── README.md                  # this file
├── Oxymakefile.toml           # 3 rules: prepare, train, eval
├── requirements.txt           # pinned itwinai + torch versions
├── config/train.yaml          # itwinai pipeline config (Hydra)
├── src/
│   ├── prepare.py             # MNIST → data/mnist.npz
│   ├── pipeline.py            # itwinai BaseComponent subclasses
│   └── eval.py                # checkpoint → metrics/test_accuracy.txt
└── scripts/
    ├── demo-cold.sh
    ├── demo-edit-eval.sh
    └── demo-edit-config.sh
```
