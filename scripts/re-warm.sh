#!/usr/bin/env bash
# re-warm.sh — Oxymake mechanical canary.
#
# Five invariants, plain English, fail fast:
#   I-build           cargo check + clippy -D warnings
#   I-tests-green     cargo test --workspace, ≥ MIN_TESTS pass
#   I-demo-runs       examples/demo end-to-end, < 60 s wallclock
#   I-baseline-stable hash of structural plan ≡ baselines/demo.sha256
#   I-rederive        wipe .oxymake/, re-run plan, decisions identical
#
# Budget: < 2 minutes wallclock on a warm cargo cache. Slower than that
# means something rotted in the build graph and the canary itself is
# the rot signal — do not "fix" by relaxing the budget.
#
# First execution creates `baselines/demo.sha256`. Subsequent executions
# verify against it. Bumping the baseline is a deliberate gesture
# (delete the file, re-run, commit the new hash in the same change).
#
# Exit codes:
#   0  every invariant holds (green)
#   1  one or more invariants failed (red — action required)
#   2  harness error (toolchain missing, can't reach repo root, ...)
#
# Run from anywhere; the script discovers the repo root by walking up.
# Honour OX_RE_WARM_SKIP_TESTS=1 only for live debugging of the script
# itself — CI must never set it.

set -uo pipefail

# --- Plumbing ----------------------------------------------------------------

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT" || { echo "fatal: cannot cd to repo root" >&2; exit 2; }

if [ ! -f "Cargo.toml" ]; then
    echo "fatal: $REPO_ROOT is not a Cargo workspace root" >&2
    exit 2
fi

BASELINE_DIR="$REPO_ROOT/baselines"
BASELINE_FILE="$BASELINE_DIR/demo.sha256"
DEMO_DIR="$REPO_ROOT/examples/demo"
OX_BIN="$REPO_ROOT/target/debug/ox"

# Colours (skip if not a terminal — log harvesting wants plain text)
if [ -t 1 ]; then
    BOLD=$'\033[1m' RED=$'\033[31m' GREEN=$'\033[32m' YELLOW=$'\033[33m' DIM=$'\033[2m' RESET=$'\033[0m'
else
    BOLD='' RED='' GREEN='' YELLOW='' DIM='' RESET=''
fi

STATUS=()      # per-invariant verdict lines, printed at the end
FAILURES=0

start_time=$(date +%s)

step() {
    local name="$1"
    echo ""
    echo "${BOLD}── $name ──${RESET}"
}

record() {
    # record <invariant> <verdict> <detail>
    local inv="$1" verdict="$2" detail="$3"
    case "$verdict" in
        pass) STATUS+=("${GREEN}✓${RESET} $inv  $detail") ;;
        fail) STATUS+=("${RED}✗${RESET} $inv  $detail"); FAILURES=$((FAILURES + 1)) ;;
        skip) STATUS+=("${YELLOW}–${RESET} $inv  $detail") ;;
    esac
}

require_tool() {
    command -v "$1" >/dev/null 2>&1 || { echo "fatal: missing $1" >&2; exit 2; }
}

require_tool cargo
require_tool python3
require_tool shasum

# --- I-build -----------------------------------------------------------------

step "I-build  (cargo check + clippy -D warnings)"

if ! cargo check --workspace --quiet 2>&1; then
    record "I-build" fail "cargo check failed"
else
    if ! cargo clippy --workspace --quiet --no-deps -- -D warnings 2>&1; then
        record "I-build" fail "clippy emitted warnings (denied)"
    else
        record "I-build" pass "cargo check + clippy -D warnings clean"
    fi
fi

# --- I-tests-green -----------------------------------------------------------

step "I-tests-green  (cargo test --workspace, ≥ \$MIN_TESTS)"

# Minimum-test threshold — set well below the current count so a silent
# reduction (someone deletes a test module without noticing) trips the
# canary. Bump deliberately when tests are added on purpose.
MIN_TESTS="${OX_MIN_TESTS:-1500}"

if [ "${OX_RE_WARM_SKIP_TESTS:-0}" = "1" ]; then
    record "I-tests-green" skip "OX_RE_WARM_SKIP_TESTS=1 (debug only — CI must NOT set this)"
else
    test_log=$(mktemp)
    if ! cargo test --workspace --quiet >"$test_log" 2>&1; then
        echo "${DIM}--- last 40 lines of cargo test output ---${RESET}"
        tail -40 "$test_log"
        record "I-tests-green" fail "cargo test --workspace failed"
    else
        # Count tests across all binaries: lines like "test result: ok. N passed; ..."
        passed=$(grep -E '^test result:' "$test_log" \
            | awk -F'[; ]+' '{for(i=1;i<=NF;i++) if($i=="passed") print $(i-1)}' \
            | awk '{s+=$1} END {print s+0}')
        if [ "$passed" -lt "$MIN_TESTS" ]; then
            record "I-tests-green" fail \
                "$passed passed (< MIN_TESTS=$MIN_TESTS — silent reduction?)"
        else
            record "I-tests-green" pass "$passed tests passed (≥ $MIN_TESTS)"
        fi
    fi
    rm -f "$test_log"
fi

# --- I-demo-runs -------------------------------------------------------------

step "I-demo-runs  (examples/demo end-to-end, < 60 s)"

if [ ! -x "$OX_BIN" ]; then
    if ! cargo build --quiet --bin ox 2>&1; then
        record "I-demo-runs" fail "cargo build --bin ox failed"
        OX_BIN=""
    fi
fi

DEMO_TMP=""
if [ -n "$OX_BIN" ] && [ -x "$OX_BIN" ]; then
    DEMO_TMP=$(mktemp -d)
    cp "$DEMO_DIR/Oxymakefile.toml" "$DEMO_TMP/"

    demo_start=$(date +%s)
    demo_log=$(mktemp)
    if ( cd "$DEMO_TMP" && "$OX_BIN" run --json ) >"$demo_log" 2>&1; then
        demo_elapsed=$(( $(date +%s) - demo_start ))
        if [ "$demo_elapsed" -gt 60 ]; then
            record "I-demo-runs" fail "demo took ${demo_elapsed}s (> 60s budget)"
        else
            record "I-demo-runs" pass "demo ran end-to-end in ${demo_elapsed}s"
        fi
    else
        echo "${DIM}--- demo stderr (last 30 lines) ---${RESET}"
        tail -30 "$demo_log"
        record "I-demo-runs" fail "demo execution failed"
    fi
    rm -f "$demo_log"
fi

# --- I-baseline-stable -------------------------------------------------------

step "I-baseline-stable  (hash of structural plan ≡ baseline)"

# Structural fingerprint: sorted job IDs + their inputs/outputs/depends_on.
# Deliberately ignores wallclock-sensitive fields (timing, run ids) and
# input *content* (the demo seeds with $RANDOM). What we are checking is:
# "the DAG resolves to the same shape it did before". A change here is
# either intentional (workflow edit) or a regression (resolver bug).
fingerprint=""
if [ -z "$OX_BIN" ] || [ ! -x "$OX_BIN" ]; then
    record "I-baseline-stable" skip "ox binary unavailable (see I-demo-runs)"
elif [ -z "$DEMO_TMP" ] || [ ! -d "$DEMO_TMP" ]; then
    record "I-baseline-stable" skip "demo workspace unavailable"
else
    plan_json=$(mktemp)
    if ! ( cd "$DEMO_TMP" && "$OX_BIN" plan --json ) >"$plan_json" 2>&1; then
        record "I-baseline-stable" fail "ox plan --json failed on demo"
    else
        fingerprint=$(python3 - "$plan_json" <<'PY'
import hashlib, json, sys
with open(sys.argv[1]) as f:
    plan = json.load(f)
projection = {
    "job_count": plan.get("job_count"),
    "jobs": sorted(
        [
            {
                "job_id": j["job_id"],
                "inputs": sorted(j.get("inputs", [])),
                "outputs": sorted(j.get("outputs", [])),
                "depends_on": sorted(j.get("depends_on", [])),
            }
            for j in plan.get("jobs", [])
        ],
        key=lambda j: j["job_id"],
    ),
}
blob = json.dumps(projection, sort_keys=True, separators=(",", ":")).encode()
print(hashlib.sha256(blob).hexdigest())
PY
        )
        if [ -z "$fingerprint" ]; then
            record "I-baseline-stable" fail "failed to compute structural fingerprint"
        elif [ ! -f "$BASELINE_FILE" ]; then
            mkdir -p "$BASELINE_DIR"
            printf '%s  demo-plan-structural\n' "$fingerprint" >"$BASELINE_FILE"
            record "I-baseline-stable" pass \
                "baseline minted (${fingerprint:0:12}…) → $(printf '%s' "${BASELINE_FILE#$REPO_ROOT/}")"
        else
            expected=$(awk '{print $1}' "$BASELINE_FILE")
            if [ "$fingerprint" = "$expected" ]; then
                record "I-baseline-stable" pass \
                    "fingerprint matches baseline (${fingerprint:0:12}…)"
            else
                record "I-baseline-stable" fail \
                    "drift: expected ${expected:0:12}…, got ${fingerprint:0:12}… — workflow edit? regression?"
            fi
        fi
    fi
    rm -f "$plan_json"
fi

# --- I-rederive --------------------------------------------------------------

step "I-rederive  (wipe .oxymake/, re-run plan, decisions identical)"

if [ -z "$OX_BIN" ] || [ ! -x "$OX_BIN" ]; then
    record "I-rederive" skip "ox binary unavailable"
elif [ -z "$DEMO_TMP" ] || [ ! -d "$DEMO_TMP" ]; then
    record "I-rederive" skip "demo workspace unavailable"
elif [ -z "$fingerprint" ]; then
    record "I-rederive" skip "no fingerprint to re-derive against"
else
    # Wipe persisted state, re-run plan, compare fingerprint.
    rm -rf "$DEMO_TMP/.oxymake"
    plan_json2=$(mktemp)
    if ! ( cd "$DEMO_TMP" && "$OX_BIN" plan --json ) >"$plan_json2" 2>&1; then
        record "I-rederive" fail "ox plan --json failed after state wipe"
    else
        rederived=$(python3 - "$plan_json2" <<'PY'
import hashlib, json, sys
with open(sys.argv[1]) as f:
    plan = json.load(f)
projection = {
    "job_count": plan.get("job_count"),
    "jobs": sorted(
        [
            {
                "job_id": j["job_id"],
                "inputs": sorted(j.get("inputs", [])),
                "outputs": sorted(j.get("outputs", [])),
                "depends_on": sorted(j.get("depends_on", [])),
            }
            for j in plan.get("jobs", [])
        ],
        key=lambda j: j["job_id"],
    ),
}
blob = json.dumps(projection, sort_keys=True, separators=(",", ":")).encode()
print(hashlib.sha256(blob).hexdigest())
PY
        )
        if [ "$rederived" = "$fingerprint" ]; then
            record "I-rederive" pass "plan re-derived from scratch matches"
        else
            record "I-rederive" fail \
                "post-wipe fingerprint ${rederived:0:12}… ≠ pre-wipe ${fingerprint:0:12}…"
        fi
    fi
    rm -f "$plan_json2"
fi

[ -n "$DEMO_TMP" ] && [ -d "$DEMO_TMP" ] && rm -rf "$DEMO_TMP"

# --- Summary -----------------------------------------------------------------

elapsed=$(( $(date +%s) - start_time ))

echo ""
echo "${BOLD}── re-warm summary ──${RESET}"
for line in "${STATUS[@]}"; do
    echo "  $line"
done
echo ""

budget_msg=""
if [ "$elapsed" -gt 120 ]; then
    budget_msg=" ${YELLOW}(over 2-min budget — build graph rot?)${RESET}"
fi
echo "Wallclock: ${elapsed}s${budget_msg}"

if [ "$FAILURES" -eq 0 ]; then
    echo "${GREEN}${BOLD}re-warm OK${RESET} — every invariant holds."
    exit 0
else
    echo "${RED}${BOLD}re-warm RED${RESET} — $FAILURES invariant(s) failed; do not ship."
    exit 1
fi
