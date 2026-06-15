# Climate Time-Series Pipeline

This cookbook builds a multi-station climate analysis pipeline in OxyMake. It
covers feature engineering, index generation, and regional aggregation across a
network of weather stations -- all driven by wildcards, snapshots, and execution
history. Mock data (random readings) keeps the example self-contained.

## What You Will Learn

- Config-driven station network and parameter sweeps
- Wildcard expansion across stations, features, and rolling windows
- Named inputs for multi-file rules
- Snapshots to compare analysis milestones
- Execution history as a lightweight lab notebook
- Tag-based filtering for fast iteration

## The Complete Oxymakefile

Create a directory and save this as `Oxymakefile.toml`:

```toml
ox_version = "0.1"

[config]
stations = ["BOS", "DEN", "SEA", "AUS", "PDX"]
windows  = [5, 10, 20, 60]
metric   = ["trend", "anomaly"]

# ── Default target ──────────────────────────────────────────────
[rule.all]
input = ["reports/network_summary.txt"]

# ── Stage 1: Generate mock temperature readings ────────────────
[rule.mock_readings]
output = ["data/readings/{station}.csv"]
tags   = { stage = "data", speed = "fast" }
shell  = """
mkdir -p data/readings
echo "date,temp" > {output}
temp=15
for day in $(seq 1 252); do
  # Random daily temperature delta between -3 and +3 degrees
  d=$(awk "BEGIN {{srand($day * 17 + $(echo {station} | cksum | cut -d' ' -f1)); printf \"%.4f\", (rand() - 0.5) * 6}}")
  temp=$(awk "BEGIN {{printf \"%.2f\", $temp + $d}}")
  printf "2025-%03d,%s\\n" "$day" "$temp" >> {output}
done
"""

# ── Stage 2: Compute features ─────────────────────────────────
[rule.features]
input  = { readings = "data/readings/{station}.csv" }
output = ["data/features/{station}_{window}d.csv"]
tags   = { stage = "features", speed = "fast" }
shell  = """
mkdir -p data/features
echo "date,{station}_trend_{window}d,{station}_var_{window}d" > {output}
tail -n +2 {input.readings} | awk -F, -v lb={window} '
  BEGIN {{ OFS="," }}
  {{
    temps[NR] = $2
    if (NR >= lb) {{
      trend = (temps[NR] - temps[NR - lb + 1]) / lb
      sum = 0; sq = 0
      for (i = NR - lb + 1; i <= NR; i++) {{
        r = temps[i] - temps[i-1]
        sum += r; sq += r * r
      }}
      var = (sq - sum*sum/lb) / (lb - 1)
      printf "%s,%.6f,%.6f\\n", $1, trend, var
    }}
  }}
' >> {output}
"""

# ── Stage 3: Generate indices ─────────────────────────────────
[rule.indices]
input  = ["data/features/{station}_{window}d.csv"]
output = ["data/indices/{station}_{metric}.csv"]
tags   = { stage = "indices" }
shell  = """
mkdir -p data/indices
echo "date,{station}_{metric}" > {output}

if [ "{metric}" = "trend" ]; then
  # Average trend across windows → warming vs cooling stations
  paste -d, data/features/{station}_*d.csv \
    | tail -n +2 \
    | awk -F, '{{ sum=0; n=0; for(i=2;i<=NF;i+=2){{ sum+=$i; n++ }}; if(n>0) printf "%s,%.6f\\n",$1,sum/n }}' \
    >> {output}
else
  # Anomaly: deviation from the mean trend
  paste -d, data/features/{station}_*d.csv \
    | tail -n +2 \
    | awk -F, '{{ sum=0; n=0; for(i=2;i<=NF;i+=2){{ sum+=$i; n++ }}; if(n>0) printf "%s,%.6f\\n",$1,-sum/n }}' \
    >> {output}
fi
"""

# ── Stage 4: Cross-station composite index ────────────────────
[rule.composite]
input  = ["data/indices/{station}_{metric}.csv"]
output = ["data/composite/{metric}_index.csv"]
tags   = { stage = "composite" }
shell  = """
mkdir -p data/composite
echo "date,station,weight" > {output}
# Rank-based regional index: center station values cross-sectionally to zero
paste -d, data/indices/*_{metric}.csv \
  | tail -n +2 \
  | awk -F, '
    BEGIN {{ split("{station}", stations, " ") }}
    {{
      n = 0; sum = 0
      for (i = 2; i <= NF; i += 2) {{ vals[++n] = $i; sum += $i }}
      mean = sum / n
      wsum = 0
      for (i = 1; i <= n; i++) {{ w[i] = vals[i] - mean; wsum += (w[i]>0?w[i]:-w[i]) }}
      if (wsum > 0) for (i = 1; i <= n; i++) w[i] /= wsum
      for (i = 1; i <= n; i++) printf "%s,%s,%.6f\\n", $1, stations[i], w[i]
    }}
  ' >> {output}
"""

# ── Stage 5: Cumulative index score ──────────────────────────
[rule.score]
input  = {
  weights  = "data/composite/{metric}_index.csv",
  readings = "data/readings/{station}.csv"
}
output = ["data/score/{metric}_score.csv"]
tags   = { stage = "score" }
shell  = """
mkdir -p data/score
echo "date,daily_index,cumulative_index" > {output}
# Simple: weight * daily reading, summed across stations
awk -F, '
  NR == FNR && FNR > 1 {{ weights[$1,$2] = $3; next }}
  FNR > 1 {{ readings[$1] = $2 }}
' {input.weights} data/readings/*.csv

# Simplified: accumulate a weighted daily index
tail -n +2 {input.weights} | awk -F, '
  {{ idx[$1] += $3 * (rand() - 0.48) * 0.02 }}
  END {{
    cum = 0
    n = asorti(idx, dates)
    for (i = 1; i <= n; i++) {{
      cum += idx[dates[i]]
      printf "%s,%.6f,%.6f\\n", dates[i], idx[dates[i]], cum
    }}
  }}
' >> {output}
"""

# ── Stage 6: Summary report ──────────────────────────────────
[rule.report]
input  = ["data/score/{metric}_score.csv"]
output = ["reports/network_summary.txt"]
tags   = { stage = "report", speed = "fast" }
shell  = """
mkdir -p reports
echo "======================================" > {output}
echo "  Climate Network Pipeline — Summary"    >> {output}
echo "======================================" >> {output}
echo ""                                        >> {output}
echo "Network: {station}"                      >> {output}
echo "Windows: {window}"                        >> {output}
echo "Metrics: {metric}"                        >> {output}
echo ""                                         >> {output}
for f in {input}; do
  index=$(basename "$f" _score.csv)
  lines=$(tail -n +2 "$f" | wc -l | tr -d ' ')
  final=$(tail -1 "$f" | cut -d, -f3)
  echo "Index: $index"                          >> {output}
  echo "  Observation days: $lines"             >> {output}
  echo "  Final cumulative index: $final"       >> {output}
  echo ""                                       >> {output}
done
echo "--- Pipeline complete ---"               >> {output}
"""
```

## Create the Project

```bash
mkdir climate-pipeline && cd climate-pipeline
# Save the Oxymakefile.toml above
```

No input data files are needed -- `mock_readings` generates synthetic data.

## Explore the DAG

```bash
ox plan
```

```
Plan: 6 rules, 42 jobs, 5 source files
Targets: reports/network_summary.txt
  1. [mock_readings-BOS] rule=mock_readings -> [data/readings/BOS.csv]
  2. [features-BOS-5d] rule=features -> [data/features/BOS_5d.csv]
  3. [features-DEN-5d] rule=features -> [data/features/DEN_5d.csv]
  ...
  40. [composite-trend] rule=composite -> [data/composite/trend.csv]
  41. [score-trend] rule=score -> [data/scores/trend.csv]
  42. [report] rule=report -> [reports/network_summary.txt]
```

The DAG fans out across stations and windows, then converges through indices
and regional aggregation into a single report.

## Run the Full Pipeline

```bash
ox run -j 4
```

OxyMake runs up to 4 jobs in parallel. The `mock_readings` jobs run first (no
dependencies), then `features` fans out across stations x windows, and
everything converges into the network report.

## Iterate on a Single Station

During development, focus on one station by requesting its leaf target
(wildcards in the target select the matching jobs):

```bash
ox run "data/indices/BOS_*.csv"
```

This builds the pipeline for BOS only. Later, run the full network:

```bash
ox run
```

BOS is cached. Only the remaining stations are computed.

## Filter by Rule

Run only the feature computation stage with `--rule` (exact name or
`/regex/`):

```bash
ox run --rule features
```

## Snapshots: Compare Analysis Milestones

After a successful run, save a snapshot:

```bash
ox snapshot create baseline --message "5-station trend + anomaly"
```

Now add a new window (120 days) and a new metric. Edit the config:

```toml
[config]
windows = [5, 10, 20, 60, 120]
metric  = ["trend", "anomaly", "seasonal"]
```

Run again and save another snapshot:

```bash
ox run -j 4
ox snapshot create v2 --message "Added 120d window + seasonal metric"
```

Compare the two milestones:

```bash
ox snapshot diff baseline v2
```

```
Workflow hash changed (config modified)
Added:    15 jobs (features/*_120d, indices/*_seasonal, ...)
Changed:  2 jobs (composite, report — new inputs)
Unchanged: 40 jobs
```

This tells you exactly what changed between analysis iterations without
manually tracking file modifications.

## Execution History as a Lab Notebook

Each `ox run` is recorded with timing, job counts, and optional notes:

```bash
ox run -j 4 --note "Baseline: 5 stations, 4 windows"
# ... iterate ...
ox run -j 4 --note "Added seasonal metric, 120d window"
```

Review your analysis timeline:

```bash
ox history
```

```
RUN          STARTED              DURATION   OK  FAIL  SKIP  NOTE
run-a1b2c3   2025-01-15 09:12     12.3s     42    0     0   Baseline: 5 stations, 4 windows
run-d4e5f6   2025-01-15 09:45      4.1s     15    0    40   Added seasonal metric, 120d window
```

Drill into a specific run:

```bash
ox history --run-id run-a1b2c3
```

This shows per-job wall time, peak memory, and exit codes -- useful for
identifying bottlenecks as your network grows.

## Scaling the Network

Add more stations by editing `[config]`:

```toml
[config]
stations = ["BOS", "DEN", "SEA", "AUS", "PDX", "ORD", "ATL", "LAX", "JFK", "MIA"]
```

Run again:

```bash
ox run -j 8
```

Only the new stations are computed. Everything else is cached. As the network
grows from 5 to 50 to 500 stations, the same Oxymakefile works -- OxyMake
expands the wildcards and parallelizes automatically.

## Next Steps

- [Growing a Workflow Organically](organic-growth.md) -- evolving from 3 rules
  to 300+ over weeks of analysis
- [Agent-Driven Workflows](agent-workflows.md) -- automating the pipeline with
  LLM agents and NDJSON event streams
- [Rules and Wildcards](../concepts/rules-and-wildcards.md) -- wildcard expansion
  and constraints
- [Snapshots](../concepts/snapshots.md) -- saving and comparing workflow state
```
