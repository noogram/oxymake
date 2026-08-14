# Oxymakefile Format

OxyMake workflows are defined in `Oxymakefile.toml`, a declarative TOML file.
This page is the complete format reference.

## Top-Level Fields

```toml
ox_version = "0.1"           # Required. OxyMake format version.
```

## Config Section

The `[config]` section defines workflow-level variables used for wildcard
expansion:

```toml
[config]
samples = ["A", "B", "C"]
chromosomes = ["chr1", "chr2", "chr3"]
models = ["linear", "ridge", "lasso"]
```

Config values are arrays of strings. They drive wildcard expansion in rules.

## Rule Definitions

Each rule is a `[rule.<name>]` table:

```toml
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "python process.py {input} {output}"
```

### Rule Fields

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `input` | Array of strings | No | Input file patterns with `{wildcards}` |
| `output` | Array of strings | Yes | Output file patterns with `{wildcards}` |
| `shell` | String | One of shell/run/script/call | Opaque shell command |
| `run` | String | One of shell/run/script/call | Inline script (with `lang`) |
| `script` | String | One of shell/run/script/call | Path to script file |
| `call` | String | One of shell/run/script/call | Python function reference |
| `lang` | String | With `run`/`script` | Language: `python`, `r`, `julia` |
| `tags` | Table of string → string | No | Key/value labels for grouping and event filtering, e.g. `tags = { stage = "align", speed = "slow" }`. An array of strings is **not** accepted. |
| `resources` | Table | No | Resource requirements |
| `env` | String | No | Environment to use |
| `when` | String | No | Conditional guard expression |
| `materialize` | String | No | `always`, `auto`, `never`, `final` |
| `params` | Table | No | Rule-specific parameters |

### Execution Modes

Four modes form a spectrum from flexibility to optimizability:

**shell** -- Opaque shell command. Maximum flexibility, no optimization.
```toml
[rule.align]
shell = "bwa mem ref.fa {input} > {output}"
```

**run** -- Inline script with language specification.
```toml
[rule.stats]
lang = "python"
run = """
import pandas as pd
df = pd.read_csv("{input}")
df.describe().to_csv("{output}")
"""
```

**script** -- External script file.
```toml
[rule.analyze]
lang = "python"
script = "scripts/analyze.py"
```

**call** -- Pure function reference. Supports in-memory Arrow IPC passing.
```toml
[rule.features]
input = [{ path = "data/{sample}.parquet", format = "parquet" }]
output = [{ path = "features/{sample}.parquet", format = "parquet", materialize = "auto" }]
call = "pipeline.features:compute_features"
```

### Wildcards

Wildcards in `{braces}` are resolved from `[config]` arrays or from a requested
target path that matches a rule output pattern. Existing files are recognized
as source inputs; their names do not themselves enumerate wildcard values.

```toml
[config]
samples = ["A", "B"]

[rule.process]
input = ["data/{sample}.csv"]     # {sample} expanded from config.samples
output = ["results/{sample}.txt"]
```

### Resources

```toml
[rule.heavy_job]
output = ["results/big.txt"]
shell = "compute_heavy"
resources = { cpus = 4, mem_gb = 16, gpu = 1, time_min = 60 }
```

### Conditional Guards

```toml
[rule.expensive]
output = ["results/{seed}.txt"]
shell = "compute {seed}"
when = "seed in @selected_seeds"
```

Guards are evaluated at DAG resolution time. Jobs whose guard is false are
never created.

## Include Directives

Split large workflows across files:

```toml
include = ["rules/alignment.toml", "rules/qc.toml"]
```

## Environment Specification

A rule declares its environment with an `environment` inline table whose
*key* is the backend and whose value is that backend's argument:

```toml
[rule.analyze]
output = ["results/summary.txt"]
shell = "python analyze.py"
environment = { uv = "requirements.txt" }
```

Supported keys: `uv`, `conda`, `docker`, `apptainer`, `nix`. Omitting
`environment` runs the command on the host as-is.

A top-level `environment` table sets the default for every rule that does not
declare its own:

```toml
environment = { uv = "pyproject.toml" }
```

There is no named-environment mechanism: `[env.NAME]` blocks and a rule-level
`env = "NAME"` reference are not part of the format, and — because rule tables
do not reject unknown keys — they are silently ignored rather than reported as
an error. See [Environments](../concepts/environments.md) for what each backend
does.

## Next Steps

- [CLI Commands](./commands.md) -- how to run workflows
- [Expression Language](./expressions.md) -- guard and expression syntax
- [Configuration](./configuration.md) -- project-level settings
