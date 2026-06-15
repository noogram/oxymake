#!/usr/bin/env bash
# chaos.sh — SIGKILL chaos test for the ox scheduler.
#
# The complement of the cancel-vs-complete
# fix (task-20260527-1811): if a worker dies under SIGKILL mid-run, the
# scheduler must still drive every job to a terminal status — no job may be
# left in `running` state in the persisted state.db.
#
# The invariant we check ("terminal status set ≡ baseline") is read in the
# weak sense the mission statement intends: under chaos, the *set of job IDs
# that reached a terminal status* must equal the baseline set. The *which*
# terminal status each job reached can differ (a chaos run will see at least
# one Failed / Cancelled where the baseline sees Completed) — what cannot
# differ is whether every job converged at all.
#
# Usage:
#   bash scripts/chaos.sh                # one chaos iteration
#   ITERATIONS=5 bash scripts/chaos.sh   # five iterations
#
# Environment:
#   OX           Path to the ox binary (default: target/debug/ox)
#   ITERATIONS   Number of chaos iterations to run (default: 3)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
OX="${OX:-$REPO_ROOT/target/debug/ox}"
ITERATIONS="${ITERATIONS:-3}"

# Colors
if [ -t 1 ]; then
    BOLD='\033[1m' RED='\033[31m' GREEN='\033[32m'
    CYAN='\033[36m' DIM='\033[2m' RESET='\033[0m'
else
    BOLD='' RED='' GREEN='' CYAN='' DIM='' RESET=''
fi

log()      { printf "${BOLD}${CYAN}[chaos]${RESET} %s\n" "$*" >&2; }
log_pass() { printf "${BOLD}${GREEN}[chaos]${RESET} %s\n" "$*" >&2; }
log_fail() { printf "${BOLD}${RED}[chaos]${RESET} %s\n" "$*" >&2; }

# ---------------------------------------------------------------------------
# Pre-flight
# ---------------------------------------------------------------------------

if [ ! -x "$OX" ]; then
    log_fail "ox binary not found at $OX"
    log_fail "  build first: cargo build --bin ox"
    exit 2
fi

# `sqlite3` is required to introspect the state.db terminal-status set.
if ! command -v sqlite3 >/dev/null 2>&1; then
    log_fail "sqlite3 not found in PATH — required to read state.db"
    exit 2
fi

# ---------------------------------------------------------------------------
# Workdir & pipeline
# ---------------------------------------------------------------------------

setup_workdir() {
    local workdir
    workdir=$(mktemp -d "${TMPDIR:-/tmp}/oxymake-chaos.XXXXXX")
    cat > "$workdir/Oxymakefile.toml" <<'EOF'
# Chaos pipeline — N independent leaf jobs joined by a sink so the chaos
# test can SIGKILL any one of them mid-run. Sleeps are long enough to make
# the race observable in CI; short enough not to dominate the suite.

ox_version = "0.1"

[config]
samples = ["a", "b", "c", "d", "e", "f"]

[rule.all]
input = ["report/final.txt"]

[rule.leaf]
output = ["data/{sample}.txt"]
wildcard_constraints = { sample = "a|b|c|d|e|f" }
shell = """
mkdir -p data
sleep 0.6
echo "{sample}" > {output}
"""

[rule.sink]
input = ["data/{sample}.txt"]
output = ["report/final.txt"]
expand = "product"
shell = """
mkdir -p report
echo "=== sink ===" > {output}
for f in {input}; do cat "$f" >> {output}; done
echo "DONE" >> {output}
"""
EOF
    echo "$workdir"
}

# Read the set of job IDs that landed in a terminal status. Terminal =
# completed | failed | cancelled. Note `cached=1` is still status='completed'
# in the schema, so the SELECT below captures cache hits as well.
terminal_status_set() {
    local workdir="$1"
    local db="$workdir/.oxymake/state.db"
    if [ ! -f "$db" ]; then
        echo ""
        return 0
    fi
    sqlite3 "$db" \
        "SELECT id FROM jobs WHERE status IN ('completed','failed','cancelled') ORDER BY id"
}

# Read the set of job IDs that are still lingering in non-terminal status
# (pending or running). Under any healthy completion path this set must be
# empty after `ox run` exits.
nonterminal_status_set() {
    local workdir="$1"
    local db="$workdir/.oxymake/state.db"
    if [ ! -f "$db" ]; then
        echo ""
        return 0
    fi
    sqlite3 "$db" \
        "SELECT id, status FROM jobs WHERE status IN ('pending','running') ORDER BY id"
}

# ---------------------------------------------------------------------------
# Baseline — record the canonical terminal-status set.
# ---------------------------------------------------------------------------

run_baseline() {
    local workdir
    workdir=$(setup_workdir)

    log "baseline: cd $workdir"
    (cd "$workdir" && "$OX" run -j 4 >/dev/null 2>&1)
    local exit_code=$?

    if [ "$exit_code" -ne 0 ]; then
        log_fail "baseline: ox run exited with $exit_code (expected 0)"
        rm -rf "$workdir"
        exit 1
    fi

    local baseline
    baseline=$(terminal_status_set "$workdir")
    if [ -z "$baseline" ]; then
        log_fail "baseline: no jobs in state.db — fixture is broken"
        rm -rf "$workdir"
        exit 1
    fi

    local lingering
    lingering=$(nonterminal_status_set "$workdir")
    if [ -n "$lingering" ]; then
        log_fail "baseline: jobs still pending/running after exit — fixture is broken"
        log_fail "  lingering:"
        echo "$lingering" | sed 's/^/    /' >&2
        rm -rf "$workdir"
        exit 1
    fi

    rm -rf "$workdir"
    echo "$baseline"
}

# ---------------------------------------------------------------------------
# Chaos — SIGKILL a random worker mid-run and check convergence.
# ---------------------------------------------------------------------------

run_chaos() {
    local iteration="$1"
    local baseline="$2"
    local workdir
    workdir=$(setup_workdir)

    log "iteration $iteration: cd $workdir"

    # Start ox in the background. We use `exec` so the wrapping subshell
    # is replaced by the ox process itself — otherwise `$!` would be the
    # subshell's PID and `pgrep -P "$!"` would return ox (one level too
    # high) instead of ox's worker children, and chaos.sh would SIGKILL
    # ox by accident. Redirect everything so we don't pollute the test
    # log with the worker's stderr.
    (cd "$workdir" && exec "$OX" run -j 4 >/dev/null 2>&1) &
    local ox_pid=$!

    # Wait until at least one child worker (a `sh -c "..."` from
    # ox-exec-local) has appeared, then SIGKILL one at random. Poll
    # instead of blindly sleeping — the worker may not have spawned yet
    # on a slow CI runner.
    local children=""
    local waited=0
    local max_wait=80   # 80 × 0.05s = 4s budget
    while [ "$waited" -lt "$max_wait" ]; do
        # `pgrep -P $ox_pid` returns direct children of ox (workers).
        # On macOS and Linux the worker is `sh -c "<rule body>"` with
        # the ox process as parent. We poll briefly to avoid racing
        # against the initial workspace-prepare step.
        children=$(pgrep -P "$ox_pid" 2>/dev/null || true)
        if [ -n "$children" ]; then
            break
        fi
        sleep 0.05
        waited=$((waited + 1))
    done

    if [ -z "$children" ]; then
        log_fail "iteration $iteration: no worker children spawned within ${max_wait} ticks"
        kill -9 "$ox_pid" 2>/dev/null || true
        wait "$ox_pid" 2>/dev/null || true
        rm -rf "$workdir"
        return 1
    fi

    # Pick one child at random and SIGKILL it. `shuf -n 1` is on coreutils;
    # fall back to awk for macOS where shuf may be absent.
    local victim
    if command -v shuf >/dev/null 2>&1; then
        victim=$(echo "$children" | shuf -n 1)
    else
        victim=$(echo "$children" | awk 'BEGIN{srand()} {a[NR]=$0} END{print a[int(rand()*NR)+1]}')
    fi

    log "iteration $iteration: SIGKILL worker pid=$victim (parent ox=$ox_pid)"
    kill -9 "$victim" 2>/dev/null || true

    # Now wait for ox itself to exit. It may exit with non-zero (a worker
    # died, so the run failed) — that's fine; what we care about is whether
    # the scheduler converged every job to a terminal status.
    wait "$ox_pid" 2>/dev/null
    local ox_exit=$?

    log "iteration $iteration: ox exited with $ox_exit"

    # --- Invariant 1: no job left lingering ---------------------------------
    local lingering
    lingering=$(nonterminal_status_set "$workdir")
    if [ -n "$lingering" ]; then
        log_fail "iteration $iteration: jobs still pending/running after chaos:"
        echo "$lingering" | sed 's/^/    /' >&2
        rm -rf "$workdir"
        return 1
    fi

    # --- Invariant 2: terminal-status SET equals baseline -------------------
    # We compare the *set of IDs* that reached a terminal status, not the
    # status itself — chaos legitimately turns some Completed into
    # Failed/Cancelled.
    local chaos_set
    chaos_set=$(terminal_status_set "$workdir")
    if [ "$chaos_set" != "$baseline" ]; then
        log_fail "iteration $iteration: terminal-status set diverges from baseline"
        log_fail "  baseline:"
        echo "$baseline" | sed 's/^/    /' >&2
        log_fail "  chaos:"
        echo "$chaos_set" | sed 's/^/    /' >&2
        rm -rf "$workdir"
        return 1
    fi

    log_pass "iteration $iteration: terminal-status set ≡ baseline ($(echo "$baseline" | wc -l | tr -d ' ') jobs)"
    rm -rf "$workdir"
    return 0
}

# ---------------------------------------------------------------------------
# Driver
# ---------------------------------------------------------------------------

log "ox binary: $OX"
log "iterations: $ITERATIONS"

log "recording baseline ..."
BASELINE=$(run_baseline)
log_pass "baseline: $(echo "$BASELINE" | wc -l | tr -d ' ') jobs reached a terminal status"

failures=0
for ((i = 1; i <= ITERATIONS; i++)); do
    if ! run_chaos "$i" "$BASELINE"; then
        failures=$((failures + 1))
    fi
done

if [ "$failures" -gt 0 ]; then
    log_fail "CHAOS FAILED — $failures / $ITERATIONS iterations violated the invariant"
    exit 1
fi

log_pass "CHAOS PASSED — $ITERATIONS / $ITERATIONS iterations preserved the invariant"
