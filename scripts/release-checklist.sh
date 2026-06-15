#!/usr/bin/env bash
# release-checklist.sh — the pre-launch checklist as a COMMAND-BACKED PROJECTION
# ============================================================================
# The most-dangerous-gate finding: a self-ticked checklist is "pre-waived by
# design" and "produces no corpse when it silently certifies". Therefore this is
# NOT a list of boxes a human ticks. Every GATE item below is a *projection of an
# exogenous referee*: a command whose exit status — not an operator's opinion —
# decides PASS/FAIL. Run it; if it exits non-zero, the repo is NOT ready to flip.
#
# The output partitions into exactly two bins (janis (e)):
#   [GATE]     — exogenous; a non-zero command fails the build / blocks the flip.
#   [ADVISORY] — explicitly non-gating; reported, never fails. Honest demotion.
# Nothing lives between.
#
# USAGE
#   scripts/release-checklist.sh              # pre-flip: checks GATE items 1-12,
#                                             # reports flip-dependent 13-14 as PENDING.
#   scripts/release-checklist.sh --post-flip  # after the public flip: 13-14 become hard.
#
# EXIT CODE
#   0  — every applicable GATE passed (ready to proceed to the next phase).
#   1  — at least one GATE failed (NOT ready). ADVISORY items never affect this.
# ============================================================================
set -uo pipefail

REPO_SLUG="noogram/oxymake"
POST_FLIP=0
[ "${1:-}" = "--post-flip" ] && POST_FLIP=1

# Resolve repo root (this script lives in scripts/).
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"

FAILS=0
PENDS=0

# ── Externalized denylists (never hard-coded in this guard) ─────────────────
# A guard that names in clear what it forbids re-leaks it (auto-doxxing —
# scrub-oxymake-main-tree, D7). So the confidential alternations are NOT baked
# in here. Supply them via environment, or via a gitignored local file
# scripts/.release-denylist.local that exports:
#   OXYMAKE_FORBID_PATTERN  — `git grep -E` alternation of private-infra markers
#   OXYMAKE_CONF_PATHS      — `grep -E` alternation of confidential dir prefixes
# When neither is provided, the two gates that need them report PEND (honest
# demotion: the CI 'Forbid Strings' job remains the exogenous referee), never a
# silent pass.
if [ -f scripts/.release-denylist.local ]; then
  # shellcheck disable=SC1091
  . scripts/.release-denylist.local
fi

c_pass=$'\033[32m'; c_fail=$'\033[31m'; c_pend=$'\033[33m'; c_adv=$'\033[36m'; c_off=$'\033[0m'

gate_pass() { printf '  %s[GATE  PASS]%s %s\n' "$c_pass" "$c_off" "$1"; }
gate_fail() { printf '  %s[GATE  FAIL]%s %s\n' "$c_fail" "$c_off" "$1"; FAILS=$((FAILS+1)); }
gate_pend() { printf '  %s[GATE  PEND]%s %s\n' "$c_pend" "$c_off" "$1"; PENDS=$((PENDS+1)); }
advisory()  { printf '  %s[ADVISORY ]%s %s\n' "$c_adv"  "$c_off" "$1"; }

echo "============================================================================"
echo " OxyMake pre-launch checklist — projection of exogenous gates"
echo " spec: release-checklist  |  repo: ${REPO_SLUG}"
echo " mode: $([ $POST_FLIP -eq 1 ] && echo 'POST-FLIP (13-14 hard)' || echo 'PRE-FLIP (13-14 pending)')"
echo "============================================================================"

# ── 1. gitleaks full-history detect exits 0 ─────────────────────────────────
if command -v gitleaks >/dev/null 2>&1; then
  if gitleaks detect --source . --log-opts="--all" --config .gitleaks.toml \
       --no-banner --redact >/tmp/rc-gitleaks.log 2>&1; then
    gate_pass "1. gitleaks detect --log-opts=--all → no leaks (exit 0)"
  else
    gate_fail "1. gitleaks detect found leaks — see /tmp/rc-gitleaks.log"
  fi
else
  gate_pend "1. gitleaks not installed locally — CI job 'Secret scan' is the referee"
fi

# ── 2. .gitleaks.toml committed + secret-scan in required_status_checks ──────
if git ls-files --error-unmatch .gitleaks.toml >/dev/null 2>&1; then
  if [ $POST_FLIP -eq 1 ]; then
    if gh api "repos/${REPO_SLUG}/branches/main/protection/required_status_checks" \
         --jq '.contexts[]' 2>/dev/null | grep -qx "Secret scan (gitleaks, full history + diff)"; then
      gate_pass "2. .gitleaks.toml committed + 'Secret scan' in required_status_checks"
    else
      gate_fail "2. 'Secret scan' NOT in required_status_checks — run scripts/apply-branch-protection.sh"
    fi
  else
    gate_pend "2. .gitleaks.toml committed ✓; required_status_checks wiring is post-flip"
  fi
else
  gate_fail "2. .gitleaks.toml is not tracked"
fi

# ── 3. artifact-map residence audit: 0 author+agent paths on main ───────────
if [ -f scripts/artifact-map-audit.py ] && [ -f .cosmon/artifact-map.toml ]; then
  if python3 scripts/artifact-map-audit.py >/tmp/rc-artmap.log 2>&1; then
    gate_pass "3. artifact-map-audit.py → 0 non-public-audience paths on the tree"
  else
    gate_fail "3. artifact-map-audit.py found confidential paths on the surface — /tmp/rc-artmap.log"
  fi
else
  gate_fail "3. scripts/artifact-map-audit.py or .cosmon/artifact-map.toml missing"
fi

# ── 4. artifact-map job in required_status_checks ───────────────────────────
if [ $POST_FLIP -eq 1 ]; then
  if gh api "repos/${REPO_SLUG}/branches/main/protection/required_status_checks" \
       --jq '.contexts[]' 2>/dev/null | grep -qx "Artifact-map residence gate"; then
    gate_pass "4. 'Artifact-map residence gate' in required_status_checks"
  else
    gate_fail "4. 'Artifact-map residence gate' NOT in required_status_checks"
  fi
else
  gate_pend "4. artifact-map required_status_checks wiring is post-flip"
fi

# ── 5. confidential paths absent from git log --all ─────────────────────────
# The residence-confidential directories must be unreachable from any commit.
# Denylist supplied externally (see OXYMAKE_CONF_PATHS above) — not inlined.
CONF_PATHS="${OXYMAKE_CONF_PATHS:-}"
if [ -z "$CONF_PATHS" ]; then
  gate_pend "5. confidential-paths denylist not provided (set OXYMAKE_CONF_PATHS or scripts/.release-denylist.local) — cannot check; CI is the referee"
elif git log --all --pretty=format: --name-only --diff-filter=A 2>/dev/null \
     | grep -qE "^(${CONF_PATHS})"; then
  # present somewhere in history — but only a leak if reachable from main HEAD tree:
  if git ls-files | grep -qE "^(${CONF_PATHS})"; then
    gate_fail "5. confidential paths are tracked on the current tree"
  else
    advisory "5. confidential paths exist in HISTORY but not the HEAD tree — full-history filter-repo is the one-shot operator act (D2/T1); not a recurring gate"
  fi
else
  gate_pass "5. confidential directories absent from the tracked tree"
fi

# ── 6. private home paths / infra markers absent (forbid-strings projection) ─
# Denylist supplied externally (see OXYMAKE_FORBID_PATTERN above) — not inlined.
FORBID="${OXYMAKE_FORBID_PATTERN:-}"
if [ -z "$FORBID" ]; then
  gate_pend "6. forbid-strings denylist not provided (set OXYMAKE_FORBID_PATTERN or scripts/.release-denylist.local) — CI 'Forbid Strings' is the referee"
elif git grep -nIE "$FORBID" -- . \
     ':(exclude).github/workflows/forbid-strings.yml' \
     ':(exclude)scripts/release-checklist.sh' >/tmp/rc-forbid.log 2>&1; then
  gate_fail "6. forbidden confidential strings present — see /tmp/rc-forbid.log ($(wc -l </tmp/rc-forbid.log) hits)"
else
  gate_pass "6. no private-infra / internal-domain / outreach-email strings in tree"
fi

# ── 7. CSTAFP single definition (no double-definition in shipping artifacts) ─
defs=$(git grep -hIoE 'CSTAFP[^.]*(Concurrent State Transitions Across independently-Failing|Cache-Staleness from Timestamp-Affecting File Provenance)' \
         -- 'docs' 'spec' '*.tex' 2>/dev/null \
       | grep -oE 'Concurrent State Transitions Across independently-Failing|Cache-Staleness from Timestamp-Affecting File Provenance' \
       | sort -u | wc -l | tr -d ' ')
if [ "${defs:-0}" -le 1 ]; then
  gate_pass "7. CSTAFP resolves to a single definition across shipping artifacts"
else
  gate_fail "7. CSTAFP has ${defs} conflicting definitions (adversary's HN-headline defect)"
fi

# ── 8. ADR-015 numbers consistent with metrics ─────────────────────────────
# The cargo-cult number 58,966 SLOC (citation-auditor) must not survive.
if git grep -qIE '58,?966' -- docs/adr/015-named-invariants.md 2>/dev/null; then
  gate_fail "8. ADR-015 still carries the cargo-cult 58,966 SLOC figure (citation-auditor)"
else
  gate_pass "8. ADR-015 free of the contradicted 58,966 SLOC figure"
fi

# ── 9. CLAUDE.local.md untracked + gitignored ──────────────────────────────
if git ls-files --error-unmatch CLAUDE.local.md >/dev/null 2>&1; then
  gate_fail "9. CLAUDE.local.md is TRACKED (the full Gas Town/polecat playbook leak)"
elif git check-ignore -q CLAUDE.local.md; then
  gate_pass "9. CLAUDE.local.md untracked and gitignored"
else
  gate_fail "9. CLAUDE.local.md not tracked but NOT gitignored — add it to .gitignore"
fi

# ── 10. deny.toml CI gate present + cargo-deny clean ───────────────────────
if [ -f deny.toml ] && [ -f .github/workflows/deny.yml ]; then
  if command -v cargo-deny >/dev/null 2>&1; then
    if cargo deny check bans licenses sources >/tmp/rc-deny.log 2>&1; then
      gate_pass "10. deny.toml + deny.yml present; cargo deny check clean"
    else
      gate_fail "10. cargo deny check failed — see /tmp/rc-deny.log"
    fi
  else
    gate_pass "10. deny.toml + deny.yml present (CI 'Deny' job is the referee)"
  fi
else
  gate_fail "10. deny.toml or .github/workflows/deny.yml missing"
fi

# ── 11. LICENSE files present (hard) + CITATION.cff valid (advisory) ───────
if [ -f LICENSE-APACHE ] && [ -f LICENSE-MIT ]; then
  gate_pass "11. LICENSE-APACHE + LICENSE-MIT present"
else
  gate_fail "11. a LICENSE file is missing"
fi
if [ -f CITATION.cff ]; then
  advisory "11b. CITATION.cff present (validate with cffconvert if available)"
else
  advisory "11b. CITATION.cff ABSENT — recommended for an academic artifact, non-blocking"
fi

# ── 12. all remote URLs uniform noogram/oxymake ────────────────────────────
if git grep -nIE 'github\.com[:/][a-zA-Z0-9_-]+/oxymake' -- . 2>/dev/null \
     | grep -vE 'noogram/oxymake' \
     | grep -vE 'RELEASE-CHECKLIST\.md|release-checklist\.sh' >/tmp/rc-urls.log 2>&1; then
  gate_fail "12. non-canonical oxymake repo URLs present (want noogram/oxymake) — /tmp/rc-urls.log"
else
  gate_pass "12. all github.com oxymake URLs point at noogram/oxymake"
fi

# ── 13. branch protection on main returns 200 with required checks ─────────
if [ $POST_FLIP -eq 1 ]; then
  if gh api "repos/${REPO_SLUG}/branches/main/protection" >/tmp/rc-prot.log 2>&1; then
    n=$(gh api "repos/${REPO_SLUG}/branches/main/protection/required_status_checks" \
          --jq '.contexts | length' 2>/dev/null || echo 0)
    if [ "${n:-0}" -ge 1 ]; then
      gate_pass "13. branch protection on main → 200 with ${n} required checks"
    else
      gate_fail "13. branch protection exists but lists 0 required checks"
    fi
  else
    gate_fail "13. branch protection on main not configured (run apply-branch-protection.sh)"
  fi
else
  gate_pend "13. branch protection is HTTP 403 until the repo is public — post-flip gate"
fi

# ── 14. push-protection + secret-scanning enabled ──────────────────────────
if [ $POST_FLIP -eq 1 ]; then
  sa=$(gh api "repos/${REPO_SLUG}" --jq '.security_and_analysis' 2>/dev/null || echo '{}')
  ss=$(echo "$sa" | python3 -c 'import sys,json;d=json.load(sys.stdin) or {};print(d.get("secret_scanning",{}).get("status",""))' 2>/dev/null)
  pp=$(echo "$sa" | python3 -c 'import sys,json;d=json.load(sys.stdin) or {};print(d.get("secret_scanning_push_protection",{}).get("status",""))' 2>/dev/null)
  if [ "$ss" = "enabled" ] && [ "$pp" = "enabled" ]; then
    gate_pass "14. GitHub secret-scanning + push-protection enabled"
  else
    gate_fail "14. secret-scanning='$ss' push-protection='$pp' (want both enabled)"
  fi
else
  gate_pend "14. GitHub secret-scanning/push-protection settable post-flip — post-flip gate"
fi

# ── 15. second independent referee (ADVISORY — honest ABSENT record) ───────
admins=$(gh api "repos/${REPO_SLUG}/collaborators" --jq '[.[]|select(.permissions.admin==true)]|length' 2>/dev/null || echo "?")
if [ "${admins:-1}" -gt 1 ] 2>/dev/null; then
  advisory "15. ${admins} repo admins — a second independent referee MAY exist (verify human independence)"
else
  advisory "15. second org-admin / independent CODEOWNER: ABSENT (only @eserie). 'main stays public' is MONITORED (topology-guard cron) not ENFORCED. Honest demotion per janis (d)#5 — see .github/CODEOWNERS."
fi

# ── 16. spec/tla release review recorded (release-gated sunset discipline) ──
# Operator decision 2026-06-10 (premortem PM#5): the TLA+ sunset reviews are
# gated on v* releases, not on calendar dates. The referee here is purely
# event-based: IF a v* tag exists, REVIEWS.md must contain a `## REVIEW` entry
# dated at-or-after that tag's commit date. No tag → no review due yet.
LAST_TAG=$(git describe --tags --abbrev=0 --match 'v*' 2>/dev/null || true)
if [ -n "$LAST_TAG" ]; then
  TAG_DATE=$(git log -1 --format=%cd --date=format:%Y-%m-%d "$LAST_TAG" 2>/dev/null || echo "9999-99-99")
  LAST_REVIEW=$(grep -oE '^## REVIEW — [0-9]{4}-[0-9]{2}-[0-9]{2}' spec/tla/REVIEWS.md 2>/dev/null \
    | grep -oE '[0-9]{4}-[0-9]{2}-[0-9]{2}' | sort | tail -n1)
  if [ -n "$LAST_REVIEW" ] && [ "$(printf '%s\n%s\n' "$TAG_DATE" "$LAST_REVIEW" | sort | tail -n1)" = "$LAST_REVIEW" ]; then
    gate_pass "16. spec/tla review recorded for ${LAST_TAG} (REVIEW ${LAST_REVIEW} ≥ tag ${TAG_DATE})"
  else
    gate_fail "16. no spec/tla REVIEW entry at-or-after ${LAST_TAG} (${TAG_DATE}) — the release-gated sunset review was skipped (spec/tla/README.md)"
  fi
else
  gate_pass "16. no v* tag yet — first spec/tla review is due at the first release"
fi

echo "----------------------------------------------------------------------------"
if [ $FAILS -eq 0 ]; then
  if [ $PENDS -gt 0 ] && [ $POST_FLIP -eq 0 ]; then
    printf '%s READY for the flip: all pre-flip GATEs pass. %d gate(s) PENDING until public.%s\n' "$c_pass" "$PENDS" "$c_off"
    echo "   Next: perform the flip, then run scripts/apply-branch-protection.sh,"
    echo "   then re-run: scripts/release-checklist.sh --post-flip"
  else
    printf '%s READY: all applicable GATEs pass.%s\n' "$c_pass" "$c_off"
  fi
  exit 0
else
  printf '%s NOT READY: %d GATE(s) failed. Do NOT flip until all are green.%s\n' "$c_fail" "$FAILS" "$c_off"
  exit 1
fi
