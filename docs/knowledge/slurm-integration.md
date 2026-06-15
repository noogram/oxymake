# SLURM Integration Knowledge Base

> Knowledge document for OxyMake SLURM executor — suitable for transcription
> into a Claude Code skill for ongoing development guidance.

## SLURM Fundamentals for Workflow Engines

### Core Commands

| Command | Purpose | Key Flags |
|---------|---------|-----------|
| `sbatch` | Submit batch job | `--parsable`, `--wrap`, `-J`, `--dependency` |
| `squeue` | Query running/pending jobs | `-j <id>`, `-h`, `-o <format>` |
| `sacct` | Query completed job accounting | `-j <id>`, `--parsable2`, `-o <fields>` |
| `scancel` | Cancel a job | `<job_id>` |
| `sinfo` | Cluster/node info | `-N`, `--version` |

### sbatch Output Parsing

```bash
# Standard: "Submitted batch job 12345"
sbatch job.sh
# Parsable: "12345" or "12345;cluster_name"
sbatch --parsable job.sh
```

Regex for standard output: `Submitted batch job (\d+)`

### sacct Output Parsing

Best practice: use `--parsable2` (pipe-delimited, no trailing pipe):
```bash
sacct -j 12345 --parsable2 --noheader -o JobID,State,ExitCode,MaxRSS,Elapsed,NodeList
# Output: 12345|COMPLETED|0:0|1024K|00:05:30|c1
```

**Gotcha**: sacct returns job steps (e.g., `12345.batch`, `12345.0`).
Filter to main job only by skipping entries containing `.` in the JobID.

### SLURM Job States

| State | Terminal? | OxyMake Mapping |
|-------|-----------|-----------------|
| PENDING | No | Queued |
| RUNNING | No | Running |
| COMPLETING | No | Running |
| COMPLETED | Yes | Completed |
| FAILED | Yes | Failed |
| TIMEOUT | Yes | Failed("exceeded time limit") |
| OUT_OF_MEMORY | Yes | Failed("OOM") |
| CANCELLED | Yes | Cancelled |
| PREEMPTED | Yes | Cancelled |
| NODE_FAIL | Yes | Failed + exclude node |

## Architecture Patterns from Major Tools

### Pattern 1: Scheduler-Managed DAG (Recommended)

**Used by**: Snakemake, Nextflow, Cromwell, OxyMake

The workflow engine manages the DAG and only submits jobs whose upstream
dependencies are satisfied. Do NOT use SLURM's `--dependency=afterok:JOBID`.

**Why**: SLURM dependencies are fragile — if a dependency job fails, all
downstream jobs are cancelled automatically, bypassing the workflow engine's
error handling logic. The engine loses control.

### Pattern 2: Adaptive Polling (Recommended)

**Used by**: Snakemake

```
Initial interval: 5s (configurable)
Backoff factor: 1.5x
Maximum interval: 60s (configurable)
Reset trigger: Any job state change
Batch queries: sacct -j id1,id2,...,idN
```

Critical at scale: individual `sacct` calls per job will overload `slurmctld`
beyond ~100 concurrent jobs. Always batch job IDs into a single query.

### Pattern 3: sacct Primary + squeue Fallback

**Used by**: Snakemake

1. Try `sacct` first (provides completed job info with metrics)
2. If `sacct` fails or returns empty, fall back to `squeue`
3. If neither has the job, wait 2s and retry sacct (race condition window)
4. If still missing, report as lost

**Why the fallback**: Some clusters don't have `slurmdbd` (SLURM accounting
daemon) configured, making `sacct` unavailable.

### Pattern 4: Failed Node Exclusion

**Used by**: Snakemake

When a job reports `NODE_FAIL`:
1. Query `sacct -j <id> -n -X -o nodelist%-256` to identify the failed node
2. Add to an in-memory exclusion set
3. Pass `--exclude=node1,node2` on all future `sbatch` submissions
4. Report excluded nodes at workflow completion

### Pattern 5: Rate Limiting

**Used by**: Nextflow

HPC centers explicitly recommend rate limiting:
```
NIH Biowulf:  submitRateLimit = '6/1min'
Yale YCRC:    submitRateLimit = '190/60min'
```

OxyMake implements this via `max_submit` (concurrent job cap) and the
scheduler's semaphore-based concurrency control.

## Key Constraints for OxyMake

### 1. State.db Locality (ADR-004)

`.oxymake/state.db` uses SQLite WAL mode, which does NOT work on network
filesystems. The scheduling process (`ox run`) must run on a node with
local disk. Compute nodes never access `state.db`.

### 2. Shared Filesystem Required

Job scripts, input data, and output data must be on a filesystem visible
to both the scheduling node and compute nodes (NFS, Lustre, GPFS).

### 3. Docker Unavailable on HPC

Most HPC clusters prohibit Docker (requires root). Use Apptainer
(formerly Singularity) instead. OxyMake should warn when Docker is
specified with the SLURM executor.

### 4. Module System

HPC clusters use `module load` for software management. SLURM job scripts
should use `module load conda` (or similar) rather than assuming software
is on PATH.

## Resource Mapping Reference

| OxyMake | SLURM | Notes |
|---------|-------|-------|
| `cpu` | `--cpus-per-task` | Per-task CPU cores |
| `mem` | `--mem` | Total memory per node |
| `mem_per_cpu` | `--mem-per-cpu` | Memory per CPU core |
| `gpu` | `--gpus` | GPU count |
| `nodes` | `--nodes` | Node count |
| `tasks` | `--ntasks` | MPI task count |
| `partition` | `--partition` | SLURM partition |
| `time` | `--time` | Wall time limit |
| `qos` | `--qos` | Quality of Service |

**Mutual exclusion**: `--mem` and `--mem-per-cpu` cannot both be specified.

## Testing Infrastructure

### Mock SLURM Scripts

Located in `tests/mock-slurm/`. Stateful mocks using a shared directory
(`$SLURM_MOCK_DIR`) to track job state transitions.

Prepend to PATH in tests: `PATH="tests/mock-slurm:$PATH"`

### Docker SLURM Cluster

Located in `docker/slurm-test-cluster/`. Uses `giovtorres/docker-centos7-slurm`
with slurmctld + 2 compute nodes + MariaDB accounting.

Start: `docker compose up -d` (wait ~20s for readiness)
Gate: `#[cfg(feature = "slurm-integration")]`

## Common Pitfalls

1. **Polling too aggressively**: Default 1s polls will get you banned from HPC
   clusters. Use adaptive backoff starting at 5s minimum.

2. **Forgetting --parsable**: Without `--parsable`, sbatch output format varies
   by SLURM version and locale.

3. **Job name length**: SLURM limits job names to 255 characters. Truncate
   rule names + wildcards.

4. **sacct field widths**: Default field widths truncate output. Always use
   `--parsable2` or specify widths like `nodelist%-256`.

5. **Exit code format**: sacct returns `exitcode:signal` (e.g., `0:0`, `1:0`,
   `0:9`). Parse only the first number for the exit code.

6. **Job step filtering**: sacct returns entries for job steps (`.batch`, `.0`).
   Filter to the main job entry only (no `.` in JobID).
