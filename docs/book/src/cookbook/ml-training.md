# ML Training Pipeline

> Coming soon.

This page will show a machine learning training pipeline in OxyMake,
covering:

- **Data preparation**: feature extraction, train/test splitting, and
  normalization
- **Hyperparameter sweeps**: wildcard-driven grid search across learning
  rates, architectures, and regularization parameters
- **GPU resource management**: declaring GPU requirements per rule for
  SLURM/Kubernetes scheduling
- **Model evaluation**: automated metric collection and comparison
- **In-memory passing**: using `call` mode with Arrow IPC to pass DataFrames
  between feature computation and training without disk I/O
