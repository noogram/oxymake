#!/usr/bin/env bash
# spec/tla/run-tlc.sh — reproducible TLC model-checking for OxyMake specs.
#
# Every number the paper quotes about the TLA+ suite (state counts,
# search depth, invariants checked) must be reproducible from this
# script (premortem finding H19). It pins the TLC version by sha256,
# runs each committed configuration, and archives the output under
# spec/tla/runs/ — the committed reference outputs live there.
#
# Usage:
#   ./run-tlc.sh            # green suite: all shipped configs, must pass
#   ./run-tlc.sh --red      # red suite: falsifiability witnesses, must FAIL
#   ./run-tlc.sh --all      # both
#
# Requirements: java 11+ (e.g. `brew install openjdk`), curl.

set -euo pipefail
cd "$(dirname "$0")"

TLA_VERSION="1.7.4"
TLA_URL="https://github.com/tlaplus/tlaplus/releases/download/v${TLA_VERSION}/tla2tools.jar"
TLA_SHA256="936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88"
CACHE_DIR="${TLC_CACHE_DIR:-.tlc-cache}"
JAR="$CACHE_DIR/tla2tools-${TLA_VERSION}.jar"
RUNS_DIR="runs"

# --- locate java -----------------------------------------------------------
find_java() {
    if [ -n "${JAVA:-}" ] && "$JAVA" -version >/dev/null 2>&1; then
        echo "$JAVA"; return
    fi
    if java -version >/dev/null 2>&1; then
        echo "java"; return
    fi
    for brew_jdk in /opt/homebrew/opt/openjdk*/bin/java /usr/local/opt/openjdk*/bin/java; do
        if [ -x "$brew_jdk" ]; then echo "$brew_jdk"; return; fi
    done
    echo "error: no java runtime found (brew install openjdk, or set JAVA=...)" >&2
    exit 1
}
JAVA_BIN="$(find_java)"

# --- fetch + verify TLC ----------------------------------------------------
if [ ! -f "$JAR" ]; then
    mkdir -p "$CACHE_DIR"
    echo "fetching tla2tools.jar v${TLA_VERSION}..."
    curl -sSL -o "$JAR" "$TLA_URL"
fi
actual_sha="$(shasum -a 256 "$JAR" | cut -d' ' -f1)"
if [ "$actual_sha" != "$TLA_SHA256" ]; then
    echo "error: tla2tools.jar sha256 mismatch (got $actual_sha)" >&2
    exit 1
fi

# --- run one (module, config) pair ----------------------------------------
# $3 = "green" (must pass) or "red" (must report a violated invariant).
run_one() {
    local module="$1" config="$2" expect="$3"
    local out="$RUNS_DIR/${config%.cfg}.out"
    local scratch
    scratch="$(mktemp -d)"
    mkdir -p "$RUNS_DIR"
    echo "==> TLC $module ($config, expect $expect)"
    set +e
    "$JAVA_BIN" -XX:+UseParallelGC -cp "$JAR" tlc2.TLC \
        -deadlock -workers auto -metadir "$scratch" \
        -config "$config" "$module" > "$out" 2>&1
    local rc=$?
    set -e
    rm -rf "$scratch"
    if [ "$expect" = "green" ]; then
        if [ $rc -ne 0 ]; then
            echo "FAIL: $config was expected to pass (see $out)" >&2
            tail -5 "$out" >&2
            return 1
        fi
        grep -E "distinct states|depth of the complete state graph" "$out" | sed 's/^/    /'
    else
        if ! grep -q "Invariant.*is violated" "$out"; then
            echo "FAIL: $config was expected to violate an invariant (see $out)" >&2
            return 1
        fi
        grep -E "Invariant.*is violated" "$out" | sed 's/^/    /'
    fi
}

green() {
    run_one CacheConsistency.tla  CacheConsistency.cfg   green
    run_one CooperativeClaim.tla  CooperativeClaim.cfg   green
    run_one CancelPropagation.tla CancelPropagation.cfg  green
}

red() {
    run_one CacheConsistency.tla  CacheConsistencyNondetKey.cfg  red
    run_one CooperativeClaim.tla  CooperativeClaimUnguarded.cfg  red
}

case "${1:-}" in
    --red) red ;;
    --all) green; red ;;
    "")    green ;;
    *) echo "usage: $0 [--red|--all]" >&2; exit 2 ;;
esac

echo "OK"
