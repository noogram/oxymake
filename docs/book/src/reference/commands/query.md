# ox query

Query the dependency graph using Bazel-style expressions.

## Usage

```bash
ox query <EXPRESSION> [OPTIONS]
```

## Expressions

| Expression | Description |
|-----------|-------------|
| `deps(X)` | All transitive dependencies of target X |
| `rdeps(X)` | All targets that transitively depend on X |
| `allpaths(X, Y)` | All paths from X to Y in the DAG |

## Options

| Flag | Description |
|------|-------------|
| `--json` | Output JSON instead of human-readable text |
| `-f, --file <FILE>` | Oxymakefile path (default: `Oxymakefile.toml`) |

## Examples

```bash
# What does annotate depend on?
ox query 'deps(annotate)'

# What depends on the data rule? (reverse dependencies)
ox query 'rdeps(data)'

# All paths from data to annotate
ox query 'allpaths(data, annotate)'

# JSON output for programmatic use
ox query 'deps(annotate)' --json
```

## See Also

- [ox dag](../../concepts/three-graphs.md) -- visualize the DAG
- [ox plan](../commands.md) -- show the execution plan
