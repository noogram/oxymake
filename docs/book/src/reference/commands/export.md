# ox export

Export an Oxymakefile to another workflow format.

## Usage

```bash
ox export <FORMAT> [OPTIONS]
```

## Formats

| Format | Description |
|--------|-------------|
| `snakemake` | Export to Snakemake format (Snakefile + config.yaml) |

## Options

| Flag | Description |
|------|-------------|
| `-f, --file <FILE>` | Path to the Oxymakefile (default: `Oxymakefile.toml`) |
| `-o, --output <FILE>` | Write output to a file instead of stdout |

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
- [Oxymakefile Format](../format.md) -- the OxyMake workflow format
