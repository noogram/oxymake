# Design note — content-addressed run manifest

**Status:** design only, nothing implemented. Exploratory note, not an ADR.
**Origin:** deliberation `delib-20260719-fbd5` §C5 — "steal Arvados' collection
idea, refuse its platform".
**Related:** ADR-001 (content-addressable cache), ADR-005 (daemon-free,
cooperative), ADR-014 (cache/state separation), ADR-017 (artifact residence),
`docs/design/output-integrity.md`.

---

## 1. The gap

OxyMake content-addresses everything *except the thing a user actually wants to
send someone*: a whole run's output set.

Today the naming ladder stops one level short:

| Object | Names | Where it lives |
|---|---|---|
| `ContentHash` | one file's bytes | `ox-core::model::ContentHash` |
| cache key (`ComputationHash`) | one job's *computation* | `ox-cache::compute_cache_key`, format `oxymake-cache-key-v4` |
| `ox.lock` | the *plan* (rules, envs, platform) | `ox-lock::Lockfile` |
| `state.db` + NDJSON | the *trace* of one execution | `ox-state`, `ox-report-json` |
| **— missing —** | **the output set a run produced** | **—** |

The paper's four-layer ladder (`docs/paper/oxymake-paper.tex`,
`tab:fair-ladder`) has the same hole: L2 has a witness (`ox.lock`, one hash for
the plan), L3's witness is "same DAG ⇒ same output hashes" — *plural*, a set with
no name. Bazel has the same shape and no aggregate name either; git has one
(`tree`), Nix has one (`.drv` output path set), Arvados has one (a *collection*,
addressed by its portable data hash). Arvados is the closest analogue and the
reason this note exists.

Concretely, three things are awkward today and become one-liners with an
aggregate object:

1. **"Send me the results of run X."** Today: enumerate outputs, tar them, hope
   the recipient's paths line up. With a manifest: one 64-hex string.
2. **Remote-cache fetch of a result set.** `DirectoryCache` moves one blob per
   call and, by its own doc comment, *"contains blobs only; callers still need
   their local cache index to know which blobs belong to a computation"*. A
   manifest is exactly that index, and it is itself a blob — so it travels over
   the transport that already exists.
3. **"Did these two runs produce the same thing?"** Today: diff two sets of
   hashes. With a manifest: compare two strings, then diff only if they differ.

## 2. What the object is

A **run manifest** is a sorted list of `(workflow-relative path → content hash,
size)` entries plus a small header, canonically encoded, and itself addressed by
the BLAKE3 hash of that encoding.

```text
manifest_hash = blake3(
    framed("format",        "oxymake-run-manifest-v1") ‖
    framed("workflow",      oxymakefile_hash)          ‖
    framed("platform",      "linux/x86_64")            ‖
    framed("entries.count", u64le)                     ‖
    for each entry, sorted by path:
        framed("entry.path", path)  ‖
        framed("entry.hash", hex)   ‖
        framed("entry.size", u64le) ‖
        framed("entry.mode", u32le)
)
```

`framed` is `ox_core::hashing::update_field` — the same length-framed,
tag-prefixed encoding already used by the cache key, so the two objects share one
injectivity argument rather than two. Sorting by path makes the hash independent
of scheduling order; length-framing makes `{"ab" → h}` and `{"a" → bh}`
distinguishable. The format-version tag plays the role
`CACHE_KEY_FORMAT_VERSION` plays for keys: bump it and old manifests are cleanly
unreachable rather than silently reinterpreted.

The serialized form on disk is JSON (matching `ox.lock` and the NDJSON reporter —
inspectable with `jq`, no new parser), but **the hash is computed over the framed
binary encoding, not over the JSON bytes**. This is deliberate: JSON
serialization is not canonical enough to be load-bearing (key order, number
formatting, escaping), and a hash that changes when `serde_json` changes its
whitespace is not an identity.

`mode` carries only the executable bit (`0o755` vs `0o644`), the same subset git
tracks. Rationale in §7.

## 3. Scope: what goes in a manifest

**Decision: the manifest names the output closure of the requested targets, not
the set of jobs that happened to execute.**

This is the load-bearing choice and the one most likely to be got wrong. Under
the alternative ("what this run rebuilt"), a fully-cached re-run produces an
*empty* manifest and a cold run produces a full one — so the manifest would name
the execution, not the result, and two identical results would carry different
names. That destroys the property the object exists for.

Under the chosen rule:

> Same `ox.lock` + same input contents + same platform ⇒ same manifest hash,
> regardless of cache state, job order, parallelism, or which machine ran it.

for the subset of rules that are deterministic — the same caveat already attached
to the L3 row of the FAIR ladder (`$^{*}$The L3 witness holds only for
deterministic rules on a fixed substrate`). A rule declared
`ReproducibilityClass::NonReproducible` (or `Approximate`) breaks manifest
stability by construction; §7/O2 covers how the manifest says so out loud
instead of pretending.

Entries are the **declared outputs** of every job in the closure — files that
exist and are content-tracked. Excluded, with reasons:

- **Log files** — not outputs; they are trace (L4), they contain timestamps, and
  including them would make every manifest unique.
- **Phony / non-cacheable targets** — no bytes to name.
- **`OutputLifecycle::Temporary` intermediates** — they may legitimately not
  exist after a run. Including them would make the manifest depend on cleanup
  timing.
- **`MaterializePolicy::Never` outputs** — in-memory only, never written, no
  bytes to name.

Outputs are files: OxyMake has no directory-output declaration today, so the
entry list is flat. O1 in §7 records what would have to change if that stops
being true.

## 4. Where it comes from

The data already exists. After the cache layer runs, each completed job has its
outputs' content hashes in `CacheEntry::output_hashes` (`ox-cache::lookup`), and
each output already carries an `ArtifactMeta` (32-byte hash + size) inline in the
`MaterializationSet`. Building a manifest is a fold over the job graph:

```text
for job in closure(requested_targets), in any order:
    for (path, meta) in job.outputs:
        entries.insert(workflow_relative_path(path, root), (meta.hash, meta.size, mode))
sort by path; hash; write
```

Cost: one BTreeMap build and one BLAKE3 pass over ~100 bytes per output. For a
10,000-output workflow that is well under a millisecond of hashing on top of a
run that already hashed every byte of every output. **No new I/O, no new hashing
of file contents, no new dependency.** This is the whole reason the idea is worth
doing: the expensive part is already paid.

The path key uses `ox_cache::key::workflow_relative_path`, which is what makes
manifests portable across checkouts at different absolute locations. Outputs that
resolve *outside* the workflow root stay absolute in that function by design;
§7/O2 says what the manifest does about it.

## 5. Where it lives, and how it travels

Three residences, in ADR-017's vocabulary:

1. **Local, automatic:** `.oxymake/manifests/<hash>.json`, written at the end of
   every successful run, plus a `latest` pointer. Content-addressed, so writing
   the same manifest twice is a no-op. Proposed retention: `ox clean` removes
   them on the same policy it applies to cache blobs.
2. **User-chosen:** `ox run --manifest-out results.oxmanifest` for the "commit
   this next to the paper" case. Small, diffable, reviewable.
3. **Remote:** the manifest is *just another blob*. `RemoteCache::store(hash,
   path)` and `fetch` take it unchanged — no new trait method, no new backend
   code, no protocol. `DirectoryCache` gains the aggregate story for free.

The remote flow that motivates the whole design:

```text
ox fetch <manifest-hash> --cache-remote <dir>
  1. remote.fetch(manifest_hash) → manifest JSON
     (RemoteCache already re-verifies blake3(dest) == key on arrival —
      the integrity contract in ox-core::traits::remote_cache applies to
      the manifest exactly as to any blob, so a poisoned manifest is
      rejected by the same check.)
  2. parse; for each entry, if local blob missing → remote.fetch(entry.hash)
  3. materialize entries at their workflow-relative paths
  4. re-verify: recompute manifest hash from what landed on disk;
     it must equal the requested hash, or fail loudly
```

Step 4 is the difference between a manifest and a tarball: the fetch is
self-verifying end to end, and a partial fetch is detectable rather than silently
half-applied.

## 6. Refused (recorded as design boundaries)

These are refusals, not backlog items. They are recorded here so a later reader
does not "helpfully" add them.

- **Keep-style content servers.** No `ox` process listens on a socket. ADR-005 is
  daemon-free and cooperative; a blob server is the beginning of a platform.
  A manifest is a file; a shared directory or an object store moves it.
- **API / token / permission layer.** Access control on manifests is the
  filesystem's or the object store's job. OxyMake has no user model and should
  not grow one to serve an aggregate hash.
- **Federation / cross-cluster manifest resolution.** Arvados' federation solves
  a multi-institution scale problem OxyMake is not addressing. One hash + one
  configured remote is the whole namespace.
- **Named, mutable collections.** Arvados collections have names, UUIDs, and
  versions alongside their portable data hash. A name that can point at different
  content is a database, and a database wants a server. Manifests are immutable
  and anonymous; if you want a name, put the hash in a file with a name, in git.
- **Manifests as a cache-key ingredient.** Tempting (one hash for a job's whole
  input set) and wrong: it would couple the per-job key format to the aggregate
  format and re-key every cache in existence for no correctness gain. Cache keys
  keep enumerating their inputs.
- **A manifest *registry* or index service.** `.oxymake/manifests/` is a
  directory, not an index. `ls` is the query language.

## 7. Known unknowns

Ordered by how much they could change the design.

- **O1 — outputs outside the workflow root.** `workflow_relative_path` keeps such
  paths absolute, which makes the manifest machine-specific. Proposal: record
  them, mark the manifest `portable: false` in the header, and hash that flag —
  so a non-portable manifest is loudly a different object from a portable one,
  rather than a portable-looking one that fails on another machine. This is the
  one open question that must be answered before the entry schema freezes.
- **O2 — rules that are not bit-deterministic.** If any job in the closure is
  declared `SeedDeterministic`, `Approximate` or `NonReproducible`
  (`ReproducibilityClass`), the manifest is still a valid *name for this result*
  but not a *prediction of the next run's result*. Proposal: hash a
  `weakest_reproducibility` header field derived from the closure's declared
  classes, so the two kinds of manifest cannot be confused. This follows the
  paper's own discipline about not reporting a guarantee one does not have.
- **O3 — directory outputs, if they are ever added.** Outputs are files today, so
  the flat entry list is complete. If a directory output is introduced, the
  manifest must choose: (a) recursively expand into per-file entries — matches
  git, keeps the list flat, but makes a directory's identity depend on traversal
  and on hidden files; or (b) a nested sub-manifest hash as one entry (a true
  recursive git-tree). (b) is more correct and more work. Recording the choice
  now costs nothing; making it now would be premature.
- **O4 — failed and partial runs.** Chosen default: write a manifest only for a
  run where every job in the requested closure succeeded. A partial manifest is a
  name for a thing nobody wants to send. (Revisit if `--keep-going` users ask.)
- **O5 — very large output sets.** A 10⁶-output manifest is ~100 MB of JSON. The
  framed hashing is streaming already; the JSON writing/parsing would need to be
  too. Not a blocker, but the naive `serde_json::to_string` must not ship.
- **O6 — symlink outputs.** Follow or record? Git records the target as content.
  Leaning the same way; unresolved.

## 8. Falsifiable predictions

If this is built, these are the claims it should be measured against — and the
observation that would refute each.

- **P1 (determinism).** Running the same workflow twice from a cold cache on the
  same platform yields identical manifest hashes. *Refuted by:* any differing
  hash for a workflow whose rules are all declared deterministic.
- **P2 (cache-independence).** A fully-cached re-run yields the same manifest
  hash as the cold run that populated the cache. *Refuted by:* a differing hash —
  which would mean the manifest names the execution, not the result, i.e. §3 was
  implemented wrong.
- **P3 (portability).** The same tree checked out at a different absolute path,
  on the same platform, yields the same manifest hash. *Refuted by:* a differing
  hash — which would indicate an absolute path leaked into an entry key
  (the cross-machine reuse bug `workflow_relative_path` exists to prevent).
- **P4 (self-verifying fetch).** Corrupting one blob in the remote store causes
  `ox fetch <manifest>` to fail rather than materialize wrong bytes. *Refuted
  by:* a successful fetch — which would mean the per-blob integrity check or the
  step-4 recomputation is not wired.
- **P5 (cost).** Manifest construction adds < 1% to wall-clock on a workflow with
  ≥ 1000 outputs. *Refuted by:* a measurable regression — which would mean
  something is re-hashing file contents that were already hashed.

P2 and P3 are the two that would indicate the design was misunderstood rather
than merely mis-built.

## 9. Sketched surface (non-binding)

Not a specification — a sketch, so §7's decisions have something concrete to
attach to. All of it would enter as **unstable** surface per `CONTRIBUTING.md`.

```text
ox run --manifest-out <path>    write the run manifest to <path>
ox manifest show <hash|path>    print entries (path, hash, size)
ox manifest diff <a> <b>        which outputs differ between two runs
ox manifest verify <hash|path>  recompute from the working tree; report drift
ox fetch <hash> [--cache-remote <dir>]   materialize a manifest's outputs
```

`ox manifest verify` is the interesting one: it answers "is my working tree still
the thing this hash names?" — the same question `ox.lock` answers for the plan,
one layer down.

## 10. What it would change in the paper

One row in `tab:fair-ladder`, and it fills the hole that made the table
asymmetric:

```text
L3 -- Execution  | Same DAG => same output hashes* | OxyMake content-cache, Bazel actions
L3'-- Result set | One hash names a run's outputs  | OxyMake run manifest, Arvados
                 |                                 | collection, git tree
```

Plus one sentence of honest attribution: the aggregate object is Arvados'
collection idea, with the platform (Keep servers, federation, permissions)
deliberately not taken. This is the same posture as crediting cwltool and Arvados
as prior art at the primitive level — the discipline the issue-#1 review asked
for.

## 11. If this is implemented

Rough shape, smallest first. Each step is independently useful and independently
abandonable.

1. Decide **O1** (outputs outside the workflow root) — the entry/header schema
   depends on it, so nothing else can start first.
2. `ox-core`: the `RunManifest` type + framed hashing + JSON (de)serialization,
   with the hash computed over the framed encoding. Pure, fully unit-testable —
   P1/P2/P3 become tests before any wiring.
3. `ox-cli`: build the manifest from the post-run job graph; write
   `.oxymake/manifests/`; `--manifest-out`. Verifies P5.
4. `ox manifest show|diff|verify`.
5. `ox fetch` over the existing `RemoteCache` trait. Verifies P4.

Steps 2–3 are where the value is; 4–5 are convenience over the same object.
