//! Micro-benchmarks for Stage 2 scheduler hot paths.
//!
//! Run with: `cargo bench -p ox-core`

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, black_box, criterion_group, criterion_main};

use ox_core::job_graph::{JobGraph, make_test_job};
use ox_core::model::{ArtifactMeta, Materialization, MaterializationSet, OutputRef};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_linear_chain(n: usize) -> (JobGraph, Vec<String>) {
    let mut jobs = Vec::with_capacity(n);
    let mut keys = Vec::with_capacity(n);
    for i in 0..n {
        let out = format!("out_{i}.bin");
        keys.push(out.clone());
        let inputs: Vec<&str> = if i > 0 {
            vec![Box::leak(format!("out_{}.bin", i - 1).into_boxed_str())]
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

// ---------------------------------------------------------------------------
// Benchmarks
// ---------------------------------------------------------------------------

/// Measure MaterializationSet lifecycle: new + add + set_artifact_meta + consumer_fired.
fn bench_materialization_overhead(c: &mut Criterion) {
    let mut group = c.benchmark_group("materialization_lifecycle");

    for n in [1, 100, 1000, 10000] {
        group.bench_with_input(BenchmarkId::new("per_output", n), &n, |b, &n| {
            b.iter(|| {
                for i in 0..n {
                    let mut ms = MaterializationSet::new(
                        OutputRef::File(PathBuf::from(format!("out_{i}.bin"))),
                        1, // 1 pending consumer
                    );
                    ms.add(Materialization::InMemory { pinned: false });
                    ms.add(Materialization::OnDisk {
                        path: PathBuf::from(format!("out_{i}.bin")),
                        verified: false,
                    });
                    let hash = [0u8; 32];
                    ms.set_artifact_meta(ArtifactMeta::new(hash, 1000));
                    ms.consumer_fired();
                    black_box(&ms);
                }
            });
        });
    }
    group.finish();
}

/// Measure BLAKE3 hashing throughput at various sizes.
fn bench_blake3_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("blake3_hash");

    for size in [1024, 65536, 1_048_576, 16_777_216] {
        let data = vec![0x42u8; size];
        let label = if size >= 1_048_576 {
            format!("{}MB", size / 1_048_576)
        } else {
            format!("{}KB", size / 1024)
        };
        group.bench_with_input(BenchmarkId::new("size", &label), &data, |b, data| {
            b.iter(|| {
                let hash = blake3::hash(black_box(data));
                black_box(hash);
            });
        });
    }
    group.finish();
}

/// Measure eviction scan time: O(N) scan for the largest evictable output.
fn bench_eviction_scan(c: &mut Criterion) {
    let mut group = c.benchmark_group("eviction_scan");

    for n in [100, 1000, 10000] {
        let (graph, keys) = build_linear_chain(n);
        let mut output_mats = graph.init_output_materializations();

        // Register all outputs with InMemory + OnDisk and make half evictable.
        for (i, key) in keys.iter().enumerate() {
            if let Some(ms) = output_mats.get_mut(key) {
                ms.add(Materialization::InMemory { pinned: false });
                ms.add(Materialization::OnDisk {
                    path: PathBuf::from(key),
                    verified: false,
                });
                ms.set_artifact_meta(ArtifactMeta::new([0u8; 32], (i as u64 + 1) * 100));
                // Make every other output evictable.
                if i % 2 == 0 {
                    while ms.pending_consumers() > 0 {
                        ms.consumer_fired();
                    }
                }
            }
        }

        group.bench_with_input(BenchmarkId::new("outputs", n), &output_mats, |b, mats| {
            b.iter(|| {
                // Simulate one eviction scan (find largest evictable).
                let largest = mats
                    .iter()
                    .filter(|(_, ms)| {
                        ms.is_evictable() && ms.has_in_memory() && ms.has_disk_fallback()
                    })
                    .max_by_key(|(_, ms)| ms.size_bytes());
                black_box(largest);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_materialization_overhead,
    bench_blake3_throughput,
    bench_eviction_scan,
);
criterion_main!(benches);
