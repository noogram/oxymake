# Expression Language

OxyMake includes a minimal expression language for conditional guards and
dynamic values in workflow definitions. The language is deliberately limited:
pure functions, no loops, no side effects.

## Guard Expressions

The `when` field on a rule accepts a boolean expression:

```toml
[rule.expensive_model]
output = ["results/{seed}_{model}.txt"]
shell = "train --seed {seed} --model {model}"
when = "seed in @selected_seeds"
```

If the guard evaluates to `false`, the job is not created in the DAG.

## Supported Operators

### Membership

```toml
when = "sample in @high_priority_samples"   # Check if wildcard is in a config list
when = "model in ['linear', 'ridge']"       # Check against inline list
```

### Comparison

```toml
when = "wildcards.threshold >= 0.5"
when = "wildcards.replicate != 'control'"
```

### Logical

```toml
when = "sample in @fast_samples and model == 'linear'"
when = "not (sample in @excluded)"
```

## Variable References

### Wildcards

Access wildcard values with bare names or the `wildcards.` prefix:

```toml
shell = "process {sample}"                   # Bare wildcard in commands
when = "wildcards.sample in @selected"       # Explicit prefix in guards
```

### Config References

Reference config arrays with `@`:

```toml
when = "sample in @priority_samples"         # @name refers to config.name
```

### Built-in Variables

| Variable | Description |
|----------|-------------|
| `{input}` | Resolved input path(s) |
| `{output}` | Resolved output path(s) |
| `{wildcards.NAME}` | Resolved wildcard value |
| `{params.NAME}` | Rule parameter value |
| `{rule}` | Rule name |

## String Interpolation

In `shell`, `run`, and `script` fields, `{braces}` perform string
interpolation:

```toml
shell = "python process.py --input {input} --output {output} --sample {wildcards.sample}"
```

Double braces `{{` and `}}` produce literal braces (useful in Python code):

```toml
run = """
result = {{"key": "value"}}
"""
```

## Design Philosophy

The expression language is intentionally not Turing-complete. Complex
configuration logic should happen outside the Oxymakefile:

```bash
python gen_config.py > config.toml     # Generate config externally
ox run --config config.toml            # Use generated config
```

This preserves static parseability: any tool can read an Oxymakefile without
executing code.

## Next Steps

- [Oxymakefile Format](./format.md) -- complete format reference
- [Rules and Wildcards](../concepts/rules-and-wildcards.md) -- wildcard patterns
- [Tags and Filtering](../concepts/tags.md) -- tag-based job selection
