#!/usr/bin/env bash
# run.sh — OX-8 R0 orthogonality attestation, 2×2 matrix runner.
#
# Provenance:
#   - orthogonality benchmark r0  (parent finding)
#   - popper §0                (R0 protocol)
#   - task-20260527-8333       (this bench)
#   - docs/attestations/ox8-r0.md  (attestation document)
#
# Protocol (popper §0):
#   Run `ox run` on workflow W over the 2×2 matrix
#     { output stable, output drift } × { store-hash stable, store-hash drift }
#   For each cell, observe:
#     1. OX-1 verdict   — did the second invocation re-execute the rule
#                         (cache miss) or skip it (cache hit)?
#     2. Guix verdict   — did the resolved /gnu/store path for `cat`
#                         change between the two invocations?
#   Orthogonality holds iff OX-1's verdict depends only on the output
#   column and the Guix verdict only on the store-hash row. Any cell
#   where both verdicts collapse together refutes orthogonality.
#
# Operationally:
#   - "output stable / drift" is driven by changing input.txt between
#     the two invocations within a cell (or not). The rule `cat`s
#     input.txt into emit.txt; changing input.txt changes the cache
#     key over inputs, which is OX-1's load-bearing axis.
#   - "store-hash stable / drift" is driven by swapping the Guix
#     manifest between the two invocations. The base manifest pins
#     `coreutils`; the drift manifest pins `coreutils-minimal` — same
#     `cat` semantics, distinct /gnu/store/<hash> for the binary.
#
# Output:
#   results/results.tsv — one row per cell with the 4 captured
#   observables (rerun?, store_path_a, store_path_b, store_drift?).
#   docs/attestations/ox8-r0.md must be filled in from this file.
#
# Usage:
#   ./bench/orthogonality-r0/run.sh [WORK_DIR]
#
# Requires `ox` in PATH. Requires `guix` in PATH to run the matrix
# end-to-end; without `guix`, the script emits a dry-run skeleton
# (cell labels + commands that would be run) so the attestation
# template is still grounded against the script that would fill it.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${1:-$SCRIPT_DIR/work}"
RESULTS_DIR="$SCRIPT_DIR/results"

mkdir -p "$WORK_DIR" "$RESULTS_DIR"

# ---------------------------------------------------------------------------
# Preconditions
# ---------------------------------------------------------------------------
if ! command -v ox >/dev/null 2>&1; then
    echo "FATAL: ox not in PATH. Install OxyMake before running R0." >&2
    exit 2
fi

HAVE_GUIX=1
if ! command -v guix >/dev/null 2>&1; then
    HAVE_GUIX=0
    echo "WARNING: guix not in PATH — emitting dry-run skeleton only." >&2
    echo "         The attestation cannot be filled in without Guix." >&2
fi

# ---------------------------------------------------------------------------
# Materialise the workspace
# ---------------------------------------------------------------------------
cp "$SCRIPT_DIR/workflow.toml"  "$WORK_DIR/workflow.toml"
cp "$SCRIPT_DIR/manifest.scm"   "$WORK_DIR/manifest-a.scm"

# Derive manifest-b.scm from manifest-a.scm: swap coreutils for
# coreutils-minimal. Both provide `cat`; they have distinct store
# hashes. This is the store-hash drift mechanism.
sed 's/"coreutils"/"coreutils-minimal"/' "$WORK_DIR/manifest-a.scm" \
    > "$WORK_DIR/manifest-b.scm"

cd "$WORK_DIR"

# ---------------------------------------------------------------------------
# Per-cell driver
# ---------------------------------------------------------------------------
# run_cell <label> <payload_first> <payload_second> <manifest_first> <manifest_second>
#
# Writes a per-cell directory under "$RESULTS_DIR/$label/" containing:
#   inv1.log     — stdout of the first  ox run
#   inv2.log     — stdout of the second ox run
#   store_a.txt  — realpath of `cat` under manifest_first
#   store_b.txt  — realpath of `cat` under manifest_second
#   verdict.tsv  — single line: label \t rerun? \t store_drift?
run_cell() {
    local label="$1" p1="$2" p2="$3" m1="$4" m2="$5"
    local cell_dir="$RESULTS_DIR/$label"
    mkdir -p "$cell_dir"

    # Fresh per-cell scratch so cache state does not leak across cells.
    local sandbox="$WORK_DIR/sandbox-$label"
    rm -rf "$sandbox"
    mkdir -p "$sandbox"
    cp workflow.toml "$sandbox/"
    cp "$m1" "$sandbox/M1.scm"
    cp "$m2" "$sandbox/M2.scm"

    (
        cd "$sandbox"
        echo "$p1" > input.txt

        if [ "$HAVE_GUIX" = 1 ]; then
            guix shell -m M1.scm -- realpath "$(guix shell -m M1.scm -- which cat)" \
                > "$cell_dir/store_a.txt" 2>&1 || true
            guix shell -m M1.scm -- ox run -f workflow.toml \
                > "$cell_dir/inv1.log" 2>&1 || true

            # Drift step: payload and/or manifest as specified by the cell.
            echo "$p2" > input.txt
            guix shell -m M2.scm -- realpath "$(guix shell -m M2.scm -- which cat)" \
                > "$cell_dir/store_b.txt" 2>&1 || true
            guix shell -m M2.scm -- ox run -f workflow.toml \
                > "$cell_dir/inv2.log" 2>&1 || true
        else
            # Dry-run: record commands that would have run; no observations.
            cat > "$cell_dir/inv1.log" <<EOF
DRY-RUN (no guix in PATH).
Would have run:
  echo "$p1" > input.txt
  guix shell -m M1.scm -- ox run -f workflow.toml
EOF
            cat > "$cell_dir/inv2.log" <<EOF
DRY-RUN (no guix in PATH).
Would have run:
  echo "$p2" > input.txt
  guix shell -m M2.scm -- ox run -f workflow.toml
EOF
            echo "DRY-RUN — guix unavailable" > "$cell_dir/store_a.txt"
            echo "DRY-RUN — guix unavailable" > "$cell_dir/store_b.txt"
        fi
    )

    # Extract verdicts. OX-1 verdict = did inv2 re-execute the rule?
    #   - "rerun"    iff inv2.log shows a successful execution line.
    #   - "no-rerun" iff inv2.log shows the cache-skip line.
    # Guix verdict = did the realpath of `cat` differ between M1 and M2?
    local rerun store_drift
    if grep -q "Cache: 1 of 1 job" "$cell_dir/inv2.log" 2>/dev/null; then
        rerun="no-rerun"
    elif grep -q "Completed 1/1" "$cell_dir/inv2.log" 2>/dev/null; then
        rerun="rerun"
    else
        rerun="indeterminate"
    fi

    if [ "$HAVE_GUIX" = 1 ] \
        && [ "$(cat "$cell_dir/store_a.txt")" = "$(cat "$cell_dir/store_b.txt")" ]; then
        store_drift="stable"
    elif [ "$HAVE_GUIX" = 1 ]; then
        store_drift="drift"
    else
        store_drift="dry-run"
    fi

    printf "%s\t%s\t%s\n" "$label" "$rerun" "$store_drift" \
        > "$cell_dir/verdict.tsv"
    printf "%s\t%s\t%s\n" "$label" "$rerun" "$store_drift"
}

# ---------------------------------------------------------------------------
# The 2×2 matrix
#
# Cell (1) — output stable, store stable: payload unchanged, manifest unchanged.
# Cell (2) — output drift,  store stable: payload changes,  manifest unchanged.
# Cell (3) — output stable, store drift:  payload unchanged, manifest changes.
# Cell (4) — output drift,  store drift:  payload changes,   manifest changes.
#
# Predicted orthogonal pattern:
#   (1) no-rerun, stable
#   (2)    rerun, stable
#   (3) no-rerun, drift
#   (4)    rerun, drift
# ---------------------------------------------------------------------------
{
    printf "cell\trerun\tstore\n"
    run_cell cell1-stable-stable alpha alpha manifest-a.scm manifest-a.scm
    run_cell cell2-drift-stable  alpha beta  manifest-a.scm manifest-a.scm
    run_cell cell3-stable-drift  alpha alpha manifest-a.scm manifest-b.scm
    run_cell cell4-drift-drift   alpha beta  manifest-a.scm manifest-b.scm
} | tee "$RESULTS_DIR/results.tsv"

echo
echo "Results: $RESULTS_DIR/results.tsv"
echo "Fill in docs/attestations/ox8-r0.md from these observations."
if [ "$HAVE_GUIX" = 0 ]; then
    echo
    echo "NOTE: dry-run only. R0 cannot be attested without Guix." >&2
    exit 3
fi
