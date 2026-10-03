# ox migrate

```sh
ox migrate --to-format 2 [-f Oxymakefile.toml] [--write]
```

The default is a report: no workflow files are changed. The report names every
file and old/new version declaration. `--write` applies the complete checked
include graph. Only destination format `2` is supported.

Absent `format_version` means legacy schema 1. The known generated `ox_version`
markers `0.1` and `0.3` become `>=0.7.0`, the release introducing schema 2. All
other legacy declarations, including a missing declaration, require manual
resolution: the tool cannot infer what binary the author intended. Valid
schema-2 files can coexist with legacy files and remain unchanged.

Migration preserves command bytes, comments and software environments, but may
normalize document formatting, including line endings. It does not extract
shell assignments. Unknown keys (including future features), unsupported
schemas, include cycles, missing files, invalid merged workflows and
unsatisfied requirements cause refusal before writing any original.

All replacements and recovery copies are staged before writing. A detected
concurrent edit or staging error leaves originals intact; a replacement error
rolls back files already replaced. This is not a crash-atomic filesystem
transaction. If the filesystem prevents rollback, the error names the retained
recovery copies. Keep normal version-control backups and avoid editing the graph
concurrently with migration.

Migration preserves cache identity. Any future change to execution semantics
will state its own cache consequences in that release's notes. A minimum binary
requirement is a capability floor, not a reproducible build guarantee.
