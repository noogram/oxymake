"""Download MNIST and write a flattened npz suitable for an MLP."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np


def _download(cache_dir: Path):
    from torchvision.datasets import MNIST

    train = MNIST(root=str(cache_dir), train=True, download=True)
    test = MNIST(root=str(cache_dir), train=False, download=True)
    return train, test


def _to_arrays(dataset):
    images = dataset.data.numpy().astype(np.float32) / 255.0
    images = images.reshape(images.shape[0], -1)
    labels = dataset.targets.numpy().astype(np.int64)
    return images, labels


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument(
        "--cache-dir",
        type=Path,
        default=None,
        help="Where torchvision caches the raw MNIST blob (default: alongside output).",
    )
    args = parser.parse_args(argv)

    cache_dir = args.cache_dir or args.output.parent / "_torchvision_cache"
    cache_dir.mkdir(parents=True, exist_ok=True)
    args.output.parent.mkdir(parents=True, exist_ok=True)

    train, test = _download(cache_dir)
    x_train, y_train = _to_arrays(train)
    x_test, y_test = _to_arrays(test)

    np.savez_compressed(
        args.output,
        x_train=x_train,
        y_train=y_train,
        x_test=x_test,
        y_test=y_test,
    )
    print(
        f"wrote {args.output} (train={x_train.shape}, test={x_test.shape})",
        flush=True,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
