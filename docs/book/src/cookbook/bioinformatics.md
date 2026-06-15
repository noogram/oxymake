# Bioinformatics Pipeline

This cookbook walks through a multi-sample FASTQ-to-BAM-to-VCF variant calling
pipeline in OxyMake. The workflow uses `sort`, `grep`, and `wc` as stand-ins
for real bioinformatics tools (BWA, samtools, GATK), so you can run it on any
machine without installing anything.

The concepts transfer directly to a production pipeline: just swap the shell
commands for real tool invocations.

## What You Will Learn

- Wildcard-driven sample processing across multiple samples
- Named inputs for rules with multiple input files
- Tags for organizing pipeline stages
- Target-based filtering to run a subset of samples
- `--rule` filtering to run a subset of stages

## The Complete Oxymakefile

Create a directory and save this as `Oxymakefile.toml`:

```toml
ox_version = "0.1"

[config]
samples = ["NA12878", "NA12891", "NA12892"]
chromosomes = ["chr1", "chr2", "chr3"]

# ── Default target ──────────────────────────────────────────────
[rule.all]
input = ["results/cohort_report.txt"]

# ── Stage 1: Generate mock FASTQ reads ─────────────────────────
[rule.simulate_reads]
output = ["fastq/{sample}_R1.fastq", "fastq/{sample}_R2.fastq"]
tags = ["stage.simulate", "fast"]
shell = """
mkdir -p fastq
for i in $(seq 1 50); do
  echo "@{sample}_read${i}/1 chr$((i % 3 + 1)):$((i * 100))" >> {output[0]}
  echo "ACGTACGTACGTACGT" >> {output[0]}
  echo "+" >> {output[0]}
  echo "IIIIIIIIIIIIIIII" >> {output[0]}
  echo "@{sample}_read${i}/2 chr$((i % 3 + 1)):$((i * 100))" >> {output[1]}
  echo "TGCATGCATGCATGCA" >> {output[1]}
  echo "+" >> {output[1]}
  echo "IIIIIIIIIIIIIIII" >> {output[1]}
done
"""

# ── Stage 2: Align reads → sorted BAM ──────────────────────────
# Stand-in: sort the FASTQ by read name to simulate alignment + sorting.
[rule.align]
input = { r1 = "fastq/{sample}_R1.fastq", r2 = "fastq/{sample}_R2.fastq" }
output = ["aligned/{sample}.bam"]
tags = ["stage.align", "compute-heavy"]
resources = { cpu = 4, mem = "8G" }
shell = """
mkdir -p aligned
echo "## BAM for {sample}" > {output}
echo "## Aligned from {input.r1} and {input.r2}" >> {output}
cat {input.r1} {input.r2} | grep "^@" | sort >> {output}
echo "## EOF" >> {output}
"""

# ── Stage 3: Call variants per chromosome ───────────────────────
# Stand-in: grep reads matching the chromosome, count them as "variants."
[rule.call_variants]
input = { bam = "aligned/{sample}.bam" }
output = ["vcf/{sample}_{chrom}.vcf"]
tags = ["stage.call", "compute-heavy"]
resources = { cpu = 2, mem = "4G" }
shell = """
mkdir -p vcf
echo "##fileformat=VCFv4.2" > {output}
echo "##source=oxymake-cookbook" >> {output}
echo "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO" >> {output}
grep "{chrom}" {input.bam} | awk '{{
  split($2, a, ":");
  printf "%s\t%s\t.\tA\tG\t30\tPASS\tDP=20\n", a[1], a[2]
}}' >> {output}
"""

# ── Stage 4: Merge per-chromosome VCFs into one per sample ─────
[rule.merge_vcf]
input = ["vcf/{sample}_{chrom}.vcf"]
output = ["vcf/{sample}_merged.vcf"]
tags = ["stage.merge"]
shell = """
echo "##fileformat=VCFv4.2" > {output}
echo "##source=oxymake-merge" >> {output}
echo "#CHROM\tPOS\tID\tREF\tALT\tQUAL\tFILTER\tINFO" >> {output}
for f in {input}; do
  grep -v "^#" "$f" >> {output}
done
sort -k1,1 -k2,2n -o {output} {output}
"""

# ── Stage 5: Per-sample QC report ──────────────────────────────
[rule.qc]
input = { bam = "aligned/{sample}.bam", vcf = "vcf/{sample}_merged.vcf" }
output = ["qc/{sample}_report.txt"]
tags = ["stage.qc", "fast"]
shell = """
mkdir -p qc
echo "=== QC Report: {sample} ===" > {output}
echo "Total reads: $(grep -c "^@" {input.bam})" >> {output}
echo "Variants called: $(grep -vc "^#" {input.vcf})" >> {output}
echo "Chromosomes: $(grep -v "^#" {input.vcf} | cut -f1 | sort -u | tr '\n' ' ')" >> {output}
"""

# ── Stage 6: Cohort report ─────────────────────────────────────
[rule.cohort_report]
input = ["qc/{sample}_report.txt"]
output = ["results/cohort_report.txt"]
tags = ["stage.report"]
shell = """
mkdir -p results
echo "=============================" > {output}
echo " Variant Calling Cohort Report" >> {output}
echo "=============================" >> {output}
echo "" >> {output}
for f in {input}; do
  cat "$f" >> {output}
  echo "" >> {output}
done
echo "--- Summary ---" >> {output}
echo "Samples processed: $(echo {input} | wc -w | tr -d ' ')" >> {output}
"""
```

## Create the Project

```bash
mkdir bioinfo-pipeline && cd bioinfo-pipeline
# Save the Oxymakefile.toml above
ox init   # if you want the .oxymake directory pre-created
```

No input data files are needed -- the `simulate_reads` rule generates
everything from scratch.

## Run the Full Pipeline

```bash
ox plan
```

```
Plan: 5 rules, 15 jobs, 3 source files
Targets: results/cohort_report.txt
  1. [simulate_reads-NA12878] rule=simulate_reads -> [fastq/NA12878_R1.fastq, fastq/NA12878_R2.fastq]
  2. [simulate_reads-NA12891] rule=simulate_reads -> [fastq/NA12891_R1.fastq, fastq/NA12891_R2.fastq]
  3. [align-NA12878] rule=align -> [aligned/NA12878.bam]
  4. [call_variants-NA12878-chr1] rule=call_variants -> [vcf/NA12878_chr1.vcf]
  ...
  15. [cohort_report] rule=cohort_report -> [results/cohort_report.txt]
```

```bash
ox run -j 4
```

OxyMake runs up to 4 jobs in parallel. The `simulate_reads` jobs run first
(no dependencies), then `align`, then `call_variants` fans out across
samples and chromosomes, and finally everything converges into the cohort
report.

## Filter by Sample

Run only one sample during development by requesting its leaf target
(wildcards in the target select the matching jobs):

```bash
ox run "qc/NA12878_report.txt"
```

This builds the pipeline for NA12878 only, skipping NA12891 and NA12892.
Combined with caching, this lets you iterate on pipeline logic without
waiting for all samples.

Later, run the full cohort:

```bash
ox run
```

NA12878 is cached. Only NA12891 and NA12892 are computed.

## Filter by Rule

Run only the QC stage with `--rule` (exact name or `/regex/`; assumes
upstream outputs exist):

```bash
ox run --rule qc
```

View the DAG grouped by stage:

```bash
ox dag --group-by tag
```

## Named Inputs

Several rules use named inputs for clarity. Compare:

```toml
# Positional (works but cryptic with multiple inputs)
input = ["aligned/{sample}.bam", "vcf/{sample}_merged.vcf"]
shell = "check {input[0]} {input[1]}"

# Named (self-documenting)
input = { bam = "aligned/{sample}.bam", vcf = "vcf/{sample}_merged.vcf" }
shell = "check {input.bam} {input.vcf}"
```

Named inputs make your workflow readable as it grows.

## Adding a New Sample

Edit `Oxymakefile.toml`:

```toml
[config]
samples = ["NA12878", "NA12891", "NA12892", "NA12893"]
```

Run again:

```bash
ox run -j 4
```

Only the NA12893 jobs run. Everything else is cached.

## Adapting to Real Tools

Replace the stand-in commands with real bioinformatics tools:

```toml
[rule.align]
input = { r1 = "fastq/{sample}_R1.fastq", r2 = "fastq/{sample}_R2.fastq" }
output = ["aligned/{sample}.bam"]
tags = ["stage.align", "compute-heavy"]
resources = { cpu = 8, mem = "32G" }
shell = """
bwa mem -t {resources.cpu} reference.fa {input.r1} {input.r2} \
  | samtools sort -@ 4 -o {output}
samtools index {output}
"""
```

The workflow structure stays the same. Only the shell commands change.

## Next Steps

- [Rules and Wildcards](../concepts/rules-and-wildcards.md) -- wildcard
  expansion and constraints
- [Tags and Filtering](../concepts/tags.md) -- organizing large workflows
- [Idempotent Execution](../concepts/idempotent-execution.md) -- cooperative
  multi-session runs
