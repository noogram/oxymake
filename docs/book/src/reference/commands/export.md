# ox export

Export cached outputs for adoption on another machine, or translate an
Oxymakefile to another workflow format.

## Usage

```bash
ox export <FORMAT> [OPTIONS]
ox export <TARGET>... --manifest <PATH>
```

## Output-adoption manifests

`--manifest` writes a versioned JSON manifest for cached targets. Each entry
contains every output path and BLAKE3 hash, the input hashes and job-spec hash,
the rule's reproducibility class, its platform scope, and the producing
platform. The output files themselves are not copied; transport them separately
and preserve their workflow-relative paths.

```bash
ox run build/counts.parquet
ox export build/counts.parquet --manifest counts.ox-cache.json
```

Only completed targets with current cache provenance can be exported. Rebuild
an older target first if its cache entry predates provenance recording.

## Formats

| Format | Description |
|--------|-------------|
| `snakemake` | Export to Snakemake format (Snakefile + config.yaml) |

## Options

| Flag | Description |
|------|-------------|
| `-f, --file <FILE>` | Path to the Oxymakefile (default: `Oxymakefile.toml`) |
| `-o, --output <FILE>` | Write output to a file instead of stdout |
| `--manifest <PATH>` | Write an output-adoption manifest instead of translating |

## Examples

```bash
# Export to stdout
ox export snakemake

# Export to file
ox export snakemake -o Snakefile

# Export a specific Oxymakefile
ox export snakemake -f pipelines/Oxymakefile.toml -o Snakefile
```

## Bidirectional Translation

OxyMake supports bidirectional Snakemake translation:

- **Import**: `ox translate Snakefile` converts Snakemake to OxyMake TOML
- **Export**: `ox export snakemake` converts OxyMake TOML back to Snakemake

This enables zero-friction migration in both directions.

## See Also

- [ox translate](./translate.md) -- import from Snakemake
- [ox import](./import.md) -- adopt outputs from another machine
- [Oxymakefile Format](../format.md) -- the OxyMake workflow format
