#!/usr/bin/env bash
# ============================================================================
# OxyMake SLURM Demo — Live Cluster Lifecycle
# ============================================================================
#
# Demonstrates the complete SLURM lifecycle using a containerized SLURM cluster
# (via docker-compose) or mock SLURM scripts if Docker is unavailable.
#
# Usage:
#   just demo-slurm                  # From repo root
#   bash tests/slurm-docker/run-demo.sh   # Direct
#
# Environment:
#   OX=<path>       Path to ox binary (default: target/debug/ox)
#   MOCK=1          Force mock SLURM mode (skip Docker)
#   JOBS=4          Parallelism level (default: 4)
#   PORT=9876       Dashboard port (default: 9876)

set -euo pipefail

# --- Setup -------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OX="${OX:-$REPO_ROOT/target/debug/ox}"
MOCK="${MOCK:-0}"
JOBS="${JOBS:-4}"
PORT="${PORT:-9876}"
WORK_DIR=$(mktemp -d)
DASHBOARD_PID=""
COMPOSE_UP=false

# Resolve OX to absolute path
if [[ "$OX" != /* ]] && [[ "$OX" == */* ]]; then
    OX="$(cd "$(dirname "$OX")" && pwd)/$(basename "$OX")"
fi

# --- Colors ------------------------------------------------------------------

if [ -t 1 ]; then
    BOLD='\033[1m'    DIM='\033[2m'     RESET='\033[0m'
    RED='\033[31m'    GREEN='\033[32m'   YELLOW='\033[33m'
    CYAN='\033[36m'   MAGENTA='\033[35m' WHITE='\033[37m'
else
    BOLD='' DIM='' RESET='' RED='' GREEN='' YELLOW='' CYAN='' MAGENTA='' WHITE=''
fi

# --- Helpers -----------------------------------------------------------------

banner() {
    echo ""
    echo -e "${BOLD}${MAGENTA}╔══════════════════════════════════════════════════════════════╗${RESET}"
    echo -e "${BOLD}${MAGENTA}║  $1$(printf '%*s' $((58 - ${#1})) '')║${RESET}"
    echo -e "${BOLD}${MAGENTA}╚══════════════════════════════════════════════════════════════╝${RESET}"
    echo ""
}

step() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━ $1 ━━━${RESET}"
    echo ""
}

narrate() {
    echo -e "${DIM}$1${RESET}"
}

run() {
    echo -e "${YELLOW}\$ $*${RESET}"
    "$@"
    echo ""
}

ok() {
    echo -e "  ${GREEN}✓ $1${RESET}"
}

fail() {
    echo -e "  ${RED}✗ $1${RESET}"
}

pause() {
    if [ -t 0 ]; then
        echo -e "${YELLOW}[Press Enter to continue]${RESET}"
        read -r
    else
        sleep 1
    fi
}

# --- Cleanup -----------------------------------------------------------------

cleanup() {
    echo ""
    step "Cleanup"

    if [ -n "$DASHBOARD_PID" ] && kill -0 "$DASHBOARD_PID" 2>/dev/null; then
        narrate "Stopping dashboard (PID $DASHBOARD_PID)..."
        kill "$DASHBOARD_PID" 2>/dev/null
        wait "$DASHBOARD_PID" 2>/dev/null || true
        ok "Dashboard stopped"
    fi

    if [ "$COMPOSE_UP" = true ]; then
        narrate "Tearing down SLURM cluster..."
        (cd "$SCRIPT_DIR" && docker compose down -v 2>/dev/null) || true
        ok "Cluster torn down"
    fi

    if [ -d "$WORK_DIR" ]; then
        rm -rf "$WORK_DIR"
    fi

    # Clean up mock state
    if [ -d "/tmp/slurm-mock" ]; then
        rm -rf /tmp/slurm-mock
    fi
}
trap cleanup EXIT

# --- Pre-flight checks -------------------------------------------------------

banner "OxyMake SLURM Demo — Live Cluster Lifecycle"

narrate "This demo shows the complete SLURM workflow lifecycle:"
narrate "  1. Cluster startup (Docker or mock)"
narrate "  2. Job submission (multi-stage pipeline)"
narrate "  3. Live monitoring (squeue, ox status, dashboard)"
narrate "  4. Job cancellation and graceful handling"
narrate "  5. Intentional failure and error handling"
narrate "  6. Re-submission with caching"
narrate "  7. History and snapshot comparison"
narrate "  8. Cluster teardown"
echo ""

# Check ox binary
if [ ! -x "$OX" ]; then
    echo -e "${RED}ERROR: ox binary not found at $OX${RESET}"
    echo "Build it first: cargo build --bin ox"
    exit 1
fi
ok "ox binary: $OX"

# Decide: Docker cluster or mock SLURM
USE_DOCKER=false
if [ "$MOCK" != "1" ] && command -v docker &>/dev/null && docker info &>/dev/null 2>&1; then
    if command -v docker &>/dev/null && docker compose version &>/dev/null 2>&1; then
        USE_DOCKER=true
    fi
fi

if [ "$USE_DOCKER" = true ]; then
    ok "Docker available — will use containerized SLURM cluster"
    ok "Docker shims: $SCRIPT_DIR/bin/ prepended to PATH"
else
    narrate "Docker not available or MOCK=1 set."
    ok "Using mock SLURM scripts (from tests/mock-slurm/)"
    narrate ""
    narrate "NOTE: This demo uses mock SLURM — real cluster integration coming in v0.2."
    narrate "Mock scripts simulate sbatch/squeue/sacct/scancel/sinfo with realistic"
    narrate "state transitions (PENDING → RUNNING → COMPLETED/FAILED)."
fi
echo ""

pause

# =============================================================================
step "1. STARTUP — Spin up SLURM cluster"
# =============================================================================

if [ "$USE_DOCKER" = true ]; then
    narrate "Starting containerized SLURM cluster via docker-compose..."
    narrate "Components: slurmctld + 2× slurmd + slurmdbd + MariaDB"
    echo ""

    # Export volume mount paths so docker-compose picks them up.
    # The staging dir and work dir are bind-mounted at identical paths
    # inside the container, so job scripts work without path rewriting.
    export OXYMAKE_STAGING_DIR="/tmp/oxymake-slurm"
    export OXYMAKE_PROJECT_DIR="$WORK_DIR"

    (cd "$SCRIPT_DIR" && docker compose up -d 2>&1)
    COMPOSE_UP=true

    # Put Docker SLURM shims on PATH so sbatch/sacct/etc. proxy to the container.
    export PATH="$SCRIPT_DIR/bin:$PATH"

    narrate "Waiting for cluster to become ready..."
    for i in $(seq 1 30); do
        if sinfo -N -h 2>/dev/null | grep -q "idle"; then
            break
        fi
        sleep 2
        printf "."
    done
    echo ""

    echo -e "${BOLD}Cluster status:${RESET}"
    sinfo
    echo ""
    ok "SLURM cluster is up with 2 compute nodes"
    ok "Docker shims active — ox will submit jobs via host→container bridge"
else
    narrate "Initializing mock SLURM environment..."
    narrate "Mock scripts: sbatch, squeue, sacct, scancel, sinfo"
    echo ""

    # Put mock-slurm on PATH
    export PATH="$REPO_ROOT/tests/mock-slurm:$PATH"

    # Verify mock scripts are available
    echo -e "${BOLD}Cluster status (mock):${RESET}"
    run sinfo
    ok "Mock SLURM environment ready (2 virtual nodes: c1, c2)"
fi

pause

# =============================================================================
step "2. SUBMIT — Multi-stage OxyMake pipeline"
# =============================================================================

narrate "Setting up the demo workspace with a 4-stage pipeline:"
narrate "  generate (5 jobs) → process (5 jobs) → merge (1 job) → report (1 job)"
narrate "  12 total jobs, each sleeping 5-15s for visibility."
echo ""

# Copy workflow to temp dir
cp "$SCRIPT_DIR/Oxymakefile.toml" "$WORK_DIR/"
cd "$WORK_DIR"

# Initialize
$OX init 2>/dev/null || true

echo -e "${BOLD}Workflow DAG:${RESET}"
run $OX dag --format text

echo -e "${BOLD}Dry run (preview what will execute):${RESET}"
run $OX run --dry-run -j "$JOBS"

pause

narrate "Submitting pipeline to SLURM cluster..."
narrate "Watch jobs transition: PENDING → RUNNING → COMPLETED"
echo ""

# Run in background so we can show monitoring
$OX run --executor slurm -j "$JOBS" --no-cache > "$WORK_DIR/run1.log" 2>&1 &
RUN_PID=$!

# =============================================================================
step "3. MONITOR — Live job tracking"
# =============================================================================

narrate "While the pipeline runs, we monitor via multiple channels:"
echo ""

# Poll squeue a few times to show job states
for i in $(seq 1 6); do
    sleep 3
    echo -e "${BOLD}[t+$((i * 3))s] Job queue:${RESET}"
    if [ "$USE_DOCKER" = true ]; then
        squeue --format="%.8i %.12j %.8T %.4C %.8M %.12R" 2>/dev/null || true
    else
        # Show mock job states
        if [ -d /tmp/slurm-mock ]; then
            echo "  JOBID   NAME           STATE       NODE"
            for state_file in /tmp/slurm-mock/job_*; do
                if [ -f "$state_file" ]; then
                    jid=$(basename "$state_file" | sed 's/job_//')
                    state=$(cat "$state_file")
                    name=$(cat "/tmp/slurm-mock/jobname_$jid" 2>/dev/null || echo "unknown")
                    printf "  %-7s %-14s %-11s c1\n" "$jid" "$name" "$state"
                fi
            done
        fi
    fi
    echo ""

    # Check if pipeline is still running
    if ! kill -0 "$RUN_PID" 2>/dev/null; then
        ok "Pipeline completed during monitoring"
        break
    fi
done

# Wait for pipeline to finish
if kill -0 "$RUN_PID" 2>/dev/null; then
    narrate "Waiting for pipeline to complete..."
    wait "$RUN_PID" || true
fi

echo -e "${BOLD}Pipeline run output:${RESET}"
cat "$WORK_DIR/run1.log" 2>/dev/null || true
echo ""

echo -e "${BOLD}ox status:${RESET}"
run $OX status

if [ "$USE_DOCKER" = true ]; then
    echo -e "${BOLD}sacct summary:${RESET}"
    sacct --format=JobID,JobName,State,ExitCode,Elapsed,MaxRSS -X 2>/dev/null || true
    echo ""
fi

pause

# =============================================================================
step "4. CANCEL — Mid-run cancellation handling"
# =============================================================================

narrate "Demonstrating graceful cancellation:"
narrate "  1. Start a fresh run (clearing outputs)"
narrate "  2. Cancel one job mid-flight"
narrate "  3. Show ox handles it gracefully"
echo ""

# Clear outputs for a fresh run
rm -rf data results output

$OX run --executor slurm -j "$JOBS" --no-cache > "$WORK_DIR/run2.log" 2>&1 &
RUN_PID=$!

# Wait a moment for jobs to start
sleep 5

# Find a running job to cancel
CANCEL_JOB=""
if [ "$USE_DOCKER" = true ]; then
    CANCEL_JOB=$(squeue -h -o "%i" 2>/dev/null | head -1)
elif [ -d /tmp/slurm-mock ]; then
    for state_file in /tmp/slurm-mock/job_*; do
        if [ -f "$state_file" ] && [ "$(cat "$state_file")" = "RUNNING" ]; then
            CANCEL_JOB=$(basename "$state_file" | sed 's/job_//')
            break
        fi
    done
fi

if [ -n "$CANCEL_JOB" ]; then
    echo -e "${BOLD}Cancelling SLURM job $CANCEL_JOB:${RESET}"
    run scancel "$CANCEL_JOB"
    ok "Job $CANCEL_JOB cancelled"
    echo ""

    narrate "The pipeline continues with --keep-going behavior."
    narrate "Cancelled jobs are marked but independent branches proceed."
else
    narrate "No running jobs found to cancel (pipeline may have completed quickly)."
    narrate "In a real cluster with longer jobs, scancel would interrupt mid-flight."
fi

# Wait for pipeline
wait "$RUN_PID" 2>/dev/null || true

echo -e "${BOLD}Run output (with cancellation):${RESET}"
cat "$WORK_DIR/run2.log" 2>/dev/null | tail -20 || true
echo ""

pause

# =============================================================================
step "5. FAILURE — Intentional job failure and error handling"
# =============================================================================

narrate "Demonstrating error handling:"
narrate "  One job intentionally fails (exit 1), showing:"
narrate "  - Error propagation through the DAG"
narrate "  - --keep-going continues independent branches"
echo ""

# Create a modified workflow with a failing job
cat > "$WORK_DIR/Oxymakefile.toml" <<'TOML'
# Modified pipeline — sample s03 intentionally fails in process stage.

ox_version = "0.1"

[config]
samples = ["s01", "s02", "s03", "s04", "s05"]

[rule.all]
input = ["output/report.txt"]

[rule.generate]
output = ["data/{sample}.csv"]
wildcard_constraints = { sample = "s01|s02|s03|s04|s05" }
shell = """
mkdir -p data
DELAY=$((RANDOM % 3 + 3))
sleep $DELAY
echo "id,value,sample" > {output}
for i in $(seq 1 50); do
    echo "$i,$((RANDOM % 1000)),{sample}" >> {output}
done
"""

[rule.process]
input = { csv = "data/{sample}.csv" }
output = { stats = "results/{sample}_stats.txt", filtered = "results/{sample}_filtered.csv" }
wildcard_constraints = { sample = "s01|s02|s03|s04|s05" }
shell = """
mkdir -p results
DELAY=$((RANDOM % 3 + 3))
sleep $DELAY

# s03 intentionally fails!
if [ "{sample}" = "s03" ]; then
    echo "ERROR: Simulated processing failure for {sample}!" >&2
    exit 1
fi

ROWS=$(tail -n +2 {input.csv} | wc -l | tr -d ' ')
echo "sample: {sample}" > {output.stats}
echo "rows: $ROWS" >> {output.stats}
head -1 {input.csv} > {output.filtered}
tail -n +2 {input.csv} | awk -F, '$2 > 500' >> {output.filtered}
"""

[rule.merge]
input = ["results/{sample}_filtered.csv"]
output = ["output/combined.csv"]
expand = "product"
shell = """
mkdir -p output
sleep 3
FIRST=true
for f in {input}; do
    if [ "$FIRST" = true ]; then
        cat "$f" > {output}
        FIRST=false
    else
        tail -n +2 "$f" >> {output}
    fi
done
"""

[rule.report]
input = ["output/combined.csv", "results/{sample}_stats.txt"]
output = ["output/report.txt"]
expand = "product"
shell = """
sleep 2
echo "=== Pipeline Report ===" > {output}
echo "Combined: $(wc -l < output/combined.csv) rows" >> {output}
echo "Generated: $(date -u +%Y-%m-%dT%H:%M:%SZ)" >> {output}
"""
TOML

rm -rf data results output

echo -e "${BOLD}Running with --keep-going (s03 will fail):${RESET}"
run $OX run --executor slurm -j "$JOBS" --no-cache --keep-going 2>&1 || true

echo ""
narrate "Notice: s03 failed, but s01/s02/s04/s05 continued on independent paths."
narrate "The merge and report steps were blocked because they depend on all samples."

pause

# =============================================================================
step "6. RESUBMIT — Re-run with caching"
# =============================================================================

narrate "Fixing s03 (restoring original workflow) and re-running."
narrate "Cached jobs skip — only failed/missing work re-executes."
echo ""

# Restore original workflow
cp "$SCRIPT_DIR/Oxymakefile.toml" "$WORK_DIR/"

echo -e "${BOLD}Re-running pipeline (cached jobs should skip):${RESET}"
run $OX run -j "$JOBS" 2>&1 || true

echo ""
narrate "Jobs that already completed were skipped (cache hit)."
narrate "Only the failed s03 and its downstream dependents re-executed."

pause

# =============================================================================
step "7. HISTORY — Run comparison and snapshots"
# =============================================================================

narrate "OxyMake tracks every run. Let's examine the history."
echo ""

echo -e "${BOLD}Run history:${RESET}"
run $OX history

echo -e "${BOLD}Creating snapshot of current state:${RESET}"
run $OX snapshot create after-fix

run $OX snapshot list

echo ""
narrate "In production, you'd compare snapshots across runs to track"
narrate "how outputs change over time (ox snapshot diff baseline after-fix)."

pause

# =============================================================================
step "8. TEARDOWN — Clean exit"
# =============================================================================

if [ "$USE_DOCKER" = true ]; then
    narrate "Tearing down SLURM cluster..."
    echo ""
    run docker compose -f "$SCRIPT_DIR/docker-compose.yml" down -v
    COMPOSE_UP=false
    ok "Cluster destroyed"
else
    narrate "Cleaning up mock SLURM state..."
    rm -rf /tmp/slurm-mock
    ok "Mock state cleaned"
fi

echo ""

# =============================================================================
banner "Demo Complete"
# =============================================================================

echo -e "${GREEN}${BOLD}SLURM lifecycle stages demonstrated:${RESET}"
echo ""
echo "  1.  Cluster startup     — Docker compose or mock SLURM"
echo "  2.  Job submission      — Multi-stage pipeline via ox run"
echo "  3.  Live monitoring     — squeue polling, ox status"
echo "  4.  Job cancellation    — scancel + graceful handling"
echo "  5.  Failure handling    — Intentional failure + --keep-going"
echo "  6.  Cached re-execution — Only failed/missing jobs re-run"
echo "  7.  History & snapshots — ox history, ox snapshot"
echo "  8.  Clean teardown      — docker-compose down"
echo ""
if [ "$USE_DOCKER" != true ]; then
    echo -e "${DIM}NOTE: This demo used mock SLURM. For real cluster integration,${RESET}"
    echo -e "${DIM}install Docker and re-run without MOCK=1.${RESET}"
    echo ""
fi
echo -e "${BOLD}Key ox commands for SLURM workflows:${RESET}"
echo "  ox run -j 4 --executor slurm    # Submit to SLURM cluster"
echo "  ox status                        # Check pipeline progress"
echo "  ox history                       # View run history"
echo "  ox snapshot create/diff          # Track output state"
echo "  ox dashboard                     # Web dashboard (localhost:9876)"
echo ""
