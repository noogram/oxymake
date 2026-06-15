# Execution Optimization Roadmap

> **Status:** Design note — execution speedup progression from disk-based transport
> to pre-warmed workers.
> **Issue:** ox-710t

## Overview

OxyMake's scheduler dispatches jobs through a three-level graph pipeline
(RuleGraph → JobGraph → ExecGraph). Today, all inter-job data flows through
the filesystem. This roadmap traces five stages of optimization, each building
on the last, from the current disk-bound baseline to a fully pipelined system
with pre-warmed workers.

---

## Stage 1: Current State — Disk-Based Transport

Every job reads inputs from disk and writes outputs to disk. The scheduler
dispatches ready jobs (all upstream outputs exist on the filesystem) via the
`Executor` trait. No data stays in memory between jobs.

```mermaid
sequenceDiagram
    participant S as Scheduler
    participant J1 as Job A (align)
    participant FS as Filesystem
    participant J2 as Job B (sort)

    S->>J1: execute(align-A)
    J1->>FS: write results/A.bam (50 MB)
    J1-->>S: JobResult::success
    Note over S: Checks disk for A.bam
    S->>J2: execute(sort-A)
    J2->>FS: read results/A.bam
    J2->>FS: write results/A.sorted.bam
    J2-->>S: JobResult::success
```

**Data flow graph (29-node compute pipeline):**

```mermaid
flowchart TD
    subgraph "Stage 1: Disk Transport (29 nodes)"
        U[cohort config] -->|disk| D1[data: dataset-A]
        U -->|disk| D2[data: dataset-B]

        D1 -->|disk| F1["features: transform-1"]
        D1 -->|disk| F2["features: transform-2"]
        D2 -->|disk| F3["features: transform-3"]
        D2 -->|disk| F4["features: transform-4"]

        F1 -->|disk| A1["feature: transform-1"]
        F2 -->|disk| A2["feature: transform-2"]
        F3 -->|disk| A3["feature: transform-3"]
        F4 -->|disk| A4["feature: transform-4"]

        A1 -->|disk| C[combine features]
        A2 -->|disk| C
        A3 -->|disk| C
        A4 -->|disk| C

        C -->|disk| OPT[optimize]
        OPT -->|disk| SIM[evaluate]
        SIM -->|disk| RPT[report]
    end

    style U fill:#e8e8e8,stroke:#666
    style RPT fill:#f9f,stroke:#333
```

**Bottleneck:** Every edge is a disk round-trip. A 50 MB intermediate written
and re-read at 500 MB/s SSD throughput costs ~200 ms. With 28 edges in the
critical path of a typical pipeline, disk I/O alone adds **~5–6 seconds** of
pure serialization overhead — often dwarfing actual compute for lightweight
transforms.

**Characteristics:**
| Metric | Value |
|--------|-------|
| Inter-job transport | Filesystem (local or NFS) |
| Serialization cost | Full write + read per edge |
| Critical path I/O | ~200 ms × edges on longest chain |
| Memory footprint | One job's data at a time |
| Executor coupling | None — any executor works |

---

## Stage 2: In-Memory Critical Path + Async Disk

Keep the critical path in memory while writing to disk asynchronously in the
background. Jobs on the critical path pass `Arc<DataFrame>` (or Arrow IPC
buffers) directly via a shared memory map. Disk writes happen concurrently for
caching/reproducibility but don't block the next job.

```mermaid
sequenceDiagram
    participant S as Scheduler
    participant J1 as Job A (align)
    participant MEM as Memory Map
    participant FS as Filesystem
    participant J2 as Job B (sort)

    S->>J1: execute(align-A)
    J1->>MEM: put(A.bam, Arc<data>)
    J1-->>FS: async write (background)
    J1-->>S: JobResult::success
    Note over S: A.bam in memory — skip disk check
    S->>J2: execute(sort-A)
    J2->>MEM: get(A.bam) → zero-copy
    J2->>MEM: put(A.sorted.bam, Arc<data>)
    J2-->>FS: async write (background)
    J2-->>S: JobResult::success
    Note over FS: Background writes complete eventually
```

**Architecture change:**

```mermaid
flowchart LR
    subgraph "Scheduler (tokio runtime)"
        direction TB
        CP["Critical Path Detector<br/>(ox-plan)"] --> MM["OutputMemoryMap<br/>Arc&lt;DashMap&lt;PathBuf, Arc&lt;Bytes&gt;&gt;&gt;"]
        MM --> ASYNC["Async Disk Writer<br/>(background tokio::spawn)"]
    end

    subgraph "Job Execution"
        J1["Job A"] -->|"put(key, data)"| MM
        MM -->|"get(key) → Arc"| J2["Job B"]
    end

    ASYNC -->|"eventual write"| FS[(Filesystem)]

    style CP fill:#ff9,stroke:#333
    style MM fill:#9f9,stroke:#333
    style ASYNC fill:#ccc,stroke:#666
```

**What changes:**
- `ox-core` scheduler gains an `OutputMemoryMap` — a concurrent map from output
  paths to `Arc<Bytes>`.
- Critical path jobs (identified by `CriticalPathPass`) use `put()` / `get()`
  instead of disk I/O.
- A background task drains the map to disk for cache consistency.
- Non-critical-path jobs still use disk (no memory pressure from off-path data).
- `MaterializePolicy::Never` outputs skip the background write entirely.

**Expected speedup on critical path:**

| Metric | Stage 1 | Stage 2 |
|--------|---------|---------|
| Critical path edge latency | ~200 ms (disk) | ~0.1 ms (memcpy) |
| 10-edge critical path overhead | ~2 s | ~1 ms |
| Off-path transport | disk | disk (unchanged) |
| Memory pressure | low | moderate (critical path data resident) |

**Effort:** ~2 weeks. Touches `ox-core` (scheduler + new `OutputMemoryMap`),
`ox-plan` (critical path annotations propagated to runtime), `ox-exec-local`
(check memory map before disk).

**Prerequisites:** None — works with local executor only. No Ray dependency.

**Expected speedup:** **2–10×** for pipelines where critical path is I/O-bound
(lightweight transforms chained sequentially). Negligible for compute-heavy jobs.

**Expected ADRs:**
- **ADR: OutputMemoryMap design** — Concurrent map API (`put`/`get`), eviction
  policy, memory budget, interaction with `MaterializePolicy`.
- **ADR: Async disk writer** — Background write strategy, consistency guarantees
  (what happens on crash before flush), integration with content-addressable
  cache (ADR-001).
- **ADR: Critical-path runtime annotations** — How `CriticalPathPass` results
  propagate from `ox-plan` to the scheduler at runtime to gate memory vs. disk
  routing.

---

## Stage 3: Ray Object Store

Replace the local `OutputMemoryMap` with Ray's distributed Plasma object store.
This extends Stage 2's in-memory passing to a cluster — zero-copy data sharing
across machines without disk round-trips.

```mermaid
flowchart TD
    subgraph "OxyMake Scheduler (Rust)"
        S[Scheduler] -->|dispatch| RX[RayExecutor]
    end

    subgraph "Ray Cluster"
        subgraph "Node 1"
            W1["Worker: features"] -->|"ray.put(df)"| OS1[(Object Store<br/>Plasma)]
        end
        subgraph "Node 2"
            OS2[(Object Store<br/>Plasma)] -->|"ray.get(ref)"| W2["Worker: feature"]
        end
        OS1 -.->|"zero-copy<br/>transfer"| OS2
    end

    RX -->|"Jobs API"| W1
    RX -->|"Jobs API"| W2

    style OS1 fill:#9cf,stroke:#333
    style OS2 fill:#9cf,stroke:#333
```

**ObjectRef chaining in the Python driver:**

```mermaid
sequenceDiagram
    participant D as Python Driver
    participant OS as Ray Object Store
    participant W1 as Worker 1
    participant W2 as Worker 2

    D->>W1: features.remote(data_ref)
    W1->>OS: ray.put(features_df) → ref_A
    W1-->>D: ref_A
    D->>W2: feature.remote(ref_A)
    W2->>OS: ray.get(ref_A) → zero-copy
    W2->>OS: ray.put(feature_df) → ref_B
    W2-->>D: ref_B
    Note over OS: Data never touches disk<br/>unless MaterializePolicy=Always
```

**MaterializePolicy mapping (already designed in ray-executor.md):**

```mermaid
flowchart LR
    subgraph "MaterializePolicy"
        A["Always"] -->|"object store + shared FS"| DSK[(Disk)]
        A --> OBJ[(Object Store)]
        AU["Auto"] -->|"object store only"| OBJ
        N["Never"] -->|"object store, evict after use"| OBJ
        F["Final"] -->|"object store → FS for leaves"| DSK
        F --> OBJ
    end

    style OBJ fill:#9cf,stroke:#333
    style DSK fill:#eee,stroke:#666
```

**What changes:**
- `ox-exec-ray` driver generation (`generate_driver()`) chains ObjectRefs
  instead of writing to shared FS between tasks.
- `call`-mode jobs get zero-copy passing via `ray.put()`/`ray.get()`.
- Shell-mode jobs still use shared FS (they can't access the object store).
- The `object_manifest.json` written by each task communicates ObjectRef hex
  strings for downstream consumption.

**Effort:** ~1 week (Phase 2 infrastructure already exists in `ox-exec-ray`).
Main work: integrate ObjectRef chaining into the driver generation pipeline
and add manifest-based ref propagation.

**Prerequisites:** Stage 2 concepts (memory map abstraction), Ray cluster available.

**Expected speedup:** **3–20×** for distributed pipelines with large
intermediates. Eliminates NFS bottleneck entirely for `call`-mode jobs.
Shell-mode jobs see no improvement.

**Expected ADRs:**
- **ADR: ObjectRef chaining protocol** — How `object_manifest.json` propagates
  ObjectRef hex strings between tasks, fallback when refs are evicted, and
  interaction with `MaterializePolicy` variants.
- **ADR: Shell-mode shared-FS fallback** — Explicit contract for when jobs fall
  back to shared filesystem (shell-mode jobs, cross-node transfers without
  Plasma).

---

## Stage 4: Per-Window Splitting (53 Nodes → Full Parallelism)

The current pipeline treats each feature as a monolithic job that computes across
all window periods internally. Splitting each feature into per-window jobs
exposes more parallelism to the scheduler and enables finer-grained caching.

**Current (29 nodes) — feature is a single monolithic job:**

```mermaid
flowchart TD
    subgraph "Current: 29 Nodes"
        D1[data: dataset-A] --> F1["features: transform-1"]
        D1 --> F2["features: transform-2"]

        F1 --> A1["feature: transform-1<br/><i>(all windows internally)</i>"]
        F2 --> A2["feature: transform-2<br/><i>(all windows internally)</i>"]

        A1 --> C[combine]
        A2 --> C
        C --> OPT[optimize]
    end

    style A1 fill:#fcc,stroke:#c33
    style A2 fill:#fcc,stroke:#c33
```

**Split (53 nodes) — each window is a separate job:**

```mermaid
flowchart TD
    subgraph "Split: 53 Nodes"
        D1[data: dataset-A] --> F1["features: transform-1"]
        D1 --> F2["features: transform-2"]

        F1 --> A1a["feature: transform-1<br/>window=5"]
        F1 --> A1b["feature: transform-1<br/>window=21"]
        F1 --> A1c["feature: transform-1<br/>window=63"]

        F2 --> A2a["feature: transform-2<br/>window=5"]
        F2 --> A2b["feature: transform-2<br/>window=21"]
        F2 --> A2c["feature: transform-2<br/>window=63"]

        A1a --> C[combine]
        A1b --> C
        A1c --> C
        A2a --> C
        A2b --> C
        A2c --> C
        C --> OPT[optimize]
    end

    style A1a fill:#cfc,stroke:#3c3
    style A1b fill:#cfc,stroke:#3c3
    style A1c fill:#cfc,stroke:#3c3
    style A2a fill:#cfc,stroke:#3c3
    style A2b fill:#cfc,stroke:#3c3
    style A2c fill:#cfc,stroke:#3c3
```

**Impact on scheduling:**

```mermaid
gantt
    title Critical Path Comparison
    dateFormat X
    axisFormat %s

    section Current (29 nodes)
    data           :d1, 0, 2
    features (2)   :f1, after d1, 3
    feature — all windows (2) :a1, after f1, 9
    combine        :c1, after a1, 2
    optimize       :o1, after c1, 3

    section Split (53 nodes)
    data           :d2, 0, 2
    features (2)   :f2, after d2, 3
    feature lb=5 (×2 parallel)  :a2, after f2, 3
    feature lb=21 (×2 parallel) :a3, after f2, 3
    feature lb=63 (×2 parallel) :a4, after f2, 3
    combine        :c2, after a4, 2
    optimize       :o2, after c2, 3
```

**What changes:**
- Oxymakefile authors declare `wildcards: [window]` on feature rules, exposing
  the window dimension to OxyMake's resolver.
- The resolver creates one job per (feature, window) pair during wildcard
  expansion — no code changes to ox-core needed.
- Cache pruning operates per-(feature, window), so only changed windows
  re-execute.
- The `combine` job gains more input edges but runs the same logic.

**Effort:** ~3 days (Oxymakefile restructuring + tests). Zero changes to
`ox-core` or `ox-plan` — wildcard resolution already handles multi-dimensional
expansion.

**Prerequisites:** None — purely a workflow declaration change. Works with any
executor backend.

**Expected speedup:** **2–3×** for feature-heavy pipelines. The monolithic feature
job (which was the critical-path bottleneck) is replaced by parallel per-window
jobs, each ~1/3 the compute. Wall-clock time for the feature stage drops from
`T` to `T/N` (where N = number of windows that fit in available parallelism).

**Expected ADRs:**
- None anticipated — this stage is a workflow-level change using existing
  wildcard resolution. If splitting introduces cache-key compatibility
  questions, an ADR amendment to ADR-001 may be warranted.

---

## Stage 5: Pre-Warm Workers

Eliminate cold-start latency by pre-warming Python environments and worker
processes before upstream jobs complete. The scheduler speculatively launches
worker processes for jobs that are "almost ready" (one remaining dependency on
the critical path).

```mermaid
sequenceDiagram
    participant S as Scheduler
    participant CP as Critical Path Monitor
    participant WP as Worker Pool
    participant J1 as Job: features
    participant J2 as Job: feature

    Note over S: features running, feature next on critical path
    CP->>WP: pre-warm(feature, env=uv-science)
    WP->>WP: spawn Python, import numpy/pandas/jax
    Note over WP: Worker warm — waiting for input

    J1-->>S: features complete
    S->>WP: dispatch(feature, inputs=[features_ref])
    WP->>J2: execute (skip import — already loaded)
    Note over J2: Saves 2–5s cold start

    J2-->>S: feature complete
```

**Architecture:**

```mermaid
flowchart TD
    subgraph "Scheduler"
        S[Run Loop] --> CPM["Critical Path Monitor"]
        CPM -->|"next-on-path with 1 dep remaining"| WP
    end

    subgraph "Worker Pool"
        WP["WorkerPoolManager"] --> W1["Warm Worker 1<br/><i>Python + numpy loaded</i>"]
        WP --> W2["Warm Worker 2<br/><i>Python + jax loaded</i>"]
        WP --> W3["Warm Worker 3<br/><i>(idle, recyclable)</i>"]
    end

    subgraph "Environment Cache"
        EC["uv env cache"] --> W1
        EC --> W2
    end

    S -->|"dispatch to warm worker"| W1
    S -->|"dispatch to warm worker"| W2

    style CPM fill:#ff9,stroke:#333
    style WP fill:#9f9,stroke:#333
```

**Pre-warm timeline showing overlap:**

```mermaid
gantt
    title Pre-Warm Overlap with Upstream Compute
    dateFormat X
    axisFormat %s

    section Without Pre-Warm
    features compute     :f1, 0, 5
    feature cold start     :crit, cs1, after f1, 3
    feature compute        :a1, after cs1, 4

    section With Pre-Warm
    features compute     :f2, 0, 5
    feature pre-warm       :active, pw, 2, 5
    feature compute (warm) :a2, after f2, 4
```

**What changes:**
- New `WorkerPool` component in `ox-exec-local` manages a pool of warm
  subprocess workers per environment (keyed by `ox-env-*` environment spec).
- `CriticalPathPass` annotates "almost ready" jobs (1 pending dep on critical path).
- Scheduler sends pre-warm hints to the `WorkerPool` when a job enters
  "almost ready" state.
- `call`-mode execution reuses a warm worker instead of spawning a new process.
- Workers have a TTL (e.g., 30s idle) and are recycled to bound memory.

**For Ray executor:** Pre-warming maps to Ray's `runtime_env` warm-up. The
driver script can issue `ray.get([])` on pre-created actors to trigger import
loading before the actual data arrives.

**Effort:** ~3 weeks. New `WorkerPool` in `ox-exec-local`, scheduler hints
from `ox-plan`, environment-keyed pool management.

**Prerequisites:** Stage 2 (in-memory critical path — pre-warming without
in-memory passing just shifts the bottleneck to disk I/O after warm-up).

**Expected speedup:** **1.3–2×** for pipelines with many short `call`-mode jobs
chained on the critical path. Each cold start saves 2–5s (Python import of
numpy/pandas/jax). On a 10-stage critical path, that's 20–50s saved.

**Expected ADRs:**
- **ADR: WorkerPool lifecycle** — Pool sizing, TTL/eviction policy,
  environment-keyed isolation, resource accounting (memory budget per warm
  worker).
- **ADR: Speculative pre-warm heuristic** — When to pre-warm (1-dep-remaining
  on critical path), cost model for wasted warm-ups, interaction with
  `ox-plan`'s `CriticalPathPass`.

---

## Summary: Cumulative Roadmap

```mermaid
flowchart LR
    S1["Stage 1<br/><b>Disk Transport</b><br/>Baseline"] -->|"+in-memory map<br/>+async disk"| S2["Stage 2<br/><b>Memory Critical Path</b><br/>2–10× on CP"]
    S2 -->|"+Ray plasma<br/>+ObjectRef chain"| S3["Stage 3<br/><b>Ray Object Store</b><br/>3–20× distributed"]
    S2 -->|"+wildcard split<br/>(independent)"| S4["Stage 4<br/><b>Per-Window Split</b><br/>2–3× parallelism"]
    S2 -->|"+worker pool<br/>+speculative launch"| S5["Stage 5<br/><b>Pre-Warm Workers</b><br/>1.3–2× cold start"]

    style S1 fill:#fcc,stroke:#c33
    style S2 fill:#ffc,stroke:#cc3
    style S3 fill:#cfc,stroke:#3c3
    style S4 fill:#cfc,stroke:#3c3
    style S5 fill:#cfc,stroke:#3c3
```

| Stage | Description | Effort | Prerequisites | Expected Speedup | Scope |
|-------|-------------|--------|---------------|------------------|-------|
| **1** | Disk-based transport (current) | — | — | Baseline | — |
| **2** | In-memory critical path + async disk | ~2 weeks | None | 2–10× on critical path | `ox-core`, `ox-plan`, `ox-exec-local` |
| **3** | Ray object store integration | ~1 week | Stage 2 concepts, Ray cluster | 3–20× distributed | `ox-exec-ray` |
| **4** | Per-feature splitting (29 → 53 nodes) | ~3 days | None (workflow change) | 2–3× feature stage | Oxymakefile only |
| **5** | Pre-warm workers | ~3 weeks | Stage 2 | 1.3–2× cold start | `ox-exec-local`, `ox-plan` |

**Recommended execution order:** Stage 4 (cheapest, independent) → Stage 2
(foundational) → Stage 3 (extends Stage 2 to cluster) → Stage 5 (refinement).

Stage 4 can be done in parallel with any other stage since it requires zero
code changes to OxyMake itself.
