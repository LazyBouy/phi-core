//! MCP transport implementations: stdio and HTTP+SSE.
/*
ARCHITECTURE: transport.rs — how messages travel between client and MCP server

The `McpTransport` trait abstracts the communication channel. Two implementations:

`StdioTransport` — subprocess communication via stdin/stdout
  - Spawns the MCP server as a child process
  - Sends JSON-RPC requests as newline-delimited JSON to the child's stdin
  - Reads JSON-RPC responses from the child's stdout (one line = one response)
  - Used for local servers: filesystem, git, shell, custom scripts

`HttpTransport` — HTTP POST for remote MCP servers
  - Sends requests as HTTP POST with JSON body
  - Used for remote or cloud-hosted MCP servers

Why a trait?
  The `McpClient` is generic over transport — tests can use a mock transport,
  production uses stdio or HTTP. Same pattern as StreamProvider for LLMs.

RUST QUIRK: `Arc<Mutex<tokio::process::ChildStdin>>` — async-safe shared mutable I/O

`ChildStdin` is an async write handle to the child's stdin.
It's not `Copy` or `Clone` — it's an exclusive resource.

Why `Arc<Mutex<...>>`?
  `McpTransport::send(&self, ...)` takes `&self` (shared reference).
  But we need to WRITE to stdin (mutate it). This requires interior mutability.
  `tokio::sync::Mutex` (async-aware mutex) guards `ChildStdin`:
    - Multiple concurrent `send()` calls wait for the lock (serialized)
    - No blocking — `.lock().await` yields to the tokio runtime while waiting

`Arc` wraps the mutex so `StdioTransport` can implement `Clone` cheaply (just bump
reference count), and so the struct can be shared across tasks.

RUST QUIRK: `BufReader<ChildStdout>` — buffered async reading
  `tokio::io::BufReader` wraps `ChildStdout` (raw byte stream) with line-buffering.
  `.read_line(&mut String)` reads until `\n` — used to receive one JSON-RPC response.
  Without buffering, we'd have to implement line-splitting manually.
  Python analogy: wrapping a socket with io.BufferedReader or using readline().
*/

use super::types::*;
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Transport trait for MCP communication.
/*
ARCHITECTURE: McpTransport — pluggable communication channel

Any struct that implements `McpTransport` can be used as the communication
channel for `McpClient`. The trait has two methods:
  `send(request) → response` — request/response round-trip
  `close()` — clean shutdown (kill process, close connections)

`Send + Sync` bounds are required because `McpClient` may be used from multiple
async tasks (e.g., when the agent executes tool calls in parallel).
*/
#[async_trait]
pub trait McpTransport: Send + Sync {
    /// Send a JSON-RPC request and receive the response.
    async fn send(&self, request: JsonRpcRequest) -> Result<JsonRpcResponse, McpError>;
    /// Close the transport (kill child process, close HTTP connections, etc.).
    async fn close(&self) -> Result<(), McpError>;
}

// ---------------------------------------------------------------------------
// Stdio Transport
// ---------------------------------------------------------------------------

/// Communicates with an MCP server via stdin/stdout of a child process.
/// One JSON-RPC message per line (newline-delimited JSON, i.e. NDJSON protocol).
pub struct StdioTransport {
    stdin: Arc<Mutex<tokio::process::ChildStdin>>, // write requests here
    stdout: Arc<Mutex<BufReader<tokio::process::ChildStdout>>>, // read responses here
    child: Arc<Mutex<Child>>,                      // keep handle to kill on close()
}

impl StdioTransport {
    /// Spawn a child process and create a stdio transport.
    pub async fn new(
        command: &str, // EXECUTABLE — binary to spawn as the MCP server subprocess
        args: &[&str], // ARGV — command-line arguments passed to the subprocess
        env: Option<HashMap<String, String>>, // ENV OVERRIDES — extra env vars injected into the child; None = inherit parent env
    ) -> Result<Self, McpError> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        if let Some(env_vars) = env {
            for (k, v) in env_vars {
                cmd.env(k, v);
            }
        }

        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("Failed to spawn '{}': {}", command, e)))?;

        /*
        RUST QUIRK: `child.stdin.take()` — transferring ownership of I/O handles

        `Child.stdin` is `Option<ChildStdin>`. After `spawn()`, it holds `Some(stdin)`.
        `.take()` moves the `ChildStdin` OUT of the `Option`, leaving `None` behind.
        We must `.take()` because we can't hold a `&mut` to it while also keeping `child`.
        Rust's borrow checker prevents two mutable references to overlapping data.

        `.ok_or_else(|| McpError::Transport("...".into()))` converts `Option<T>` → `Result<T, McpError>`:
          `Some(stdin)` → `Ok(stdin)`
          `None`        → `Err(McpError::Transport("Failed to capture stdin"))`
        The `?` propagates the error out if `None`.
        Python analogy: stdin = child.stdin or raise McpError("Failed to capture stdin")
        */
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture stdout".into()))?;

        Ok(Self {
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(BufReader::new(stdout))), // wrap for line-buffered reads
            child: Arc::new(Mutex::new(child)),
        })
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn send(
        &self,
        request: JsonRpcRequest, // OUTGOING — serialized to newline-terminated JSON, written to the child's stdin
    ) -> Result<JsonRpcResponse, McpError> {
        let mut line = serde_json::to_string(&request)?;
        line.push('\n');

        // Write request
        {
            let mut stdin = self.stdin.lock().await;
            stdin
                .write_all(line.as_bytes())
                .await
                .map_err(|e| McpError::Transport(format!("Write error: {}", e)))?;
            stdin
                .flush()
                .await
                .map_err(|e| McpError::Transport(format!("Flush error: {}", e)))?;
        }

        // Read response
        let mut response_line = String::new();
        {
            let mut stdout = self.stdout.lock().await;
            let bytes_read = stdout
                .read_line(&mut response_line)
                .await
                .map_err(|e| McpError::Transport(format!("Read error: {}", e)))?;
            if bytes_read == 0 {
                return Err(McpError::ConnectionClosed);
            }
        }

        let response: JsonRpcResponse = serde_json::from_str(response_line.trim())?;
        Ok(response)
    }

    async fn close(&self) -> Result<(), McpError> {
        // Drop stdin to signal EOF, then kill the child
        let mut child = self.child.lock().await;
        let _ = child.kill().await;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// HTTP Transport
// ---------------------------------------------------------------------------

/// Communicates with an MCP server via HTTP POST (JSON-RPC over HTTP).
pub struct HttpTransport {
    client: reqwest::Client,
    base_url: String,
}

impl HttpTransport {
    /// Create a new HTTP transport.
    pub fn new(url: &str) -> Result<Self, McpError> {
        let client = reqwest::Client::new();
        Ok(Self {
            client,
            base_url: url.trim_end_matches('/').to_string(),
        })
    }
}

#[async_trait]
impl McpTransport for HttpTransport {
    async fn send(
        &self,
        request: JsonRpcRequest, // OUTGOING — sent as HTTP POST body to base_url; response parsed from JSON reply
    ) -> Result<JsonRpcResponse, McpError> {
        let resp = self
            .client
            .post(&self.base_url)
            .json(&request)
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("HTTP error: {}", e)))?;

        if !resp.status().is_success() {
            return Err(McpError::Transport(format!(
                "HTTP {} from server",
                resp.status()
            )));
        }

        let response: JsonRpcResponse = resp
            .json()
            .await
            .map_err(|e| McpError::Transport(format!("Response parse error: {}", e)))?;
        Ok(response)
    }

    async fn close(&self) -> Result<(), McpError> {
        // HTTP is stateless; nothing to close.
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_stdio_transport_with_cat() {
        // Use `cat` as a simple echo server — it reflects stdin to stdout.
        let transport = StdioTransport::new("cat", &[], None).await.unwrap();

        let request = JsonRpcRequest::new("test/echo", Some(serde_json::json!({"hello": "world"})));
        let request_id = request.id;

        // Write the request; cat will echo it back as-is.
        // Since cat echoes JSON-RPC requests, the "response" will actually be the request.
        // This tests the transport layer I/O, not protocol correctness.
        let mut line = serde_json::to_string(&request).unwrap();
        line.push('\n');

        {
            let mut stdin = transport.stdin.lock().await;
            stdin.write_all(line.as_bytes()).await.unwrap();
            stdin.flush().await.unwrap();
        }

        let mut response_line = String::new();
        {
            let mut stdout = transport.stdout.lock().await;
            stdout.read_line(&mut response_line).await.unwrap();
        }

        // Cat echoes the request, so we can parse it as a request
        let echoed: JsonRpcRequest = serde_json::from_str(response_line.trim()).unwrap();
        assert_eq!(echoed.id, request_id);
        assert_eq!(echoed.method, "test/echo");

        transport.close().await.unwrap();
    }

    #[test]
    fn test_http_transport_creation() {
        let transport = HttpTransport::new("http://localhost:8080/mcp").unwrap();
        assert_eq!(transport.base_url, "http://localhost:8080/mcp");

        // Trailing slash stripped
        let transport = HttpTransport::new("http://localhost:8080/mcp/").unwrap();
        assert_eq!(transport.base_url, "http://localhost:8080/mcp");
    }
}
