# Environments

Real-world workflows need specific software packages, library versions, and
runtime configurations. OxyMake supports multiple **environment backends**
that isolate each rule's execution in a reproducible environment.

## Declaring an Environment

Add an `environment` field to any rule:

```toml
[rule.analyze]
input = ["data/{sample}.csv"]
output = ["results/{sample}.json"]
lang = "python"
environment = { uv = "pyproject.toml" }
run = """
import pandas as pd
df = pd.read_csv("{input}")
df.describe().to_json("{output}")
"""
```

The environment is resolved at execution time. OxyMake ensures the
environment is set up before the rule runs.

## Supported Backends

### uv (Python)

The recommended backend for Python workflows. Uses
[uv](https://docs.astral.sh/uv/) to create and manage virtual environments
from a `pyproject.toml` or `requirements.txt`.

```toml
environment = { uv = "pyproject.toml" }
```

OxyMake calls `uv sync` to ensure the environment matches the lockfile.
The environment hash (from `uv.lock`) is included in the cache key, so
changing a dependency invalidates affected outputs.

### conda

For workflows that need non-Python packages (C libraries, R, etc.):

```toml
environment = { conda = "environment.yaml" }
```

OxyMake creates or updates a conda environment from the YAML specification.

### Docker / OCI Containers

For maximum isolation and reproducibility:

```toml
environment = { docker = "python:3.11-slim" }
```

The job runs inside a container. OxyMake mounts the workspace and handles
input/output file staging. The image digest is included in the cache key.

### Nix

For fully reproducible builds with Nix:

```toml
environment = { nix = "flake.nix#devShell" }
```

### Apptainer (Singularity)

For HPC environments where Docker is unavailable:

```toml
environment = { apptainer = "image.sif" }
```

### System (default)

No isolation. Uses whatever Python/R/tools are on `$PATH`:

```toml
environment = { system = true }
```

This is the default when no `environment` is specified. Suitable for
`shell`-mode rules that call system utilities.

## How Isolation Works

Each environment backend follows the same lifecycle:

1. **Resolve**: Determine the exact environment specification (lockfile
   hash, image digest, flake hash)
2. **Prepare**: Create or update the environment if needed (`uv sync`,
   `docker pull`, `conda env create`)
3. **Execute**: Run the job inside the environment
4. **Hash**: Include the environment specification hash in the cache key

The key insight is step 4: the environment specification is part of the
cache key. If you update a dependency in `pyproject.toml` and the lockfile
changes, all rules using that environment will be recomputed.

## Mixing Environments

Different rules can use different environments in the same workflow:

```toml
[rule.download]
environment = { system = true }
shell = "wget {url} -O {output}"

[rule.analyze]
environment = { uv = "pyproject.toml" }
call = "analysis:run"
lang = "python"

[rule.visualize]
environment = { conda = "envs/plotting.yaml" }
script = "scripts/plot.R"
```

OxyMake manages each environment independently. There is no requirement
that all rules share the same environment.

## Environment and Executors

| Executor | Environment handling |
|----------|---------------------|
| Local | Environment resolved on the local machine |
| SLURM | Environment must be available on compute nodes |
| K8s | Docker image used as the pod container |
| Ray | Environment resolved on Ray worker nodes |

For SLURM, ensure that conda environments or uv projects are accessible
from the compute nodes (e.g., on a shared filesystem).

## Environment Caching

Environment setup can be slow (minutes for large conda environments).
OxyMake caches the prepared environment and only re-creates it when the
specification changes:

- **uv**: Rebuilds when `uv.lock` changes
- **conda**: Rebuilds when `environment.yaml` changes
- **Docker**: Re-pulls when the image tag resolves to a new digest
- **Nix**: Rebuilds when the flake lock changes

This means the first run may be slow (environment setup), but subsequent
runs reuse the prepared environment instantly.
