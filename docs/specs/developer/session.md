<!-- Last verified: 2026-04-05 by Claude Code -->
# Session

A named container grouping all `LoopRecord`s for one agent session. A Session represents a task the agent performs. It has identity, formation history, configuration, and contains an ordered sequence of Loops (iterations).

Sessions are created automatically by `SessionRecorder` when a new `session_id` first appears in an `AgentStart` event, or explicitly by the caller.

## Concept Overview

```
Session [EXISTS]
├── HEADER
│   ├── session_id, agent_id [EXISTS]
│   ├── formation [EXISTS] — Explicit / FirstLoop / InactivityTimeout
│   ├── scope [EXISTS] — Ephemeral / Persistent (SessionScope enum)
│   ├── created_at, last_active_at [EXISTS]
│   ├── parent_spawn_ref [EXISTS] — cross-session link
│   ├── Task Name, Task Status [CONCEPTUAL]
│   └── Callbacks: before_task / after_task [EXISTS]
├── LINE ITEMS: Loops [EXISTS]
├── LINE ITEMS: Input Filters [EXISTS]
└── SUMMARY: total_usage(), loop_chain_to() [EXISTS]
```

---

## HEADER

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `session_id` | `String` | `[EXISTS]` | Stable identifier. Matches `AgentStart.session_id`. Generated as UUID v4 at `BasicAgent::new()`. |
| `agent_id` | `String` | `[EXISTS]` | The agent that owns this session. Taken from the first `AgentStart` event. |
| `formation` | `SessionFormation` | `[EXISTS]` | How the session was created. See Formation section below. |
| `scope` | `SessionScope` | `[EXISTS]` | `Ephemeral` (default, in-memory only) or `Persistent` (session logs retained). Declared via config `[session] scope = "persistent"`. |
| `created_at` | `DateTime<Utc>` | `[EXISTS]` | Timestamp of the first `AgentStart` event for this session. |
| `last_active_at` | `DateTime<Utc>` | `[EXISTS]` | Updated each time a new loop opens (on `AgentStart`). Reflects when the last loop started, not when it last had activity. |
| `parent_spawn_ref` | `Option<SpawnRef>` | `[EXISTS]` | Cross-session link when this session was spawned as a sub-agent. Points back to parent session, loop, tool call. Inverse of `LoopRecord.child_loop_refs`. |
| Task Name | `String` | `[CONCEPTUAL]` | Human-readable label for the task this session represents. |
| Task Status | enum | `[CONCEPTUAL]` | Status of the task (e.g., Pending, Running, Completed, Failed). Derived from loop statuses but would be a first-class field. |

### Formation `[EXISTS]`

How the session was initially created. Enum `SessionFormation`:

| Variant | Status | Description |
|---------|--------|-------------|
| `Explicit { timestamp }` | `[EXISTS]` | Created by direct construction (tests, tooling). `SessionRecorder` never sets this. |
| `FirstLoop { timestamp }` | `[EXISTS]` | Created automatically when a new `session_id` first appeared in an `AgentStart` event. |
| `InactivityTimeout { threshold_secs, previous_session_id, timestamp }` | `[EXISTS]` | New session opened because the agent was idle longer than the threshold. Requires prior `session_id` rotation via `BasicAgent::check_and_rotate`. |

### Callbacks `[EXISTS]`

Callbacks are configured on `SessionRecorderConfig`, not on the `Session` struct directly.

| Callback | Type | Status | Description |
|----------|------|--------|-------------|
| `before_task` | `Option<BeforeTaskFn>` | `[EXISTS]` | Fires on the first `AgentStart` event with a new `session_id`. Blank by default. |
| `after_task` | `Option<AfterTaskFn>` | `[EXISTS]` | Fires on `flush()`. Blank by default. |

---

## LINE ITEMS: Loops (Iterations) `[EXISTS]`

Ordered list of all `LoopRecord`s in this session, sorted by `started_at`.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `loops` | `Vec<LoopRecord>` | `[EXISTS]` | All completed and in-progress loop records. See [loop.md](loop.md). |

### Loop Tree Structure

The tree is implicit via `parent_loop_id` / `children_loop_ids` links:

- **Root loops** -- `parent_loop_id` is `None` (or points to a loop in a different session for sub-agent roots).
- **Continuation chains** -- `parent_loop_id` -> `loop_id` within the same session.
- **Parallel branches** -- siblings sharing the same `parent_loop_id`, each with `parallel_group` set.
- **Sub-agent children** -- in `child_loop_refs` on the parent loop (cross-session, not in `loops` vec).

---

## LINE ITEMS: Input Filters `[EXISTS]`

Input filters validate user messages before the LLM is called. Stored on `AgentLoopConfig.input_filters`, conceptually a Session-level concern.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `input_filters` | `Vec<Arc<dyn InputFilter>>` | `[EXISTS]` | Each filter returns Pass, Warn, or Reject for a given message. Reject aborts the loop before any LLM call and emits `InputRejected`. |

---

## SUMMARY Methods `[EXISTS]`

Methods on the `Session` struct for querying and aggregating.

| Method | Status | Description |
|--------|--------|-------------|
| `total_usage()` | `[EXISTS]` | Cumulative `Usage` across all loops. Sums input, output, reasoning, cache_read, cache_write, total_tokens. |
| `loop_chain_to(target_loop_id)` | `[EXISTS]` | Builds the linear chain of loop IDs from root to target by walking `parent_loop_id` links backward. Returns chronological order (root first). Handles parallel branches (only selected path) and reruns (only active ancestor chain). |
| `root_loops()` | `[EXISTS]` | Returns loops whose `parent_loop_id` is `None` or belongs to a different session. |
| `children_of(loop_id)` | `[EXISTS]` | Returns direct same-session children of a loop. |
| `parallel_siblings(loop_id)` | `[EXISTS]` | Returns all loops in the same parallel group. |
| `get_loop(loop_id)` | `[EXISTS]` | Look up a loop by ID. |

---

## Code Reference

| File | What it contains |
|------|-----------------|
| `src/session/model.rs` | `Session` struct, `SessionFormation` enum, `SpawnRef` struct, `SessionError` enum. All methods (`total_usage`, `loop_chain_to`, `root_loops`, `children_of`, `parallel_siblings`, `get_loop`). |

---

## Conceptual Notes

- **Session scope** (Ephemeral vs Persistent) does not exist in code. All sessions are currently ephemeral by default. Adding scope would gate whether Introspection is required.
- **Model/thinking/temperature per-loop** -- These settings are no longer on `Session`. They are tracked per-loop via `LoopConfigSnapshot` on each `LoopRecord` (see [loop.md](loop.md)). The fallback hierarchy is Loop -> Agent default.
- **Task Name and Task Status** would give sessions first-class task identity, enabling task dashboards and workflow tracking.
- **before_task / after_task callbacks** now exist on `SessionRecorderConfig`. `before_task` fires on the first `AgentStart` with a new `session_id`; `after_task` fires on `flush()`. This mirrors the existing before_loop/after_loop and before_turn/after_turn callback pattern at the Session level.
