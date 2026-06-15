# Design Note: JAX JIT Pre-Warmup

> **Status:** Idea — not yet designed
> **Related:** Stage 5 (pre-warm workers), fork-after-import limitation

## Problem

JAX's XLA compiler JIT-compiles functions on first call with specific input
shapes. This compilation takes 0.5-2s per function and the cache is per-process
(MAP_PRIVATE pages). The fork-after-import pattern loses the JIT cache because
the forked child gets COW copies of the parent's pages, but XLA's internal
state is invalidated by the fork.

Even with persistent workers (no fork), the first dispatch to a warm worker
pays the JIT cost. Subsequent dispatches with the same shapes are fast.

## Proposed Solution: Synthetic Data Warmup

Add a `warmup` block to call-mode rules that specifies a function to run
during the pre-warm phase with synthetic data matching the expected shapes:

```toml
[rule.signals]
call = "scripts.oxymake.call_all_signals:compute_all_signals"
environment = { uv = "pyproject.toml" }

[rule.signals.warmup]
# Run this function during pre-warm to trigger JIT compilation.
# The function receives synthetic data with the same shapes as production.
function = "scripts.oxymake.call_all_signals:warmup_signals"
# Or: auto-generate synthetic data from input shapes:
# shapes = { data = [100, 50], data_meta = "json" }
```

The warmup function would:
1. Create synthetic numpy arrays with the same dtypes and shapes
2. Call the target function once (triggers JIT compilation)
3. Discard the result

## User Contract

- The user must provide a `warmup` function or shape specification
- The warmup runs during `ensure_warm()`, before any real dispatch
- Warmup cost is amortized across all dispatches to this worker
- This is opt-in — without `warmup`, the first dispatch pays JIT cost

## Complexity Assessment

- **Oxymakefile parsing**: add optional `warmup` section to call rules
- **Warm worker protocol**: add a `warmup` command (like `exec` but discard result)
- **Shape inference**: deferred — require explicit warmup function first
- **Estimated effort**: 2-3 days

## Alternative: Persistent JIT Cache

JAX supports `jax.config.update("jax_compilation_cache_dir", "/path")` for
persisting compiled XLA executables to disk. This would survive across runs
(not just within a run). Worth investigating as a complementary approach.
