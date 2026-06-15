# OxyMake Pre-Launch Checklist — a projection, not a tick-list

> **Source:** délib-20260529-13a7, Q-REL-4 (enforcement) + Q-REL-5 (checklist),
> governed by janis's locked discipline:
> *A discipline whose referee, player, and clock-keeper are the same single
> operator is decoration.*

This document is **not** a list of boxes a human ticks. janis's most-dangerous-gate
finding is that a self-ticked checklist is *pre-waived by design* and *produces no
corpse when it silently certifies* — it manufactures the illusion of completeness.

So the checklist is a **command-backed projection** of exogenous referees. The
single source of truth is the script:

```bash
scripts/release-checklist.sh            # pre-flip:  GATE items 1–12, 13–14 pending
scripts/release-checklist.sh --post-flip  # post-flip: 13–14 become hard gates
```

It exits **non-zero** if any GATE fails. **Do not flip the repo public while it
exits non-zero.** The corpse is the exit code, not an operator's opinion.

## The fixed sequence (janis keystone — do not reorder)

The repo is **private**, so branch protection is HTTP 403 and **every CI job is a
radar, not a gate** until the flip. The exogeneity of every gate is *downstream of
the flip itself*. Therefore:

```
   clean  →  flip  →  protect  →  enforce
```

1. **clean** — make `scripts/release-checklist.sh` (pre-flip) exit 0.
2. **flip** — `gh api -X PATCH repos/noogram/oxymake -f private=false`.
3. **protect** — `scripts/apply-branch-protection.sh` (wires required checks +
   push-protection; only settable once public).
4. **enforce** — `scripts/release-checklist.sh --post-flip` exits 0.

## The two bins (janis (e)) — nothing lives between

Every item is **exactly one** of:

- **[GATE]** — exogenous: a command's non-zero exit blocks the flip / fails CI.
- **[ADVISORY]** — explicitly non-gating: reported, never blocks. Honest demotion.

| # | Item | Bin | Exogenous referee (the command / service) |
|---|------|-----|-------------------------------------------|
| 1 | gitleaks full-history detect exits 0 | GATE | `gitleaks detect --log-opts=--all` + CI job *Secret scan* |
| 2 | `.gitleaks.toml` committed + *Secret scan* in `required_status_checks` | GATE | `git ls-files` + `gh api …/required_status_checks` |
| 3 | 0 non-public-audience paths on the surface | GATE | `scripts/artifact-map-audit.py` + CI job *Artifact-map residence gate* |
| 4 | *Artifact-map residence gate* in `required_status_checks` | GATE | `gh api …/required_status_checks` |
| 5 | confidential dirs absent from the tracked tree | GATE | `git ls-files` grep |
| 6 | private-infra / internal-domain / outreach-email strings absent | GATE | `git grep` + CI job *Forbid confidential strings* |
| 7 | CSTAFP single definition in shipping artifacts | GATE | `git grep` definition-count |
| 8 | ADR-015 free of the contradicted `58,966` SLOC figure | GATE | `git grep` |
| 9 | `CLAUDE.local.md` untracked + gitignored | GATE | `git ls-files` + `git check-ignore` |
| 10 | `deny.toml` + `deny.yml` present; `cargo deny check` clean | GATE | `cargo deny` + CI job *Deny* |
| 11 | `LICENSE-APACHE` + `LICENSE-MIT` present | GATE | file existence |
| 11b | `CITATION.cff` present + valid | ADVISORY | recommended for an academic artifact, non-blocking |
| 12 | all `github.com` oxymake URLs are `noogram/oxymake` | GATE | `git grep` |
| 13 | branch protection on `main` → 200 with ≥1 required check | GATE (post-flip) | `gh api …/branches/main/protection` |
| 14 | GitHub secret-scanning + push-protection enabled | GATE (post-flip) | `gh api … security_and_analysis` |
| 15 | second independent referee named | **ADVISORY** | see below — honestly ABSENT |
| 16 | spec/tla release review recorded — if a `v*` tag exists, `spec/tla/REVIEWS.md` holds a `## REVIEW` entry at-or-after it | GATE | `git describe` + `grep` on `REVIEWS.md` (event-based; replaces the calendar sunset reviews voided 2026-06-10, premortem PM#5) |

## Item 15 — the honest ABSENT record (janis (d) #5)

As of 2026-05-30, `noogram` has exactly **one** org member and **one** repo admin:
`@eserie` — the operator himself. A CODEOWNERS entry pointing at the operator's own
account is *a self-appointed mindguard in a reviewer's hat* — theater, not a gate.

Therefore the *second independent referee* requirement is **UNMET**, and is recorded
as **ADVISORY**, not faked as a gate. Consequences, stated plainly:

- *"main stays public"* is **MONITORED** by `.github/workflows/topology-guard.yml`
  (a daily cron that raises a tracked issue if `.private` becomes `true`) — but
  **not ENFORCED**: the same operator who could flip it back can also disable the
  cron. The cron leaves a corpse; it cannot prevent the act.
- A second `noogram` org-admin (or an org-level repository rule) is the structural
  upgrade that would make flip-back non-unilateral. When one exists, promote item 15
  from ADVISORY to GATE and update `.github/CODEOWNERS`.

## What the exogenous gates are (Q-REL-4)

| Workflow | Job name (the `required_status_checks` context) | Referee for |
|----------|--------------------------------------------------|-------------|
| `.github/workflows/secret-scan.yml` | `Secret scan (gitleaks, full history + diff)` | credentials (gate i) |
| `.github/workflows/forbid-strings.yml` | `Forbid confidential strings (release gate v)` | confidential residue (gate v) |
| `.github/workflows/artifact-map.yml` | `Artifact-map residence gate` | residence (gate ii) |
| `.github/workflows/topology-guard.yml` | (cron, opens an issue) | main-stays-public monitor (gate iv) |

None of these is a `.git/hooks/pre-commit` (those are `--no-verify`-bypassable and
self-refereed). None carries `continue-on-error` or a skip-env (which would demote
it to advisory silently). They become **gates** only once their job name is listed
in `required_status_checks` — which `scripts/apply-branch-protection.sh` does
post-flip.

## Engineering Definition-of-Done gates (the `ci.yml` floor)

Distinct from the public-prep referees above, the standard code-quality gates
from `CLAUDE.md` must pass before any release. CI (`.github/workflows/ci.yml`)
enforces them; `just ci` reproduces them locally:

| Gate | Command | CI job |
|------|---------|--------|
| Build | `cargo check --workspace` | `build` |
| Test | `cargo test --workspace` | `test` |
| Lint | `cargo clippy --workspace -- -D warnings` | `clippy` |
| Format | `cargo fmt --all -- --check` | `fmt` |
| Docs | `RUSTDOCFLAGS=-Dwarnings cargo doc --workspace --no-deps` | `doc` |

The **Docs** gate is the trap door: `cargo doc -Dwarnings` fails on broken
intra-doc links, but `cargo check` does **not** — so a doc-link break is invisible
to the local Definition of Done unless `cargo doc` is run explicitly. It is now
part of `just ci`, `just tag-release`, and the `CLAUDE.md` DoD table for exactly
this reason. Run `just doc` (or `just ci`) before submitting.
