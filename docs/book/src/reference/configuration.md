# Configuration

OxyMake uses a layered configuration system. Workflow-level settings live in
`Oxymakefile.toml`, and project-level settings live in `.oxymake/config.toml`.

## Workflow Configuration

The `[config]` section in `Oxymakefile.toml` defines variables for wildcard
expansion:

```toml
[config]
samples = ["A", "B", "C"]
models = ["linear", "ridge"]
```

These values drive wildcard resolution in rules.

## Project Settings

The `.oxymake/config.toml` file (created by `ox init`) stores project-level
defaults:

```toml
[defaults]
jobs = 4                    # Default -j value
executor = "local"          # Default executor
materialize = "always"      # Default materialization policy

[cache]
dir = ".oxymake/cache"      # Cache directory location
max_size_gb = 10            # Maximum cache size

[state]
dir = ".oxymake"            # State directory
```

## Environment Variables

OxyMake respects the following environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `OXYMAKE_JOBS` | Default parallelism | 1 |
| `OXYMAKE_EXECUTOR` | Default executor | `local` |
| `OXYMAKE_CACHE_DIR` | Cache directory | `.oxymake/cache` |
| `OXYMAKE_LOG` | Log level | `warn` |
| `OX_CACHE_VALIDATION` | Cache validation strategy (`mtime`, `mtime+hash`, `hash`) | `mtime+hash` |

## Configuration Precedence

Settings are resolved in order (later overrides earlier):

1. Built-in defaults
2. User global config (`~/.config/oxymake/config.toml`)
3. `.oxymake/config.toml`
4. Environment variables
5. Command-line flags

## State Directory

The `.oxymake/` directory contains:

```
.oxymake/
  state.db          # SQLite execution state + audit log
  cache/            # Content-addressable output cache
  config.toml       # Project settings
```

The state database (`state.db`) uses SQLite WAL mode for concurrent access.
It must reside on local disk (not NFS/Lustre/GPFS).

## Next Steps

- [Oxymakefile Format](./format.md) -- workflow definition reference
- [CLI Commands](./commands.md) -- command reference
- [Expression Language](./expressions.md) -- expression syntax
