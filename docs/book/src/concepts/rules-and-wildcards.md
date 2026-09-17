# Rules and Wildcards

## What is a Rule?

A rule declares a transformation: given these **inputs**, produce these
**outputs** by running this **command**. OxyMake figures out what needs
to run based on what you ask for.

```toml
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "python process.py {input} {output}"
```

This single rule handles ANY sample. When you ask for `results/A.txt`,
OxyMake matches the output pattern, extracts `sample = "A"`, substitutes
it into the input pattern to get `data/A.csv`, and runs the command.

## Wildcards

Wildcards are placeholders in curly braces: `{sample}`, `{cohort}`,
`{model}`. They appear in input and output file patterns.

### How wildcards resolve

OxyMake uses **backward chaining**: start from the output you want,
find which rule can produce it, extract wildcard values from the match.

```
You ask for: results/patient_42.txt
                     ↓
Pattern:     results/{sample}.txt
                     ↓
Extracted:   sample = "patient_42"
                     ↓
Input becomes: data/patient_42.csv
```

### Multiple wildcards

Rules can have multiple wildcards:

```toml
[rule.analyze]
input = ["data/{cohort}/{region}.parquet"]
output = ["results/{cohort}/{region}/report.html"]
shell = "python analyze.py {input} {output}"
```

### Wildcard expansion from config

When you have a list of known values, put them in `[config]`:

```toml
[config]
samples = ["A", "B", "C"]

[rule.all]
input = ["results/{sample}.txt"]
```

The `all` rule has `{sample}` in its inputs but no outputs — it's an
**aggregation target**. OxyMake expands `{sample}` from `config.samples`
to request `results/A.txt`, `results/B.txt`, `results/C.txt`.

### Expansion modes

When multiple wildcards expand from config lists, the expansion can be:

```toml
[config]
samples = ["A", "B"]
conditions = ["treated", "control"]

[rule.experiment]
output = ["results/{sample}_{condition}.csv"]
expand = "product"    # default: A_treated, A_control, B_treated, B_control
```

| Mode | Behavior | Count |
|------|----------|-------|
| `product` (default) | All combinations (Cartesian product) | N × M |
| `zip` | Parallel pairs (lengths must match) | N |

### Wildcard constraints

Restrict which values a wildcard can take:

```toml
[rule.process]
output = ["results/{sample}.txt"]

[rule.process.wildcard_constraints]
sample = "[A-Z][a-z0-9_]*"    # regex: starts with uppercase letter
```

### Existing files and rule ownership

An existing file that matches a wildcard output belongs to that rule when its
inputs can be resolved. For example, with `raw/{x}.csv` → `data/{x}.csv`, if both
`raw/manual.csv` and a hand-written `data/manual.csv` exist, `ox run` can regenerate
and overwrite `data/manual.csv`, just as with make pattern rules.

If `raw/manual.csv` is missing and has no producer, an existing hand-maintained
`data/manual.csv` is instead treated as a source. The same applies to leading
wildcards (`{dir}/data.csv`) and config-derived directories
(`{config.outdir}/{x}.csv`). Missing intermediate outputs are still rebuilt when
their source inputs are available. Cycles, ambiguous producers, and invalid
configuration remain errors. A missing source inside another producer is also
an error; an existing final output cannot hide a broken dependency chain. Fixed
inputs of wildcard rules, such as `cfg/settings.txt`, are required for every
instance even when sample-specific inputs are missing.

Use `wildcard_constraints` to reserve hand-maintained names, even when matching
raw inputs exist. For example, this rule only owns numbered samples, leaving
`data/manual.csv` as a source:

```toml
[rule.prepare]
input = ["raw/{x}.csv"]
output = ["data/{x}.csv"]
shell = "cp {input} {output}"

[rule.prepare.wildcard_constraints]
x = "sample[0-9]+"
```

Outputs recorded in the cache retain their provenance requirements. If their
original inputs are missing, only verified `cache-import` adoption permits them
as leaves. `ox run --no-cache` disables that shortcut and errors until the inputs
are restored. A modified adopted output also requires its producer's inputs.
An absent cache permits unrecorded manual files, while a corrupt or unreadable
cache denies that fallback. CLI, MCP and the Rust API share these rules and check
source existence and cache provenance relative to the Oxymakefile directory.

### Conditional guards

Rules can apply only to certain wildcard values:

```toml
[config]
special_samples = ["X1", "X2"]

[rule.extra_analysis]
input = ["results/{sample}.txt"]
output = ["extra/{sample}_analysis.html"]
when = "sample in @special_samples"
```

This rule exists only for samples X1 and X2. Other samples don't get
the extra analysis — no phantom nodes in the graph, no skipped jobs.

Guards support: `in @list`, `not in @list`, `== 'value'`, `!= 'value'`,
`=~ 'regex'`.

## The Four Execution Modes

| Mode | Keyword | Who manages I/O | In-memory possible |
|------|---------|-----------------|-------------------|
| Shell | `shell = "..."` | You | No |
| Inline script | `run = "..."` | You | No |
| External script | `script = "path"` | You | No |
| Pure function | `call = "mod:fn"` | OxyMake | Yes |

Start with `shell` or `run` for quick prototyping. Migrate to `call`
when your function stabilizes and you want OxyMake to optimize I/O.

See [Execution Modes](./execution-modes.md) for details.
