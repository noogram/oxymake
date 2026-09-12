//! Shared cache and staleness analysis for `plan` and `run`.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use ox_cache::{CacheHitStatus, CacheStore, CacheValidation};
use ox_cache_remote::RemoteCache;
use ox_core::job_graph::JobGraph;
use ox_core::model::{JobId, RunReason};

use super::run::{
    input_file_paths, job_cache_key_with_components, job_provenance, output_file_paths,
    restore_remote_outputs,
};

/// The jobs satisfied by cache and the reason every other job must execute.
pub struct ExecutionPlan {
    pub skip_jobs: HashSet<JobId>,
    pub run_reasons: HashMap<JobId, RunReason>,
    /// What each cache hit was keyed on, for the audit trail (#12). Only
    /// hits appear here: a job that runs is keyed again by the scheduler.
    pub provenance: HashMap<String, ox_state::db::JobProvenance>,
}

/// Determine which jobs would execute for the current filesystem and cache.
///
/// This is the single selection pass used by both `ox plan` and `ox run`.
/// Invariant: for any state of the tree, plan's job count equals the number of
/// jobs run would execute with the same cache settings.
/// When a job is stale, every downstream job is also selected because its
/// input will be rebuilt even if its current output has a valid cache entry.
pub fn determine_execution(
    job_graph: &JobGraph,
    cache_enabled: bool,
    validation: CacheValidation,
    mut store: Option<&mut CacheStore>,
    remote: Option<(&dyn RemoteCache, &tokio::runtime::Runtime)>,
) -> ExecutionPlan {
    let mut skip_jobs = HashSet::new();
    let mut run_reasons = HashMap::new();
    let mut provenance = HashMap::new();

    if !cache_enabled {
        for job_id in job_graph.job_ids() {
            run_reasons.insert(job_id.clone(), RunReason::CacheDisabled);
        }
        return ExecutionPlan {
            skip_jobs,
            run_reasons,
            provenance,
        };
    }

    let mut stale_jobs = HashSet::new();
    if let Ok(topo) = job_graph.topological_order() {
        for job_id in topo {
            if stale_jobs.contains(job_id) {
                run_reasons.insert(job_id.clone(), RunReason::UpstreamRebuilt);
                for downstream in job_graph.downstream(job_id) {
                    stale_jobs.insert(downstream.clone());
                }
                continue;
            }

            let mut is_hit = false;
            if let Some(job) = job_graph.get_job(job_id) {
                let output_paths = output_file_paths(job);
                let output_refs: Vec<&Path> = output_paths.iter().map(|p| p.as_path()).collect();

                if output_paths.is_empty() {
                    run_reasons.insert(job_id.clone(), RunReason::NotCacheable);
                } else if validation == CacheValidation::Mtime {
                    let input_paths = input_file_paths(job);
                    let input_refs: Vec<&Path> = input_paths.iter().map(|p| p.as_path()).collect();
                    match CacheStore::check_mtime_stateless(&input_refs, &output_refs) {
                        Ok(CacheHitStatus::Hit) => {
                            is_hit = true;
                            skip_jobs.insert(job_id.clone());
                        }
                        Ok(CacheHitStatus::Mismatch { path }) => {
                            run_reasons.insert(job_id.clone(), RunReason::OutputStale { path });
                        }
                        Ok(CacheHitStatus::OutputMissing { path }) => {
                            run_reasons.insert(job_id.clone(), RunReason::OutputMissing { path });
                        }
                        Ok(CacheHitStatus::Miss) | Err(_) => {
                            run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                        }
                    }
                } else if let Some(cache_store) = store.as_deref_mut() {
                    if let Some(components) =
                        job_cache_key_with_components(job, Some(&mut *cache_store))
                    {
                        let cache_key = components.cache_key.clone();
                        let status = if let Some((remote_cache, runtime)) = remote {
                            if runtime.block_on(restore_remote_outputs(
                                remote_cache,
                                cache_store,
                                &cache_key,
                                &output_paths,
                            )) {
                                CacheHitStatus::Hit
                            } else {
                                cache_store
                                    .check_cached(&cache_key, &output_refs)
                                    .unwrap_or(CacheHitStatus::Miss)
                            }
                        } else {
                            cache_store
                                .check_cached(&cache_key, &output_refs)
                                .unwrap_or(CacheHitStatus::Miss)
                        };
                        match status {
                            CacheHitStatus::Hit => {
                                is_hit = true;
                                skip_jobs.insert(job_id.clone());
                                let entry = cache_store.get(&cache_key);
                                provenance.insert(
                                    job_id.as_str().to_string(),
                                    job_provenance(
                                        job,
                                        &components,
                                        entry.as_ref().map(|e| &e.output_hashes),
                                    ),
                                );
                            }
                            CacheHitStatus::Mismatch { path } => {
                                run_reasons.insert(job_id.clone(), RunReason::OutputStale { path });
                            }
                            CacheHitStatus::OutputMissing { path } => {
                                run_reasons
                                    .insert(job_id.clone(), RunReason::OutputMissing { path });
                            }
                            CacheHitStatus::Miss => {
                                run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                            }
                        }
                    } else {
                        run_reasons.insert(job_id.clone(), RunReason::NotCacheable);
                    }
                } else {
                    run_reasons.insert(job_id.clone(), RunReason::CacheMiss);
                }
            }

            if !is_hit {
                for downstream in job_graph.downstream(job_id) {
                    stale_jobs.insert(downstream.clone());
                }
            }
        }
    }

    ExecutionPlan {
        skip_jobs,
        run_reasons,
        provenance,
    }
}
