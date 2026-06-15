# ox test

Test and validate a workflow without executing it.

The `ox test` command resolves the DAG, checks for structural errors, and
optionally simulates execution order — all without running any shell commands.
Use it to catch misconfigurations before committing to a full run.

## Usage

```bash
ox test                             # Validate entire workflow
ox test results/report.html         # Validate a specific target
ox test --dry-run                   # Simulate execution order
ox test --json                      # Output NDJSON diagnostics
```

## Arguments

| Argument | Description |
|----------|-------------|
| `[TARGETS]...` | Target files or patterns to test (default: all) |

## Options

| Flag | Description |
|------|-------------|
| `-f, --file <FILE>` | Oxymakefile path (default: `Oxymakefile.toml`) |
| `-n, --dry-run` | Simulate execution order without running |
| `--json` | Output NDJSON |

## What It Checks

- Oxymakefile parses without errors
- All wildcards resolve against `[config]` values
- Dependency graph is acyclic
- Every input is either a source file or produced by a rule
- Wildcard constraints are satisfied

## Examples

```bash
# Quick validation in CI
ox test || exit 1

# Check a single target's dependency chain
ox test results/{sample}_stats.tsv

# Dry-run to see execution order
ox test --dry-run
```

## See Also

- [ox lint](../commands.md#ox-lint) — lighter-weight syntax check
- [ox plan](../commands.md#ox-plan) — show full execution plan
