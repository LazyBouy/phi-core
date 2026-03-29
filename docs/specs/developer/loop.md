# Loop

A complete record of one agent-loop execution, stored as `LoopRecord`. Loops are the iterations within a Session. Each Loop contains Turns (steps), tracks its model/provider configuration, accumulates usage, and links to parent/child loops for tree navigation.

Loops are created by `agent_loop` (origin loops) or `agent_loop_continue` (continuation loops). The `SessionRecorder` materializes `LoopRecord` structs from the `AgentStart` / `AgentEnd` event pairs.

## Concept Overview

```
Loop [EXISTS — LoopRecord]
├── HEADER
│   ├── loop_id [EXISTS] — "{session_id}.{config_segment}.{N}"
│   ├── status [EXISTS] — Pending/Running/Completed/Rejected/Aborted
│   ├── continuation_kind [EXISTS] — Default/Rerun/Branch/Compaction
│   ├── parent_loop_id [EXISTS]
│   ├── timing [EXISTS] — started_at, ended_at
│   ├── Model [EXISTS] — falls back: Loop → Session → Agent default
│   ├── config [EXISTS] — LoopConfigSnapshot
│   ├── usage, compaction_block [EXISTS]
│   └── Callbacks: before_loop / after_loop / on_error [EXISTS]
├── LINE ITEMS: Turns [EXISTS as events; PLANNED as struct]
├── LINE ITEMS: Same-session children, Sub-agent spawns [EXISTS]
├── LINE ITEMS: Parallel group [EXISTS]
└── LINE ITEMS: Events [EXISTS]
```

---

## HEADER

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `loop_id` | `String` | `[EXISTS]` | Unique identifier. Format: `"{session_id}.{config_segment}.{N}"`. The config_segment encodes which model/provider produced this loop. N is a monotonic counter per (session, config). |
| `session_id` | `String` | `[EXISTS]` | Session this loop belongs to. |
| `agent_id` | `String` | `[EXISTS]` | Agent that ran this loop. |
| `status` | `LoopStatus` | `[EXISTS]` | Lifecycle state: Pending, Running, Completed, Rejected, Aborted. See Status section below. |
| `continuation_kind` | `Option<ContinuationKind>` | `[EXISTS]` | How this loop relates to its parent. `None` for origin loops or sub-agent loops. `Some(Default)` for regular continuations. `Some(Rerun)` for retries. `Some(Branch)` for branch explorations. `Some(Compaction)` for standalone compaction passes. |
| `parent_loop_id` | `Option<String>` | `[EXISTS]` | The loop that directly preceded this one. `None` for origin loops. For sub-agent loops, points to the tool-call loop in a different session. |
| `started_at` | `DateTime<Utc>` | `[EXISTS]` | Timestamp from `AgentStart`. |
| `ended_at` | `Option<DateTime<Utc>>` | `[EXISTS]` | Timestamp from `AgentEnd`. `None` while running or pending. |
| `rejection` | `Option<String>` | `[EXISTS]` | Set when `AgentEnd.rejection` is `Some` (input filter blocked the run). |
| `metadata` | `Option<serde_json::Value>` | `[EXISTS]` | Opaque caller-supplied metadata from `AgentStart` (e.g., request id, trace ID). |

### Model for this Loop `[EXISTS]`

The model/provider identity is captured as a lightweight snapshot, not the full config (which contains secrets and non-serializable closures).

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `config` | `Option<LoopConfigSnapshot>` | `[EXISTS]` | Populated from the first `Message::Assistant` seen. `None` if loop ended before any assistant message. |
| `config.model` | `String` | `[EXISTS]` | Model id string (e.g., `"claude-opus-4-6"`, `"gpt-4o"`). |
| `config.provider` | `String` | `[EXISTS]` | Provider name (e.g., `"anthropic"`, `"openai"`). |
| `config.config_id` | `Option<String>` | `[EXISTS]` | Stable config identity from `AgentLoopConfig.config_id`. Matches the `config_segment` in `loop_id`. |

**Model fallback hierarchy**: Loop (`AgentLoopConfig.model_config`) -> Session (`[CONCEPTUAL]`) -> Agent default (`BasicAgent.model_config`).

### Usage `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `usage` | `Usage` | `[EXISTS]` | Token usage from `AgentEnd.usage`. Accumulated across all turns in this loop. Fields: input, output, reasoning, cache_read, cache_write, total_tokens. |

### Compaction `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `compaction_block` | `Option<CompactionBlock>` | `[EXISTS]` | Non-destructive compaction overlay. When `Some`, the context loader uses this block instead of raw messages. Original messages remain untouched. |

### Status `[EXISTS]`

Lifecycle state of a `LoopRecord`. Enum `LoopStatus`:

```
Pending -> Running -> Completed
                   -> Rejected
                   -> Aborted
```

| Variant | Status | Description |
|---------|--------|-------------|
| `Pending` | `[EXISTS]` | Loop id appeared in `ParallelLoopStart` but `AgentStart` has not yet arrived. Only for parallel-evaluation branches. |
| `Running` | `[EXISTS]` | `AgentStart` was received; the loop is executing. |
| `Completed` | `[EXISTS]` | `AgentEnd` was received with no rejection. |
| `Rejected` | `[EXISTS]` | `AgentEnd` was received with `rejection: Some(_)`. Input filter blocked the run. |
| `Aborted` | `[EXISTS]` | `SessionRecorder::flush` was called before `AgentEnd` arrived (e.g., process shutdown). |

### Callbacks `[EXISTS]`

| Callback | Status | Description |
|----------|--------|-------------|
| `before_loop` | `[EXISTS]` | Fires before `AgentStart` is emitted. Defined as `BeforeLoopFn` on `AgentLoopConfig`. Blank by default. |
| `after_loop` | `[EXISTS]` | Fires after `AgentEnd` is emitted. Defined as `AfterLoopFn`. Receives messages and usage. Blank by default. |
| `on_error` | `[EXISTS]` | Fires when `StopReason::Error` is encountered. Defined as `OnErrorFn`. Blank by default. |

---

## LINE ITEMS: Turns (Steps) `[EXISTS]` as events; `[PLANNED]` as struct

Turns exist as `TurnStart` / `TurnEnd` event pairs in the loop's event stream. See [turn.md](turn.md).

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| (derived from events) | — | `[EXISTS]` as event pairs | Each turn is bounded by `TurnStart` and `TurnEnd` events in `self.events`. |

---

## LINE ITEMS: Same-session Children `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `children_loop_ids` | `Vec<String>` | `[EXISTS]` | Loop IDs of same-session child loops (continuations, reruns, branches). Parent->children direction. Does not include cross-session sub-agent children. |

---

## LINE ITEMS: Sub-agent Spawns (Cross-session) `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `child_loop_refs` | `Vec<ChildLoopRef>` | `[EXISTS]` | Cross-session links to sub-agent loops spawned by tool calls. Each entry has: `tool_call_id`, `tool_name`, `child_loop_id`, `child_session_id`. |

`ChildLoopRef` fields:

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `tool_call_id` | `String` | `[EXISTS]` | The `ToolCall.id` that triggered sub-agent execution. |
| `tool_name` | `String` | `[EXISTS]` | The tool name that performed the spawn. |
| `child_loop_id` | `String` | `[EXISTS]` | The sub-agent's `AgentStart.loop_id`. |
| `child_session_id` | `String` | `[EXISTS]` | The sub-agent's session. Extracted from `child_loop_id` prefix. |

---

## LINE ITEMS: Parallel Group `[EXISTS]`

Set when this loop was part of an evaluational-parallelism group (`agent_loop_parallel`).

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `parallel_group` | `Option<ParallelGroupRecord>` | `[EXISTS]` | `None` for non-parallel loops. |
| `all_loop_ids` | `Vec<String>` | `[EXISTS]` | All branch loop IDs in config order. |
| `selected_loop_id` | `String` | `[EXISTS]` | The winning branch's loop ID. |
| `selected_config_index` | `usize` | `[EXISTS]` | 0-based index of the winner in the original configs. |
| `evaluation_usage` | `Usage` | `[EXISTS]` | Token usage from the judge LLM (zero for non-judge strategies). |
| `is_selected` | `bool` | `[EXISTS]` | `true` if this `LoopRecord` is the evaluation winner. |

---

## LINE ITEMS: Events `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `events` | `Vec<LoopEvent>` | `[EXISTS]` | Ordered event stream for this loop. |

Each `LoopEvent` has:

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `sequence` | `u64` | `[EXISTS]` | Monotonic counter (0-based). Gaps indicate filtered events (e.g., streaming deltas when `include_streaming_events` is false). |
| `event` | `AgentEvent` | `[EXISTS]` | The original event. `event.loop_id()` matches this `LoopRecord.loop_id`. |

---

## Messages `[EXISTS]`

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `messages` | `Vec<AgentMessage>` | `[EXISTS]` | All new messages produced by this loop, from `AgentEnd.messages`. Authoritative for replay and branching. |

---

## Loop Origin Classification

| `parent_loop_id` | `continuation_kind` | Meaning |
|---|---|---|
| `None` | `None` | Fresh origin loop (`agent_loop`) |
| `Some(p)`, same session | `Some(Default)` | Regular continuation |
| `Some(p)`, same session | `Some(Rerun)` | Retry / error recovery |
| `Some(p)`, same session | `Some(Branch)` | Branch exploration |
| `Some(p)`, different session | `None` | Sub-agent loop (spawned by a tool) |

---

## Code Reference

| File | What it contains |
|------|-----------------|
| `src/session/model.rs` | `LoopRecord` struct, `LoopStatus` enum, `LoopConfigSnapshot` struct, `ChildLoopRef` struct, `ParallelGroupRecord` struct, `LoopEvent` struct, `OpenLoop` struct. |
| `src/agent_loop/run.rs` | `run_loop` function — the core loop engine. Implements the outer loop (follow-ups) and inner loop (tool calls + steering). Accumulates `Usage`, fires turn events and hooks. |

---

## Conceptual Notes

- **Session-level model override** is not yet implemented. The fallback chain is currently Loop -> Agent default. Adding a Session layer would make it Loop -> Session -> Agent default, matching the conceptual hierarchy.
- **Turns as a struct** within LoopRecord is planned but not yet materialized. Currently turns are reconstructed from `TurnStart` / `TurnEnd` event pairs in the `events` vec. A future `Turn` struct on LoopRecord would make turn-level querying more direct.
- **LoopConfigSnapshot** intentionally does not store the full `AgentLoopConfig` because it contains API keys and non-serializable hook closures. The snapshot captures just enough for cost attribution, replay identification, and parallel branch differentiation.
