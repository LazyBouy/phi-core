<!-- Last verified: 2026-04-05 by Claude Code -->
# Message

The message entities form the communication substrate of the entire system. Messages flow through Agent, Session, Loop, and Turn. The type hierarchy separates atomic content blocks, conversation-level messages, agent-level routing envelopes, and token usage tracking.

## Concept Overview

```
Message System [EXISTS]
├── Content [EXISTS] — Text / Image / Thinking / ToolCall
├── Message [EXISTS] — User / Assistant / ToolResult
├── AgentMessage [EXISTS] — Llm(LlmMessage) | Extension(ExtensionMessage)
├── LlmMessage [EXISTS] — Message + Option<TurnId>
├── StopReason [EXISTS] — Stop/Length/ToolUse/Error/Aborted/...
└── Usage [EXISTS] — input/output/reasoning/cache tokens
```

---

## Content `[EXISTS]`

The atomic unit of all message payloads. Every message is composed of `Vec<Content>`. A single LLM turn can contain multiple content blocks (e.g., a Thinking block followed by Text, or Text followed by multiple ToolCalls).

Enum `Content`, tagged by `"type"` in JSON:

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `Text` | `[EXISTS]` | `text: String` | Plain string payload sent to/from the LLM. |
| `Image` | `[EXISTS]` | `data: String`, `mime_type: String` | Binary image encoded as base64 string (not a file path). LLMs receive image bytes inline. |
| `Thinking` | `[EXISTS]` | `thinking: String`, `signature: Option<String>` | Internal chain-of-thought from the LLM (e.g., Claude extended thinking). Visible in UI, never re-sent as content to LLM. `signature` is a cryptographic integrity token from the provider that must be echoed back unmodified in multi-turn conversations. |
| `ToolCall` | `[EXISTS]` | `id: String`, `name: String`, `arguments: serde_json::Value` | LLM's request to invoke a tool with structured JSON arguments. The `id` links to a corresponding `ToolResult`. |

---

## Message `[EXISTS]`

The conversation-level message enum. Tagged by `"role"` in JSON. Each variant carries `Vec<Content>` plus role-specific metadata.

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `User` | `[EXISTS]` | `content: Vec<Content>`, `timestamp: u64` | User turn. Mixed media supported (text + images). Timestamp is unix millis. Helper constructor: `Message::user(text)`. |
| `Assistant` | `[EXISTS]` | `content: Vec<Content>`, `stop_reason: StopReason`, `model: String`, `provider: String`, `usage: Usage`, `timestamp: u64`, `error_message: Option<String>` | LLM's response, fully annotated. `stop_reason` tells why generation stopped. `model`/`provider` captured for cost tracking and multi-provider routing. Failed turns are persisted, not dropped. |
| `ToolResult` | `[EXISTS]` | `tool_call_id: String`, `tool_name: String`, `content: Vec<Content>`, `is_error: bool`, `timestamp: u64` | Tool execution result returned to LLM. `tool_call_id` links back to the specific `ToolCall` in the assistant content. `is_error: true` means the LLM sees the failure and can recover/retry. |

### Helper Methods on Message

| Method | Status | Description |
|--------|--------|-------------|
| `user(text)` | `[EXISTS]` | Constructor for simple text user messages. |
| `role()` | `[EXISTS]` | Returns `"user"`, `"assistant"`, or `"toolResult"`. |
| `is_context_overflow()` | `[EXISTS]` | Checks if an assistant message represents a context overflow error by inspecting `error_message` against known provider overflow patterns. |

---

## StopReason `[EXISTS]`

Why an assistant message's generation stopped. Enum with camelCase serialization.

| Variant | Status | Description |
|---------|--------|-------------|
| `Stop` | `[EXISTS]` | Natural end of generation. |
| `Length` | `[EXISTS]` | Max tokens reached. |
| `ToolUse` | `[EXISTS]` | LLM requested tool execution. |
| `Error` | `[EXISTS]` | Provider error during generation. |
| `Aborted` | `[EXISTS]` | Cancelled by caller. |
| `MaxTurns` | `[EXISTS]` | Maximum allowed turns reached. |
| `UserStop` | `[EXISTS]` | Stopped by explicit user command. |
| `Handoff` | `[EXISTS]` | Agent handing off to human operator. |
| `GuardRail` | `[EXISTS]` | Stopped by internal guardrail (content moderation, safety filter). |
| `ContextCompacted` | `[EXISTS]` | Context was compacted, potentially losing information. |
| `Paused` | `[EXISTS]` | Generation paused (waiting for external input). |

---

## AgentMessage `[EXISTS]`

The agent loop's two-lane routing envelope. Decides whether content goes INTO the LLM context window or SIDEWAYS to the UI/app without consuming tokens.

Enum `AgentMessage`, untagged in JSON (discriminated by `role` field):

| Variant | Status | Description |
|---------|--------|-------------|
| `Llm(LlmMessage)` | `[EXISTS]` | Enters the LLM context window. Serialized into the API request. |
| `Extension(ExtensionMessage)` | `[EXISTS]` | NEVER enters the context window. Only emitted as `AgentEvent`s. For UI notifications, debug events, session metadata, progress markers. |

### Key Design: One-way Conversion

`Message -> AgentMessage::Llm` exists via `From<Message>`. There is no path for `ExtensionMessage` to become an `Llm` variant. The type system enforces that UI-only content can never accidentally slip into the LLM context.

### Methods on AgentMessage

| Method | Status | Description |
|--------|--------|-------------|
| `role()` | `[EXISTS]` | Delegates to inner message's role. |
| `as_llm()` | `[EXISTS]` | Returns `Option<&Message>`. `None` for Extension. |
| `turn_id()` | `[EXISTS]` | Returns `Option<&TurnId>`. `None` for Extension. |
| `with_turn_id(Option<TurnId>)` | `[EXISTS]` | Sets turn_id on LLM messages. No-op for Extension. |

---

## LlmMessage `[EXISTS]`

An LLM-bound message with optional turn tracking metadata. Wraps `Message` + `Option<TurnId>`.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `message` | `Message` | `[EXISTS]` | The underlying conversation message. |
| `turn_id` | `Option<TurnId>` | `[EXISTS]` | Which turn produced this message. `None` for messages that predate turn tracking or are created outside the agent loop. |

### Custom Serde (Flatten Pattern)

`LlmMessage` uses custom `Serialize` / `Deserialize` implementations to flatten into the same JSON shape as a bare `Message` with an optional `turnId` field injected. This maintains backward compatibility: old data without `turnId` deserializes as `turn_id: None`.

**Why custom serde**: `#[serde(flatten)]` does not work with serde's internally-tagged enums (`#[serde(tag = "role")]` on `Message`). Manual serialize/deserialize is the only way to achieve the flatten-into-Message pattern.

### Constructors

| Method | Status | Description |
|--------|--------|-------------|
| `new(message)` | `[EXISTS]` | Creates LlmMessage without turn tracking (`turn_id: None`). |
| `with_turn(message, turn_id)` | `[EXISTS]` | Creates LlmMessage with a specific `TurnId`. |

---

## ExtensionMessage `[EXISTS]`

App-only message that never enters the LLM context window. Streamed as events for UI/app consumption.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `role` | `String` | `[EXISTS]` | Always `"extension"`. Acts as discriminator in untagged deserialization. Named `role` for consistency with `Message` but functions more like a type/category marker. |
| `kind` | `String` | `[EXISTS]` | Message category (e.g., `"notification"`, `"system"`, `"debug"`). App-specific. |
| `data` | `serde_json::Value` | `[EXISTS]` | Arbitrary JSON payload. Serialized from any `impl Serialize` via `ExtensionMessage::new()`. |

---

## Usage `[EXISTS]`

Token metrics per turn or accumulated across loops/sessions.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `input` | `u64` | `[EXISTS]` | Input tokens consumed. |
| `output` | `u64` | `[EXISTS]` | Output tokens generated. |
| `reasoning` | `u64` | `[EXISTS]` | Reasoning/thinking tokens — a subset of `output`. Non-zero only for providers that report reasoning tokens separately (OpenAI o-series). Defaults to 0. |
| `cache_read` | `u64` | `[EXISTS]` | Tokens served from prompt cache. |
| `cache_write` | `u64` | `[EXISTS]` | Tokens written to prompt cache. |
| `total_tokens` | `u64` | `[EXISTS]` | Total tokens (may differ from sum of above depending on provider reporting). |

### Methods on Usage

| Method | Status | Description |
|--------|--------|-------------|
| `estimated_cost(&CostConfig)` | `[EXISTS]` | Dollar cost calculation using per-million-token rates. `reasoning` tokens are already counted in `output` (no double-charge). |
| `combine(&Usage)` | `[EXISTS]` | Adds two Usage values (e.g., sum across parallel branches or multi-step loops). |
| `cache_hit_rate()` | `[EXISTS]` | Fraction of input tokens served from cache (0.0-1.0). Returns 0.0 if no input tokens processed. |

### Where Usage Appears

| Location | Status | Description |
|----------|--------|-------------|
| `Message::Assistant.usage` | `[EXISTS]` | Per-turn usage on the assistant message itself. |
| `AgentEvent::TurnEnd.usage` | `[EXISTS]` | Direct per-turn access without destructuring the message. |
| `AgentEvent::AgentEnd.usage` | `[EXISTS]` | Accumulated across all turns in a loop. |
| `LoopRecord.usage` | `[EXISTS]` | Captured from `AgentEnd.usage`. |
| `Session.total_usage()` | `[EXISTS]` | Summed across all loops. |

---

## Code Reference

| File | What it contains |
|------|-----------------|
| `src/types/content.rs` | `Content` enum (Text, Image, Thinking, ToolCall), `Message` enum (User, Assistant, ToolResult), `StopReason` enum, `now_ms()` helper. |
| `src/types/agent_message.rs` | `TurnId` struct, `LlmMessage` struct (with custom serde), `AgentMessage` enum, `From<Message>` impl. |
| `src/types/extension.rs` | `ExtensionMessage` struct. |
| `src/types/usage.rs` | `Usage` struct, `CacheConfig` struct, `CacheStrategy` enum, `ThinkingLevel` enum. |

---

## Conceptual Notes

- **LlmMessage serde** is a critical compatibility mechanism. Any future fields added to LlmMessage must maintain the flatten-into-Message JSON pattern. Do not use `#[serde(flatten)]` with `Message`.
- **ExtensionMessage naming**: The `role` field is named for consistency with `Message` but functions as a type discriminator. A more accurate name would be `type` or `category`, but `role` enables consistent untagged serde deserialization across the `AgentMessage` enum.
- **StopReason** includes several forward-looking variants (MaxTurns, UserStop, Handoff, GuardRail, ContextCompacted, Paused) adopted from other agentic frameworks. These exist as enum variants but may not yet be emitted by all code paths.
- **Usage.reasoning** is a subset of `output`, not an additional charge. It is non-zero only for OpenAI o-series models that report reasoning tokens separately.
