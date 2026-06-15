# Docker SLURM Shims

Transparent filesystem bridge for `ox run --executor slurm` against a
Docker-based SLURM cluster.

## How it works

The shim scripts in this directory proxy SLURM CLI commands (`sbatch`, `sacct`,
`squeue`, `scancel`, `sinfo`) to the Docker container via `docker exec`. The
`docker-compose.yml` mounts host directories at identical paths inside the
container, so job scripts that reference host absolute paths work without
rewriting.

## Usage

```bash
# 1. Set environment variables for volume mounts
export OXYMAKE_PROJECT_DIR="$(pwd)"          # Your project directory
export OXYMAKE_STAGING_DIR="/tmp/oxymake-slurm"  # Staging (default)

# 2. Start the Docker SLURM cluster
cd tests/slurm-docker
docker compose up -d

# 3. Wait for the cluster to be ready
docker exec slurmctld sinfo

# 4. Put the shims on PATH (BEFORE any real SLURM install)
export PATH="$(pwd)/bin:$PATH"

# 5. Run ox with the SLURM executor from your project directory
cd "$OXYMAKE_PROJECT_DIR"
ox run --executor slurm -j 4

# 6. Tear down when done
cd tests/slurm-docker
docker compose down -v
```

## Environment variables

| Variable | Default | Description |
|---|---|---|
| `SLURM_DOCKER_CONTAINER` | `slurmctld` | Docker container name for SLURM controller |
| `OXYMAKE_PROJECT_DIR` | `/tmp/oxymake-project` | Host project directory (bind-mounted into containers) |
| `OXYMAKE_STAGING_DIR` | `/tmp/oxymake-slurm` | Host staging directory for job scripts |

## Architecture

```
Host                          Docker Container
────                          ────────────────
ox run --executor slurm
  │
  ├─ writes job.sh to /tmp/oxymake-slurm/...
  │                           │ (same path via bind mount)
  ├─ calls sbatch job.sh ─────┤
  │   (shim)                  ├─ docker exec slurmctld sbatch job.sh
  │                           │
  │                           ├─ job runs, reads inputs from project dir
  │                           │  (same path via bind mount)
  │                           ├─ job writes outputs to project dir
  │                           │
  ├─ calls sacct -j <id> ─────┤
  │   (shim)                  └─ docker exec slurmctld sacct -j <id>
  │
  └─ outputs appear at host project dir paths
```
