# DAG Resolution Research: Optimal Algorithms for OxyMake

> **Full research archived to vault:** `vault/research/oxymake/dag-algorithms.md`

**Issue:** ox-vs2

## Summary

OxyMake (Rust) is 2x slower than Snakemake (Python) at 10K+ jobs due to five
algorithmic deficiencies (linear rule scan, no regex caching, excessive string
allocation, no memoization, pattern re-parsing). Projected 18–30x improvement
with four-phase optimization plan.

## Performance Targets

| Jobs   | Current | Phase 1 target | Phase 2 target |
|-------:|--------:|---------------:|---------------:|
| 10,000 | 8.86s   | ≤2.0s          | ≤0.5s          |
| 50,000 | 43.28s  | ≤10.0s         | ≤2.5s          |

## Implementation Phases

1. **Quick Wins** — regex caching (`OnceCell`), extension-based output index,
   known-producers cache, pre-parse patterns at load time.
2. **String Interning** — `lasso` interner, `SmallVec` wildcards, `bumpalo` arena.
3. **Parallelism** — `rayon` for cartesian product and independent subtree resolution.
4. **Advanced** — incremental resolution cache, Aho-Corasick automaton, lazy graph.
