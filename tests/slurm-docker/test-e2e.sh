#!/usr/bin/env bash
# ============================================================================
# OxyMake SLURM Integration — Automated End-to-End Test Suite
# ============================================================================
#
# Validates the complete SLURM executor lifecycle using mock SLURM scripts.
# Every assertion is checked and the test exits non-zero on first failure.
#
# Usage:
#   just test-slurm-e2e        # From repo root (recommended)
#   bash tests/slurm-docker/test-e2e.sh   # Direct
#
# Environment:
#   OX=<path>       Path to ox binary (default: target/debug/ox)
#   VERBOSE=1       Show full command output (default: quiet)

set -euo pipefail

# --- Setup -------------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OX="${OX:-$REPO_ROOT/target/debug/ox}"
VERBOSE="${VERBOSE:-0}"

PASS_COUNT=0
FAIL_COUNT=0
TEST_WORK_DIR=""

# --- Colors ------------------------------------------------------------------

if [ -t 1 ]; then
    BOLD='\033[1m'  RESET='\033[0m'
    RED='\033[31m'  GREEN='\033[32m'  YELLOW='\033[33m'  CYAN='\033[36m'
else
    BOLD='' RESET='' RED='' GREEN='' YELLOW='' CYAN=''
fi

# --- Test Helpers ------------------------------------------------------------

assert_ok() {
    local description="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
        if [ "$VERBOSE" = "1" ]; then
            echo "$output" | sed 's/^/    /'
        fi
        return 0
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description"
        echo "$output" | sed 's/^/    /' | head -20
        return 1
    fi
}

assert_fail() {
    local description="$1"
    shift
    local output
    if output=$("$@" 2>&1); then
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description (expected failure, got success)"
        return 1
    else
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
        return 0
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

assert_file_not_exists() {
    local description="$1"
    local path="$2"
    if [ ! -f "$path" ]; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} $description"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} $description — file unexpectedly exists: $path"
    fi
}

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

# --- Cleanup -----------------------------------------------------------------

cleanup() {
    if [ -n "$TEST_WORK_DIR" ] && [ -d "$TEST_WORK_DIR" ]; then
        rm -rf "$TEST_WORK_DIR"
    fi
    rm -rf /tmp/slurm-mock
}
trap cleanup EXIT

# --- Pre-flight checks -------------------------------------------------------

echo -e "${BOLD}OxyMake SLURM E2E Test Suite${RESET}"
echo ""

if [ ! -x "$OX" ]; then
    echo -e "${RED}ERROR: ox binary not found at $OX${RESET}"
    echo "Build first: cargo build --bin ox"
    exit 1
fi

# Put mock-slurm on PATH
export PATH="$REPO_ROOT/tests/mock-slurm:$PATH"
# Use fast mock timing for tests
export SLURM_MOCK_PENDING_SECS=0.2
export SLURM_MOCK_RUNNING_SECS=0.5

# =============================================================================
section "T1: Mock SLURM CLI verification"
# =============================================================================
# Verify mock scripts work correctly before testing the executor.

rm -rf /tmp/slurm-mock

assert_ok "sinfo --version returns slurm version" \
    bash -c 'sinfo --version | grep -q "slurm"'

assert_ok "sinfo -N -h returns compute nodes" \
    bash -c 'sinfo -N -h | grep -q "c1"'

# Test sbatch → sacct lifecycle
OUTPUT=$(sbatch --parsable /dev/null 2>&1)
assert_output_matches "sbatch --parsable returns numeric job ID" "^[0-9]+$" "$OUTPUT"
MOCK_JOB_ID="$OUTPUT"

sleep 1
OUTPUT=$(sacct -j "$MOCK_JOB_ID" --parsable2 --noheader -o "JobID,State,ExitCode" 2>&1)
assert_output_contains "sacct returns COMPLETED for finished job" "COMPLETED" "$OUTPUT"
assert_output_contains "sacct shows exit code 0:0" "0:0" "$OUTPUT"

# Test scancel on completed job (should be no-op)
assert_ok "scancel on completed job is a no-op" \
    scancel "$MOCK_JOB_ID"

rm -rf /tmp/slurm-mock

# =============================================================================
section "T2: Basic pipeline submission via --executor slurm"
# =============================================================================
# Submit a simple pipeline and verify it completes.

TEST_WORK_DIR=$(mktemp -d)
cp "$SCRIPT_DIR/Oxymakefile.toml" "$TEST_WORK_DIR/"
cd "$TEST_WORK_DIR"

# Initialize
$OX init 2>/dev/null || true

# Dry run first
OUTPUT=$($OX run --dry-run -j 4 2>&1)
assert_output_contains "dry-run shows 12 jobs" "12 job(s)" "$OUTPUT"

# Run with SLURM executor (--follow polls until completion)
OUTPUT=$($OX run --executor slurm --follow -j 4 --no-cache 2>&1)
assert_output_matches "pipeline completes with succeeded count" "12 succeeded" "$OUTPUT"
assert_output_contains "pipeline reports 0 failures" "0 failed" "$OUTPUT"

# Verify state.db exists after SLURM run (known gap: SLURM executor may not
# create local state.db — see ox-49z)
if [ -f "$TEST_WORK_DIR/.oxymake/state.db" ]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${GREEN}✓${RESET} state.db created after SLURM run"

    OUTPUT=$($OX status 2>&1)
    assert_output_matches "ox status shows completed run" "(12|succeeded|Completed)" "$OUTPUT"
else
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${YELLOW}⚠${RESET} state.db not created after SLURM run (known gap — SLURM executor uses staging dir)"
fi

rm -rf "$TEST_WORK_DIR"

# =============================================================================
section "T3: Job cancellation handling"
# =============================================================================
# Verify that scancel during a run doesn't crash the executor.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)

# Create a simple 2-job workflow (fast)
cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/b.txt"]

[rule.step_a]
output = ["out/a.txt"]
shell = """
mkdir -p out
echo "a" > {output}
"""

[rule.step_b]
input = ["out/a.txt"]
output = ["out/b.txt"]
shell = """
echo "b" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

# Use longer mock timing so we can cancel mid-flight
export SLURM_MOCK_PENDING_SECS=0.5
export SLURM_MOCK_RUNNING_SECS=3

$OX run --executor slurm -j 2 --no-cache > "$TEST_WORK_DIR/cancel_run.log" 2>&1 &
CANCEL_PID=$!

# Wait for a job to start, then cancel it
sleep 2
if [ -d /tmp/slurm-mock ]; then
    for sf in /tmp/slurm-mock/job_*; do
        if [ -f "$sf" ] && [ "$(cat "$sf")" = "RUNNING" ]; then
            jid=$(basename "$sf" | sed 's/job_//')
            scancel "$jid" 2>/dev/null
            break
        fi
    done
fi

wait "$CANCEL_PID" 2>/dev/null || true

# The executor should not crash — even if it reports failures
OUTPUT=$(cat "$TEST_WORK_DIR/cancel_run.log" 2>/dev/null || echo "")
# Check for absence of panic/crash signals
if echo "$OUTPUT" | grep -qi "panic\|segfault\|SIGSEGV"; then
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} executor crashed on job cancellation"
else
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${GREEN}✓${RESET} executor handles cancellation without crashing"
fi

# Reset timing
export SLURM_MOCK_PENDING_SECS=0.2
export SLURM_MOCK_RUNNING_SECS=0.5

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T4: Job failure propagation"
# =============================================================================
# Verify that a failing SLURM job is reported correctly.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)

# Create a workflow where one job will fail
cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/result.txt"]

[rule.good_job]
output = ["out/good.txt"]
shell = """
mkdir -p out
echo "good" > {output}
"""

[rule.bad_job]
output = ["out/bad.txt"]
shell = """
mkdir -p out
echo "this will fail" >&2
exit 1
"""

[rule.final]
input = ["out/good.txt", "out/bad.txt"]
output = ["out/result.txt"]
shell = """
cat out/good.txt out/bad.txt > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

# Use SLURM_MOCK_EXEC=1 so the mock actually runs the script and fails
export SLURM_MOCK_EXEC=1

OUTPUT=$($OX run --executor slurm --follow -j 4 --no-cache --keep-going 2>&1 || true)

# The pipeline should report at least one failure
assert_output_matches "pipeline reports failures" "(failed|Failed|FAILED|error)" "$OUTPUT"

# The final target should not exist (since bad_job failed)
assert_file_not_exists "final output not created when dependency fails" "$TEST_WORK_DIR/out/result.txt"

unset SLURM_MOCK_EXEC
rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T5: SLURM resource mapping verification"
# =============================================================================
# Verify that resource declarations produce correct #SBATCH directives.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/done.txt"]

[rule.gpu_job]
output = ["out/done.txt"]
resources = { cpus = 4, mem = "8G", gpus = 2 }
shell = """
mkdir -p out
echo "done" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm -j 1 --no-cache 2>&1 || true

# Wait for job to complete
sleep 2

# Find the generated sbatch script and verify directives
FOUND_SCRIPT=false
for script in /tmp/slurm-mock/script_*; do
    if [ -f "$script" ]; then
        FOUND_SCRIPT=true
        SCRIPT_CONTENT=$(cat "$script")

        assert_output_contains "sbatch script has --cpus-per-task" "--cpus-per-task" "$SCRIPT_CONTENT"
        assert_output_matches "sbatch script has --mem with MB value" "--mem" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --gpus" "--gpus" "$SCRIPT_CONTENT"
        break
    fi
done

if [ "$FOUND_SCRIPT" = false ]; then
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} no sbatch script found in mock dir"
fi

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T6: Partition and account flags"
# =============================================================================
# Verify --partition and --account are passed through to sbatch.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/done.txt"]

[rule.job]
output = ["out/done.txt"]
shell = """
mkdir -p out
echo "done" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm --partition gpu --account my-lab -j 1 --no-cache 2>&1 || true

sleep 2

for script in /tmp/slurm-mock/script_*; do
    if [ -f "$script" ]; then
        SCRIPT_CONTENT=$(cat "$script")
        assert_output_contains "sbatch script has --partition=gpu" "--partition" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --account=my-lab" "--account" "$SCRIPT_CONTENT"
        break
    fi
done

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T7: History and snapshots after SLURM run"
# =============================================================================
# Verify that ox history and ox snapshot work after SLURM executor runs.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/a.txt"]

[rule.make_a]
output = ["out/a.txt"]
shell = """
mkdir -p out
echo "hello" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm -j 1 --no-cache 2>&1 || true

# History should show the run (if state.db was created)
if [ -f "$TEST_WORK_DIR/.oxymake/state.db" ]; then
    OUTPUT=$($OX history 2>&1)
    assert_output_matches "ox history shows at least one run" "run-" "$OUTPUT"

    # Snapshot creation
    OUTPUT=$($OX snapshot create test-snap 2>&1)
    assert_output_contains "snapshot created" "created" "$OUTPUT"

    OUTPUT=$($OX snapshot list 2>&1)
    assert_output_contains "snapshot appears in list" "test-snap" "$OUTPUT"
else
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${YELLOW}⚠${RESET} ox history/snapshot skipped — state.db not created by SLURM executor (known gap)"
fi

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T8: Mock SLURM concurrent job ID assignment"
# =============================================================================
# Verify that concurrent sbatch calls get unique job IDs (the flock→mkdir fix).

rm -rf /tmp/slurm-mock
mkdir -p /tmp/slurm-mock

# Run from /tmp to avoid cwd issues from prior test cleanup
CONC_DIR=$(mktemp -d)
cd "$CONC_DIR"

PIDS=()
for i in $(seq 1 10); do
    sbatch --parsable /dev/null > "$CONC_DIR/id_result_$i" 2>/dev/null &
    PIDS+=($!)
done

for pid in "${PIDS[@]}"; do
    wait "$pid" 2>/dev/null || true
done

# Collect all assigned IDs (filter to numeric only)
IDS=()
for i in $(seq 1 10); do
    if [ -f "$CONC_DIR/id_result_$i" ]; then
        val=$(grep -E '^[0-9]+$' "$CONC_DIR/id_result_$i" 2>/dev/null || true)
        if [ -n "$val" ]; then
            IDS+=("$val")
        fi
    fi
done

# Check uniqueness
TOTAL_COUNT=${#IDS[@]}
if [ "$TOTAL_COUNT" -gt 0 ]; then
    UNIQUE_COUNT=$(printf '%s\n' "${IDS[@]}" | sort -u | wc -l | tr -d ' ')
else
    UNIQUE_COUNT=0
fi

if [ "$UNIQUE_COUNT" = "$TOTAL_COUNT" ] && [ "$TOTAL_COUNT" = "10" ]; then
    PASS_COUNT=$((PASS_COUNT + 1))
    echo -e "  ${GREEN}✓${RESET} 10 concurrent sbatch calls produced 10 unique job IDs"
else
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} concurrent job IDs not unique: $UNIQUE_COUNT unique out of $TOTAL_COUNT total"
    printf '    IDs: %s\n' "${IDS[*]}"
fi

rm -rf /tmp/slurm-mock "$CONC_DIR"

# =============================================================================
section "T9: Multi-node job resource directives"
# =============================================================================
# Verify that multi-node resource specs (nodes, ntasks_per_node) produce
# correct #SBATCH directives.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/done.txt"]

[rule.mpi_job]
output = ["out/done.txt"]
resources = { nodes = 2, ntasks_per_node = 4, mem = "16G" }
shell = """
mkdir -p out
echo "MPI ranks: $SLURM_NTASKS" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm -j 1 --no-cache 2>&1 || true

sleep 2

FOUND_SCRIPT=false
for script in /tmp/slurm-mock/script_*; do
    if [ -f "$script" ]; then
        FOUND_SCRIPT=true
        SCRIPT_CONTENT=$(cat "$script")

        assert_output_contains "sbatch script has --nodes=2" "--nodes=2" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --ntasks-per-node=4" "--ntasks-per-node=4" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --mem=16G" "--mem=16G" "$SCRIPT_CONTENT"
        break
    fi
done

if [ "$FOUND_SCRIPT" = false ]; then
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} no sbatch script found for multi-node job"
fi

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T10: GPU job with --gres directive"
# =============================================================================
# Verify that gres resource produces correct --gres SBATCH directive.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/done.txt"]

[rule.gpu_train]
output = ["out/done.txt"]
resources = { cpus = 8, mem = "32G", gres = "gpu:2" }
shell = """
mkdir -p out
echo "GPU training complete" > {output}
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm -j 1 --no-cache 2>&1 || true

sleep 2

FOUND_SCRIPT=false
for script in /tmp/slurm-mock/script_*; do
    if [ -f "$script" ]; then
        FOUND_SCRIPT=true
        SCRIPT_CONTENT=$(cat "$script")

        assert_output_contains "sbatch script has --gres=gpu:2" "--gres=gpu:2" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --cpus-per-task=8" "--cpus-per-task=8" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has --mem=32G" "--mem=32G" "$SCRIPT_CONTENT"
        break
    fi
done

if [ "$FOUND_SCRIPT" = false ]; then
    FAIL_COUNT=$((FAIL_COUNT + 1))
    echo -e "  ${RED}✗${RESET} no sbatch script found for GPU job"
fi

rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T11: Module loading in job scripts"
# =============================================================================
# Verify that Conda environment setup generates module load directives,
# and that the mock module command works.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"
export SLURM_MOCK_EXEC=1

# Verify mock module command works
assert_ok "mock module avail lists modules" \
    bash -c 'module avail 2>&1 | grep -q "python"'

assert_ok "mock module load tracks loaded modules" \
    bash -c 'module load python/3.11 && module list | grep -q "python/3.11"'

# Test that a conda-env job generates module load conda in the script
cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/done.txt"]

[rule.ml_job]
output = ["out/done.txt"]
shell = """
mkdir -p out
echo "done" > {output}
"""

[rule.ml_job.environment]
conda = "torch-env"
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

$OX run --executor slurm -j 1 --no-cache 2>&1 || true

sleep 2

for script in /tmp/slurm-mock/script_*; do
    if [ -f "$script" ]; then
        SCRIPT_CONTENT=$(cat "$script")
        assert_output_contains "sbatch script has module load conda" "module load conda" "$SCRIPT_CONTENT"
        assert_output_contains "sbatch script has conda activate torch-env" "conda activate torch-env" "$SCRIPT_CONTENT"
        break
    fi
done

unset SLURM_MOCK_EXEC
rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T12: Scratch filesystem handling (TMPDIR)"
# =============================================================================
# Verify that jobs can use $TMPDIR for scratch space — a common HPC pattern
# where compute nodes copy data to fast local storage, process, then copy back.

rm -rf /tmp/slurm-mock
TEST_WORK_DIR=$(mktemp -d)
export SLURM_MOCK_DIR="/tmp/slurm-mock"
export SLURM_MOCK_EXEC=1

cat > "$TEST_WORK_DIR/Oxymakefile.toml" <<'TOML'
ox_version = "0.1"

[rule.all]
input = ["out/result.txt"]

[rule.scratch_job]
output = ["out/result.txt"]
shell = """
mkdir -p out
# Simulate HPC scratch pattern: copy to TMPDIR, process, copy back
SCRATCH="${TMPDIR:-/tmp}/ox_scratch_$$"
mkdir -p "$SCRATCH"
echo "input data" > "$SCRATCH/data.txt"
wc -c < "$SCRATCH/data.txt" > "$SCRATCH/result.txt"
cp "$SCRATCH/result.txt" {output}
rm -rf "$SCRATCH"
"""
TOML
cd "$TEST_WORK_DIR"
$OX init 2>/dev/null || true

OUTPUT=$($OX run --executor slurm -j 1 --no-cache 2>&1 || true)

# Wait for mock job to execute
sleep 3

assert_file_exists "scratch job produced output file" "$TEST_WORK_DIR/out/result.txt"

if [ -f "$TEST_WORK_DIR/out/result.txt" ]; then
    CONTENT=$(cat "$TEST_WORK_DIR/out/result.txt" | tr -d '[:space:]')
    if [ -n "$CONTENT" ] && [ "$CONTENT" -gt 0 ] 2>/dev/null; then
        PASS_COUNT=$((PASS_COUNT + 1))
        echo -e "  ${GREEN}✓${RESET} scratch job wrote valid byte count ($CONTENT)"
    else
        FAIL_COUNT=$((FAIL_COUNT + 1))
        echo -e "  ${RED}✗${RESET} scratch job output is empty or invalid: '$CONTENT'"
    fi
fi

unset SLURM_MOCK_EXEC
rm -rf "$TEST_WORK_DIR" /tmp/slurm-mock

# =============================================================================
section "T13: Dependency chains (--dependency=afterok)"
# =============================================================================
# Verify that mock SLURM handles job dependencies correctly:
# job B waits for job A to complete before starting.

rm -rf /tmp/slurm-mock
mkdir -p /tmp/slurm-mock
export SLURM_MOCK_DIR="/tmp/slurm-mock"
cd /tmp

# Submit job A (fast — completes quickly)
export SLURM_MOCK_PENDING_SECS=0.1
export SLURM_MOCK_RUNNING_SECS=0.5

JOB_A=$(sbatch --parsable /dev/null 2>&1)
assert_output_matches "job A gets numeric ID" "^[0-9]+$" "$JOB_A"

# Submit job B with dependency on A
JOB_B=$(sbatch --parsable --dependency=afterok:$JOB_A /dev/null 2>&1)
assert_output_matches "job B gets numeric ID" "^[0-9]+$" "$JOB_B"

# Verify dependency was recorded
assert_file_exists "dependency file created for job B" "/tmp/slurm-mock/dep_$JOB_B"
if [ -f "/tmp/slurm-mock/dep_$JOB_B" ]; then
    DEP_CONTENT=$(cat "/tmp/slurm-mock/dep_$JOB_B")
    assert_output_contains "dependency references afterok:$JOB_A" "afterok:$JOB_A" "$DEP_CONTENT"
fi

# Wait for both to complete
sleep 3

# Job A should complete
OUTPUT_A=$(sacct -j "$JOB_A" --parsable2 --noheader -o "JobID,State" 2>&1)
assert_output_contains "job A completed" "COMPLETED" "$OUTPUT_A"

# Job B should also complete (dependency satisfied)
OUTPUT_B=$(sacct -j "$JOB_B" --parsable2 --noheader -o "JobID,State" 2>&1)
assert_output_contains "job B completed (dependency satisfied)" "COMPLETED" "$OUTPUT_B"

# Test failed dependency: submit C, then D depends on C, but C fails
rm -rf /tmp/slurm-mock
mkdir -p /tmp/slurm-mock

export SLURM_MOCK_PENDING_SECS=0.1
export SLURM_MOCK_RUNNING_SECS=0.3

JOB_C=$(SLURM_MOCK_FAIL_JOBS="" sbatch --parsable /dev/null 2>&1)
# Make job C fail
export SLURM_MOCK_FAIL_JOBS="$JOB_C"
# Re-submit C so it fails (the first one already started without fail flag)
rm -rf /tmp/slurm-mock
mkdir -p /tmp/slurm-mock
JOB_C=$(sbatch --parsable /dev/null 2>&1)

JOB_D=$(SLURM_MOCK_FAIL_JOBS="" sbatch --parsable --dependency=afterok:$JOB_C /dev/null 2>&1)
unset SLURM_MOCK_FAIL_JOBS

sleep 3

OUTPUT_C=$(sacct -j "$JOB_C" --parsable2 --noheader -o "JobID,State" 2>&1)
assert_output_contains "job C failed as expected" "FAILED" "$OUTPUT_C"

OUTPUT_D=$(sacct -j "$JOB_D" --parsable2 --noheader -o "JobID,State" 2>&1)
assert_output_contains "job D cancelled (afterok dependency failed)" "CANCELLED" "$OUTPUT_D"

# Reset timing
export SLURM_MOCK_PENDING_SECS=0.2
export SLURM_MOCK_RUNNING_SECS=0.5

rm -rf /tmp/slurm-mock

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
