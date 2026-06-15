# Intermediate File Deletion Benchmark

Minimal reproduction demonstrating how ox and snakemake handle deleted
intermediate files.

## The DAG

A simple linear chain with no wildcards:

```
step_a → step_b → step_c → step_d
```

Each step writes a timestamp and appends its input, creating a traceable
provenance chain.

## The Test

1. Run the full pipeline from scratch (both tools produce all 4 files)
2. Delete `step_b.txt` — an intermediate output
3. Rerun — both tools should detect the gap and rebuild B → C → D

## Expected vs Observed

| Tool      | Detects missing intermediate? | Rebuilds downstream? |
|-----------|-------------------------------|----------------------|
| ox        | Yes                           | Yes (B, C, D)       |
| snakemake | No                            | No ("Nothing to do") |

**Why snakemake misses it:** Snakemake uses mtime-based comparison between
each rule's *direct* inputs and outputs. When `step_b.txt` is deleted,
snakemake checks whether the final target `step_d.txt` needs rebuilding by
looking at `step_c.txt` → `step_d.txt`. Since `step_d.txt` exists and is
newer than `step_c.txt`, snakemake concludes nothing needs to be done. It
does not verify that all intermediate files in the chain still exist.

**Why ox catches it:** ox tracks every declared output in the DAG. When any
output file is missing, ox marks that rule (and all downstream rules) as
needing execution, regardless of mtime relationships.

## Running

```bash
# Requires: ox, snakemake, just
just compare
```

## Files

- `Oxymakefile.toml` — ox workflow definition
- `Snakefile` — equivalent snakemake workflow
- `justfile` — automated comparison recipe
