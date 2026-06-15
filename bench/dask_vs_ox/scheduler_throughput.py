#!/usr/bin/env python3
"""Pure scheduler throughput benchmark: Dask vs OxyMake.

Measures task dispatch rate with no-op tasks to isolate scheduling
overhead from compute and I/O.

Usage:
    python3 bench/dask_vs_ox/scheduler_throughput.py
"""
import json
import sys
import time

import dask


def bench_parallel(n, scheduler):
    """N independent no-op tasks."""
    tasks = [dask.delayed(lambda: None)() for _ in range(n)]
    t0 = time.perf_counter()
    dask.compute(*tasks, scheduler=scheduler)
    return time.perf_counter() - t0


def bench_chain(n, scheduler):
    """Linear chain of N dependent identity tasks."""
    x = dask.delayed(lambda: 0)()
    for _ in range(n):
        x = dask.delayed(lambda v: v)(x)
    t0 = time.perf_counter()
    dask.compute(x, scheduler=scheduler)
    return time.perf_counter() - t0


def bench_diamond(n, scheduler):
    """1 root -> N parallel -> 1 sink."""
    root = dask.delayed(lambda: 0)()
    middle = [dask.delayed(lambda v: v)(root) for _ in range(n)]
    sink = dask.delayed(lambda *args: None)(*middle)
    t0 = time.perf_counter()
    dask.compute(sink, scheduler=scheduler)
    return time.perf_counter() - t0


def run_suite(scales, scheduler, repeats=3):
    """Run all shapes at all scales, return results."""
    results = []
    for n in scales:
        for shape, fn in [("parallel", bench_parallel),
                          ("chain", bench_chain),
                          ("diamond", bench_diamond)]:
            times = []
            for _ in range(repeats):
                t = fn(n, scheduler)
                times.append(t)
            median = sorted(times)[len(times) // 2]
            throughput = n / median
            r = {
                "system": f"dask_{scheduler}",
                "shape": shape,
                "n": n,
                "median_s": round(median, 4),
                "tasks_per_sec": round(throughput),
            }
            results.append(r)
            print(
                f"  dask {scheduler:12s} | {shape:10s} | N={n:>7,d} | "
                f"{median:.4f}s | {throughput:>10,.0f} tasks/s"
            )
    return results


def main():
    print("=== Dask Scheduler Throughput (no-op tasks) ===")
    print()

    scales = [1_000, 10_000, 100_000]

    # Check if 1M is feasible (takes ~30s to build graph)
    if "--with-1m" in sys.argv:
        scales.append(1_000_000)

    all_results = []

    for scheduler in ["synchronous", "threads"]:
        print(f"--- Scheduler: {scheduler} ---")
        results = run_suite(scales, scheduler, repeats=3)
        all_results.extend(results)
        print()

    # Summary table
    print("=== Summary ===")
    print()
    print(f"| {'Shape':10s} | {'N':>8s} | {'Dask sync':>14s} | {'Dask threads':>14s} |")
    print(f"| {'-'*10} | {'-'*8} | {'-'*14} | {'-'*14} |")

    for shape in ["parallel", "chain", "diamond"]:
        for n in scales:
            sync = next(
                (r for r in all_results
                 if r["shape"] == shape and r["n"] == n
                 and r["system"] == "dask_synchronous"),
                None,
            )
            thr = next(
                (r for r in all_results
                 if r["shape"] == shape and r["n"] == n
                 and r["system"] == "dask_threads"),
                None,
            )
            sync_str = f"{sync['tasks_per_sec']:>10,d} t/s" if sync else "N/A"
            thr_str = f"{thr['tasks_per_sec']:>10,d} t/s" if thr else "N/A"
            print(f"| {shape:10s} | {n:>8,d} | {sync_str:>14s} | {thr_str:>14s} |")

    # Dump JSON
    with open("bench/dask_vs_ox/dask_throughput_results.json", "w") as f:
        json.dump(all_results, f, indent=2)
    print()
    print("Results saved to bench/dask_vs_ox/dask_throughput_results.json")


if __name__ == "__main__":
    main()
