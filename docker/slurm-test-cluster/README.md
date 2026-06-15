# SLURM Test Cluster

Docker-based SLURM cluster for OxyMake integration tests.

## Quick Start

```bash
docker compose up -d
# Wait ~20 seconds for services to initialize
docker compose exec slurmctld sinfo -N -h
# Should show: c1 idle, c2 idle
```

## Submit a Test Job

```bash
docker compose exec slurmctld bash -c '
  echo "#!/bin/bash
hostname
date
sleep 2
echo done" > /shared/test.sh && sbatch /shared/test.sh'
# Output: Submitted batch job 1

# Check status:
docker compose exec slurmctld sacct --parsable2 --noheader -o JobID,State,ExitCode
```

## Architecture

| Service | Role | Port |
|---------|------|------|
| `mysql` | MariaDB for SLURM accounting | 3306 |
| `slurmdbd` | SLURM database daemon | 6819 |
| `slurmctld` | SLURM controller | 6817 |
| `c1`, `c2` | Compute nodes | — |

All containers share Munge authentication keys and SLURM config via named volumes.
The `/shared` volume is mounted on all nodes, simulating a shared filesystem.

## CI Integration

```yaml
# GitHub Actions example
- name: Start SLURM cluster
  run: |
    cd docker/slurm-test-cluster
    docker compose up -d
    sleep 20  # Wait for cluster readiness
    docker compose exec -T slurmctld sinfo -N -h

- name: Run integration tests
  run: cargo test --workspace --features slurm-integration

- name: Tear down
  if: always()
  run: docker compose -f docker/slurm-test-cluster/docker-compose.yml down -v
```

## Teardown

```bash
docker compose down -v  # Remove containers and volumes
```
