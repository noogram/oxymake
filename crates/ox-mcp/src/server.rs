//! MCP server — stdio JSON-RPC event loop.
//!
//! Reads JSON-RPC requests line-by-line from stdin, dispatches them, and
//! writes responses to stdout. Diagnostic output goes to stderr.

use std::io::{BufRead, Write};
use std::path::PathBuf;

use anyhow::Result;
use serde_json::json;

use crate::protocol::{
    InitializeResult, JsonRpcRequest, JsonRpcResponse, ServerCapabilities, ServerInfo,
    ToolCallParams, ToolsCapability, ToolsListResult,
};
use crate::tools;

/// Configuration for the MCP server.
pub struct ServerConfig {
    /// Working directory for Oxymakefile resolution.
    pub workdir: PathBuf,
    /// Log level for stderr diagnostics.
    pub log_level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Quiet,
    Info,
    Debug,
}

/// Run the MCP server over stdio (blocking).
pub fn run_stdio(config: ServerConfig) -> Result<()> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    let mut reader = stdin.lock();
    let mut writer = stdout.lock();

    log(&config, LogLevel::Info, "OxyMake MCP server started");
    log(
        &config,
        LogLevel::Info,
        &format!("Working directory: {}", config.workdir.display()),
    );

    let mut line = String::new();
    loop {
        line.clear();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            // EOF — client disconnected
            log(&config, LogLevel::Info, "Client disconnected (EOF)");
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        log(&config, LogLevel::Debug, &format!("< {trimmed}"));

        let request: JsonRpcRequest = match serde_json::from_str(trimmed) {
            Ok(r) => r,
            Err(e) => {
                let resp = JsonRpcResponse::error(None, -32700, format!("Parse error: {e}"));
                write_response(&mut writer, &resp, &config)?;
                continue;
            }
        };

        // Notifications (no id) don't get responses
        let is_notification = request.id.is_none();

        let response = dispatch(&request, &config);

        if !is_notification {
            if let Some(resp) = response {
                write_response(&mut writer, &resp, &config)?;
            }
        }
    }

    Ok(())
}

fn dispatch(request: &JsonRpcRequest, config: &ServerConfig) -> Option<JsonRpcResponse> {
    match request.method.as_str() {
        "initialize" => Some(handle_initialize(request)),
        "notifications/initialized" => {
            log(config, LogLevel::Debug, "Client initialized");
            None // notification, no response
        }
        "tools/list" => Some(handle_tools_list(request)),
        "tools/call" => Some(handle_tools_call(request, config)),
        "ping" => Some(JsonRpcResponse::success(request.id.clone(), json!({}))),
        _ => {
            // Unknown methods that are notifications (no id) are ignored
            if request.id.is_some() {
                Some(JsonRpcResponse::method_not_found(
                    request.id.clone(),
                    &request.method,
                ))
            } else {
                None
            }
        }
    }
}

fn handle_initialize(request: &JsonRpcRequest) -> JsonRpcResponse {
    let result = InitializeResult {
        protocol_version: "2024-11-05".into(),
        capabilities: ServerCapabilities {
            tools: ToolsCapability {
                list_changed: Some(false),
            },
        },
        server_info: ServerInfo {
            name: "ox-mcp".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        },
    };

    JsonRpcResponse::success(
        request.id.clone(),
        serde_json::to_value(result).unwrap_or_default(),
    )
}

fn handle_tools_list(request: &JsonRpcRequest) -> JsonRpcResponse {
    let catalog = tools::tool_catalog();
    let result = ToolsListResult { tools: catalog };
    JsonRpcResponse::success(
        request.id.clone(),
        serde_json::to_value(result).unwrap_or_default(),
    )
}

fn handle_tools_call(request: &JsonRpcRequest, config: &ServerConfig) -> JsonRpcResponse {
    let params: ToolCallParams = match serde_json::from_value(request.params.clone()) {
        Ok(p) => p,
        Err(e) => {
            return JsonRpcResponse::error(
                request.id.clone(),
                -32602,
                format!("Invalid params: {e}"),
            );
        }
    };

    log(
        config,
        LogLevel::Debug,
        &format!("Tool call: {} args={}", params.name, params.arguments),
    );

    let result = tools::handle_tool_call(&params.name, &params.arguments, &config.workdir);

    JsonRpcResponse::success(
        request.id.clone(),
        serde_json::to_value(result).unwrap_or_default(),
    )
}

fn write_response(
    writer: &mut impl Write,
    response: &JsonRpcResponse,
    config: &ServerConfig,
) -> Result<()> {
    let json = serde_json::to_string(response)?;
    log(config, LogLevel::Debug, &format!("> {json}"));
    writeln!(writer, "{json}")?;
    writer.flush()?;
    Ok(())
}

fn log(config: &ServerConfig, level: LogLevel, message: &str) {
    if level_enabled(config.log_level, level) {
        let prefix = match level {
            LogLevel::Quiet => "",
            LogLevel::Info => "[ox-mcp] ",
            LogLevel::Debug => "[ox-mcp:debug] ",
        };
        eprintln!("{prefix}{message}");
    }
}

fn level_enabled(configured: LogLevel, requested: LogLevel) -> bool {
    match configured {
        LogLevel::Quiet => false,
        LogLevel::Info => matches!(requested, LogLevel::Info),
        LogLevel::Debug => matches!(requested, LogLevel::Info | LogLevel::Debug),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_quiet_blocks_all() {
        assert!(!level_enabled(LogLevel::Quiet, LogLevel::Info));
        assert!(!level_enabled(LogLevel::Quiet, LogLevel::Debug));
        assert!(!level_enabled(LogLevel::Quiet, LogLevel::Quiet));
    }

    #[test]
    fn level_info_passes_info_only() {
        assert!(level_enabled(LogLevel::Info, LogLevel::Info));
        assert!(!level_enabled(LogLevel::Info, LogLevel::Debug));
    }

    #[test]
    fn level_debug_passes_info_and_debug() {
        assert!(level_enabled(LogLevel::Debug, LogLevel::Info));
        assert!(level_enabled(LogLevel::Debug, LogLevel::Debug));
    }

    #[test]
    fn dispatch_initialize() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(1)),
            method: "initialize".into(),
            params: serde_json::json!({}),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        let result = resp.result.unwrap();
        assert_eq!(result["protocolVersion"], "2024-11-05");
        assert_eq!(result["serverInfo"]["name"], "ox-mcp");
    }

    #[test]
    fn dispatch_ping() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(2)),
            method: "ping".into(),
            params: serde_json::json!(null),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn dispatch_tools_list() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(3)),
            method: "tools/list".into(),
            params: serde_json::json!({}),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        let result = resp.result.unwrap();
        let tools = result["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
    }

    #[test]
    fn dispatch_notification_returns_none() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: "notifications/initialized".into(),
            params: serde_json::json!(null),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        assert!(dispatch(&req, &config).is_none());
    }

    #[test]
    fn dispatch_unknown_method_returns_error() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(99)),
            method: "bogus/method".into(),
            params: serde_json::json!(null),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32601);
    }

    #[test]
    fn dispatch_unknown_notification_ignored() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: None,
            method: "bogus/notification".into(),
            params: serde_json::json!(null),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        assert!(dispatch(&req, &config).is_none());
    }

    #[test]
    fn dispatch_tools_call_valid_tool() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(20)),
            method: "tools/call".into(),
            params: serde_json::json!({
                "name": "ox_status",
                "arguments": {}
            }),
        };
        let dir = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            workdir: dir.path().into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        // Should return a success response (tool handles missing db gracefully)
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn dispatch_tools_call_unknown_tool() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(21)),
            method: "tools/call".into(),
            params: serde_json::json!({
                "name": "nonexistent_tool",
                "arguments": {}
            }),
        };
        let dir = tempfile::tempdir().unwrap();
        let config = ServerConfig {
            workdir: dir.path().into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        // Unknown tool returns a success JSON-RPC response with error content in the tool result
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn dispatch_tools_call_invalid_params() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: Some(serde_json::json!(10)),
            method: "tools/call".into(),
            params: serde_json::json!("not an object"),
        };
        let config = ServerConfig {
            workdir: "/tmp".into(),
            log_level: LogLevel::Quiet,
        };
        let resp = dispatch(&req, &config).unwrap();
        assert!(resp.error.is_some());
        assert_eq!(resp.error.unwrap().code, -32602);
    }
}
