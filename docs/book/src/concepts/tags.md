# Tags and Filtering

Tags let you organize rules into logical groups and selectively run subsets
of your workflow.

## Assigning Tags

Add tags to any rule in your `Oxymakefile.toml`:

```toml
[rule.align]
input = ["data/{sample}.fastq"]
output = ["aligned/{sample}.bam"]
shell = "bwa mem ref.fa {input} | samtools sort > {output}"
tags = ["alignment", "compute-heavy"]

[rule.qc]
input = ["aligned/{sample}.bam"]
output = ["qc/{sample}_report.html"]
shell = "fastqc {input} -o qc/"
tags = ["qc", "fast"]
```

## Filtering by Tag

Run only jobs matching a tag:

```bash
ox run --tag alignment        # Only alignment jobs
ox run --tag qc               # Only QC jobs
ox run --tag compute-heavy    # Only compute-heavy jobs
```

Exclude jobs by tag:

```bash
ox run --exclude-tag slow     # Skip slow jobs
```

## Tag-Based DAG Views

Tags integrate with the DAG visualization:

```bash
ox dag --group-by tag         # Group nodes by tag in the DAG view
ox plan --tag alignment       # Show plan for alignment jobs only
```

## Hierarchical Organization

Use dotted tag names for hierarchy:

```toml
tags = ["pipeline.alignment", "resource.gpu"]
```

This enables filtering at different levels:

```bash
ox run --tag "pipeline.*"         # All pipeline stages
ox run --tag "resource.gpu"       # Only GPU jobs
```

## Use Cases

- **Selective re-runs**: Re-run only QC after parameter changes
- **Resource-based scheduling**: Tag GPU vs CPU jobs for different executors
- **Stage grouping**: Organize large workflows into logical phases
- **Development iteration**: Run only the stage you are working on

## Next Steps

- [The Three Graphs](./three-graphs.md) -- how tags affect DAG visualization
- [Execution Modes](./execution-modes.md) -- how jobs are executed
