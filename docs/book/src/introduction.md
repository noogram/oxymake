# OxyMake

> Next-generation workflow orchestration in Rust.
> The `uv` of computational workflows.

OxyMake is a fast, declarative workflow orchestration tool that combines the
proven ideas of Snakemake (file-based rules, backward-chaining DAG, wildcards)
with modern engineering: content-addressable caching, polyglot execution,
in-memory data passing, and first-class support for both human and AI agent users.

## Key Features

- **Fast DAG resolution**: 10K-job DAG resolved in 69 ms on M4 Max, 33.3× faster than Snakemake 7.32.4 (100K-job scaling out of scope for this benchmark wave; cold end-to-end is slower than Snakemake — an honest trade for content-addressable correctness)
- **Content-addressable**: no phantom re-runs from git checkout or file copies
- **Polyglot**: shell, Python, R, Julia — each rule chooses its language
- **Daemon-free**: `ox run` starts, works, exits. No server to manage.
- **Agent-friendly**: `--json` output, structured events, typed API
- **Scales**: same workflow on laptop, SLURM cluster, or Ray cluster (Kubernetes designed, not yet implemented)

## Quick Example

```toml
# Oxymakefile.toml
ox_version = "0.1"

[config]
samples = ["A", "B", "C"]

[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.json"]
shell = "python process.py {input} {output}"

[rule.report]
input = ["results/{sample}.json"]
output = ["reports/summary.html"]
shell = "python report.py {input} > {output}"
```

```bash
ox run                    # build everything
ox run -j 8               # 8 parallel jobs
ox status                 # what's running?
ox plan                   # what would run?
```
