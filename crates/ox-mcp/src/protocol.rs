//! MCP (Model Context Protocol) JSON-RPC types.
//!
//! Implements the subset of MCP 2024-11-05 needed for a stdio-based tool server.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// JSON-RPC 2.0 envelope
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<serde_json::Value>,
    pub method: String,
    #[serde(default)]
    pub params: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl JsonRpcResponse {
    pub fn success(id: Option<serde_json::Value>, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: Some(result),
            error: None,
        }
    }

    pub fn error(id: Option<serde_json::Value>, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message,
                data: None,
            }),
        }
    }

    pub fn method_not_found(id: Option<serde_json::Value>, method: &str) -> Self {
        Self::error(id, -32601, format!("Method not found: {method}"))
    }
}

// ---------------------------------------------------------------------------
// MCP types
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list_changed: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<ToolDefinition>,
}

#[derive(Debug, Deserialize)]
pub struct ToolCallParams {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<ToolContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "isError")]
    pub is_error: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct ToolContent {
    #[serde(rename = "type")]
    pub content_type: String,
    pub text: String,
}

impl ToolCallResult {
    pub fn text(text: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".into(),
                text,
            }],
            is_error: None,
        }
    }

    pub fn json(value: &serde_json::Value) -> Self {
        Self::text(serde_json::to_string_pretty(value).unwrap_or_default())
    }

    pub fn error(message: String) -> Self {
        Self {
            content: vec![ToolContent {
                content_type: "text".into(),
                text: message,
            }],
            is_error: Some(true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_rpc_request_deserializes() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert_eq!(req.jsonrpc, "2.0");
        assert_eq!(req.method, "tools/list");
        assert_eq!(req.id, Some(serde_json::json!(1)));
    }

    #[test]
    fn json_rpc_request_without_id() {
        let raw = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert!(req.id.is_none());
        assert_eq!(req.method, "notifications/initialized");
    }

    #[test]
    fn json_rpc_request_default_params() {
        let raw = r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let req: JsonRpcRequest = serde_json::from_str(raw).unwrap();
        assert!(req.params.is_null());
    }

    #[test]
    fn success_response_serializes() {
        let resp =
            JsonRpcResponse::success(Some(serde_json::json!(42)), serde_json::json!({"ok": true}));
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["id"], 42);
        assert_eq!(json["result"]["ok"], true);
        assert!(json.get("error").is_none());
    }

    #[test]
    fn error_response_serializes() {
        let resp =
            JsonRpcResponse::error(Some(serde_json::json!(1)), -32600, "Invalid Request".into());
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["jsonrpc"], "2.0");
        assert_eq!(json["error"]["code"], -32600);
        assert_eq!(json["error"]["message"], "Invalid Request");
        assert!(json.get("result").is_none());
    }

    #[test]
    fn method_not_found_response() {
        let resp = JsonRpcResponse::method_not_found(Some(serde_json::json!(5)), "foo/bar");
        let json = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["error"]["code"], -32601);
        assert!(
            json["error"]["message"]
                .as_str()
                .unwrap()
                .contains("foo/bar")
        );
    }

    #[test]
    fn success_response_omits_null_fields() {
        let resp = JsonRpcResponse::success(None, serde_json::json!("ok"));
        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(!json_str.contains("\"id\""));
        assert!(!json_str.contains("\"error\""));
    }

    #[test]
    fn initialize_result_serializes_camel_case() {
        let result = InitializeResult {
            protocol_version: "2024-11-05".into(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    list_changed: Some(false),
                },
            },
            server_info: ServerInfo {
                name: "test".into(),
                version: "0.1.0".into(),
            },
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["protocolVersion"], "2024-11-05");
        assert_eq!(json["serverInfo"]["name"], "test");
        assert_eq!(json["capabilities"]["tools"]["list_changed"], false);
    }

    #[test]
    fn tool_definition_serializes_camel_case() {
        let td = ToolDefinition {
            name: "ox_status".into(),
            description: "Get status".into(),
            input_schema: serde_json::json!({"type": "object"}),
        };
        let json = serde_json::to_value(&td).unwrap();
        assert_eq!(json["inputSchema"]["type"], "object");
    }

    #[test]
    fn tool_call_params_deserializes() {
        let raw = r#"{"name":"ox_status","arguments":{"group_by":"rule"}}"#;
        let params: ToolCallParams = serde_json::from_str(raw).unwrap();
        assert_eq!(params.name, "ox_status");
        assert_eq!(params.arguments["group_by"], "rule");
    }

    #[test]
    fn tool_call_params_default_arguments() {
        let raw = r#"{"name":"ox_status"}"#;
        let params: ToolCallParams = serde_json::from_str(raw).unwrap();
        assert!(params.arguments.is_null());
    }

    #[test]
    fn tool_call_result_text() {
        let r = ToolCallResult::text("hello".into());
        assert_eq!(r.content.len(), 1);
        assert_eq!(r.content[0].content_type, "text");
        assert_eq!(r.content[0].text, "hello");
        assert!(r.is_error.is_none());
    }

    #[test]
    fn tool_call_result_json() {
        let r = ToolCallResult::json(&serde_json::json!({"key": "value"}));
        assert!(r.is_error.is_none());
        let parsed: serde_json::Value = serde_json::from_str(&r.content[0].text).unwrap();
        assert_eq!(parsed["key"], "value");
    }

    #[test]
    fn tool_call_result_error() {
        let r = ToolCallResult::error("boom".into());
        assert_eq!(r.is_error, Some(true));
        assert_eq!(r.content[0].text, "boom");
    }

    #[test]
    fn tool_call_result_serializes_is_error_field() {
        let r = ToolCallResult::error("fail".into());
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["isError"], true);
    }

    #[test]
    fn tool_call_result_text_omits_is_error() {
        let r = ToolCallResult::text("ok".into());
        let json_str = serde_json::to_string(&r).unwrap();
        assert!(!json_str.contains("isError"));
    }

    #[test]
    fn tools_list_result_serializes() {
        let result = ToolsListResult {
            tools: vec![ToolDefinition {
                name: "test_tool".into(),
                description: "A test".into(),
                input_schema: serde_json::json!({}),
            }],
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["tools"].as_array().unwrap().len(), 1);
        assert_eq!(json["tools"][0]["name"], "test_tool");
    }
}
