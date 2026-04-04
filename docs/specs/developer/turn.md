# Turn

A single LLM call-and-response cycle within a Loop. One Loop may have many Turns: the initial response plus one per tool-call round-trip or steering message injection.

**Status**: Turn `[EXISTS]` as both a first-class struct (`Turn` on `LoopRecord.turns`) and as an event-pair (`TurnStart` / `TurnEnd`). The `SessionRecorder` materializes `Turn` structs from the event stream.

## Concept Overview

```
Turn [EXISTS as struct on LoopRecord.turns; EXISTS as event-pair TurnStart/TurnEnd]
├── HEADER
│   ├── TurnId [EXISTS] — { loop_id, turn_index }
│   ├── triggered_by [EXISTS] — User/SubAgent/Continuation/Branch
│   ├── usage [EXISTS] — per-turn from TurnEnd
│   └── Callbacks: before_turn / after_turn [EXISTS]
└── LINE ITEMS: Actions
    ├── Messages [EXISTS] — Input (User) + Output (Assistant)
    ├── Tool Executions [EXISTS]
    └── Streaming [EXISTS] — MessageUpdate deltas
```

---

## HEADER

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `TurnId` | struct | `[EXISTS]` | Identifies the turn. Composed of `loop_id: String` and `turn_index: u32`. Carried on every `LlmMessage` produced during the turn. |
| `turn_index` | `u32` | `[EXISTS]` | Zero-based index within the current loop (0 = first turn after `AgentStart`). Present on `TurnStart` and `TurnEnd` events. |
| `triggered_by` | `TurnTrigger` | `[EXISTS]` | What caused this turn to begin. See Trigger section below. |
| `usage` | `Usage` | `[EXISTS]` | Per-turn token usage. Carried on `TurnEnd.usage`. Fields: input, output, reasoning, cache_read, cache_write, total_tokens. |
| `timestamp` (start) | `DateTime<Utc>` | `[EXISTS]` | Wall-clock time when the turn began. On `TurnStart.timestamp`. |
| `timestamp` (end) | `DateTime<Utc>` | `[EXISTS]` | Wall-clock time when the turn completed (after all tool calls finished). On `TurnEnd.timestamp`. |

### TurnTrigger `[EXISTS]`

Identifies what caused a new turn to begin. Enum `TurnTrigger`:

| Variant | Status | Description |
|---------|--------|-------------|
| `User` | `[EXISTS]` | First turn triggered by a user message (`agent_loop`). |
| `SubAgent` | `[EXISTS]` | This agent was invoked as a sub-agent by a parent agent. |
| `Continuation` | `[EXISTS]` | Continuation turn: tool round-trip, steering message, or `Default` / `Rerun` continuation. |
| `Branch` | `[EXISTS]` | First turn of a `Branch` continuation (`agent_loop_continue` with `ContinuationKind::Branch`). Subsequent turns within the same branched loop use `Continuation`. |

### Callbacks `[EXISTS]`

| Callback | Status | Description |
|----------|--------|-------------|
| `before_turn` | `[EXISTS]` | Fires BEFORE `TurnStart` event is emitted. Defined as `BeforeTurnFn` on `AgentLoopConfig`. Receives `(&[AgentMessage], usize)` (messages, turn index). Returning `false` aborts the turn. |
| `after_turn` | `[EXISTS]` | Fires AFTER `TurnEnd` event is emitted. Defined as `AfterTurnFn`. Receives `(&[AgentMessage], &Usage)`. |

---

## LINE ITEMS: Messages `[EXISTS]`

Messages produced and consumed during the turn.

| Message Type | Direction | Status | Description |
|--------------|-----------|--------|-------------|
| Input (User / Steering / Follow-up) | Into LLM | `[EXISTS]` | Injected after `TurnStart`. Includes initial prompt messages (first turn only), pending steering messages, and follow-up messages. Each emits `MessageStart` / `MessageEnd` events. All carry the current `TurnId`. |
| Output (Assistant) | From LLM | `[EXISTS]` | The LLM's streamed response. Emitted as `MessageStart` -> `MessageUpdate` (streaming deltas) -> `MessageEnd`. Carries `StopReason`, model, provider, usage. Pushed to context and new_messages with `TurnId`. |

---

## LINE ITEMS: Tool Executions `[EXISTS]`

Tool calls extracted from the assistant message's `Content::ToolCall` items.

| Field | Status | Description |
|-------|--------|-------------|
| Tool calls | `[EXISTS]` | Extracted from `Message::Assistant.content` as `(id, name, arguments)` tuples. |
| `ToolExecutionStart` event | `[EXISTS]` | Emitted per tool call before `execute()`. Carries `tool_call_id`, `tool_name`, `args`. |
| `ToolExecutionUpdate` event | `[EXISTS]` | Emitted during execution for streaming partial results (via `ctx.on_update`). Not all tools emit these. |
| `ToolExecutionEnd` event | `[EXISTS]` | Emitted when tool finishes. Carries `result`, `is_error`, optional `child_loop_id` (for sub-agent tools). |
| `ProgressMessage` event | `[EXISTS]` | Plain text status updates from tools (via `ctx.on_progress`). |
| Tool results | `[EXISTS]` | `Message::ToolResult` messages appended to context with the current `TurnId`. Fed back to LLM in the next turn. |
| `TurnEnd.tool_results` | `[EXISTS]` | All tool result messages for this turn. Empty when no tool calls were made (`StopReason::Stop`). |

---

## LINE ITEMS: Streaming Deltas `[EXISTS]`

Incremental token-level updates from the LLM stream, carried on `MessageUpdate` events.

| Variant | Status | Description |
|---------|--------|-------------|
| `StreamDelta::Text { delta }` | `[EXISTS]` | A text token fragment from the LLM's response. |
| `StreamDelta::Thinking { delta }` | `[EXISTS]` | A thinking/reasoning chunk (extended thinking mode only). |
| `StreamDelta::ToolCallDelta { delta }` | `[EXISTS]` | A fragment of JSON arguments for a tool call. Must be accumulated and parsed after `MessageEnd`. |

---

## Per-Turn Event Ordering

The event ordering is strictly enforced every iteration of the inner loop in `run_loop`:

```
before_turn hook  ->  TurnStart event
                  ->  [MessageStart/End for prompt/steering messages]
                  ->  [Compaction if threshold exceeded]
                  ->  [MessageStart -> MessageUpdate* -> MessageEnd for assistant response]
                  ->  [ToolExecutionStart -> ToolExecutionUpdate* -> ToolExecutionEnd for each tool]
                  ->  TurnEnd event
                  ->  after_turn hook
```

---

## Code Reference

| File | What it contains |
|------|-----------------|
| `src/agent_loop/run.rs` | `run_loop` function — implements the turn cycle. `TurnStart` / `TurnEnd` event emission, `before_turn` / `after_turn` hook invocation, turn trigger determination, usage accumulation, tool call extraction and execution. |
| `src/types/event.rs` | `TurnTrigger` enum, `AgentEvent::TurnStart` and `AgentEvent::TurnEnd` variants, `StreamDelta` enum. |
| `src/types/agent_message.rs` | `TurnId` struct — `{ loop_id, turn_index }`. Carried on `LlmMessage.turn_id`. |
| `src/session/model.rs` | `Turn` struct — materialized turn record on `LoopRecord.turns`. Fields: `turn_id`, `triggered_by`, `usage`, `input_messages`, `output_message`, `tool_results`, `started_at`, `ended_at`. |
| `src/session/recorder.rs` | `SessionRecorder` — builds `Turn` structs from `TurnStart`/`MessageEnd`/`TurnEnd` event pairs. |

---

## Conceptual Notes

- **Turn as a first-class struct** is implemented. The `Turn` struct on `LoopRecord.turns` contains: `turn_id`, `triggered_by`, `usage`, `input_messages`, `output_message`, `tool_results`, `started_at`, `ended_at`. Built by `SessionRecorder` from `TurnStart`/`TurnEnd` event pairs. The flat `LoopRecord.messages` is kept independently for backward compatibility and use by compaction/context building. Old sessions without `turns` deserialize with an empty vec via `#[serde(default)]`.
- **Turn lifecycle** is entirely within a single Loop. A turn never spans loops. The inner loop in `run_loop` continues when there are tool calls or pending steering messages; each iteration is one turn.
- **Execution limits** are checked BEFORE `before_turn` fires, so hooks are not invoked for impossible turns. If a limit is reached, a system message (`[Agent stopped: ...]`) is emitted and the loop returns.
- **Compaction** can occur within a turn (after `TurnStart`, before the LLM call), making a single turn potentially include a compaction event in its span.
