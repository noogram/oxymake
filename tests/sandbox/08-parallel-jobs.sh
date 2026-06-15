#!/usr/bin/env bash
# Parallel jobs: -j1 and -j4 both produce correct outputs
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true

# Sequential
$OX run -j 1
assert_file "report/final.txt"
assert_contains "report/final.txt" "DONE"

rm -rf data output report

# Parallel
$OX run -j 4
assert_file "report/final.txt"
assert_contains "report/final.txt" "DONE"
