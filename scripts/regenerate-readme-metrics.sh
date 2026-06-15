#!/usr/bin/env bash
# Regenerate metric-driven regions of README.md from metrics/metrics.json.
#
# This is a README consumer of the canonical metrics file (ADR-016).
# It rewrites content between tagged anchor blocks ; prose around the
# anchors is operator-owned and never touched.
#
# Anchor syntax (HTML comments are invisible in rendered Markdown) :
#
#   <!-- METRICS:BEGIN <metric-key> -->
#   <managed line, rewritten by this script>
#   <!-- METRICS:END -->
#
# Adding a new managed metric line :
#   1. Add the metric to metrics/metrics.json (via this or the bench script).
#   2. Wrap the line in README.md with the BEGIN/END comments.
#   3. Add a render case in render_line() below.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

METRICS_JSON="metrics/metrics.json"
README="README.md"

if [ ! -f "$METRICS_JSON" ]; then
    echo "regenerate-readme-metrics: $METRICS_JSON missing — run scripts/regenerate-metrics.sh first" >&2
    exit 1
fi

if [ ! -f "$README" ]; then
    echo "regenerate-readme-metrics: $README missing — nothing to rewrite" >&2
    exit 1
fi

python3 scripts/regenerate-readme-metrics.py "$METRICS_JSON" "$README"
