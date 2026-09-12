# ox cache-import

Adopt outputs produced in another workflow tree as verified local cache
entries.

## Usage

```bash
ox cache-import <MANIFEST> [-f <OXYMAKEFILE>]
```

Place every output named by the manifest at the same workflow-relative path,
then run `ox cache-import`. OxyMake re-hashes every output before changing
`.oxymake/cache/cache.db`; one mismatch rejects the entire manifest. It also
checks that the local rule's job specification, platform scope, output set, and
reproducibility policy agree with the manifest.

```bash
# After copying counts.ox-cache.json and build/counts.parquet:
ox cache-import counts.ox-cache.json
ox run build/model.rds
```

Imported outputs whose producer inputs are absent become trusted resolver
leaves. Downstream jobs can therefore run without copying the raw inputs or
maintaining a second, pruned Oxymakefile.

An `exact` entry can only be imported on its producing OS/architecture. Rules
with `cache_platform = "any"` may cross that boundary. Non-reproducible rules
are always refused.

The manifest has a fixed kind and a versioned format. Newer unsupported
versions are rejected before their body is decoded, with both producer and
consumer `ox` versions and an upgrade remedy in the error.

## See also

- [ox cache-export](./cache-export.md) -- create an adoption manifest
- [Content-addressable cache](../../concepts/cache.md)
