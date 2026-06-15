#!/usr/bin/env bash
# OxyMake Guided Tour — An annotated terminal walkthrough
#
# This script explains each ox command before running it, shows the output
# with context, and guides you through the core workflow features.
#
# Usage:
#   just demo-guided                  # from repo root (builds ox first)
#   OX=/path/to/ox bash examples/guided-tour.sh   # manual
#
# Requirements:
#   - oxymake binary (cargo build --bin ox)
#   - Optional: graphviz (for DAG PNG export)

set -euo pipefail

# --- Setup -------------------------------------------------------------------

OX="${OX:-ox}"
if [[ "$OX" != /* ]] && [[ "$OX" == */* ]]; then
    OX="$(cd "$(dirname "$OX")" && pwd)/$(basename "$OX")"
fi
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WORK_DIR=$(mktemp -d)
DASHBOARD_PID=""

cleanup() {
    if [ -n "$DASHBOARD_PID" ] && kill -0 "$DASHBOARD_PID" 2>/dev/null; then
        kill "$DASHBOARD_PID" 2>/dev/null
        wait "$DASHBOARD_PID" 2>/dev/null || true
    fi
    rm -rf "$WORK_DIR"
}
trap cleanup EXIT

cp "$SCRIPT_DIR/demo/Oxymakefile.toml" "$WORK_DIR/"
cd "$WORK_DIR"

# Colors (skip if not a terminal)
if [ -t 1 ]; then
    BOLD='\033[1m'  DIM='\033[2m'  GREEN='\033[32m'
    CYAN='\033[36m' YELLOW='\033[33m' BLUE='\033[34m' RESET='\033[0m'
else
    BOLD='' DIM='' GREEN='' CYAN='' YELLOW='' BLUE='' RESET=''
fi

step() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo -e "${BOLD}${CYAN}  $1${RESET}"
    echo -e "${BOLD}${CYAN}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${RESET}"
    echo ""
}

explain() {
    echo -e "${BLUE}  ℹ  $1${RESET}"
}

run() {
    echo -e "${DIM}  \$ $*${RESET}"
    "$@" 2>&1 | sed 's/^/  /'
    echo ""
}

pause() {
    if [ -t 0 ]; then
        echo -e "  ${YELLOW}[Enter to continue]${RESET}"
        read -r
    fi
}

# ==============================================================================
step "Welcome to the OxyMake Guided Tour"
# ==============================================================================

echo -e "  OxyMake is a workflow orchestrator inspired by Snakemake, written in Rust."
echo -e "  This tour walks you through the core features with a real pipeline."
echo ""
echo -e "  ${BOLD}What you'll see:${RESET}"
echo "    1. Project setup and workflow validation"
echo "    2. DAG visualization"
echo "    3. Pipeline execution and caching"
echo "    4. Status, history, and monitoring"
echo "    5. Lockfiles and snapshots"
echo ""
echo -e "  Working directory: ${DIM}$WORK_DIR${RESET}"
pause

# ==============================================================================
step "1. Initialize the project"
# ==============================================================================

explain "ox init creates the .oxymake/ directory for state tracking."
explain "Our Oxymakefile.toml defines a 3-stage word-frequency pipeline:"
explain "  generate → process (sort) → merge into report"
echo ""

run $OX init 2>/dev/null || true
pause

# ==============================================================================
step "2. Validate the workflow"
# ==============================================================================

explain "ox lint checks your Oxymakefile for errors before you run anything."
explain "It catches missing inputs, circular dependencies, and syntax issues."
echo ""

run $OX lint
pause

# ==============================================================================
step "3. Visualize the DAG"
# ==============================================================================

explain "ox dag shows the dependency graph. The DAG has 7 jobs:"
explain "  3 generate jobs → 3 process jobs → 1 merge job"
echo ""

echo -e "  ${BOLD}Text format (quick terminal view):${RESET}"
run $OX dag --format text

echo -e "  ${BOLD}Mermaid format (renders in GitHub / Obsidian):${RESET}"
echo '  ```mermaid'
$OX dag --format mermaid 2>&1 | sed 's/^/  /'
echo '  ```'
echo ""

if command -v dot &>/dev/null; then
    echo -e "  ${BOLD}Graphviz PNG:${RESET}"
    $OX dag --format dot | dot -Tpng -o dag.png
    echo -e "  Written to dag.png ($(wc -c < dag.png | tr -d ' ') bytes)"
    echo ""
else
    echo -e "  ${DIM}(Install graphviz for PNG export: brew install graphviz)${RESET}"
    echo ""
fi
pause

# ==============================================================================
step "4. Dry run — preview what will execute"
# ==============================================================================

explain "ox run --dry-run shows which jobs would execute, without running them."
explain "This is useful for verifying your workflow before committing to a run."
echo ""

run $OX run --dry-run
pause

# ==============================================================================
step "5. Execute the pipeline"
# ==============================================================================

explain "ox run executes jobs in dependency order with parallel scheduling."
explain "Watch the output — each job shows its rule name and target file."
echo ""

run $OX run
pause

# ==============================================================================
step "6. Inspect the results"
# ==============================================================================

explain "The pipeline generated data, processed it, and merged into a report."
echo ""

echo -e "  ${BOLD}Pipeline output (report/summary.txt):${RESET}"
cat report/summary.txt | sed 's/^/  /'
echo ""
pause

# ==============================================================================
step "7. Check status and history"
# ==============================================================================

explain "ox status shows the current state of all jobs."
echo ""
run $OX status

explain "ox history shows past runs with timing information."
echo ""
run $OX history
pause

# ==============================================================================
step "8. Content-addressable caching"
# ==============================================================================

explain "Re-running the same pipeline — nothing changed, so nothing executes."
explain "OxyMake tracks content hashes, not timestamps."
echo ""

run $OX run
echo -e "  ${GREEN}Notice: 0 jobs ran — everything was cached.${RESET}"
echo ""

explain "Now let's delete two outputs and re-run. Only affected jobs re-execute."
echo ""
rm -f results/alpha_sorted.csv report/summary.txt
run $OX run
echo -e "  ${GREEN}Only the deleted outputs (and their dependents) were rebuilt.${RESET}"
pause

# ==============================================================================
step "9. NDJSON event stream (for agent integration)"
# ==============================================================================

explain "ox run --json emits structured NDJSON events — one per line."
explain "This is how CI systems and AI agents consume OxyMake output."
echo ""

rm -f results/beta_sorted.csv report/summary.txt
run $OX run --json
pause

# ==============================================================================
step "10. Reproducibility lockfile"
# ==============================================================================

explain "ox lock generate creates a lockfile with content hashes for every output."
explain "ox lock verify checks that current outputs match the lockfile."
explain "This catches drift — someone changed an output without re-running the pipeline."
echo ""

run $OX lock generate
echo -e "  ${BOLD}Lockfile contents:${RESET}"
cat ox.lock | sed 's/^/  /'
echo ""

explain "Verifying (should pass — outputs match):"
run $OX lock verify
pause

# ==============================================================================
step "11. Snapshots — checkpoint and compare"
# ==============================================================================

explain "ox snapshot create saves the current state under a named tag."
explain "You can later diff snapshots to see what changed between runs."
echo ""

run $OX snapshot create tour-v1

explain "Let's force a re-run with new random data and take a second snapshot."
echo ""
rm -rf data results report
$OX run >/dev/null 2>&1
run $OX snapshot create tour-v2

explain "Comparing the two snapshots:"
run $OX snapshot diff tour-v1 tour-v2
pause

# ==============================================================================
step "12. Static workflow validation"
# ==============================================================================

explain "ox test runs validation checks on your workflow definition."
explain "Think of it as 'cargo test' for your pipeline."
echo ""

run $OX test
pause

# ==============================================================================
step "13. Monitoring tools"
# ==============================================================================

explain "OxyMake includes two monitoring interfaces:"
echo ""
echo -e "  ${BOLD}ox top${RESET}          Terminal UI (ratatui) — live job status"
echo -e "  ${BOLD}ox top --mock${RESET}    Try it now with simulated data"
echo -e "  ${BOLD}ox dashboard${RESET}     Web dashboard on http://127.0.0.1:9876"
echo ""
echo -e "  ${BOLD}Try them:${RESET}"
echo "    just top-mock           # TUI with mock data"
echo "    just demo-dashboard     # Full dashboard demo"
echo ""
pause

# ==============================================================================
step "Tour complete"
# ==============================================================================

echo -e "  ${GREEN}${BOLD}Features covered:${RESET}"
echo ""
echo "    ox init         Project scaffolding"
echo "    ox lint         Workflow validation"
echo "    ox dag          DAG visualization (text, dot, mermaid)"
echo "    ox run          Pipeline execution (parallel, cached)"
echo "    ox run --json   Structured NDJSON events"
echo "    ox status       Job status summary"
echo "    ox history      Run history with timing"
echo "    ox lock         Reproducibility lockfile + drift detection"
echo "    ox snapshot     State checkpoints + diff"
echo "    ox test         Static workflow validation"
echo "    ox top          TUI live dashboard"
echo "    ox dashboard    Web dashboard (axum/htmx/SSE)"
echo ""
echo -e "  ${BOLD}Next steps:${RESET}"
echo "    just demo              # Interactive demo (same pipeline, less explanation)"
echo "    just demo-dashboard    # Dashboard demo with live updates"
echo "    just benchmark         # Performance benchmarks at scale"
echo ""
echo -e "  ${DIM}Working directory was: $WORK_DIR${RESET}"
