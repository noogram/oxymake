# ADR-018: Differentiation After the CWL Review — Envelope, Not Primitive

## Status
Proposed

## Metadata

- **Date:** 2026-07-20
- **Kind:** `decision`
- **Family:** `CAS`
- **Supersedes:** `none` (narrows the *positioning* claims of ADR-001 and
  OXYMAKE-THESIS.md §3.1 without abrogating either mechanism; see
  [Sunset](#sunset))

## Context

Michael R. Crusoe (CWL co-founder) reviewed the OxyMake arXiv paper as
[noogram/oxymake#1](https://github.com/noogram/oxymake/issues/1) and corrected
three factual claims about CWL. Commit `e4aa98e` already landed the paper-side
correction. Deliberation `delib-20260719-fbd5` (dewey · popper · torvalds ·
feynman · godin) then asked the harder question: **what does the real semantics
of CWL imply for OxyMake itself?**

Three external facts are load-bearing here, and none of them were reflected in
our positioning:

1. **CWL is a specification, not an engine.** It has no change-detection
   policy, because specifications do not have one. Comparing "CWL" to OxyMake
   on caching is a category error — the comparable objects are *cwltool* and
   *ox*.
2. **cwltool already ships a content-sensitive cache.** `--cachedir` builds a
   canonical dictionary from the realised command line; stdin/stdout/stderr;
   file size plus `checksum` — falling back to size plus **mtime** when no
   checksum is available; the selected container string; selected
   requirements/hints; and the preserved environment. The cache entry is named
   from a digest of that dictionary. So it is content-addressed where checksums
   exist and metadata-addressed where they do not. Content-sensitive
   memoization is therefore *not* a differentiator; it is prior art in the
   very ecosystem we positioned ourselves against. (Corrected 2026-07-20
   against current `cwltool/command_line_tool.py`; the earlier summary "keys on
   a checksum of the command line plus input `checksum` fields" omitted the
   fallback, the requirements, and the environment.)
3. **The CWL data model carries explicit data links and checksums**
   (`File.checksum`, `sha1$…`), and Arvados/Keep runs a deployed
   content-addressed store with block-level dedup and cross-user reuse — a
   strictly larger content-addressing story than ours, at platform scale.

Meanwhile an internal audit of this repository (evidence below) found that the
*documented* differentiation is partly unbuilt:

| Claim | Where stated | Reality |
|---|---|---|
| `trust_scope` is a cache-key dimension | `OXYMAKE-THESIS.md` §3.1 formula; `docs/book/src/concepts/cache.md:119` | **Not in the key.** `crates/ox-cache/src/key.rs:70-107` hashes version tag, rule source, sorted `(path, hash)` inputs, params, env, shell, platform. No `trust_scope` symbol exists in `crates/ox-cache*`. |
| `ox run --cache-remote s3://…` | `docs/book/src/concepts/cache.md:116` | **Flag does not exist.** No `cache_remote` arg in `crates/ox-cli`; `ox-cli/Cargo.toml` does not even depend on `ox-cache-remote`. |
| Remote cache backends | book, same page | `DirectoryCache` is fully implemented (`crates/ox-cache-remote/src/directory.rs:57-141`, with fetch-time hash re-verification). `S3Cache` and `GcsCache` return `Unavailable("… not yet implemented")` (`s3.rs:93-137`, `gcs.rs:63-105`). |
| Cache keys travel across trees | ADR-001 "shareable across machines" | **True, and verified.** Input paths enter the key as workflow-declared *relative* patterns (`crates/ox-core/src/resolver.rs:490,507` → `crates/ox-cli/src/commands/run.rs:328`); no cwd or absolute root is hashed. Identical content in `/a/proj` and `/b/proj` yields the same key on the same platform. |
| Content-addressing "by default" | thesis §3.1, paper abstract | **Half true.** The *key* is always content-derived. Output *re-verification* depth is policy-dependent: the default `mtime+hash` (ADR-006, 2026-06-10 amendment) skips hashing when mtime+size match, so a same-size/same-mtime output corruption is invisible unless the operator opts into `--cache-validation=hash`. |

The glass-house note is part of the context: the only pure-mtime engine the
paper actually measured was OxyMake's own former default (ADR-006 pre-amendment).

## Decision

### 1. Reposition the differentiation from *primitive* to *envelope*

The claim "OxyMake is content-addressed where the workflow ecosystem is
timestamp-based" is **retired**. It is false against cwltool and against
Arvados, and it was never true of CWL-the-spec.

The surviving claim, and the one this project defends:

> **Content-addressed verifying execution, on by default, from a single
> daemon-free binary on a plain filesystem** — no deployed CAS service, no
> coordinator, no opt-in flag to get a content-derived key.

The three axes that make this a non-empty cell, stated as a conjunction rather
than as any single novel primitive:

- **ergonomics** — filename-pattern rules (the Make/Snakemake family), not a
  document-oriented spec;
- **addressing** — a portable content-derived key covering rule source, inputs,
  params, env and platform, *on by default*, not behind `--cache`;
- **locus** — one self-contained `ox` executable that bundles no interpreter
  and requires no OxyMake daemon for local execution (ADR-005), no server-side
  store. (Originally "one static binary"; narrowed 2026-07-21 to the paper's
  attested formulation — no linkage attestation has been published, see the
  ERRATUM "Static linkage" entry.)

cwltool occupies {document spec} × {opt-in content hash} × {local}. Arvados
occupies {document spec} × {deployed CAS} × {cluster}. Neither occupies ours;
that is the whole of the claim, and it is smaller than what was published.

### 2. State the default's guarantee precisely — do not repeat the over-claim

Every future statement of "content-addressed by default" MUST carry its exact
scope, in this form:

> The cache **key** is always content-derived. Output **re-verification** depth
> is policy-controlled: `mtime+hash` (default) re-hashes only when metadata
> moved; `hash` re-hashes unconditionally and is required for shared or remote
> caches.

The default stays `mtime+hash` (ADR-006 amendment stands — flipping to `hash`
would eat the cold-path cost that motivated ADR-006 in the first place). The
correction is to the *sentence*, not to the *policy*. Publishing the
repositioned claim without this asterisk would reproduce, in miniature, exactly
the over-claim the review caught.

### 3. Adopt "spec vs engine" as a vocabulary invariant

In this repository — ADRs, book, thesis, paper — a specification is never
assigned a change-detection behaviour. Comparisons are engine-to-engine
(`ox` ↔ `cwltool`, `snakemake`, `nextflow`) or spec-to-spec
(`Oxymakefile.toml` ↔ CWL, WDL). This is the generator of the five wrong
claims dewey's audit found; naming it is how it stops recurring.

### 4. Close the documented-but-unbuilt gaps (ordered)

- **G1 — docs drift, blocking.** `docs/book/src/concepts/cache.md` documents a
  `--cache-remote` flag, an `s3://` backend, and a `trust_scope` key dimension,
  none of which exist. Same class of defect as the one Crusoe found, one close
  reader away. Fix the docs *now*, independent of whether the features land.
- **G2 — thesis key formula.** `OXYMAKE-THESIS.md` §3.1 prints a cache-key
  formula containing `trust_scope`. Align it with `key.rs` or add an
  Attestation-Table row marking the dimension aspirational.
- **G3 — wire `--cache-remote <dir>`** to the working `DirectoryCache`. This is
  the smallest true version of "the cache travels": a plain shared directory,
  no service. ADR-006 already mandates promotion to `ContentHash` on remote
  caches; enforce it at the wiring point.
- **G4 — run manifest.** Add an aggregate object naming a whole run's output
  set by one hash. `ox.lock` (`crates/ox-lock/src/model.rs:19-42`) records
  inputs, rule/env specs and platform, but **no output hashes** — there is
  currently no single name for "what this run produced". This is the honest
  thing to steal from Arvados/git-tree, and it is the precondition for any
  credible provenance story.

### 5. Refuse: platform features and CWL interop

**Refused — platform.** No Keep-style block server, no federation, no
permission model. These contradict ADR-005 (daemon-free cooperative model) and
would put us on Arvados' ground, where we lose.

**Refused — CWL interop, both directions.**
- *Import* is architecturally unfaithful: CWL permits JavaScript expressions
  evaluated at runtime (`InlineJavascriptRequirement`, `valueFrom`), while
  OxyMake rules are strictly applicative and statically parseable (ADR-002).
  A faithful importer would require an expression evaluator that dissolves the
  static-analysis property that is the reason TOML was chosen.
- *Export* is work whose payoff is helping users leave, and it invites
  "OxyMake supports CWL" — a promise this audience knows the true cost of.

**Accepted instead — collaborate at the cache-semantics layer.** A reproducible
cross-engine cache-behaviour harness (`cwltool --cache` / `ox` / Arvados) is the
productive overlap. It is simultaneously popper's refutation experiment, the
erratum's evidence, and a legible entry gesture to `workflows.community`. It is
also risk management: if cwltool-with-cache *does* re-verify outputs on hit,
the corruption-detection differentiator collapses and we would rather learn
that from our own harness than from the next reviewer.

## Consequences

**Easier.** Positioning becomes defensible under close reading by domain
experts. The spec-vs-engine invariant kills a whole class of future error at the
source. G3/G4 are small, and G4 unlocks provenance, remote-cache manifests, and
run-level diffing at once.

**Harder.** The marketing surface shrinks: a conjunction is harder to put in an
abstract than "we are content-addressed and they are not". The precise-default
sentence is longer than the slogan it replaces. And every occurrence of the old
claim across paper, thesis, book and README must be found and narrowed —
subtractive work, but broad.

**Risk carried forward.** If the harness shows cwltool re-verifies outputs on
cache hit, §1's envelope narrows again to {ergonomics × default-on × single
binary}, with corruption-detection dropped entirely. That outcome is
anticipated, not fatal, and is the reason the harness is scheduled *before* any
further public claim.

## Alternatives Considered

**Narrow the claim to cwltool only, keep "content-addressing is our
differentiator".** Rejected: Arvados/Keep is a strictly larger content-addressing
system, so the claim fails on a second front. Fixing one leg of a false
universal invites the next reviewer to kick the other.

**Make `hash` the default so "content-addressed by default" needs no
asterisk.** Rejected: it reverses ADR-006 for a rhetorical gain and reintroduces
the ~15s fully-cached 35-job run that ADR-006 was written to eliminate. Stating
the guarantee precisely is cheaper and more honest than bending the engine to
fit a sentence.

**Build CWL import to claim ecosystem interop.** Rejected on faithfulness, per
§5. A partial importer that silently drops runtime expressions is worse than no
importer: it produces workflows that appear to run and quietly compute
something else.

**Say nothing and quietly fix the docs.** Rejected. The review was public; a
silent fix is the one move that converts a correctable error into a credibility
question.

## Amendment (2026-07-21)

The Context table above records the audited state of 2026-07-20 and is kept as
history. Three of its rows are superseded by code that landed since:

- **`ox run --cache-remote <dir>` exists and is wired** (commit `b0afe3b`,
  hardened by `6985e19`). `ox-cli` depends on `ox-cache-remote`, the flag
  drives the `DirectoryCache` backend, and validation is force-promoted to
  `ContentHash` at the wiring point, as G3 demanded.
- **G3 is discharged only partially.** The directory backend is a blob
  transport: it stores content-addressed artefact bytes, but the SQLite index
  mapping a computation key to its output paths and hashes stays local to each
  checkout. "The cache travels" therefore holds for blobs, not for the
  computation manifest — a fresh checkout pointing at the shared directory
  re-executes unless the local index is transferred too. G3 stays partial
  until a remote computation-key-addressed manifest (the G4 object, stored
  remotely) lands.
- **Path handling in the key is now format v4** (`6985e19`), and the
  "no cwd or absolute root is hashed" row needs its boundary stated: the
  workflow root is the invocation directory (not the parent of the `-f`
  file); relative paths are interpreted from that root and lexically
  normalised; existing paths are canonicalised so symlink escapes are
  detected; a path inside the root enters the key relative, while one that
  escapes the root — absolute spelling, `..` prefix, or symlink — enters as a
  normalised absolute path and does not travel across checkouts.

The "one static binary" wording in §1 is narrowed in place (marked inline) to
the paper's attested formulation, consistent with the ERRATUM entry
"Static linkage".

## Sunset

This ADR does not abrogate any mechanism. It narrows *claims*:

- **ADR-001** — the CacheStore mechanism stands unchanged. Its Context section
  frames the alternative as "Snakemake uses mtime"; read that as scoped to
  Snakemake and GNU Make, never generalised to "the workflow ecosystem".
- **ADR-006** — unchanged and reaffirmed. This ADR adds a *statement*
  obligation (§2) around the default it sets, not a policy change.
- **OXYMAKE-THESIS.md §3.1** — the `trust_scope` term in the printed key
  formula is unattested (G2). Until it is built or removed, the Attestation
  Table row for §6.4 overstates coverage.

Backlog decomposition (G1–G4, the harness, and the claim sweep) lives in
[`ops/backlog/crusoe-implications-2026-07-20.md`](../../ops/backlog/crusoe-implications-2026-07-20.md).
Nothing there is nucleated by this ADR.
