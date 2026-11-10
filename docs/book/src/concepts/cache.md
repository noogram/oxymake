# Content-Addressable Cache

One of the most frustrating things about traditional build tools is the
**phantom re-run**: you check out a branch, and everything rebuilds even
though nothing actually changed. OxyMake eliminates this by using **file
content** as the source of truth, not timestamps.

## How It Works

Every time OxyMake runs a job, it computes a **cache key** from everything
that could affect the output:

```
cache_key = blake3(
    format_version ||
    rule_source_hash ||
    sorted((input_path, input_content_hash) pairs) ||
    params_hash ||
    env_content_hash ||
    shell_executable ||
    clean_outputs ||
    (platform if cache_platform = "exact")
)
```

Every field is length-framed with a domain-separation tag, so two
different job specifications can never hash to the same key. If the key
matches a previously computed result, the job is skipped. The key
includes:

- **Rule source hash** -- if you change the shell command, inline code, or
  function reference, the cache is invalidated
- **Input content hashes** -- blake3 of every input file's contents, bound
  to its path; parameter files and (in script mode) the script file itself
  count as inputs, so editing `script.py` invalidates the cache
- **Params hash** -- any parameters passed via `--set` or `[config]`
- **Environment content hash** -- the *content* of the referenced spec file
  (`requirements.txt`, conda YAML, nix expression), every
  `.python-version` in the workflow root and its parent directories (for uv),
  or the container image reference for Docker/Apptainer
- **Shell executable** -- the same command under `/bin/bash` and `/bin/zsh`
  can behave differently
- **Platform** -- OS and architecture (a Linux build is not reusable on macOS)

A rule may explicitly set `cache_platform = "any"` when its outputs are valid
across operating systems and architectures. The default is `"exact"`. This is
a user assertion: OxyMake records it for audit but cannot prove it.

Three exclusions to know about: `call`-mode function bodies are tracked only
if you declare the module as an input; `UV_PYTHON` and changes to the installed
interpreter set do not enter uv environment hashes (use a checked-in
`.python-version` when this matters); and mutable container tags are hashed as
written (pin images by digest -- `python@sha256:...` -- if you need re-pushed
tags to invalidate the cache).

## Why Not Timestamps?

Timestamps lie. Here are common situations where they cause phantom re-runs
in tools like Make or Snakemake:

| Scenario | What happens to mtime | Content changed? |
|----------|-----------------------|-------------------|
| `git checkout` | Reset to now | No |
| `cp` without `-p` | Reset to now | No |
| NFS clock skew | Arbitrary | No |
| CI fresh clone | All files are "new" | No |
| `touch` command | Updated | No |

## Validation Strategies (ADR-006)

OxyMake's cache validation is **pluggable** — you choose the right
speed/correctness tradeoff for your workflow:

| Strategy | Flag | Behavior |
|----------|------|----------|
| `mtime+hash` (default) | `--cache-validation=mtime+hash` | If mtime/size differ, compute BLAKE3 hash. Fast on steady-state, correct on change. |
| `mtime` (opt-in) | `--cache-validation=mtime` | Pure filesystem metadata (stat calls only). Fastest, but **never verifies content** — unsuitable for shared/multi-user caches. A partial output left by a hard kill (SIGKILL, power loss) counts as fresh if it is newer than the inputs. |
| `hash` | `--cache-validation=hash` | Always compute BLAKE3 hash. Bit-exact. Required for shared/remote caches. |

```bash
ox run                                  # default: mtime+hash (fast + content-verifying)
ox run --cache-validation=mtime         # Make-parity opt-in (no content check)
ox run --cache-validation=hash          # strict mode (CI)
OX_CACHE_VALIDATION=hash ox run         # via environment variable
```

Configure per project in `Oxymakefile.toml`:

```toml
[config]
cache_validation = "mtime+hash"
```

Remote caches automatically promote to `hash` regardless of the configured
strategy, because mtime is not meaningful across machines.

## Local Cache Metadata

Local cache metadata lives in `.oxymake/cache/cache.db`. It records the
computation key, output paths, content hashes, and validation metadata:

```
.oxymake/cache/
  cache.db
```

The outputs themselves remain at their declared workflow paths. Deleting the
local metadata makes jobs run again unless `--cache-remote` supplies matching
entries or you use `mtime` validation. It does not delete outputs or execution
history.

## Continuing a DAG on Another Machine

Suppose a Linux box holds a dataset that is too large to move and runs the
heavy preparation stages, while a macOS box has the GPU needed for the final
fits. The two checkouts must contain the same workflow definition. Mark only
the rule whose prepared data will cross the platform boundary:

```toml
[rule.merge_counts]
input = ["data/*.parquet"]
output = ["build/counts.parquet"]
shell = "duckdb -c '...your merge command...'"
cache_platform = "any"

[rule.fit_model]
input = ["build/counts.parquet"]
output = ["build/fits.rds"]
shell = "Rscript fit.R"
```

On the Linux box, run the heavy target and export its cache metadata:

```bash
ox run build/counts.parquet
ox cache-export build/counts.parquet -o counts.ox-cache.json
rsync -aR build/counts.parquet counts.ox-cache.json mac-gpu:/path/to/project/
```

This transfers the prepared product and manifest, but not `data/`. From the
root of the matching checkout on the macOS box, adopt the product and run the
fit:

```bash
ox cache-import counts.ox-cache.json
ox run build/fits.rds
```

Import re-hashes every output and rejects the whole manifest before changing
the cache if a hash, output set, job specification, platform scope, or
reproducibility policy does not match. Once imported, `build/counts.parquet` is
a trusted resolver leaf, so its producer is pruned even though the raw inputs
are absent on the Mac. Rules using the default `cache_platform = "exact"` can
only be imported on the producing OS and architecture.

`cache_platform = "any"` is an assertion by the workflow author, not a fact
OxyMake can verify. The surface makes the unsafe case deliberate, local, and
enumerable: there is no plural or workflow-wide form, so unprotecting *n* rules
costs *n* typed lines, and the audit is:

```bash
grep -n cache_platform Oxymakefile.toml
```

It does **not** make the unsafe case hard. Whether a rule emits machine code or
some other platform-specific artefact is invisible to the parser. For example,
putting `cache_platform = "any"` on a `cargo build` rule can serve a Linux
binary to macOS. Keep the default `"exact"` unless you can defend the rule's
outputs as cross-platform data.

OxyMake currently supports a shared filesystem directory as its remote
artifact backend. Point `--cache-remote` at a directory reachable from every
machine, for example a team NFS mount:

```bash
# Production: everything cached locally
ox run

# Use a shared directory; fetched artifacts are verified by BLAKE3.
ox run --cache-remote /mnt/team/oxymake-cache
```

`--cache-remote` promotes validation to `hash`: timestamps from another
machine are never trusted. The directory backend stores each output under its
content hash and verifies the hash after every fetch.

Each completed unit also publishes a computation entry under
`entries/<prefix>/<cache-key>.json`, after all its output blobs have been
stored. A checkout with fresh local state, the same workflow and raw input
bytes can therefore restore and skip those units using the shared directory
alone. Entries are published per unit, so work recorded before a failure or
interruption remains available. A wiped local state directory with matching
outputs can recover the index without fetching the blobs again. The design is
recorded in
[ADR-022](https://github.com/noogram/oxymake/blob/main/docs/adr/022-remote-cache-manifest.md).

Restoration checks the entry's job specification, platform scope, input
provenance and declared output paths. It verifies fetched blobs by BLAKE3 before installing any of a
unit's outputs, then validates all outputs by content. Missing, corrupt,
incompatible, oversized or non-regular entries produce a cache miss and
re-execution. Symlinked entries and entry-tree directories are rejected before
opening the entry, with a warning.
`ox run --dry-run` does not read remote entries or fetch outputs. With
`--cache-remote`, its header and uncached-output warning explain that listed
jobs may be restored instead of executed during a real run. Its JSON summary
includes `remote_entries_checked = false` only with `--cache-remote`. The
summary's `remote_restore_possible` is true only when at least one listed job
can use remote caching; each job event also reports this flag. Forced jobs
(and their forced consumers) and jobs with caching disabled report false.

A failed fetch leaves declared outputs untouched. The final installation
renames staged files one by one: if a later rename fails, earlier renames
remain installed, so the unit can have a partial set of restored outputs.
No restored cache row is written in that case; the unit re-executes.

Writers of the remote directory are trusted producers: they can choose which
output hashes satisfy a computation key. Hash verification detects corrupt
bytes; it does not prove that a writer performed the declared computation.
Use filesystem permissions to restrict who can write the directory.

Remote lookup still needs the raw inputs to compute a key. If the receiving
machine lacks those inputs, transfer outputs with
[`ox cache-export`](../reference/commands/cache-export.md) and
[`ox cache-import`](../reference/commands/cache-import.md) instead. These
explicit adoption manifests keep their separate format and validation rules.
Copying `.oxymake/` between machines is unsupported. S3 and GCS URLs are not
supported yet.

The local `cache.db` gains an additive, forward-only `restored` column (default
`0`). Remote restores set it to `1`, preserve the producing platform and
provenance, take the reproducibility class from the local rule, and leave
`adopted = 0`; `adopted` continues to mark explicit
`cache-import`. This cache migration is separate from `state.db`'s schema.
`ox invalidate` removes local rows and, by default, output files. Without
`--cache-remote`, a later run can restore the result again. To recall remote
results too, use `ox invalidate --rule NAME --cache-remote DIR` (or select with
`--output` or `--all`, and use `--cascade` with `--rule`). It removes entries addressed by the
invalidated local keys and the selected jobs' calculable current keys, even
with fresh local state. Current-key recall uses the workflow's default
configuration; `ox invalidate` has no `--set` or `--profile` flags. Entries from
runs using those overrides are recalled through local rows only. With `-f`,
remote recall uses `.oxymake` in the workflow directory and honours verified
adopted inputs. Inputs must be present to calculate a current key;
historical keys require a local record. `--all` does not clear unrelated
entries in the shared directory, and no blobs are removed. Unsupported newer
entry versions are also removed by this explicit recall. Review entry paths
with `--dry-run`; add `--json` for a structured list. `--keep-outputs` preserves
local output files. Recall does not reach checkouts that already hold a local
row for the key, and a concurrent run may publish the entry again.

## Cache and Materialization

The cache interacts with the materialization policy:

| Policy | Written to disk? | Cached? |
|--------|-----------------|---------|
| `always` (default) | Yes | Yes |
| `auto` | Only if needed | Yes, when materialized |
| `never` | No (memory only) | No |
| `final` | Only if DAG leaf | Yes, when materialized |

Outputs with `materialize = "never"` are kept in memory and never enter the
cache. This is a deliberate trade-off: you get speed at the cost of
reproducibility. The next `ox run` will recompute them.

## Managing the Cache

```bash
# Preview removal of orphaned cache metadata
ox clean --cache-only --dry-run

# Remove orphaned cache metadata
ox clean --cache-only --yes

# Remove all cache metadata
ox clean --cache-only --all --yes
```

## Why This Matters

The content-addressable cache means you can:

1. **Switch branches freely** without phantom re-runs
2. **Add new rules** without invalidating existing cached results
3. **Share computation** across machines of the same platform by default, and
   across platforms only for rules declaring `cache_platform = "any"` through
   [`ox cache-export`](../reference/commands/cache-export.md) and
   [`ox cache-import`](../reference/commands/cache-import.md)
4. **Resume interrupted runs** -- completed work is preserved
5. **Verify adopted bytes** -- import hashes every transferred output before
   recording it locally

## Incremental external datasets

Use per-rule `clean_outputs = "never"` with the local executor when an
idempotent script materializes a remote dataset incrementally. Declare the
real files as outputs: completed downloads survive reruns and failures,
while successful outputs are still hashed normally. The script owns checking
staleness and completeness. See the [output cleanup reference and S3
recipe](../reference/format.md#output-cleanup) for all three policies.
