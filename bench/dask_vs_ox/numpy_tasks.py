"""Shared numpy task functions for OxyMake vs Dask benchmark.

Each function has two variants:
- Disk variant: reads/writes .npy files (for OxyMake shell mode)
- Memory variant: operates on numpy arrays (for Dask + OxyMake call mode)
"""
import numpy as np
import sys


# ---------------------------------------------------------------------------
# Chunk creation
# ---------------------------------------------------------------------------

def create_chunk(path: str, rows: int = 1250, cols: int = 1000):
    """Create and save a random float64 array (~10MB default)."""
    arr = np.random.randn(rows, cols)
    np.save(path, arr)


# ---------------------------------------------------------------------------
# Workload A: Embarrassingly parallel (map sin²+cos²)
# ---------------------------------------------------------------------------

def map_sincos(input_path: str, output_path: str):
    """Disk variant: load → sin²+cos² → save."""
    arr = np.load(input_path)
    result = np.sin(arr) ** 2 + np.cos(arr) ** 2
    np.save(output_path, result)


def map_sincos_memory(arr):
    """Memory variant for Dask and OxyMake call mode."""
    return np.sin(arr) ** 2 + np.cos(arr) ** 2


# ---------------------------------------------------------------------------
# Workload B: Linear chain (scalar multiply + noise)
# ---------------------------------------------------------------------------

def chain_step(input_path: str, output_path: str, scalar: float = 1.001):
    """Disk variant: load → multiply + noise → save."""
    arr = np.load(input_path)
    result = arr * scalar + np.random.randn(*arr.shape) * 0.001
    np.save(output_path, result)


def chain_step_memory(arr, scalar: float = 1.001):
    """Memory variant."""
    return arr * scalar + np.random.randn(*arr.shape) * 0.001


# ---------------------------------------------------------------------------
# Workload C: Tree reduction (element-wise mean)
# ---------------------------------------------------------------------------

def reduce_mean(path_a: str, path_b: str, output_path: str):
    """Disk variant: load two arrays → mean → save."""
    a = np.load(path_a)
    b = np.load(path_b)
    np.save(output_path, (a + b) / 2.0)


def reduce_mean_memory(a, b):
    """Memory variant."""
    return (a + b) / 2.0


# ---------------------------------------------------------------------------
# CLI entry point for OxyMake shell mode
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    func_name = sys.argv[1]
    args = sys.argv[2:]

    if func_name == "create_chunk":
        create_chunk(args[0], int(args[1]) if len(args) > 1 else 1250,
                     int(args[2]) if len(args) > 2 else 1000)
    elif func_name == "map_sincos":
        map_sincos(args[0], args[1])
    elif func_name == "chain_step":
        chain_step(args[0], args[1], float(args[2]) if len(args) > 2 else 1.001)
    elif func_name == "reduce_mean":
        reduce_mean(args[0], args[1], args[2])
    else:
        print(f"Unknown function: {func_name}", file=sys.stderr)
        sys.exit(1)
