# Erratum — OxyMake paper

This file lists the factual corrections applied to the OxyMake paper after
its arXiv v2 submission. Each entry quotes the superseded v2 wording, states
the correction, and points to the current v3 paper or a primary source that
settles it.

The published v2 text is the state of `docs/paper/oxymake-paper.tex` at git
commit `7b55bfb` (the version submitted to arXiv). Every quoted "v2 wording"
below can be located there with `git show 7b55bfb:docs/paper/oxymake-paper.tex`.

The corrections were triggered by Michael R. Crusoe's review (GitHub issue #1
on `noogram/oxymake`), which questioned the paper's characterisation of the
Common Workflow Language. Following up on that review led to a full re-check
of the paper's claims about other systems, about the framing of the problem,
and about OxyMake itself. This erratum records the outcome of that re-check.

---

## A. Framing and motivation

**The premise that the Make lineage detects change by timestamp.**
*v2 wording (abstract):* "Make-lineage workflow runners decide whether a job
must re-run from file-modification time (mtime, a timestamp)." *v2 wording
(introduction):* "Workflow engines in the Make lineage — Snakemake, Nextflow,
CWL runners — descend from a change-detection heuristic built on file
modification times."

This over-generalised. Timestamp comparison is GNU Make's mechanism and the
mechanism of any pure-mtime fast path, but it is not the change-detection
policy of the modern engines named alongside it. Snakemake 7 records
per-output provenance and does not compare live input/output timestamps;
Nextflow fingerprints file content under `cache 'deep'`; cwltool can key a
cache on content checksums. v2's own "honest accounting" paragraph already
conceded that Snakemake 7 re-runs zero jobs under mtime churn, which
contradicts the lineage-wide premise stated in its abstract and introduction.
v3 removes the lineage-wide claim and scopes the timestamp problem to where it
holds: GNU Make's live mtime comparison and any runner's pure-mtime fast path.
Source: v3 abstract and introduction; primary sources for each engine are in
section B below.

**The motivation, reformulated.**
*v2 wording:* the paper motivated content-addressing as a fix for a
change-detection heuristic that the whole Make lineage was said to inherit.

Because the lineage-wide premise does not hold, the motivation is restated
around what OxyMake actually does: it derives the rebuild decision from the
declared content of a job's inputs — rule source, input bytes, parameters,
environment, platform — rather than from filesystem metadata. The claim is
that a content-derived key is the right basis for the decision, not that every
prior engine got change detection wrong by using timestamps. v3 states the
motivation in these terms.
Source: v3 introduction ("A content-derived rebuild decision").

**Title: "Formally-Specified" withdrawn.**
*v2 title:* "OxyMake: A Formally-Specified, Content-Addressable Workflow
Engine."

What is formally specified is a set of TLA+ specifications that
model-check bounded safety properties of the concurrent state protocol for
two to three sessions. That is one subsystem checked at bounded scope, not the
engine, its resolver, or its cache semantics. The adjective claimed for the
whole what holds for one part, and is removed from the title. "Content-
Addressable" also becomes "Content-Addressed", the established term. The v3
title is "OxyMake: A Content-Addressed Workflow Engine".
Source: v3 title and the section on named invariants and formal
specifications (scope of the TLA+ specifications).

---

## B. Characterisations of other systems

**CWL is a standard, not an engine.**
*v2 wording:* "Workflow engines in the Make lineage — Snakemake, Nextflow, CWL
runners — descend from a change-detection heuristic built on file modification
times."

CWL is a vendor-neutral standard for describing command-line-tool workflows.
Steps are wired by explicit data links, and the specification prescribes no
change-detection policy. Grouping "CWL runners" with timestamp-based engines
attributes to the standard a mechanism it does not define. v3 describes CWL as
a specification and does not place it in a timestamp lineage.
Sources: [CWL Workflow v1.2 — WorkflowStepInput](https://www.commonwl.org/v1.2/Workflow.html#WorkflowStepInput),
[CWL CommandLineTool v1.2](https://www.commonwl.org/v1.2/CommandLineTool.html).

**CWL "verbose and lacks optimization capabilities."**
*v2 wording:* "The Common Workflow Language (CWL) provides a platform-
independent specification but is verbose and lacks optimization capabilities."

Both halves conflate the standard with its implementations. Verbosity is a
property of a document, not of a specification, and "optimization" — caching,
scheduling, reuse — is the responsibility of a runner, not of the language.
v3 removes the editorial judgement and describes CWL as a portable
specification whose execution behaviour depends on the runner.
Sources: [CWL v1.2 specification](https://www.commonwl.org/v1.2/),
[cwltool reference runner](https://cwltool.readthedocs.io/en/stable/).

**CWL reference implementations "retain mtime-era assumptions."**
*v2 wording:* "Nextflow's channel-based runtime and CWL's portable
specification retain mtime-era assumptions in their reference implementations."

The CWL reference runner, cwltool, does not depend on mtime for reuse: with
`--cachedir` it names a cache entry from a digest of a canonical description
of the command line, container, requirements, environment, and each input's
size and checksum. That is content-derived, not timestamp-derived. v3 drops
the "mtime-era assumptions" characterisation of cwltool.
Sources: [`cwltool --cachedir`](https://cwltool.readthedocs.io/en/stable/cli.html#cmdoption-cwltool-cachedir),
[CWL `File.checksum`](https://www.commonwl.org/v1.2/CommandLineTool.html#File).

**Nextflow default caching.**
*v2 wording:* the paper's discussion of mtime-based reuse implicitly grouped
Nextflow with timestamp-trusting engines.

Nextflow's standard file fingerprint uses the full path, size, and last-
modified time; `cache 'deep'` fingerprints file content; and a recorded task
is reused only when the run is launched with `-resume`. Task records are
always written, so the presence of a cache does not by itself mean reuse is
active. v3 states the fingerprint modes and the `-resume` condition.
Sources: [Nextflow process `cache` directive](https://docs.seqera.io/nextflow/reference/process#cache),
[Nextflow caching and resuming](https://docs.seqera.io/nextflow/cache-and-resume#modified-inputs).

**Cromwell "tightly coupled to cloud backends" / "a separate execution
engine."**
*v2 wording:* "the execution model is tightly coupled to cloud backends";
"WDL requires a separate execution engine (Cromwell, miniWDL)."

Cromwell's Local backend is pre-enabled and is the default. `cromwell run`
executes one workflow as a command-line process and exits; a server deployment
is required only for server-mode operation, not to run a workflow. The
"tightly coupled to cloud backends" characterisation does not hold for the
default local path. v3 states the run/server distinction.
Sources: [Cromwell backends](https://cromwell.readthedocs.io/en/latest/backends/Backends/),
[Cromwell Local backend](https://cromwell.readthedocs.io/en/stable/backends/Local/),
[Cromwell run and server modes](https://cromwell.readthedocs.io/en/latest/Modes/).

**Galaxy "sacrifices programmability."**
*v2 wording:* "Galaxy offers a web-based interface optimized for biologists but
sacrifices programmability."

Galaxy exposes a REST API and the BioBlend Python client for programmatic
control, and Planemo is a command-line tool for developing and testing Galaxy
tools and workflows. A web interface is one of several surfaces, not the only
one. v3 removes the "sacrifices programmability" claim.
Sources: [Galaxy API](https://docs.galaxyproject.org/en/latest/api_doc.html),
[BioBlend](https://bioblend.readthedocs.io/en/latest/),
[Planemo](https://planemo.readthedocs.io/en/latest/).

**WDL "every task must declare a full runtime block."**
*v2 wording:* "every input must be typed, every task must declare a full
runtime block."

In WDL 1.2 the `runtime` section and its individual attributes are optional. A
task need not declare a runtime block. v3 replaces the claim with the accurate
requirement (typed declarations) and drops the runtime-block assertion.
Source: [WDL 1.2 specification — runtime section](https://github.com/openwdl/wdl/blob/wdl-1.2/SPEC.md#runtime-section).

**Ray and Dask "require Python."**
*v2 wording:* "Frameworks like Ray and Dask provide distributed execution with
task-level parallelism but require Python."

Dask is Python-native, but Ray officially supports Java and provides a C++ API
in addition to Python. The defensible contrast is the programming surface (a
task/actor API rather than a declarative workflow model), not the
implementation language. v3 makes the contrast on surface, not language.
Source: [Ray — getting started and language APIs](https://docs.ray.io/en/latest/ray-overview/getting-started.html).

**Airflow and Argo and content-addressable caching.**
*v2 wording:* "Both systems excel at scheduling heterogeneous tasks across
distributed infrastructure but define workflows imperatively (Python or YAML),
limiting static analysis and content-addressable caching."

Both systems expose memoization or caching whose keys can be user-supplied and
therefore content-derived; the imperative definition does not preclude
content-keyed reuse. The accurate statement is that neither *automatically*
derives a reusable result key from the content of all declared file inputs.
v3 makes that narrower claim.
Sources: [Airflow tasks](https://airflow.apache.org/docs/apache-airflow/stable/core-concepts/tasks.html),
[Argo Workflows memoization](https://argo-workflows.readthedocs.io/en/latest/memoization/).

**Bazel and Buck "domain-specific features that build systems lack."**
*v2 wording:* OxyMake provides "domain-specific features (wildcard expansion,
environment management, gates) that build systems lack."

Bazel has target patterns for wildcard-style expansion, configurable
toolchains and platform constraints for environment management, and Starlark
extension points for policy. The claim that build systems lack these features
does not hold as stated. v3 compares one axis at a time, on semantics rather
than on presence.
Sources: [Bazel — target patterns](https://bazel.build/run/build#specifying-build-targets),
[Bazel — toolchains](https://bazel.build/extending/toolchains),
[Bazel — platforms and constraints](https://bazel.build/extending/platforms),
[Buck2 documentation](https://buck2.build/docs/).

**Bazel's "static dependency graph."**
*v2 wording:* OxyMake "positions closest to Bazel in scheduling strategy,
sharing its static dependency graph and content-addressable caching."

Bazel supports action-time input discovery (discovered inputs), so its graph
is not wholly static. v3 narrows the analogy to "analysis normally constructs
an action graph before execution" and acknowledges discovered inputs.
Source: [Bazel — dependency discovery](https://bazel.build/extending/rules#dependency-discovery).

**Nix and Guix "the same inputs always resolve to the same output."**
*v2 wording:* "the same inputs always resolve to the same output and a changed
input always forces a rebuild."

This conflates derivation identity with byte-reproducible output. The same
declared derivation inputs select the same store identity, and changed declared
inputs select a different derivation; whether the build produces bit-identical
bytes is a separate property that a content-derived store name does not confer.
v3 states the distinction.
Sources: [Eelco Dolstra, *The Purely Functional Software Deployment Model*, PhD thesis, 2006](https://edolstra.github.io/pubs/phd-thesis.pdf),
[Nix reference manual — store derivations](https://nixos.org/manual/nix/stable/),
[GNU Guix manual — The Store](https://guix.gnu.org/manual/en/html_node/The-Store.html).

**"Snakemake pipelines port directly."**
*v2 wording (abstract):* "keeping the Make rule model so Snakemake pipelines
port directly."

The paper's own compatibility section lists embedded Python expressions and
`run:` blocks as unsupported by the translator. "Port directly" overstates the
coverage. v3 narrows the claim to "many rule-oriented Snakemake pipelines
translate directly — embedded Python expressions and `run:` blocks require
manual migration", matching the compatibility section.
Source: v3 abstract and the Snakemake translation section.

---

## C. OxyMake's own claims

**Benchmark sizes: 100 / 1,000 / 10,000 were not the job counts.**
*v2 wording:* the three benchmark tables labelled their rows "100 / 1,000 /
10,000" jobs, and the prose read "resolves a $10^4$-job DAG in 69 ms" and
"101.9× at 100 jobs to 33.3× at $10^4$".

The benchmark harness builds a four-layer DAG of $3N{+}2$ jobs, and the
measured scales are $N=33$, $333$, $3333$ — that is, 101, 1,001, and 10,001
jobs. The round numbers were the target scales, not the counts. v2's own
methodology text stated the mapping ("$N=3333$ ($10{,}001$ jobs)"), but the
table labels and the prose used the round numbers. v3 labels every table and
every prose mention with the exact counts.
Source: `bench/snakemake-vs-oxymake/generate.py` (the $3N{+}2$ job-count
formula, present at commit `7b55bfb`) and `bench/snakemake-vs-oxymake/RESULTS.md`.

**End-to-end table: ratio cells inverted against their own header.**
*v2 wording:* the cold end-to-end table's ratio column was headed "OxyMake /
Snakemake" but printed "0.80× (slower) / 0.44× (slower) / 0.70× (slower)".

Those figures are the reciprocals (Snakemake / OxyMake). From the table's own
wall-clock cells, 1.37 s / 1.10 s = 1.25, 9.74 s / 4.31 s = 2.26, and
2.4 min / 1.6 min = 1.44. The v2 caption already gave the correct range
("OxyMake runs 1.25–2.3× slower"), so the table contradicted its own caption.
v3 prints 1.25× / 2.26× / 1.44× under the same header.
Derivation: arithmetic on the unchanged wall-clock cells of the v2 table.

**Crate table: per-crate rows were stale and did not sum to the totals.**
*v2 wording:* the crate table printed per-crate line and test figures next to
auto-generated workspace totals; the prose read "`ox-core` contains 29% of the
codebase and 39% of unit tests."

The listed rows sum to 40,238 lines and 889 unit tests, but the Total row read
58,966 lines and 1,756 tests (macros regenerated at build time). The per-crate
rows were a stale snapshot: re-measured at commit `7b55bfb`, `ox-core` was
about 15,600 code lines (printed: 9,623) and `ox-cache` about 2,200 (printed:
554). The derived shares followed from neither the printed rows nor the printed
totals. v3 recounts every row from the tree, states in the caption that the
columns sum to the Total row, and corrects the shares to 24% of code and 29% of
tests.
Derivation: per-crate line counts recomputed on the `7b55bfb` tree; column sums
from the v2 table itself.

**FAIR self-assessment: "native compliance on 9 of 11" withdrawn.**
*v2 wording:* "OxyMake achieves native compliance on 9 of 11 assessed
indicators."

Re-scored against the indicator definitions of the cited FAIR papers, most of
the cells marked "Native" do not hold: a lockfile hash is not a registry-backed
persistent identifier (F1); there is no standard metadata vocabulary (F2);
artefact accessibility is not guaranteed (A1); the workflow language is engine-
specific (I2); only one platform was exercised, so cross-platform is not
assessed (I3); and there is no community packaging standard such as RO-Crate
(R3). The v3 table is explicitly labelled a self-assessment, not an external
audit, and scores 3 native, 6 partial, 1 not assessed, and 1 future.
Source: v3 FAIR compliance assessment, against Goble et al. 2020, Wilkinson
et al. 2025, and Chue Hong et al. 2022.

**Shared cache: scope is a blob transport, not a self-sufficient remote cache.**
*v2 wording:* the paper listed a shared cache among OxyMake's delivered
properties ("cache portability across same-platform machines") and the feature
table marked S3/GCS remote cache as a scaffold.

The shared cache is opt-in (`ox run --cache-remote <dir>`) and, as it ships, is
a directory-backed **blob transport**: it stores content-addressed artefact
bytes only. The local index that maps a computation key to its output paths and
hashes stays local to each checkout and is not synchronised. A second checkout
pointing at the same shared directory therefore re-executes the job unless the
local index is also transferred. Setting `--cache-remote` does correctly force
`hash` validation, because a shared store has no meaningful mtime relationship
with the local workspace. The S3 and GCS backends remain scaffolds, so "shared
cache" means a shared filesystem path, not an object store. v3 states the
opt-in status, the blob-transport scope, and the local-index boundary.
Source: `crates/ox-cache-remote/src/directory.rs` and the two-checkout
integration test in `crates/ox-cli/tests/cli.rs`, which passes only because it
copies the local index alongside the shared blobs.

**Cross-machine key portability: bounds stated.**
*v2 wording:* the cache key was presented as travelling across same-platform
machines ("travels across same-platform machines and shared caches").

The key hashes each input's path alongside its content, so portability requires
that the paths agree. Input paths are now recorded relative to the workflow
root, so two checkouts of the same tree at different absolute locations produce
the same key. The boundary, now stated in the paper: an input lying outside the
workflow root cannot be relativised and is recorded as a normalised absolute
path, so it does not travel; key identity requires the same OS and architecture
and the same layout within the workflow root, not the same location of that
root. v3 states these bounds where the travel claim is made.
Source: `crates/ox-cache/src/key.rs` (`workflow_relative_path`, cache key
format v4) and its tests, including the two-checkout test in
`crates/ox-cli/tests/cli.rs`.

**"Auditable from the `ox.lock` record alone."**
*v2 wording:* content-addressing buys "a rebuild decision auditable from the
`ox.lock` record alone."

`ox.lock` suffices to audit the declared graph and every key derivation, but
the rebuild decision a given checkout actually takes also depends on that
checkout's live reuse state: the local key-to-output index (which is not shipped
with the artefact bytes), the outputs present on disk, and the selected
validation policy. v3 states the split and cross-references it wherever key
identity is claimed; it is consistent with the blob-transport entry above.
Source: v3 introduction ("A content-derived rebuild decision") and the section
on content-addressing.

**Static-linkage claim.**
*v2 wording:* "a single statically linked binary with no runtime dependencies"
(and equivalent phrasings, three occurrences).

No per-target linkage attestation was published, so the unqualified "statically
linked" claim was not substantiated. v3 replaces it with "one self-contained
`ox` executable that bundles no interpreter and requires no OxyMake daemon for
local execution, linking only the host platform's system libraries." A per-
target `file`/`ldd`/`otool` attestation is required before any static-linkage
claim is restored.
Source: v3 implementation section (self-contained-binary wording).

---

## D. Claims contradicted by this repository (post-v3 self-review, 2026-07-25)

Sections A--C were driven by an external review of the paper's claims about
*other* systems. This section records the same class of error found by turning
the check inward: published claims that the OxyMake repository itself
contradicts. Each was re-verified against the code before the text was changed;
in every case the text was corrected, never the code. The first three were
found on 2026-07-25; the fourth was added on 2026-08-10 after a review panel
raised it and it was verified against the code
(`ops/audits/panel-findings-verification-2026-08.md`).

**The MCP tool surface.**
*Superseded wording:* "`ox serve --mcp` … lets an MCP-speaking agent call
`run`, `plan`, `status`, and gate approval as tools and read the NDJSON event
stream." Elsewhere: "an MCP server exposes the same operations."

`crates/ox-mcp/src/tools.rs` registers exactly eight tools --- `ox_status`,
`ox_plan`, `ox_dag`, `ox_logs`, `ox_history`, `ox_lint`, `ox_explain`,
`ox_clean` --- a catalogue pinned by a test in the same file. There is no
`ox_run` tool, no gate-approval tool, and no event-stream tool. The real
surface is read-only inspection plus one destructive `ox_clean`; starting a run
and approving a gate stay on the CLI. This is a *more* conservative posture
than the one the paper announced, so the correction strengthens the claim it
replaces. The same overclaim was corrected in `AGENTS.md`/`CLAUDE.md`, `ox
guide` (`crates/ox-cli/src/commands/guide.rs` and `docs/man/ox-guide.1`), a
stale "Execute a workflow with ox_run" hint in `ox-mcp`, and a comment in
`ox-cli` claiming a nonexistent `ox_subscribe` MCP tool. `README.md` already
listed the eight tools correctly.
Source: `crates/ox-mcp/src/tools.rs`.

**Snakemake translation required no manual intervention.**
*Superseded wording:* "All four translated without manual intervention."

`benchmark/snakemake-compat/RESULTS.md` states the opposite: it carries a "What
needs manual fixes" table, a per-workflow "Manual fixes" column with counts of
5, 4, 3 and 4, and the summary line "All 4 workflows execute to completion
after manual fixes." Two of the four translated with no escalation at all and
still needed four hand edits each. The paper now names the recurring
interventions --- missing `expand = "product"` on aggregation rules,
`{input.name}` returning only the first path under `expand`, `{{` brace
escaping, `/bin/sh` versus `/bin/bash`, and `BTreeMap` rule ordering defeating
`rule all` --- and describes `ox translate` as a migration aid rather than a
drop-in transpiler. The paragraph also previously pointed at
`crates/ox-translate/tests/fixtures/` and described four workflows that are not
there; it now cites `benchmark/snakemake-compat/`, where the four evaluated
workflows, their translated Oxymakefiles, and the result log actually live.
Source: `benchmark/snakemake-compat/RESULTS.md`.

**The `tags` rule field is an array of strings.**
*Superseded wording:* `docs/book/src/reference/format.md` documented `tags` as
"Array of strings", and the bioinformatics cookbook --- the book's flagship
tutorial --- used `tags = ["stage.align", "compute-heavy"]`.

`crates/ox-format/src/parse.rs:255` declares `tags: BTreeMap<String, String>`.
The array form is a hard TOML parse error (`invalid type: sequence, expected a
map`), so the tutorial's workflow could not be loaded at all. The reference and
both cookbooks now use the key/value form, and `docs/book/src/concepts/tags.md`
was rewritten: it had also documented `ox run --tag`, `ox run --exclude-tag`
and `ox dag --group-by tag`, none of which select jobs (there are no such `run`
flags, and `dag`'s `--group-by` is accepted but unused). Tags are a labelling
and observation mechanism, surfaced in `ox plan --json` and in `job_queued`
events filterable with `ox subscribe --where KEY=VALUE`.

Verifying the cookbooks end-to-end exposed four further defects in them, all
now fixed and both cookbooks confirmed to lint and run to completion (22/22 and
40/40 jobs): an ambiguous producer between `call_variants` and `merge_vcf`
(fixed with `wildcard_constraints`), two aggregation rules missing `expand =
"product"`, a `chromosomes` config key that cannot resolve a `{chrom}` wildcard
(only the exact name or its plural resolves), TOML-escaped `\t`/`\n` breaking
an embedded `awk` program, a multi-line inline table (invalid TOML) in the
climate cookbook, and that cookbook's use of `asorti`, a GNU awk extension
absent from the BSD awk on macOS. The stale sample `ox plan` transcripts in both
cookbooks were replaced with real output.
Source: `crates/ox-format/src/parse.rs:255`; `ox lint` and `ox run` on both
cookbook workflows.

**The local-disk requirement on `.oxymake/` is satisfiable by relocating the
state directory.**
*Superseded wording:* "OxyMake requires `.oxymake/` to reside on local disk;
the scheduler runs on the submission node, and compute nodes never touch
SQLite." The book's Slurm chapter drew the corresponding deployment
explicitly: a diagram placing `state.db` on the submission node's local disk
and `project/` — inputs and outputs — on the shared filesystem.

That split has no code path. `.oxymake` is a bare relative `PathBuf` at every
site that opens state, cache, logs or events
(`crates/ox-cli/src/commands/run.rs:1018`, `:1101`, `:1423`, `:1436`;
`clean.rs:58`; `invalidate.rs:77`; `status.rs:84`; `init.rs:56`), and no flag,
config key, or environment variable feeds any of them. `OXYMAKE_CACHE_DIR`,
documented in the book's configuration reference, appears in no `.rs` file at
all — nor do `OXYMAKE_JOBS`, `OXYMAKE_EXECUTOR` and `OXYMAKE_LOG` from the same
table, nor the `.oxymake/config.toml` the chapter described as the store of
project-level defaults. Because the state directory *and* the rule's inputs and
outputs are all resolved relative to the process working directory, the two
cannot be separated: putting the project on a shared filesystem necessarily
puts `state.db` there too.

The requirement itself is not false, and the discharge of `StateDbAtomicCommit`
still stands — but it is a requirement on the operator to run the whole
workspace from local disk, not a facility the engine offers. The paper now says
so, and names the absence of a relocation mechanism as a limitation rather than
implying one exists. The book's Slurm diagram and configuration reference were
corrected to match, and the three unimplemented environment variables were
removed from the reference table.
Source: `crates/ox-cli/src/commands/run.rs:1101`; full evidence and the
reproduction (`OXYMAKE_CACHE_DIR` set, `.oxymake/` still created in the working
directory) in `ops/audits/panel-findings-verification-2026-08.md`.

---

## E. Claims corrected by the adversarial pre-mortem review (2026-08)

Sections A–C came from an external review, section D from turning the check
inward. This section records the outcome of a third pass: a two-seat
adversarial pre-mortem double review of the paper (45 findings), a
verification pass that established each finding against a primary source, a
second referee round (15 further findings), and a regeneration of the
benchmark of record on an idle host with the binary pinned to the tree. Only
findings whose verdict was CONFIRMED or PARTIAL were acted on, and in every
case the text was corrected against the code or the cited source, never the
other way round.

Entries appear here only when the false claim was published in v2. Several
corrections landed on text that was written after v2 and so need no entry.

### E.1 Characterisations of other systems

**Snakemake 7 replaced timestamp comparison with a provenance record.**
*v2 wording (introduction):* "since the 7.x line it records per-output
provenance (code, parameters, input set, software environment) instead of
comparing live input-versus-output timestamps." *v2 wording (related work):*
"Its change detection has evolved from live mtime comparison to recorded
per-output provenance in the 7.x line."

The recorded checksum is additive, not a replacement. In Snakemake 7.32.4 an
input is collected into `reason.updated_input` only when it exists, is newer
than the oldest output, **and** its recorded SHA-256 no longer matches
(`snakemake/dag.py:1141-1152`); the mtime test is the first conjunct. A
checksum is recorded only for inputs smaller than 100,000 bytes
(`snakemake/io.py:582-588`), and for a larger input the mtime comparison
decides alone (`dag.py:1076-1083`). The benchmark's perturbed file is 638
bytes, which is why the checksum decides there. v3 states the conjunction and
the threshold, and no longer says that live timestamps are ignored.
Source: Snakemake 7.32.4 as installed, files and lines above.

**`snakemake --dryrun` exercises no cache validation.**
*v2 wording:* "DAG resolution (`ox plan` versus `snakemake --dryrun`)
exercises no cache validation at all."

`DAG.init()` calls `update_needrun(create_inventory=True)` unconditionally
(`snakemake/dag.py:248`), and `init()` is on the dry-run path
(`workflow.py:895`). The dry run therefore stats inputs and outputs, builds an
mtime inventory, reads `.snakemake/metadata` and checksums eligible inputs.
`ox plan` does none of this: it never constructs a `CacheStore`. The
comparison is not symmetric, so the ratio bounds rather than isolates the
resolution difference. v3 says so.
Source: `snakemake/dag.py:248`, `snakemake/workflow.py:895`;
`crates/ox-cli/src/commands/plan.rs`.

**Nix: chapter 6 introduced content-addressed storage.**
*v2 wording:* "Dolstra's Nix thesis [ch. 6] introduced content-addressed
storage for software deployment, where store paths encode cryptographic hashes
of all build inputs."

Chapter 5 is the extensional model — the input-addressed store, which is what
the quoted sentence describes. Chapter 6 is the intensional model, which
extends content-addressing to derivation outputs. v3 cites ch. 5 for the
input-addressed store and names ch. 6 for what it actually contains, and adds
that OxyMake's key is input-addressed while the artefact store beneath it is
content-addressed.
Source: [Eelco Dolstra, *The Purely Functional Software Deployment Model*,
2006](https://edolstra.github.io/pubs/phd-thesis.pdf), chapters 1, 5 and 6.

**Bazel placed in OxyMake's scheduler column.**
*v2 wording:* "placing OxyMake in the same scheduler column as Make and Bazel
rather than the suspending column occupied by Shake."

Mokhov, Mitchell and Peyton Jones classify Bazel's scheduler as *restarting*,
not topological, because it supports action-time input discovery; the
topological × verifying-traces cell OxyMake claims is the one they assign to
Ninja. v3 places OxyMake with Make, Ninja and Buck, and states that Bazel is
in the restarting column. The affinity with Bazel that does hold — an analysis
phase that builds an action graph before execution, and content-based caching
— is stated as such.
Source: Mokhov, Mitchell, Peyton Jones, *Build Systems à la Carte: Theory and
Practice*, JFP 30 (2020), Tables 1 and 2.

**Snakemake's `report` as a same-plan-hash witness.**
*v2 wording:* the reproducibility-ladder table listed "OxyMake `ox.lock`,
Snakemake `report`" as tool examples for the L2 witness "same plan hash ⇒ same
DAG".

A Snakemake report is a self-contained HTML/ZIP artefact carrying runtime
statistics, provenance information and workflow topology. The documentation
defines no plan hash and makes no DAG-identity claim from hash equality. v3
lists `ox.lock` alone in that cell.
Source: [Snakemake — reports](https://snakemake.readthedocs.io/en/stable/snakefiles/reporting.html).

**"Same input hash ⇒ same binary" as the substrate witness.**
*v2 wording:* the same table's L1 row read "Same input hash ⇒ same binary",
with Guix, Nix and Docker digests as examples.

An input-addressed store fixes the output *store path*; bit-identical output
bytes additionally require the build itself to be deterministic. The Nix
project states that deterministic references and sandboxing "alone [are] not
sufficient: builds may still leak timestamps or have other nondeterminisms."
v3 reads "same path" and carries the distinction in the caption. This is the
same conflation section B corrects for Nix and Guix in prose; it survived in
the table.
Source: [reproducible.nixos.org](https://reproducible.nixos.org/).

### E.2 OxyMake's own claims

**"No hardcoded thresholds."**
*v2 wording:* "The engine never interprets intent (no heuristics, no hardcoded
thresholds)."

The engine carries fixed operational constants. `ox clean` treats a session as
stale at a heartbeat age of five minutes
(`crates/ox-cli/src/commands/clean.rs:126`, `:275-277`, calling
`find_stale_sessions(300)`). The defensible claim is that no threshold depends
on the workload; v3 makes that claim and names the five-minute constant.
Source: `crates/ox-cli/src/commands/clean.rs:126`;
`crates/ox-state/src/session.rs:142`.

**"Sub-100 ms no-op runs with no dependency on `.oxymake/`."**
*v2 wording (`mtime` policy):* "delivers sub-100 ms no-op runs with no
dependency on `.oxymake/`."

Two errors. The independence holds for the *lookup* only: recording a
completed job writes the cache entry to the state database in every mode
(`crates/ox-cli/src/commands/run.rs:533-560`). And no-op run time scales with
workflow size — the smallest measured `mtime` warm run is 313 ms at 101 jobs,
and 500 ms at 10,001 jobs. v3 states both.
Source: `crates/ox-cli/src/commands/run.rs:500-508`, `:533-560`;
`bench/snakemake-vs-oxymake/data/measurements.tsv`.

**"The resolution phase is sub-100 ms."**
*v2 wording:* "The resolution phase is sub-100 ms, so we time it with
`hyperfine`."

The same subsection's own table reported 69 ms at the largest scale in v2 and
118.3 ms in the re-measured run. v3 says "millisecond-scale (5.9–118.3 ms
across the measured scales)".
Source: `bench/snakemake-vs-oxymake/RESULTS.md`.

**The three-argument cache-key guarantee.**
*v2 wording:* "same inputs + same rule + same parameters ⇒ same cache key."

The key equation has seven arguments: key-format version, rule source, input
content hashes, parameters, environment specification, shell, and platform.
Three of them are not optional refinements — an environment or shell change
alone changes the key. v3 states the whole declared specification and its
scope conditions.
Source: the cache-key equation in v3; `crates/ox-cache/src/key.rs`.

**The `output missing` cascade as a derived correctness property.**
*v2 wording:* "The 'output missing' row addresses a subtle correctness
property that timestamp-based systems miss."

When a recorded output is missing, OxyMake re-executes its producer and,
unconditionally, that producer's transitive dependents; it does not re-check
whether a regenerated input's hash still matches
(`crates/ox-cli/src/commands/run.rs:1194-1212`). That is a topologically
propagated dirty bit — a conservative policy, not a property derived from
content. v3 states it as a policy and names the behaviour.
Source: `crates/ox-cli/src/commands/run.rs:1194-1212`.

**`InMemory` outputs are promoted to `File`, so "the workflow runs correctly
everywhere".**
*v2 wording:* "the scheduler automatically promotes `InMemory` outputs to
`File` — the workflow runs correctly everywhere, with degraded performance on
backends that lack native object stores."

Promotion is bounded by the configured serialization format: a value the
format cannot represent does not survive it, and `materialize = "never"` means
memory-only and not cached. Both limits are stated elsewhere in the paper. v3
states the condition where the promotion is claimed.
Source: v3 execution-spectrum and limitations sections.

**"Adding rules never invalidates existing results."**
*v2 wording:* "content-addressable incrementality (adding rules never
invalidates existing results)."

A new rule does not change the key of a job whose own declared specification
is unchanged, but it can change which rule produces a path, or make resolution
ambiguous: the resolver collects every matching producer and returns
`AmbiguousProducer` when priorities do not separate them
(`crates/ox-core/src/resolver.rs:227-260`). A workflow that resolved before can
fail to resolve after. v3 states the narrower property and the residual hazard.
Source: `crates/ox-core/src/resolver.rs:227-260`.

**`Send + Sync` bounds enforce thread safety.**
*v2 wording:* "`Send + Sync` bounds enforce thread safety for the concurrent
scheduler"; elsewhere, "`Send + Sync` bounds make the concurrent scheduler
thread-safe by construction."

These bounds rule out data races on shared state at compile time. They do not
cover the protocol-level hazards the paper's own TLA+ section exists for:
deadlock, lost updates, and torn multi-step protocols across sessions. v3
states what the bounds rule out and points at the specifications for the rest.
Source: v3 implementation section and the section on named invariants.

**`ox-core` never performs file I/O.**
*v2 wording:* "Each crate has a strict boundary: `ox-core` never performs file
I/O or network calls … These boundaries are enforced by Cargo dependency
rules."

`crates/ox-core/src/disk_writer.rs` is production code and performs
`create_dir_all`, `File::create`, `rename` and a parent-directory fsync;
`crates/ox-core/src/scheduler.rs` reads files on the scheduler path; and
`ox-core`'s manifest enables tokio's `fs` feature. Nor does Cargo enforce the
boundaries: it prevents a crate from calling a *workspace* crate it does not
depend on, but `std::fs`, `std::net` and `std::process` are linked into every
crate without appearing in any manifest, and "never decides whether a job
should re-run" is a semantic property no dependency graph constrains. The
no-network half of the claim holds. v3 states the intended boundaries, what
Cargo actually enforces, and the one boundary not held today.
Source: `crates/ox-core/src/disk_writer.rs`, `crates/ox-core/src/scheduler.rs`,
`crates/ox-core/Cargo.toml`.

**Audit metrics are "impossible to backfill".**
*v2 wording:* "It is impossible to backfill if not collected from run 1."

Metrics not recorded at the time of a run cannot be reconstructed afterwards,
which is the true and weaker statement. The table is append-only by convention
only: there is no hash chain, no signature and no tamper-evidence in
`crates/ox-state/src`. v3 says both.
Source: `crates/ox-state/src/db.rs:118`, `:1220`; `crates/ox-state/src/lib.rs:26`.

**"Translation is bidirectional."**
*v2 wording:* "Translation is **bidirectional** and supports multiple source
formats"; "The bidirectional translator bridges both ecosystems: bioinformatics
teams can import their existing WDL workflows into OxyMake … and export back to
WDL."

`ox translate` and `ox export` exist for both Snakemake and WDL, but only the
Snakemake import direction is evaluated anywhere in the paper: the repository
contains no `.wdl` file, and `benchmark/snakemake-compat/` holds four Snakemake
workflows and no export-direction experiment. v3 keeps the commands and states
that the export direction and WDL translation are described but not exercised.
Source: `benchmark/snakemake-compat/`; absence of any `.wdl` file in the
repository.

**The resolution bound `O(R × P)`.**
*v2 wording:* "in time O(R × P) in the rule count R and the output-pattern
count P"; "the asymptotic complexity remains O(R × P)"; "a hash-based prefix
index that would reduce lookup to amortized O(R + P)."

`ProducerIndex::build` makes one pass over rules × output patterns, i.e. O(P)
construction, and `find_producer` is a linear scan over the same P entries per
target. Since `entries` already ranges over all rules, R does not multiply P;
resolving T targets costs O(T × P). v3 states a one-time O(P) compilation plus
O(T × P) lookup, and gives the future-work bound as expected O(P) construction
and O(T) lookup.
Source: `crates/ox-core/src/resolver.rs:207-221`, `:227-244`.

**The FAIR indicator codes.**
*v2 wording:* the FAIR table's column was headed "Indicator" and its rows were
labelled F1–F3, A1–A2, I1–I3, R1–R3 against the cited FAIR-workflow
literature.

Wilkinson et al. (*Sci Data* 12:328, 2025) define the indicator set F1, F1.1,
F1.2, F2, F3, F4 / A1, A1.1, A1.2, A2 / I1, I2, I3, I4 / R1, R1.1, R1.2, R1.3,
R2, R3. Seven of the paper's eleven rows carried a code whose definition does
not match the row: F3 there is "metadata explicitly include the workflow
identifier", A2 is "metadata accessible even when the workflow is no longer
available", R1 is "a plurality of accurate and relevant attributes", R2 is
"qualified references to other workflows", and I1–I3 are likewise mismatched;
F4 and I4 appear nowhere in the paper. Renumbering would have meant reassessing
every row against a finer set, so v3 drops the codes, heads the column
"Aspect", and states in the caption that the labels are this paper's own
condensation and not the numbered indicators of Wilkinson et al. Section C's
withdrawal of "native compliance on 9 of 11" stands; two further rows —
provenance and reproducibility — are downgraded from Native to Partial on the
paper's own grounds.
Source: [Wilkinson et al., *Sci Data* 12:328 (2025)](https://www.nature.com/articles/s41597-025-04451-9), Table 1.

**The scaling ladder "without modification".**
*v2 wording:* "the same declarative workflow runs on a local machine (`-j N`),
SLURM cluster (`--executor slurm`), or Kubernetes (`--executor k8s`) without
modification."

The workflow *file* is accepted unchanged, but neither distributed path is
exercised in the evaluation, and both require the workspace — state database
included — on storage visible to the submission and compute nodes, which the
local-disk requirement of section D does not support. The Kubernetes executor
is designed and not implemented. v3 says all three things.
Source: v3 executor-backends and evaluation sections; section D above.

**The BLAKE3 throughput figure.**
*v2 wording:* "over 6.9 GiB/s single-threaded on modern x86-64 hardware."

The figure is a Cascade Lake-SP measurement on a 16 KiB input, and the
evaluated host is arm64, where BLAKE3 takes a different SIMD path. The hash
phase was never profiled in this system. v3 names the platform of the published
figure, states that the evaluated architecture is a different one, and keeps
the concession that the expectation rests on published figures for another
architecture rather than on a measurement of this system.
Source: the BLAKE3 entry in `docs/paper/references.bib`.

**Startup time "median of 3 runs".**
*v2 wording:* "Startup time (median of 3 runs)."

`docs/paper/experiment-results.md` records five runs per command and reports
the median of five. The same file dates the startup and binary-footprint
micro-benchmarks to 2026-04-01 under Darwin 24.6.0, whereas v2's evaluation
preamble stated one Darwin version for all experiments. v3 corrects the run
count and separates the two measurement dates and OS versions. The v2 gloss
"regardless of workflow size" also generalised from three measured commands and
is narrowed to them.
Source: `docs/paper/experiment-results.md:3-5`, `:8`, `:35-46`.

### E.3 The measured results

**Every head-to-head number in v2 is superseded, not refined.**
*v2 wording:* the DAG-resolution, end-to-end, warm-rerun and memory figures of
the evaluation section.

The benchmark of record was regenerated on an otherwise idle host with the
`ox` binary built from the tree under measurement (`cargo build --release`,
invoked as `target/release/ox` rather than resolved from `$PATH`) at commit
`03864f8`, against Snakemake 7.32.4, both engines at 16-way parallelism. These
are different samples from a different run, not a re-rounding of v2's, and they
replace v2's throughout:

| Quantity (10,001 jobs unless noted) | v2 | v3 |
|---|---|---|
| `ox plan` / `snakemake --dryrun` @ 101 | 4 ms / 418 ms | 5.9 ms / 595.1 ms |
| `ox plan` / `snakemake --dryrun` @ 1,001 | 10 ms / 512 ms | 13.1 ms / 792.4 ms |
| `ox plan` / `snakemake --dryrun` @ 10,001 | 69 ms / 2,310 ms | 118.3 ms / 2,721.7 ms |
| Resolution speedups (101 / 1,001 / 10,001) | 101.9× / 50.7× / 33.3× | 100.86× / 60.49× / 23.01× |
| Cold end-to-end, Snakemake / OxyMake | 1.6 min / 2.4 min | 92.666 s / 196.934 s |
| Cold end-to-end ratio range across scales | 1.25–2.3× slower | 1.47–2.57× slower |
| Warm no-op, Snakemake | 2.81 s | 3.478 s |
| Warm no-op, OxyMake `mtime` | 372 ms (7.54×) | 500.1 ms (6.95×) |
| Warm no-op, OxyMake `hash` | 698 ms (4.02×) | 1.821 s (1.91×) |
| Warm no-op, OxyMake `mtime+hash` | not measured | 1.378 s (2.52×) |
| Peak RSS, cold, OxyMake / Snakemake | 90.7 / 184.7 MiB | 89.9 / 184.5 MiB |

v2 also carried the warm figures without naming the validation policy that
produced them, and described the shipped `mtime+hash` default as following the
same metadata fast path as `mtime` on an undisturbed tree. It does not: it
re-hashes any file whose metadata moved, and the measured gap between the two
is a factor of 2.8. Every warm figure in v3 names its policy, and the
`mtime+hash` default is now measured rather than inferred.
Source: `bench/snakemake-vs-oxymake/RESULTS.md` and
`bench/snakemake-vs-oxymake/data/measurements.tsv`, regenerated at commit
`03864f8`.

**"Both systems re-run exactly the 3 affected jobs."**
*v2 wording:* "**Minimal-rebuild correctness**: rewriting the content of one
Layer-1 input, both systems re-run exactly the 3 affected jobs at every scale —
OxyMake's content-addressed decision is as tight as Snakemake's provenance
decision."

The record contradicts this. The expected scope is 3 jobs; Snakemake re-runs 3
at every scale and OxyMake re-runs 4. v3 reports both counts, renames the
finding from "minimal-rebuild correctness" to "rebuild scope", and states that
the decision is tight but not measured to be minimal on this workload, with the
extra job named as an undiagnosed overshoot.
Source: `bench/snakemake-vs-oxymake/RESULTS.md`, detailed rebuild-scope table.

**An unprofiled causal explanation of the narrowing resolution ratio.**
*v2 wording:* the ratio narrows with scale "because Snakemake amortises its
fixed Python-interpreter startup over more jobs while OxyMake's resolution
grows linearly … so most of the 101-job row is fixed interpreter startup and
import rather than resolution work."

The harness records aggregate wall time per (size, system, phase, cache) and
nothing finer; no phase-level profile exists. The aggregate timings support the
statement that the small rows are dominated by cost that does not scale with
the graph, but not the allocation of that cost among interpreter startup,
imports, metadata work and resolution. v3 states the observation and withholds
the mechanism.
Source: `bench/snakemake-vs-oxymake/run.sh` and its recorded columns.

**"Every mtime perturbed."**
*v2 wording:* "the `git checkout` scenario — every mtime perturbed, no content
changed"; "mtime churn: every timestamp moves, no byte changes."

The harness runs a single `touch` on one shared input, `bench_lib.py`, and its
own comment says so. One file's mtime moves. The re-run counts are unaffected —
the file is a declared input of every `process` job — but the scenario
description overstated the perturbation. v3 says "one shared input's mtime".
Source: `bench/snakemake-vs-oxymake/run.sh:292-300`, `:305`, `:312`, `:320`.

---

*Maintained by Noogram. Corrections and counterexamples are welcome as issues
on `noogram/oxymake`.*
