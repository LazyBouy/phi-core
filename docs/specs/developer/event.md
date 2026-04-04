# Event Lifecycle

`AgentEvent` is the runtime's event vocabulary -- it captures every significant happening in the agent loop that a UI, logger, or analysis consumer might react to. Events are emitted through an `mpsc::UnboundedSender<AgentEvent>` channel during execution and consumed by `SessionRecorder` (or any external subscriber) on the receiving end.

## Concept Overview

```
Event [EXISTS]
├── AgentEvent [EXISTS] — 15 variants
│   ├── Session: AgentStart/End [EXISTS]
│   ├── Loop: ParallelLoopStart/End, CompactionStarted/Ended [EXISTS]
│   ├── Turn: TurnStart/End [EXISTS]
│   ├── Message: MessageStart/Update/End [EXISTS]
│   ├── Tool: ToolExecutionStart/Update/End, ProgressMessage [EXISTS]
│   └── Input: InputRejected [EXISTS]
├── StreamDelta [EXISTS] — Text/Thinking/ToolCallDelta
├── ContinuationKind [EXISTS] — Default/Rerun/Branch/Compaction
└── TurnTrigger [EXISTS] — User/SubAgent/Continuation/Branch
```

---

## AgentEvent [EXISTS]

15 variants grouped by scope. Each variant carries a `loop_id` for correlation (except `ParallelLoopStart/End` which use `session_id`).

### Session-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `AgentStart` | [EXISTS] | `agent_id`, `session_id`, `loop_id`, `parent_loop_id`, `continuation_kind`, `timestamp`, `metadata` | Fires once when `agent_loop()` is entered, before any LLM call |
| `AgentEnd` | [EXISTS] | `loop_id`, `messages`, `usage`, `timestamp`, `rejection` | Fires once when `agent_loop()` exits; `rejection` is `Some` if an InputFilter blocked the input |

### Loop-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `ParallelLoopStart` | [EXISTS] | `session_id`, `loop_ids`, `timestamp` | Emitted before parallel branch dispatch; lists all branch loop_ids |
| `ParallelLoopEnd` | [EXISTS] | `session_id`, `selected_loop_id`, `selected_config_index`, `evaluation_usage`, `timestamp` | Emitted after evaluation selects a winning branch |
| `CompactionStarted` | [EXISTS] | `loop_id`, `estimated_tokens`, `message_count`, `timestamp` | Emitted before compaction strategy runs |
| `CompactionEnded` | [EXISTS] | `loop_id`, `messages_before`, `messages_after`, `estimated_tokens_before`, `estimated_tokens_after`, `loops_compacted`, `timestamp` | Emitted after compaction completes |

### Turn-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `TurnStart` | [EXISTS] | `loop_id`, `turn_index`, `timestamp`, `triggered_by` | Fires at the start of each LLM turn (one LLM call = one turn) |
| `TurnEnd` | [EXISTS] | `loop_id`, `message`, `usage`, `timestamp`, `tool_results` | Fires at the end of each LLM turn |

### Message-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `MessageStart` | [EXISTS] | `loop_id`, `message` | New message created (assistant: when SSE stream opens; user/tool: immediately) |
| `MessageUpdate` | [EXISTS] | `loop_id`, `message`, `delta` | Streaming token/chunk; `delta` is the increment, `message` is the accumulator |
| `MessageEnd` | [EXISTS] | `loop_id`, `message` | Message fully complete; safe to persist |

### Tool-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `ToolExecutionStart` | [EXISTS] | `loop_id`, `tool_call_id`, `tool_name`, `args` | Tool call begins (before `execute()`) |
| `ToolExecutionUpdate` | [EXISTS] | `loop_id`, `tool_call_id`, `tool_name`, `partial_result` | Mid-execution partial result (via `ctx.on_update`) |
| `ToolExecutionEnd` | [EXISTS] | `loop_id`, `tool_call_id`, `tool_name`, `result`, `is_error`, `child_loop_id` | Tool finished; `child_loop_id` is `Some` for sub-agent tools |
| `ProgressMessage` | [EXISTS] | `loop_id`, `tool_call_id`, `tool_name`, `text` | User-facing status text (via `ctx.on_progress`) |

### Input-scoped Events

| Variant | Status | Fields | Description |
|---------|--------|--------|-------------|
| `InputRejected` | [EXISTS] | `loop_id`, `reason` | InputFilter rejected the user's message; agent loop returns immediately |

---

## Event Scoping (Bracket Relationships)

Events form a nested bracket structure:

```
AgentStart                         -- session-scoped
  TurnStart                        -- turn-scoped (0-based index)
    MessageStart                   -- message-scoped (assistant message)
      MessageUpdate (N times)      -- streaming deltas
    MessageEnd
    ToolExecutionStart             -- tool-scoped (per tool call)
      ToolExecutionUpdate (0..N)   -- partial results
      ProgressMessage (0..N)       -- status text
    ToolExecutionEnd
    MessageStart                   -- message-scoped (tool result message)
    MessageEnd
  TurnEnd
  TurnStart                        -- next turn (tool round-trip)
    ...
  TurnEnd
AgentEnd                           -- session-scoped
```

For parallel evaluation:
```
ParallelLoopStart                  -- loop-scoped (lists all branch IDs)
  AgentStart (branch 1)            -- nested full lifecycle per branch
  AgentEnd (branch 1)
  AgentStart (branch 2)
  AgentEnd (branch 2)
ParallelLoopEnd                    -- loop-scoped (announces winner)
```

---

## StreamDelta [EXISTS]

Incremental token-level updates from the LLM stream. Carried inside `MessageUpdate` events.

| Variant | Status | Description |
|---------|--------|-------------|
| `Text { delta }` | [EXISTS] | A text token fragment |
| `Thinking { delta }` | [EXISTS] | A thinking/reasoning chunk (extended thinking mode only) |
| `ToolCallDelta { delta }` | [EXISTS] | A fragment of tool call argument JSON (accumulate until `MessageEnd`) |

---

## ContinuationKind [EXISTS]

How an `agent_loop_continue` call relates to the session's prior loops. Surfaced in `AgentStart` for observability.

| Variant | Status | Description |
|---------|--------|-------------|
| `Default` | [EXISTS] | Unspecified continuation; preserves original semantics |
| `Rerun { tag }` | [EXISTS] | Retry from equivalent state; tag is RFC 3339 UTC timestamp |
| `Branch { tag }` | [EXISTS] | Exploration of a different path from a branching point |
| `Compaction` | [EXISTS] | Standalone context-compaction pass; no LLM call |

---

## TurnTrigger [EXISTS]

Identifies what caused a new turn to begin. Carried in `TurnStart`.

| Variant | Status | Description |
|---------|--------|-------------|
| `User` | [EXISTS] | First turn triggered by a user message |
| `SubAgent` | [EXISTS] | Invoked as a sub-agent by a parent agent |
| `Continuation` | [EXISTS] | Continuation turn: tool round-trip, steering, or Default/Rerun continuation |
| `Branch` | [EXISTS] | First turn of a Branch continuation; subsequent turns use Continuation |

---

## Event Flow

```
Producer: agent_loop (src/agent_loop/)
    |
    | mpsc::UnboundedSender<AgentEvent>
    v
Consumer: SessionRecorder (src/session/recorder.rs)
    |
    | on_event() dispatches by variant
    v
Storage: Session -> LoopRecord -> LoopEvent[]
```

The `SessionRecorder` consumes events and builds a structured tree:
- `AgentStart` opens a `LoopRecord` (status: `Running`)
- `AgentEnd` closes it (status: `Completed` or `Rejected`)
- `TurnEnd` extracts config snapshots from assistant messages
- `ToolExecutionEnd` records `ChildLoopRef` for sub-agent traceability
- `ParallelLoopEnd` retroactively sets `ParallelGroupRecord` on all branch records
- `MessageUpdate` events are optionally recorded (off by default; 100-1000x more numerous)
- All other events append to `LoopRecord.events` as `LoopEvent { sequence, event }`

---

## Code Reference

| Concept | File |
|---------|------|
| `AgentEvent`, `StreamDelta`, `ContinuationKind`, `TurnTrigger` | `src/types/event.rs` |
| `SessionRecorder`, `SessionRecorderConfig` | `src/session/recorder.rs` |
| Event emission (AgentStart, TurnStart, MessageUpdate, etc.) | `src/agent_loop/run.rs`, `src/agent_loop/streaming.rs` |
| Tool lifecycle events (ToolExecutionStart/Update/End) | `src/agent_loop/tools.rs` |
| `LoopRecord`, `LoopEvent`, `Session` | `src/session/model.rs` |

---

## Conceptual Notes

- **before_task / after_task callbacks** [CONCEPTUAL] -- `AgentStart` and `AgentEnd` are session-scoped and should trigger `before_task` / `after_task` callbacks. Currently these callbacks do not exist; `before_loop` / `after_loop` serve a similar role at the loop level but are not semantically session-scoped.
- **Session Scope** [CONCEPTUAL] -- The plan envisions sessions having an explicit scope (Ephemeral vs Persistent). Persistent sessions would mandate Introspection. This scope would influence which events trigger persistence.
- **Error Events** -- The current design uses `StopReason::Error` and the `on_error` callback for LLM errors. A dedicated `AgentEvent::Error` variant for more granular error reporting (tool failures, network issues, etc.) is noted as a potential improvement in the source comments.
- **Event Replay** -- `LoopRecord.events` stores the full event stream (as `Vec<LoopEvent>`), enabling replay or analysis of past runs. `SessionRecorderConfig.include_streaming_events` controls whether the high-volume `MessageUpdate` deltas are included.
