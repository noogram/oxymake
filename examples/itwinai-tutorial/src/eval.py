"""Evaluate a trained MLP against MNIST test set and write accuracy."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import numpy as np
import torch
from torch import nn


def _build_model(hidden_dim: int) -> nn.Module:
    return nn.Sequential(
        nn.Linear(784, hidden_dim),
        nn.ReLU(),
        nn.Linear(hidden_dim, 10),
    )


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--checkpoint", required=True, type=Path)
    parser.add_argument("--data", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args(argv)

    ckpt = torch.load(args.checkpoint, map_location="cpu", weights_only=True)
    hidden_dim = int(ckpt.get("hidden_dim", 64))
    model = _build_model(hidden_dim)
    model.load_state_dict(ckpt["state_dict"])
    model.eval()

    data = np.load(args.data)
    x_test = torch.from_numpy(data["x_test"])
    y_test = torch.from_numpy(data["y_test"])

    with torch.no_grad():
        logits = model(x_test)
        preds = logits.argmax(dim=1)
        acc = float((preds == y_test).float().mean().item())

    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(f"{acc:.6f}\n")
    print(f"test_accuracy={acc:.4f} -> {args.output}", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
