#!/usr/bin/env bash
# A/B comparison: disk-only vs memory-budget execution
#
# Usage:
#   ./bench/compare.sh <oxymakefile> [runs] [budget]
#
# Example:
#   ./bench/compare.sh /path/to/your-pipeline/Oxymakefile.toml 5 1G
#
# Outputs a comparison table with wall times and cache hit rates.

set -euo pipefail

OXYMAKEFILE="${1:?Usage: compare.sh <oxymakefile> [runs] [budget]}"
RUNS="${2:-5}"
BUDGET="${3:-1G}"

echo "=== A/B Comparison ==="
echo "  Oxymakefile: $OXYMAKEFILE"
echo "  Runs: $RUNS per config"
echo "  Budget: $BUDGET"
echo ""

# Helper: run ox and extract wall time from output.
run_ox() {
    local label="$1"
    shift
    local start end elapsed
    start=$(date +%s%N)
    ox run -f "$OXYMAKEFILE" -j4 --no-cache "$@" 2>&1 | tail -3
    end=$(date +%s%N)
    elapsed=$(( (end - start) / 1000000 )) # milliseconds
    echo "$elapsed"
}

echo "--- Baseline (disk-only) ---"
disk_times=()
for i in $(seq 1 "$RUNS"); do
    ms=$(run_ox "disk-$i" --memory-budget 0)
    disk_times+=("$ms")
    echo "  Run $i: ${ms}ms"
done

echo ""
echo "--- Treatment (memory-budget $BUDGET) ---"
mem_times=()
for i in $(seq 1 "$RUNS"); do
    ms=$(run_ox "mem-$i" --memory-budget "$BUDGET")
    mem_times+=("$ms")
    echo "  Run $i: ${ms}ms"
done

echo ""
echo "=== Results ==="

# Compute median (sorted middle element).
median() {
    local sorted
    sorted=($(printf '%s\n' "$@" | sort -n))
    local n=${#sorted[@]}
    echo "${sorted[$((n/2))]}"
}

disk_median=$(median "${disk_times[@]}")
mem_median=$(median "${mem_times[@]}")

if [ "$disk_median" -gt 0 ]; then
    speedup=$(echo "scale=2; $disk_median / $mem_median" | bc 2>/dev/null || echo "N/A")
else
    speedup="N/A"
fi

echo "  Disk-only median:     ${disk_median}ms"
echo "  Memory-budget median: ${mem_median}ms"
echo "  Speedup: ${speedup}x"
