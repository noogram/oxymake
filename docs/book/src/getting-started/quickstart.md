# Quickstart

Get up and running with OxyMake in under five minutes. This guide covers
only features that are tested and working in v0.1.0.

## Install

Build and install from source (Rust 1.85+ required):

```bash
git clone https://github.com/noogram/oxymake.git
cd oxymake
cargo install --path crates/ox-cli
```

This installs both `ox` and `oxymake` to `~/.cargo/bin/`.

Verify:

```bash
ox --version
# ox 0.1.0
```

## Create a Project

```bash
mkdir my-pipeline
cd my-pipeline
ox init
```

This creates a starter `Oxymakefile.toml` and a `.oxymake/` directory.

> The generated template uses `{input}` and `{output}` placeholders
> for input/output file expansion, plus `{config.key}` for config substitution.

## Your First Workflow

Create the Oxymakefile:

```bash
cat > Oxymakefile.toml << 'EOF'
ox_version = "0.1"

[config]
samples = ["A", "B"]

# Default target: require all results to exist.
[rule.all]
input = ["results/{sample}.txt"]

# Process each sample's CSV into a sorted text file.
[rule.process]
input = ["data/{sample}.csv"]
output = ["results/{sample}.txt"]
shell = "sort data/{sample}.csv > results/{sample}.txt"
EOF
```

Key concepts:
- **`[config]`** defines variables. Here `samples = ["A", "B"]` means OxyMake
  will create one job per sample.
- **`{sample}`** in paths and shell commands is replaced with each value from
  the config list.
- **`[rule.all]`** is the default target. It has inputs but no outputs, so it
  just ensures its inputs exist.
- Use explicit paths with config variables in `shell` commands (e.g.,
  `data/{sample}.csv`), not `{input}`/`{output}`.

Create some input data:

```bash
mkdir -p data results
echo -e "charlie,3\nalpha,1\nbravo,2" > data/A.csv
echo -e "zulu,26\nmike,13" > data/B.csv
```

## Validate

Check your Oxymakefile for errors:

```bash
ox lint
# Oxymakefile is valid (2 rules)
```

## Preview (Dry Run)

See what OxyMake would do without running anything:

```bash
ox run --dry-run
```

Output:
```
Dry run: 2 job(s) would execute for 2 target(s)
  [process-B] rule=process outputs=[results/B.txt]
  [process-A] rule=process outputs=[results/A.txt]
```

## Run

Execute the workflow:

```bash
ox run
```

Output:
```
Completed: 2 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)
```

Check the results:

```bash
cat results/A.txt
# alpha,1
# bravo,2
# charlie,3
```

## Caching

Run the same command again:

```bash
ox run
```

Output:
```
Cache: 2 of 2 job(s) up-to-date, skipping.
Completed: 0 succeeded, 0 failed, 2 skipped, 0 cancelled (0.0s)
```

Nothing ran. OxyMake detected that all inputs are unchanged and all outputs
exist. Modify an input and re-run to see only the affected jobs execute.

## Build a Specific Target

Build only one output:

```bash
rm results/A.txt
ox run results/A.txt
```

Only `process-A` runs. `results/B.txt` is untouched.

## Multi-Step Pipeline

OxyMake resolves dependency chains automatically. Here is a two-step
pipeline that uppercases text, then counts characters:

```bash
mkdir pipeline && cd pipeline

cat > Oxymakefile.toml << 'EOF'
ox_version = "0.1"

[config]
names = ["alice", "bob"]

[rule.all]
input = ["final/{name}.txt"]

[rule.uppercase]
input = ["raw/{name}.txt"]
output = ["mid/{name}.txt"]
shell = "tr '[:lower:]' '[:upper:]' < raw/{name}.txt > mid/{name}.txt"

[rule.count]
input = ["mid/{name}.txt"]
output = ["final/{name}.txt"]
shell = "wc -c < mid/{name}.txt > final/{name}.txt"
EOF

mkdir -p raw mid final
echo "hello world" > raw/alice.txt
echo "oxymake rocks" > raw/bob.txt

ox run --dry-run
# 4 jobs: uppercase-alice, uppercase-bob, count-alice, count-bob

ox run
# Completed: 4 succeeded, 0 failed, 0 skipped, 0 cancelled (0.0s)

cat final/alice.txt
# 12
```

OxyMake figures out that `count` depends on `uppercase` and runs them in the
correct order.

## Error Handling

If a job fails, OxyMake stops and reports the failure:

```bash
cat > Oxymakefile.toml << 'EOF'
ox_version = "0.1"
[rule.broken]
output = ["out.txt"]
shell = "exit 1"
EOF

ox run
# error: job broken failed: exit code 1
# Completed: 0 succeeded, 1 failed, 0 skipped, 0 cancelled (0.0s)
# Exit code: 1
```

Use `--keep-going` (or `-k`) to continue running independent jobs even when
one fails:

```bash
cat > Oxymakefile.toml << 'EOF'
ox_version = "0.1"
[config]
items = ["ok", "fail"]

[rule.all]
input = ["out/{item}.txt"]

[rule.process]
input = ["in/{item}.txt"]
output = ["out/{item}.txt"]
shell = "if [ '{item}' = 'fail' ]; then exit 1; fi; cp in/{item}.txt out/{item}.txt"
EOF

mkdir -p in out
echo "good" > in/ok.txt
echo "bad" > in/fail.txt

ox run -k
# Completed: 1 succeeded, 1 failed, 0 skipped, 0 cancelled (0.0s)
# Exit code: 1
# out/ok.txt was created; out/fail.txt was not
```

## Static Rules

Rules without config variables produce a single job:

```bash
cat > Oxymakefile.toml << 'EOF'
ox_version = "0.1"
[rule.greet]
output = ["greeting.txt"]
shell = "echo 'Hello OxyMake' > greeting.txt"
EOF

ox run
cat greeting.txt
# Hello OxyMake
```

## Alternate Oxymakefile

Use `-f` to point to a different file:

```bash
ox run -f path/to/other.toml
```

## Known Limitations (v0.1.0)

- **`-j N` (parallel execution):** All jobs run sequentially regardless of
  the `-j` value.
- **`--set` (config override):** Does not override config values.

## Next Steps

- Read [Your First Workflow](./first-workflow.md) for a more detailed
  walkthrough
- Explore `ox run --help` for all available options
