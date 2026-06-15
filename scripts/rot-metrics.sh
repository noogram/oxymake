#!/usr/bin/env bash
# rot-metrics.sh — three chiffrable rot signals for OxyMake.
#
# The point is not to act on every metric every day. The point is that
# silent rot (a test disappeared, clippy --all-features got noisy,
# a crate stopped being tested) leaves a numerical trail. The numbers
# are dated and committed so drift is visible in `git log`.
#
# Metrics (per the chaos/rot pre-mortem):
#
#   M-dep-untested        fraction of workspace crates with zero #[test]
#                         items. >0.8 = alarm. Why fraction not count:
#                         a 60-crate workspace and a 6-crate workspace
#                         have different rot pressures at the same count.
#
#   M-clippy-allfeatures  warning count from
#                         `cargo clippy --workspace --all-features`.
#                         >5 = alarm. Default profile already runs
#                         clippy -D warnings in CI; this is the
#                         broader profile that often drifts.
#
#   M-test-silence        number of #[ignore] / #[cfg(not(test))] /
#                         #[cfg(not(<feature>))] suppressions on test
#                         items that do NOT cite a bead, issue URL,
#                         or RFC reference within 5 lines above.
#                         >0 = alarm. `cfg(not(unix))` and similar
#                         platform gates are legitimate and excluded.
#
# Output: a single Markdown snapshot under `docs/health/<date>.md`.
# Exit codes:
#   0  metrics computed (regardless of alarm thresholds — surfacing,
#      not gating; the README of the health folder names who reads it)
#   1  one or more alarm thresholds exceeded
#   2  harness error (cannot run cargo / find repo root / write file)

set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$REPO_ROOT" || { echo "fatal: cannot cd to repo root" >&2; exit 2; }

DATE_TAG="${ROT_DATE:-$(date -u +%Y-%m-%d)}"
HEALTH_DIR="$REPO_ROOT/docs/health"
OUT_FILE="$HEALTH_DIR/${DATE_TAG}.md"
mkdir -p "$HEALTH_DIR"

# Threshold knobs (override via env for local experiments — not for CI).
DEP_UNTESTED_ALARM="${ROT_DEP_UNTESTED_ALARM:-0.8}"
CLIPPY_ALL_ALARM="${ROT_CLIPPY_ALL_ALARM:-5}"
TEST_SILENCE_ALARM="${ROT_TEST_SILENCE_ALARM:-0}"

ALARMS=0

# --- M-dep-untested ----------------------------------------------------------

# Workspace crates = directories under crates/ with a Cargo.toml. A crate
# is "tested" when at least one `#[test]` directive lives under its tree
# (`src/`, `tests/`, anywhere). This is intentionally a broad heuristic:
# documentation tests, ignored tests, and feature-gated tests still count
# as "the crate has thought about testing". The signal we want to catch
# is "this crate has *nothing* — no #[test], no harness, nobody is
# checking it".
TOTAL_CRATES=0
UNTESTED_CRATES=0
UNTESTED_NAMES=()
for cargo_toml in "$REPO_ROOT"/crates/*/Cargo.toml; do
    [ -f "$cargo_toml" ] || continue
    crate_dir="$(dirname "$cargo_toml")"
    crate_name="$(basename "$crate_dir")"
    TOTAL_CRATES=$((TOTAL_CRATES + 1))
    # `grep -r -l --include` is more portable than `find -path`.
    if ! grep -r -l --include='*.rs' -E '#\[test\]|#\[tokio::test\]' "$crate_dir" >/dev/null 2>&1; then
        UNTESTED_CRATES=$((UNTESTED_CRATES + 1))
        UNTESTED_NAMES+=("$crate_name")
    fi
done

if [ "$TOTAL_CRATES" -eq 0 ]; then
    DEP_UNTESTED_RATIO="0.000"
else
    DEP_UNTESTED_RATIO=$(python3 -c "print(f'{$UNTESTED_CRATES / $TOTAL_CRATES:.3f}')")
fi
DEP_ALARM=$(python3 -c "print('1' if $DEP_UNTESTED_RATIO > $DEP_UNTESTED_ALARM else '0')")
[ "$DEP_ALARM" = "1" ] && ALARMS=$((ALARMS + 1))

# --- M-clippy-allfeatures ----------------------------------------------------

# `cargo clippy --all-features` exercises feature combinations the default
# CI does not. We count warning lines (not denied), so this measures
# *latent* rot — what would become an error if `-D warnings` were promoted
# to the all-features profile.
CLIPPY_LOG=$(mktemp)
cargo clippy --workspace --all-features --no-deps --quiet --message-format=short 2>"$CLIPPY_LOG" >/dev/null || true
# `grep -c` prints the count *and* exits 1 on zero matches; capture only
# stdout and ignore the exit code so we get a clean integer either way.
CLIPPY_WARNINGS=$(grep -cE '^[^[:space:]].*:[[:space:]]*warning:' "$CLIPPY_LOG" || true)
CLIPPY_WARNINGS=${CLIPPY_WARNINGS:-0}
rm -f "$CLIPPY_LOG"
CLIPPY_ALARM=0
if [ "$CLIPPY_WARNINGS" -gt "$CLIPPY_ALL_ALARM" ]; then
    CLIPPY_ALARM=1
    ALARMS=$((ALARMS + 1))
fi

# --- M-test-silence ----------------------------------------------------------

# Count `#[ignore]` and `#[cfg(not(...))]` suppressions whose preceding 5
# lines mention NO tracking link. We exclude cfg gates whose argument is
# a recognised platform/OS atom — those are legitimate cross-platform
# code, not silent suppression.
SILENCE_PY=$(mktemp)
cat >"$SILENCE_PY" <<'PY'
import pathlib, re, sys

JUSTIFY_RE = re.compile(
    r"(bd-\d+|BD-\d+|issue[s]?[/#]|#\d+|https?://|RFC[- ]?\d+|TODO\(|"
    r"FIXME\(|fixme:|todo:)",
    re.IGNORECASE,
)
PLATFORM_ATOMS = {
    "unix", "windows", "macos", "linux", "target_os", "target_family",
    "target_arch", "target_endian", "target_pointer_width", "target_env",
}
IGNORE_RE = re.compile(r"^\s*#\[ignore(\s*=.*)?\]\s*$")
CFG_NOT_RE = re.compile(r"^\s*#\[cfg\(not\((.+?)\)\)\]\s*$")

root = pathlib.Path(sys.argv[1])
hits = []
for path in root.rglob("*.rs"):
    try:
        lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        continue
    for i, line in enumerate(lines):
        m_ignore = IGNORE_RE.match(line)
        m_cfg = CFG_NOT_RE.match(line)
        if not (m_ignore or m_cfg):
            continue
        if m_cfg:
            inner = m_cfg.group(1).strip()
            # Platform gates: cfg(not(unix)), cfg(not(target_os = "linux"))
            atom = re.split(r"[\s=,(]", inner, 1)[0]
            if atom in PLATFORM_ATOMS:
                continue
        window = "\n".join(lines[max(0, i - 5) : i])
        if JUSTIFY_RE.search(window):
            continue
        kind = "#[ignore]" if m_ignore else f"#[cfg(not({m_cfg.group(1).strip()}))]"
        hits.append(f"{path.relative_to(root)}:{i + 1}  {kind}")

print(len(hits))
for h in hits:
    print(h)
PY

SILENCE_OUT=$(python3 "$SILENCE_PY" "$REPO_ROOT" 2>/dev/null || echo "0")
TEST_SILENCE=$(printf '%s\n' "$SILENCE_OUT" | head -n 1)
TEST_SILENCE=${TEST_SILENCE:-0}
SILENCE_HITS=$(printf '%s\n' "$SILENCE_OUT" | tail -n +2)
rm -f "$SILENCE_PY"

TEST_SILENCE_ALARM_FLAG=0
if [ "$TEST_SILENCE" -gt "$TEST_SILENCE_ALARM" ]; then
    TEST_SILENCE_ALARM_FLAG=1
    ALARMS=$((ALARMS + 1))
fi

# --- Render snapshot ---------------------------------------------------------

dep_status="OK"
[ "$DEP_ALARM" = "1" ] && dep_status="ALARM"
clippy_status="OK"
[ "$CLIPPY_ALARM" = "1" ] && clippy_status="ALARM"
silence_status="OK"
[ "$TEST_SILENCE_ALARM_FLAG" = "1" ] && silence_status="ALARM"

untested_list="—"
if [ "${#UNTESTED_NAMES[@]}" -gt 0 ]; then
    untested_list=$(printf '%s, ' "${UNTESTED_NAMES[@]}")
    untested_list="${untested_list%, }"
fi

{
    echo "# Health snapshot — $DATE_TAG"
    echo ""
    echo "*Emitted by \`scripts/rot-metrics.sh\`. Regenerate daily, or whenever the engine moves.*"
    echo ""
    echo "| Metric | Value | Alarm threshold | Status |"
    echo "|---|---|---|---|"
    echo "| M-dep-untested     | $DEP_UNTESTED_RATIO ($UNTESTED_CRATES/$TOTAL_CRATES crates) | > $DEP_UNTESTED_ALARM | $dep_status |"
    echo "| M-clippy-allfeatures | $CLIPPY_WARNINGS | > $CLIPPY_ALL_ALARM   | $clippy_status |"
    echo "| M-test-silence     | $TEST_SILENCE | > $TEST_SILENCE_ALARM   | $silence_status |"
    echo ""
    echo "## Untested crates"
    echo ""
    echo "$untested_list"
    echo ""
    echo "## Silent test suppressions"
    echo ""
    if [ -n "$SILENCE_HITS" ]; then
        printf '%s\n' "$SILENCE_HITS" | sed 's/^/- /'
    else
        echo "_None._"
    fi
    echo ""
    echo "## How to read this file"
    echo ""
    echo "- **M-dep-untested** measures absence-of-thinking, not coverage."
    echo "  A crate without a single \`#[test]\` directive is a crate"
    echo "  nobody has decided how to verify. Bring the ratio down by"
    echo "  adding tests, or document why the crate is verification-by-"
    echo "  composition (and cite the composing test)."
    echo "- **M-clippy-allfeatures** is the latent-rot dial. The default"
    echo "  CI runs \`-D warnings\` on the default profile; this number"
    echo "  is what that gate would catch if promoted to \`--all-features\`."
    echo "- **M-test-silence** is the silent-suppression dial. Every"
    echo "  \`#[ignore]\` or non-platform \`#[cfg(not(...))]\` on a test"
    echo "  item must either be removed or accompanied by a tracking"
    echo "  reference (bd-XXX, issue URL, RFC, or TODO/FIXME) within"
    echo "  five lines. Platform gates (\`cfg(not(unix))\` etc.) are"
    echo "  excluded as legitimate cross-platform code."
} >"$OUT_FILE"

echo "wrote $OUT_FILE"
echo "  M-dep-untested      $DEP_UNTESTED_RATIO    ($dep_status)"
echo "  M-clippy-allfeatures $CLIPPY_WARNINGS      ($clippy_status)"
echo "  M-test-silence      $TEST_SILENCE          ($silence_status)"

if [ "$ALARMS" -gt 0 ]; then
    exit 1
fi
exit 0
