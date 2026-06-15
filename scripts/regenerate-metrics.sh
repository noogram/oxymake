#!/usr/bin/env bash
# Regenerate metrics/metrics.json from live workspace measurements.
#
# This is the workspace-level canonical metrics producer (ADR-016).
# Downstream surfaces (paper, README, benchmark RESULTS) read from
# metrics/metrics.json via their own thin renderers.
#
# Schema (per ADR-016) :
#   {
#     "<metric_key>": {
#       "value":       <number or string>,
#       "unit":        "<unit>",
#       "source":      "<command or script that produced this>",
#       "measured_at": "<UTC date YYYY-MM-DD>"
#     },
#     ...
#   }
#
# Optional fields per metric :
#   "hardware":   { "cpu": "...", "ram_gib": ..., "os": "..." }  // when perf-related
#   "method":     "median-of-N" | "mean" | "single-shot"
#   "n_runs":     <int>                                          // for perf
#
# Sources of truth (one per structural metric) :
#   sloc            tokei crates/ -t rust          (Rust "code" column, blanks excluded)
#   tests           cargo test --workspace          (sum of "test result:" pass counts)
#   crates          ls crates/                      (one directory per crate)
#   commits         git log --oneline               (commit count on HEAD)
#   dev_days        git log unique commit dates     (calendar days with at least one commit)
#   doc_files       find docs/ -name '*.md'         (markdown files under docs/)
#
# Perf metrics (e.g. dag_resolution_10k_ms) are written by the bench
# harness ; this script preserves any pre-existing perf entries when
# regenerating structural counts.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$ROOT"

err() { printf 'regenerate-metrics: %s\n' "$*" >&2; }

require() {
    if ! command -v "$1" >/dev/null 2>&1; then
        err "required tool '$1' not found in PATH"
        exit 1
    fi
}

require tokei
require jq
require cargo
require git

TODAY="$(date -u +%Y-%m-%d)"

SLOC=$(tokei crates/ -t rust -o json | jq -r '.Rust.code')

# Build artefacts first so the test-result lines parse cleanly.
cargo test --workspace --no-run >/dev/null 2>&1
TESTS=$(cargo test --workspace 2>&1 \
        | grep -E '^test result:' \
        | awk '{n+=$4} END {print n+0}')

CRATES=$(find crates -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')
COMMITS=$(git log --oneline | wc -l | tr -d ' ')
DEVDAYS=$(git log --format='%ad' --date=short | sort -u | wc -l | tr -d ' ')
DOCFILES=$(find docs -name '*.md' | wc -l | tr -d ' ')

OUT="metrics/metrics.json"
mkdir -p "$(dirname "$OUT")"

# Preserve pre-existing perf entries (dag_resolution_*, throughput_*, etc.)
# Bench harness owns these ; structural producer must not stomp them.
if [ -f "$OUT" ]; then
    PERF_PRESERVED=$(jq 'with_entries(select(.key | test("^(dag_resolution|throughput|wall_time|peak_rss)_")))' "$OUT")
else
    PERF_PRESERVED='{}'
fi

# Compose new structural section.
STRUCTURAL=$(jq -n \
    --argjson sloc "$SLOC" \
    --argjson tests "$TESTS" \
    --argjson crates "$CRATES" \
    --argjson commits "$COMMITS" \
    --argjson devdays "$DEVDAYS" \
    --argjson docfiles "$DOCFILES" \
    --arg date "$TODAY" \
    '{
      "sloc":      { "value": $sloc,     "unit": "lines",  "source": "tokei crates/ -t rust", "measured_at": $date },
      "tests":     { "value": $tests,    "unit": "count",  "source": "cargo test --workspace", "measured_at": $date },
      "crates":    { "value": $crates,   "unit": "count",  "source": "ls crates/",             "measured_at": $date },
      "commits":   { "value": $commits,  "unit": "count",  "source": "git log --oneline",      "measured_at": $date },
      "dev_days":  { "value": $devdays,  "unit": "days",   "source": "git log unique dates",   "measured_at": $date },
      "doc_files": { "value": $docfiles, "unit": "count",  "source": "find docs -name *.md",   "measured_at": $date }
    }')

# Merge : structural section overwrites stale structural keys ;
# preserved perf section is layered back on top.
jq -n \
    --argjson structural "$STRUCTURAL" \
    --argjson perf "$PERF_PRESERVED" \
    '$structural * $perf' \
    > "$OUT.tmp"
mv "$OUT.tmp" "$OUT"

err "wrote $OUT (sloc=$SLOC tests=$TESTS crates=$CRATES commits=$COMMITS dev_days=$DEVDAYS doc_files=$DOCFILES)"
