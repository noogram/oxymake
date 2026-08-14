#!/usr/bin/env bash
# Capture the delivery envelope, not a synthetic performance number.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly OUT="${RESULTS_DIR:-$ROOT/results}/envelope-$(date -u +%Y%m%dT%H%M%SZ).md"
mkdir -p "$(dirname "$OUT")"

bytes() { wc -c < "$1" | tr -d ' '; }
{
  echo '# Delivery-envelope observation'
  echo
  echo "- Recorded: $(date -u +%FT%TZ)"
  echo '- OxyMake local run needs one `ox` executable and a writable filesystem; no daemon is started by this harness.'
  if command -v ox >/dev/null 2>&1; then
    ox_path="$(command -v ox)"
    echo "- Observed ox path: \`$ox_path\` ($(bytes "$ox_path") bytes)"
    echo "- Observed ox version: \`$(ox --version 2>&1 | head -1)\`"
  else echo '- OxyMake binary: unavailable on PATH.'; fi
  if command -v cwltool >/dev/null 2>&1; then echo "- Observed cwltool version: \`$(cwltool --version 2>&1 | head -1)\`"; else echo '- cwltool: unavailable on PATH.'; fi
  echo '- cwltool cache mode uses a local cache directory, but its interpreter/container envelope is environment-specific and is not inferred here.'
  echo '- Arvados/Keep requires a configured deployment. Record its API, Keep, database, dispatch, and worker services from deployment-managed observability; this harness does not equate a client install with a platform deployment.'
} > "$OUT"
printf 'results: %s\n' "$OUT"
