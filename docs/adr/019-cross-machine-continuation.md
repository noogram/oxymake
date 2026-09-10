# ADR-019: Cross-Machine Continuation — Adoption Is the Primitive, `cache_platform` Is the Flag

## Status
Accepted (operator arbitration 2026-09-10 on delib-20260910-b20f: adoption-primitive framing kept, provenance columns block the release, issue #12 stays decoupled).

## Metadata

- **Date:** 2026-09-10
- **Kind:** `decision`
- **Family:** `CAS`
- **Supersedes:** `none` (narrows the shareability claim of `ADR-001` L42
  without abrogating the cache-key mechanism; see [Sunset](#sunset))

## Context

[noogram/oxymake#7](https://github.com/noogram/oxymake/issues/7) asks how a DAG
can be continued on another machine. The reporter's workflow: heavy upstream
stages run on a Linux box that holds the data — *because the data is too big to
move* — and final model fits run on a macOS box that has the GPU.

The cache key (`crates/ox-cache/src/key.rs`, format `oxymake-cache-key-v5`) is

```text
blake3( format_version ‖ rule_source ‖ sorted (path, input_hash) pairs
      ‖ params_hash? ‖ env_hash? ‖ shell_executable?
      ‖ clean_outputs ‖ platform )
```

with `platform = "<os>/<arch>"` from `current_platform()`. A job built on
`linux/x86_64` therefore can never be a hit on `darwin/aarch64`, even when every
input file is byte-identical and every output is a deterministic data file.
`ADR-001` L42 promises "Cache is shareable across machines (same content = same
hash)"; with the platform term that promise holds only between machines of the
same OS/arch.

The issue proposes a per-rule opt-out, `cache_platform = "any"`. Deliberation
`delib-20260910-b20f` (architect · tolnay · popper · torvalds) examined it under
a fixed operator premise — *the default does not change; the platform term stays
in the key by default* — and returned a finding that reframes the decision.

### The finding: the opt-out alone is inert

Three panelists independently established, from the tree, that removing the
platform term delivers **zero cache hits** for issue #7. Two blockers, in
series:

1. **Machine B cannot compute the key.** `job_cache_key`'s input-hashing
   closure returns `None` when a declared input is absent
   (`crates/ox-cli/src/commands/run.rs:333`). The reporter's raw data is not on
   the Mac, so no key is computed at all, the producer is *scheduled*, and the
   run fails on a missing prerequisite. **The platform term is never
   consulted.**
2. **Even with a computable key, no local row exists.** `check_cached` returns
   `Miss` the moment there is no `cache_entries` row for the key
   (`crates/ox-cache/src/lookup.rs:350-358`), and the only code that creates one
   is `CacheStore::record`, called *after* a job runs (`lookup.rs:478`).

Blocker 2 is general — it holds even for users whose two machines carry
identical inputs. Blocker 1 is the reporter's specifically. Neither is addressed
by what enters the cache key.

Two escape routes were examined and refuted from code rather than assumed:

- **The remote cache is not the answer.** `restore_remote_outputs` calls
  `store.get(cache_key)` and returns `false` without a *local* entry
  (`run.rs:456-459`) — it transports bytes, never metadata. B must already know
  the key and the entry to fetch anything, which is the exact thing B cannot do.
  `crates/ox-cache-remote`'s S3/GCS backends are trait stubs; only
  `DirectoryCache` is reachable from the CLI.
- **Copying the databases is not a supported transfer.** The state DB is not on
  the cache-hit path at all (`is_cached` consults `cache.db`; `jobs.cache_key`
  in `state.db` is written but never read to decide a hit), and copying
  `cache.db` carries another machine's `mtime_secs`/`size` in `output_records`
  (`lookup.rs:151-152`), degrading every output to a full re-hash under the
  default `mtime+hash` validation (`ADR-006`).

### What the reporter actually built

Their workaround is a study-level script that publishes verified upstream
products with a manifest of source hashes, then generates a **derived DAG** in
which producer rules are deleted and their outputs become leaves — kept in sync
with the real `Oxymakefile.toml` by hand.

That script does two things the engine has no word for: it *asserts* that files
are trustworthy products with given hashes, and it *prunes the producers*. Only
the pruning makes the Mac run possible, because pruning is what removes the
demand for absent raw inputs. A cache flag cannot express pruning.

Underneath is a structural gap: **the engine has exactly two notions of
completion — `run.rs` executed it, or nothing.** Every other legitimate way a
result can exist (adopted from a peer, published by a study script, filled from
a remote cache) must be faked outside the engine by mutating the workflow file.

### What the engine already knows

`ArtifactProvenance { input_hashes, job_spec_hash, reproducibility }`
(`crates/ox-core/src/model.rs:437`) is built at `run.rs:557-565` and persisted
into `cache_entries` (`reproducibility_class`, `input_hashes_json`,
`job_spec_hash`; `lookup.rs:140-146`). It is **not** an unwired struct — only
the `job_history` copy is `None` (issue #12). Note what it does not contain:
*platform*. `job_spec_hash` is assembled at `run.rs:381-403` from rule source,
params, env, shell and `clean_outputs`, and deliberately excludes the platform.
The engine's own provenance model already separates *what computation this was*
from *where it ran*. The cache key is the thing that conflated them.

## Decision

Three parts, shipping together.

### 1. `cache_platform` — per-rule, string enum, default `"exact"`

```toml
[rule.merge_counts]
input  = ["data/*.parquet"]
output = "build/counts.parquet"
shell  = "duckdb -c '...'"
cache_platform = "any"        # default: "exact"
```

- **Granularity: rule-level only.** There is no workflow-level default, no
  `include`-level setting, no `--cache-platform` CLI flag, and no `OX_*`
  environment variable. **Precedence rules are therefore trivial by
  construction: there is exactly one place the value can come from, so there is
  nothing to resolve.** This is the decision, not an omission — see
  [Alternatives](#alternatives-considered).
- **Value space: a string enum**, `"exact"` (default, current behaviour) and
  `"any"`. Not a boolean. A string enum grows by adding an accepted value
  (`"arch"`, if os-independent-but-arch-dependent is ever wanted); a boolean can
  only grow by adding a second field that can contradict the first.
- **Rust side:** `PlatformScope` is `#[non_exhaustive]` on day one, so adding a
  variant later is not a breaking change for downstream `match`. (The existing
  `ReproducibilityClass` is a bare `pub enum`; that mistake is not copied.)
- **Parse behaviour:** an unknown value is a hard `ParseError::InvalidField`,
  following `clean_outputs` (`crates/ox-format/src/parse.rs:849-861`) and
  **not** `reproducibility` (`parse.rs:862-868`, which silently falls back to
  the default). An unverifiable safety assertion must never be silently
  misread.
- **One combination the parser rejects:** `cache_platform = "any"` together with
  `reproducibility = "non_reproducible"`. Asserting cross-platform equivalence
  of outputs that are not equivalent to themselves on one machine is incoherent,
  and both fields are visible in the same table. `approximate` and
  `seed_deterministic` are **not** rejected — approximate cross-platform reuse
  is the reporter's actual case.

**Key encoding.** The platform slot becomes presence-framed:
`update_opt_field(hasher, "platform", scope_is_any.then_some(None))` — i.e.
`Some(platform)` under `"exact"`, `None` under `"any"`.
`update_opt_field(h, tag, Some(v))` *literally calls* `update_field(h, tag, v)`
(`crates/ox-core/src/hashing.rs:41-44`), so non-adopters' keys are byte-identical
and the golden constant `21323fdb…` (`key.rs:472`) is unchanged. Adopters get
`update_absent_field` — a *tagged* absence, injectively distinct from every
present platform by construction. This is preferred over hashing a sentinel
string `"any"`, which would rely on no real platform ever being that string
(true today only because `current_platform()` contains a slash — an accident,
not a guarantee), and over dropping the field entirely, which would leave an
untagged hole that the next appended field turns into a real ambiguity.

**Implementation constraint (load-bearing — see the falsification test).** The
real platform string MUST still be supplied to `compute_cache_key`, with
suppression happening *inside* it based on a separate `platform_scope` field on
`CacheKeySpec`. An implementation that instead passes `platform: None` at the
call site makes the invariance test a tautology.

### 2. Provenance: two columns on `cache_entries`

```sql
ALTER TABLE cache_entries ADD COLUMN platform TEXT;        -- producing machine's current_platform()
ALTER TABLE cache_entries ADD COLUMN platform_scope TEXT;  -- "exact" | "any", in force at key time
```

- `platform` answers *from which platform did this artefact come?* Without it a
  cross-platform hit is indistinguishable from a local one.
- `platform_scope` answers *was this entry eligible to be served
  cross-platform?* — distinguishing a deliberate opt-in from a key collision.

This is the **minimum** that makes a wrong `cache_platform` claim refutable
after the fact, and it **blocks the release**. Not for purity: without it there
is no query that distinguishes a corrupted output from a correct one, and **no
way to enumerate which outputs are suspect in order to invalidate them**. A
defect that cannot be enumerated cannot be recalled. The cost is two columns and
two bind parameters in an `INSERT` that is already written.

These columns go on `cache_entries`, not `job_history`, because
`cache_entries` already carries `ArtifactProvenance`, is keyed by `cache_key`,
and is where the cross-machine artefact lives. `job_history` is per-run audit.

### 3. `ox import` / `ox export` — **in scope, ships in the same release**

Because of the finding above, `cache_platform` shipped alone would close issue #7
with a feature that cannot produce the outcome the issue asks for.

- **`ox export <targets> --manifest <path>`** on machine A serializes what
  `cache.db` already stores: for each target, the output paths with their content
  hashes, plus `ArtifactProvenance` (input hashes, `job_spec_hash`,
  `reproducibility`) and the origin `platform`.
- **`ox import <manifest>`** on machine B requires the output files to be present
  locally, **re-hashes them and refuses on mismatch**, computes the *local* cache
  key, and writes a `cache_entries` row with provenance, `platform` (the
  origin's, from the manifest) and `platform_scope`. It refuses when the rule is
  `non_reproducible`.

This is a command, not a subsystem: no new crate, no daemon, no network. The
closest analogue, `crates/ox-cli/src/commands/invalidate.rs` (261 lines), already
performs the mirror-image "resolve targets → compute cache keys → mutate
cache.db" traversal, and `CacheStore::record` already accepts
`Option<&ArtifactProvenance>`.

### The preserved property, and its falsification test

**Property (successor to `ADR-001` L42).** *For any rule declaring
`cache_platform = "any"`, the cache key is a function of exactly the v5
ingredients minus `platform`: two jobs whose `(rule_source, sorted (path,
input_hash) pairs, params_hash, env_hash, shell_executable, clean_outputs)`
tuples are equal produce the same cache key on every platform, and two jobs
differing in any one of those remaining ingredients still produce different cache
keys.*

Note what the property does **not** claim: it says nothing about whether the
*outputs* are identical across platforms. That is the user's assertion, not the
engine's property. Conflating the two is how L42 got into trouble.

**The falsifier**, as an assertion a test can make — in `crates/ox-cache/src/key.rs`
tests, alongside the existing `changes_with_platform`:

```rust
#[test]
fn platform_any_suppresses_platform_but_nothing_else() {
    let inputs = pairs(&[("a.txt", "aaa")]);

    // (1) INVARIANCE — the key ignores the platform argument under `Any`.
    let mut s1 = spec("echo hello", &inputs, None, None);
    s1.platform_scope = PlatformScope::Any;
    s1.platform = "linux/x86_64";
    let mut s2 = s1.clone();
    s2.platform = "macos/aarch64";
    assert_eq!(compute_cache_key(&s1), compute_cache_key(&s2));

    // (2) NON-COLLAPSE — every remaining ingredient still discriminates under
    //     `Any`; dropping platform did not vacate a framing slot.
    let mut s3 = s1.clone();
    s3.clean_outputs = CleanOutputs::Never;
    assert_ne!(compute_cache_key(&s1), compute_cache_key(&s3));
    let mut s4 = s1.clone();
    s4.shell_executable = Some("/bin/zsh");
    assert_ne!(compute_cache_key(&s1), compute_cache_key(&s4));
    let mut s5 = s1.clone();
    s5.params_hash = Some("p");
    assert_ne!(compute_cache_key(&s1), compute_cache_key(&s5));

    // (3) DEFAULT UNCHANGED — under `Exact`, `golden_key_stability`'s constant
    //     (`21323fdb…`, key.rs:472) is byte-identical. Asserted by leaving that
    //     test untouched: adding `platform_scope` must NOT change it.
}
```

Part (1) is only meaningful under the implementation constraint stated above.
Part (3) is where the non-obvious content lies: it forbids any implementation
that adds an unconditional `update_field(hasher, "platform_scope", …)`, which
would change every existing key and invalidate every user's cache for a feature
they did not enable. A second golden constant is added for `PlatformScope::Any`.

**Named assumption** (`ADR-015` boundary style):
**`PlatformEntersTheKeyOnlyViaCacheKeySpec::platform`.** A single-process unit
test cannot assert that a darwin binary and a linux binary agree; it asserts only
that the key *function* is invariant under its platform argument. The residual
gap — that no other platform-dependent value reaches the key by another route (a
shell path that differs by OS, an env hash reading a platform-specific lockfile,
an absolute path not root-relativized on one machine) — is not closed by any test
in this repo. It is partially corroborated for free: the golden constants are
asserted on ubuntu in CI and on darwin on the maintainer's machine, a genuine
two-platform observation of the same constant. That is a habit, not a guarantee.

### Cache key format version

**No bump for this ADR.** The presence-framed encoding leaves non-adopters' keys
byte-identical. Issue #8 (hashing `pyproject.toml`) lands on the same milestone
and genuinely does add to the key; it carries the single `oxymake-cache-key-v6`
bump and the single golden-constant update. Land this ADR's change first
(encoding-preserving), then #8. **Users pay one full cache invalidation on this
milestone instead of two.**

## Consequences

**Easier:**

- The reporter deletes the derived-DAG script. Adopted products become
  first-class cache entries; downstream jobs key off product hashes exactly as
  they do today; the pruning is a property of recorded state rather than of a
  hand-maintained rewrite of `Oxymakefile.toml`.
- A cross-platform hit becomes *diagnosable*: `cache_entries` says the entry was
  produced on `linux/x86_64` under `scope = any` while the consumer is
  `darwin/aarch64`, and `input_hashes_json` + `job_spec_hash` let the job
  identity be re-derived and re-run for byte comparison.
- Suspect artefacts become **enumerable**, hence recallable, by a single query
  over `platform_scope`.
- `ADR-001` L42's promise becomes accurate: shareable across machines of the same
  platform by default, and across platforms exactly where a rule declares it.

**Harder:**

- A second unverifiable user assertion enters the rule surface, alongside
  `reproducibility`. Surface design makes the unsafe case *deliberate, local and
  enumerable* — no plural form, so unprotecting *n* rules costs *n* typed lines
  in the rules that own the risk, and the audit is `grep -n cache_platform` — but
  it does **not** make it *hard*. Whether a rule emits a machine-code artefact is
  invisible to the parser and always will be. A user who writes
  `cache_platform = "any"` on a rule running `cargo build` gets a wrong
  cross-platform hit and no spelling prevents it. **This is why part 2 blocks the
  release**: an unverifiable assertion whose failures are also unobservable is
  not a bounded risk.
- `cache_platform = "any"` is more dangerous than a wrong `reproducibility`
  claim, and the difference is precisely a recording gap. A wrong
  `reproducibility` claim fails on the machine where the inputs sit on the same
  disk — locally reproducible on demand. A wrong `cache_platform` claim fails on
  a *different* machine, from an artefact whose origin would otherwise be
  nowhere recorded.
- Two new CLI subcommands to maintain, and a manifest format that becomes a
  compatibility surface.

## Alternatives Considered

**A workflow-level `[config] cache_platform`, or a new `[workflow]` section
with rule-level override.** Rejected on three independent grounds. (i) `[config]`
is a *value namespace* (`resolver::Config { lists, scalars }`,
`crates/ox-core/src/resolver.rs:140`) consumed by wildcard expansion and guard
evaluation, with no schema and no validation: a user list named `cache_platform`
would collide with the engine setting, a guard could read it, and a typo would be
silently ignored. A cache-correctness switch in an unvalidated string map is
worse than the switch itself. (ii) A new `[workflow]`/`[settings]` section is a
permanent public surface that drags in a precedence chain and an `include`-merge
rule, creating the possibility that **an included file weakens the platform
binding of a rule it does not own**. (iii) Decisively: a workflow-wide switch is
the aggregate gesture whose absence *is* the safety argument. A single line whose
blast radius is the entire DAG, whose cost of over-application is zero at the
moment of application, and which is never unset because nothing ever fails
visibly again — six weeks later a rule that links a platform-specific binary
serves a Linux ELF on darwin, and the failure surfaces three stages downstream
with the cache reporting a hit. A user who genuinely wants forty rules unbound
can type forty lines; that cost *is* the safety property.

**Parser heuristics that detect compiled artefacts.** Sniffing the execution
block for `cargo`, `make`, `gcc`, or `.so`, or inferring from `env`. Rejected:
heuristic dressed as a guarantee. It fires on `echo "make it so"` and misses
`uv run build.py`. A container reference pins a runtime and argues neither way.

**A boolean `platform_independent = true`.** Rejected: no additive growth path,
and the only way to extend it is a second field that can contradict the first.
`cache_platform = "any"` also reads as an *assertion*; `platform_in_key = false`
reads as a *switch*, and this is an assertion.

**Shipping `cache_platform` alone and filing `ox import` as follow-up work.**
The tempting option: one field, one ADR, a milestone closed. Rejected because the
code says it delivers zero hits for issue #7 — B still cannot key the producer,
and no local `cache_entries` row exists. The reporter would read the release
note, try it, get a scheduled producer and a missing-input failure, and keep the
derived-DAG script. **A fix that closes the issue without deleting that script is
worse than leaving the issue open, because it removes the pressure to solve it.**

**Routing this through `crates/ox-cache-remote`.** Rejected from code, not
principle: `restore_remote_outputs` refuses without a local entry
(`run.rs:456-459`), S3/GCS are trait stubs, and the reporter's two machines may
share no network store at all. It solves byte transport; the reporter's bytes
already move. It becomes useful *after* adoption exists, because an adoption
record is what a remote cache would need in order to be trusted.

**Adding `cache_key` and `platform` to `job_history` as part of this work.**
Deferred to issue #12, deliberately and on the record. `job_history` is per-run
audit; the cross-machine artefact lives in `cache_entries`. An imported artifact
is not a job that ran here, and forging a `job_history` row for it would make the
ledger lie in a new way. Coupling this ADR to #12 would either block it
indefinitely or invite shipping the option ahead of it. #12 remains independently
worth fixing — it is also the evidence that the completion record was wanted
before issue #7 existed: `job_cache_key_with_components` computes the key, input
hashes, params and env hashes at `run.rs:406-423`, and `finalize_job_history`
(`crates/ox-state/src/db.rs:1370-1388`) throws all of them away.

## What this ADR is not doing

- Not changing the default. `platform` stays in the key under `"exact"`.
- Not bumping `CACHE_KEY_FORMAT_VERSION`. Issue #8 owns the single v6.
- Not making copying `.oxymake/` a supported transfer. It stays unsupported;
  `ox export`/`ox import` is the supported path.
- Not touching `crates/ox-cache-remote`.
- Not fixing issue #12.
- Not attempting to *verify* platform independence. The engine cannot, and this
  ADR does not pretend otherwise — it makes the claim deliberate, local,
  enumerable, and refutable after the fact, which is the most that is available.

## Sunset

`ADR-001` L42 ("Cache is shareable across machines (same content = same hash)")
is **narrowed, not abrogated**. Its accurate reading after this ADR: *the cache
is shareable across machines of the same platform by content alone; across
platforms it is shareable exactly for rules declaring `cache_platform = "any"`,
and only through `ox export` / `ox import`.* `ADR-001`'s `Status:` line gains a
`Partially superseded by ADR-019` note pointing at that line, per the corpus
Sunset rule.

## Provenance

Deliberation `delib-20260910-b20f` (formula `deep-think`), panel architect ·
tolnay · popper · torvalds. Nine framed sub-questions, all treated; zero
substitutions across eleven declared substitution hypotheses. One panel claim was
adjudicated against by reading the code: the assertion that presence-framing the
platform field would invalidate every existing key is false —
`update_opt_field(…, Some(v))` calls `update_field(…, v)` verbatim
(`crates/ox-core/src/hashing.rs:41-44`). Frame, per-persona responses, and
synthesis are retained with the molecule.
