# ox import

Adopt outputs produced in another workflow tree as verified local cache
entries.

## Usage

```bash
ox import <MANIFEST> [-f <OXYMAKEFILE>]
```

Place every output named by the manifest at the same workflow-relative path,
then run `ox import`. OxyMake re-hashes every output before changing
`.oxymake/cache/cache.db`; one mismatch rejects the entire manifest. It also
checks that the local rule's job specification, platform scope, output set, and
reproducibility policy agree with the manifest.

```bash
# After copying counts.ox-cache.json and build/counts.parquet:
ox import counts.ox-cache.json
ox run build/model.rds
```

Imported outputs whose producer inputs are absent become trusted resolver
leaves. Downstream jobs can therefore run without copying the raw inputs or
maintaining a second, pruned Oxymakefile.

An `exact` entry can only be imported on its producing OS/architecture. Rules
with `cache_platform = "any"` may cross that boundary. Non-reproducible rules
are always refused.

The manifest format is versioned. Unknown versions are rejected rather than
being interpreted approximately.

## See also

- [ox export](./export.md) -- create an adoption manifest
- [Content-addressable cache](../../concepts/cache.md)
