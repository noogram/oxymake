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

Opt into resource admission with `--resource-budget`, for example
`ox run -j 3 --resource-budget cpu=6,gpu=1`. Two jobs each requesting four
CPUs then run sequentially even with `-j 3`; two half-GPU jobs may run
together. An empty budget leaves dispatch bounded by `-j` alone.

`-j` caps concurrent jobs, `--resource-budget` caps declared resources held by
those jobs, and `--memory-budget` separately caps in-memory outputs. A budget
is per `ox run`; concurrent processes each get their own capacity, with no
host-wide coordination or capacity promise.

Admission validates the selected graph before cache checks, claims, output
cleanup or recipes. It reserves only resources named in the budget, using the
[resource alias and value contract](../reference/format.md#resources).
Reservations cover preparation, execution, finalization and output hashing.
Cancellation keeps a running attempt's reservation until its task exits;
retries release reservations before backoff and acquire them again for the
next attempt. Budgeted retry backoff remains responsive to shutdown.

Each scheduler run owns its budget. Concurrent runs each receive their own
capacity; claims and output locks do not coordinate resource budgets. These
reservations do not constrain actual CPU use or RSS. `--memory-budget` remains
the separate budget for in-memory output materialization. No fairness or
preemption policy is promised.

Fatal executor errors and task panics retain the existing cancel/abort path:
the scheduler requests executor cancellation and aborts its remaining Tokio
tasks. Guards release when those tasks are dropped, including on unwind;
this path does not wait for external child processes to exit. A process abort
cannot run Rust destructors, and its per-run budget disappears with it.

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
| `cpu` / `cpus` | `num_cpus` | Exact to `0.0001` before Ray conversion |
| `mem` / `memory` | runtime environment | Bytes or a binary-unit string |
| `mem_mb` / `mem_gb` | runtime environment | MiB / GiB converted to bytes |
| `gpu` / `gpus` | `num_gpus` | Fractional GPUs up to one, or whole multi-GPU counts |
| any custom key / `custom:*` | Custom resources | Case-sensitive custom resources |

The native Ray DAG driver forwards CPU, GPU and custom resources. Memory is
normalized and validated but is not a Ray scheduling reservation.

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
