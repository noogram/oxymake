# cargo-mutants Audit: ox-core

**Date:** 2026-04-04
**Tool:** cargo-mutants v24.11 with --timeout 120 -j 4
**Scope:** `cargo mutants -p ox-core`

## Results Summary

| Metric | Count | % of Viable |
|--------|------:|------------:|
| Total mutants | 424 | — |
| Unviable (compile errors) | 67 | — |
| **Viable** | **357** | **100%** |
| Caught (killed by tests) | 274 | 76.8% |
| Timeout (infinite loop) | 20 | 5.6% |
| **Missed (survived)** | **63** | **17.6%** |

**Mutation score (caught + timeout) / viable: 82.4%**
**Strict mutation score (caught only) / viable: 76.8%**

## Missed Mutants by File

### scheduler.rs — 22 missed (35% of all missed)

The scheduler is the execution engine core. Key untested areas:

| Function | Missed | What survives |
|----------|-------:|---------------|
| `handle_completion` | 10 | Negation deletes (`!`), `&&` → `||`, `>` → `>=`, `>` → `==`/`<` |
| `SchedulerState::evictable_outputs` | 5 | Return `vec![]`, `vec![String::new()]`, `vec!["xyzzy"]`; `&&` → `||`; `!` delete |
| `run_scheduler_with_cache` | 3 | `exit_code == 0` guard → true/false; field deletion in `JobResult` |
| `SchedulerState::register_materializations` | 1 | Replace with `()` |
| `SchedulerState::decrement_input_consumers` | 1 | Replace with `()` |
| `SchedulerState::cheapest_materialization` | 1 | Return `None` |
| `cancel_remaining` | 1 | Replace with `()` |

### model.rs — 20 missed (32% of all missed)

| Area | Missed | What survives |
|------|-------:|---------------|
| `TargetPattern` equality/conversion | 11 | `AsRef<str>` → `""`/`"xyzzy"`; `PartialEq` for `&str`, `str`, `String` — all three impls untested |
| `MaterializationSet` | 5 | `try_remove` off-by-one (`<` → `<=`), `iter` → `empty()`, `is_empty` → `true`, `Display` comparators |
| `RunReason::is_interesting` | 2 | → `true` / `false` |
| `OutputStream::Display` | 1 | → `Ok(Default::default())` |
| `MaterializationSet::is_empty` | 1 | → `true` |

### traits/benchmark.rs — 8 missed (13% of all missed)

`format_benchmark_tsv` is entirely untested. All arithmetic operator mutations survive:
- Line 24: `/` → `*`, `/` → `%`
- Line 25: `%` → `/`, `%` → `+`, `/` → `%`, `/` → `*`
- Line 26: `%` → `+`, `%` → `/`

### job_graph.rs — 5 missed (8% of all missed)

| Function | Missed | What survives |
|----------|-------:|---------------|
| `job_edges` | 4 | Return `vec![]`; `==` → `!=` (3 comparators) |
| `inner_mut` | 1 | Return `Box::leak(Box::new(Default::default()))` |

### resolver.rs — 4 missed (6% of all missed)

`find_producer` disambiguation logic:
- Delete match arm 0
- `>` → `==`, `<`, `>=`

### wildcard.rs — 2 missed (3% of all missed)

- `Pattern::eq`: `&&` → `||`
- `Pattern::parse`: match guard `chars.peek() == Some(&'}')` → `true`

## Timeout Mutants (20)

Timeouts indicate mutations that cause infinite loops — the tests do exercise these
code paths (the mutant doesn't escape), but there's no assertion that detects the
behavioral change quickly enough. These are effectively "caught" for scoring purposes.

| File | Count | Functions |
|------|------:|-----------|
| scheduler.rs | 14 | `run_scheduler_with_cache`, `SchedulerState::set_status/get_status/promote_downstream`, `find_ready_jobs`, `handle_completion`, `cancel_downstream` |
| job_graph.rs | 4 | `get_job`, `downstream` |
| event.rs | 2 | `EventBus::emit` |

## Recommendations

### Priority 1: scheduler.rs (22 missed + 14 timeouts)

Add unit tests for:
1. **`handle_completion`** — verify status transitions, downstream promotion triggers, and cancel logic
2. **`SchedulerState` methods** — `register_materializations`, `decrement_input_consumers`, `cheapest_materialization`, `evictable_outputs` need direct unit tests with assertions on return values
3. **`cancel_remaining`** — verify it actually cancels jobs

### Priority 2: model.rs (20 missed)

1. **`TargetPattern`** — Add tests for `AsRef<str>`, and all three `PartialEq` impls
2. **`MaterializationSet`** — Test `try_remove` boundary, `iter`, `is_empty`, `Display`
3. **`RunReason::is_interesting`** — Test both true/false paths

### Priority 3: traits/benchmark.rs (8 missed)

Add basic tests for `format_benchmark_tsv` arithmetic correctness.

### Priority 4: job_graph.rs (5 missed)

Test `job_edges` filtering with mixed edge types.

### Priority 5: resolver.rs + wildcard.rs (6 missed)

Test `find_producer` with multiple candidate producers and `Pattern::parse` with braces.
