# API Reference

## Top-Level Functions

### `agent_loop()`

```rust
pub async fn agent_loop(
    prompts: Vec<AgentMessage>,
    context: &mut AgentContext,
    config: &AgentLoopConfig<'_>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: CancellationToken,
) -> Vec<AgentMessage>
```

Start an agent loop with new prompt messages. Returns all messages generated during the run.

### `agent_loop_continue()`

```rust
pub async fn agent_loop_continue(
    context: &mut AgentContext,
    config: &AgentLoopConfig<'_>,
    tx: mpsc::UnboundedSender<AgentEvent>,
    cancel: CancellationToken,
) -> Vec<AgentMessage>
```

Resume from existing context. The last message must not be an assistant message.

### `default_tools()`

```rust
pub fn default_tools() -> Vec<Box<dyn AgentTool>>
```

Returns: `BashTool`, `ReadFileTool`, `WriteFileTool`, `EditFileTool`, `ListFilesTool`, `SearchTool`.

## Agent Trait

The runtime interface for all agent implementations. Programs against this trait to remain independent of the specific implementation.

```rust
use phi_core::Agent;  // trait must be in scope to call trait methods
```

Trait methods cover: prompting (`prompt`, `prompt_messages`, `prompt_with_sender`, `prompt_messages_with_sender`, `continue_loop`, `continue_loop_with_sender`), state access (`messages`, `is_streaming`, `agent_id`, `session_id`, `last_loop_id`), message mutation (`clear_messages`, `append_message`, `replace_messages`, `save_messages`, `restore_messages`, `set_tools`), control (`abort`, `reset`), and steering/follow-up queues (`steer`, `follow_up`, `clear_steering_queue`, `clear_follow_up_queue`, `clear_all_queues`, `set_steering_mode`, `set_follow_up_mode`).

The trait is object-safe: `Box<dyn Agent>` and `&mut dyn Agent` work for runtime polymorphism.

`phi_core::*` re-exports `Agent`, so `use phi_core::*` brings it into scope automatically.

## BasicAgent Struct

The default in-memory `Agent` implementation. Owns a single linear message history, tool registry, and provider reference.

### Construction

```rust
let agent = BasicAgent::new(provider);
```

| Signature | Description |
|-----------|-------------|
| `BasicAgent::new(provider: impl StreamProvider + 'static) -> Self` | Create a new agent with the given provider |

### Builder Methods

All return `Self` for chaining (unless noted as `Result`).

**Core**

| Method | Description |
|--------|-------------|
| `with_system_prompt(prompt) -> Self` | Set the system prompt |
| `with_model(model) -> Self` | Set the model identifier |
| `with_api_key(key) -> Self` | Set the API key |
| `with_thinking(level: ThinkingLevel) -> Self` | Set thinking level (`Off`, `Minimal`, `Low`, `Medium`, `High`) |
| `with_max_tokens(max: u32) -> Self` | Set max output tokens |
| `with_model_config(config: ModelConfig) -> Self` | Set model config (base URL, headers, compat flags) for multi-provider support |

**Tools & Integrations**

| Method | Description |
|--------|-------------|
| `with_tools(tools: Vec<Box<dyn AgentTool>>) -> Self` | Set tools (replaces existing) |
| `with_sub_agent(sub: SubAgentTool) -> Self` | Add a sub-agent tool |
| `with_skills(skills: SkillSet) -> Self` | Load skills and append their index to the system prompt |
| `async with_mcp_server_stdio(command, args, env) -> Result<Self, McpError>` | Connect to MCP server via stdio and add its tools |
| `async with_mcp_server_http(url) -> Result<Self, McpError>` | Connect to MCP server via HTTP and add its tools |
| `async with_openapi_file(path, config, filter) -> Result<Self, OpenApiError>` | Load tools from an OpenAPI spec file *(requires `openapi` feature)* |
| `async with_openapi_url(url, config, filter) -> Result<Self, OpenApiError>` | Fetch spec from URL and add tools *(requires `openapi` feature)* |
| `with_openapi_spec(spec_str, config, filter) -> Result<Self, OpenApiError>` | Parse spec string and add tools *(requires `openapi` feature)* |

**Context & Limits**

| Method | Description |
|--------|-------------|
| `with_context_config(config: ContextConfig) -> Self` | Set context compaction config |
| `with_execution_limits(limits: ExecutionLimits) -> Self` | Set execution limits (max turns, tokens, duration) |
| `with_compaction_strategy(strategy: impl CompactionStrategy) -> Self` | Set a custom compaction strategy |
| `without_context_management() -> Self` | Disable automatic context compaction and execution limits |

**Behavior**

| Method | Description |
|--------|-------------|
| `with_messages(msgs: Vec<AgentMessage>) -> Self` | Pre-load message history |
| `with_cache_config(config: CacheConfig) -> Self` | Set prompt caching configuration |
| `with_tool_execution(strategy: ToolExecutionStrategy) -> Self` | Set tool execution strategy (`Parallel`, `Sequential`, `Batched`) |
| `with_retry_config(config: RetryConfig) -> Self` | Set retry configuration |
| `with_input_filter(filter: impl InputFilter) -> Self` | Add an input filter (runs on user messages before LLM call) |

**Callbacks**

| Method | Description |
|--------|-------------|
| `on_before_loop(f: Fn(&[AgentMessage], u64) -> bool) -> Self` | Called once before `AgentStart`; return `false` to abort the entire run |
| `on_after_loop(f: Fn(&[AgentMessage], &Usage)) -> Self` | Called once after `AgentEnd` with all new messages and accumulated usage |
| `on_before_turn(f: Fn(&[AgentMessage], usize) -> bool) -> Self` | Called before each LLM call; return `false` to abort |
| `on_after_turn(f: Fn(&[AgentMessage], &Usage)) -> Self` | Called after each LLM response and tool execution |
| `on_error(f: Fn(&str)) -> Self` | Called when the LLM returns `StopReason::Error` |
| `on_before_tool_execution(f: Fn(&str, &str, &Value) -> bool) -> Self` | Called before each tool call `(call_id, name, args)`; return `false` to skip |
| `on_after_tool_execution(f: Fn(&str, &str, bool)) -> Self` | Called after each tool call `(name, call_id, is_error)` |
| `on_before_tool_execution_update(f: Fn(&str, &str, &str) -> bool) -> Self` | Called before each streaming tool update `(name, call_id, text)`; return `false` to suppress the event |
| `on_after_tool_execution_update(f: Fn(&str, &str, &str)) -> Self` | Called after each streaming tool update `(name, call_id, text)` |

### Prompting

| Method | Description |
|--------|-------------|
| `async prompt(text) -> UnboundedReceiver<AgentEvent>` | Send a text prompt, returns event stream |
| `async prompt_messages(messages) -> UnboundedReceiver<AgentEvent>` | Send messages as prompt |
| `async prompt_with_sender(text, tx: UnboundedSender<AgentEvent>)` | Send a text prompt, streaming events to a caller-provided sender for real-time consumption |
| `async prompt_messages_with_sender(messages, tx)` | Send messages, streaming events to a caller-provided sender |
| `async continue_loop() -> UnboundedReceiver<AgentEvent>` | Resume from current context with `ContinuationKind::Default` |
| `async continue_loop_with_sender(tx: UnboundedSender<AgentEvent>, kind: ContinuationKind)` | Resume from current context with an explicit continuation kind, streaming events to a caller-provided sender |

### State Access

| Method | Description |
|--------|-------------|
| `messages() -> &[AgentMessage]` | Get the full message history |
| `is_streaming() -> bool` | Whether the agent is currently running |
| `agent_id() -> &str` | Stable UUID assigned at construction; included in every `AgentStart` event |
| `session_id() -> &str` | Stable UUID assigned at construction; groups all loops from this `Agent` instance |
| `last_loop_id() -> Option<&str>` | The `loop_id` of the most recently started loop; `None` before first run |

### State Mutation

| Method | Description |
|--------|-------------|
| `set_tools(tools: Vec<Box<dyn AgentTool>>)` | Replace the tool set |
| `clear_messages()` | Clear all messages |
| `append_message(msg: AgentMessage)` | Add a message to history |
| `replace_messages(msgs: Vec<AgentMessage>)` | Replace all messages |
| `save_messages() -> Result<String, serde_json::Error>` | Serialize message history to JSON |
| `restore_messages(json: &str) -> Result<(), serde_json::Error>` | Restore message history from JSON |

### Steering & Follow-Up Queues

| Method | Description |
|--------|-------------|
| `steer(msg: AgentMessage)` | Queue a steering message (interrupts mid-tool-execution) |
| `follow_up(msg: AgentMessage)` | Queue a follow-up message (processed after agent finishes) |
| `clear_steering_queue()` | Clear pending steering messages |
| `clear_follow_up_queue()` | Clear pending follow-up messages |
| `clear_all_queues()` | Clear both queues |
| `set_steering_mode(mode: QueueMode)` | Set delivery mode: `OneAtATime` or `All` |
| `set_follow_up_mode(mode: QueueMode)` | Set delivery mode: `OneAtATime` or `All` |

### Control

| Method | Description |
|--------|-------------|
| `abort()` | Cancel the current run via `CancellationToken` |
| `reset()` | Clear all state (messages, queues, streaming flag) |

## Re-exports

The crate re-exports key types from `lib.rs`:

```rust
pub use agents::{Agent, BasicAgent, QueueMode};  // Agent trait + BasicAgent struct
pub use agents::SubAgentTool;
pub use agent_loop::{agent_loop, agent_loop_continue};
pub use context::{CompactionStrategy, DefaultCompaction};
pub use retry::RetryConfig;
pub use skills::SkillSet;
pub use types::*;  // Message, Content, AgentMessage, AgentEvent, etc.
```
