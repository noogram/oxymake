#!/usr/bin/env bash
# Synthetic benchmark: measures per-job transport overhead
#
# Compares:
#   1. Disk-only (materialize=always, no memory budget)
#   2. Memory-budget (materialize=auto, --memory-budget)
#   3. Pure Python baseline (no ox, direct subprocess chain)
#
# Usage:
#   ./bench/synthetic/run_benchmark.sh [jobs] [size]
#   ./bench/synthetic/run_benchmark.sh 50 1M
#   ./bench/synthetic/run_benchmark.sh 20 10M

set -euo pipefail

JOBS="${1:-50}"
SIZE="${2:-1M}"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OXYMAKE_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"

cd "$OXYMAKE_DIR"

echo "=== Synthetic Benchmark ==="
echo "  Jobs: $JOBS"
echo "  Size per output: $SIZE"
echo ""

# Generate Oxymakefiles
python3 bench/synthetic/generate.py --jobs "$JOBS" --size "$SIZE" --materialize always \
    > bench/synthetic/Oxymakefile-disk.toml
python3 bench/synthetic/generate.py --jobs "$JOBS" --size "$SIZE" --materialize auto \
    > bench/synthetic/Oxymakefile-memory.toml

# Clean up
rm -rf bench_output

# --- Test 1: Disk-only ---
echo "--- [1/3] Disk-only (materialize=always) ---"
rm -rf bench_output .oxymake/cache
time_start=$(python3 -c "import time; print(time.time())")
ox run -f bench/synthetic/Oxymakefile-disk.toml -j1 --no-cache 2>&1 | tail -3
time_end=$(python3 -c "import time; print(time.time())")
disk_time=$(python3 -c "print(f'{$time_end - $time_start:.2f}')")
echo "  Wall time: ${disk_time}s"
echo ""

# --- Test 2: Memory-budget ---
echo "--- [2/3] Memory-budget (materialize=auto, --memory-budget 512M) ---"
rm -rf bench_output .oxymake/cache
time_start=$(python3 -c "import time; print(time.time())")
ox run -f bench/synthetic/Oxymakefile-memory.toml -j1 --memory-budget 512M --no-cache 2>&1 | tail -3
time_end=$(python3 -c "import time; print(time.time())")
mem_time=$(python3 -c "print(f'{$time_end - $time_start:.2f}')")
echo "  Wall time: ${mem_time}s"
echo ""

# --- Test 3: Pure Python baseline (no ox overhead) ---
echo "--- [3/3] Pure Python baseline (no ox, direct chain) ---"
rm -rf bench_output && mkdir -p bench_output
# Create seed
dd if=/dev/urandom of=bench_output/step_000.bin bs="$SIZE" count=1 2>/dev/null

time_start=$(python3 -c "import time; print(time.time())")
for i in $(seq 1 $((JOBS - 1))); do
    prev=$(printf "bench_output/step_%03d.bin" $((i - 1)))
    curr=$(printf "bench_output/step_%03d.bin" $i)
    python3 bench/synthetic/identity.py "$prev" "$curr"
done
time_end=$(python3 -c "import time; print(time.time())")
py_time=$(python3 -c "print(f'{$time_end - $time_start:.2f}')")
echo "  Wall time: ${py_time}s"
echo ""

# --- Test 4: Pure cp baseline (no Python, no ox) ---
echo "--- [4/4] Pure cp baseline (cp only, no Python) ---"
rm -rf bench_output && mkdir -p bench_output
dd if=/dev/urandom of=bench_output/step_000.bin bs="$SIZE" count=1 2>/dev/null

time_start=$(python3 -c "import time; print(time.time())")
for i in $(seq 1 $((JOBS - 1))); do
    prev=$(printf "bench_output/step_%03d.bin" $((i - 1)))
    curr=$(printf "bench_output/step_%03d.bin" $i)
    cp "$prev" "$curr"
done
time_end=$(python3 -c "import time; print(time.time())")
cp_time=$(python3 -c "print(f'{$time_end - $time_start:.2f}')")
echo "  Wall time: ${cp_time}s"
echo ""

# --- Summary ---
echo "=== Results ==="
echo ""
per_job_disk=$(python3 -c "print(f'{$disk_time / $JOBS * 1000:.0f}')")
per_job_mem=$(python3 -c "print(f'{$mem_time / $JOBS * 1000:.0f}')")
per_job_py=$(python3 -c "print(f'{$py_time / ($JOBS - 1) * 1000:.0f}')")
per_job_cp=$(python3 -c "print(f'{$cp_time / ($JOBS - 1) * 1000:.0f}')")

echo "  | Config              | Total    | Per job  | Overhead vs cp |"
echo "  |---------------------|----------|----------|----------------|"
echo "  | cp baseline         | ${cp_time}s   | ${per_job_cp}ms     | —              |"
echo "  | Python subprocess   | ${py_time}s   | ${per_job_py}ms     | Python startup |"
echo "  | ox disk-only        | ${disk_time}s   | ${per_job_disk}ms     | + ox scheduler |"
echo "  | ox memory-budget    | ${mem_time}s   | ${per_job_mem}ms     | + memory layer |"
echo ""
echo "  Per-job overhead breakdown:"
echo "    cp (pure I/O):      ${per_job_cp}ms"
echo "    Python startup:     $(python3 -c "print(f'{$per_job_py - $per_job_cp:.0f}')")ms"
echo "    ox scheduler:       $(python3 -c "print(f'{$per_job_disk - $per_job_py:.0f}')")ms"
echo "    memory layer delta: $(python3 -c "print(f'{$per_job_mem - $per_job_disk:.0f}')")ms"

# Clean up
rm -rf bench_output
