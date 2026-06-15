"""itwinai pipeline components — MNIST MLP trainer.

Two BaseComponent subclasses wired by Hydra in config/train.yaml:

  NpzDataLoader  → reads data/mnist.npz, yields (train_loader, test_loader)
  MlpTrainer     → 784→64→10 MLP, 1 CPU epoch, writes checkpoints/model.pth

The components are minimal on purpose. The tutorial point is the OxyMake
DAG above them, not the model.
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import torch
from torch import nn
from torch.utils.data import DataLoader, TensorDataset

from itwinai.components import BaseComponent, monitor_exec


class NpzDataLoader(BaseComponent):
    """Load mnist.npz and emit (train_loader, test_loader)."""

    def __init__(self, npz_path: str, batch_size: int = 128, name: str | None = None):
        super().__init__(name=name)
        self.save_parameters(**self.locals2params(locals()))
        self.npz_path = npz_path
        self.batch_size = batch_size

    @monitor_exec
    def execute(self):
        data = np.load(self.npz_path)
        train_ds = TensorDataset(
            torch.from_numpy(data["x_train"]),
            torch.from_numpy(data["y_train"]),
        )
        test_ds = TensorDataset(
            torch.from_numpy(data["x_test"]),
            torch.from_numpy(data["y_test"]),
        )
        train_loader = DataLoader(train_ds, batch_size=self.batch_size, shuffle=True)
        test_loader = DataLoader(test_ds, batch_size=self.batch_size, shuffle=False)
        return train_loader, test_loader


class MlpTrainer(BaseComponent):
    """Train a 2-layer MLP and write a checkpoint."""

    def __init__(
        self,
        checkpoint_path: str,
        hidden_dim: int = 64,
        lr: float = 1e-3,
        epochs: int = 1,
        seed: int = 0,
        name: str | None = None,
    ):
        super().__init__(name=name)
        self.save_parameters(**self.locals2params(locals()))
        self.checkpoint_path = checkpoint_path
        self.hidden_dim = hidden_dim
        self.lr = lr
        self.epochs = epochs
        self.seed = seed

    @monitor_exec
    def execute(self, loaders):
        train_loader, _test_loader = loaders
        torch.manual_seed(self.seed)

        model = nn.Sequential(
            nn.Linear(784, self.hidden_dim),
            nn.ReLU(),
            nn.Linear(self.hidden_dim, 10),
        )
        opt = torch.optim.Adam(model.parameters(), lr=self.lr)
        loss_fn = nn.CrossEntropyLoss()

        model.train()
        for epoch in range(self.epochs):
            total = 0.0
            n_batches = 0
            for x, y in train_loader:
                opt.zero_grad()
                logits = model(x)
                loss = loss_fn(logits, y)
                loss.backward()
                opt.step()
                total += float(loss.item())
                n_batches += 1
            avg = total / max(n_batches, 1)
            print(f"epoch {epoch}: avg_loss={avg:.4f}", flush=True)

        Path(self.checkpoint_path).parent.mkdir(parents=True, exist_ok=True)
        torch.save(
            {"state_dict": model.state_dict(), "hidden_dim": self.hidden_dim},
            self.checkpoint_path,
        )
        print(f"wrote checkpoint {self.checkpoint_path}", flush=True)
        return self.checkpoint_path
