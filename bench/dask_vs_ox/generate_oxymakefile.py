#!/usr/bin/env python3
"""Generate Oxymakefile for dask-vs-ox benchmark workloads.

Usage:
    python3 bench/dask_vs_ox/generate_oxymakefile.py --workload a --n 100 --mode shell
    python3 bench/dask_vs_ox/generate_oxymakefile.py --workload c --n 64 --mode call
"""
import argparse
import math
import sys


def generate_workload_a(n: int, results_dir: str, mode: str) -> str:
    """Embarrassingly parallel: N independent map tasks."""
    lines = [
        f"# Workload A: {n} parallel map tasks (sin²+cos²)",
        f'ox_version = "0.1"',
        "",
        "[config]",
        f'results_dir = "{results_dir}"',
        f'data_dir = "{results_dir}/chunks"',
        "",
    ]

    # Map rules (chunks pre-generated, so we just process them)
    out_paths = []
    for i in range(n):
        inp = f"{{config.data_dir}}/chunk_{i:04d}.npy"
        out = f"{{config.results_dir}}/result_{i:04d}.npy"
        out_paths.append(out)

        if mode == "shell":
            lines.append(f"[rule.map_{i:04d}]")
            lines.append(f'input = ["{inp}"]')
            lines.append(f'output = ["{out}"]')
            lines.append(f'shell = "python3 bench/dask_vs_ox/numpy_tasks.py map_sincos {inp} {out}"')
            lines.append("")
        else:  # call
            lines.append(f"[rule.map_{i:04d}]")
            lines.append(f'output = ["{out}"]')
            lines.append(f'lang = "python"')
            lines.append(f'call = "bench.dask_vs_ox.numpy_tasks:map_sincos_memory"')
            lines.append(f"[rule.map_{i:04d}.input]")
            lines.append(f'arr = "{inp}"')
            lines.append("")

    # All target
    all_inputs = ", ".join(f'"{p}"' for p in out_paths)
    lines.append("[rule.all]")
    lines.append(f"input = [{all_inputs}]")

    return "\n".join(lines)


def generate_workload_b(n: int, results_dir: str, mode: str) -> str:
    """Linear chain: N sequential tasks."""
    lines = [
        f"# Workload B: {n}-step linear chain",
        f'ox_version = "0.1"',
        "",
        "[config]",
        f'results_dir = "{results_dir}"',
        f'data_dir = "{results_dir}/chunks"',
        "",
    ]

    for i in range(n):
        inp = f"{{config.data_dir}}/chunk_0000.npy" if i == 0 else f"{{config.results_dir}}/step_{i-1:04d}.npy"
        out = f"{{config.results_dir}}/step_{i:04d}.npy"

        if mode == "shell":
            lines.append(f"[rule.step_{i:04d}]")
            lines.append(f'input = ["{inp}"]')
            lines.append(f'output = ["{out}"]')
            lines.append(f'shell = "python3 bench/dask_vs_ox/numpy_tasks.py chain_step {inp} {out}"')
            lines.append("")
        else:
            lines.append(f"[rule.step_{i:04d}]")
            lines.append(f'output = ["{out}"]')
            lines.append(f'lang = "python"')
            lines.append(f'call = "bench.dask_vs_ox.numpy_tasks:chain_step_memory"')
            lines.append(f"[rule.step_{i:04d}.input]")
            lines.append(f'arr = "{inp}"')
            lines.append("")

    lines.append("[rule.all]")
    lines.append(f'input = ["{{config.results_dir}}/step_{n-1:04d}.npy"]')

    return "\n".join(lines)


def generate_workload_c(n_leaves: int, results_dir: str, mode: str) -> str:
    """Tree reduction: binary tree of means."""
    lines = [
        f"# Workload C: {n_leaves}-leaf binary tree reduction",
        f'ox_version = "0.1"',
        "",
        "[config]",
        f'results_dir = "{results_dir}"',
        f'data_dir = "{results_dir}/chunks"',
        "",
    ]

    # Leaf level — read from pre-generated chunks
    level_keys = []
    for i in range(n_leaves):
        key = f"leaf_{i:04d}"
        inp = f"{{config.data_dir}}/chunk_{i:04d}.npy"
        out = f"{{config.results_dir}}/{key}.npy"
        level_keys.append((key, out))

        # Identity copy — leaf just passes through
        if mode == "shell":
            lines.append(f"[rule.{key}]")
            lines.append(f'input = ["{inp}"]')
            lines.append(f'output = ["{out}"]')
            lines.append(f'shell = "cp {inp} {out}"')
            lines.append("")
        else:
            lines.append(f"[rule.{key}]")
            lines.append(f'output = ["{out}"]')
            lines.append(f'lang = "python"')
            lines.append(f'call = "bench.dask_vs_ox.numpy_tasks:map_sincos_memory"')
            lines.append(f"[rule.{key}.input]")
            lines.append(f'arr = "{inp}"')
            lines.append("")

    # Reduction levels
    depth = 0
    while len(level_keys) > 1:
        next_level = []
        for j in range(0, len(level_keys) - 1, 2):
            key_a, path_a = level_keys[j]
            key_b, path_b = level_keys[j + 1]
            rkey = f"reduce_d{depth}_{j // 2:04d}"
            out = f"{{config.results_dir}}/{rkey}.npy"
            next_level.append((rkey, out))

            if mode == "shell":
                lines.append(f"[rule.{rkey}]")
                lines.append(f'input = ["{path_a}", "{path_b}"]')
                lines.append(f'output = ["{out}"]')
                lines.append(f'shell = "python3 bench/dask_vs_ox/numpy_tasks.py reduce_mean {path_a} {path_b} {out}"')
                lines.append("")
            else:
                lines.append(f"[rule.{rkey}]")
                lines.append(f'output = ["{out}"]')
                lines.append(f'lang = "python"')
                lines.append(f'call = "bench.dask_vs_ox.numpy_tasks:reduce_mean_memory"')
                lines.append(f"[rule.{rkey}.input]")
                lines.append(f'a = "{path_a}"')
                lines.append(f'b = "{path_b}"')
                lines.append("")

        if len(level_keys) % 2 == 1:
            next_level.append(level_keys[-1])
        level_keys = next_level
        depth += 1

    # All target
    root_path = level_keys[0][1]
    lines.append("[rule.all]")
    lines.append(f'input = ["{root_path}"]')

    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--workload", choices=["a", "b", "c"], required=True)
    parser.add_argument("--n", type=int, required=True)
    parser.add_argument("--mode", choices=["shell", "call"], default="shell")
    parser.add_argument("--results-dir", default="bench_output")
    args = parser.parse_args()

    generators = {
        "a": generate_workload_a,
        "b": generate_workload_b,
        "c": generate_workload_c,
    }
    print(generators[args.workload](args.n, args.results_dir, args.mode))


if __name__ == "__main__":
    main()
