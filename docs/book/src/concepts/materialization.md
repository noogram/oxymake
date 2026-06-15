# Materialization Policy

When a `call`-mode rule produces an output, does it need to be written to
disk? Not always. OxyMake lets you control this with the **materialization
policy**, enabling significant speedups for workflows where intermediate
outputs are only consumed by other `call`-mode rules.

## The Four Policies

| Policy | Behavior |
|--------|----------|
| `always` | (default) Write to disk after every job. Reproducible, cacheable. |
| `auto` | Write to disk only if a downstream job needs a file (not a `call` peer) |
| `never` | Keep in memory only. Lost if the process dies. Not cached. |
| `final` | Write to disk only if this output is a leaf of the DAG (a final result) |

## Declaring Materialization

Set the policy on individual outputs:

```toml
[rule.compute_features]
output = [{
    path = "features/{sample}.parquet",
    format = "parquet",
    materialize = "auto"
}]
call = "pipeline.features:compute_features"
lang = "python"

[rule.train_model]
output = [{
    path = "models/{sample}.pkl",
    format = "pickle",
    materialize = "always"
}]
call = "pipeline.model:train"
lang = "python"
```

In this example, the features DataFrame is only written to disk if a
non-`call` downstream rule needs it as a file. The model is always saved.

## Setting the Policy per Output

Materialization is declared per output in the Oxymakefile, on the structured
output form:

```toml
[rule.compute_features]
# ...
output = [
    { path = "data/features.parquet", materialize = "auto" },
]
```

Valid values are `auto` (the default — write to disk only when a downstream
file consumer needs it), `never` (keep in memory; no disk, no caching),
`final` (write only leaf outputs), and `always` (write and cache every
output). There is no global `ox run` flag that overrides the policy today;
control it in the Oxymakefile per output.

Guidance for development workflows:

- During prototyping, set `materialize = "never"` on intermediate outputs to
  iterate fast
- For production, use the default `auto` (or `always`) for full caching and
  reproducibility
- For presentations or reports, set leaf outputs to `final`

## How It Works with `call` Mode

When two consecutive rules both use `call` mode on the local executor,
OxyMake can pass data directly in memory:

```
compute_features  ──[DataFrame in memory]──>  train_model
     (call)                                      (call)
```

No file is written between them. The `format` field tells OxyMake how to
serialize the data if materialization is needed later (e.g., for caching
or for a `shell`-mode downstream rule).

### The Flow

1. `compute_features` runs and returns a DataFrame
2. If `materialize = "auto"` and the next consumer is also `call` mode:
   pass the DataFrame directly in memory
3. If `materialize = "auto"` and the next consumer is `shell` mode:
   write the DataFrame to disk using the `parquet` codec
4. If `materialize = "always"`: always write to disk (and cache)
5. If `materialize = "never"`: never write to disk (no cache)

## Constraints

Not everything supports non-`always` materialization:

- **`shell`, `run`, and `script` modes** always materialize. They manage
  their own I/O and need real files.
- **Distributed executors** (SLURM, K8s) force materialization because
  jobs run on separate machines.
- **Non-materialized outputs are not cached.** If the process dies or you
  restart, they will be recomputed. This is an explicit trade-off: speed
  vs. reproducibility.

## The `--materialize` Flag

The CLI flag sets the **floor** for materialization:

| Flag value | Effect |
|------------|--------|
| `always` | All outputs written to disk (default behavior) |
| `auto` | Per-output policy respected |
| `never` | No outputs written (memory only, for testing) |
| `final` | Only DAG-leaf outputs written |

## Practical Example

Consider a three-stage pipeline:

```toml
[rule.load_data]
output = [{ path = "data/{s}.parquet", format = "parquet", materialize = "auto" }]
call = "pipeline:load_data"
lang = "python"

[rule.compute_features]
output = [{ path = "features/{s}.parquet", format = "parquet", materialize = "auto" }]
call = "pipeline:compute_features"
lang = "python"

[rule.generate_report]
output = [{ path = "reports/{s}.html", materialize = "always" }]
call = "pipeline:generate_report"
lang = "python"
```

With `ox run --materialize=final`:
- `load_data` output: kept in memory (not a leaf)
- `compute_features` output: kept in memory (not a leaf)
- `generate_report` output: written to disk (it is a leaf)

Only the final HTML report touches the filesystem. The intermediate
parquet files exist only in memory during execution.
