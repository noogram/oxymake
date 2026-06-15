#!/usr/bin/env python3
"""Generate synthetic Oxymakefiles and Snakefiles at various job scales.

Usage:
    python3 generate.py [--sizes 1000,5000,10000,50000] [--outdir .]

Each size N produces a workflow with ~N total jobs arranged as:
  - Level 1: generate_{i} -> data/{i}.txt           (N/2 jobs)
  - Level 2: process_{i} -> results/{i}.out          (N/2 jobs)
  - Level 3: merge -> final.txt                      (1 job)
Total: N + 1 jobs (we round N/2 to get close to target)
"""

import argparse
import os


def items_for_target(n_jobs: int) -> int:
    """Number of items needed to produce ~n_jobs total jobs (2 rules per item + 1 merge)."""
    return max(1, (n_jobs - 1) // 2)


def generate_oxymakefile(n_jobs: int) -> str:
    n_items = items_for_target(n_jobs)
    # Build the items list as a TOML array
    items = [f'"item_{i:06d}"' for i in range(n_items)]

    # Chunk items list into lines of ~100 chars for readability
    lines = []
    current_line: list[str] = []
    current_len = 0
    for item in items:
        if current_len + len(item) + 2 > 100 and current_line:
            lines.append(", ".join(current_line))
            current_line = []
            current_len = 0
        current_line.append(item)
        current_len += len(item) + 2
    if current_line:
        lines.append(", ".join(current_line))

    items_str = ",\n  ".join(lines)

    # Wildcard constraint pattern: item_000000|item_000001|...
    # For large N this is huge, so we use a regex pattern instead
    constraint = r"item_\\d{6}"

    return f"""\
# Synthetic benchmark: {n_jobs} target jobs ({n_items} items x 2 rules + merge)
ox_version = "0.1"

[config]
items = [
  {items_str}
]

[rule.all]
input = ["final.txt"]

[rule.generate]
output = ["data/{{item}}.txt"]
wildcard_constraints = {{ item = "{constraint}" }}
shell = "echo {{item}} > {{output}}"

[rule.process]
input = ["data/{{item}}.txt"]
output = ["results/{{item}}.out"]
wildcard_constraints = {{ item = "{constraint}" }}
shell = "wc -c < {{input}} > {{output}}"

[rule.merge]
input = ["results/{{item}}.out"]
output = ["final.txt"]
expand = "product"
shell = "cat {{input}} > {{output}}"
"""


def generate_snakefile(n_jobs: int) -> str:
    n_items = items_for_target(n_jobs)
    items = [f'"item_{i:06d}"' for i in range(n_items)]

    # Chunk for readability
    lines = []
    current_line: list[str] = []
    current_len = 0
    for item in items:
        if current_len + len(item) + 2 > 100 and current_line:
            lines.append(", ".join(current_line))
            current_line = []
            current_len = 0
        current_line.append(item)
        current_len += len(item) + 2
    if current_line:
        lines.append(", ".join(current_line))

    items_str = ",\n    ".join(lines)

    return f"""\
# Synthetic benchmark: {n_jobs} target jobs ({n_items} items x 2 rules + merge)

ITEMS = [
    {items_str}
]

rule all:
    input:
        "final.txt"

rule generate:
    output:
        "data/{{item}}.txt"
    shell:
        "echo {{wildcards.item}} > {{output}}"

rule process:
    input:
        "data/{{item}}.txt"
    output:
        "results/{{item}}.out"
    shell:
        "wc -c < {{input}} > {{output}}"

rule merge:
    input:
        expand("results/{{item}}.out", item=ITEMS)
    output:
        "final.txt"
    shell:
        "cat {{input}} > {{output}}"
"""


def main():
    parser = argparse.ArgumentParser(description="Generate synthetic benchmark workflows")
    parser.add_argument(
        "--sizes",
        default="1000,5000,10000,50000",
        help="Comma-separated list of target job counts (default: 1000,5000,10000,50000)",
    )
    parser.add_argument(
        "--outdir",
        default=".",
        help="Output directory for generated files (default: .)",
    )
    args = parser.parse_args()

    sizes = [int(s.strip()) for s in args.sizes.split(",")]
    outdir = args.outdir
    os.makedirs(outdir, exist_ok=True)

    for n in sizes:
        size_dir = os.path.join(outdir, f"scale_{n}")
        os.makedirs(size_dir, exist_ok=True)

        oxymake_path = os.path.join(size_dir, "Oxymakefile.toml")
        with open(oxymake_path, "w") as f:
            f.write(generate_oxymakefile(n))

        snake_path = os.path.join(size_dir, "Snakefile")
        with open(snake_path, "w") as f:
            f.write(generate_snakefile(n))

        actual_jobs = items_for_target(n) * 2 + 1
        print(f"  Generated scale_{n}/ ({actual_jobs} jobs, {items_for_target(n)} items)")


if __name__ == "__main__":
    main()
