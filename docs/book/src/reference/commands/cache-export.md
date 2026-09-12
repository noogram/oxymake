# ox cache-export

Describe cached outputs so another machine can adopt them.

## Usage

```bash
ox cache-export <TARGET>... [-o <PATH>]
```

The command writes a versioned JSON manifest containing every output path and
BLAKE3 hash, input hashes, job-spec hash, reproducibility class, platform scope,
producing platform, manifest kind, and producing `ox` version. Output files are
not copied; transport them separately and preserve their workflow-relative
paths.

```bash
ox run build/counts.parquet
ox cache-export build/counts.parquet -o counts.ox-cache.json
```

Without `-o`, the manifest is written to stdout. There is no default manifest
filename. Only completed targets with current cache provenance can be exported;
rebuild an older target first if necessary.

## See also

- [ox cache-import](./cache-import.md) -- adopt the described outputs
- [ox export](./export.md) -- translate an Oxymakefile to Snakemake or WDL
