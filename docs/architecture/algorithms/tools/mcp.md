<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `McpClient::initialize` *(src/mcp/)*

**Purpose:** Perform the 3-step MCP handshake to establish a session with a tool server.

```
FUNCTION McpClient::connect_stdio(
  command: String,
  args: Vec<String>,
  env: Option<Map<String,String>>
) -> Result<McpClient, McpError>

  // Spawn child process
  process ← spawn_process(command, args, env,
    stdin=piped, stdout=piped, stderr=inherit)
  // McpError::Transport on spawn failure

  transport ← StdioTransport { process }
  client ← McpClient { transport: Arc(Mutex(transport)), server_info: None }

  AWAIT client.initialize()
  RETURN Ok(client)

END FUNCTION

FUNCTION McpClient::initialize() -> Result<ServerInfo, McpError>

  // Step 1: send initialize
  result ← AWAIT self.send_request("initialize", {
    protocolVersion: "2024-11-05",
    capabilities: {},
    clientInfo: { name: "phi-core", version: CARGO_PKG_VERSION }
  })
  // Deserialize result as InitializeResult { protocolVersion, capabilities, serverInfo }

  self.server_info ← Some(result.serverInfo)

  // Step 2: send notifications/initialized (no params)
  AWAIT self.send_request("notifications/initialized", None)
  // Server may ignore the response id for this notification

  RETURN Ok(result.serverInfo)

END FUNCTION

FUNCTION McpClient::send_request(method: String, params: Option<Value>) -> Result<Value, McpError>

  request ← JsonRpcRequest {
    jsonrpc: "2.0",
    id: ATOMIC_COUNTER.fetch_add(1),  // monotonically increasing from 1
    method,
    params
  }

  response ← AWAIT self.transport.send(request)

  IF response.error is Some THEN
    RETURN Err(JsonRpc { code: error.code, message: error.message })
  END IF

  IF response.result is None THEN
    RETURN Err(Protocol("Empty result"))
  END IF

  RETURN Ok(response.result)

END FUNCTION

FUNCTION McpClient::list_tools() -> Result<Vec<McpToolInfo>, McpError>
  result ← AWAIT self.send_request("tools/list", {})
  RETURN deserialize result.tools as Vec<McpToolInfo>
END FUNCTION

FUNCTION McpClient::call_tool(name: String, arguments: Value) -> Result<McpToolCallResult, McpError>
  result ← AWAIT self.send_request("tools/call", { name, arguments })
  RETURN deserialize result as McpToolCallResult
END FUNCTION
```

---
