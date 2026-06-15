#!/usr/bin/env bash
# OxyMake Demo — Executable tutorial
#
# Run from the repo root:
#   cd examples/demo && bash run-demo.sh
#
# Or from anywhere (pass the ox binary path):
#   OX=/path/to/oxymake bash examples/demo/run-demo.sh
#
# Requirements: oxymake binary, optional: graphviz (for DAG PNG)

set -euo pipefail

# --- Setup -------------------------------------------------------------------

# Resolve OX to absolute path before we cd to the temp dir
OX="${OX:-ox}"
if [[ "$OX" != /* ]] && [[ "$OX" == */* ]]; then
    OX="$(cd "$(dirname "$OX")" && pwd)/$(basename "$OX")"
fi
DEMO_DIR="$(cd "$(dirname "$0")" && pwd)"
WORK_DIR=$(mktemp -d)
trap 'rm -rf "$WORK_DIR"' EXIT

cp "$DEMO_DIR/Oxymakefile.toml" "$WORK_DIR/"
cd "$WORK_DIR"

# Colors (skip if not a terminal)
if [ -t 1 ]; then
    BOLD='\033[1m'  DIM='\033[2m'  GREEN='\033[32m'
    CYAN='\033[36m' YELLOW='\033[33m' RESET='\033[0m'
else
    BOLD='' DIM='' GREEN='' CYAN='' YELLOW='' RESET=''
fi

step() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━ $1 ━━━${RESET}"
    echo ""
}

run() {
    echo -e "${DIM}\$ $*${RESET}"
    "$@"
    echo ""
}

pause() {
    if [ -t 0 ]; then
        echo -e "${YELLOW}[Press Enter to continue]${RESET}"
        read -r
    fi
}

# ==============================================================================
step "1. Initialize project"
# ==============================================================================

run $OX init 2>/dev/null || true  # Creates .oxymake/ (Oxymakefile already copied)
echo "Working directory: $WORK_DIR"
echo "Oxymakefile.toml copied from examples/demo/"
pause

# ==============================================================================
step "2. Validate the workflow"
# ==============================================================================

run $OX lint
pause

# ==============================================================================
step "3. Visualize the DAG"
# ==============================================================================

echo -e "${BOLD}Text format:${RESET}"
run $OX dag --format text

echo -e "${BOLD}Mermaid format (renders in GitHub/Obsidian):${RESET}"
run $OX dag --format mermaid

if command -v dot &>/dev/null; then
    echo -e "${BOLD}Graphviz PNG:${RESET}"
    $OX dag --format dot | dot -Tpng -o dag.png
    echo "  Written to dag.png ($(wc -c < dag.png) bytes)"
    echo ""
else
    echo -e "${DIM}(Install graphviz to generate dag.png: brew install graphviz)${RESET}"
    echo ""
fi
pause

# ==============================================================================
step "4. Preview execution (dry run)"
# ==============================================================================

run $OX run --dry-run
pause

# ==============================================================================
step "5. Execute the pipeline"
# ==============================================================================

run $OX run
pause

# ==============================================================================
step "6. Inspect results"
# ==============================================================================

echo -e "${BOLD}Pipeline output:${RESET}"
cat report/summary.txt
echo ""
pause

# ==============================================================================
step "7. Check status and history"
# ==============================================================================

run $OX status
run $OX history
pause

# ==============================================================================
step "8. Content-addressable caching"
# ==============================================================================

echo "Re-running the same pipeline (nothing changed):"
run $OX run
echo -e "${GREEN}0 jobs ran — all outputs are up to date.${RESET}"
echo ""

echo "Deleting an output and re-running (only affected jobs re-execute):"
rm -f results/alpha_sorted.csv report/summary.txt
run $OX run
pause

# ==============================================================================
step "9. NDJSON event stream (for agents)"
# ==============================================================================

echo "Running with --json output (after removing outputs to force re-execution):"
rm -f results/beta_sorted.csv report/summary.txt
run $OX run --json
pause

# ==============================================================================
step "10. Reproducibility lockfile"
# ==============================================================================

run $OX lock generate
echo -e "${BOLD}Lockfile contents:${RESET}"
cat ox.lock
echo ""

echo "Verifying (should pass):"
run $OX lock verify
pause

# ==============================================================================
step "11. Snapshots"
# ==============================================================================

echo "Creating snapshot of current state:"
run $OX snapshot create baseline

run $OX snapshot list

echo "Removing outputs and re-running (generates new random data):"
rm -rf data results report
$OX run >/dev/null 2>&1

echo "Creating second snapshot:"
run $OX snapshot create after-rerun

echo "Comparing snapshots:"
run $OX snapshot diff baseline after-rerun
pause

# ==============================================================================
step "12. Workflow validation (ox test)"
# ==============================================================================

run $OX test
pause

# ==============================================================================
step "13. Run history (multiple runs recorded)"
# ==============================================================================

run $OX history
pause

# ==============================================================================
step "14. Monitoring tools"
# ==============================================================================

echo -e "${BOLD}Available monitoring:${RESET}"
echo ""
echo "  ox top              # TUI dashboard (ratatui, live updates)"
echo "  ox top --mock       # TUI with mock data (try it now!)"
echo "  ox top --stdin      # TUI fed by: ox run --json | ox top --stdin"
echo "  ox dashboard        # Web dashboard on http://127.0.0.1:9876"
echo ""
echo -e "${DIM}(These are interactive — run them separately)${RESET}"
echo ""

# ==============================================================================
step "Demo complete"
# ==============================================================================

echo -e "${GREEN}${BOLD}OxyMake features demonstrated:${RESET}"
echo ""
echo "  1.  ox init          — Project scaffolding"
echo "  2.  ox lint          — Workflow validation"
echo "  3.  ox dag           — DAG visualization (text, dot, mermaid)"
echo "  4.  ox run --dry-run — Execution preview"
echo "  5.  ox run           — Pipeline execution"
echo "  6.  ox status        — Job status summary"
echo "  7.  ox history       — Run history with timing"
echo "  8.  Caching          — Content-addressable, skip unchanged"
echo "  9.  ox run --json    — NDJSON events for agent integration"
echo "  10. ox lock          — Reproducibility lockfile + drift detection"
echo "  11. ox snapshot      — State snapshots + diff"
echo "  12. ox test          — Static workflow validation"
echo "  13. ox top           — TUI live dashboard"
echo "  14. ox dashboard     — Web dashboard (axum/htmx/SSE)"
echo ""
echo "Working directory was: $WORK_DIR"
