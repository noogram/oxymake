#!/usr/bin/env bash
# OxyMake UX Sandbox — run individual scenario scripts
#
# Usage:
#   bash tests/sandbox/run-sandbox.sh                # Run all 12 scenarios
#   bash tests/sandbox/run-sandbox.sh 01-fresh-run   # Run one scenario
#   bash tests/sandbox/run-sandbox.sh --list          # List available scenarios
#
# Environment:
#   OX=path/to/ox   Override ox binary (default: target/debug/ox)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

OX="${OX:-$REPO_ROOT/target/debug/ox}"
export OX SCRIPT_DIR REPO_ROOT

# Colors
if [ -t 1 ]; then
    BOLD='\033[1m' RED='\033[31m' GREEN='\033[32m'
    CYAN='\033[36m' DIM='\033[2m' RESET='\033[0m'
else
    BOLD='' RED='' GREEN='' CYAN='' DIM='' RESET=''
fi

PASS=0 FAIL=0 TOTAL_START=$(date +%s)
RESULTS=()

list_scenarios() {
    echo "Available sandbox scenarios:"
    echo ""
    for script in "$SCRIPT_DIR"/[0-9][0-9]-*.sh; do
        name=$(basename "$script" .sh)
        desc=$(head -2 "$script" | tail -1 | sed 's/^#\s*//')
        printf "  %-35s %s\n" "$name" "$desc"
    done
}

run_one() {
    local script="$1"
    local name
    name=$(basename "$script" .sh)
    local start logfile
    start=$(date +%s)
    logfile=$(mktemp "${TMPDIR:-/tmp}/sandbox-log.XXXXXX")

    printf "${BOLD}${CYAN}[SANDBOX]${RESET} %-40s " "$name"

    if bash "$script" >"$logfile" 2>&1; then
        local elapsed=$(( $(date +%s) - start ))
        printf "${GREEN}PASS${RESET} (%ds)\n" "$elapsed"
        PASS=$((PASS + 1))
        RESULTS+=("PASS|${elapsed}s|$name")
    else
        local exit_code=$?
        local elapsed=$(( $(date +%s) - start ))
        printf "${RED}FAIL${RESET} (%ds, exit %d)\n" "$elapsed" "$exit_code"
        FAIL=$((FAIL + 1))
        RESULTS+=("FAIL|${elapsed}s|$name")
        echo -e "${DIM}--- output ---${RESET}"
        tail -30 "$logfile" | sed 's/^/  /'
        echo -e "${DIM}--- end ---${RESET}"
    fi
    rm -f "$logfile"
}

main() {
    local filter="${1:-}"

    if [ "$filter" = "--list" ] || [ "$filter" = "-l" ]; then
        list_scenarios
        exit 0
    fi

    if [ ! -x "$OX" ]; then
        echo -e "${RED}ERROR: ox binary not found at $OX${RESET}"
        echo "Build first: cargo build --bin ox"
        exit 1
    fi

    echo -e "${BOLD}${CYAN}--- OxyMake UX Sandbox ---${RESET}"
    echo -e "Binary: ${DIM}$OX${RESET}"
    echo ""

    for script in "$SCRIPT_DIR"/[0-9][0-9]-*.sh; do
        local name
        name=$(basename "$script" .sh)
        if [ -n "$filter" ] && [ "$name" != "$filter" ]; then
            continue
        fi
        run_one "$script"
    done

    local total_elapsed=$(( $(date +%s) - TOTAL_START ))
    echo ""
    echo -e "${BOLD}--- Sandbox Report ---${RESET}"
    printf "  %-10s %-8s %s\n" "STATUS" "TIME" "SCENARIO"
    printf "  %-10s %-8s %s\n" "------" "----" "--------"
    for result in "${RESULTS[@]}"; do
        IFS='|' read -r status elapsed name <<< "$result"
        local color="$GREEN"
        [ "$status" = "FAIL" ] && color="$RED"
        printf "  ${color}%-10s${RESET} %-8s %s\n" "$status" "$elapsed" "$name"
    done
    echo ""
    echo -e "  ${GREEN}Pass: $PASS${RESET}  ${RED}Fail: $FAIL${RESET}  Total: ${total_elapsed}s"
    echo ""

    if [ "$FAIL" -gt 0 ]; then
        echo -e "${RED}${BOLD}SANDBOX FAILED${RESET} — $FAIL scenario(s) failed"
        exit 1
    else
        echo -e "${GREEN}${BOLD}SANDBOX PASSED${RESET} — all $PASS scenarios passed"
    fi
}

main "$@"
