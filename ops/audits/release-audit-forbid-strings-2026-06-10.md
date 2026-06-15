# Release-audit ↔ forbid-strings reconciliation (R-OUTIL, M8 verdict R7/F11)

**Date:** 2026-06-10 · **Molecule:** task-20260610-04fe · **Status:** divergence
root-caused; cosmon-side fix filed; residual flags accepted as documented false
positives.

> Literal-string hygiene: this document refers to the flagged domain in
> defanged form (`serie-research[.]dev`) so that it does not itself add a
> fourth gate-C flag. Same discipline as the M8 verdict.

## Symptom (référé contre référé)

`cs release-audit --dry-run` (gate C, detector `structural-string`) flags 3
occurrences of the public maintainer contact domain `serie-research[.]dev`:

| File | Line | What it actually is |
|------|------|---------------------|
| `CHANGELOG.md` | 23 | maintainer contact in the v0.1.0 entry — intentional |
| `SECURITY.md` | 18 | security-report contact email — intentional |
| `.github/workflows/forbid-strings.yml` | 25 | the gate's own comment explaining *why* the domain is deliberately **not** forbidden |

`forbid-strings.yml` (CI referee) explicitly exempts the
domain: citation-auditor verified it as the *intended public maintainer
identity — benign*, live in `oxymake-paper.tex`, `CHANGELOG.md`,
`SECURITY.md`. Two referees over the same surface disagree; a detector that
cries permanently on a known-benign string institutionalizes alarm fatigue and
will mask a real leak.

## Root cause — the pattern list is out of repo

The gate-C pattern list is **hardcoded in the cosmon binary**, not in any
oxymake-repo config:

- Source of truth: `cosmon` galaxy,
  `crates/cosmon-cli/src/cmd/release_audit.rs`, `structural_hits_in_line()`.
- It unconditionally flags the research domain in any non-purged tracked file.
  The only carve-outs are `AUTHOR_EMAIL` (`@serie.dev`, homeserver check
  only), the `claudion` provenance path, and the detector's *own* Rust source
  (`is_rule_definition_source`) — which covers cosmon's rule file but **not**
  an application repo's rule file (`forbid-strings.yml`), hence flag #3.
- Neither `.cosmon/artifact-map.toml` (genre→audience map only) nor
  `cs release-audit --config` (generic cosmon config) carries an exemption
  list. There is nothing in this repo to patch.

The detector was written for the cosmon public distribution (claudion
vendoring, oidc bindings, client renames); oxymake's intentional public
maintainer identity is a policy it cannot express today.

## Resolution

Per the mission's explicit instruction ("si la config du release-audit est
hors repo, documenter la limite au lieu de patcher en aveugle") and the
cosmon-ward feedback rule (surface core pathologies as typed molecules, never
silently patch around them):

1. **Cosmon-ward issue filed:** `task-20260610-af93` (cosmon galaxy, kind
   `issue`) — proposes a maintainer-contact carve-out analogous to
   `AUTHOR_EMAIL`, or a per-repo exemption config (e.g.
   `.cosmon/release-audit.toml` allowlist with justification comments) so
   both referees share one exemption list.
2. **Coherence rule documented** in `RELEASING.md` (pre-publish hygiene).
3. **Accepted-false-positive baseline:** until the cosmon fix lands,
   `cs release-audit` on this repo reports **exactly 3** `structural-string`
   flags (the table above). Anything other than these 3 is a real regression
   — treat a count ≠ 3 or a different file/line set as RED.

## Verification

- `cs release-audit --dry-run` re-run after this change: still exactly the 3
  known flags; this document and the `RELEASING.md` line add none (defanged).
- `forbid-strings` pattern (`git grep` of the workflow's PATTERN): clean.
