<!-- Last verified: 2026-04-05 by Claude Code -->
# Messages & Events

## Message Types

### `Message`

The core LLM message type, tagged by role:

```rust
pub enum Message {
    User {
        content: Vec<Content>,
        timestamp: u64,
    },
    Assistant {
        content: Vec<Content>,
        stop_reason: StopReason,
        model: String,
        provider: String,
        usage: Usage,
        timestamp: u64,
        error_message: Option<String>,
    },
    ToolResult {
        tool_call_id: String,
        tool_name: String,
        content: Vec<Content>,
        is_error: bool,
        timestamp: u64,
        child_loop_id: Option<String>,  // set by sub-agent tools
    },
}
```

Create user messages easily:

```rust
let msg = Message::user("Hello, world!");
```

### `AgentMessage`

Wraps `Message` with support for extension messages (UI-only, notifications, etc.):

```rust
pub enum AgentMessage {
    Llm(LlmMessage),
    Extension(ExtensionMessage),
}

pub struct LlmMessage {
    pub message: Message,
    /// Which turn produced this message. `None` for messages that predate
    /// turn tracking or are created outside the agent loop.
    pub turn_id: Option<TurnId>,
}

pub struct ExtensionMessage {
    pub role: String,
    pub kind: String,
    pub data: serde_json::Value,
}
```

Create extension messages with the convenience constructor:

```rust
let ext = ExtensionMessage::new("status_update", serde_json::json!({"status": "running"}));
let msg = AgentMessage::Extension(ext);
```

The `kind` field categorizes the extension (e.g., `"status_update"`, `"ui_event"`, `"notification"`). Use `as_llm()` to extract the `Message` if it's an LLM message. `LlmMessage` wraps a `Message` with an optional `TurnId { loop_id, turn_index }` for compaction tracking — this allows the compaction system to identify which turn produced each message. The default `convert_to_llm` function filters out `Extension` messages before sending to the provider.

All core message types implement `Serialize`, `Deserialize`, `Clone`, and `PartialEq`, enabling state persistence and test assertions.

## Content

Each message contains `Vec<Content>`:

```rust
pub enum Content {
    Text { text: String },
    Image { data: String, mime_type: String },
    Thinking { thinking: String, signature: Option<String> },
    ToolCall { id: String, name: String, arguments: serde_json::Value },
}
```

An assistant message can contain multiple content blocks — e.g., thinking + text + tool calls.

The `signature` field on `Content::Thinking` is a cryptographic integrity token issued by the LLM provider (Anthropic calls it `signature`, OpenAI calls it `encrypted_content`, Gemini calls it `thought_signature`). It must be echoed back **unmodified** in multi-turn conversations — tampering or omitting it causes the provider to reject the request. It is `None` on providers that don't support extended thinking or on the first-turn generation.

## StopReason

```rust
pub enum StopReason {
    Stop,              // Natural completion
    Length,            // Hit max tokens
    ToolUse,           // Wants to call tools
    Error,             // Provider error
    Aborted,           // Cancelled by user
    MaxTurns,          // Reached maximum allowed turns
    UserStop,          // Explicit user stop command
    Handoff,           // Handing off to a human operator
    GuardRail,         // Stopped by content moderation / safety filter
    ContextCompacted,  // Context was compacted to fit within limits
    Paused,            // Paused waiting for external input
}
```

## Usage

Token usage from the provider:

```rust
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub total_tokens: u64,
}
```

## AgentEvent

Events emitted during the agent loop for real-time UI updates:

| Event | When |
|-------|------|
| `AgentStart { agent_id, session_id, loop_id, parent_loop_id, continuation_kind, timestamp }` | Loop begins. `loop_id` is `"{session_id}.{config_id}.{N}"`. `parent_loop_id` is `Some` for continuations and sub-agents. `continuation_kind` is `Some` for `agent_loop_continue` calls. |
| `AgentEnd { messages, timestamp, rejection }` | Loop finishes; `rejection` is `Some` when an InputFilter blocked input |
| `TurnStart { turn_index, timestamp, triggered_by }` | New LLM call starting; `turn_index` is 0-based, `triggered_by` is `User \| SubAgent \| Continuation \| Branch` |
| `TurnEnd { message, timestamp, tool_results }` | LLM call + tool execution complete |
| `MessageStart { message }` | A message is available |
| `MessageUpdate { message, delta }` | Streaming delta arrived |
| `MessageEnd { message }` | Message finalized |
| `ToolExecutionStart { tool_call_id, tool_name, args }` | Tool about to run |
| `ToolExecutionUpdate { tool_call_id, tool_name, partial_result }` | Tool progress |
| `ToolExecutionEnd { tool_call_id, tool_name, result, is_error, child_loop_id }` | Tool finished. `child_loop_id` is `Some` when the tool was a sub-agent — it identifies the child loop that ran. |
| `ProgressMessage { tool_call_id, tool_name, text }` | User-facing progress text from a tool |
| `InputRejected { reason }` | Input filter rejected the user's message |

## StreamDelta

Deltas within `MessageUpdate`:

```rust
pub enum StreamDelta {
    Text { delta: String },
    Thinking { delta: String },
    ToolCallDelta { delta: String },
}
```

## Agent State

The `Agent` struct provides access to its current state:

```rust
// Check if the agent is currently streaming a response
if agent.is_streaming() {
    // Use steer() or follow_up() instead of prompt()
    agent.steer(AgentMessage::Llm(Message::user("New instruction")));
}

// Access the full message history
let messages: &[AgentMessage] = agent.messages();

// Check the last message
if let Some(last) = messages.last() {
    println!("Last message role: {}", last.role());
}
```

The `is_streaming()` flag is `true` between `prompt()`/`continue_loop()` call and completion. While streaming, calling `prompt()` will panic — use `steer()` or `follow_up()` instead.
