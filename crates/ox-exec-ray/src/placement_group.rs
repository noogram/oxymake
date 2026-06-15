//! Ray placement group support for multi-node job scheduling.
//!
//! Placement groups allow co-scheduling of related tasks on specific node
//! topologies (STRICT_PACK, PACK, STRICT_SPREAD, SPREAD). This is useful
//! for multi-node training, distributed inference, and jobs that require
//! specific hardware locality.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Placement group scheduling strategy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlacementStrategy {
    /// All bundles on the same node (hard constraint).
    StrictPack,
    /// Try to place all bundles on the same node (soft constraint).
    #[default]
    Pack,
    /// Each bundle on a different node (hard constraint).
    StrictSpread,
    /// Try to spread bundles across nodes (soft constraint).
    Spread,
}

/// A resource bundle within a placement group.
///
/// Each bundle represents a set of resources that must be co-located.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceBundle {
    /// CPU count for this bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_cpus: Option<f64>,
    /// GPU count for this bundle (supports fractional).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub num_gpus: Option<f64>,
    /// Memory in bytes for this bundle.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<u64>,
    /// Custom resources.
    #[serde(flatten, skip_serializing_if = "HashMap::is_empty")]
    pub custom: HashMap<String, f64>,
}

impl ResourceBundle {
    /// Create a bundle with the given CPU and GPU counts.
    pub fn new(num_cpus: Option<f64>, num_gpus: Option<f64>) -> Self {
        Self {
            num_cpus,
            num_gpus,
            memory: None,
            custom: HashMap::new(),
        }
    }

    /// Convert to the JSON format Ray expects in placement group bundles.
    pub fn to_ray_bundle(&self) -> HashMap<String, f64> {
        let mut bundle = HashMap::new();
        if let Some(cpus) = self.num_cpus {
            bundle.insert("CPU".to_string(), cpus);
        }
        if let Some(gpus) = self.num_gpus {
            bundle.insert("GPU".to_string(), gpus);
        }
        if let Some(mem) = self.memory {
            bundle.insert("memory".to_string(), mem as f64);
        }
        for (k, v) in &self.custom {
            bundle.insert(k.clone(), *v);
        }
        bundle
    }
}

/// Configuration for a Ray placement group.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacementGroupConfig {
    /// Human-readable name for the placement group.
    pub name: String,
    /// Scheduling strategy.
    pub strategy: PlacementStrategy,
    /// Resource bundles to co-schedule.
    pub bundles: Vec<ResourceBundle>,
    /// Maximum time to wait for the placement group to be created (seconds).
    #[serde(default = "default_pg_timeout")]
    pub timeout_secs: u64,
}

fn default_pg_timeout() -> u64 {
    300 // 5 minutes
}

impl PlacementGroupConfig {
    /// Create a simple placement group for multi-node GPU jobs.
    ///
    /// Creates `num_nodes` bundles, each with the given GPU and CPU counts.
    pub fn multi_node_gpu(
        name: &str,
        num_nodes: usize,
        gpus_per_node: f64,
        cpus_per_node: f64,
    ) -> Self {
        let bundles = (0..num_nodes)
            .map(|_| ResourceBundle::new(Some(cpus_per_node), Some(gpus_per_node)))
            .collect();

        Self {
            name: name.to_string(),
            strategy: PlacementStrategy::StrictSpread,
            bundles,
            timeout_secs: default_pg_timeout(),
        }
    }

    /// Build the Ray API request body for creating this placement group.
    pub fn to_ray_request(&self) -> Value {
        let bundles: Vec<_> = self.bundles.iter().map(|b| b.to_ray_bundle()).collect();
        serde_json::json!({
            "name": self.name,
            "strategy": self.strategy,
            "bundles": bundles,
        })
    }
}

/// Response from placement group creation.
#[derive(Debug, Deserialize)]
pub struct PlacementGroupResponse {
    /// The placement group ID.
    pub placement_group_id: String,
}

/// Status of a placement group.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum PlacementGroupStatus {
    /// Waiting for resources.
    Pending,
    /// All bundles scheduled.
    Created,
    /// Resources removed.
    Removed,
    /// Rescheduling after failure.
    Rescheduling,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_multi_node_gpu_config() {
        let pg = PlacementGroupConfig::multi_node_gpu("train-4x", 4, 8.0, 32.0);
        assert_eq!(pg.bundles.len(), 4);
        assert_eq!(pg.strategy, PlacementStrategy::StrictSpread);
        for bundle in &pg.bundles {
            assert_eq!(bundle.num_gpus, Some(8.0));
            assert_eq!(bundle.num_cpus, Some(32.0));
        }
    }

    #[test]
    fn test_resource_bundle_to_ray() {
        let bundle = ResourceBundle {
            num_cpus: Some(4.0),
            num_gpus: Some(0.5),
            memory: Some(1024 * 1024 * 1024),
            custom: HashMap::from([("TPU".into(), 1.0)]),
        };
        let ray = bundle.to_ray_bundle();
        assert_eq!(ray["CPU"], 4.0);
        assert_eq!(ray["GPU"], 0.5);
        assert_eq!(ray["TPU"], 1.0);
        assert!(ray.contains_key("memory"));
    }

    #[test]
    fn test_placement_group_request() {
        let pg = PlacementGroupConfig::multi_node_gpu("test", 2, 1.0, 4.0);
        let req = pg.to_ray_request();
        assert_eq!(req["name"], "test");
        assert_eq!(req["bundles"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_strategy_default() {
        assert_eq!(PlacementStrategy::default(), PlacementStrategy::Pack);
    }
}
