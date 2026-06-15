#!/usr/bin/env bash
# Demo 1 — cold cache: every rule executes.
set -euo pipefail

cd "$(dirname "$0")/.."

ox clean || true
ox run all

echo
echo "Expected: 3 rules executed (prepare + train + eval)."
echo "Re-run \`ox run all\` and you should see 3 cache hits."
