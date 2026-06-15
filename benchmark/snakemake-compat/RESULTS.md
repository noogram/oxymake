# Snakemake Compatibility Benchmark Results

Benchmark of `ox translate` + `ox run` on real-world Snakemake workflow patterns.

## Test Workflows

| # | Workflow | Rules | Samples | Features Tested |
|---|----------|-------|---------|-----------------|
| 01 | tutorial-basic | 8 | 3 | Named I/O, expand(VARIABLE), threads, multi-target |
| 02 | rnaseq-counts | 6 | 4 | configfile, wildcard_constraints, resources, config refs |
| 03 | csv-etl | 6 | 3 sources | expand(VARIABLE), named outputs, multi-output rules |
| 04 | multi-sample-qc | 5 | 5 | configfile, expand(config[key]), resources, multi-input expand |

## Translation Results

### What translates cleanly (no manual fixes needed)

- **Basic rule structure**: input/output/shell blocks
- **Named I/O**: `input = { genome = "...", reads = "..." }` works perfectly
- **Wildcards**: `{sample}`, `{source}` preserved correctly
- **wildcard_constraints**: per-rule constraints translated to `[rule.X.wildcard_constraints]`
- **params**: both literal and config-referenced params
- **threads → resources.cpu**: `threads: 4` becomes `resources = { cpu = 4 }`
- **resources**: `mem_mb=4000` becomes `mem = "4000M"`
- **expand(pattern, key=config["key"])**: config-referenced expand works (Tier 1)
- **expand(pattern, key=[literals])**: literal-list expand works (Tier 1)
- **configfile + YAML resolution**: config values extracted from YAML correctly
- **Config references in params**: `config["genome"]` → `{config.genome}`

### What needs manual fixes

| Issue | Severity | Workaround |
|-------|----------|------------|
| **expand(pattern, var=PYTHON_VAR)** escalated | HIGH | Add values to `[config]` section, set `expand = "product"` |
| **No `expand = "product"` on aggregation rules** | HIGH | Manually add `expand = "product"` to rules that aggregate wildcard inputs |
| **`{input.name}` with expand only returns first file** | HIGH | Use unnamed `{input}` and filter by pattern in shell |
| **`{{` double-brace escaping not handled** | MEDIUM | Replace `${{var}}` with `${var}` (ox doesn't use `{{` escape) |
| ~~**Output directories not auto-created**~~ | ~~MEDIUM~~ | **Fixed**: executor auto-creates output parent dirs in `prepare_workspace` |
| **`/bin/sh` not `/bin/bash`** | MEDIUM | Avoid bash-only features like `<()` process substitution |
| **BTreeMap rule ordering** | LOW | Default target uses alphabetically-first rule, not `rule all`; specify targets explicitly |
| **`{wildcards.X}` → `{X}`** | LOW | Translator preserves `{wildcards.sample}` but ox uses `{sample}` |
| **Global wildcard_constraints not translated** | LOW | Apply constraints to individual rules manually |
| **`{log}` redirect target** | LOW | Translator creates `log = { stdout = "...", stderr = "..." }` but `{log}` in shell needs adjustment |

### What's unsupported

- **Python imports and top-level code**: warned, silently dropped
- **Dynamic thread counts** (`threads: lambda wildcards: ...`): escalated
- **Wrapper directive**: escalated (needs inlining)
- **Shadow/group/envmodules**: escalated
- **benchmark/retries/priority directives**: deferred (waiting on OxyMake features)

## Execution Results

| Workflow | Translate | Dry-run | Execute | Manual fixes | Time |
|----------|-----------|---------|---------|--------------|------|
| 01-tutorial-basic | 2 escalations | Pass (explicit targets) | **Pass** (13 jobs) | 5 | 2.2s |
| 02-rnaseq-counts | Clean (1 info) | Pass (explicit targets) | **Pass** (17 jobs) | 4 | ~90s |
| 03-csv-etl | 1 escalation | Pass (explicit targets) | **Pass** (9 jobs) | 3 | 3.3s |
| 04-multi-sample-qc | Clean (3 infos) | Pass (explicit targets) | **Pass** (16 jobs) | 4 | ~60s |

All 4 workflows execute to completion after manual fixes.

## Key Findings

### 1. expand() with Python variables is the #1 gap
When `expand()` references a Python variable (`SAMPLES = [...]`), the translator
escalates it as "complex iterator". The translator correctly identifies the
pattern and creates an escalation, but cannot auto-resolve because the variable
is Python-scoped. **Fix**: The translator already puts the variable values in
`[config]`; it should also set `expand = "product"` on the consuming rule.

### 2. Named inputs don't work correctly with expand
`{input.name}` in an expanded rule only substitutes the first matching file path.
This is a significant limitation for aggregation rules that iterate over all
expanded inputs by name. **Workaround**: Use unnamed `{input}` and filter in shell.

### 3. Output directory auto-creation ~~needed~~ **FIXED**
The executor's `prepare_workspace` now auto-creates parent directories for all
`OutputRef::File` outputs before rule execution, matching Snakemake's behavior.
Manual `mkdir -p` in shell commands is no longer needed.

### 4. Shell runs under /bin/sh, not /bin/bash
Process substitution `<()`, arrays, and other bash-isms fail. Use POSIX-compatible
shell constructs.

### 5. BTreeMap ordering affects default target
Rules are stored in BTreeMap (alphabetical order), so `rule all` may not be the
first rule. Rules like `align` sort before `all`. Specify targets explicitly.

## Recommendations for Translator Improvements

1. **Auto-set `expand = "product"`** when translating `expand()` calls
2. **Handle `{{` → `{` escaping** (Snakemake's literal brace syntax)
3. **Warn about bash-only constructs** in shell blocks
4. ~~**Add `mkdir -p` for output directories** automatically~~ **Done**
5. **Translate `{wildcards.X}` to `{X}`** in shell blocks
6. **Support named inputs in expand** — concatenate all paths for `{input.name}`
