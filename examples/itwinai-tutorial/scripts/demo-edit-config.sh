#!/usr/bin/env bash
# Demo 3 — edit train.yaml: `train` and `eval` re-run.
#
# config/train.yaml is declared as an input of `train`. Editing it flips
# the cache key of `train`, which invalidates `eval` downstream.
# `prepare` is untouched.
set -euo pipefail

cd "$(dirname "$0")/.."

# Make sure the cache is warm.
ox run all

# Trivial in-place edit: append a no-op trailing comment.
printf '\n# nudge: %s\n' "$(date +%s)" >> config/train.yaml

ox run all

echo
echo "Expected: \`train\` and \`eval\` re-executed; \`prepare\` was a cache hit."
