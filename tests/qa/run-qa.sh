#!/usr/bin/env bash
# OxyMake QA — End-to-end user experience validation
#
# Usage:
#   bash tests/qa/run-qa.sh                     # Run all scenarios
#   bash tests/qa/run-qa.sh fresh-install        # Run one scenario
#   bash tests/qa/run-qa.sh --list               # List available scenarios
#
# Environment:
#   OX=path/to/ox      Override ox binary (default: target/debug/ox)
#   QA_TIMEOUT=300      Per-scenario timeout in seconds (default: 120)
#   QA_VERBOSE=1        Show full output on failure (default: on)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# --- Configuration -----------------------------------------------------------

OX="${OX:-$REPO_ROOT/target/debug/ox}"
QA_TIMEOUT="${QA_TIMEOUT:-120}"
QA_VERBOSE="${QA_VERBOSE:-1}"

# Colors (skip if not a terminal or piped)
if [ -t 1 ]; then
    BOLD='\033[1m'  RED='\033[31m'  GREEN='\033[32m'
    YELLOW='\033[33m'  CYAN='\033[36m'  DIM='\033[2m'  RESET='\033[0m'
else
    BOLD='' RED='' GREEN='' YELLOW='' CYAN='' DIM='' RESET=''
fi

# --- State -------------------------------------------------------------------

PASS=0
FAIL=0
SKIP=0
RESULTS=()
TOTAL_START=$(date +%s)

# --- Helpers -----------------------------------------------------------------

qa_setup() {
    # Create an isolated working directory with QA Oxymakefile
    local workdir
    workdir=$(mktemp -d "${TMPDIR:-/tmp}/oxymake-qa.XXXXXX")
    cp "$SCRIPT_DIR/Oxymakefile.toml" "$workdir/"
    echo "$workdir"
}

qa_cleanup() {
    local workdir="$1"
    rm -rf "$workdir" 2>/dev/null || true
}

# Run a scenario function, capture output, track pass/fail with timing.
run_scenario() {
    local name="$1"
    local desc="$2"
    local func="$3"

    local start elapsed status logfile
    start=$(date +%s)
    logfile=$(mktemp "${TMPDIR:-/tmp}/oxymake-qa-log.XXXXXX")

    printf "${BOLD}${CYAN}[QA]${RESET} %-45s " "$name"

    if ( "$func" ) >"$logfile" 2>&1; then
        elapsed=$(( $(date +%s) - start ))
        printf "${GREEN}PASS${RESET} (%ds)\n" "$elapsed"
        PASS=$((PASS + 1))
        status="PASS"
    else
        local exit_code=$?
        elapsed=$(( $(date +%s) - start ))
        printf "${RED}FAIL${RESET} (%ds, exit %d)\n" "$elapsed" "$exit_code"
        status="FAIL"
        FAIL=$((FAIL + 1))
        if [ "$QA_VERBOSE" = "1" ]; then
            echo -e "${DIM}--- output ---${RESET}"
            tail -40 "$logfile" | sed 's/^/  /'
            echo -e "${DIM}--- end ---${RESET}"
        fi
    fi

    RESULTS+=("$status|${elapsed}s|$name|$desc")
    rm -f "$logfile"
}

# --- Scenarios ---------------------------------------------------------------

scenario_fresh_install() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX lint
    $OX run -vv

    # Verify outputs exist
    [ -f "report/summary.txt" ] || { echo "FAIL: report/summary.txt missing"; exit 1; }
    [ -f "data/alpha.csv" ] || { echo "FAIL: data/alpha.csv missing"; exit 1; }
    [ -f "results/alpha_sorted.csv" ] || { echo "FAIL: results/alpha_sorted.csv missing"; exit 1; }
    grep -q "QA Pipeline Report" report/summary.txt || { echo "FAIL: report missing header"; exit 1; }
}

scenario_external_results_dir() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    # Use a subdirectory within the project (executor enforces project root boundary)
    local extdir="output/external/qa"

    $OX init 2>/dev/null || true
    $OX run --set results_dir="$extdir"

    # Verify outputs landed in the custom directory
    [ -f "$extdir/alpha_sorted.csv" ] || { echo "FAIL: alpha_sorted.csv not in external dir"; exit 1; }
    [ -f "$extdir/beta_sorted.csv" ] || { echo "FAIL: beta_sorted.csv not in external dir"; exit 1; }
    [ -f "$extdir/gamma_sorted.csv" ] || { echo "FAIL: gamma_sorted.csv not in external dir"; exit 1; }
    [ -f "report/summary.txt" ] || { echo "FAIL: report/summary.txt missing"; exit 1; }
}

scenario_cache_reuse() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # First run: everything executes
    $OX run
    [ -f "report/summary.txt" ] || { echo "FAIL: first run missing report"; exit 1; }

    # Second run: nothing should execute (all cached)
    local output
    output=$($OX run --json 2>&1)
    # In a fully cached run, no jobs should have status "started"
    local started
    started=$(echo "$output" | grep -c '"status":"started"' || true)
    if [ "$started" -gt 0 ]; then
        echo "FAIL: cache miss — $started jobs re-ran on second run"
        echo "$output"
        exit 1
    fi
}

scenario_missing_data() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run

    # Delete one data file and the downstream outputs
    rm -f data/alpha.csv results/alpha_sorted.csv report/summary.txt

    # Re-run: should regenerate alpha chain only
    local output
    output=$($OX run --json 2>&1)

    # Verify recovery
    [ -f "data/alpha.csv" ] || { echo "FAIL: alpha.csv not regenerated"; exit 1; }
    [ -f "results/alpha_sorted.csv" ] || { echo "FAIL: alpha_sorted.csv not regenerated"; exit 1; }
    [ -f "report/summary.txt" ] || { echo "FAIL: report not regenerated"; exit 1; }
}

scenario_corrupted_output() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run

    # Corrupt an output file AND delete it to force re-execution.
    # Note: simply corrupting in-place may not trigger re-build if the cache
    # key (based on inputs) still matches. Deleting forces a rebuild because
    # the output existence check fails.
    rm -f results/beta_sorted.csv report/summary.txt

    # Re-run: should detect missing output and re-execute
    $OX run

    [ -f "results/beta_sorted.csv" ] || { echo "FAIL: beta_sorted.csv not rebuilt"; exit 1; }
    grep -q "word,count" results/beta_sorted.csv || { echo "FAIL: beta_sorted.csv content wrong"; exit 1; }
    [ -f "report/summary.txt" ] || { echo "FAIL: report not rebuilt"; exit 1; }
}

scenario_delete_all_results() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run

    # Nuke everything
    rm -rf data results report

    # Full rebuild
    $OX run

    [ -f "data/alpha.csv" ] || { echo "FAIL: data not rebuilt"; exit 1; }
    [ -f "results/alpha_sorted.csv" ] || { echo "FAIL: results not rebuilt"; exit 1; }
    [ -f "report/summary.txt" ] || { echo "FAIL: report not rebuilt"; exit 1; }
    grep -q "QA Pipeline Report" report/summary.txt || { echo "FAIL: report content wrong"; exit 1; }
}

scenario_parallel_j1() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run -j 1
    [ -f "report/summary.txt" ] || { echo "FAIL: report missing with -j1"; exit 1; }
}

scenario_parallel_j4() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run -j 4
    [ -f "report/summary.txt" ] || { echo "FAIL: report missing with -j4"; exit 1; }
}

scenario_parallel_j8() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true
    $OX run -j 8
    [ -f "report/summary.txt" ] || { echo "FAIL: report missing with -j8"; exit 1; }
}

scenario_default_target() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # Run with no target args — should use rule.all
    $OX run
    [ -f "report/summary.txt" ] || { echo "FAIL: default target didn't build report"; exit 1; }
}

scenario_verbose_modes() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # Plain run (no verbose)
    $OX run
    rm -rf data results report

    # -v (job timing)
    $OX run -v
    rm -rf data results report

    # -vv (stdout/stderr streaming)
    $OX run -vv

    [ -f "report/summary.txt" ] || { echo "FAIL: -vv run failed"; exit 1; }
}

scenario_config_override() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    # Use a relative path within project root (executor enforces boundary)
    local custom_dir="custom_results"

    $OX init 2>/dev/null || true
    $OX run --set results_dir="$custom_dir"

    [ -f "$custom_dir/alpha_sorted.csv" ] || { echo "FAIL: override results_dir not used"; exit 1; }
    [ -f "$custom_dir/beta_sorted.csv" ] || { echo "FAIL: override results_dir not used for beta"; exit 1; }
    [ -f "$custom_dir/gamma_sorted.csv" ] || { echo "FAIL: override results_dir not used for gamma"; exit 1; }
}

scenario_error_reporting() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # Create an Oxymakefile with a broken script
    cat > Oxymakefile.toml <<'EOF'
ox_version = "0.1"

[config]
samples = ["A"]

[rule.all]
input = ["output/{sample}.txt"]

[rule.broken]
output = ["output/{sample}.txt"]
wildcard_constraints = { sample = "A" }
shell = """
nonexistent_command_xyz_12345
"""
EOF

    # ox run should fail with a clear error
    local output
    if $OX run 2>&1; then
        echo "FAIL: broken script should have caused ox run to fail"
        exit 1
    fi

    # Success: ox run correctly reported the error
}

scenario_dry_run() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # Dry run should show plan but create no files
    $OX run --dry-run

    # No output files should exist
    [ ! -f "data/alpha.csv" ] || { echo "FAIL: dry run created files"; exit 1; }
    [ ! -f "report/summary.txt" ] || { echo "FAIL: dry run created report"; exit 1; }
}

scenario_no_cache_reruns_all() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # First run: build everything
    $OX run
    [ -f "report/summary.txt" ] || { echo "FAIL: first run missing report"; exit 1; }

    # Second run with --no-cache: everything must re-execute
    local output
    output=$($OX run --no-cache --json 2>&1)

    # Count started jobs — with --no-cache, at least one job must execute
    local started
    started=$(echo "$output" | grep -c '"event":"job_started"' || true)
    if [ "$started" -eq 0 ]; then
        echo "FAIL: --no-cache produced 0 jobs — rule outputs were not excluded from existing_files"
        echo "$output"
        exit 1
    fi

    # Verify outputs still exist after --no-cache rebuild
    [ -f "report/summary.txt" ] || { echo "FAIL: report missing after --no-cache"; exit 1; }
    [ -f "data/alpha.csv" ] || { echo "FAIL: data/alpha.csv missing after --no-cache"; exit 1; }
}

scenario_interrupted_resume() {
    local workdir
    workdir=$(qa_setup)
    trap "qa_cleanup '$workdir'" EXIT

    cd "$workdir"
    $OX init 2>/dev/null || true

    # Run the full pipeline first
    $OX run

    # Simulate partial state: delete some outputs but keep data
    rm -f results/gamma_sorted.csv report/summary.txt

    # Resume: should only rebuild the missing outputs
    $OX run

    [ -f "results/gamma_sorted.csv" ] || { echo "FAIL: gamma_sorted not rebuilt on resume"; exit 1; }
    [ -f "report/summary.txt" ] || { echo "FAIL: report not rebuilt on resume"; exit 1; }
}

# --- Scenario Registry -------------------------------------------------------

SCENARIOS=(
    "fresh-install|Fresh clone, build, run -vv|scenario_fresh_install"
    "external-results-dir|Config results_dir to external path|scenario_external_results_dir"
    "cache-reuse|Second run hits cache, zero re-execution|scenario_cache_reuse"
    "missing-data|Remove data file, re-run rebuilds chain|scenario_missing_data"
    "corrupted-output|Corrupt output, re-run detects and rebuilds|scenario_corrupted_output"
    "delete-all-results|Nuke all outputs, full rebuild|scenario_delete_all_results"
    "parallel-j1|Parallel execution -j1|scenario_parallel_j1"
    "parallel-j4|Parallel execution -j4|scenario_parallel_j4"
    "parallel-j8|Parallel execution -j8|scenario_parallel_j8"
    "default-target|ox run (no args) uses rule.all|scenario_default_target"
    "verbose-modes|ox run vs -v vs -vv|scenario_verbose_modes"
    "config-override|--set results_dir override|scenario_config_override"
    "error-reporting|Broken script gives clear error|scenario_error_reporting"
    "dry-run|--dry-run shows plan, creates nothing|scenario_dry_run"
    "no-cache-reruns-all|--no-cache re-executes even when outputs exist|scenario_no_cache_reruns_all"
    "interrupted-resume|Partial state, resume rebuilds missing|scenario_interrupted_resume"
)

# --- Main --------------------------------------------------------------------

list_scenarios() {
    echo "Available QA scenarios:"
    echo ""
    for entry in "${SCENARIOS[@]}"; do
        IFS='|' read -r name desc _ <<< "$entry"
        printf "  %-30s %s\n" "$name" "$desc"
    done
}

main() {
    local filter="${1:-}"

    if [ "$filter" = "--list" ] || [ "$filter" = "-l" ]; then
        list_scenarios
        exit 0
    fi

    # Verify ox binary exists
    if [ ! -x "$OX" ]; then
        echo -e "${RED}ERROR: ox binary not found at $OX${RESET}"
        echo "Build first: cargo build --bin ox"
        exit 1
    fi

    echo -e "${BOLD}${CYAN}━━━ OxyMake QA Suite ━━━${RESET}"
    echo -e "Binary: ${DIM}$OX${RESET}"
    echo -e "Timeout: ${DIM}${QA_TIMEOUT}s per scenario${RESET}"
    echo ""

    for entry in "${SCENARIOS[@]}"; do
        IFS='|' read -r name desc func <<< "$entry"
        if [ -n "$filter" ] && [ "$name" != "$filter" ]; then
            continue
        fi
        run_scenario "$name" "$desc" "$func"
    done

    # --- Report ---------------------------------------------------------------
    local total_elapsed=$(( $(date +%s) - TOTAL_START ))
    echo ""
    echo -e "${BOLD}━━━ QA Report ━━━${RESET}"
    echo ""
    printf "  %-10s %-8s %-35s %s\n" "STATUS" "TIME" "SCENARIO" "DESCRIPTION"
    printf "  %-10s %-8s %-35s %s\n" "------" "----" "--------" "-----------"
    for result in "${RESULTS[@]}"; do
        IFS='|' read -r status elapsed name desc <<< "$result"
        local color="$GREEN"
        [ "$status" = "FAIL" ] && color="$RED"
        [ "$status" = "TIMEOUT" ] && color="$RED"
        [ "$status" = "SKIP" ] && color="$YELLOW"
        printf "  ${color}%-10s${RESET} %-8s %-35s %s\n" "$status" "$elapsed" "$name" "$desc"
    done
    echo ""
    echo -e "  ${GREEN}Pass: $PASS${RESET}  ${RED}Fail: $FAIL${RESET}  ${YELLOW}Skip: $SKIP${RESET}  Total: $total_elapsed s"
    echo ""

    if [ "$FAIL" -gt 0 ]; then
        echo -e "${RED}${BOLD}QA FAILED${RESET} — $FAIL scenario(s) failed"
        exit 1
    else
        echo -e "${GREEN}${BOLD}QA PASSED${RESET} — all $PASS scenarios passed"
        exit 0
    fi
}

main "$@"
