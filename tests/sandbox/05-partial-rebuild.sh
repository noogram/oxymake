#!/usr/bin/env bash
# Partial rebuild: delete one output, re-run rebuilds only affected chain
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run

# Delete one intermediate + downstream
rm -f data/x.csv output/x_sorted.csv report/final.txt

$OX run

assert_file "data/x.csv"
assert_file "output/x_sorted.csv"
assert_file "report/final.txt"
assert_contains "report/final.txt" "Sandbox Report"
