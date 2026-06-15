#!/usr/bin/env bash
# Dry run: --dry-run shows plan but creates no files
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run --dry-run

assert_no_file "data/x.csv"
assert_no_file "output/x_sorted.csv"
assert_no_file "report/final.txt"
