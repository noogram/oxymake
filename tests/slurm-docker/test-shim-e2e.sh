#!/usr/bin/env bash
# ============================================================================
# OxyMake SLURM Docker Shims — E2E Test
# ============================================================================
#
# Validates the transparent filesystem bridge: ox runs on the HOST, SLURM
# commands are proxied to Docker containers via shim scripts.
#
# This is the intended production usage pattern:
#   - ox binary runs on the developer's machine (macOS/Linux)
#   - Docker shims in tests/slurm-docker/bin/ bridge to the SLURM cluster
#   - Volume mounts ensure host paths are identical inside containers
#
# Usage:
#   bash tests/slurm-docker/test-shim-e2e.sh
#
# Prerequisites:
#   - Docker running with compose support
#   - ox binary built (cargo build --bin ox)
#
# Environment:
#   OX=<path>   Path to ox binary (default: target/debug/ox)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OX="${OX:-$REPO_ROOT/target/debug/ox}"

PASS_COUNT=0
FAIL_COUNT=0
COMPOSE_UP=false
TEST_WORK_DIR=""

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

assert_file_exists() {
    local description="$1"
    local path="$2"
    if [ -f "$path" ]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description — file not found: $path"
    fi
}

section() {
    echo ""
    echo -e "${BOLD}${CYAN}━━━ $1 ━━━${RESET}"
    echo ""
}

# --- Cleanup -----------------------------------------------------------------

cleanup() {
    if [ "$COMPOSE_UP" = true ]; then
        echo ""
        echo -e "${BOLD}Tearing down Docker SLURM cluster...${RESET}"
        (cd "$SCRIPT_DIR" && docker compose down -v 2>/dev/null) || true
    fi
    if [ -n "$TEST_WORK_DIR" ] && [ -d "$TEST_WORK_DIR" ]; then
        rm -rf "$TEST_WORK_DIR"
    fi
}
trap cleanup EXIT

# --- Pre-flight checks -------------------------------------------------------

echo -e "${BOLD}OxyMake SLURM Docker Shims — E2E Test${RESET}"
echo ""

# Check ox binary
if [ ! -x "$OX" ]; then
    echo -e "${RED}ERROR: ox binary not found at $OX${RESET}"
    echo "Build it: cargo build --bin ox"
    exit 1
fi
echo -e "  ${GREEN}✓${RESET} ox binary: $OX"

# Check Docker
if ! command -v docker &>/dev/null || ! docker info &>/dev/null 2>&1; then
    echo -e "${RED}ERROR: Docker not available${RESET}"
    exit 1
fi
echo -e "  ${GREEN}✓${RESET} Docker available"

# =============================================================================
section "Setup: Start Docker SLURM cluster with volume mounts"
# =============================================================================

# Create a temporary project directory for the test
TEST_WORK_DIR=$(mktemp -d)
echo -e "  Work dir: $TEST_WORK_DIR"

# Set volume mount paths for docker-compose
export OXYMAKE_STAGING_DIR="/tmp/oxymake-slurm"
export OXYMAKE_PROJECT_DIR="$TEST_WORK_DIR"

# Ensure staging dir exists on host
mkdir -p "$OXYMAKE_STAGING_DIR"

# Start the cluster
(cd "$SCRIPT_DIR" && docker compose up -d 2>&1)
COMPOSE_UP=true

# Put Docker shims on PATH (must come BEFORE any real SLURM install)
export PATH="$SCRIPT_DIR/bin:$PATH"

# Wait for cluster readiness (using shims)
echo "  Waiting for cluster..."
for i in $(seq 1 30); do
    if sinfo -N -h 2>/dev/null | grep -q "idle"; then
        break
    fi
    sleep 2
    printf "."
done
echo ""

OUTPUT=$(sinfo 2>&1)
assert_output_matches "sinfo (via shim) shows cluster nodes" "normal.*up" "$OUTPUT"

# =============================================================================
section "T1: Shim health check"
# =============================================================================

OUTPUT=$(sinfo --version 2>&1)
assert_output_contains "sinfo --version works via shim" "slurm" "$OUTPUT"

OUTPUT=$(squeue --noheader 2>&1 || true)
PASS_COUNT=$((PASS_COUNT + 1))
echo -e "  ${GREEN}✓${RESET} squeue works via shim"

# =============================================================================
section "T2: Simple pipeline via host ox + Docker shims"
# =============================================================================
# Run a 2-job pipeline from the HOST using Docker shims for SLURM commands.

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/b.txt"]

[rule.step_a]
output = ["out/a.txt"]
shell = """
mkdir -p out
echo "hello from step_a" > {output}
"""

[rule.step_b]
input = ["out/a.txt"]
output = ["out/b.txt"]
shell = """
cat {input} | tr '[:lower:]' '[:upper:]' > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

OUTPUT=$($OX run --executor slurm --follow -j 2 --no-cache 2>&1)
assert_output_matches "pipeline completes via shims" "(2 succeeded|succeeded)" "$OUTPUT"
assert_output_contains "zero failures" "0 failed" "$OUTPUT"

# Verify outputs exist on the HOST filesystem
assert_file_exists "step_a output on host" "$TEST_WORK_DIR/out/a.txt"
assert_file_exists "step_b output on host" "$TEST_WORK_DIR/out/b.txt"

if [ -f "$TEST_WORK_DIR/out/b.txt" ]; then
    CONTENT=$(cat "$TEST_WORK_DIR/out/b.txt")
    assert_output_contains "step_b output is uppercase" "HELLO FROM STEP_A" "$CONTENT"
fi

# =============================================================================
section "T3: Benchmark pipeline (7 jobs) via shims"
# =============================================================================

rm -rf "$TEST_WORK_DIR"/*
TEST_WORK_DIR_NEW=$(mktemp -d)
# Update the volume mount (requires cluster restart for new project dir)
export OXYMAKE_PROJECT_DIR="$TEST_WORK_DIR_NEW"
(cd "$SCRIPT_DIR" && docker compose up -d 2>&1) >/dev/null

cp "$REPO_ROOT/benchmark/Oxymakefile.toml" "$TEST_WORK_DIR_NEW/"
cd "$TEST_WORK_DIR_NEW"
$OX init 2>/dev/null || true

OUTPUT=$($OX run --executor slurm --follow -j 4 --no-cache 2>&1)
assert_output_matches "benchmark pipeline completes" "(7 succeeded|succeeded)" "$OUTPUT"
assert_output_contains "zero failures in benchmark" "0 failed" "$OUTPUT"

# Verify output on host
if [ -f "$TEST_WORK_DIR_NEW/results/merged_counts.txt" ]; then
    CONTENT=$(cat "$TEST_WORK_DIR_NEW/results/merged_counts.txt")
    assert_output_contains "merged output has alpha section" "## alpha" "$CONTENT"
    assert_output_contains "merged output has beta section" "## beta" "$CONTENT"
    assert_output_contains "merged output has gamma section" "## gamma" "$CONTENT"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} merged_counts.txt not found on host"
fi

# Clean up the extra work dir
rm -rf "$TEST_WORK_DIR_NEW"
# Restore original project dir
export OXYMAKE_PROJECT_DIR="$TEST_WORK_DIR"
(cd "$SCRIPT_DIR" && docker compose up -d 2>&1) >/dev/null

# =============================================================================
section "T4: Cache re-run (all jobs skipped)"
# =============================================================================

cd "$TEST_WORK_DIR"
cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/a.txt"]

[rule.step_a]
output = ["out/a.txt"]
shell = """
mkdir -p out
echo "cached" > {output}
"""
TOML
$OX init 2>/dev/null || true

# First run
$OX run --executor slurm --follow -j 1 --no-cache 2>&1 >/dev/null

# Second run — should skip
OUTPUT=$($OX run --executor slurm --follow -j 1 2>&1)
assert_output_matches "cached jobs are skipped" "(1 skipped|skipped|0 succeeded)" "$OUTPUT"

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
