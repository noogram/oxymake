# Executors

OxyMake separates **what** to run (rules, DAG) from **where** to run it
(executors). The same workflow runs on a laptop or a thousand-node cluster
with zero changes -- just switch the `--executor` flag.

## Available Executors

| Executor | Flag | Backend | GPU | Memory Passing |
|----------|------|---------|-----|----------------|
| Local | `--executor local` (default) | Tokio thread pool | OS-level | Same-process |
| SLURM | `--executor slurm` | `sbatch` / `sacct` | GRES | Shared filesystem |
| Ray | `--executor ray` | Ray Jobs API | First-class | Object store (zero-copy) |
| Kubernetes | `--executor k8s` | kube-rs (planned) | Device plugin | -- |

## Local Executor

The default. Runs jobs as subprocesses on the local machine.

```bash
ox run                # single job at a time
ox run -j 8           # 8 parallel jobs
```

Best for development, small pipelines, and single-node execution.

## SLURM Executor

Submits jobs to an HPC cluster via `sbatch` and polls status with `sacct`.

```bash
ox run --executor slurm
```

Features:
- Job arrays for wildcard expansions
- GPU scheduling via GRES
- Resource mapping: `cpu`, `mem`, `gpu` map to SLURM `--cpus-per-task`,
  `--mem`, `--gres=gpu:N`

## Ray Executor

Submits jobs to a Ray cluster via the Ray Jobs API. Ray provides elastic
distributed execution with a shared object store for fast intermediate data
passing.

### Setup

Start a Ray head node (or connect to an existing cluster):

```bash
ray start --head
# Dashboard: http://127.0.0.1:8265
```

Run the workflow:

```bash
ox run --executor ray
```

### Configuration

Configure the Ray executor in `.oxymake/config.toml` or `Oxymakefile.toml`:

```toml
[executor.ray]
dashboard_address = "http://127.0.0.1:8265"
working_dir = "/shared/oxymake"
poll_interval_min = "2s"
poll_interval_max = "30s"
max_submit = 10
```

| Setting | Default | Description |
|---------|---------|-------------|
| `dashboard_address` | `http://127.0.0.1:8265` | Ray dashboard URL |
| `working_dir` | `.` | Staging directory on shared filesystem |
| `poll_interval_min` | `2s` | Minimum status polling interval |
| `poll_interval_max` | `30s` | Maximum status polling interval |
| `max_submit` | unlimited | Max concurrent job submissions |
| `autoscaler_aware` | `false` | Query cluster capacity before submitting |

### Resource Mapping

| OxyMake | Ray | Notes |
|---------|-----|-------|
| `cpu` | `num_cpus` | Direct mapping |
| `mem` | `memory` | Bytes |
| `gpu` | `num_gpus` | Fractional GPUs supported (`gpu = 0.5`) |
| `custom:*` | Custom resources | Arbitrary Ray custom resources |

### Memory Passing

When two consecutive `call`-mode rules run on the Ray executor, data passes
through Ray's object store without disk writes. OxyMake's materialization
policies map to Ray behavior:

| Policy | Ray Behavior |
|--------|--------------|
| `always` | Write to shared FS + object store |
| `auto` | Object store only (materialized if downstream needs file) |
| `never` | Object store only, evicted after consumers finish |
| `final` | Object store, written to shared FS only for DAG leaves |

### Execution Modes

The Ray executor supports all four execution modes:

- **shell** -- commands run as Ray job entrypoints
- **run** -- inline scripts submitted as Ray jobs
- **script** -- external scripts submitted as Ray jobs
- **call** -- Python functions with object store integration

## Choosing an Executor

| Use Case | Recommended Executor |
|----------|---------------------|
| Development / CI | Local |
| HPC cluster (static allocation) | SLURM |
| Cloud / elastic GPU clusters | Ray |
| ML pipelines with in-memory passing | Ray |
| Kubernetes-native environments | K8s (planned) |

## Mixed-Executor DAGs

OxyMake owns the DAG; executors are job-dispatch backends. A future
enhancement will allow per-rule executor assignment, enabling mixed-executor
DAGs where some rules run locally and others dispatch to Ray or SLURM.

## Next Steps

- [Execution Modes](./execution-modes.md) -- the four ways rules execute
- [Materialization Policy](./materialization.md) -- controlling disk I/O
- [Configuration](../reference/configuration.md) -- project settings
