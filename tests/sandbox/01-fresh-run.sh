#!/usr/bin/env bash
# Fresh run: init + lint + run produces all outputs
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX lint
$OX run -vv

assert_file "data/x.csv"
assert_file "data/y.csv"
assert_file "data/z.csv"
assert_file "output/x_sorted.csv"
assert_file "report/final.txt"
assert_contains "report/final.txt" "Sandbox Report"
assert_contains "report/final.txt" "DONE"
