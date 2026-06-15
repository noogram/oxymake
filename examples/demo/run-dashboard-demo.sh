#!/usr/bin/env bash
# OxyMake Dashboard Demo
#
# Runs the pipeline, launches the web dashboard, opens the browser,
# then re-runs the pipeline so you can see live updates.
#
# Usage:
#   cd examples/demo && bash run-dashboard-demo.sh
#   OX=/path/to/ox bash examples/demo/run-dashboard-demo.sh

set -euo pipefail

# --- Setup -------------------------------------------------------------------

OX="${OX:-ox}"
if [[ "$OX" != /* ]] && [[ "$OX" == */* ]]; then
    OX="$(cd "$(dirname "$OX")" && pwd)/$(basename "$OX")"
fi
DEMO_DIR="$(cd "$(dirname "$0")" && pwd)"
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

SLOW="${SLOW:-1}"
if [ "$SLOW" = "1" ]; then
    cp "$DEMO_DIR/Oxymakefile-slow.toml" "$WORK_DIR/Oxymakefile.toml"
else
    cp "$DEMO_DIR/Oxymakefile.toml" "$WORK_DIR/"
fi
cd "$WORK_DIR"

# Colors
if [ -t 1 ]; then
    BOLD='\033[1m' DIM='\033[2m' GREEN='\033[32m'
    CYAN='\033[36m' YELLOW='\033[33m' RESET='\033[0m'
else
    BOLD='' DIM='' GREEN='' CYAN='' YELLOW='' RESET=''
fi

step() { echo ""; echo -e "${BOLD}${CYAN}━━━ $1 ━━━${RESET}"; echo ""; }
run()  { echo -e "${DIM}\$ $*${RESET}"; "$@"; echo ""; }

pause() {
    if [ -t 0 ]; then
        echo -e "${YELLOW}[Press Enter to continue]${RESET}"
        read -r
    fi
}

PORT="${PORT:-9876}"

# --- Port management ---------------------------------------------------------

find_available_port() {
    local port="$1"
    local max_attempts=10
    for _ in $(seq 1 "$max_attempts"); do
        if ! lsof -i :"$port" -sTCP:LISTEN -t &>/dev/null; then
            echo "$port"
            return 0
        fi
        # Check if the process is an ox dashboard we can kill
        local pids
        pids=$(lsof -i :"$port" -sTCP:LISTEN -t 2>/dev/null || true)
        if [ -n "$pids" ]; then
            local is_ox=0
            for pid in $pids; do
                if ps -p "$pid" -o args= 2>/dev/null | grep -q "ox.*dashboard\|oxymake"; then
                    is_ox=1
                    echo -e "${YELLOW}Killing stale ox dashboard (PID $pid) on port $port${RESET}" >&2
                    kill "$pid" 2>/dev/null && sleep 0.5
                fi
            done
            if [ "$is_ox" = "1" ] && ! lsof -i :"$port" -sTCP:LISTEN -t &>/dev/null; then
                echo "$port"
                return 0
            fi
        fi
        echo -e "${DIM}Port $port in use by another process, trying $((port + 1))...${RESET}" >&2
        port=$((port + 1))
    done
    echo ""
    return 1
}

PORT=$(find_available_port "$PORT")
if [ -z "$PORT" ]; then
    echo "ERROR: Could not find an available port (tried $PORT through $((PORT + 9)))"
    exit 1
fi

# ==============================================================================
step "1. Initialize and run the pipeline (creates state.db)"
# ==============================================================================

JOBS="${JOBS:-4}"
$OX init 2>/dev/null || true
if [ "$SLOW" = "1" ]; then
    echo -e "${BOLD}Running slow demo workflow with -j $JOBS ...${RESET}"
    echo -e "24 jobs across 7 rules with sleep 2-5s each. Total: ~30-60s."
    echo -e "${DIM}First run: building pipeline state.db (~30s with -j $JOBS)...${RESET}"
    echo ""
    run $OX run -j "$JOBS"
else
    run $OX run
fi
echo -e "State database created at ${BOLD}.oxymake/state.db${RESET}"
pause

# ==============================================================================
step "2. Launch the web dashboard"
# ==============================================================================

echo -e "Starting dashboard on ${BOLD}http://127.0.0.1:${PORT}${RESET}"
$OX dashboard --port "$PORT" --db .oxymake/state.db &
DASHBOARD_PID=$!
sleep 1

# Verify it started
if ! kill -0 "$DASHBOARD_PID" 2>/dev/null; then
    echo "ERROR: Dashboard failed to start"
    exit 1
fi

echo -e "${GREEN}Dashboard running (PID $DASHBOARD_PID)${RESET}"
echo ""

# Open browser
if command -v open &>/dev/null; then
    open "http://127.0.0.1:${PORT}"
elif command -v xdg-open &>/dev/null; then
    xdg-open "http://127.0.0.1:${PORT}"
else
    echo "Open http://127.0.0.1:${PORT} in your browser"
fi

pause

# ==============================================================================
step "3. Visualize the DAG"
# ==============================================================================

echo -e "${BOLD}Rule DAG (text):${RESET}"
run $OX dag --format text

echo -e "${BOLD}Mermaid (paste into GitHub/Obsidian):${RESET}"
echo '```mermaid'
$OX dag --format mermaid
echo '```'
echo ""

if command -v dot &>/dev/null; then
    $OX dag --format dot | dot -Tpng -o dag.png
    echo -e "Graphviz PNG written to ${BOLD}dag.png${RESET}"
    echo ""
fi
pause

# ==============================================================================
step "4. Re-run pipeline (watch the dashboard update live)"
# ==============================================================================

echo -e "Deleting outputs and re-running. ${BOLD}Watch the dashboard!${RESET}"
echo ""
if [ "$SLOW" = "1" ]; then
    echo -e "${GREEN}Watch the dashboard — you should see jobs turn blue (running)"
    echo -e "then green (completed). The Gantt chart will show real timeline bars.${RESET}"
    echo ""
    rm -rf src build lib test dist
    run $OX run -j "$JOBS"
else
    rm -rf data results report
    run $OX run
fi
pause

# ==============================================================================
step "5. Check status via CLI and API"
# ==============================================================================

echo -e "${BOLD}CLI status:${RESET}"
run $OX status

echo -e "${BOLD}CLI history:${RESET}"
run $OX history

echo -e "${BOLD}Dashboard API (/api/status):${RESET}"
curl -s "http://127.0.0.1:${PORT}/api/status" | python3 -m json.tool 2>/dev/null || \
    curl -s "http://127.0.0.1:${PORT}/api/status"
echo ""
echo ""

echo -e "${BOLD}Dashboard API (/api/jobs):${RESET}"
curl -s "http://127.0.0.1:${PORT}/api/jobs" | python3 -m json.tool 2>/dev/null || \
    curl -s "http://127.0.0.1:${PORT}/api/jobs"
echo ""
echo ""

echo -e "${BOLD}Dashboard API (/api/dag):${RESET}"
curl -s "http://127.0.0.1:${PORT}/api/dag" | python3 -m json.tool 2>/dev/null || \
    curl -s "http://127.0.0.1:${PORT}/api/dag"
echo ""
pause

# ==============================================================================
step "6. NDJSON event stream + re-execution"
# ==============================================================================

echo "Removing outputs and running with --json (structured events):"
if [ "$SLOW" = "1" ]; then
    rm -rf src build lib test dist
else
    rm -rf data results report
fi
echo ""
run $OX run --json
pause

# ==============================================================================
step "7. Lockfile and snapshots"
# ==============================================================================

run $OX lock generate
echo "Verifying lockfile:"
run $OX lock verify

echo "Creating snapshot:"
run $OX snapshot create demo-v1

run $OX snapshot list
pause

# ==============================================================================
step "8. Workflow validation"
# ==============================================================================

run $OX test
pause

# ==============================================================================
step "Dashboard demo complete"
# ==============================================================================

echo -e "${GREEN}${BOLD}The dashboard is still running at http://127.0.0.1:${PORT}${RESET}"
echo ""
echo "  Endpoints:"
echo "    GET /              — HTML dashboard (htmx, live SSE updates)"
echo "    GET /api/status    — JSON workflow status"
echo "    GET /api/dag       — JSON DAG (nodes + edges)"
echo "    GET /api/jobs      — JSON job list (?status= filter)"
echo "    GET /api/events    — SSE event stream"
echo ""
echo -e "${YELLOW}Press Enter to stop the dashboard and clean up.${RESET}"
if [ -t 0 ]; then read -r; fi

echo "Stopping dashboard..."
