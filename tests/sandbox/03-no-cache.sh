#!/usr/bin/env bash
# No-cache: --no-cache forces re-execution of all jobs
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run

# Re-run with --no-cache — jobs must re-execute
output=$($OX run --no-cache --json 2>&1)
started=$(echo "$output" | grep -c '"event":"job_started"' || true)
if [ "$started" -eq 0 ]; then
    echo "FAIL: --no-cache produced 0 started jobs"
    echo "$output"
    exit 1
fi

assert_file "report/final.txt"
