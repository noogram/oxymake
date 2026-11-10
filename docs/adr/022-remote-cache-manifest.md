# ADR-022: Remote Cache Entries — One Record per Computation Key, Published After Its Blobs

## Status
Accepted (2026-11-10)

## Metadata

- **Date:** 2026-11-09
- **Kind:** `decision`
- **Family:** `CAS`
- **Supersedes:** `none` (extends `--cache-remote`; complements ADR-019
  without changing it)

## Context

[noogram/oxymake#66](https://github.com/noogram/oxymake/issues/66) asks that a
run with fresh local state restore and skip the units another run already
computed, from a shared `--cache-remote` directory alone. Acceptance: two runs
with disjoint state directories share one remote directory; the second restores
and skips every unit the first completed, including after the first was
interrupted, and verifies every restored output by hash.

### What `--cache-remote` does today

`DirectoryCache` (`crates/ox-cache-remote/src/directory.rs`) stores blobs,
named by their BLAKE3 content hash as in the content-addressed cache of
ADR-001, under `<remote>/<first two hex chars>/<hash>`. `store` stages to a
sibling temporary file and renames it into place; `fetch` copies the blob and
deletes the copy if its hash does not match the name. The directory carries no computation keys.

The lookup is local. `restore_remote_outputs` (`crates/ox-cli/src/commands/run.rs`)
receives the job's cache key from its caller, reads the `cache_entries` row for
that key from the local `cache.db`, and fetches the blobs named in its
`output_records`.
Without a local row it returns `false` and the job runs. It is called from two
places: the planning pre-scan (`execution_plan::determine_execution`, which
walks the job graph in topological order) and the scheduler's cache check
(`SchedulerCache::check_with_reason`). Publication happens in
`SchedulerCache::record_with_hashes`: after the local row is written, each
output blob is stored; a failed store only prints a warning. The Ray path
reaches the same function through `ray_records::Follower::with_cache` while
`ox run --follow` is attached. `--cache-remote` forces `content` validation
(ADR-006).

So a second checkout, another host, or a wiped `.oxymake/` has the bytes but
not the map from computation to bytes. The Ray guide says so: "the remote
directory carries bytes, not the computation-key metadata needed to identify a
hit".

### Facts the decision rests on

- **The key is portable between checkouts on one platform.** The cache key
  (`compute_cache_key`, format `oxymake-cache-key-v7`) hashes the serialized
  execution block, the input `(path, hash)` pairs with in-project paths made
  relative to the workflow root (`workflow_relative_path`), the params, env and
  shell terms, `clean_outputs`, and the platform under `cache_platform =
  "exact"`. Two checkouts of the same workflow over the same input bytes compute
  the same key. Observed with the current binary: two checkouts at different
  absolute paths, one rule with an in-project output and one with an output
  under an absolute `[config]` root, produced identical keys for both units.
  One known exception: for a uv environment the key hashes every
  `.python-version` in the working directory and its ancestors
  (`env_spec_content_hash`), so a checkout placed under a different ancestor
  pin gets a different key, hence a miss.
- **The key needs the inputs.** `job_cache_key_with_components` returns `None`
  when a declared input does not exist. The pre-scan runs in topological order,
  so an upstream unit restored from the remote makes its outputs available for
  the downstream key in the same pass. Raw inputs must still be present on the
  restoring side.
- **Output paths enter the key only through the execution block.** The
  resolver interpolates `{output}` into the execution block, so a shell or
  inline-run command that names its outputs gets a different key for a
  different output path. A script or call block usually names a script path or
  a function, not the outputs, so two jobs with the same key can declare
  different outputs.
- **Output paths in `cache.db` are declared strings.** `output_records.path` is
  the path as the job declares it after resolution: relative inside the
  project, absolute under an ADR-021 config root. It is not normalized.
- **The adoption manifest is a different document.**
  `oxymake.cache-adoption-manifest` (`commands/cache_export.rs`) is a batch,
  addressed by target, with no cache key. `ox cache-import` recomputes the
  local key from the transferred provenance, requires the outputs to be present
  already, refuses `non_reproducible` targets, and refuses `format_version > 1`
  as an error. Its types are private to `ox-cli`.
- **NFS caches negative lookups.** `ox_core::output_visibility` documents it: a
  `stat` that found nothing keeps failing for 30 to 60 s by default even after
  another host creates the file; reading the parent directory revalidates it.
  `output_exists` applies that, and `wait_for_outputs` adds a bounded retry.
- **ADR-021's path check runs before the pre-scan.** `ox run` calls
  `ox_core::output_path::validate_output_paths` before
  `determine_execution`, so every declared output has been accepted against
  the declared roots before any restore writes a file.

## Options

**A. One entry file per computation key.** After its blobs, a run publishes
`<remote>/entries/<k0k1>/<cache_key>.json`, written to a staging name and
renamed into place. A reader opens exactly the file named by the key it
computed. This is the split between a content-addressed store and an action
cache used by remote build systems such as the Bazel remote execution API.

**B. One shared index rewritten in place.** A single `index.json` (or an SQLite
database) in the remote directory maps keys to outputs; each run reads,
updates and rewrites it, under a lock.

**C. The adoption manifest as it is.** Runs write
`oxymake.cache-adoption-manifest` documents into the remote directory, either
one batch per run or one single-entry document per unit named by key, and
readers adopt them as `ox cache-import` does.

**D. One append-only journal per writer.** Each run appends one JSON line per
recorded unit to `<remote>/journal/<run-id>.jsonl` (the ADR-020 run id makes
the file single-writer); a reader loads every journal into memory at start.

### Evaluation

| Criterion | A. Entry per key | B. Rewritten index | C. Adoption manifest | D. Journals |
| --- | --- | --- | --- | --- |
| Concurrent writers on a shared filesystem | Each publish is a rename of a file no one else writes. Two heads publishing the same key both produce a complete file; one wins. No lock. | Read-modify-write across hosts loses updates without a lock. `flock`/`fcntl` on NFS depend on the client and lock daemon; SQLite's documentation warns against network filesystems because their locking is unreliable. | Batch form: one file per run, no conflict. Per-key form: as A. | Single writer per file, no conflict. |
| NFS visibility | A missing entry reads as a miss. A negative lookup can be revalidated by reading the parent directory, as `output_exists` does. The worst case is a re-execution. | The whole index is one file: a reader may see an older version for the attribute-cache interval; a reader during a rewrite needs rename semantics to avoid a torn file. Worst case is a miss. | As A or D depending on form. | Appended data becomes visible to another client only after the writer closes or flushes and the reader revalidates; a reader may see a truncated last line. Worst case is a miss. |
| Interrupted writer | Every unit recorded before the interruption is already published. A crash between blob and entry leaves blobs without an entry: wasted space, no wrong hit. | Units recorded since the last rewrite are lost unless the index is rewritten per unit, which multiplies the lost-update window. | Batch form: an interrupted run publishes nothing unless it writes per unit. Per-key form: as A. | Published up to the last complete line. |
| Lookup cost | One `open` per job, by name. | One read of the whole index per run; rewrite cost grows with the index. | Batch form: no lookup by key, because the document has no key; a reader must recompute keys for every entry it finds. Per-key form: as A. | Read and parse every journal ever written at each run start; needs compaction. |
| Hash verification on restore | Entry names output hashes; blobs are verified by `fetch` and the restored files re-hashed by `content` validation. | Same. | Same; `ox cache-import` already re-hashes. | Same. |
| `cache_platform` | Under `exact` the key contains the platform, so other platforms never find the entry; under `any` they do. The entry carries origin platform and scope for the local row. | Same content, same property. | Same fields exist in the adoption entry. | Same. |
| Outputs outside the project (ADR-021) | Entry paths are matched against the local job's declared outputs; the destination is always the local declared path, already checked. | Same. | Adoption entries store declared path strings without normalization; usable once normalized as in A. | Same as A. |
| Versioning | `kind` + `format_version` per entry; a reader skips what it does not understand. Old entries stay readable by name. | One version for the whole index: a format change forces every writer and reader to move together. | Ties the remote format to the adoption manifest's version, which `ox cache-import` treats as a hard error above 1. | Per line, as A, but a reader must parse every version ever written. |
| Compatibility with ADR-019 | Leaves `cache-export`/`cache-import` unchanged; reuses the entry field vocabulary. | Neutral. | Couples two contracts: remote needs (a key, per-unit publication) would change the adoption format, and `cache-import` would accept documents written for a different trust path. | Neutral. |
| Recall of one bad entry | Delete one file. | Rewrite the index. | Batch form: edit or delete a whole run's document. | Rewrite journals. |

Hash verification, platform handling and the path rule are properties of what
an entry contains and of the restore procedure, not of the storage layout; they
are listed so the comparison is complete. The layouts differ on concurrency,
interruption, lookup, versioning and recall, and on those A is the only option
without a weakness that matters for #66. D is the closest alternative: it is
safe under concurrency but turns every run start into a scan of the whole
history. C in its per-key form is A with the adoption document's name; the
decision below keeps A's addressing and the adoption entry's field names, under
a separate `kind`.

## Decision

Option A.

### Layout

```text
<remote>/
  ab/abcdef…                      blobs, unchanged
  entries/
    3f/3f9c…e1.json               one entry per cache key
```

`entries` is not a two-character hex prefix, so the entry tree cannot collide
with blob prefix directories, although cache keys and content hashes are both
64 hex characters.

### Entry format

```json
{
  "kind": "oxymake.remote-cache-entry",
  "format_version": 1,
  "producer_version": "0.8.0",
  "key_format": "oxymake-cache-key-v7",
  "cache_key": "3f9c…e1",
  "outputs": [
    { "path": "build/counts.parquet", "content_hash": "a41b…07" }
  ],
  "provenance": {
    "input_hashes": [["data/a.parquet", "77d0…3c"]],
    "job_spec_hash": "c2e8…9a",
    "reproducibility": "deterministic"
  },
  "origin_platform": "linux/x86_64",
  "platform_scope": "exact"
}
```

`outputs`, `provenance`, `origin_platform` and `platform_scope` have the same
names and wire spellings as in an adoption manifest entry, so one set of serde
types serves both. The `kind` is distinct: an entry is not an adoption manifest,
`ox cache-import` does not accept it, and the two formats are versioned
independently. The entry carries no timestamp, so two heads that compute the
same deterministic unit with the same `ox` version write identical bytes.

**Output paths.** Each `path` is the declared output path made absolute against
the project root and normalized lexically (the ADR-021 rule), then written
relative to the project root when it lies under it and absolute otherwise. No
symlink resolution: the writer's outputs exist and the reader's may not, and
canonicalizing on one side only would make the two disagree. The rule is the
same on both sides; `output_records.path` is not copied verbatim.

### Publish

In `SchedulerCache::record_with_hashes`, after the local row is written:

1. Store every output blob.
2. If any blob store failed, publish no entry. An entry never names a blob its
   writer did not publish.
3. If `entries/<k0k1>/<cache_key>.json` already exists, parses, has the same
   `cache_key`, names the same output hashes as the unit just recorded, and
   every blob it names exists, publish nothing. Otherwise write the entry to a
   staging name in the same directory and rename it into place, replacing any
   existing entry.

The staging name is unique across hosts: it is derived from the ADR-020 run id
of the publishing run (minted before the scheduler's cache is built, so it is
known at every publish site) and a per-process counter. The blob store's existing staging name (PID plus
sub-second nanoseconds) moves to the same scheme. Publish failures print a
warning and never fail the run, as blob stores do today.

Two heads that publish the same key at the same moment both write complete
files; the later rename wins. For a deterministic rule the two files describe
the same bytes. For an `approximate` or `seed_deterministic` rule they may
name different outputs; each is a valid result of the computation and its blobs
exist. Outside such a race, a unit that already has a valid entry is restored
rather than executed, so a unit executes with an entry in place mainly when it
was forced (`--forcerun`). A forced run is a statement that the previous result
should not be reused, and the key does not always see why (a call block's
module source is not part of the key), so step 3 replaces the entry whenever
the recorded hashes differ.

Publication is per unit, at the moment the unit is recorded. An interrupted run
has therefore published every unit it recorded. Under Ray, units are recorded
while `ox run --follow` is attached; a task that completes on the cluster after
the head has stopped is not recorded by `ox status` reconciliation and is not
published (see Open questions).

### Restore

In `restore_remote_outputs`, when no local `cache_entries` row exists for the
key:

1. Read `entries/<k0k1>/<cache_key>.json`. Before concluding it is absent, read
   the parent directory once, as `output_exists` does; do not wait. Refuse a
   file larger than 1 MiB.
2. Treat the entry as a miss, with one warning per run and kind of defect, when
   it does not parse, its `kind` differs, its `format_version` is higher than
   supported, its `cache_key` differs from the requested key, its
   `job_spec_hash` differs from the local job's, its `platform_scope` differs
   from the local rule's, or, under `exact`, its `origin_platform` differs from
   the local platform. Most of these cannot happen for a well-formed entry,
   because the key binds them; they detect an entry stored under the wrong
   name or written by a defective producer.
3. Normalize the local job's declared outputs with the rule above. The set must
   equal the entry's set of paths; otherwise it is a miss. The destination of
   every restored file is the local declared path, which ADR-021's check
   accepted before the pre-scan. A path read from the entry is never used as a
   destination.
4. For each output: if the destination exists and hashes to the entry's value,
   keep it; otherwise fetch the blob to a staging file beside the destination.
   `fetch` verifies the blob against its hash. If any output fails, remove the
   staging files and report a miss; no declared output has been touched. If all
   succeed, rename the staging files into place.
5. Write a local `cache_entries` row with the entry's provenance, its
   `origin_platform` (not the local platform) and its `platform_scope`, and mark
   it as restored from a remote (a new `restored` column beside `adopted`, which
   keeps its meaning of an explicit `ox cache-import`).
6. Return the result of the existing `is_cached` check, which re-hashes every
   output under `content` validation.

The pre-scan and the scheduler's cache check both go through this function, so
there is one restore path for every executor. `ox run --dry-run` keeps passing
no remote to its pre-scan: it neither reads entries nor fetches blobs, so with
fresh local state it reports as runs the units a real run would restore.

### What the remote entry is trusted for

Verification establishes that the restored bytes are the bytes the entry names.
It does not establish that the entry describes the output of that computation:
a writer able to write the remote directory can publish a wrong entry with
matching blobs.

This is a new grant of trust. Today a writer of the remote directory can only
offer bytes for hashes that the reader's local `cache.db` already names, and
`fetch` rejects anything else; the `RemoteCache` trait documents writers of the
directory as a threat for that reason. With entries, a writer of `entries/`
chooses which outputs satisfy any key a reader computes. After this decision,
anyone who can write the remote directory is trusted as a producer for every
run that uses it, as anyone who can write `cache.db` is for one checkout. The
boundary is the directory's filesystem permissions. Restore step 5 makes every
restored row enumerable, and the per-key file makes one bad entry removable
without touching the others.

### Versioning

- `kind` identifies the document; `format_version` is 1.
- Within version 1, fields may be added; readers ignore unknown fields.
- A change that a version-1 reader would misread bumps `format_version`. A
  reader that finds a higher version treats the entry as a miss with a warning,
  not an error: a fleet with mixed `ox` versions re-executes instead of failing.
- A change of `CACHE_KEY_FORMAT_VERSION` changes every key, so entries written
  under the old format are never looked up again. `key_format` is recorded for
  diagnosis only.

### Reproducibility classes

Entries are published and restored for every class the local cache records,
including `non_reproducible`. The remote entry extends the local index to other
checkouts of the same workflow; the local cache already serves a recorded
`non_reproducible` unit as a hit, and the parser already rejects
`cache_platform = "any"` with `non_reproducible`, so such an entry is never
served across platforms. `ox cache-export` keeps its refusal: adoption is an
explicit transfer of a result to a machine without its inputs, which is a
different claim.

## Consequences

- A run with fresh local state, sharing the remote directory and the input
  data, skips every unit whose entry was published, including the units
  recorded by an interrupted run. A wiped `.oxymake/` with outputs still in
  place is covered by step 4 without fetching: the outputs are kept when their
  hashes match the entry.
- A visibility lag on a network filesystem or a defective entry produces a
  re-execution, never a restore of unverified bytes.
- The remote directory grows by one small file per recorded unit, spread over
  256 prefix directories, in addition to the blobs.
- `ox invalidate` removes local rows only. With `--cache-remote`, the next run
  would restore the invalidated unit from its remote entry. Recalling a remote
  result means deleting its entry file; increment 4 gives `ox invalidate` a way
  to do it.
- `cache.db` gains one column, `restored`, by the existing additive migration.
- A remote entry is a new compatibility surface, versioned on its own.
- Remote garbage collection is not addressed. Any later collector must delete an
  entry before the blobs it names, never the reverse.
- The `RemoteCache` trait gains entry methods with default implementations, so
  the S3 and GCS stubs are unaffected until they are implemented.

## What this ADR does not do

- It does not solve ADR-019's case, a consumer that lacks the raw inputs: the
  lookup uses a locally computed key, and that key needs the inputs. `ox
  cache-export` / `ox cache-import` remain the path for that case.
- It does not change the cache key, `cache_platform`, or the adoption manifest.
- It does not make copying `.oxymake/` a supported transfer.
- It adds no lock, daemon or network service.

## Implementation increments

1. **Transport.** Add `fetch_entry(key) -> Option<bytes>` and
   `store_entry(key, bytes)` to `RemoteCache` with default implementations;
   implement them in `DirectoryCache` under `entries/`, with run-id-based
   staging names (also for blobs) and the parent-directory revalidation. Tests:
   round trip, no staging file left, two concurrent publishers leave one
   parseable entry, oversize entry refused.
2. **Publish and restore.** Shared entry types for the adoption manifest and the
   remote entry; publish in `SchedulerCache::record_with_hashes` after all blob
   stores succeed; restore in `restore_remote_outputs` per the procedure above;
   the `restored` column. Tests for each miss condition in Restore step 2 and
   for the all-or-nothing fetch.
3. **Acceptance test for #66.** Two checkouts at different absolute paths with
   disjoint `.oxymake/` directories and one remote directory. The first run
   stops at a failing middle unit; the second run skips every unit the first
   recorded, re-executes the rest, and re-executes a unit whose blob was
   tampered with. One case uses an output under an absolute config root. One
   case uses the Ray driver stub to check that a task recorded under `--follow`
   is published.
4. **Recall.** `ox invalidate --cache-remote <dir>` removes the entries of the
   invalidated keys (entries only, not blobs).
5. **Documentation.** The cache chapter (continuation over a shared directory,
   and the boundary with `cache-import`), the Ray guide sentence quoted above,
   the `DirectoryCache` and `RemoteCache` doc comments, and a `CHANGELOG.md`
   entry under Added.

## Alternatives Considered

The four options are evaluated above. In short: **B** needs a lock that a
shared filesystem does not reliably provide, and loses or delays the units of an
interrupted run. **C** either has no lookup by key (batch form) or is A under
another document's name, coupling the remote format to the adoption contract and
to `ox cache-import`'s hard version check. **D** is safe under concurrency but
makes every run start read the whole history and needs compaction.

Two variants of A were also considered:

- **No-replace publication** (`link(2)` or `renameat2(RENAME_NOREPLACE)`) to
  make the first writer win in every case. `RENAME_NOREPLACE` is Linux-only and
  not supported over NFS; hard links are unavailable on some mounts. A first
  writer that always wins would also keep a forced run from replacing an entry,
  and the race outcome it prevents is valid either way.
- **Reading the entry path as the restore destination.** Simpler, but it lets a
  shared, writable file choose where `ox` writes, which ADR-021 exists to
  prevent. Rejected.

## Questions settled at acceptance

- `ox status` does not publish entries when it reconciles Ray task records after
  the head has stopped. Publication happens where the local entry is recorded;
  a later increment can extend it if a reader needs it.
- `restored` is a new marker beside `adopted`; `adopted` keeps its meaning of
  an explicit `ox cache-import`.
- Recall through `ox invalidate` (increment 4) ships separately, after
  increments 1 to 3.
- Entries follow the local cache, including `non_reproducible` units, as
  described under Reproducibility classes; `ox cache-export` keeps its refusal.
- Reading entries requires no opt-in beyond `--cache-remote`, which is already
  marked unstable. The documentation of `--cache-remote` states the trust it
  grants to writers of the remote directory, as described above.

## Amendment — 2026-11-10: rules added during implementation review

An adversarial review of the first implementation found cases that the text
above leaves open or gets wrong. The implementation follows these rules, which
take precedence over the sections they amend.

- **Publish, step 3.** An existing entry whose `format_version` is higher than
  the producer supports is never replaced. The producer reads only the
  `kind` and `format_version` header before deciding. Without this rule, two
  `ox` versions sharing one directory would overwrite each other's entries on
  every run, and both would keep re-executing.
- **Restore, step 2.** An entry whose recorded input hashes differ from the
  local job's input hashes is a miss, like the other mismatches listed there.
- **Restore, step 5.** The restored row records the local rule's
  reproducibility class, not the class stored in the entry. The class is not
  part of the cache key, so a producer could otherwise mark as `deterministic`
  a result that the reader's rule declares `non_reproducible`, and
  `ox cache-export` would then export it.
- **Restore, scope.** Jobs that will execute anyway are not restored: jobs
  forced with `--forcerun`, and jobs excluded by `--until` or `--omit-from`.
  This applies to the pre-scan and to the scheduler's cache check alike.
- **Restore, step 3.** The local declared outputs and the entry's paths are
  compared as sets after normalization.
- **Restore, step 4.** The staging fetch is all-or-nothing, but the final
  rename loop is not a transaction. If a rename fails after earlier ones
  succeeded, some declared outputs are already replaced, no row is written,
  and the unit re-executes. The cache chapter documents this window.
