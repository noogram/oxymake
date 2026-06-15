#!/usr/bin/env python3
"""Tiny Python step for the head-to-head benchmark.

Invoked as `python3 bench_lib.py <input> <output>`.

Keeps per-job work small so that DAG resolution and orchestration overhead
dominates wall-time at scale (the variable under test).
"""
import sys
from pathlib import Path


def process_one(input_path: str, output_path: str) -> None:
    text = Path(input_path).read_text()
    Path(output_path).write_text(text.upper())


if __name__ == "__main__":
    if len(sys.argv) != 3:
        print("usage: bench_lib.py <input> <output>", file=sys.stderr)
        sys.exit(2)
    process_one(sys.argv[1], sys.argv[2])
