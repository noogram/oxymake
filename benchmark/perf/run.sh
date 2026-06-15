#!/usr/bin/env bash
# Benchmark: DAG resolution performance at scale
#
# Measures `ox plan` (DAG resolution only, no execution) at 1K-50K jobs.
# Optionally compares against `snakemake --dryrun` if snakemake is installed.
#
# Usage:
#   bash benchmark/perf/run.sh              # run all sizes
#   bash benchmark/perf/run.sh 1000 5000    # run specific sizes
#
# Requirements:
#   - ox binary in PATH (cargo install --path .)
#   - Python 3 (for generation script)
#   - Optional: hyperfine (for statistical timing)
#   - Optional: snakemake (for comparison)
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OX="$(command -v ox)" || { echo "ERROR: 'ox' not found in PATH"; exit 1; }
RESULTS_FILE="${SCRIPT_DIR}/results.md"
WORKDIR="${SCRIPT_DIR}/workloads"
RUNS=5  # number of timing runs (when not using hyperfine)
DASHBOARD=false
DASHBOARD_PID=""

# --- Parse arguments ---
SIZES=""
for arg in "$@"; do
    case "$arg" in
        --dashboard)
            DASHBOARD=true
            ;;
        *)
            SIZES="${SIZES:+$SIZES }$arg"
            ;;
    esac
done
if [ -z "$SIZES" ]; then
    SIZES="1000 5000 10000 50000"
fi

# --- Cleanup handler ---
cleanup() {
    if [ -n "$DASHBOARD_PID" ] && kill -0 "$DASHBOARD_PID" 2>/dev/null; then
        echo ""
        echo "--- Stopping dashboard (PID $DASHBOARD_PID) ---"
        kill "$DASHBOARD_PID" 2>/dev/null || true
        wait "$DASHBOARD_PID" 2>/dev/null || true
    fi
}
trap cleanup EXIT

# --- Pre-flight checks ---
if [ ! -x "$OX" ]; then
    echo "ERROR: 'ox' binary not found or not executable"
    echo "       Install: cargo install --path ."
    exit 1
fi

HAS_HYPERFINE=false
if command -v hyperfine &>/dev/null; then
    HAS_HYPERFINE=true
fi

HAS_SNAKEMAKE=false
if command -v snakemake &>/dev/null; then
    HAS_SNAKEMAKE=true
fi

echo "============================================"
echo " Oxymake DAG Resolution Benchmark"
echo "============================================"
echo ""
echo "Binary:     $OX"
echo "Hyperfine:  $HAS_HYPERFINE"
echo "Snakemake:  $HAS_SNAKEMAKE"
echo "Dashboard:  $DASHBOARD"
echo "Sizes:      $SIZES"
echo "Runs:       $RUNS (used when hyperfine unavailable)"
echo ""

# --- Start dashboard in background (if requested) ---
if $DASHBOARD; then
    echo "--- Starting dashboard on port 9876 ---"
    "$OX" dashboard --port 9876 &>/dev/null &
    DASHBOARD_PID=$!
    echo "  Dashboard PID: $DASHBOARD_PID"
    echo "  URL: http://127.0.0.1:9876"
    echo ""
fi

# --- Generate workloads ---
echo "--- Generating synthetic workloads ---"
rm -rf "$WORKDIR"
python3 "${SCRIPT_DIR}/generate.py" --sizes "$(echo $SIZES | tr ' ' ',')" --outdir "$WORKDIR"
echo ""

# --- Timing helper (returns median of $RUNS runs in seconds) ---
time_command() {
    local cmd="$1"
    local workdir="$2"
    local label="$3"

    if $HAS_HYPERFINE; then
        # hyperfine outputs JSON with statistical summary
        local json
        json=$(hyperfine --runs "$RUNS" --warmup 1 --export-json /dev/stdout \
            --command-name "$label" \
            "cd '$workdir' && $cmd" 2>/dev/null)
        echo "$json" | python3 -c "
import sys, json
data = json.load(sys.stdin)
r = data['results'][0]
print(f\"{r['median']:.4f} {r['min']:.4f} {r['max']:.4f} {r['stddev']:.4f}\")
"
    else
        # Manual timing: collect $RUNS measurements, report median
        local times=()
        for _ in $(seq 1 $RUNS); do
            local start end elapsed
            start=$(python3 -c 'import time; print(time.perf_counter())')
            (cd "$workdir" && eval "$cmd") >/dev/null 2>&1
            end=$(python3 -c 'import time; print(time.perf_counter())')
            elapsed=$(python3 -c "print(f'{$end - $start:.4f}')")
            times+=("$elapsed")
        done
        # Sort and pick median
        local sorted
        sorted=$(printf '%s\n' "${times[@]}" | sort -n)
        local median min_t max_t
        median=$(echo "$sorted" | awk "NR==$(( (RUNS + 1) / 2 ))")
        min_t=$(echo "$sorted" | head -1)
        max_t=$(echo "$sorted" | tail -1)
        echo "$median $min_t $max_t 0.0000"
    fi
}

# --- Run benchmarks ---
declare -A OX_RESULTS
declare -A SM_RESULTS

for size in $SIZES; do
    dir="${WORKDIR}/scale_${size}"
    if [ ! -d "$dir" ]; then
        echo "WARN: $dir not found, skipping"
        continue
    fi

    echo "--- Scale: ${size} jobs ---"

    # Oxymake: ox plan (DAG resolution, no execution)
    echo -n "  ox plan ... "
    result=$(time_command "'$OX' plan -f Oxymakefile.toml" "$dir" "ox-plan-${size}")
    median=$(echo "$result" | awk '{print $1}')
    min_t=$(echo "$result" | awk '{print $2}')
    max_t=$(echo "$result" | awk '{print $3}')
    stddev=$(echo "$result" | awk '{print $4}')
    echo "${median}s (min=${min_t}s max=${max_t}s stddev=${stddev}s)"
    OX_RESULTS[$size]="$result"

    # Snakemake: dry-run (DAG resolution, no execution)
    if $HAS_SNAKEMAKE; then
        echo -n "  snakemake --dryrun ... "
        result=$(time_command "snakemake --dryrun --quiet --cores 1" "$dir" "snake-${size}")
        median=$(echo "$result" | awk '{print $1}')
        min_t=$(echo "$result" | awk '{print $2}')
        max_t=$(echo "$result" | awk '{print $3}')
        stddev=$(echo "$result" | awk '{print $4}')
        echo "${median}s (min=${min_t}s max=${max_t}s stddev=${stddev}s)"
        SM_RESULTS[$size]="$result"
    fi

    echo ""
done

# --- Generate results.md ---
echo "--- Writing results to $RESULTS_FILE ---"

{
    echo "# DAG Resolution Benchmark Results"
    echo ""
    echo "Generated: $(date -u '+%Y-%m-%d %H:%M:%S UTC')"
    echo ""
    echo "**Setup:**"
    echo "- Binary: \`oxymake\` (release build)"
    echo "- Timing: $(if $HAS_HYPERFINE; then echo "hyperfine ($RUNS runs + 1 warmup)"; else echo "manual ($RUNS runs, median)"; fi)"
    echo "- Metric: DAG resolution time (\`ox plan\` / \`snakemake --dryrun\`)"
    echo "- Platform: $(uname -s) $(uname -m), $(sysctl -n hw.ncpu 2>/dev/null || nproc 2>/dev/null || echo '?') cores"
    echo ""

    # Oxymake results table
    echo "## Oxymake (\`ox plan\`)"
    echo ""
    echo "| Jobs | Median (s) | Min (s) | Max (s) | Stddev (s) |"
    echo "|-----:|-----------:|--------:|--------:|-----------:|"
    for size in $SIZES; do
        if [ -n "${OX_RESULTS[$size]:-}" ]; then
            read -r median min_t max_t stddev <<< "${OX_RESULTS[$size]}"
            echo "| $size | $median | $min_t | $max_t | $stddev |"
        fi
    done
    echo ""

    # Snakemake results table (if available)
    if $HAS_SNAKEMAKE; then
        echo "## Snakemake (\`snakemake --dryrun\`)"
        echo ""
        echo "| Jobs | Median (s) | Min (s) | Max (s) | Stddev (s) |"
        echo "|-----:|-----------:|--------:|--------:|-----------:|"
        for size in $SIZES; do
            if [ -n "${SM_RESULTS[$size]:-}" ]; then
                read -r median min_t max_t stddev <<< "${SM_RESULTS[$size]}"
                echo "| $size | $median | $min_t | $max_t | $stddev |"
            fi
        done
        echo ""

        # Comparison table
        echo "## Comparison"
        echo ""
        echo "| Jobs | Oxymake (s) | Snakemake (s) | Speedup |"
        echo "|-----:|------------:|--------------:|--------:|"
        for size in $SIZES; do
            if [ -n "${OX_RESULTS[$size]:-}" ] && [ -n "${SM_RESULTS[$size]:-}" ]; then
                ox_median=$(echo "${OX_RESULTS[$size]}" | awk '{print $1}')
                sm_median=$(echo "${SM_RESULTS[$size]}" | awk '{print $1}')
                speedup=$(python3 -c "
ox = float('$ox_median')
sm = float('$sm_median')
if ox > 0:
    print(f'{sm/ox:.1f}x')
else:
    print('inf')
")
                echo "| $size | $ox_median | $sm_median | $speedup |"
            fi
        done
        echo ""
    else
        echo "## Snakemake Comparison"
        echo ""
        echo "_Snakemake not installed. Install with \`pip install snakemake\` for comparison._"
        echo ""
    fi

    echo "## Reproduction"
    echo ""
    echo '```bash'
    echo "# Install ox (if not already installed)"
    echo "cargo install --path ."
    echo ""
    echo "# Run full benchmark"
    echo "just -f benchmark/Justfile benchmark"
    echo ""
    echo "# Run specific sizes"
    echo "just -f benchmark/Justfile benchmark 1000 5000"
    echo '```'
} > "$RESULTS_FILE"

# --- Save timestamped copy ---
TIMESTAMP=$(date -u '+%Y%m%d-%H%M%S')
TIMESTAMPED_FILE="${SCRIPT_DIR}/results-${TIMESTAMP}.md"
cp "$RESULTS_FILE" "$TIMESTAMPED_FILE"

echo ""
echo "Done. Results written to:"
echo "  Latest:     $RESULTS_FILE"
echo "  Archived:   $TIMESTAMPED_FILE"
echo ""
cat "$RESULTS_FILE"
