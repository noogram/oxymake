# ox lock

Generate or verify a reproducibility lockfile.

The `ox lock` command captures a cryptographic snapshot of the entire workflow
— rule definitions, config values, input hashes — into an `ox.lock` file. Use
it to detect unintended changes between runs or across machines.

## Subcommands

### `ox lock generate`

Generate an `ox.lock` file from the current workflow state.

```bash
ox lock generate                        # Write ox.lock next to Oxymakefile.toml
ox lock generate -o locks/my.lock       # Write to a custom path
ox lock generate -f path/Oxymakefile.toml
```

**Options:**

| Flag | Description |
|------|-------------|
| `-f, --file <FILE>` | Oxymakefile path (default: `Oxymakefile.toml`) |
| `-o, --output <OUTPUT>` | Output lockfile path (default: `ox.lock` next to the Oxymakefile) |

### `ox lock verify`

Verify the current state against an existing `ox.lock`.

```bash
ox lock verify                          # Verify against ox.lock
ox lock verify -l locks/my.lock         # Verify against a custom lockfile
```

**Options:**

| Flag | Description |
|------|-------------|
| `-f, --file <FILE>` | Oxymakefile path (default: `Oxymakefile.toml`) |
| `-l, --lockfile <LOCKFILE>` | Lockfile path (default: `ox.lock` next to the Oxymakefile) |

**Exit codes:**
- `0` — Lock matches current state
- `1` — Mismatch detected (details printed to stderr)

## Examples

```bash
# Pin the workflow before a release
ox lock generate
git add ox.lock && git commit -m "lock: pin workflow v2.1"

# CI: verify nothing drifted
ox lock verify || { echo "Workflow changed since lock!"; exit 1; }
```

## See Also

- [CLI Commands](../commands.md) — full command index
- [Content-Addressable Cache](../../concepts/cache.md) — how OxyMake tracks state
