# Backlog proposals — CWL-review implications (2026-07-20)

**Source:** deliberation `delib-20260719-fbd5`, task `task-20260719-b6f8`.
**Governing decision:** [ADR-018](../../docs/adr/018-differentiation-after-cwl-review.md).

> **Nothing here is nucleated.** This file is a proposal list for the operator.
> Each entry is written so it can be lifted into a molecule verbatim, but the
> nucleation gesture is deliberately withheld.

Ordering below is dependency-and-risk order, not priority order. B1 is the only
entry that is credibility-blocking on its own.

---

## B1 — Fix the book's cache page (documented-but-unbuilt)

- **Kind:** `task` · **Temp:** `hot` · **Blocked by:** —
- **Scope:** `docs/book/src/concepts/cache.md`

`cache.md:116` documents `ox run --cache-remote s3://my-bucket/…`. The flag does
not exist in `crates/ox-cli` (no `cache_remote` arg; `ox-cli/Cargo.toml` does not
depend on `ox-cache-remote`). `cache.md:119-120` documents a `trust_scope`
cache-poisoning guard with no implementation anywhere in `crates/ox-cache*`.
`S3Cache`/`GcsCache` are stubs returning `Unavailable` (`s3.rs:93-137`,
`gcs.rs:63-105`).

**Do:** describe only what ships. State that remote cache exists as a library
(`DirectoryCache`, working) and is not yet wired to the CLI. Delete `trust_scope`
or mark it explicitly as a design sketch.

**Done when:** every flag, URI scheme and key dimension named in `cache.md` is
either exercised by a test or labelled unimplemented in the same paragraph.

**Why it's first:** this is a Crusoe-class defect of the same shape as the one
already found, sitting in the public book. It costs an afternoon.

---

## B2 — Align the thesis cache-key formula with `key.rs`

- **Kind:** `task` · **Temp:** `hot` · **Blocked by:** —
- **Scope:** `OXYMAKE-THESIS.md` §3.1 + Attestation Table

The printed formula includes `+ trust_scope`. `compute_cache_key`
(`crates/ox-cache/src/key.rs:70-107`) hashes: format-version tag, rule source,
sorted `(path, content_hash)` input pairs, params hash, env hash, shell
executable, platform. No `trust_scope`.

**Do:** remove the term, or keep it and add an Attestation-Table row reading
*Aspirational — not in the key*. The §6.4 row ("Cache key includes everything")
currently reads *Accepted — implemented* while the thesis text names a dimension
that is not implemented; that is exactly the drift the table exists to catch.

**Done when:** the formula in the thesis is byte-comparable to what `key.rs`
hashes, or the gap is a named row.

---

## B3 — Claim sweep: retire the false universal repo-wide

- **Kind:** `task` · **Temp:** `hot` · **Blocked by:** —
- **Scope:** `README.md`, `OXYMAKE-THESIS.md`, `docs/book/src/**`, `STATUS.md`

Grep for every statement of the form "workflow engines use timestamps",
"content-addressed unlike X", "the Make lineage". Replace with the ADR-018 §1
envelope formulation and the ADR-018 §2 precise-default sentence. Apply the
spec-vs-engine invariant (ADR-018 §3): no specification is assigned a
change-detection behaviour.

**Note:** the thesis does not currently name CWL at all — its exposure is
narrower than the paper's. Expect this sweep to be small and mostly subtractive.

**Done when:** no surviving sentence generalises mtime-based invalidation beyond
GNU Make, Snakemake, and Nextflow's default cache mode (which genuinely hashes
`lastModified` into its key — the one true member of the original trio, keep it,
primary-sourced).

---

## B4 — Wire `--cache-remote <dir>` to `DirectoryCache`

- **Kind:** `task` · **Temp:** `warm` · **Blocked by:** B1
- **Scope:** `crates/ox-cli`, `crates/ox-cache-remote`

`DirectoryCache` is complete and correct — atomic rename-into-place plus
fetch-time hash re-verification (`directory.rs:57-141`) — and reachable from
nothing. Add the dependency, add the flag, accept a plain directory path only
(no URI schemes until a backend exists).

ADR-006 mandates promotion to `ContentHash` validation whenever a remote cache is
in play; enforce that at the wiring point, not by documentation.

**Done when:** two workflow trees on the same machine sharing one `--cache-remote`
directory produce a cross-tree cache hit, covered by an integration test, and the
validation policy is observably `hash` regardless of configured default.

**Deliberately out of scope:** S3/GCS. Leave the stubs. A shared directory over
NFS/SSHFS is the daemon-free-shaped answer, and it is the one we can defend.

---

## B5 — Run manifest: name a whole run's outputs by one hash

- **Kind:** `task` · **Temp:** `warm` · **Blocked by:** B4
- **Scope:** `crates/ox-lock` or a new aggregate object

`ox.lock` (`crates/ox-lock/src/model.rs:19-42`) records `oxymakefile_hash`,
platform, per-rule execution/env hashes, declared input+output *pattern strings*,
and a global input path→content-hash table. It records **no output content
hashes** (`LockedRule.outputs` at `model.rs:64` is patterns, not hashes), and
there is no run-level aggregate.

**Do:** a git-tree analogue — a sorted `(output_path, content_hash)` set hashed
into one manifest hash, written per run.

**Why:** it is the missing L3 aggregate. It is the precondition for provenance
claims, for remote-cache manifest transfer (B4), for run-to-run diffing, and for
answering "did this run produce the same bytes as that one" with one comparison
instead of N.

**Done when:** two runs with identical inputs produce byte-identical manifest
hashes, and a single mutated output byte changes it.

---

## B6 — Cross-engine cache-behaviour harness (the keystone)

- **Kind:** `task` · **Temp:** `warm` · **Blocked by:** B4
- **Scope:** new, `bench/` or a sibling repo

One artefact doing three jobs: refutation experiment, erratum evidence, and
community entry gesture.

**Experiment A — output corruption.** Populate a cache in `cwltool --cache` and
in `ox` (both default and `--cache-validation=hash`). Corrupt a cached output
preserving size and mtime. Re-run. Record, per engine and per policy, whether the
corruption is detected or the poisoned output is served.

**Experiment B — delivery envelope.** Measure what it takes to get a working
content-addressed cache from cold: single static binary + plain filesystem (`ox`)
vs `cwltool --cache` vs an Arvados/Keep deployment. Report setup steps, running
services, and disk footprint — this is the *actual* differentiator claim, so it is
the one that must be measured.

**Live risk, state it up front:** if cwltool re-verifies outputs on cache hit,
Experiment A refutes our corruption-detection differentiator and ADR-018 §1
narrows further. Running this before making any further public claim is the point.

**Done when:** the harness is reproducible from a clean checkout by a third party
and its results are stated in a form that could embarrass us.

---

## B7 — Public erratum and reply to issue #1

- **Kind:** `task` · **Temp:** `warm` · **Blocked by:** B3
- **Scope:** GitHub issue noogram/oxymake#1 · **operator-gated (outbound)**

Concede all three of Crusoe's corrections plainly, plus the two the internal
audit surfaced (Cromwell's default backend is local — contradicting the paper's
own §5.7; Galaxy ships a REST API, BioBlend and Planemo, so "sacrifices
programmability" is unsupported). Five, not three: surpassing the reviewer's ask
is the trust-building move.

Commit to a v3 with no date; leave the issue open until v3 is live and linked
back. Offer the B6 harness. Do not promise CWL support (ADR-018 §5). No
defensive rebuttal, no flattery, no manufactured launch moment.

**Explicitly withheld:** this entry is *not* to be executed by a worker.
Outbound public communication on an open review thread is an operator gesture.

---

## Explicitly not proposed

- **CWL import/export.** Refused in ADR-018 §5 — import is architecturally
  unfaithful (runtime JS expressions vs strictly-applicative rules), export is
  work so users can leave.
- **Keep-style block server, federation, permission model.** Contradicts ADR-005.
- **S3/GCS backends.** Not until someone asks with a workflow. The stubs stay
  stubs; B1 makes the docs say so.
- **Flipping the default to `hash`.** Reverses ADR-006 for a rhetorical gain.
  ADR-018 §2 fixes the sentence instead.
