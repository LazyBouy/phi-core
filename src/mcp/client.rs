//! High-level MCP client.
/*
ARCHITECTURE: McpClient — the MCP protocol layer

`McpClient` is a stateful wrapper that manages the full MCP connection lifecycle:
  1. Connect: `connect_stdio()` spawns a process; `connect_http()` opens an HTTP session
  2. Handshake: `initialize()` sends the `initialize` + `notifications/initialized` messages
  3. Discover: `list_tools()` fetches available tools from the server
  4. Execute: `call_tool(name, arguments)` invokes a specific tool
  5. Shutdown: `close()` kills the process or closes the HTTP connection

The MCP "handshake" is mandatory — servers refuse requests unless the client
first sends `initialize` with the protocol version and client capabilities.
After the server responds, we send `notifications/initialized` (a one-way notification).

RUST QUIRK: `Arc<Mutex<Box<dyn McpTransport>>>` — three layers of wrapping, each with a purpose

Let's unpack from the inside out:
  `Box<dyn McpTransport>`      — heap-allocated trait object (type-erased transport)
  `Mutex<Box<dyn McpTransport>>` — exclusive access lock (one request at a time)
    The MCP stdio transport is NOT safe for concurrent requests — each request/response
    must complete before the next starts. `Mutex` enforces this.
  `Arc<Mutex<Box<dyn McpTransport>>>` — shared ownership
    `McpClient` is cloned and passed to multiple `McpToolAdapter` instances.
    `Arc` lets all adapters share the same underlying transport safely.
    `.clone()` on `Arc` just bumps a reference count — cheap.

Python analogy:
  self._transport = threading.Lock()  # wrapping a transport object
  # Arc is implicit in Python (reference counting + GIL)
*/

use super::transport::{HttpTransport, McpTransport, StdioTransport, DEFAULT_REQUEST_TIMEOUT};
use super::types::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

/// Configuration knobs for [`McpClient`] construction.
///
/// Currently only carries the per-request timeout, but kept as a struct so future
/// options can be added without breaking the public API.
#[derive(Debug, Clone)]
pub struct McpClientConfig {
    /// Per-request timeout applied to every transport `send()` call.
    pub request_timeout: Duration,
}

impl Default for McpClientConfig {
    fn default() -> Self {
        Self {
            request_timeout: DEFAULT_REQUEST_TIMEOUT,
        }
    }
}

/// High-level MCP client that manages connection lifecycle and protocol.
pub struct McpClient {
    transport: Arc<Mutex<Box<dyn McpTransport>>>, // shared, locked, type-erased transport
    server_info: Option<ServerInfo>,              // populated after initialize()
    capabilities: Option<ServerCapabilities>,     // populated after initialize()
}

impl McpClient {
    /// Connect to an MCP server via stdio (spawn a child process).
    ///
    /// Uses the default per-request timeout (`DEFAULT_REQUEST_TIMEOUT`, 30 s).
    /// For a custom timeout, use [`McpClient::connect_stdio_with_config`].
    pub async fn connect_stdio(
        command: &str, // EXECUTABLE — binary to spawn as the MCP server subprocess
        args: &[&str], // ARGV — command-line args for the subprocess
        env: Option<HashMap<String, String>>, // ENV OVERRIDES — extra env vars; None = inherit parent env
    ) -> Result<Self, McpError> {
        Self::connect_stdio_with_config(command, args, env, McpClientConfig::default()).await
    }

    /// Connect to an MCP server via stdio with custom configuration.
    pub async fn connect_stdio_with_config(
        command: &str,
        args: &[&str],
        env: Option<HashMap<String, String>>,
        config: McpClientConfig,
    ) -> Result<Self, McpError> {
        let transport = StdioTransport::new(command, args, env)
            .await?
            .with_timeout(config.request_timeout);
        let mut client = Self {
            transport: Arc::new(Mutex::new(Box::new(transport))),
            server_info: None,
            capabilities: None,
        };
        client.initialize().await?;
        Ok(client)
    }

    /// Connect to an MCP server via HTTP.
    ///
    /// Uses the default per-request timeout (`DEFAULT_REQUEST_TIMEOUT`, 30 s).
    /// For a custom timeout, use [`McpClient::connect_http_with_config`].
    pub async fn connect_http(url: &str) -> Result<Self, McpError> {
        Self::connect_http_with_config(url, McpClientConfig::default()).await
    }

    /// Connect to an MCP server via HTTP with custom configuration.
    pub async fn connect_http_with_config(
        url: &str,
        config: McpClientConfig,
    ) -> Result<Self, McpError> {
        let transport = HttpTransport::new_with_timeout(url, config.request_timeout)?;
        let mut client = Self {
            transport: Arc::new(Mutex::new(Box::new(transport))),
            server_info: None,
            capabilities: None,
        };
        client.initialize().await?;
        Ok(client)
    }

    /// Create from an existing transport (useful for testing).
    pub fn from_transport(transport: Box<dyn McpTransport>) -> Self {
        Self {
            transport: Arc::new(Mutex::new(transport)),
            server_info: None,
            capabilities: None,
        }
    }

    /// Initialize the MCP connection (handshake).
    pub async fn initialize(&mut self) -> Result<ServerInfo, McpError> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": ClientInfo::default()
        });

        let request = JsonRpcRequest::new("initialize", Some(params));
        let response = self.send_request(request).await?;

        let result: InitializeResult = serde_json::from_value(response)?;
        self.server_info = Some(result.server_info.clone());
        self.capabilities = Some(result.capabilities);

        // Send initialized notification (no response expected, but we send it as a request
        // since our transport is request/response. Some servers ignore the id on notifications.)
        let notify = JsonRpcRequest::new("notifications/initialized", None);
        // Best-effort: ignore errors on the notification
        let _ = self.send_request(notify).await;

        Ok(result.server_info)
    }

    /// List available tools from the server.
    pub async fn list_tools(&self) -> Result<Vec<McpToolInfo>, McpError> {
        let request = JsonRpcRequest::new("tools/list", Some(serde_json::json!({})));
        let response = self.send_request(request).await?;

        let result: ToolsListResult = serde_json::from_value(response)?;
        Ok(result.tools)
    }

    /// Call a tool on the server.
    /*
    DESIGN: Why `name` AND `arguments` are separate parameters
      `name`      = SELECTOR — which tool on the MCP server to invoke (like a function name)
      `arguments` = INPUT    — the JSON arguments for that specific invocation
    Mirrors the same registry-vs-invocation split as AgentTool: the server has a registry of
    named tools; each call selects one by name and provides the arguments for that call.
    */
    pub async fn call_tool(
        &self,
        name: &str, // SELECTOR — tool name on the MCP server (must match tools/list result)
        arguments: serde_json::Value, // INPUT — JSON arguments for this invocation (schema defined by the server)
    ) -> Result<McpToolCallResult, McpError> {
        let params = serde_json::json!({
            "name": name,
            "arguments": arguments
        });

        let request = JsonRpcRequest::new("tools/call", Some(params));
        let response = self.send_request(request).await?;

        let result: McpToolCallResult = serde_json::from_value(response)?;
        Ok(result)
    }

    /// Close the connection.
    pub async fn close(&self) -> Result<(), McpError> {
        self.transport.lock().await.close().await
    }

    /// Get server info (available after initialize).
    pub fn server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }

    /// Send a request and extract the result value, classifying errors.
    /*
    RUST QUIRK: `self.transport.lock().await` — async mutex acquisition

    `tokio::sync::Mutex::lock()` returns a future that resolves when the lock is acquired.
    `.await` suspends the current task (not the OS thread!) until the lock is free.
    Returns `MutexGuard<Box<dyn McpTransport>>` — auto-unlocks when guard is dropped.

    The guard is held for the duration of `transport.send(request).await?`, then dropped.
    This is important: if we made TWO requests concurrently, the second would wait here
    until the first's guard drops. Serial request ordering is enforced.

    ARCHITECTURE: Error classification
    JSON-RPC errors come in two forms:
      1. Transport error: network failure, process died, parse error
         → `McpError::Transport(String)` or `McpError::Protocol(String)`
      2. Application error: server-side error (tool not found, invalid arguments)
         → `response.error` is populated, returned as `McpError::JsonRpc { code, message }`

    After extracting the result, we also handle the case where NEITHER `result` nor `error`
    is present — technically invalid JSON-RPC but defensive against buggy servers.
    */
    async fn send_request(&self, request: JsonRpcRequest) -> Result<serde_json::Value, McpError> {
        let transport = self.transport.lock().await; // acquire exclusive lock
        let response = transport.send(request).await?; // blocks until response arrives

        if let Some(error) = response.error {
            return Err(McpError::JsonRpc {
                code: error.code,
                message: error.message,
            });
        }

        response
            .result
            .ok_or_else(|| McpError::Protocol("Response has neither result nor error".into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Integration test would require a running MCP server.
    // Unit tests for the client logic are covered via mock transport in tool_adapter tests.

    #[test]
    fn test_client_info_default() {
        let info = ClientInfo::default();
        assert_eq!(info.name, "phi-core");
        assert!(!info.version.is_empty());
    }
}
