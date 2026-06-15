//! Autoscaler-aware concurrency management.
//!
//! Queries the Ray cluster state to determine available resources and
//! dynamically adjust concurrency limits. This prevents over-submission
//! when the autoscaler is still scaling up, and enables higher throughput
//! when capacity is available.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::Mutex;

/// Cluster node information from the Ray API.
#[derive(Debug, Clone, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier.
    #[serde(rename = "nodeId")]
    pub node_id: String,
    /// Whether this node is alive.
    #[serde(rename = "isHeadNode")]
    pub is_head_node: bool,
    /// Current state of the node.
    pub state: String,
    /// Resources available on this node.
    #[serde(default, rename = "resourcesTotal")]
    pub resources_total: HashMap<String, f64>,
    /// Resources currently available (not allocated).
    #[serde(default, rename = "resourcesAvailable")]
    pub resources_available: HashMap<String, f64>,
}

/// Summary of cluster-wide resource availability.
#[derive(Debug, Clone, Default)]
pub struct ClusterResources {
    /// Total CPUs across all alive nodes.
    pub total_cpus: f64,
    /// Available (unallocated) CPUs.
    pub available_cpus: f64,
    /// Total GPUs across all alive nodes.
    pub total_gpus: f64,
    /// Available (unallocated) GPUs.
    pub available_gpus: f64,
    /// Total memory bytes.
    pub total_memory: f64,
    /// Available memory bytes.
    pub available_memory: f64,
    /// Number of alive nodes.
    pub alive_nodes: usize,
    /// Number of pending (scaling up) nodes.
    pub pending_nodes: usize,
}

/// Response from the Ray nodes API.
#[derive(Debug, Deserialize)]
pub struct NodesResponse {
    /// List of nodes in the cluster.
    #[serde(default)]
    pub data: Vec<NodeInfo>,
}

/// Autoscaler-aware concurrency advisor.
///
/// Periodically queries cluster state and recommends a concurrency limit
/// based on available resources. This allows the executor to avoid
/// overwhelming a cluster that's still scaling up.
#[derive(Debug)]
pub struct AutoscalerAdvisor {
    /// Base concurrency (user-configured max_submit).
    base_concurrency: Option<usize>,
    /// Minimum concurrency even when cluster is under-provisioned.
    min_concurrency: usize,
    /// How often to refresh cluster state.
    refresh_interval: Duration,
    /// Cached cluster resources.
    cached: Mutex<CachedState>,
}

#[derive(Debug)]
struct CachedState {
    resources: Option<ClusterResources>,
    last_refresh: Option<Instant>,
}

impl AutoscalerAdvisor {
    /// Create a new autoscaler advisor.
    pub fn new(base_concurrency: Option<usize>, min_concurrency: usize) -> Self {
        Self {
            base_concurrency,
            min_concurrency,
            refresh_interval: Duration::from_secs(30),
            cached: Mutex::new(CachedState {
                resources: None,
                last_refresh: None,
            }),
        }
    }

    /// Update the cached cluster resources.
    pub async fn update(&self, resources: ClusterResources) {
        let mut cached = self.cached.lock().await;
        cached.resources = Some(resources);
        cached.last_refresh = Some(Instant::now());
    }

    /// Check if the cached state needs refreshing.
    pub async fn needs_refresh(&self) -> bool {
        let cached = self.cached.lock().await;
        match cached.last_refresh {
            None => true,
            Some(t) => t.elapsed() > self.refresh_interval,
        }
    }

    /// Recommend a concurrency limit based on cluster state.
    ///
    /// The recommendation considers:
    /// 1. User-configured base concurrency (ceiling)
    /// 2. Available CPUs (each job needs at least 1 CPU)
    /// 3. Pending nodes (scale-up headroom)
    pub async fn recommended_concurrency(&self) -> Option<usize> {
        let cached = self.cached.lock().await;
        let resources = match &cached.resources {
            Some(r) => r,
            None => return self.base_concurrency,
        };

        // Estimate capacity: available CPUs + pending node capacity.
        // Assume each pending node brings the average per-node CPU count.
        let avg_cpus_per_node = if resources.alive_nodes > 0 {
            resources.total_cpus / resources.alive_nodes as f64
        } else {
            0.0
        };

        let pending_cpu_capacity = resources.pending_nodes as f64 * avg_cpus_per_node;
        let effective_cpus = resources.available_cpus + pending_cpu_capacity;

        // Each job needs at least 1 CPU (Ray default).
        let cpu_based_limit = effective_cpus.floor() as usize;

        // Apply ceiling from user config and floor from min_concurrency.
        let limit = match self.base_concurrency {
            Some(base) => cpu_based_limit.min(base),
            None => cpu_based_limit,
        };

        Some(limit.max(self.min_concurrency))
    }
}

/// Aggregate node-level resources into cluster-wide totals.
pub fn aggregate_cluster_resources(nodes: &[NodeInfo]) -> ClusterResources {
    let mut result = ClusterResources::default();

    for node in nodes {
        if node.state != "ALIVE" {
            if node.state == "PENDING" {
                result.pending_nodes += 1;
            }
            continue;
        }

        result.alive_nodes += 1;
        result.total_cpus += node.resources_total.get("CPU").copied().unwrap_or(0.0);
        result.available_cpus += node.resources_available.get("CPU").copied().unwrap_or(0.0);
        result.total_gpus += node.resources_total.get("GPU").copied().unwrap_or(0.0);
        result.available_gpus += node.resources_available.get("GPU").copied().unwrap_or(0.0);
        result.total_memory += node.resources_total.get("memory").copied().unwrap_or(0.0);
        result.available_memory += node
            .resources_available
            .get("memory")
            .copied()
            .unwrap_or(0.0);
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(cpus_total: f64, cpus_avail: f64, gpus_total: f64, state: &str) -> NodeInfo {
        NodeInfo {
            node_id: "node-1".into(),
            is_head_node: false,
            state: state.into(),
            resources_total: HashMap::from([
                ("CPU".into(), cpus_total),
                ("GPU".into(), gpus_total),
            ]),
            resources_available: HashMap::from([("CPU".into(), cpus_avail)]),
        }
    }

    #[test]
    fn test_aggregate_alive_nodes() {
        let nodes = vec![
            make_node(16.0, 8.0, 4.0, "ALIVE"),
            make_node(16.0, 4.0, 4.0, "ALIVE"),
            make_node(16.0, 16.0, 4.0, "PENDING"),
        ];
        let res = aggregate_cluster_resources(&nodes);
        assert_eq!(res.alive_nodes, 2);
        assert_eq!(res.pending_nodes, 1);
        assert_eq!(res.total_cpus, 32.0);
        assert_eq!(res.available_cpus, 12.0);
        assert_eq!(res.total_gpus, 8.0);
    }

    #[tokio::test]
    async fn test_advisor_no_state() {
        let advisor = AutoscalerAdvisor::new(Some(10), 1);
        // Without cluster state, returns base concurrency.
        assert_eq!(advisor.recommended_concurrency().await, Some(10));
    }

    #[tokio::test]
    async fn test_advisor_with_state() {
        let advisor = AutoscalerAdvisor::new(Some(100), 2);
        advisor
            .update(ClusterResources {
                total_cpus: 32.0,
                available_cpus: 16.0,
                total_gpus: 8.0,
                available_gpus: 4.0,
                total_memory: 0.0,
                available_memory: 0.0,
                alive_nodes: 2,
                pending_nodes: 0,
            })
            .await;
        let rec = advisor.recommended_concurrency().await.unwrap();
        // 16 available CPUs, capped at 100 base → 16.
        assert_eq!(rec, 16);
    }

    #[tokio::test]
    async fn test_advisor_min_concurrency() {
        let advisor = AutoscalerAdvisor::new(Some(10), 5);
        advisor
            .update(ClusterResources {
                available_cpus: 2.0,
                alive_nodes: 1,
                total_cpus: 4.0,
                ..ClusterResources::default()
            })
            .await;
        let rec = advisor.recommended_concurrency().await.unwrap();
        // 2 available CPUs, but min_concurrency is 5.
        assert_eq!(rec, 5);
    }

    #[tokio::test]
    async fn test_advisor_with_pending_nodes() {
        let advisor = AutoscalerAdvisor::new(None, 1);
        advisor
            .update(ClusterResources {
                total_cpus: 16.0,
                available_cpus: 4.0,
                alive_nodes: 1,
                pending_nodes: 2,
                ..ClusterResources::default()
            })
            .await;
        let rec = advisor.recommended_concurrency().await.unwrap();
        // 4 available + 2 pending nodes × 16 avg cpus = 36.
        assert_eq!(rec, 36);
    }
}
