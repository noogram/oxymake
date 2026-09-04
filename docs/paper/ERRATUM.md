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
mtime inventory and reads `.snakemake/metadata`. `ox plan` does none of this:
it never constructs a `CacheStore`. The comparison is not symmetric, so the
ratio bounds rather than isolates the resolution difference. v3 says so.
*Superseded in part by F.9:* no input checksum is in fact computed on either
measured row, and v3 no longer says one is.
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

### E.4 Corrections from the round-3 referee pass (2026-08-27)

A third, two-seat referee round was run on the v3 draft before submission.
Nine of its findings were confirmed against the paper, the repository, or a
primary source. The seven below correct wording that was published in v2; the
remainder landed on text written after v2.

**"The local, SLURM, and Ray executors are fully implemented."**
*v2 wording (limitations, "Distributed executors"):* "The local, SLURM, and Ray
executors are fully implemented."

Both distributed backends require the workspace — inputs, outputs and the
state database — on storage visible to the submission and the compute nodes,
while `.oxymake/` is created in the process working directory and cannot be
relocated by any flag, config key or environment variable. The two
requirements cannot be satisfied together, so neither backend has a supported
deployment configuration in v0.1.0. v3 says that the local executor is the one
exercised here and that the Slurm and Ray executors are implemented but have
no supported deployment configuration, and it weakens the contribution and
introduction claims from "runs the same workflow from a laptop to Slurm or
Ray" to "accepts the same workflow file unmodified".
Source: the paper's own §3.4 and §7.3 local-disk requirement.

**Binary size, startup time and release build time were measured on a
different tree from the one evaluated.**
*v2 wording:* "a 14.9\,MB statically-linked executable"; "`ox --help` 6\,ms";
"Full release build time 71\,s".

Those three figures come from `docs/paper/experiment-results.md` dated
2026-04-01, which records the tree it measured as 52,514 Rust SLOC, 23 crates
and 1,330 tests. The paper describes and evaluates a tree of 64,596 SLOC, 25
crates and 1,928 tests. All three were re-measured at commit `1a4f724` on
2026-08-27, on the same host: the binary is 17,421,504 bytes (17.4 MB),
`ox --help` has a `hyperfine -N` median of 2.21 ms over 200 runs (2.37 ms for
`ox lint` on 10 rules, 5.85 ms on 1,000 rules), and a `cargo build --release`
over the workspace after `cargo clean --release` takes 25.7 s. The startup
figures are not directly comparable with the 2026-04-01 ones, which were taken
through a Python `subprocess` harness that includes its own spawn cost; v3
says so where it quotes them.
Source: `docs/paper/experiment-results.md`, section "Re-measurement at commit
`1a4f724`".

**Pegasus "requires significant infrastructure".**
*v2 wording (related work):* "Pegasus targets large-scale distributed
workflows but requires significant infrastructure."

Pegasus documents a localhost deployment scenario: "The simplest execution
environment does not involve HTCondor. Pegasus is capable of planning small
workflows for local execution using a shell planner." Significant
infrastructure is a property of its distributed modes, not a requirement of
the system. v3 states the range and locates the contrast in the distributed
modes' execution and data-staging infrastructure.
Source: Pegasus user guide, "Deployment Scenarios — Localhost"
(<https://pegasus.isi.edu/documentation/user-guide/deployment-scenarios.html>).

**"TOML parsing completes in microseconds."**
*v2 wording (performance ceiling):* "TOML parsing completes in microseconds
(versus Python import time of hundreds of milliseconds)."

No measurement in the record isolates parse time. `experiment-results.md`
measures whole-command elapsed time and cannot establish a microsecond figure
for one phase inside it. v3 keeps the design distinction — the workflow format
is inert, parsed as data rather than executed — and drops the timing.
Source: `docs/paper/experiment-results.md`, experiment 3.

**The year-2030 mtime and the explicit `--rerun-triggers mtime`.**
*v2 wording (mtime-churn benchmark):* "it re-ran zero jobs even with the
input's mtime forced to the year 2030, under an explicit
`--rerun-triggers mtime`."

The harness runs a plain `touch` on `bench_lib.py` and invokes Snakemake under
its default rerun-triggers; neither the year 2030 nor an explicit
`--rerun-triggers mtime` appears in it or in the recorded results. What the
record supports is a `touch` and a far-future timestamp, both at zero re-runs,
under the default triggers. v3 says that. The companion figure in the same
paragraph — the perturbed file at 638 bytes — is correct: `bench_lib.py` is
638 bytes.
Source: `bench/snakemake-vs-oxymake/run.sh:292-330`,
`bench/snakemake-vs-oxymake/RESULTS.md` (mtime-churn findings note),
`bench/snakemake-vs-oxymake/bench_lib.py`.

**"Heavy plugins (Kubernetes, S3) are opt-in features."**
*v2 wording (plugin architecture):* "Heavy plugins (Kubernetes, S3) are opt-in
features. This ensures the default binary remains small and fast to build."

Neither backend is implemented, as the same subsection and the limitations
section state, so there is nothing to opt into. v3 describes the feature-flag
mechanism as where those plugins will be gated when implemented, and credits
it today with excluding optional codecs and reporters.
Source: the paper's own §4.5 "(planned)" markers, §6.8 and §7.3.

**The build-system classification stated without its exception.**
*v2 wording (build system theory):* "In this taxonomy, OxyMake combines a
topological scheduler with a verifying-traces rebuilder augmented with
content-addressing."

The deleted-output cascade propagates staleness unconditionally: it does not
re-check whether a regenerated input's hash still matches, so on that path the
engine behaves as a topologically propagated dirty bit rather than as a
verifying-traces rebuilder. v3 already disclosed this where the cascade is
described; the disclosure is now also attached to the classification itself.
Source: the paper's §4.6.

### E.5 Corrections from the round-4 referee pass (2026-09-02)

**"TLC found seven bugs."**
*v2 wording (introduction):* "where TLC found seven bugs across ten systems
that testing, code review, and fault injection had missed."
*v2 wording (named invariants):* "which reports seven bugs found by TLC in ten
AWS systems."

The number seven in Newcombe et al. counts teams, not bugs: "Amazon now has
seven teams using TLA+" (CACM 58(4):68). The article's table "Applying TLA+ to
some of Amazon's more complex systems" reports two, one, three, three and one
bugs across its six entries, which is ten, plus further bugs found in proposed
fixes and optimizations. The paper's accompanying list of
techniques the bugs escaped is unchanged and correct: the article's own list is
"deep design reviews, code reviews, static code analysis, stress testing, and
fault-injection testing" (p. 66).
Source: Newcombe et al., CACM 58(4):66-73, table on p. 69 and text on p. 68.
*Superseded by F.4:* the interim wording "ten bugs in ten systems" was itself
wrong, and v3 now describes what the table supports.

**BLAKE3 "over 6.9 GiB/s".**
*v2 wording (Rust as implementation language):* "BLAKE3 hashes at over
6.9~GiB/s single-threaded on modern x86-64 hardware."

The BLAKE3 project's own benchmark data for that configuration is 6,866 MiB/s,
which is approximately 6.70 GiB/s, not over 6.9 GiB/s -- a unit conversion
error that overstates the figure by about 3%. `bar_chart.py`, the script that
generates the chart in the BLAKE3 README, hardcodes `("BLAKE3", 6866)` with the
axis labelled "Speed (MiB/s)" and the title "Performance on AWS c5.metal, 16
KiB input, 1 thread". v3 quotes 6,866 MiB/s with the GiB/s conversion, and the
`blake3spec` bib note is corrected to match.
Source: `BLAKE3-team/BLAKE3-specs`, `benchmarks/bar_chart.py`, lines 11, 28,
71-72.

**The ProducerIndex speedup stated as measured.**
*v2 wording (scalability):* "the constant drops substantially, since
per-target regex compilation was the dominant cost at scale."

No before/after measurement of the pre-index implementation exists in the
record, and no phase-level profile attributes cost to regex construction -- the
paper states elsewhere that no such profile was run. v3 states the improvement
as expected rather than measured, and marks the regex-construction attribution
as a design argument.
Source: `docs/paper/experiment-results.md`; the paper's own §5.1, §6.1
and §6.2 profiling disclaimers.


---

## F. Claims corrected by the round-5 review (2026-09)

A fourth pass added two independent outside seats: a citation audit that
fetched the primary source behind every `\cite` in the paper, and a general
referee read that recomputed every number from the evidence bundle. As in
section E, only CONFIRMED findings were acted on, each was re-verified against
the primary source before the edit, and the text was corrected against the
source, never the other way round. Entries appear here when the false claim was
published in v2; corrections landing on post-v2 text are listed at the end
without a v2 quotation.

### F.1 A quotation attributed to Goble et al. that they do not contain

*v2 wording (§2, FAIR workflows):* "the observation that `a workflow that
cannot be readily reused is like a scientific paper that cannot be read' is
load-bearing: it is the principle that motivates OxyMake's three-graph
architecture."

That sentence does not appear in Goble et al. A full-text search of the article
for "cannot be read", "scientific paper", "readily" and "paper that" returns
nothing; the phrase "scientific paper" does not occur in it. v3 removes the
quotation and quotes instead what the article does say: "Workflows are research
products in their own right, encapsulating methodological know-how that is to
be found and published, accessed and cited, exchanged and combined with others,
and reused as well as adapted."
Source: Goble et al., *FAIR Computational Workflows*, Data Intelligence
2(1-2):108-121, doi:10.1162/dint_a_00033, §3 ("FAIR criteria for workflows as
digital objects").

### F.2 "The Goble three-layer model"

*v2 wording:* "The Goble three-layer model. ... They identify three
layers---*abstract workflow* ... *concrete workflow* ... and *execution
trace*... Each layer requires independent FAIR compliance." The same
attribution carried the three-graph contribution and the per-principle "Goble
layer(s) A/C/T" annotations.

Goble et al. §3 ("Forms") lists **four** forms, in different words: "A workflow
can be a CWL specification with test or exemplar data; an implementation of
that design in a WfMS; an instantiation of that implementation ready to be run
with input data and parameters set and computational services spun up; a run
result with intermediate and final data products and provenance logs", and it
says each form "may have different FAIR criteria", not that each requires
independent FAIR compliance. The labels *abstract* and *concrete workflow* are
not theirs. The three-layer split is the paper's own reading, and the
architecture does not depend on the attribution. v3 quotes the four forms,
states the three-layer collapse as ours ("we adopt a three-layer reading"), and
renames the derived headings accordingly.
Source: Goble et al. §3, "Forms" paragraph.

### F.3 "FAIR workflow indicators defined by Goble et al."

*v2 wording (§7, FAIR compliance):* "We assess OxyMake against the FAIR
workflow indicators defined by Goble et al. and operationalized by Wilkinson et
al."

Goble et al. define no indicators. Their conclusions state the opposite:
"FAIR principles for data, and for software, are generally applicable, but need
to be extended in order to address the processual nature of workflows.
Consequently new FAIR indicators will also need to be developed." v3 assesses
against the FAIR principles for workflows *argued for* by Goble et al. and made
concrete as numbered entries by Wilkinson et al., and notes that Wilkinson et
al. call those entries principles, not indicators.
Source: Goble et al. §4 (Conclusions); Wilkinson et al., Sci Data 12:328,
Table 1.

### F.4 Newcombe et al.: what the table reports

*v2 wording:* "which reports seven bugs found by TLC in ten AWS systems"; the
E-section correction replaced this with "ten bugs found by TLC in ten AWS
systems", which is also wrong.

Ten is the number of systems TLA+ was applied to ("Amazon engineers have used
TLA+ on 10 large complex real-world systems", p. 67), not a bug count. The
table on p. 69 covers six components of four systems -- S3 (two rows),
DynamoDB, EBS, and an internal distributed lock manager (two rows) -- and its
entries read "Found two bugs, then others in proposed optimizations", "Found
one bug, then another in the first proposed fix", "Found three bugs requiring
traces of up to 35 steps", "Found three bugs", "Improved confidence though
failed to find a liveness bug", and "Found one bug and verified an aggressive
optimization"; no total is stated. The escaped-technique list is also stated of
particular bugs ("The bug had passed unnoticed through extensive design
reviews, code reviews, and testing"), not of every tabulated one. v3 states
what the table supports and attributes the escaped-review claim to the cases
the article describes.
Source: Newcombe et al., CACM 58(4):66-73, p. 67 and the table on p. 69.

### F.5 petgraph's topological sort is not Kahn's algorithm

*v2 wording:* "The `petgraph` library provides $O(|V|+|E|)$ topological sort
via Kahn's algorithm." The `petgraph` bib note said the same.

`petgraph::algo::toposort` -- the function the engine calls
(`crates/ox-core/src/dag.rs:61`, `job_graph.rs:62`) -- is a depth-first
finish-order algorithm: the source carries the comment "based on kosaraju scc",
builds a `finish_stack`, reverses it, and detects cycles with a reverse-graph
pass. It keeps no in-degree counts and no ready queue, which is what defines
Kahn's algorithm. The documented O(|V|+|E|) bound is correct; the mechanism is
not. (petgraph does ship a Kahn-style walker, `visit::Topo`, but the engine does
not use it and it documents no complexity bound.) v3 says depth-first and names
the API.
Source: petgraph 0.8.3, `src/algo/mod.rs`, `pub fn toposort`.

### F.6 Wrong section locator for Mokhov et al. (2020)

*v2 wording:* `\cite[\S3]{mokhovBuildSystemsCarte2020}`, in both the
build-theory section and the related-work comparison.

In the JFP version, §3 is "Build Systems, Abstractly"; the scheduler/rebuilder
taxonomy is §4 ("Schedulers") and §5 ("Rebuilders"), with Table 2 at the end of
§5. The 2018 locator "§3-4" is correct for the ICFP version. v3 cites §4-5 for
the 2020 version at both sites.
Source: Mokhov, Mitchell & Peyton Jones, *Build Systems à la Carte: Theory and
Practice*, JFP 30:e11, section headings and Table 2.

### F.7 PiGx cited as a guix-cwl workflow

*v2 wording:* "The guix-cwl reference workflows [Prins 2018; Wurmus et al.
2018] illustrate one such substrate-composition pattern---a CWL workflow run
under a Guix-managed environment"; and "Compared to the guix-cwl stack [same
two references]".

PiGx is built on Snakemake, not CWL: the full text of Wurmus et al. contains no
occurrence of "CWL" or "Common Workflow Language" and states "we used
SnakeMake, which provides target-driven execution". Only the Prins repository
supports the guix-cwl description. v3 cites Prins alone for guix-cwl and
describes PiGx separately as the same substrate pattern with Snakemake as the
orchestration layer.
Source: Wurmus et al., GigaScience 7(12):giy123 (PMC6275446), full text.

### F.8 Buck2's "fully content-addressed execution model"

*v2 wording:* "Meta's Buck2 pushes this design further with a fully
content-addressed execution model built on the Starlark configuration
language."

The cited source describes Starlark rules, a single incremental dependency
graph, and remote execution over recursive digests; it makes no "fully
content-addressed execution model" claim. v3 says "a content-hashed,
remote-execution-oriented model".
Source: the cited Buck2 announcement.

### F.9 Corrections to text written after v2

These landed on v3-only wording and carry no v2 quotation.

- **The dry run does not checksum inputs.** The correction recorded in
  section E wrote that
  `snakemake --dryrun` "evaluates the rerun triggers for every job
  unconditionally, so the dry run stats inputs and outputs, reads
  `.snakemake/metadata` and checksums eligible inputs." In `dag.py`
  (v7.32.4, L1141-1153) the checksum comparison sits behind
  `f.exists and f.is_newer(output_mintime_)`, and the whole block behind
  `if not reason`. On the cold row a missing output already sets the reason,
  so the block is skipped; on the warm row every input predates its outputs,
  so `is_newer` is false and the `and` short-circuits before
  `is_same_checksum`. Nor is the evaluation unconditional: the
  params/input/code/software-env triggers run only when no earlier reason
  fired. v3 states what happens on each measured row.
- **The measured binary's provenance.** §6 states that the measured build is
  the in-tree `target/release/ox` at commit `03864f8`, while the benchmark of
  record listed `ox: cargo install --path .` under "Binaries" and named no
  commit. The paper is correct -- the run used `OX=$PWD/target/release/ox`
  against a release build of that tree -- and the record's boilerplate was
  never updated by the harness. Since the paper and the record must agree, the
  record's prose is amended: `bench/snakemake-vs-oxymake/RESULTS.md` now states
  the actual invocation and the commit. No number in it was touched.
- **End-to-end times are single runs, not medians of three.** §6 and §7.3 said
  the minutes-scale end-to-end phase was timed "as the median of three runs".
  In `bench/snakemake-vs-oxymake/run.sh`, `measured_run()` executes the command
  once per cell; the harness's `RUNS` variable (default 3) is consumed only by
  `clean_resolve()`, the hyperfine-timed resolution phase. v3 says a single
  timed run per cell and scopes `RUNS` to the resolution phase.
- **The cold-path differential.** §6.2 said OxyMake "performs work Snakemake
  does not: it hashes every rule's source, inputs, parameters, environment and
  platform into a BLAKE3 cache key and writes a content-addressed store and an
  `ox.lock` audit record." Snakemake 7.32.4 also records per-job provenance
  and SHA-256 checksums of eligible inputs at every job completion
  (`persistence.py:284-305`), so hashing is not the differential; and `ox run`
  writes no `ox.lock` (only `ox lock generate` does) and copies no artefact
  bytes into a blob store unless `--cache-remote` is set, which the harness
  does not set. v3 names the shared work, then the OxyMake-only work: folding
  the declaration into a single BLAKE3 key and hashing every output into the
  local cache index.
- **Smaller corrections.** Snakemake's launch paper carries no adoption claim,
  so the superlative "the most widely used workflow system in bioinformatics"
  is replaced by the 2021 article's own "one of the most widely used workflow
  management systems in science"; BioBlend is not named in the cited 2018
  Galaxy article, so the sentence now says "a REST API with language bindings";
  Dolstra's "we know that we have specified all the dependencies" is located in
  ch. 10 and rests on the build running in a temporary directory with only
  store inputs visible, not on a namespace sandbox; Cromwell's `md5` is
  described as one configurable local hashing strategy rather than the default;
  Ray's Java and C++ surfaces are attributed to Ray as it stands rather than to
  the OSDI'18 paper; Guix is called an input-addressed (functional) store, the
  distinction the paper itself draws in §2.2; the abstract's "usually buy the
  property with infrastructure" becomes "often", since the paper's own survey
  splits evenly, and its identity claim now carries the in-root condition
  stated in §1 and §3.1; the §3.3 sentence on distributed backends no longer
  says compute nodes need the state database visible, since it also says they
  never touch it; the §6.2 explanation of the `mtime+hash` gap now names key
  computation and index lookup rather than re-hashing alone; §7.2's "documented
  pain" is restated as a consequence of a per-tree record's scope rather than a
  cited finding; and §7.3 now records that only the Snakemake 7.x line was
  measured.

**Deferred.** Two class-B findings are recorded rather than fixed: the
benchmark record does not state the number of end-to-end samples per cell or
the 638-byte size of the perturbed input (`bench_lib.py`), both of which belong
in a regenerated `RESULTS.md` rather than in prose written around it.

---

---

*Maintained by Noogram. Corrections and counterexamples are welcome as issues
on `noogram/oxymake`.*

## G. Gate enforcement (2026-09-05, issue #2)

**"Gate approval is programmatic" / "Gate approval — Full — `ox gate approve/reject`".**
*v2 wording (agent API):* "Gate approval is programmatic: `ox gate approve qc_check --approver "ci:qc-runner" --reason "metrics within threshold"`."
*v2 wording (implementation matrix):* "Gate approval | Full | `ox gate approve/reject`", under a caption stating that all rows are implemented.
*v2 wording (positioning):* "its gates are run-time human or agent approval checkpoints inside a workflow".

The checkpoint does not exist at run time in v0.1.0. `[gate.*]` declarations
are parsed (`ox-format::Gate`), `ox gate list/approve/reject` read and write
the `gates` table, and the scheduler carries the blocking logic behind a
`GateCheck` trait — but the only production call to the scheduler
(`crates/ox-cli/src/commands/run.rs`, the `run_scheduler_with_cache` call)
passes `None` for the checker, the only `GateCheck` implementations are the
permissive `NoGates` and two test doubles, and `Db::create_gate` is called by
no run-path code. A guarded rule therefore runs without approval and
`ox gate list` reports no pending gate. The v2 example command is also not the
shipped interface: `ox gate approve` takes the numeric record identifier
printed by `ox gate list`, not a gate name. Reported as
https://github.com/noogram/oxymake/issues/2 (2026-09-03), which reproduces it
on macOS arm64 and Linux x86_64.

v3 states at each site that gates are declared and managed but not yet
enforced, gives the shipped `approve <id>` form, and adds a "Gate
enforcement" paragraph to the limitations. The code is unchanged by this
correction; enforcement is tracked in the issue.
Source: `crates/ox-core/src/traits/gate.rs` (module doc: "gates are not
enforced unless a concrete implementation is provided"); `run.rs` call site;
`ox-state/src/db.rs` `create_gate` and its single test-only caller.

