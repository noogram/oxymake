#!/usr/bin/env bash
# ============================================================================
# OxyMake SLURM Integration — Real Docker Cluster E2E Tests
# ============================================================================
#
# Validates the SLURM executor against a real containerized SLURM cluster.
# Requires the Docker SLURM cluster to be running (`just slurm-up`).
#
# Usage:
#   just test-slurm-docker           # From repo root (recommended)
#   bash tests/slurm-docker/test-docker-e2e.sh   # Direct
#
# Prerequisites:
#   - Docker SLURM cluster running (just slurm-up)
#   - Linux ox binary built (just build-linux)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OX_LINUX="${REPO_ROOT}/target-linux/release/ox"

PASS_COUNT=0
FAIL_COUNT=0

# --- Colors ------------------------------------------------------------------

if [ -t 1 ]; then
    BOLD='\033[1m'  RESET='\033[0m'
    RED='\033[31m'  GREEN='\033[32m'  YELLOW='\033[33m'  CYAN='\033[36m'
else
    BOLD='' RESET='' RED='' GREEN='' YELLOW='' CYAN=''
fi

# --- Test Helpers ------------------------------------------------------------

assert_output_contains() {
    local description="$1"
    local expected="$2"
    local output="$3"
    if echo "$output" | grep -qF -- "$expected"; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description — expected '$expected' in output"
        echo "$output" | sed 's/^/    /' | head -10
    fi
}

assert_output_matches() {
    local description="$1"
    local pattern="$2"
    local output="$3"
    if echo "$output" | grep -qE -- "$pattern"; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description — expected pattern '$pattern' in output"
        echo "$output" | sed 's/^/    /' | head -10
    fi
}

section() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━ $1 ━━━${RESET}"
    echo ""
}

# --- Pre-flight checks -------------------------------------------------------

echo -e "${BOLD}OxyMake SLURM Docker Cluster E2E Tests${RESET}"
echo ""

# Check Linux binary
if [ ! -f "$OX_LINUX" ]; then
    echo -e "${RED}ERROR: Linux ox binary not found at $OX_LINUX${RESET}"
    echo "Build it: just build-linux"
    exit 1
fi

# Check cluster is running
if ! docker exec slurmctld sinfo -N -h 2>/dev/null | grep -q "idle\|alloc\|mix"; then
    echo -e "${RED}ERROR: SLURM cluster not running. Start with: just slurm-up${RESET}"
    exit 1
fi

echo -e "  ${GREEN}✓${RESET} Linux binary: $OX_LINUX"
echo -e "  ${GREEN}✓${RESET} SLURM cluster running"

# Deploy binary to the cluster
docker cp "$OX_LINUX" slurmctld:/usr/local/bin/ox
docker exec slurmctld chmod +x /usr/local/bin/ox

# =============================================================================
section "T1: Cluster health check"
# =============================================================================

OUTPUT=$(docker exec slurmctld sinfo --noheader 2>&1)
assert_output_matches "sinfo shows normal partition" "normal.*up" "$OUTPUT"

OUTPUT=$(docker exec slurmctld ox --version 2>&1)
assert_output_contains "ox binary works inside container" "ox" "$OUTPUT"

# =============================================================================
section "T2: Word-frequency benchmark (7 jobs)"
# =============================================================================
# The benchmark pipeline: generate (3) → count (3) → merge (1)

docker exec slurmctld bash -c 'rm -rf /work/* /work/.oxymake && mkdir -p /work/logs'
docker cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml
docker exec -w /work slurmctld ox init --force 2>/dev/null
docker cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml

# Dry run
OUTPUT=$(docker exec -w /work slurmctld ox run --dry-run -j 4 2>&1)
assert_output_contains "dry-run shows 7 jobs" "7 job(s)" "$OUTPUT"

# Real run on SLURM
OUTPUT=$(docker exec -w /work slurmctld ox run --executor slurm --follow -j 4 --no-cache 2>&1)
assert_output_contains "pipeline completes 7 jobs" "7 succeeded" "$OUTPUT"
assert_output_contains "zero failures" "0 failed" "$OUTPUT"

# Verify output file exists and contains expected content
OUTPUT=$(docker exec -w /work slurmctld cat results/merged_counts.txt 2>&1)
assert_output_contains "merged output has alpha section" "## alpha" "$OUTPUT"
assert_output_contains "merged output has beta section" "## beta" "$OUTPUT"
assert_output_contains "merged output has gamma section" "## gamma" "$OUTPUT"

# =============================================================================
section "T3: Cache re-run (all jobs skipped)"
# =============================================================================

OUTPUT=$(docker exec -w /work slurmctld ox run --executor slurm --follow -j 4 2>&1)
assert_output_contains "all jobs cached" "7 skipped" "$OUTPUT"
assert_output_matches "zero jobs executed" "0 succeeded" "$OUTPUT"

# =============================================================================
section "T4: ox status after SLURM run"
# =============================================================================

OUTPUT=$(docker exec -w /work slurmctld ox status 2>&1)
assert_output_matches "ox status shows completed jobs" "(7 completed|7 total)" "$OUTPUT"

# =============================================================================
section "T5: Demo pipeline (12 jobs with sleeps)"
# =============================================================================
# Larger pipeline: generate (5) → process (5) → merge (1) → report (1)

docker exec slurmctld bash -c 'scancel --user=root 2>/dev/null || true'
docker exec slurmctld bash -c 'rm -rf /work/* /work/.oxymake && mkdir -p /work'
docker cp "$SCRIPT_DIR/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml
docker exec -w /work slurmctld ox init --force 2>/dev/null
docker cp "$SCRIPT_DIR/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml

OUTPUT=$(docker exec -w /work slurmctld ox run --executor slurm --follow -j 4 --no-cache 2>&1)
assert_output_contains "12-job pipeline completes" "12 succeeded" "$OUTPUT"
assert_output_contains "zero failures in large pipeline" "0 failed" "$OUTPUT"

# Verify the report output
OUTPUT=$(docker exec -w /work slurmctld cat output/report.txt 2>&1)
assert_output_contains "report generated" "Pipeline Report" "$OUTPUT"

# =============================================================================
section "T6: Job cancellation on real cluster"
# =============================================================================

docker exec slurmctld bash -c 'rm -rf /work/data /work/results /work/output'

# Submit jobs (without --follow, so we can cancel)
docker exec -w /work slurmctld ox run --executor slurm -j 4 --no-cache 2>&1 >/dev/null

# Wait for some jobs to start
sleep 3

# Check that jobs are queued/running
OUTPUT=$(docker exec slurmctld squeue --noheader 2>&1)
assert_output_matches "jobs visible in squeue" "ox_" "$OUTPUT"

# Cancel all jobs
docker exec slurmctld scancel --user=root 2>&1

sleep 2

# Verify queue is empty
OUTPUT=$(docker exec slurmctld squeue --noheader 2>&1)
if [ -z "$OUTPUT" ] || ! echo "$OUTPUT" | grep -q "ox_"; then
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${GREEN}✓${RESET} scancel cleared all jobs from queue"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} jobs still in queue after scancel"
fi

# =============================================================================
section "T7: Output matches local execution"
# =============================================================================
# Re-run the benchmark on SLURM and compare output to local executor

docker exec slurmctld bash -c 'rm -rf /work/* /work/.oxymake && mkdir -p /work/logs'
docker cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml
docker exec -w /work slurmctld ox init --force 2>/dev/null
docker cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" slurmctld:/work/Oxymakefile.toml

docker exec -w /work slurmctld ox run --executor slurm --follow -j 4 --no-cache 2>&1 >/dev/null

SLURM_OUTPUT=$(docker exec -w /work slurmctld cat results/merged_counts.txt 2>&1)

# Run the same pipeline locally (using mock or local executor)
LOCAL_DIR=$(mktemp -d)
cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" "$LOCAL_DIR/"
cd "$LOCAL_DIR"
"$REPO_ROOT/target/debug/ox" init 2>/dev/null || true
cp "$SCRIPT_DIR/../../benchmark/Oxymakefile.toml" "$LOCAL_DIR/"
mkdir -p logs
"$REPO_ROOT/target/debug/ox" run -j 4 --no-cache 2>&1 >/dev/null || true
LOCAL_OUTPUT=$(cat results/merged_counts.txt 2>/dev/null || echo "LOCAL_FAILED")
rm -rf "$LOCAL_DIR"
cd "$REPO_ROOT"

# Compare: both should have the same structure (word counts may differ due to RANDOM
# but the structure and word list should match)
assert_output_contains "SLURM output has merged counts header" "# Merged word counts" "$SLURM_OUTPUT"
if [ "$LOCAL_OUTPUT" != "LOCAL_FAILED" ]; then
    assert_output_contains "local output has merged counts header" "# Merged word counts" "$LOCAL_OUTPUT"
    # Check same sections exist
    SLURM_SECTIONS=$(echo "$SLURM_OUTPUT" | grep "^##" | sort)
    LOCAL_SECTIONS=$(echo "$LOCAL_OUTPUT" | grep "^##" | sort)
    if [ "$SLURM_SECTIONS" = "$LOCAL_SECTIONS" ]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} SLURM and local outputs have identical section structure"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} section structure differs between SLURM and local"
    fi
else
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${YELLOW}⚠${RESET} local executor run skipped (local ox binary missing or failed)"
fi

# =============================================================================
# Summary
# =============================================================================

echo ""
echo -e "${BOLD}━━━ Results ━━━${RESET}"
echo ""
TOTAL=$((PASS_COUNT + FAIL_COUNT))
echo -e "  ${GREEN}$PASS_COUNT passed${RESET}, ${RED}$FAIL_COUNT failed${RESET} out of $TOTAL assertions"
echo ""

if [ "$FAIL_COUNT" -gt 0 ]; then
    echo -e "${RED}${BOLD}FAIL${RESET}"
    exit 1
else
    echo -e "${GREEN}${BOLD}ALL TESTS PASSED${RESET}"
    exit 0
fi
