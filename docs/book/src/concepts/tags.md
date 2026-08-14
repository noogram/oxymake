# Tags and Filtering

Tags are key/value labels you attach to a rule. They travel with every
concrete job the rule produces, so you can group, filter and report on jobs
without touching the workflow structure.

## Assigning Tags

`tags` is a **table of string keys to string values** — not an array. An array
of strings is a parse error.

```toml
[rule.align]
input  = ["data/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell  = "bwa mem ref.fa {input} | samtools sort > {output}"
tags   = { stage = "align", cost = "heavy" }

[rule.qc]
input  = ["aligned/{sample}.bam"]
output = ["qc/{sample}_report.html"]
shell  = "fastqc {input} -o qc/"
tags   = { stage = "qc", speed = "fast" }
```

Pick your own keys. `stage`, `speed`, `cost` and `owner` are common, but
nothing is reserved. Each job's resolved wildcard values are merged into its
tag map too, so a job of `align` above carries
`{ sample = "NA12878", stage = "align", cost = "heavy" }`.

## Where Tags Show Up

**In the plan.** `ox plan --json` carries each job's `tags` map, so a script or
agent can select jobs by tag before deciding what to do:

```bash
ox plan --json | jq '.jobs[] | select(.tags.stage == "align") | .job_id'
```

**In the event stream.** The `job_queued` event carries the originating job's
tags, and `ox subscribe` filters on them with `--where KEY=VALUE` (repeatable,
AND logic):

```bash
ox subscribe --where stage=align                 # only alignment jobs
ox subscribe --where stage=align --where cost=heavy
```

Events that carry no tags — everything other than `job_queued` — are dropped
while a `--where` filter is active.

## What Tags Do Not Do

Tags are a **labelling and observation** mechanism, not a job selector for
execution. There is no `ox run --tag` or `ox run --exclude-tag`. To run a
subset of the workflow, use the mechanisms that do exist:

```bash
ox run results/report.txt      # explicit targets
ox run --rule align            # a single rule
ox run --until merge_vcf        # stop at a rule
ox run --omit-from qc           # skip a subtree
```

`ox status --group-by` accepts `rule` and `stage` (an alias for rule name); it
does not group by tag.

## Use Cases

- **Observability**: subscribe to just the GPU jobs and route their events
  elsewhere.
- **Reporting**: post-process `ox plan --json` to count work per stage.
- **Downstream scheduling**: an agent reads tags to decide which executor or
  queue a job belongs to.

## Next Steps

- [The Three Graphs](./three-graphs.md) -- how the DAG is built and viewed
- [Execution Modes](./execution-modes.md) -- how jobs are executed
