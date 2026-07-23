# Adversarial falsification audit — Crusoe revision v3

**Audit date:** 2026-07-20
**Revision under test:** merge commits `9d60058` (paper v3) and `702a414` (ADR-018)
**Method:** claim-by-claim attempted falsification against primary documentation,
current upstream source where documentation was insufficient, and the repository code.
`CONFIRMED` means the attempted counterexample failed. `REFUTED` means at least one
material counterexample succeeded. This report intentionally makes no paper changes.

## Executive verdict

**REFUTED as publication-ready.** The revision substantially improves the CWL-family
account and the checked-in PDF does contain the new caveat, but the central delivery-envelope
claim is still broader than the implementation. Most importantly:

1. the standalone arXiv abstract is stale and repeats the retired “Make-lineage runners”
   generalisation without the `mtime+hash` limitation;
2. “shared cache ships on by default” and “remote caches automatically promote to `hash`”
   are not implemented in `ox`;
3. same-platform portability is conditional on path identity, while the key explicitly hashes
   paths;
4. the paper contradicts its corrected Snakemake account twice; and
5. no visible erratum exists in the paper, PDF, abstract, or repository paper directory.

The defensible thesis is narrower: **a daemon-free local executable whose job key is derived
from declared input content and selected execution metadata by default, with metadata-gated
output re-verification by default and unconditional output hashing available by opt-in.**

## 1. The five corrected external-system characterisations

### C1 — CWL is a specification with explicit data links, not an engine with an mtime policy

**Paper claim:** `oxymake-paper.tex:154-158,420-430`.

**Verdict: CONFIRMED.** The CWL Workflow v1.2.1 specification defines step inputs as
connections from workflow inputs or upstream step outputs through the `source` field, including
explicit rules for inbound data links. The CommandLineTool specification defines execution
requirements while leaving implementation latitude for staging and execution. No cache or
change-detection policy is prescribed.

Primary sources:

- [CWL Workflow v1.2.1 — WorkflowStepInput](https://www.commonwl.org/v1.2/Workflow.html#WorkflowStepInput)
- [CWL CommandLineTool v1.2.1](https://www.commonwl.org/v1.2/CommandLineTool.html)

**Proposed fix:** none to the factual core. Remove or source the editorial assertion that CWL
documents are “verbose by design” for machine consumption (`:425-427`); the specification does
not establish an authorial motive.

### C2 — `cwltool` offers opt-in reuse keyed from content-sensitive execution inputs

**Paper claim:** `oxymake-paper.tex:234-237,431-435` says its optional cache is keyed on “a
content hash of tool, inputs, and container.”

**Verdict: REFUTED (mechanism overstated).** `--cachedir` is indeed opt-in and reuses
intermediate outputs. Current `cwltool` source, however, does **not** hash the whole tool
description. It builds a canonical dictionary from the generated command line; stdin/stdout/
stderr; file size plus checksum (falling back to size plus mtime when no checksum is available);
the selected container string; selected requirements/hints; and preserved environment, then
names the cache entry with MD5 of that dictionary. “Content hash of tool” is not an accurate
description, and the fallback is not content-addressed.

Primary sources:

- [`cwltool --cachedir` CLI documentation](https://cwltool.readthedocs.io/en/stable/cli.html#cmdoption-cwltool-cachedir)
- [`cwltool.command_line_tool` cache-key implementation, lines 842-939](https://github.com/common-workflow-language/cwltool/blob/main/cwltool/command_line_tool.py#L842-L939)
- [CWL `File.checksum` semantics](https://www.commonwl.org/v1.2/CommandLineTool.html#File)

**Proposed fix:** say: “With `--cachedir`, cwltool fingerprints the realised command,
content checksums where available (otherwise size/mtime), container selection, and selected
requirements.” Do not call the entire key a hash of the tool description.

### C3 — Galaxy has web-first and separate programmatic surfaces

**Paper claim:** `oxymake-paper.tex:440-444`.

**Verdict: CONFIRMED, with a scope correction.** Galaxy exposes a REST API; BioBlend is the
documented Python client; Planemo is a CLI for developing/testing Galaxy tools and workflows.
The existence claim is correct. Grouping Planemo with the general runtime API slightly obscures
its developer-tool role.

Primary sources:

- [Galaxy API documentation](https://docs.galaxyproject.org/en/latest/api_doc.html)
- [BioBlend documentation](https://bioblend.readthedocs.io/en/latest/)
- [Planemo documentation](https://planemo.readthedocs.io/en/latest/)

**Proposed fix:** replace “reaches programmatic use through ... Planemo” with “also exposes a
REST API and BioBlend client; Planemo provides a CLI for tool/workflow development and tests.”

### C4 — Cromwell is backend-agnostic and local execution is available by default

**Paper claim:** `oxymake-paper.tex:445-450`.

**Verdict: CONFIRMED for backends; REFUTED for mandatory deployment.** Cromwell documents
pluggable Local, HPC, Google Cloud, TES, and AWS Batch backends; Local is pre-enabled and is the
default backend. But `cromwell run` executes one workflow as a CLI process and exits. Therefore
“at the cost of a separate engine deployment” is only true for server-mode operations, not for
local default execution.

Primary sources:

- [Cromwell backends](https://cromwell.readthedocs.io/en/latest/backends/Backends/)
- [Cromwell Local backend](https://cromwell.readthedocs.io/en/stable/backends/Local/)
- [Cromwell run and server modes](https://cromwell.readthedocs.io/en/latest/Modes/)

**Proposed fix:** use “requires a separate execution engine” rather than “deployment,” and
distinguish one-shot `run` mode from server mode.

### C5 — Nextflow standard caching fingerprints file metadata, including mtime

**Paper claim:** `oxymake-paper.tex:112-114,152-153,226-232`.

**Verdict: CONFIRMED, but not exact enough for the abstract.** Current primary documentation
says the standard mode includes file name/path, size, and last-updated timestamp, while `deep`
includes content. The cache guide is more specific: the input-file hash uses the **full path**,
mtime, and size. Task records are always written, but reuse requires launching with `-resume`.
Thus the mtime characterisation is correct; “default cache mode” should not imply cache reuse is
active without `-resume`.

Primary sources:

- [Nextflow process `cache` directive](https://docs.seqera.io/nextflow/reference/process#cache)
- [Nextflow caching and resuming — task hash and modified inputs](https://docs.seqera.io/nextflow/cache-and-resume#modified-inputs)

**Proposed fix:** say “Nextflow's standard file fingerprint uses full path, size and mtime;
cached execution is reused only under `-resume`; `cache 'deep'` fingerprints contents.” This
also resolves the documentation's “name” versus “full path” shorthand in favour of its detailed
cache guide.

## 2. Central repositioned thesis and its asterisk

### T1 — “Bazel/Nix-grade content-addressing”

**Verdict: REFUTED.** The key mechanism is content-sensitive, but equivalence to the guarantee
envelope of Bazel/Nix is not supported:

- OxyMake has no sandbox, so undeclared reads silently escape the key (`:802-825`).
- Mutable image tags are hashed as strings, not resolved digests (`:767-770`; `key.rs:117-121`).
- Under the default, equal-size/equal-mtime modification of either a cached input hash or an
  output can be trusted without reading bytes (`run.rs:300-306`; `lookup.rs:275-285`).
- No working remote/shared-cache path is wired into the CLI.

The paper itself correctly admits the first two limitations, which makes a grade-equivalence
slogan especially unsafe.

**Proposed fix:** avoid “Bazel/Nix-grade.” Claim adoption of the *content-derived-key
principle*, with declared-input, environment, path, and validation-policy boundaries stated in
the same paragraph.

### T2 — “On by default” content addressing

**Verdict: CONFIRMED only for key construction; REFUTED for end-to-end verification.**
`CacheValidation::default()` is `MtimeHash`, and a database-backed run computes a BLAKE3 job
key. But both input hash reuse and output validation trust unchanged metadata. This is exactly
the distinction ADR-018 §2 requires.

**Proposed fix:** use ADR-018's mandatory wording verbatim wherever “by default” appears:
the key is content-derived; re-verification depth is policy-controlled; `mtime+hash` trusts an
unchanged `(mtime,size)` pair; `hash` reads bytes unconditionally.

### T3 — The asterisk is honest everywhere, including abstracts

**Verdict: REFUTED.** The TeX abstract (`:104-139`) describes a content-derived key but never
states that default input/output re-verification can trust unchanged metadata. More seriously,
`docs/paper/arxiv-abstract.txt:1` is the pre-v3 abstract: it starts with the retired universal
“Make-lineage workflow runners” framing, omits the corrected Nextflow account, and has no
`mtime+hash` caveat. The checked-in PDF and arXiv source tarball do contain the body caveat.

**Proposed fix:** regenerate `arxiv-abstract.txt` from the TeX abstract and add one compact
sentence to both abstracts: “The key is content-derived by default; the default verifier trusts
unchanged size+mtime, while `--cache-validation=hash` re-reads every byte.”

### T4 — Single daemon-free binary

**Verdict: CONFIRMED for the local engine envelope, REFUTED as currently universalised.** A
local `ox run` does not require an OxyMake daemon. However, the paper repeatedly upgrades this
to “single static binary with no runtime dependencies” (`:304,1475,1964-1966,2017`) without a
reproducible linkage attestation. It also advertises SLURM/Ray execution, whose external runtime
services are outside the no-coordinator envelope.

**Proposed fix:** say “one `ox` executable and no OxyMake daemon for local execution.” Report
the linkage target and `file`/`ldd`/`otool` evidence before claiming static linkage or no runtime
dependencies.

### T5 — Shared cache ships on by default and automatically forces full hashing

**Verdict: REFUTED.** The paper says the “shared cache” ships on by default (`:241-243`) and
that remote caches automatically promote to `hash` (`:795-796`). `ox-cli` has no dependency on
`ox-cache-remote`, accepts no remote-cache argument, and only opens local `.oxymake`. The
directory backend exists as a library; S3/GCS explicitly return unavailable. The paper's own
feature table labels S3/GCS as scaffold (`:2053`). ADR-018 correctly identifies this as unbuilt.

**Proposed fix:** remove “shared cache” from the shipping conjunction and change automatic
promotion to a design requirement for a future wired remote cache. Until then claim portable
key *format*, not shipped shared-cache reuse.

### T6 — Cache decisions travel across same-platform machines

**Verdict: REFUTED as an unqualified claim.** `key.rs:75-87` deliberately hashes each input
path with its content hash. `run.rs:328` uses `p.display()` at the call site. Relative paths can
produce stable keys when workflows are laid out identically, but absolute paths and differently
resolved path spellings produce different keys. In addition, there is no CLI transport for the
cache entries.

**Proposed fix:** say “keys can remain identical across identically laid-out trees when all
hashed paths are stable and the OS/architecture matches.” Add an actual cross-root test covering
relative and absolute inputs before broadening it.

## 3. Remaining external claims found by a full `.tex` sweep

### E1 — Snakemake's current invalidation behaviour

**Verdict: REFUTED in two surviving sentences.** The corrected account says Snakemake 7 uses
recorded provenance and the paper's benchmark reports zero jobs rerun after mtime churn
(`:182-195,409-414,1869-1875`). Yet `:783-785` says pure `mtime` “matches Make/Snakemake
behavior,” and `:1782-1786` says Snakemake merely “checks file mtimes and moves on.” Those are
the very claims v3 otherwise retracts.

**Proposed fix:** change the former to “matches Make-style metadata-only behavior” and the
latter to “uses its local provenance records without OxyMake's content-store writes.”

### E2 — “Snakemake pipelines port directly”

**Verdict: REFUTED.** The abstract's universal statement (`:125-127`) conflicts with the
compatibility section, which lists arbitrary Python expressions and `run:` blocks as unsupported
and requiring migration (`:1928-1952`). Four successful fixtures do not support all pipelines.

**Proposed fix:** “many rule-oriented Snakemake pipelines can be translated; embedded Python
and `run:` blocks require manual migration.”

### E3 — WDL requires every task to declare a full runtime block

**Verdict: REFUTED.** `:1954-1960` says every WDL task must declare a full runtime block. In
WDL 1.2, the `runtime` section and individual attributes are optional.

Primary source: [WDL 1.2 specification — task runtime section](https://github.com/openwdl/wdl/blob/wdl-1.2/SPEC.md#runtime-section)

**Proposed fix:** remove that sentence. Describe ceremony using measured syntax or required
typed declarations, not an optional block.

### E4 — Ray and Dask require Python

**Verdict: REFUTED.** `:679-682` says both require Python. Ray officially supports Java and a
C++ API in addition to Python. Dask is Python-native, but the conjunction is false.

Primary source: [Ray language API documentation](https://docs.ray.io/en/latest/ray-overview/getting-started.html)

**Proposed fix:** “Dask is Python-native; Ray is multi-language but its principal workflow
surface differs from declarative file rules.”

### E5 — Airflow/Argo never key execution on content

**Verdict: REFUTED as a universal.** `:667-677` correctly describes their normal DAG/task
model, but both expose cache/memoization mechanisms whose keys can be user-supplied and therefore
content-derived. The defensible contrast is lack of an automatic declared-file-content key, not
impossibility.

Primary sources:

- [Airflow task-instance model](https://airflow.apache.org/docs/apache-airflow/stable/core-concepts/tasks.html)
- [Argo Workflows memoization](https://argo-workflows.readthedocs.io/en/latest/memoization/)

**Proposed fix:** “Neither automatically derives a reusable result key from the content of all
declared file inputs.”

### E6 — Bazel/Buck lack wildcard expansion, environment management, and gates

**Verdict: REFUTED/UNSUPPORTED.** The bundle claim at `:2162-2166` is too broad: Bazel has
target patterns, configurable toolchains/platform environments, and policy/analysis extension
points, even if none maps exactly to OxyMake's feature names.

**Proposed fix:** compare concrete semantics one axis at a time and cite primary manuals;
avoid “features ... that build systems lack.”

### E7 — OxyMake's static DAG is simply the same scheduler strategy as Bazel

**Verdict: REFUTED as stated.** `:487-495,2168-2177` treats Bazel as a wholly static dependency
graph. Bazel supports action-time input discovery/discovered inputs; its evaluation graph is not
equivalent to an OxyMake DAG fully fixed before execution.

Primary source: [Bazel rules — dependency discovery](https://bazel.build/extending/rules#dependency-discovery)

**Proposed fix:** narrow the analogy to “analysis normally constructs an action graph before
execution,” and explicitly acknowledge discovered inputs.

### E8 — “Nix/Guix: same inputs always resolve to the same output”

**Verdict: REFUTED if read as output determinism.** `:197-203` conflates derivation identity
with reproducible bytes. A content-derived store/derivation name does not make an impure or
nondeterministic builder deterministic. The key changes with declared inputs; output identity is
a separate property.

**Proposed fix:** “the same declared derivation inputs select the same store identity; changed
declared inputs select a different derivation.” Do not promise byte-identical output without a
reproducibility premise.

## 4. Paper ↔ ADR-018 ↔ code coherence

| Claim | Paper | ADR-018 | Code | Verdict |
|---|---|---|---|---|
| Key includes rule source, input `(path,hash)`, params, environment and platform | `:119-121,204-206,746-759` | Accurate | `key.rs:34-98`; also hashes shell executable and a format tag | **CONFIRMED**, paper formula is incomplete but not false |
| Input content hashes are always freshly read | implied by “pure function” | Distinguishes key from verification | `run.rs:300-326` may reuse a stored hash on unchanged metadata | **REFUTED** unless “key value” is separated from “how ingredients are obtained” |
| Default is `mtime+hash` | `:191,776` | Required wording | `strategy.rs:24-27,62-67`; `run.rs:1026-1050` | **CONFIRMED** |
| Equal-size/equal-mtime output corruption is caught by default | explicitly denied at `:779-782` | explicitly denied | `lookup.rs:275-285,364-375` trusts it | **CONFIRMED denial** |
| Remote/shared cache is live and forces `hash` | `:241-243,795-796` | Says it is unbuilt | no `ox-cli` dependency or flag; S3/GCS stubs | **REFUTED** |
| `trust_scope` is part of the key | not in paper formula | says unbuilt | absent from `CacheKeySpec` | **CONFIRMED absent** |
| Keys travel across trees/machines | `:123,206-207,225,1807-1809` | says verified for relative patterns | path bytes enter key at `key.rs:84-86`; call site uses displayed path | **REFUTED as universal; conditional for stable relative paths** |
| Output hashes are part of the cache key | paper sometimes blurs “outputs hashed” with key | distinguishes output verification | outputs live in validation records, not `CacheKeySpec` | **REFUTED if read as key ingredient** |
| Kubernetes execution works unchanged | `:683-685` | not addressed | no `ox-exec-k8s`; CLI accepts only local/slurm/ray | **REFUTED** (paper feature table itself says Planned at `:2051`) |

ADR-018 is generally more candid than the paper and code comments. Its own cwltool summary should
nevertheless be updated to reflect the current fallback to `(size,mtime)` when a checksum is
unavailable and the additional selected requirements/environment in the key dictionary.

## 5. Crusoe acknowledgment and erratum visibility

### A1 — Acknowledgment is sober

**Verdict: CONFIRMED.** `:2466-2472` thanks Michael R. Crusoe for corrections, identifies issue
`noogram/oxymake#1`, and attributes the broader audit to what the review prompted rather than to
him. It is concise and non-promotional. The PDF and arXiv source tarball contain it.

### A2 — Erratum is visible

**Verdict: REFUTED.** There is no section or note labelled “Erratum,” “Correction,” or
“Revision note” in the paper, abstract, or paper directory. An acknowledgment is not an erratum:
it does not tell a reader what the former claims were, which claims changed, or which version was
affected. The stale standalone abstract makes this omission operationally significant.

**Proposed fix:** add a short, prominent revision/erratum note near the front matter and a
repository `docs/paper/ERRATUM.md`, linked from the paper landing surface. State the corrected
claims (CWL spec/engine distinction, cwltool cache, Nextflow standard mode, Snakemake provenance,
Galaxy/Cromwell scope) and the affected prior version. Keep the acknowledgment unchanged.

## Minimum fix set before calling v3 corrected

1. Synchronise both abstracts and include the default verifier limitation.
2. Correct the cwltool key description to match current source, including checksum fallback.
3. Remove shipping shared/remote-cache claims and the automatic-promotion statement.
4. Qualify cross-machine portability by path stability and actual transport availability.
5. Remove the two surviving Snakemake-mtime contradictions.
6. Narrow “Snakemake pipelines port directly.”
7. Correct the WDL runtime-block, Ray, Airflow/Argo, Bazel, Nix determinism, and Kubernetes claims.
8. Publish a visible erratum/revision note.

Until those are made, ADR-018's repositioning is directionally sound but the paper does not yet
honour ADR-018's own “every future statement” rule.
