#!/usr/bin/env python3
"""Dask benchmark runner for OxyMake vs Dask comparison.

Usage:
    python3 bench/dask_vs_ox/dask_benchmark.py --workload a --n 100 --scheduler threads
    python3 bench/dask_vs_ox/dask_benchmark.py --workload c --n 64 --scheduler synchronous
"""
import argparse
import json
import math
import time

import dask
import numpy as np

from numpy_tasks import (
    map_sincos_memory,
    chain_step_memory,
    reduce_mean_memory,
)

ROWS, COLS = 1250, 1000  # ~10MB per chunk


def run_workload_a(n: int, scheduler: str) -> dict:
    """Embarrassingly parallel: N independent map tasks."""
    chunks = [np.random.randn(ROWS, COLS) for _ in range(n)]

    delayed_tasks = [dask.delayed(map_sincos_memory)(c) for c in chunks]

    t0 = time.perf_counter()
    dask.compute(*delayed_tasks, scheduler=scheduler)
    wall = time.perf_counter() - t0

    return {"workload": "A_map", "n": n, "scheduler": scheduler,
            "wall_s": round(wall, 4), "throughput": round(n / wall, 1)}


def run_workload_b(n: int, scheduler: str) -> dict:
    """Linear chain: sequential dependent tasks."""
    initial = np.random.randn(ROWS, COLS)

    prev = dask.delayed(lambda x: x)(initial)  # wrap initial
    for _ in range(n):
        prev = dask.delayed(chain_step_memory)(prev)

    t0 = time.perf_counter()
    dask.compute(prev, scheduler=scheduler)
    wall = time.perf_counter() - t0

    return {"workload": "B_chain", "n": n, "scheduler": scheduler,
            "wall_s": round(wall, 4), "throughput": round(n / wall, 1)}


def run_workload_c(n_leaves: int, scheduler: str) -> dict:
    """Tree reduction: binary tree of element-wise means."""
    leaves = [dask.delayed(lambda: np.random.randn(ROWS, COLS))()
              for _ in range(n_leaves)]

    # Build binary tree
    level = leaves
    total_tasks = n_leaves
    while len(level) > 1:
        next_level = []
        for i in range(0, len(level) - 1, 2):
            next_level.append(dask.delayed(reduce_mean_memory)(level[i], level[i + 1]))
            total_tasks += 1
        if len(level) % 2 == 1:
            next_level.append(level[-1])
        level = next_level

    t0 = time.perf_counter()
    dask.compute(level[0], scheduler=scheduler)
    wall = time.perf_counter() - t0

    return {"workload": "C_tree", "n": n_leaves, "scheduler": scheduler,
            "wall_s": round(wall, 4), "throughput": round(total_tasks / wall, 1),
            "total_tasks": total_tasks}


def main():
    parser = argparse.ArgumentParser(description="Dask benchmark runner")
    parser.add_argument("--workload", choices=["a", "b", "c"], required=True)
    parser.add_argument("--n", type=int, required=True)
    parser.add_argument("--scheduler", choices=["synchronous", "threads"],
                        default="threads")
    parser.add_argument("--repeats", type=int, default=3)
    args = parser.parse_args()

    runners = {"a": run_workload_a, "b": run_workload_b, "c": run_workload_c}
    runner = runners[args.workload]

    results = []
    for i in range(args.repeats):
        result = runner(args.n, args.scheduler)
        result["repeat"] = i
        results.append(result)
        print(json.dumps(result), flush=True)

    # Print median
    walls = sorted(r["wall_s"] for r in results)
    median = walls[len(walls) // 2]
    throughputs = sorted(r["throughput"] for r in results)
    med_tp = throughputs[len(throughputs) // 2]
    print(json.dumps({"summary": True, "workload": results[0]["workload"],
                       "n": args.n, "scheduler": args.scheduler,
                       "median_wall_s": median, "median_throughput": med_tp}),
          flush=True)


if __name__ == "__main__":
    main()
