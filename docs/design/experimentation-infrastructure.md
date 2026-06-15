# Experimentation Infrastructure — Design Spec

> **Status:** Partially implemented (A/B comparison script done)
> **Date:** 2026-04-06
> **Remaining:** ox-bench-gen, --metrics instrumentation

---

## Implemented

### A/B Comparison Script (`bench/compare.sh`)

```bash
./bench/compare.sh <oxymakefile> [runs] [budget]
# Example:
./bench/compare.sh /path/to/your-pipeline/Oxymakefile.toml 5 1G
```

Runs N iterations of disk-only vs memory-budget, reports median wall time and speedup.

---

## Future: Synthetic Pipeline Generator (`ox-bench-gen`)

### Purpose
Generate Oxymakefiles with configurable DAG topology for controlled experiments.

### Interface
```
ox-bench-gen --jobs N --fanout F --depth D --output-size S \
             --shape {linear|diamond|wide|tree} \
             --mix shell:0.7,call:0.3 \
             --policy auto:0.6,never:0.2,always:0.2
```

### Shapes
- `linear`: chain of N jobs
- `diamond`: 1 → N → 1
- `wide`: N independent jobs
- `tree`: binary tree of depth D

### Implementation
- Standalone binary or `ox bench-gen` subcommand
- Emits valid Oxymakefile.toml to stdout
- Shell jobs: `dd if=/dev/urandom bs=S count=1 > output.bin`
- Call jobs: Python identity function

---

## Future: `--metrics` Instrumentation

### Purpose
Emit structured NDJSON events for memory/eviction/lock timing.

### Metrics

| Metric | Source | When |
|--------|--------|------|
| `memory_watermark_bytes` | enforce_memory_budget | After each job completion |
| `eviction_count` / `eviction_bytes` | enforce_memory_budget | After each eviction |
| `register_mat_lock_us` | handle_completion timer | After each completion |
| `disk_writer_queue_depth` | channel len | Periodically |
| `input_data_hit_rate` | collect_input_data | Per job dispatch |
| `peak_rss_bytes` | getrusage(RUSAGE_SELF) | At end of run |

### Interface
```
ox run --metrics metrics.ndjson
```

### Implementation
- `MetricsCollector` trait with NDJSON file sink
- Zero-overhead when not enabled (empty trait impl)
- Injected into `run_scheduler_with_cache` alongside EventBus
- ~200 lines of code

---

## CI Integration

| Suite | Command | Cadence |
|-------|---------|---------|
| Unit + proptest | `cargo test --workspace` | Every PR |
| Stress tests | `cargo test -p ox-core --test stress_scheduler -- --ignored` | Nightly |
| Benchmarks | `cargo bench -p ox-core` | Weekly |
| A/B comparison | `./bench/compare.sh` | Manual / release |
