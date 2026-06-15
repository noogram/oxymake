#!/usr/bin/env bash
# Cache hit: second run executes zero jobs
source "$(dirname "$0")/helpers.sh"

workdir=$(sandbox_setup)
trap "sandbox_cleanup '$workdir'" EXIT
cd "$workdir"

$OX init 2>/dev/null || true
$OX run

# Second run — everything cached
output=$($OX run --json 2>&1)
started=$(echo "$output" | grep -c '"status":"started"' || true)
if [ "$started" -gt 0 ]; then
    echo "FAIL: cache miss — $started jobs re-ran on second run"
    echo "$output"
    exit 1
fi
