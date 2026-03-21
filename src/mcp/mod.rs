//! MCP (Model Context Protocol) client support.
//!
//! Connect to MCP tool servers and use their tools seamlessly within phi-core.
//!
//! # Example
//!
//! ```rust,no_run
//! use phi_core::mcp::McpClient;
//!
//! # async fn example() -> Result<(), phi_core::mcp::McpError> {
//! // Connect to an MCP server via stdio
//! let client = McpClient::connect_stdio("npx", &["-y", "@modelcontextprotocol/server-filesystem", "/tmp"], None).await?;
//! # Ok(())
//! # }
//! ```
/*
ARCHITECTURE: MCP — extending agents with external tool servers

The Model Context Protocol (MCP) is an open standard (from Anthropic) for connecting
AI agents to tool servers. An MCP server is an external process that exposes tools
over a standardized JSON-RPC 2.0 protocol.

Why MCP vs built-in tools?
  - MCP tools are EXTERNAL — they run in separate processes, can be in any language
  - They can expose arbitrary capabilities (filesystem, databases, APIs, browsers, ...)
  - They're reusable across different agents and frameworks
  - Official server catalog at github.com/modelcontextprotocol/servers

The MCP client in this module:
  1. Spawns or connects to an MCP server (stdio or HTTP)
  2. Performs the MCP initialization handshake (capability negotiation)
  3. Lists available tools (`tools/list`)
  4. Calls tools (`tools/call`) and returns results
  5. Wraps each MCP tool as an `McpToolAdapter` implementing `AgentTool`

This means the agent loop is completely unaware it's using MCP — it sees
`Box<dyn AgentTool>` objects, same as built-in tools.

Module layout:
  `types.rs`        — JSON-RPC 2.0 types + MCP protocol types
  `transport.rs`    — `McpTransport` trait + `StdioTransport` + `HttpTransport`
  `client.rs`       — `McpClient`: handshake + tool list + tool call
  `tool_adapter.rs` — `McpToolAdapter`: wraps an MCP tool as `AgentTool`
*/

pub mod client;
pub mod tool_adapter;
pub mod transport;
pub mod types;

pub use client::McpClient;
pub use tool_adapter::McpToolAdapter;
pub use transport::{HttpTransport, McpTransport, StdioTransport};
pub use types::{McpContent, McpError, McpToolCallResult, McpToolInfo, ServerInfo};
