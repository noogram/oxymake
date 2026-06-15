#!/usr/bin/env bash
# Plan output: ox plan shows execution plan without running anything
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true

# Plan should produce output
output=$($OX plan 2>&1)
[ -n "$output" ] || { echo "FAIL: ox plan produced no output"; exit 1; }

# JSON plan
output_json=$($OX plan --json 2>&1)
[ -n "$output_json" ] || { echo "FAIL: ox plan --json produced no output"; exit 1; }

# No files should have been created
assert_no_file "data/x.csv"
assert_no_file "report/final.txt"
