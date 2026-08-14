#!/usr/bin/env bash
# Reproducible cache-output corruption experiment.  Results are deliberately
# observations, not assertions about any engine version.
set -euo pipefail

readonly ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
readonly RESULTS_DIR="${RESULTS_DIR:-$ROOT/results}"
readonly EXPECTED='immutable-cache-output'
ENGINE="all"

usage() {
  cat <<'EOF'
Usage: ./run.sh [--engine ox-default|ox-hash|cwltool|arvados|all]

Writes a timestamped, self-contained observation directory below results/.
The Arvados case needs arvados-cwl-runner plus ARVADOS_API_HOST and
ARVADOS_API_TOKEN; it is skipped rather than fabricated when unavailable.
EOF
}

while (($#)); do
  case "$1" in
    --engine) ENGINE="${2:?--engine needs a value}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) printf 'unknown argument: %s\n' "$1" >&2; usage >&2; exit 2 ;;
  esac
done

case "$ENGINE" in ox-default|ox-hash|cwltool|arvados|all) ;; *) usage >&2; exit 2 ;; esac

RUN_DIR="$RESULTS_DIR/$(date -u +%Y%m%dT%H%M%SZ)"
mkdir -p "$RUN_DIR"
readonly RUN_DIR
printf 'started_at_utc=%s\nengine=%s\n' "$(date -u +%FT%TZ)" "$ENGINE" > "$RUN_DIR/metadata.txt"

sha256() {
  if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}';
  else shasum -a 256 "$1" | awk '{print $1}'; fi
}

size() { wc -c < "$1" | tr -d ' '; }

record_file() {
  local label="$1"
  local file="$2"
  {
    printf '%s.path=%s\n' "$label" "$file"
    printf '%s.sha256=%s\n' "$label" "$(sha256 "$file")"
    printf '%s.size_bytes=%s\n' "$label" "$(size "$file")"
    # UTC, nanoseconds where supported.  The raw stat output is kept too.
    printf '%s.mtime_raw=' "$label"; stat -c '%y' "$file" 2>/dev/null || stat -f '%Sm' -t '%Y-%m-%dT%H:%M:%S%z' "$file"
  } >> "$RUN_DIR/observations.txt"
}

corrupt_same_size_and_mtime() {
  local file="$1"
  local reference="$file.before-corruption"
  cp -p "$file" "$reference"
  printf X | dd of="$file" bs=1 count=1 conv=notrunc status=none
  touch -r "$reference" "$file"
  rm -f "$reference"
}

classify() {
  local label="$1"
  local output="$2"
  local value
  value="$(tr -d '\r\n' < "$output")"
  if [[ "$value" == "$EXPECTED" ]]; then
    printf '%s=clean_output_observed (engine re-executed or restored a verified copy)\n' "$label" >> "$RUN_DIR/verdicts.txt"
  else
    printf '%s=poisoned_output_served (corruption survived cache reuse)\n' "$label" >> "$RUN_DIR/verdicts.txt"
  fi
}

run_ox() {
  local policy="$1"
  local label="ox-$2"
  local work="$RUN_DIR/$label"
  if ! command -v ox >/dev/null 2>&1; then
    printf '%s=SKIPPED: ox not on PATH\n' "$label" >> "$RUN_DIR/verdicts.txt"; return
  fi
  mkdir -p "$work"
  cp -f "$ROOT/Oxymakefile.toml" "$work/Oxymakefile.toml"
  (
    cd "$work"
    ox run --cache-validation "$policy" > first.log 2>&1
    record_file "$label.before" output/result.txt
    corrupt_same_size_and_mtime output/result.txt
    record_file "$label.corrupted" output/result.txt
    ox run --cache-validation "$policy" > second.log 2>&1
    record_file "$label.after" output/result.txt
    classify "$label" output/result.txt
  )
}

find_cwl_cached_output() {
  local cache="$1"
  local matches=()
  while IFS= read -r file; do matches+=("$file"); done < <(find "$cache" -type f -name result.txt -print | sort)
  if ((${#matches[@]} != 1)); then
    printf 'expected one cached result.txt, found %s\n' "${#matches[@]}" >&2
    return 1
  fi
  printf '%s\n' "${matches[0]}"
}

run_cwltool() {
  local label="cwltool"
  local work="$RUN_DIR/$label"
  local cache="$work/cache"
  local out1="$work/out-first"
  local out2="$work/out-second"
  if ! command -v cwltool >/dev/null 2>&1; then
    printf '%s=SKIPPED: cwltool not on PATH\n' "$label" >> "$RUN_DIR/verdicts.txt"; return
  fi
  mkdir -p "$work"
  cwltool --version >> "$RUN_DIR/versions.txt" 2>&1 || true
  cwltool --cachedir "$cache" --outdir "$out1" "$ROOT/workflow.cwl" > "$work/first.log" 2>&1
  local cached
  cached="$(find_cwl_cached_output "$cache")"
  record_file "$label.cached.before" "$cached"
  corrupt_same_size_and_mtime "$cached"
  record_file "$label.cached.corrupted" "$cached"
  cwltool --cachedir "$cache" --outdir "$out2" "$ROOT/workflow.cwl" > "$work/second.log" 2>&1
  record_file "$label.out.after" "$out2/result.txt"
  classify "$label" "$out2/result.txt"
}

run_arvados() {
  local label="arvados"
  if ! command -v arvados-cwl-runner >/dev/null 2>&1; then
    printf '%s=SKIPPED: arvados-cwl-runner not on PATH\n' "$label" >> "$RUN_DIR/verdicts.txt"; return
  fi
  if [[ -z "${ARVADOS_API_HOST:-}" || -z "${ARVADOS_API_TOKEN:-}" ]]; then
    printf '%s=SKIPPED: ARVADOS_API_HOST and ARVADOS_API_TOKEN are required\n' "$label" >> "$RUN_DIR/verdicts.txt"; return
  fi
  printf '%s=BLOCKED: configured deployment required; use arvados-run.sh to run and collect UUIDs\n' "$label" >> "$RUN_DIR/verdicts.txt"
}

printf 'expected_output=%s\n' "$EXPECTED" > "$RUN_DIR/observations.txt"
if [[ "$ENGINE" == all || "$ENGINE" == ox-default ]]; then run_ox mtime+hash default; fi
if [[ "$ENGINE" == all || "$ENGINE" == ox-hash ]]; then run_ox hash hash; fi
if [[ "$ENGINE" == all || "$ENGINE" == cwltool ]]; then run_cwltool; fi
if [[ "$ENGINE" == all || "$ENGINE" == arvados ]]; then run_arvados; fi
printf 'finished_at_utc=%s\n' "$(date -u +%FT%TZ)" >> "$RUN_DIR/metadata.txt"
printf 'results: %s\n' "$RUN_DIR"
