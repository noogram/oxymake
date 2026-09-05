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

## Named Profiles

Per-run defaults live in the Oxymakefile, as `[profile.NAME]` sections selected
with `ox run --profile NAME`:

```toml
[profile.cluster]
jobs = 32
executor = "slurm"
partition = "gpu"
```

There is no project-level settings file. `ox init` creates `Oxymakefile.toml`
and an empty `.oxymake/` directory; it does not write a `config.toml`, and no
`.oxymake/config.toml` is read if you create one by hand.

## User Global Config

`$XDG_CONFIG_HOME/oxymake/config.toml` (or `~/.config/oxymake/config.toml`) is
read for exactly two top-level keys:

```toml
cache_validation = "hash"   # default cache validation strategy
open_dashboard = true       # open the web dashboard on `ox run`
```

## Environment Variables

OxyMake reads the following environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `OX_CACHE_VALIDATION` | Cache validation strategy (`mtime`, `mtime+hash`, `hash`) | `mtime+hash` |
| `OX_SHUTDOWN_GRACE_SECS` | Seconds a cancelled job may take to exit after `SIGTERM` before `ox run` sends `SIGKILL` to its process group. `0` escalates immediately. | `10` |

Everything else is set with a command-line flag or in the Oxymakefile.

## Configuration Precedence

`cache_validation` — the one setting with a full resolution chain — is resolved
in order (later overrides earlier):

1. Built-in default (`mtime+hash`)
2. User global config (`~/.config/oxymake/config.toml`)
3. `cache_validation` under `[config]` in the Oxymakefile
4. `OX_CACHE_VALIDATION`
5. `--cache-validation` on the command line

## State Directory

The `.oxymake/` directory contains:

```
.oxymake/
  state.db          # SQLite execution state + audit log
  cache/            # Content-addressable output cache
  logs/             # Per-rule stdout/stderr
  events/           # NDJSON run event streams
```

`.oxymake/` is created in the directory `ox` is invoked from. Its location is
not configurable: no flag, config key, or environment variable relocates it.

The state database (`state.db`) uses SQLite WAL mode for concurrent access, and
must reside on local disk (not NFS/Lustre/GPFS). Because a rule's inputs and
outputs are also resolved relative to the working directory, satisfying that
requirement means running the whole workspace from local disk — the database
cannot be kept local while the project tree lives on shared storage.

## Next Steps

- [Oxymakefile Format](./format.md) -- workflow definition reference
- [CLI Commands](./commands.md) -- command reference
- [Expression Language](./expressions.md) -- expression syntax
