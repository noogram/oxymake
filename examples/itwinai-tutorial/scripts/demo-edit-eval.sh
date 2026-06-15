#!/usr/bin/env bash
# Demo 2 — edit eval.py: only `eval` re-runs.
#
# Touching the rule's *source* (a declared input of `eval`) flips its
# content hash; `prepare` and `train` are unaffected.
set -euo pipefail

cd "$(dirname "$0")/.."

# Make sure the cache is warm.
ox run all

# Trivial in-place edit: append a trailing newline.
printf '\n' >> src/eval.py

ox run all

echo
echo "Expected: only \`eval\` re-executed; \`prepare\` and \`train\` were cache hits."
