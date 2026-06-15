# ox translate

Translate a Snakefile into OxyMake TOML.

The `ox translate` command parses a Snakemake `Snakefile` and emits an
equivalent `Oxymakefile.toml`. Use it to migrate existing Snakemake workflows
to OxyMake without rewriting rules by hand.

## Usage

```bash
ox translate Snakefile                       # Writes Snakefile.translated.toml
ox translate Snakefile -o Oxymakefile.toml   # Writes a custom path
```

When `-o` is omitted, the translator writes two files next to the input:

- `<INPUT>.translated.toml` — the generated Oxymakefile
- `<INPUT>.translated.toml.escalations.toml` — written only when the IR
  contains escalations

Every run emits a one-line summary to stderr:

```
translated: N rules (X mechanical, Y with escalations); dropped: Z unsupported top-level constructs; includes: K files NOT followed
```

`ox translate` exits with status `2` when escalations were recorded so CI
or shell scripts can gate on a clean translation. The files are still
written; only the exit code changes.

## Arguments

| Argument | Description |
|----------|-------------|
| `<SNAKEFILE>` | Path to the Snakefile to translate |

## Options

| Flag | Description |
|------|-------------|
| `-o, --output <OUTPUT>` | Write the translated TOML to this path instead of the default `<INPUT>.translated.toml`. The escalation file lands at `<OUTPUT>.escalations.toml`. |

## Translation Notes

The translator handles the most common Snakemake patterns:

- `rule` blocks → `[[rule]]` sections
- `input` / `output` → `inputs` / `outputs`
- `expand()` calls → OxyMake wildcard `{sample}` syntax
- `params` → `[rule.params]`
- `shell` → `command`

Complex Python logic inside Snakefiles (e.g., `run:` blocks, conditional
inputs, `lambda` wildcards) may require manual adjustment after translation.
Review the generated TOML and run `ox lint` to verify.

## Examples

```bash
# Quick migration — produces Snakefile.translated.toml
ox translate Snakefile
ox lint -f Snakefile.translated.toml   # Verify the result
ox plan -f Snakefile.translated.toml   # Check execution plan

# Custom output path
ox translate Snakefile -o Oxymakefile.toml

# CI gate: fail the job when escalations were emitted
ox translate Snakefile || echo "needs manual review"
```

## See Also

- [Oxymakefile Format](../format.md) — full TOML reference
- [ox lint](../commands.md#ox-lint) — validate the generated file
