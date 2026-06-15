# ADR-017: Artifact Residence Topology (Topology B)

## Status
Accepted (release-readiness review).

## Metadata

- **Date:** 2026-05-30
- **Kind:** `decision`
- **Family:** `EXT`
- **Supersedes:** none

## Context

Before this repository flips from private to public, one governance rule is
load-bearing for the whole release:

> A fresh public clone of `main` must contain ONLY public/team artifacts.

`.cosmon/artifact-map.toml` declares, per genre, what every tracked file *is*
(its genre) and *who it is for* (its audience ∈ `{public, team, author+agent,
partner:<name>, solo}`). A file's **residence** — where it is allowed to live —
is derived from its audience: public/team belong on the published surface,
everything else does not. The open question this ADR closes is *where the
non-public residences physically live* — the topology.

Three candidate topologies were on the table:

- **A** — all genres on `main`; non-public files held off the public surface by
  `.gitignore` only.
- **B** — `main` carries `public`/`team` only; `author+agent` and
  `partner:<name>` live on an **orphan `narration` branch**; `solo` is
  local-only.
- **C** — non-public artifacts moved to a separate private sub-repository.

## Decision

**Topology B.** It keeps the residence model simple — one repository, one
artifact map, one audit (`scripts/artifact-map-audit.py`) proving coverage over
a single `git ls-files`.

### Canonical residence rules (derived from audience)

| Audience | Genres (this repo) | Residence | On a fresh `main` clone? |
|----------|--------------------|-----------|--------------------------|
| `public` | paper, book, adr, format-doc, api-doc, architecture-doc, manifest, crate-source, benchmark-harness | `main` (tracked) | **Yes** |
| `team` | (reserved) | `main` | Yes |
| `author+agent` | paper-premortem, outreach, chronicle, cosmon-feedback, decision-record, private-archive | orphan **`narration`** (no shared history with `main`) | **No** |
| `partner:<name>` | (reserved) | orphan `narration` (optionally encrypted) | No |
| `solo` | cosmon-state | local-only, `.git/info/exclude` | No (never tracked) |

The `narration` branch is created with `git checkout --orphan narration`, so it
shares **no commit ancestor** with `main`: walking parents from the `main` tip
never reaches it. The public remote is configured to publish only `main`.

`solo` artifacts (`.cosmon/state/**`) are kept out of the index by
`.git/info/exclude` — which is exactly why `.cosmon/artifact-map.toml` itself
must be **force-tracked** (`git add -f`): the map is `public` (it matches the
`code` catch-all, not `cosmon-state`), but it lives under the `.cosmon/`
directory that `.git/info/exclude` holds back for the `solo` state tree.

### Public-clone proof (summary)

1. *Totality (I1):* the `code` catch-all `**/*` is declared LAST ⇒ every
   tracked path classifies.
2. *Partition (I2):* longest-match + declaration-order agree ⇒ the tracked set
   partitions deterministically by audience.
3. *Confidential off main:* `author+agent`/`partner` → orphan; `solo` →
   `.git/info/exclude`. After the history rewrite (below), none is reachable
   from any `main` commit.
4. *Orphan unreachable:* `--orphan` ⇒ no shared ancestor.
5. ∴ a fresh clone of `main` sees only `public`/`team`. ∎

### The audit (totality and residence)

The audit (`scripts/artifact-map-audit.py`, run by CI) checks two invariants
over `git ls-files`:

- **I1 — totality:** every tracked path matches at least one genre glob.
  Guaranteed by the `code` catch-all `**/*` declared LAST in the map.
- **Residence:** no tracked path classifies as a non-public audience. Anything
  that resolves to `author+agent` / `partner:*` / `solo` is a confidential leak.

### The enforcement gate (exogenous)

`.github/workflows/artifact-map.yml` runs `scripts/artifact-map-audit.py` on
every push/PR to `main` and turns the build **red** if any tracked path is
unmapped (I1) **or** classifies as a non-public audience (a confidential leak).
It has no skip-env and no `continue-on-error`; silencing it requires a visible
commit. It is NOT a `pre-commit` hook (`--no-verify`-bypassable, self-refereed —
forbidden — an author who can skip their own check is no check at all).

The gate only becomes a true *gate* once the `artifact-map` job is listed in
`required_status_checks` under branch protection — available only after the
repo is public (or on GitHub Pro). That wiring is a downstream operator act;
this ADR and the workflow build the gate and prove it goes red on a planted
violation.

## Consequences

- One repo, one map, one audit — auditability and totality are preserved.
- A new confidential genre is protected the moment it is mapped to a non-public
  audience: the gate fails closed (`PUBLIC_AUDIENCES` is a positive allow-list).
- The initial flip requires a **history rewrite** (`git filter-repo`) of the
  already-tracked confidential paths, plus moving them onto `narration`, BEFORE
  publication — "stop tracking going forward" is insufficient because history is
  append-only and a prior commit a public clone fetches still holds the file.
  This rewrite is incompatible with the shared-object-store worktree / merge-
  queue model and is therefore an **escalated, operator-run** step, not part of
  this MQ-landed task.
- A totality-only audit (every path classifies) is not enough: it returns OK on
  a confidential-on-main leak. The release gate needs the **residence** check
  too, which is why CI runs `scripts/artifact-map-audit.py` — it enforces I1
  **and** residence in one pass.

## Alternatives Considered

- **A (gitignore-on-main) — rejected, internally contradictory with the
  residence model.** The model maps `author+agent → orphan branch`, never "main
  but untracked." A would keep confidential files physically in the `main`
  working tree, held out only by `.gitignore` — an unprotected single point of
  failure: one `git add -f`, one tooling glob, one materialized synthesis, and
  the file is on `main` forever (history is append-only). A is "declare and
  hope," which the residence model already rejects.
- **C (separate private sub-repo) — rejected, breaks I1/I2.** Two repos ⇒ the
  artifact map cannot prove totality over one `git ls-files`; two maps, two
  audits, an unenforced sync invariant, and fragmented history (a decision
  record and the ADR it becomes would live in different repos). Reserve a
  separate repo only for a genuinely different residence class (encrypted
  partner data at scale).

## Sunset

None. If a future decision changes the residence topology, that ADR carries
`Supersedes: ADR-017` and this Status line moves to `Superseded by ADR-XXX`.
