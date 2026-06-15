#!/usr/bin/env bash
# Shared helpers for sandbox scenarios

set -euo pipefail

SCRIPT_DIR="${SCRIPT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)}"
REPO_ROOT="${REPO_ROOT:-$(cd "$SCRIPT_DIR/../.." && pwd)}"
OX="${OX:-$REPO_ROOT/target/debug/ox}"

# Create an isolated workdir with the sandbox Oxymakefile
sandbox_setup() {
    local workdir
    workdir=$(mktemp -d "${TMPDIR:-/tmp}/oxymake-sandbox.XXXXXX")
    cp "$SCRIPT_DIR/Oxymakefile.toml" "$workdir/"
    echo "$workdir"
}

sandbox_cleanup() {
    rm -rf "$1" 2>/dev/null || true
}

# Assert a file exists
assert_file() {
    [ -f "$1" ] || { echo "FAIL: $1 missing"; exit 1; }
}

# Assert a file does NOT exist
assert_no_file() {
    [ ! -f "$1" ] || { echo "FAIL: $1 should not exist"; exit 1; }
}

# Assert file contains a string
assert_contains() {
    grep -q "$2" "$1" || { echo "FAIL: $1 does not contain '$2'"; exit 1; }
}
