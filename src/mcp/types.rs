//! MCP (Model Context Protocol) JSON-RPC 2.0 types.
/*
ARCHITECTURE: types.rs — the MCP wire format

MCP uses JSON-RPC 2.0 as its wire protocol. Every request is a JSON object:
  {"jsonrpc": "2.0", "id": 42, "method": "tools/list", "params": {...}}
Every response is:
  {"jsonrpc": "2.0", "id": 42, "result": {...}}  // success
  {"jsonrpc": "2.0", "id": 42, "error": {"code": -32601, "message": "..."}}  // error

The MCP protocol adds domain types on top: `McpToolInfo`, `McpToolCallResult`, etc.
These are deserialized from the `result` field of JSON-RPC responses.

`#[serde(rename_all = "camelCase")]` is used on many types because MCP's JSON
uses camelCase keys (JavaScript convention) while Rust uses snake_case.
*/

use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};

/*
RUST QUIRK: `static AtomicU64` — a global counter that's safe to use from multiple threads

`static` declares a value that lives for the entire program lifetime (not stack-allocated,
not heap-allocated via Box — it lives in the binary's `.data` segment).

`AtomicU64` is an integer with "atomic" read-modify-write operations: hardware instructions
that update the value in a single uninterruptible step. No locks needed.

`AtomicU64::new(1)` — initial value is 1.
`.fetch_add(1, Ordering::Relaxed)` — atomically increment by 1 and return the old value.
  `Ordering::Relaxed` — no memory ordering constraints (just atomicity). Correct here because
  we only need uniqueness, not ordering between different thread operations.

This lets every JSON-RPC request get a unique ID even when requests are sent
from multiple concurrent async tasks (e.g., parallel tool calls).
Python analogy: `itertools.count(1)` but thread-safe.
*/
static REQUEST_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Generate a unique, monotonically increasing JSON-RPC request ID.
pub fn next_request_id() -> u64 {
    REQUEST_ID_COUNTER.fetch_add(1, Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// JSON-RPC 2.0
// ---------------------------------------------------------------------------

/*
RUST QUIRK: `#[serde(skip_serializing_if = "Option::is_none")]`

When serializing to JSON, omit the `params` field entirely if it's `None`.
Without this, serde would serialize `None` as `"params": null`, which is
technically valid JSON-RPC but some servers reject it.
`"Option::is_none"` is a string naming the predicate function used to check.
Python analogy: a dataclass field with `json.dumps(include=...)` logic.
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String, // always "2.0"
    pub id: u64,         // unique per-request ID (from AtomicU64 counter)
    pub method: String,  // e.g. "tools/list", "tools/call", "initialize"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>, // method-specific parameters
}

impl JsonRpcRequest {
    pub fn new(method: impl Into<String>, params: Option<serde_json::Value>) -> Self {
        Self {
            jsonrpc: "2.0".into(),
            id: next_request_id(), // auto-assign unique ID
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    pub id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

// ---------------------------------------------------------------------------
// MCP Protocol types
// ---------------------------------------------------------------------------

/// Client info sent during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

impl Default for ClientInfo {
    fn default() -> Self {
        Self {
            name: "phi-core".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Server info received during initialization.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    #[serde(default)]
    pub version: String,
}

/// Server capabilities received during initialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ServerCapabilities {
    #[serde(default)]
    pub tools: Option<serde_json::Value>,
    #[serde(default)]
    pub resources: Option<serde_json::Value>,
    #[serde(default)]
    pub prompts: Option<serde_json::Value>,
}

/// Initialize result from the server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub protocol_version: String,
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfo,
}

/// MCP tool as returned by tools/list.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolInfo {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub input_schema: serde_json::Value,
}

/// tools/list result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolsListResult {
    pub tools: Vec<McpToolInfo>,
}

/// Content item in a tool call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum McpContent {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image {
        data: String,
        #[serde(rename = "mimeType")]
        mime_type: String,
    },
}

/// tools/call result.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolCallResult {
    pub content: Vec<McpContent>,
    #[serde(default)]
    pub is_error: bool,
}

// ---------------------------------------------------------------------------
// MCP Error
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("Transport error: {0}")]
    Transport(String),
    #[error("Protocol error: {0}")]
    Protocol(String),
    #[error("JSON-RPC error {code}: {message}")]
    JsonRpc { code: i64, message: String },
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Connection closed")]
    ConnectionClosed,
    #[error("Request timed out after {duration:?}")]
    Timeout { duration: std::time::Duration },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_rpc_request_serialization() {
        let req = JsonRpcRequest {
            jsonrpc: "2.0".into(),
            id: 1,
            method: "initialize".into(),
            params: Some(serde_json::json!({"protocolVersion": "2024-11-05"})),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"method\":\"initialize\""));

        let parsed: JsonRpcRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.id, 1);
        assert_eq!(parsed.method, "initialize");
    }

    #[test]
    fn test_json_rpc_response_deserialization() {
        let json = r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"test","version":"1.0"}}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.id, Some(1));
        assert!(resp.result.is_some());
        assert!(resp.error.is_none());
    }

    #[test]
    fn test_json_rpc_error_response() {
        let json =
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#;
        let resp: JsonRpcResponse = serde_json::from_str(json).unwrap();
        assert!(resp.error.is_some());
        let err = resp.error.unwrap();
        assert_eq!(err.code, -32601);
    }

    #[test]
    fn test_initialize_result_deserialization() {
        let json = r#"{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"test-server","version":"0.1.0"}}"#;
        let result: InitializeResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.server_info.name, "test-server");
        assert!(result.capabilities.tools.is_some());
    }

    #[test]
    fn test_mcp_tool_info_deserialization() {
        let json = r#"{"name":"read_file","description":"Read a file","inputSchema":{"type":"object","properties":{"path":{"type":"string"}}}}"#;
        let tool: McpToolInfo = serde_json::from_str(json).unwrap();
        assert_eq!(tool.name, "read_file");
        assert_eq!(tool.description.as_deref(), Some("Read a file"));
    }

    #[test]
    fn test_mcp_tool_call_result() {
        let json = r#"{"content":[{"type":"text","text":"file contents here"}],"isError":false}"#;
        let result: McpToolCallResult = serde_json::from_str(json).unwrap();
        assert_eq!(result.content.len(), 1);
        assert!(!result.is_error);
    }

    #[test]
    fn test_unique_request_ids() {
        let id1 = next_request_id();
        let id2 = next_request_id();
        assert_ne!(id1, id2);
    }
}
