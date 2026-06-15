#!/usr/bin/env bash
# Head-to-head bench: OxyMake vs Snakemake at ≥ 10⁴ jobs.
#
# Measures:
#   - DAG resolution (cold / warm)
#   - Job submission throughput
#   - End-to-end wall time (cold / warm cache)
#   - Peak RSS
#   - Cache decision correctness
#
# Single-command reproducer:
#   bash bench/snakemake-vs-oxymake/run.sh
#
# Defaults: scales = 100, 1000, 10000; runs per measurement = 3.
# Override:
#   SIZES="100 1000 10000" RUNS=5 bash run.sh

set -euo pipefail

# ── Resolve paths ──────────────────────────────────────────────────────────
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORKDIR="${WORKDIR:-${SCRIPT_DIR}/workloads}"
RESULTS_DIR="${RESULTS_DIR:-${SCRIPT_DIR}/data}"
RESULTS_FILE="${SCRIPT_DIR}/RESULTS.md"
PYTHON_BIN="${PYTHON_BIN:-python3}"

# ── Parameters ─────────────────────────────────────────────────────────────
SIZES="${SIZES:-100 1000 10000}"
RUNS="${RUNS:-3}"
JOBS="${JOBS:-16}"      # parallelism for ox run / snakemake -j
CORES_LABEL="${CORES_LABEL:-$JOBS}"

# ── Binaries ───────────────────────────────────────────────────────────────
OX="${OX:-$(command -v ox || true)}"
SNAKEMAKE="${SNAKEMAKE:-$(command -v snakemake || true)}"
# hyperfine gives statistically-robust sub-process timing WITHOUT the
# python/​/usr/bin/time/sh wrapper overhead that `measured_run` carries. That
# wrapper adds a fixed ~50–250 ms floor (two interpreter spawns), which is
# negligible for minutes-scale end-to-end runs but DOMINATES — and badly
# distorts the speedup ratio of — the sub-100 ms DAG-resolution phase. So the
# resolution phase uses hyperfine when present (see `clean_resolve`).
HYPERFINE="${HYPERFINE:-$(command -v hyperfine || true)}"

if [[ -z "$OX" ]]; then
    echo "ERROR: 'ox' not found. Install with: cargo install --path ." >&2
    exit 1
fi
if [[ -z "$SNAKEMAKE" ]]; then
    echo "ERROR: 'snakemake' not found. Install with: pip install snakemake" >&2
    exit 1
fi

# ── Platform detection for `time` (RSS units differ across BSD/GNU) ─────────
OS_NAME="$(uname -s)"
HW_INFO="$(uname -srm)"
if [[ "$OS_NAME" == "Darwin" ]]; then
    CORES="$(sysctl -n hw.ncpu)"
    CPU_BRAND="$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo 'unknown')"
    MEM_TOTAL_KB="$(($(sysctl -n hw.memsize) / 1024))"
    TIME_BIN="/usr/bin/time"
    TIME_OPT="-l"
    # macOS /usr/bin/time -l reports "maximum resident set size" in bytes.
    RSS_REGEX='([0-9]+)[[:space:]]+maximum resident set size'
    RSS_UNIT_DIV=1024  # bytes → KiB
else
    CORES="$(nproc 2>/dev/null || echo 1)"
    CPU_BRAND="$(grep 'model name' /proc/cpuinfo 2>/dev/null | head -1 | sed 's/^.*: //' || echo 'unknown')"
    MEM_TOTAL_KB="$(grep MemTotal /proc/meminfo 2>/dev/null | awk '{print $2}')"
    TIME_BIN="/usr/bin/time"
    TIME_OPT="-v"
    # GNU /usr/bin/time -v reports "Maximum resident set size (kbytes)".
    RSS_REGEX='Maximum resident set size \(kbytes\):[[:space:]]+([0-9]+)'
    RSS_UNIT_DIV=1  # already KiB
fi

# ── Pre-flight ─────────────────────────────────────────────────────────────
mkdir -p "$RESULTS_DIR"
rm -rf "$WORKDIR"
mkdir -p "$WORKDIR"

# bench_lib.py lives one directory above scale_N/. Copy (not symlink) so the
# git-checkout / mtime-churn scenario can `touch` it without mutating the
# repo's real source file.
cp "$SCRIPT_DIR/bench_lib.py" "$WORKDIR/bench_lib.py"

echo "============================================================"
echo "  OxyMake vs Snakemake — head-to-head bench"
echo "============================================================"
echo "  ox        : $OX ($($OX --version))"
echo "  snakemake : $SNAKEMAKE ($($SNAKEMAKE --version))"
echo "  python    : $($PYTHON_BIN --version)"
echo "  OS        : $HW_INFO"
echo "  CPU       : $CPU_BRAND"
echo "  Cores     : $CORES (using -j $JOBS)"
echo "  Memory    : $(($MEM_TOTAL_KB / 1024 / 1024)) GiB"
echo "  Sizes     : $SIZES"
echo "  Runs      : $RUNS per measurement"
echo "============================================================"

# ── Generate workloads ─────────────────────────────────────────────────────
echo ""
echo "--- Generating workloads ---"
"$PYTHON_BIN" "$SCRIPT_DIR/generate.py" \
    --sizes "$(echo "$SIZES" | tr ' ' ',')" \
    --outdir "$WORKDIR" \
    --bench-lib "../bench_lib.py"

# ── Timing helpers ─────────────────────────────────────────────────────────
# `measured_run <cmd> <cwd>` → echoes "wall_secs peak_rss_kib" (RSS may be empty).
measured_run() {
    local cmd="$1"
    local cwd="$2"
    local stats_file
    stats_file="$(mktemp)"

    local t0 t1 wall
    t0="$($PYTHON_BIN -c 'import time; print(f"{time.perf_counter():.6f}")')"
    # /usr/bin/time prints stats on stderr; capture to a file, drop stdout.
    ( cd "$cwd" && "$TIME_BIN" "$TIME_OPT" sh -c "$cmd" 2>"$stats_file" >/dev/null )
    t1="$($PYTHON_BIN -c 'import time; print(f"{time.perf_counter():.6f}")')"
    wall="$($PYTHON_BIN -c "print(f'{$t1 - $t0:.4f}')")"

    local rss_raw=""
    if [[ -s "$stats_file" ]]; then
        rss_raw="$(grep -Eo "$RSS_REGEX" "$stats_file" | grep -Eo '[0-9]+' | head -1 || true)"
    fi
    local rss_kib=""
    if [[ -n "$rss_raw" ]]; then
        rss_kib="$(echo "$rss_raw $RSS_UNIT_DIV" | awk '{ printf("%.0f", $1 / $2) }')"
    fi
    rm -f "$stats_file"
    echo "$wall $rss_kib"
}

# `median_of_runs <cmd> <cwd> <runs>` → echoes "median_wall median_rss_kib"
median_of_runs() {
    local cmd="$1"
    local cwd="$2"
    local n_runs="$3"
    local walls=() rss_vals=()
    for _ in $(seq 1 "$n_runs"); do
        # shellcheck disable=SC2046
        read -r w r <<< "$(measured_run "$cmd" "$cwd")"
        walls+=("$w")
        if [[ -n "$r" ]]; then rss_vals+=("$r"); fi
    done
    local median_wall median_rss
    median_wall=$(printf '%s\n' "${walls[@]}" | sort -n | awk "NR==$(( (n_runs + 1) / 2 ))")
    if (( ${#rss_vals[@]} > 0 )); then
        median_rss=$(printf '%s\n' "${rss_vals[@]}" | sort -n | awk "NR==$(( (${#rss_vals[@]} + 1) / 2 ))")
    else
        median_rss=""
    fi
    echo "$median_wall $median_rss"
}

# `clean_resolve <cmd> <cwd> <runs>` → echoes "median_wall_secs ''" (no RSS).
# DAG resolution is a sub-100 ms phase, so it must NOT be timed through
# `measured_run` (whose two python spawns + /usr/bin/time + sh add a ~50–250 ms
# floor that swamps the signal and compresses the OxyMake-vs-Snakemake ratio).
# hyperfine times the bare command with statistical warmup; the fallback is a
# single-process bash-builtin timer (still wrapper-free), never measured_run.
clean_resolve() {
    local cmd="$1" cwd="$2" runs="$3"
    if [[ -n "$HYPERFINE" ]]; then
        local warm=2; (( runs < 5 )) && warm=1
        local minr=$(( runs < 5 ? 5 : runs ))
        local json; json="$(mktemp)"
        ( cd "$cwd" && "$HYPERFINE" -N --warmup "$warm" --min-runs "$minr" \
            --export-json "$json" "$cmd" >/dev/null 2>&1 ) || true
        local med
        med="$("$PYTHON_BIN" -c "import json; print(f\"{json.load(open('$json'))['results'][0]['median']:.4f}\")" 2>/dev/null || echo "")"
        rm -f "$json"
        echo "$med "
    else
        local walls=() t
        for _ in $(seq 1 "$runs"); do
            t="$( ( cd "$cwd" && TIMEFORMAT='%R'; time eval "$cmd" >/dev/null 2>&1 ) 2>&1 )"
            walls+=("$t")
        done
        local med
        med=$(printf '%s\n' "${walls[@]}" | sort -n | awk "NR==$(( (runs + 1) / 2 ))")
        echo "$med "
    fi
}

# Clean cache/outputs between cold runs.
clean_workload() {
    local d="$1"
    # OxyMake state.
    rm -rf "$d/.oxymake" "$d/seed.txt" "$d/step1" "$d/step2" "$d/step3" "$d/merged.txt"
    # Snakemake state.
    rm -rf "$d/.snakemake"
}

# ── Run the matrix ─────────────────────────────────────────────────────────
echo ""
echo "--- Running benchmark matrix ---"
echo ""

DATA_FILE="$RESULTS_DIR/measurements.tsv"
echo -e "size\tsystem\tphase\tcache\twall_s\tpeak_rss_kib" > "$DATA_FILE"

emit() {
    # size system phase cache wall rss
    local sz="$1" sys="$2" phase="$3" cache="$4" wall="$5" rss="${6:-}"
    echo -e "${sz}\t${sys}\t${phase}\t${cache}\t${wall}\t${rss}" >> "$DATA_FILE"
    printf "  %5s | %-10s | %-12s | %-4s | %8ss | %10s KiB\n" \
        "$sz" "$sys" "$phase" "$cache" "$wall" "${rss:--}"
}

CACHE_DATA="$RESULTS_DIR/cache_decisions.tsv"
echo -e "size\tsystem\tjobs_run\tjobs_expected" > "$CACHE_DATA"

# Git-checkout / mtime-churn scenario: a checkout (or tree-copy, or
# backup-restore) bumps the mtime of a tracked input WITHOUT changing its
# bytes. This file records how many jobs each strategy re-runs in that case.
# `system` is one of: snakemake, ox-mtime, ox-hash.
CHURN_DATA="$RESULTS_DIR/mtime_churn.tsv"
echo -e "size\tsystem\tjobs_run" > "$CHURN_DATA"

for size in $SIZES; do
    dir="$WORKDIR/scale_${size}"
    [[ -d "$dir" ]] || { echo "WARN: $dir missing, skipping"; continue; }
    expected_jobs=$(( size + 1 ))    # generator emits 3·N+2 for N≥(size-2)/3; close enough for label
    echo "▶ Scale ${size} (~${expected_jobs} jobs)"

    # ── OxyMake side ──────────────────────────────────────────────────────
    clean_workload "$dir"
    read -r wall rss <<< "$(clean_resolve "$OX plan -f workflow.toml --json" "$dir" "$RUNS")"
    emit "$size" "ox" "dag-resolve" "cold" "$wall" "$rss"

    clean_workload "$dir"
    read -r wall rss <<< "$(measured_run "$OX run -f workflow.toml -j $JOBS" "$dir")"
    emit "$size" "ox" "e2e-run" "cold" "$wall" "$rss"

    read -r wall rss <<< "$(clean_resolve "$OX plan -f workflow.toml --json" "$dir" "$RUNS")"
    emit "$size" "ox" "dag-resolve" "warm" "$wall" "$rss"

    read -r wall rss <<< "$(measured_run "$OX run -f workflow.toml -j $JOBS" "$dir")"
    emit "$size" "ox" "e2e-run" "warm" "$wall" "$rss"

    # Warm re-run under full content-addressing (--cache-validation hash). This
    # is the mode the paper's correctness thesis rests on; the default warm row
    # above is the mtime fast-path. Measuring both shows what hashing costs on
    # a no-op rebuild.
    read -r wall rss <<< "$(measured_run "$OX run -f workflow.toml -j $JOBS --cache-validation hash" "$dir")"
    emit "$size" "ox" "e2e-run" "warm-hash" "$wall" "$rss"

    # OxyMake cache-decision: modify one Layer-1 input → expect 3 rebuilt jobs.
    echo "ox-cache-correctness-check-$(date +%s%N)" > "$dir/step1/i_000000.txt"
    ox_run_out="$( ( cd "$dir" && $OX run -f workflow.toml -j 1 2>&1 ) )"
    ox_rebuilt="$(echo "$ox_run_out" | grep -Eo '[0-9]+ succeeded' | head -1 | grep -Eo '[0-9]+' || echo '?')"
    echo -e "${size}\tox\t${ox_rebuilt}\t3" >> "$CACHE_DATA"

    # ── Snakemake side (fresh clean state — no shared mtime/cache pollution) ─
    clean_workload "$dir"
    read -r wall rss <<< "$(clean_resolve "$SNAKEMAKE -s workflow.smk --dryrun --quiet --cores 1" "$dir" "$RUNS")"
    emit "$size" "snakemake" "dag-resolve" "cold" "$wall" "$rss"

    clean_workload "$dir"
    read -r wall rss <<< "$(measured_run "$SNAKEMAKE -s workflow.smk --cores $JOBS --quiet" "$dir")"
    emit "$size" "snakemake" "e2e-run" "cold" "$wall" "$rss"

    read -r wall rss <<< "$(clean_resolve "$SNAKEMAKE -s workflow.smk --dryrun --quiet --cores 1" "$dir" "$RUNS")"
    emit "$size" "snakemake" "dag-resolve" "warm" "$wall" "$rss"

    read -r wall rss <<< "$(measured_run "$SNAKEMAKE -s workflow.smk --cores $JOBS --quiet" "$dir")"
    emit "$size" "snakemake" "e2e-run" "warm" "$wall" "$rss"

    # Snakemake cache-decision: modify same Layer-1 input → expect 3 rebuilt (excluding `all`).
    sleep 1   # mtime resolution: ensure modified mtime > step2 outputs
    echo "snakemake-cache-correctness-check-$(date +%s%N)" > "$dir/step1/i_000000.txt"
    sm_dryrun="$( cd "$dir" && $SNAKEMAKE -s workflow.smk --dryrun --quiet --cores 1 2>&1 || true )"
    sm_rebuilt_with_all="$(echo "$sm_dryrun" | awk '/^total / { print $2 }' | head -1)"
    if [[ -z "$sm_rebuilt_with_all" ]]; then sm_rebuilt_with_all=1; fi
    sm_rebuilt=$(( sm_rebuilt_with_all - 1 ))    # subtract the implicit `all`
    echo -e "${size}\tsnakemake\t${sm_rebuilt}\t3" >> "$CACHE_DATA"

    ox_status="$([[ "$ox_rebuilt" == "3" ]] && echo PASS || echo "rebuilt=$ox_rebuilt expected=3")"
    sm_status="$([[ "$sm_rebuilt" == "3" ]] && echo PASS || echo "rebuilt=$sm_rebuilt expected=3")"
    echo "  cache-decision: ox=$ox_status | snakemake=$sm_status"

    # ── Git-checkout / mtime-churn scenario ────────────────────────────────
    # Simulate `git checkout` (or tree-copy / backup-restore): bump the mtime
    # of the shared tracked input (bench_lib.py) WITHOUT changing a byte. A
    # purely mtime-based decision must re-run every job that reads it; a
    # content-addressed decision must re-run zero. This is the table the
    # correctness thesis lives or dies on.
    #
    # bench_lib.py is the named `lib` input of every `process` job (declared in
    # the fixture), so the churn radius is process + finalize + merge.

    # OxyMake, mtime fast-path (the default):
    clean_workload "$dir"
    ( cd "$dir" && $OX run -f workflow.toml -j "$JOBS" >/dev/null 2>&1 )
    touch "$WORKDIR/bench_lib.py"
    ox_mtime_churn="$( ( cd "$dir" && $OX run -f workflow.toml -j "$JOBS" --cache-validation mtime 2>&1 ) | grep -Eo '[0-9]+ succeeded' | head -1 | grep -Eo '[0-9]+' || echo '?')"
    echo -e "${size}\tox-mtime\t${ox_mtime_churn}" >> "$CHURN_DATA"

    # OxyMake, full content-addressing:
    clean_workload "$dir"
    ( cd "$dir" && $OX run -f workflow.toml -j "$JOBS" >/dev/null 2>&1 )
    touch "$WORKDIR/bench_lib.py"
    ox_hash_churn="$( ( cd "$dir" && $OX run -f workflow.toml -j "$JOBS" --cache-validation hash 2>&1 ) | grep -Eo '[0-9]+ succeeded' | head -1 | grep -Eo '[0-9]+' || echo '?')"
    echo -e "${size}\tox-hash\t${ox_hash_churn}" >> "$CHURN_DATA"

    # Snakemake (default rerun-triggers), same churn:
    clean_workload "$dir"
    ( cd "$dir" && $SNAKEMAKE -s workflow.smk --cores "$JOBS" --quiet >/dev/null 2>&1 )
    sleep 1
    touch "$WORKDIR/bench_lib.py"
    sm_churn_dryrun="$( cd "$dir" && $SNAKEMAKE -s workflow.smk --dryrun --quiet --cores 1 2>&1 || true )"
    if echo "$sm_churn_dryrun" | grep -qi "Nothing to be done"; then
        sm_churn=0
    else
        sm_churn_with_all="$(echo "$sm_churn_dryrun" | awk '/^total / { print $2 }' | head -1)"
        [[ -z "$sm_churn_with_all" ]] && sm_churn_with_all=1
        sm_churn=$(( sm_churn_with_all - 1 ))   # subtract the implicit `all`
    fi
    echo -e "${size}\tsnakemake\t${sm_churn}" >> "$CHURN_DATA"

    echo "  mtime-churn (git checkout): snakemake=$sm_churn | ox-mtime=$ox_mtime_churn | ox-hash=$ox_hash_churn"
done

# ── Job-submission throughput (derived) ────────────────────────────────────
# throughput = total_jobs / (e2e_wall - dag_resolve_wall) for the cold runs at
# the largest scale. Computed and written into RESULTS.md by the renderer.

# ── Render RESULTS.md ──────────────────────────────────────────────────────
echo ""
echo "--- Rendering RESULTS.md ---"
"$PYTHON_BIN" "$SCRIPT_DIR/render_results.py" \
    --data "$DATA_FILE" \
    --cache-data "$CACHE_DATA" \
    --churn-data "$CHURN_DATA" \
    --out "$RESULTS_FILE" \
    --plot "$SCRIPT_DIR/scaling.pdf" \
    --hardware "$HW_INFO | $CPU_BRAND | $CORES cores | $(($MEM_TOTAL_KB / 1024 / 1024)) GiB" \
    --snakemake-version "$($SNAKEMAKE --version)" \
    --jobs "$JOBS"

echo ""
echo "✅ Done. See: $RESULTS_FILE"
echo "   Plot:    $SCRIPT_DIR/scaling.pdf"
echo "   Data:    $DATA_FILE"
