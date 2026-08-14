#!/usr/bin/env bash
# Run this only against an Arvados deployment you administer. It records IDs;
# it never guesses deployment topology or claims a result from a local machine.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
: "${ARVADOS_API_HOST:?set ARVADOS_API_HOST}"
: "${ARVADOS_API_TOKEN:?set ARVADOS_API_TOKEN}"
command -v arvados-cwl-runner >/dev/null || { echo 'arvados-cwl-runner is required' >&2; exit 127; }

mkdir -p "$ROOT/results/arvados"
arvados-cwl-runner --no-wait "$ROOT/workflow.cwl" | tee "$ROOT/results/arvados/submission-$(date -u +%Y%m%dT%H%M%SZ).log"
