#!/usr/bin/env python3
"""Generator for the OxyMake vs Snakemake head-to-head bench.

Emits paired `workflow.smk` + `workflow.toml` at one or more scales.

DAG topology (rule-fan + chain):

    seed (shell)
       │
       ▼
    gen_{i}        (N shell rules — Layer 1)
       │
       ▼
    process_{i}    (N python-via-shell rules — Layer 2)
       │
       ▼
    finalize_{i}   (N file-copy rules — Layer 3, file-only deps)
       │
       ▼
    merge (single)

Total jobs at scale N = 3·N + 2 (seed + 3·N transforms + merge).

For N=3333 → 10001 jobs (≥ 10⁴, the bench precondition).
"""

from __future__ import annotations

import argparse
import os


def items_for_target(n_jobs: int) -> int:
    # 3 transforms per item + 2 standalone (seed + merge).
    # Round UP so total jobs ≥ n_jobs (required: 10⁴ for the pre-mortem gate).
    return max(1, -((-(n_jobs - 2)) // 3))


def fmt_items_toml(n: int) -> str:
    items = [f'"i_{i:06d}"' for i in range(n)]
    rows: list[str] = []
    line: list[str] = []
    line_len = 0
    for it in items:
        if line and line_len + len(it) + 2 > 96:
            rows.append(", ".join(line))
            line = [it]
            line_len = len(it)
        else:
            line.append(it)
            line_len += len(it) + 2
    if line:
        rows.append(", ".join(line))
    return ",\n  ".join(rows)


def fmt_items_py(n: int) -> str:
    items = [f'"i_{i:06d}"' for i in range(n)]
    rows: list[str] = []
    line: list[str] = []
    line_len = 0
    for it in items:
        if line and line_len + len(it) + 2 > 96:
            rows.append(", ".join(line))
            line = [it]
            line_len = len(it)
        else:
            line.append(it)
            line_len += len(it) + 2
    if line:
        rows.append(", ".join(line))
    return ",\n    ".join(rows)


def gen_oxymakefile(n_items: int, bench_lib: str) -> str:
    items_str = fmt_items_toml(n_items)
    constraint = r"i_\\d{6}"
    return (
        f'# Head-to-head bench: OxyMake side — {n_items} items × 3 layers + 2 = {3 * n_items + 2} jobs\n'
        f'ox_version = "0.1"\n'
        f'\n'
        f'[config]\n'
        f'items = [\n'
        f'  {items_str}\n'
        f']\n'
        f'\n'
        f'[rule.all]\n'
        f'input = ["merged.txt"]\n'
        f'\n'
        f'# Layer 0 — seed (single shell rule).\n'
        f'[rule.seed]\n'
        f'output = ["seed.txt"]\n'
        f'shell = "printf \'seed\\n\' > {{output}}"\n'
        f'\n'
        f'# Layer 1 — shell rule (per item).\n'
        f'[rule.gen]\n'
        f'input = ["seed.txt"]\n'
        f'output = ["step1/{{item}}.txt"]\n'
        f'wildcard_constraints = {{ item = "{constraint}" }}\n'
        f'shell = "printf \'%s\\n\' {{item}} > {{output}}"\n'
        f'\n'
        f'# Layer 2 — Python-via-shell rule (per item).\n'
        f'# bench_lib.py is declared as a named input so editing it correctly\n'
        f'# invalidates the cache (a tool the job reads IS a dependency).\n'
        f'[rule.process]\n'
        f'input = {{ data = "step1/{{item}}.txt", lib = "{bench_lib}" }}\n'
        f'output = ["step2/{{item}}.txt"]\n'
        f'wildcard_constraints = {{ item = "{constraint}" }}\n'
        f'shell = "python3 {{input.lib}} {{input.data}} {{output}}"\n'
        f'\n'
        f'# Layer 3 — file-only / cp rule (per item).\n'
        f'[rule.finalize]\n'
        f'input = ["step2/{{item}}.txt"]\n'
        f'output = ["step3/{{item}}.txt"]\n'
        f'wildcard_constraints = {{ item = "{constraint}" }}\n'
        f'shell = "cp {{input}} {{output}}"\n'
        f'\n'
        f'# Merge (single).\n'
        f'[rule.merge]\n'
        f'input = ["step3/{{item}}.txt"]\n'
        f'output = ["merged.txt"]\n'
        f'expand = "product"\n'
        f'shell = "cat {{input}} > {{output}}"\n'
    )


def gen_snakefile(n_items: int, bench_lib: str) -> str:
    items_str = fmt_items_py(n_items)
    return (
        f'# Head-to-head bench: Snakemake side — {n_items} items × 3 layers + 2 = {3 * n_items + 2} jobs\n'
        f'\n'
        f'ITEMS = [\n'
        f'    {items_str}\n'
        f']\n'
        f'\n'
        f'rule all:\n'
        f'    input:\n'
        f'        "merged.txt"\n'
        f'\n'
        f'# Layer 0 — seed (single shell rule).\n'
        f'rule seed:\n'
        f'    output:\n'
        f'        "seed.txt"\n'
        f'    shell:\n'
        f'        "printf \'seed\\n\' > {{output}}"\n'
        f'\n'
        f'# Layer 1 — shell rule (per item).\n'
        f'rule gen:\n'
        f'    input:\n'
        f'        "seed.txt"\n'
        f'    output:\n'
        f'        "step1/{{item}}.txt"\n'
        f'    shell:\n'
        f'        "printf \'%s\\n\' {{wildcards.item}} > {{output}}"\n'
        f'\n'
        f'# Layer 2 — Python-via-shell rule (per item).\n'
        f'# bench_lib.py is declared as a named input so editing it correctly\n'
        f'# invalidates the cache (a tool the job reads IS a dependency).\n'
        f'rule process:\n'
        f'    input:\n'
        f'        data="step1/{{item}}.txt",\n'
        f'        lib="{bench_lib}"\n'
        f'    output:\n'
        f'        "step2/{{item}}.txt"\n'
        f'    shell:\n'
        f'        "python3 {{input.lib}} {{input.data}} {{output}}"\n'
        f'\n'
        f'# Layer 3 — file-only / cp rule (per item).\n'
        f'rule finalize:\n'
        f'    input:\n'
        f'        "step2/{{item}}.txt"\n'
        f'    output:\n'
        f'        "step3/{{item}}.txt"\n'
        f'    shell:\n'
        f'        "cp {{input}} {{output}}"\n'
        f'\n'
        f'# Merge (single).\n'
        f'rule merge:\n'
        f'    input:\n'
        f'        expand("step3/{{item}}.txt", item=ITEMS)\n'
        f'    output:\n'
        f'        "merged.txt"\n'
        f'    shell:\n'
        f'        "cat {{input}} > {{output}}"\n'
    )


def main() -> None:
    parser = argparse.ArgumentParser(description="Generate head-to-head bench workflows.")
    parser.add_argument(
        "--sizes",
        default="100,1000,10000",
        help="Comma-separated job counts (≥ 4 each). Default: 100,1000,10000.",
    )
    parser.add_argument(
        "--outdir",
        default=".",
        help="Output directory. One subdir per size.",
    )
    parser.add_argument(
        "--bench-lib",
        default="../bench_lib.py",
        help="bench_lib.py path as it should appear (literally) inside the generated shell commands. "
             "Default is `../bench_lib.py` (correct for scale_N/ subdirs).",
    )
    parser.add_argument(
        "--canonical",
        action="store_true",
        help="Write workflow.{toml,smk} directly into --outdir instead of scale_N/ subdirs. "
             "Use for the single canonical bench root (writes bench_lib path literally).",
    )
    args = parser.parse_args()

    sizes = [int(s.strip()) for s in args.sizes.split(",")]
    outdir = os.path.abspath(args.outdir)
    os.makedirs(outdir, exist_ok=True)

    if args.canonical:
        if len(sizes) != 1:
            raise SystemExit("--canonical requires exactly one --sizes value")
        n_items = items_for_target(sizes[0])
        actual_jobs = 3 * n_items + 2
        with open(os.path.join(outdir, "workflow.toml"), "w") as f:
            f.write(gen_oxymakefile(n_items, args.bench_lib))
        with open(os.path.join(outdir, "workflow.smk"), "w") as f:
            f.write(gen_snakefile(n_items, args.bench_lib))
        print(f"  {outdir}/workflow.{{toml,smk}}  → {actual_jobs} jobs ({n_items} items)")
        return

    for n_target in sizes:
        n_items = items_for_target(n_target)
        actual_jobs = 3 * n_items + 2
        size_dir = os.path.join(outdir, f"scale_{n_target}")
        os.makedirs(size_dir, exist_ok=True)

        with open(os.path.join(size_dir, "workflow.toml"), "w") as f:
            f.write(gen_oxymakefile(n_items, args.bench_lib))
        with open(os.path.join(size_dir, "workflow.smk"), "w") as f:
            f.write(gen_snakefile(n_items, args.bench_lib))

        print(f"  scale_{n_target}/  → {actual_jobs} jobs ({n_items} items)")


if __name__ == "__main__":
    main()
