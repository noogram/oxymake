# Execution Modes

OxyMake supports four ways to execute a rule, forming a spectrum from
maximum flexibility to maximum optimization. All four modes coexist in
the same workflow -- you pick the right one for each rule.

## The Spectrum

```
shell       Opaque, files only, maximum flexibility
run         Inline script, files only, author manages I/O
script      External script, files only, author manages I/O
call        Pure function, files OR memory, OxyMake manages I/O
```

As you move from `shell` to `call`, OxyMake gains more optimization power
(in-memory data passing, task fusion, automatic serialization) -- but you
give up direct control over I/O.

## Mode 1: `shell` -- Command Line

The most flexible mode. You write a shell command, and OxyMake interpolates
file paths into its command string before passing that string to the configured
shell. This is raw shell text, not typed argv passing: validate any
third-party-derived path, wildcard, or configuration value before interpolating
it. See [SECURITY.md](https://github.com/noogram/oxymake/blob/main/SECURITY.md#shell-interpolation-is-a-datacode-boundary).

```toml
[rule.align]
input = ["data/{sample}.fastq", "refs/genome.fa"]
output = ["results/{sample}.bam"]
shell = "bwa mem -t {resources.cpu} {input[1]} {input[0]} > {output}"
resources = { cpu = 8 }
```

Use `shell` when you are wrapping an existing command-line tool. OxyMake
treats the command as a black box -- it just passes file paths and checks
that outputs were created.

## Mode 2: `run` -- Inline Script

Write a short script directly in the Oxymakefile. OxyMake interpolates
`{input}` and `{output}` as file paths.

```toml
[rule.analyze]
input = ["data/{sample}.csv"]
output = ["results/{sample}.json"]
lang = "python"
run = """
import pandas as pd
import json
df = pd.read_csv("{input}")
stats = df.describe().to_dict()
with open("{output}", "w") as f:
    json.dump(stats, f)
"""
```

Use `run` for rapid prototyping -- when the logic is short enough to live
in the workflow file. You manage all file I/O yourself.

## Mode 3: `script` -- External Script

Like `run`, but the code lives in a separate file. Keeps the Oxymakefile
clean when scripts are long.

```toml
[rule.transform]
input = ["data/{sample}.parquet"]
output = ["results/{sample}.parquet"]
script = "scripts/transform.py"
environment = { uv = "pyproject.toml" }
```

The script receives file paths via command-line arguments or environment
variables.

## Mode 4: `call` -- Pure Function

The key innovation. Your function receives **objects**, not file paths, and
returns objects. OxyMake handles all I/O outside the function.

```toml
[rule.compute_features]
input = [{ path = "data/{sample}.parquet", format = "parquet" }]
output = [{ path = "features/{sample}.parquet", format = "parquet", materialize = "auto" }]
call = "pipeline.features:compute_features"
lang = "python"
```

The Python function is pure:

```python
import polars as pl

def compute_features(df: pl.DataFrame) -> pl.DataFrame:
    return df.with_columns(
        mean_depth=pl.col("depth").rolling_mean(20),
        depth_std=pl.col("depth").rolling_std(60),
    )
```

The function never reads or writes files. OxyMake:

1. Reads the input file using the `parquet` codec, producing a DataFrame
2. Calls `compute_features(df)` and receives the result
3. Writes the result to disk using the `parquet` codec (if materialization
   policy requires it)

In **memory mode** (when both upstream and downstream are `call` rules on
the local executor), step 1 receives the DataFrame directly from the
upstream job and step 3 passes it directly to the downstream job -- zero
disk I/O.

### Named Arguments

For functions with multiple inputs, use named inputs:

```toml
[rule.train_model]
input = { features = "features/{sample}.parquet", config = "configs/model.yaml" }
output = { model = "models/{sample}.pkl" }
call = "pipeline.model:train"
lang = "python"
```

```python
def train(features: pl.DataFrame, config: dict) -> Model:
    ...
```

The input keys (`features`, `config`) map to function parameter names.

### Warm workers and resource observations

`ox run --warm-workers fork` retains an imported Python template but forks a
new child for each `call` dispatch. The template reaps that child with
`os.wait4`, so the child's peak RSS and CPU time can be written to that
dispatch's benchmark output; peak RSS also reaches job history on Linux and
macOS. CPU time is local to the child. Peak RSS is the child's high-water mark,
but it starts with the warm template footprint inherited at fork. Only memory
above that baseline was added by this dispatch; the reported total is not the
memory the dispatch would need by itself. For a template that has imported a
large library such as NumPy or PyTorch, that baseline can dominate a small
dispatch's reported peak.

`ox run --warm-workers persistent` executes every dispatch inside one retained
process. Its RSS is a high-water mark over the worker's whole lifetime, not a
per-dispatch peak, and CPU intervals are not isolated. OxyMake therefore leaves
both observations absent rather than reporting a delta or sample as job usage.

## When to Use Each Mode

| Situation | Recommended mode |
|-----------|-----------------|
| Wrapping an existing CLI tool | `shell` |
| Quick one-off analysis | `run` |
| Reusable script, too long for inline | `script` |
| Pure data transformation, wants optimization | `call` |
| Prototyping (will refactor later) | `run` then migrate to `call` |

## The Migration Path

The natural evolution of a rule:

1. **Start with `run`**: Write inline code during exploration
2. **Extract to `script`**: When the code gets long, move it to a file
3. **Refactor to `call`**: When the function stabilizes, make it pure
   and let OxyMake manage I/O

Each step is backward-compatible -- the outputs are the same files.
The cache key changes (because the rule source changes), so the first
run after migration will recompute, but subsequent runs benefit from
the optimization.

## Interaction with Executors

| Mode | Local executor | SLURM/K8s executor |
|------|---------------|-------------------|
| `shell` | Subprocess | Remote submission |
| `run` | Subprocess | Remote submission |
| `script` | Subprocess | Remote submission |
| `call` (memory) | In-process via Arrow IPC | Forced to materialize |
| `call` (file) | Subprocess + codec | Remote submission + codec |

Distributed executors (SLURM, K8s) cannot pass objects in memory between
machines, so they automatically force `call` mode to materialize. Your
workflow does not need to change -- OxyMake handles this transparently.
