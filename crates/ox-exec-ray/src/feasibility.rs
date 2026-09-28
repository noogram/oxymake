//! Admission against each live node's total logical capacity (never free capacity).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{error::RayError, resource_mapper::RayResources};

#[derive(Debug, Deserialize)]
pub(crate) struct NodeCapacity {
    node_ip: String,
    state: String,
    resources_total: BTreeMap<String, f64>,
}

/// Decode the State API envelope strictly: unknown or incomplete snapshots must
/// not masquerade as a cluster with no capacity.
pub(crate) fn parse_nodes(value: serde_json::Value) -> Result<Vec<NodeCapacity>, RayError> {
    let unknown = |message: &str| RayError::NodeInspectionPayload(message.into());
    if value.get("result").and_then(|v| v.as_bool()) != Some(true) {
        return Err(unknown("unknown or unsuccessful State API envelope"));
    }
    let result = value
        .pointer("/data/result")
        .ok_or_else(|| unknown("missing data.result"))?;
    let rows = result
        .get("result")
        .and_then(|v| v.as_array())
        .ok_or_else(|| unknown("missing node list"))?;
    for field in ["total", "num_after_truncation", "num_filtered"] {
        if result.get(field).and_then(|v| v.as_u64()) != Some(rows.len() as u64) {
            return Err(unknown("incomplete or truncated node snapshot"));
        }
    }
    if result
        .get("partial_failure_warning")
        .is_some_and(|v| !v.is_null() && v.as_str() != Some(""))
    {
        return Err(unknown("partial node snapshot"));
    }
    let nodes: Vec<NodeCapacity> = serde_json::from_value(serde_json::Value::Array(rows.clone()))
        .map_err(|e| unknown(&format!("invalid node schema: {e}")))?;
    for node in &nodes {
        if node.node_ip.is_empty() || !matches!(node.state.as_str(), "ALIVE" | "DEAD") {
            return Err(unknown("unknown node state or empty node address"));
        }
        if node
            .resources_total
            .values()
            .any(|n| !n.is_finite() || *n < 0.0)
        {
            return Err(unknown("node capacities must be finite and nonnegative"));
        }
    }
    Ok(nodes.into_iter().filter(|n| n.state == "ALIVE").collect())
}

pub(crate) fn check_request(
    rule: &str,
    resources: &RayResources,
    nodes: &[NodeCapacity],
) -> Result<(), RayError> {
    // An empty snapshot says nothing about what a scaling cluster can provide.
    // The caller emits one warning for the whole DAG before submitting.
    if nodes.is_empty() {
        return Ok(());
    }
    let mut request: BTreeMap<String, f64> = resources.custom.clone().into_iter().collect();
    request.insert("CPU".into(), resources.num_cpus.unwrap_or(1.0));
    request.insert("GPU".into(), resources.num_gpus.unwrap_or(0.0));
    request.retain(|_, n| *n > 0.0);
    if nodes.iter().any(|node| {
        request
            .iter()
            .all(|(key, amount)| node.resources_total.get(key).copied().unwrap_or(0.0) >= *amount)
    }) {
        return Ok(());
    }
    let render = |values: &BTreeMap<String, f64>, separator: &str| {
        values
            .iter()
            .map(|(k, v)| format!("{k}{separator}{v}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let capacities = nodes
        .iter()
        .map(|node| {
            format!(
                "{} {{{}}}",
                node.node_ip,
                render(&node.resources_total, ": ")
            )
        })
        .collect::<Vec<_>>()
        .join("; ");
    Err(RayError::InfeasibleRequest(format!(
        "rule `{rule}` requests {}; no single live node provides the complete request\n  nodes: {capacities}\n  use --ray-allow-pending to wait for a future node",
        render(&request, "="),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resource_mapper::map_resources;
    use ox_core::model::ResourceValue;
    use serde_json::json;

    fn snapshot(node: serde_json::Value) -> serde_json::Value {
        json!({"result":true,"data":{"result":{"total":1,"num_after_truncation":1,"num_filtered":1,"result":[node]}}})
    }

    #[test]
    fn total_gpu_custom_and_default_cpu_are_required_on_one_live_node() {
        let nodes = parse_nodes(snapshot(json!({"node_ip":"a","state":"ALIVE","resources_total":{"CPU":1,"GPU":1,"metal":1},"resources_available":{}}))).unwrap();
        for (key, amount) in [("cpu", 2), ("gpu", 2), ("metal", 2)] {
            let request =
                map_resources(&BTreeMap::from([(key.into(), ResourceValue::Int(amount))])).unwrap();
            assert!(check_request("a", &request, &nodes).is_err());
        }
        assert!(check_request("default", &RayResources::default(), &nodes).is_ok());
        let zero = parse_nodes(snapshot(
            json!({"node_ip":"a","state":"ALIVE","resources_total":{"CPU":0}}),
        ))
        .unwrap();
        assert!(
            check_request("default", &RayResources::default(), &zero)
                .unwrap_err()
                .to_string()
                .contains("CPU=1")
        );
        let dead = parse_nodes(snapshot(
            json!({"node_ip":"a","state":"DEAD","resources_total":{"CPU":8,"metal":4}}),
        ))
        .unwrap();
        assert!(check_request("default", &RayResources::default(), &dead).is_ok());
    }

    #[test]
    fn invalid_capacities_and_partial_snapshots_fail_closed() {
        for node in [
            json!({"node_ip":"a","state":"ALIVE"}),
            json!({"node_ip":"a","state":"ALIVE","resources_total":{"CPU":-1}}),
            json!({"node_ip":"a","state":"ALIVE","resources_total":{"CPU":"8"}}),
        ] {
            assert!(matches!(
                parse_nodes(snapshot(node)),
                Err(RayError::NodeInspectionPayload(_))
            ));
        }
        let mut partial =
            snapshot(json!({"node_ip":"a","state":"ALIVE","resources_total":{"CPU":1}}));
        partial["data"]["result"]["partial_failure_warning"] = json!("GCS incomplete");
        assert!(parse_nodes(partial).is_err());
    }

    #[test]
    fn empty_live_node_snapshot_is_not_proof_of_infeasibility() {
        let request =
            map_resources(&BTreeMap::from([("metal".into(), ResourceValue::Int(1))])).unwrap();
        assert!(check_request("a", &request, &[]).is_ok());
    }
}
