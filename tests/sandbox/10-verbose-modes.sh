#!/usr/bin/env bash
# Verbose modes: plain, -v, and -vv all complete successfully
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true

$OX run
rm -rf data output report

$OX run -v
rm -rf data output report

$OX run -vv
assert_file "report/final.txt"
