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

*Maintained by Noogram. Corrections and counterexamples are welcome as issues
on `noogram/oxymake`.*
