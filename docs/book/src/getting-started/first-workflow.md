# Your First Workflow

This tutorial walks you through creating a simple 3-rule workflow from
scratch. By the end, you will understand how OxyMake resolves dependencies,
runs jobs, and caches results.

## Step 1: Create a Project

Create a new directory and initialize OxyMake:

```bash
mkdir my-pipeline
cd my-pipeline
ox init
```

This creates a starter `Oxymakefile.toml`. We will replace its contents.

## Step 2: Create Some Input Data

Create a `data/` directory with two CSV files:

```bash
mkdir data
```

`data/alice.csv`:
```text
name,score
Alice,85
Alice,92
Alice,78
```

`data/bob.csv`:
```text
name,score
Bob,91
Bob,88
Bob,95
```

## Step 3: Write the Workflow

Replace the contents of `Oxymakefile.toml` with:

```toml
ox_version = "0.1"

[config]
students = ["alice", "bob"]

# Rule 1: Compute statistics for each student
[rule.stats]
input = ["data/{student}.csv"]
output = ["results/{student}_stats.json"]
lang = "python"
run = """
import csv, json

scores = []
with open("{input}") as f:
    for row in csv.DictReader(f):
        scores.append(int(row["score"]))

stats = {
    "student": "{wildcards.student}",
    "mean": sum(scores) / len(scores),
    "min": min(scores),
    "max": max(scores),
    "count": len(scores),
}

with open("{output}", "w") as f:
    json.dump(stats, f, indent=2)
"""

# Rule 2: Combine all student stats into a summary
[rule.summary]
input = ["results/{student}_stats.json"]
output = ["results/summary.json"]
lang = "python"
run = """
import json, glob

all_stats = []
for path in sorted(glob.glob("results/*_stats.json")):
    with open(path) as f:
        all_stats.append(json.load(f))

with open("{output}", "w") as f:
    json.dump(all_stats, f, indent=2)
"""

# Rule 3: Default target -- build the summary
[rule.all]
input = ["results/summary.json"]
```

This workflow has three rules:

1. **stats** -- computes per-student statistics (runs once per student)
2. **summary** -- combines all stats into one file
3. **all** -- an aggregation target that tells OxyMake what to build

> **Interpolation note.** Inside `run`/`shell` blocks, OxyMake substitutes
> the placeholders it recognizes -- `{input}`, `{output}`, `{wildcards.X}`,
> `{config.X}`, and so on -- and leaves everything else untouched. It does
> **not** treat `{{`/`}}` as escaped braces, so write ordinary Python dict
> literals with single braces (`stats = { ... }`). The recognized
> placeholders are listed in the [Expression Language](../reference/expressions.md)
> reference.

## Step 4: Plan

Before running, see what OxyMake will do:

```bash
ox plan
```

You should see something like:

```
Plan: 3 rules, 3 jobs, 2 source files
Targets: results/summary.json
  1. [stats-bob] rule=stats -> [results/bob_stats.json]
  2. [stats-alice] rule=stats -> [results/alice_stats.json]
  3. [summary] rule=summary -> [results/summary.json]
```

OxyMake resolved the `{student}` wildcard from `config.students` and
created two concrete jobs for the `stats` rule (with the ids `stats-alice`
and `stats-bob`), plus one for `summary`.

## Step 5: Run

```bash
ox run
```

Output (timings will vary):

```
  Resolving 3 jobs (3 to run, 0 cached)
  ▸ summary — upstream rebuilt
  ✓ Completed 3/3 in 0.6s (4.8 jobs/s)
    3 succeeded
Completed: 3 succeeded, 0 failed, 0 skipped, 0 cancelled (0.6s)
```

The last line is the canonical summary: `N succeeded, N failed, N skipped,
N cancelled`. A run is successful when `failed` and `cancelled` are both `0`.

Check the results:

```bash
cat results/alice_stats.json
```

```json
{
  "student": "alice",
  "mean": 85.0,
  "min": 78,
  "max": 92,
  "count": 3
}
```

## Step 6: See Caching in Action

Run the same command again:

```bash
ox run
```

Output:

```
Cache: 3 of 3 job(s) up-to-date, skipping.
Completed: 0 succeeded, 0 failed, 3 skipped, 0 cancelled (0.0s)
```

Nothing ran. OxyMake detected that all inputs are unchanged and all
outputs exist with the correct content hashes, so all three jobs are
reported as `skipped`.

Now modify one input:

```bash
echo "Alice,99" >> data/alice.csv
ox run
```

Output:

```
Cache: 1 of 3 job(s) up-to-date, skipping.
  Resolving 3 jobs (2 to run, 1 cached)
  [1/3] ✓ stats-bob [cached]
  ▸ summary — upstream rebuilt
  ✓ Completed 3/3 in 0.4s (7.5 jobs/s)
    2 succeeded, 1 skipped
Completed: 2 succeeded, 0 failed, 1 skipped, 0 cancelled (0.4s)
```

Only `stats-alice` and `summary` re-ran. `stats-bob` was cached (reported
as `skipped`) because its input did not change.

## Step 7: Add a New Student

Edit `Oxymakefile.toml` and add a student:

```toml
[config]
students = ["alice", "bob", "charlie"]
```

Create the data file:

```bash
echo "name,score
Charlie,76
Charlie,82
Charlie,90" > data/charlie.csv
```

Run again:

```bash
ox run
```

```
Cache: 2 of 4 job(s) up-to-date, skipping.
  Resolving 4 jobs (2 to run, 2 cached)
  [1/4] ✓ stats-alice [cached]
  [2/4] ✓ stats-bob [cached]
  ▸ summary — upstream rebuilt
  ✓ Completed 4/4 in 0.4s (10.5 jobs/s)
    2 succeeded, 2 skipped
Completed: 2 succeeded, 0 failed, 2 skipped, 0 cancelled (0.4s)
```

Only the new student was computed. Alice and Bob's stats were cached
(reported as `skipped`).

## What You Learned

1. **Rules declare intent** -- input/output patterns with wildcards
2. **Config drives expansion** -- `students = [...]` determines which
   jobs are created
3. **Content-addressable caching** -- unchanged inputs mean cached outputs
4. **Incremental execution** -- adding data or rules only computes what
   is new
5. **Backward chaining** -- OxyMake figures out the dependency order
   automatically

## Next Steps

- [The Three Graphs](../concepts/three-graphs.md) -- understand how
  OxyMake resolves your workflow
- [Content-Addressable Cache](../concepts/cache.md) -- why caching works
  correctly
- [Execution Modes](../concepts/execution-modes.md) -- the four ways to
  execute a rule
