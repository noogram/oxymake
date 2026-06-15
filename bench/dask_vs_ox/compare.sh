#!/usr/bin/env bash
# OxyMake vs Dask benchmark comparison
#
# Usage:
#   ./bench/dask_vs_ox/compare.sh          # Full suite
#   ./bench/dask_vs_ox/compare.sh --quick   # N=10 only
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OXYMAKE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
cd "$OXYMAKE_DIR"

QUICK=false
[[ "${1:-}" == "--quick" ]] && QUICK=true

NCPU=$(python3 -c "import os; print(os.cpu_count())")
RESULTS_DIR="bench_output"

echo "=== OxyMake vs Dask Benchmark ==="
echo "  CPUs: $NCPU"
echo "  Quick: $QUICK"
echo ""

# Helper: create N chunks in $RESULTS_DIR/chunks/
create_chunks() {
    local n=$1
    mkdir -p "$RESULTS_DIR/chunks"
    python3 -c "
import numpy as np, os
for i in range($n):
    path = '$RESULTS_DIR/chunks/chunk_{:04d}.npy'.format(i)
    if not os.path.exists(path):
        np.save(path, np.random.randn(1250, 1000))
"
}

# Helper: time a command, return seconds
time_cmd() {
    local start end
    start=$(python3 -c "import time; print(time.time())")
    eval "$@" > /dev/null 2>&1
    end=$(python3 -c "import time; print(time.time())")
    python3 -c "print(f'{$end - $start:.3f}')"
}

# Scales to test
if $QUICK; then
    A_SCALES=(10)
    B_SCALES=(10)
    C_SCALES=(16)
else
    A_SCALES=(10 100 500)
    B_SCALES=(10 100 500)
    C_SCALES=(16 64 256)
fi

echo "================================================================"
echo "  Workload A: Embarrassingly Parallel (N independent map tasks)"
echo "================================================================"
echo ""
printf "| %-6s | %-12s | %-12s | %-14s | %-14s |\n" "N" "Dask sync" "Dask threads" "OxMake sh j=$NCPU" "OxMake sh j=1"
printf "| %-6s | %-12s | %-12s | %-14s | %-14s |\n" "------" "------------" "------------" "--------------" "--------------"

for N in "${A_SCALES[@]}"; do
    create_chunks "$N"

    # Dask synchronous
    d_sync=$(cd "$SCRIPT_DIR" && python3 dask_benchmark.py --workload a --n "$N" --scheduler synchronous --repeats 1 | tail -1 | python3 -c "import json,sys; print(json.load(sys.stdin).get('median_wall_s', json.load(open('/dev/stdin')).get('wall_s', '?')))" 2>/dev/null || echo "?")
    # Simpler: just time it
    d_sync=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload a --n $N --scheduler synchronous --repeats 1")

    # Dask threads
    d_thr=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload a --n $N --scheduler threads --repeats 1")

    # OxyMake shell j=N
    python3 bench/dask_vs_ox/generate_oxymakefile.py --workload a --n "$N" --mode shell --results-dir "$RESULTS_DIR" > /tmp/ox_bench_a.toml
    rm -rf "$RESULTS_DIR"/result_*.npy .oxymake/cache
    ox_par=$(time_cmd "ox run -f /tmp/ox_bench_a.toml -j$NCPU --no-cache")

    # OxyMake shell j=1
    rm -rf "$RESULTS_DIR"/result_*.npy .oxymake/cache
    ox_seq=$(time_cmd "ox run -f /tmp/ox_bench_a.toml -j1 --no-cache")

    printf "| %-6s | %-12s | %-12s | %-14s | %-14s |\n" "$N" "${d_sync}s" "${d_thr}s" "${ox_par}s" "${ox_seq}s"
done

echo ""
echo "================================================================"
echo "  Workload B: Linear Chain (N sequential tasks)"
echo "================================================================"
echo ""
printf "| %-6s | %-12s | %-12s | %-14s |\n" "N" "Dask sync" "Dask threads" "OxMake sh j=1"
printf "| %-6s | %-12s | %-12s | %-14s |\n" "------" "------------" "------------" "--------------"

for N in "${B_SCALES[@]}"; do
    create_chunks 1  # only need 1 seed chunk

    d_sync=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload b --n $N --scheduler synchronous --repeats 1")
    d_thr=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload b --n $N --scheduler threads --repeats 1")

    python3 bench/dask_vs_ox/generate_oxymakefile.py --workload b --n "$N" --mode shell --results-dir "$RESULTS_DIR" > /tmp/ox_bench_b.toml
    rm -rf "$RESULTS_DIR"/step_*.npy .oxymake/cache
    ox_seq=$(time_cmd "ox run -f /tmp/ox_bench_b.toml -j1 --no-cache")

    printf "| %-6s | %-12s | %-12s | %-14s |\n" "$N" "${d_sync}s" "${d_thr}s" "${ox_seq}s"
done

echo ""
echo "================================================================"
echo "  Workload C: Tree Reduction (N leaves → 1 root)"
echo "================================================================"
echo ""
printf "| %-6s | %-12s | %-12s | %-14s |\n" "N" "Dask sync" "Dask threads" "OxMake sh j=$NCPU"
printf "| %-6s | %-12s | %-12s | %-14s |\n" "------" "------------" "------------" "--------------"

for N in "${C_SCALES[@]}"; do
    create_chunks "$N"

    d_sync=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload c --n $N --scheduler synchronous --repeats 1")
    d_thr=$(cd "$SCRIPT_DIR" && time_cmd "python3 dask_benchmark.py --workload c --n $N --scheduler threads --repeats 1")

    python3 bench/dask_vs_ox/generate_oxymakefile.py --workload c --n "$N" --mode shell --results-dir "$RESULTS_DIR" > /tmp/ox_bench_c.toml
    rm -rf "$RESULTS_DIR"/leaf_*.npy "$RESULTS_DIR"/reduce_*.npy .oxymake/cache
    ox_par=$(time_cmd "ox run -f /tmp/ox_bench_c.toml -j$NCPU --no-cache")

    printf "| %-6s | %-12s | %-12s | %-14s |\n" "$N" "${d_sync}s" "${d_thr}s" "${ox_par}s"
done

echo ""
echo "Done."
rm -rf "$RESULTS_DIR"
