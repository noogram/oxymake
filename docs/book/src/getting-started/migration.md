# Migrating to schema 2

Schema 2 makes unsupported declarations fail visibly. It requires a minimum
capable OxyMake binary in every schema-2 file, and catches structural typos.
Legacy files keep working, with a warning that their binary requirement is not
enforced.

Start with a review:

```sh
ox migrate --to-format 2 -f Oxymakefile.toml
```

The report covers all included files. Generated markers `0.1` and `0.3` become
`>=0.7.0`, because that is the release introducing this contract. Commands and
software environments retain their meaning. If the tool cannot justify a
requirement or finds unknown fields, resolve those declarations yourself and
run the preview again. Do not remove a field until you understand its intent.

Apply the resolved migration explicitly, then inspect the changes:

```sh
ox migrate --to-format 2 -f Oxymakefile.toml --write
git diff
ox lint -f Oxymakefile.toml
```

Use `environment = { uv = "requirements.txt" }` for the software backend.
There is no variables table yet, and rule `env` is rejected in schema 2. Keep
any existing shell assignments in the command. Schema 2 supports
[named resource classes](../reference/format.md#named-resource-classes-schema-2).
Migrate each file that declares or references a class: schema 1 ignores those
fields and warns. Overrides preserve the class spelling, so rule `cpus = 6`
over class `cpu = 2` expands both `{threads}` and `{resources.cpu}` to `6`.
Resource exports remain future work.

Migration preserves cache identity. Any future change to execution semantics
will state its own cache consequences in that release's notes. Command bytes are
preserved, but document formatting, including line endings, may be normalized.
Older binaries that predate version enforcement cannot be made to reject a
future file retroactively; ensure the installed binary supports schema 2.
