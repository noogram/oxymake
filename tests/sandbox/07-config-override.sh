#!/usr/bin/env bash
# Config override: --set out_dir redirects transform outputs
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run --set out_dir="custom_out"

assert_file "custom_out/x_sorted.csv"
assert_file "custom_out/y_sorted.csv"
assert_file "custom_out/z_sorted.csv"
assert_file "report/final.txt"
