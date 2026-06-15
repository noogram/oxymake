#!/usr/bin/env bash
# Full rebuild: nuke all outputs, re-run rebuilds everything
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run

rm -rf data output report

$OX run

assert_file "data/x.csv"
assert_file "data/y.csv"
assert_file "data/z.csv"
assert_file "output/x_sorted.csv"
assert_file "report/final.txt"
assert_contains "report/final.txt" "DONE"
