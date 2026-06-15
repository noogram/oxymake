//! # ox-mcp — MCP Server for OxyMake
//!
//! Implements the Model Context Protocol (MCP) over stdio, exposing OxyMake
//! commands as typed tool calls with structured JSON responses.
//!
//! The MCP server is a thin translation layer: each tool call maps to
//! existing functionality in ox-api, ox-state, and ox-format.

pub mod protocol;
pub mod server;
pub mod tools;

pub use server::{LogLevel, ServerConfig, run_stdio};
