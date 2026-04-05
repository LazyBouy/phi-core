<!-- Last verified: 2026-04-05 by Claude Code -->
# Tool System

The tool system defines how agents interact with the external world. Every capability an agent has -- running shell commands, reading files, calling APIs, delegating to sub-agents -- is expressed as a tool implementing the `AgentTool` trait. The agent loop discovers tools by name from a registry, executes them with lifecycle events, and feeds results back to the LLM.

## Concept Overview

```
Tool [EXISTS]
├── AgentTool trait [EXISTS] — name, label, description, parameters_schema, execute
├── ToolContext [EXISTS] — tool_call_id, tool_name, cancel, on_update, on_progress
├── ToolResult [EXISTS] — content, details, child_loop_id
├── ToolError [EXISTS] — Failed/NotFound/InvalidArgs/Cancelled
├── ToolExecutionStrategy [EXISTS] — Sequential/Parallel/Batched
├── SubAgentTool [EXISTS] — spawns child agent loop
├── Sources: Built-in [EXISTS] / OpenAPI [EXISTS] / MCP [EXISTS]
└── Callbacks: before/after_tool_execution, before/after_update [EXISTS]
```

---

## AgentTool Trait [EXISTS]

The core extension point. Implement this trait to create custom tools.

| Method | Signature | Status | Description |
|--------|-----------|--------|-------------|
| `name()` | `-> &str` | [EXISTS] | Unique identifier used in LLM tool_use (e.g. `"bash"`) |
| `label()` | `-> &str` | [EXISTS] | Human-readable label for UI display |
| `description()` | `-> &str` | [EXISTS] | Description sent to the LLM so it knows when/how to use the tool |
| `parameters_schema()` | `-> serde_json::Value` | [EXISTS] | JSON Schema for parameters; LLM uses this to format arguments |
| `execute()` | `(params, ctx) -> Result<ToolResult, ToolError>` | [EXISTS] | Execute the tool with LLM-chosen arguments and system-injected context |

**Design**: `params` (LLM input) and `ctx` (system environment) are deliberately separate parameters. `params` varies per call; `ctx` provides cancellation, streaming callbacks, and correlation IDs that are the same shape for every tool.

---

## ToolContext [EXISTS]

Per-invocation context passed to `execute()`. Using a struct (rather than individual parameters) future-proofs the trait -- adding fields is non-breaking.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `tool_call_id` | `String` | [EXISTS] | Unique ID for this invocation; correlates Start/Update/End events |
| `tool_name` | `String` | [EXISTS] | Name of the tool being invoked |
| `cancel` | `CancellationToken` | [EXISTS] | Check `is_cancelled()` in long-running tools; child token of the parent loop's token |
| `on_update` | `Option<ToolUpdateFn>` | [EXISTS] | Callback for streaming partial `ToolResult`s (UI/logging only; not sent to LLM) |
| `on_progress` | `Option<ProgressFn>` | [EXISTS] | Callback for user-facing progress text (emits `ProgressMessage` events) |

**Callback wiring**: The agent loop creates `on_update` and `on_progress` closures that capture a cloned `tx` channel sender. When a tool calls `on_update(partial)`, the closure pushes an `AgentEvent::ToolExecutionUpdate` into the channel. The tool never touches the event system directly.

---

## ToolResult [EXISTS]

What a tool hands back to the runtime after execution.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `content` | `Vec<Content>` | [EXISTS] | Tool output (text, images, etc.) |
| `details` | `serde_json::Value` | [EXISTS] | Freeform metadata (not sent to LLM) |
| `child_loop_id` | `Option<String>` | [EXISTS] | Set by `SubAgentTool` to the child loop's ID; `None` for regular tools |

**Note**: The runtime transforms `struct ToolResult` into `Message::ToolResult` by enriching it with correlation metadata (`tool_call_id`, `tool_name`, `is_error`, `timestamp`) before it enters the LLM conversation.

---

## ToolError [EXISTS]

Error taxonomy for tool execution failures. Errors are converted to `ToolResult` with `is_error=true` so the LLM sees the failure and can self-correct.

| Variant | Display | Status |
|---------|---------|--------|
| `Failed(String)` | `"{message}"` | [EXISTS] |
| `NotFound(String)` | `"Tool not found: {name}"` | [EXISTS] |
| `InvalidArgs(String)` | `"Invalid arguments: {message}"` | [EXISTS] |
| `Cancelled` | `"Cancelled"` | [EXISTS] |

---

## ToolExecutionStrategy [EXISTS]

Controls how multiple tool calls from a single LLM response are executed. Set at agent construction time (not a per-turn LLM decision).

| Variant | Status | Behavior |
|---------|--------|----------|
| `Sequential` | [EXISTS] | One at a time; checks steering between each. Use for tools with shared mutable state |
| `Parallel` (default) | [EXISTS] | All concurrent via `futures::join_all`; checks steering after all complete. Best latency for independent tools |
| `Batched { size }` | [EXISTS] | N tools in parallel per batch; checks steering between batches. Balances speed with human-in-the-loop control |

**Steering**: The human-in-the-loop interrupt mechanism. Between tool executions (or batches), the loop checks whether the human has sent a new instruction, cancellation, or correction.

---

## SubAgentTool [EXISTS]

A tool that delegates work to a child agent loop. When the parent LLM calls it, a fresh `agent_loop()` runs with its own system prompt, tools, and provider. The child loop's final text output is returned as the tool result.

| Attribute | Status | Description |
|-----------|--------|-------------|
| `tool_name` | [EXISTS] | Unique name for the sub-agent tool |
| `tool_description` | [EXISTS] | Description for the parent LLM |
| `system_prompt` | [EXISTS] | Child agent's system prompt |
| `model_config` | [EXISTS] | Child agent's model configuration |
| `provider_override` | [EXISTS] | Optional custom provider (testing) |
| `tools` | [EXISTS] | Tools available to the child agent |
| `thinking_level` | [EXISTS] | Thinking level for the child loop |

**Design constraints**: Sub-agents are NOT given other SubAgentTools (static depth prevention). Cancellation propagates from parent to child. Events stream back to the parent via `on_update`.

---

## Built-in Tools [EXISTS]

Six tools returned by `default_tools()`:

| Tool | File | Status | Description |
|------|------|--------|-------------|
| `BashTool` | `tools/bash.rs` | [EXISTS] | Run shell commands |
| `ReadFileTool` | `tools/file.rs` | [EXISTS] | Read file contents |
| `WriteFileTool` | `tools/file.rs` | [EXISTS] | Write or overwrite a file |
| `EditFileTool` | `tools/edit.rs` | [EXISTS] | Precise text replacement within a file |
| `ListFilesTool` | `tools/list.rs` | [EXISTS] | List directory contents |
| `SearchTool` | `tools/search.rs` | [EXISTS] | Grep / content search across files |

---

## OpenAPI Tools [EXISTS]

`OpenApiToolAdapter` parses an OpenAPI 3.0 spec and creates one `AgentTool` per operation. Each adapter makes HTTP requests to the API endpoint when executed. Feature-gated behind the `openapi` Cargo feature.

Factory methods: `from_str`, `from_file`, `from_url`, `from_spec`.

---

## MCP Tools [EXISTS]

`McpToolAdapter` bridges MCP server tools to the `AgentTool` trait using the Adapter pattern. All adapters for the same server share one `McpClient` (via `Arc<Mutex<McpClient>>`). Name collision prevention uses an optional prefix namespace (e.g. `"filesystem__read_file"`).

---

## Tool Callbacks [EXISTS]

Lifecycle hooks that fire around tool execution. All are `Option<Arc<dyn Fn(...)>>` on `AgentLoopConfig`.

| Hook | Signature | Status | Fires When |
|------|-----------|--------|------------|
| `before_tool_execution` | `(tool_name, tool_call_id, args) -> bool` | [EXISTS] | Before `ToolExecutionStart`; return `false` to skip the call |
| `after_tool_execution` | `(tool_name, tool_call_id, is_error)` | [EXISTS] | After `ToolExecutionEnd` |
| `before_tool_execution_update` | `(tool_name, tool_call_id, text) -> bool` | [EXISTS] | Before each `ToolExecutionUpdate`; return `false` to suppress the event |
| `after_tool_execution_update` | `(tool_name, tool_call_id, text)` | [EXISTS] | After each `ToolExecutionUpdate` (only if not suppressed) |

**Hook ordering**: Hooks fire strictly before their paired event is emitted. When `before_tool_execution` returns `false`, no `ToolExecutionStart`/`End` events are emitted; a synthetic error `ToolResult` is sent to the LLM so it knows the call was skipped.

---

## Code Reference

| Concept | File |
|---------|------|
| `AgentTool` trait, `ToolContext`, `ToolResult`, `ToolError`, `ToolExecutionStrategy` | `src/types/tool.rs` |
| `ToolUpdateFn`, `ProgressFn` type aliases | `src/types/tool.rs` |
| Tool dispatch, `execute_tool_calls`, `execute_single_tool`, `skip_tool_call` | `src/agent_loop/tools.rs` |
| `SubAgentTool` | `src/agents/sub_agent.rs` |
| Built-in tools (`BashTool`, `ReadFileTool`, etc.) | `src/tools/` |
| `OpenApiToolAdapter` | `src/openapi/adapter.rs` |
| `McpToolAdapter` | `src/mcp/tool_adapter.rs` |
| Tool callback type aliases (`BeforeToolExecutionFn`, etc.) | `src/agent_loop/config.rs` |
| `ToolDefinition` (schema sent to LLM, not executable) | `src/provider/traits.rs` |

---

## Conceptual Notes

- **Tool Permission System** [CONCEPTUAL] -- The plan includes an Agent-level Permissions tab with include/exclude rules for allowed/denied actions. This would gate tool execution at a higher level than the `before_tool_execution` hook.
- **Tool Result Streaming to LLM** [CONCEPTUAL] -- Currently `on_update` partial results are UI-only. A future design could allow streaming tool results to the LLM mid-execution for real-time reasoning.
- **ToolDefinition vs AgentTool Split** -- `ToolDefinition` (in `provider/traits.rs`) is the schema half sent to the LLM; `AgentTool` (in `types/tool.rs`) is the executable half. The agent loop bridges them: converts `AgentTool` to `ToolDefinition` before streaming, then matches `ToolCall` content back to `AgentTool` by name for execution.
