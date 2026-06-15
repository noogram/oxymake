//! Stress tests for Stage 2 scheduler memory management.
//!
//! These tests exercise the scheduler's eviction, memory accounting, and
//! materialization tracking at scale. They are marked `#[ignore]` for CI:
//!
//! ```sh
//! cargo test -p ox-core --test stress_scheduler -- --ignored --nocapture
//! ```
//!
//! The tests operate at the `MaterializationSet` / eviction level (not the
//! full scheduler loop) to isolate memory-management behavior from I/O.

use std::path::PathBuf;
use std::sync::Arc;

use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::{ArtifactMeta, Materialization};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a linear chain DAG: job_0 → job_1 → ... → job_{n-1}.
fn build_linear_chain(n: usize, prefix: &str) -> (JobGraph, Vec<String>) {
    let mut jobs = Vec::with_capacity(n);
    let mut keys = Vec::with_capacity(n);

    for i in 0..n {
        let out = format!("{prefix}/out_{i}.bin");
        keys.push(out.clone());
        let inputs: Vec<&str> = if i > 0 {
            vec![Box::leak(
                format!("{prefix}/out_{}.bin", i - 1).into_boxed_str(),
            )]
        } else {
            vec![]
        };
        jobs.push(make_test_job(
            Box::leak(format!("job_{i}").into_boxed_str()),
            &inputs,
            &[Box::leak(out.into_boxed_str())],
        ));
    }

    (JobGraph::build(jobs).unwrap(), keys)
}

/// Build a diamond DAG: root → {middle_0..middle_{n-1}} → sink.
fn build_diamond(n: usize, prefix: &str) -> (JobGraph, Vec<String>) {
    let root_out = format!("{prefix}/root.bin");
    let sink_out = format!("{prefix}/sink.bin");
    let mut keys = vec![root_out.clone()];

    let mut jobs = vec![make_test_job(
        "root",
        &[],
        &[Box::leak(root_out.clone().into_boxed_str())],
    )];

    let mut middle_keys = Vec::new();
    for i in 0..n {
        let out = format!("{prefix}/mid_{i}.bin");
        middle_keys.push(out.clone());
        keys.push(out.clone());
        jobs.push(make_test_job(
            Box::leak(format!("mid_{i}").into_boxed_str()),
            &[Box::leak(root_out.clone().into_boxed_str())],
            &[Box::leak(out.into_boxed_str())],
        ));
    }

    let middle_leaked: Vec<&str> = middle_keys
        .iter()
        .map(|s| Box::leak(s.clone().into_boxed_str()) as &str)
        .collect();
    let middle_refs: Vec<&str> = middle_leaked.iter().copied().collect();
    keys.push(sink_out.clone());
    jobs.push(make_test_job(
        "sink",
        &middle_refs,
        &[Box::leak(sink_out.into_boxed_str())],
    ));

    (JobGraph::build(jobs).unwrap(), keys)
}

// ---------------------------------------------------------------------------
// Stress tests
// ---------------------------------------------------------------------------

/// 1000-job linear chain with 10KB outputs and 100KB budget.
///
/// Forces ~90% eviction rate. Validates accounting consistency
/// under sustained eviction pressure.
#[test]
#[ignore]
fn stress_linear_1000_jobs_tight_budget() {
    let n = 1000;
    let output_size = 10_000u64; // 10KB
    let budget = 100_000u64; // 100KB

    let (graph, keys) = build_linear_chain(n, "linear");
    let _topo = graph.topological_order().unwrap();

    // Manually create Frontier-like tracking.
    let mut output_mats = graph.init_output_materializations();
    let mut memory_store: std::collections::HashMap<String, Arc<[u8]>> =
        std::collections::HashMap::new();
    let mut memory_used: u64 = 0;
    let mut peak_memory: u64 = 0;
    let mut eviction_count: usize = 0;

    for (i, key) in keys.iter().enumerate() {
        // Simulate register_materializations: add InMemory + OnDisk.
        if let Some(ms) = output_mats.get_mut(key) {
            let data: Arc<[u8]> = Arc::from(vec![0x42u8; output_size as usize]);
            let hash = blake3::hash(&data);
            ms.set_artifact_meta(ArtifactMeta::new(*hash.as_bytes(), output_size));
            ms.add(Materialization::InMemory { pinned: false });
            ms.add(Materialization::OnDisk {
                path: PathBuf::from(key),
                verified: false,
            });
            memory_store.insert(key.clone(), data);
            memory_used += output_size;
            if memory_used > peak_memory {
                peak_memory = memory_used;
            }
        }

        // Simulate consumer_fired for previous output.
        if i > 0 {
            let prev = &keys[i - 1];
            if let Some(ms) = output_mats.get_mut(prev) {
                ms.consumer_fired();
            }
        }

        // Simulate enforce_memory_budget.
        while memory_used > budget {
            let largest = output_mats
                .iter()
                .filter(|(_, ms)| ms.is_evictable() && ms.has_in_memory() && ms.has_disk_fallback())
                .max_by_key(|(_, ms)| ms.size_bytes())
                .map(|(k, ms)| (k.clone(), ms.size_bytes()));

            match largest {
                Some((k, size)) => {
                    if let Some(ms) = output_mats.get_mut(&k) {
                        if ms.evict_in_memory() {
                            memory_used = memory_used.saturating_sub(size);
                            memory_store.remove(&k);
                            eviction_count += 1;
                        } else {
                            break;
                        }
                    }
                }
                None => break,
            }
        }

        // Invariant: accounting matches reality.
        let actual: u64 = memory_store.values().map(|v| v.len() as u64).sum();
        assert_eq!(
            memory_used, actual,
            "accounting drift at job {i}: tracked={memory_used}, actual={actual}"
        );
    }

    eprintln!(
        "  Linear 1000: peak={:.0}KB, budget={:.0}KB, evictions={eviction_count}",
        peak_memory as f64 / 1024.0,
        budget as f64 / 1024.0,
    );

    assert!(eviction_count > 0, "evictions should have occurred");
    assert!(
        peak_memory <= budget + output_size * 2,
        "peak {peak_memory} should be bounded by budget {budget} + margin"
    );
}

/// 500-wide diamond (1→500→1) with 50KB outputs and 1MB budget.
///
/// Tests parallel eviction pressure: 500 middle outputs all become
/// evictable at the same time (when sink consumes them).
#[test]
#[ignore]
fn stress_diamond_500_wide() {
    let n = 500;
    let output_size = 50_000u64; // 50KB
    let budget = 1_000_000u64; // 1MB

    let (graph, keys) = build_diamond(n, "diamond");
    let _topo = graph.topological_order().unwrap();
    let mut output_mats = graph.init_output_materializations();
    let mut memory_store: std::collections::HashMap<String, Arc<[u8]>> =
        std::collections::HashMap::new();
    let mut memory_used: u64 = 0;
    let mut eviction_count: usize = 0;

    // Register all outputs.
    for key in &keys {
        if let Some(ms) = output_mats.get_mut(key) {
            let data: Arc<[u8]> = Arc::from(vec![0x42u8; output_size as usize]);
            ms.add(Materialization::InMemory { pinned: false });
            ms.add(Materialization::OnDisk {
                path: PathBuf::from(key),
                verified: false,
            });
            ms.set_artifact_meta(ArtifactMeta::new([0u8; 32], output_size));
            memory_store.insert(key.clone(), data);
            memory_used += output_size;
        }
    }

    let peak_memory = memory_used;
    eprintln!(
        "  Diamond 500: total_memory={:.1}MB before eviction",
        peak_memory as f64 / (1024.0 * 1024.0),
    );

    // Simulate: root is consumed by all middle jobs.
    if let Some(ms) = output_mats.get_mut(&keys[0]) {
        for _ in 0..n {
            ms.consumer_fired();
        }
    }

    // Simulate: each middle job is consumed by sink.
    for i in 1..=n {
        if let Some(ms) = output_mats.get_mut(&keys[i]) {
            ms.consumer_fired();
        }
    }

    // Now enforce budget — many outputs should be evictable.
    while memory_used > budget {
        let largest = output_mats
            .iter()
            .filter(|(_, ms)| ms.is_evictable() && ms.has_in_memory() && ms.has_disk_fallback())
            .max_by_key(|(_, ms)| ms.size_bytes())
            .map(|(k, ms)| (k.clone(), ms.size_bytes()));

        match largest {
            Some((k, size)) => {
                if let Some(ms) = output_mats.get_mut(&k) {
                    if ms.evict_in_memory() {
                        memory_used = memory_used.saturating_sub(size);
                        memory_store.remove(&k);
                        eviction_count += 1;
                    } else {
                        break;
                    }
                }
            }
            None => break,
        }
    }

    eprintln!(
        "  Diamond 500: after eviction: used={:.1}MB, evictions={eviction_count}",
        memory_used as f64 / (1024.0 * 1024.0),
    );

    assert!(eviction_count > 0, "evictions should have occurred");
    assert!(
        memory_used <= budget + output_size,
        "memory {memory_used} should be within budget {budget}"
    );

    // Accounting check.
    let actual: u64 = memory_store.values().map(|v| v.len() as u64).sum();
    assert_eq!(memory_used, actual, "accounting drift after eviction");
}
