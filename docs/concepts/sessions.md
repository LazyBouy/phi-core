<!-- Last verified: 2026-05-16 by Claude Code -->

# Sessions

A **Session** is a named container (keyed by `session_id`) that groups all
`LoopRecord`s belonging to one agent session. Sessions provide persistent,
structured memory of every agent interaction — suitable for logging, replay,
branching, and tracing agent-spawning chains.

The `session` module is split into sub-modules: `model`, `recorder`, `storage`, `helpers`.

```
Session (session_id)
├── LoopRecord (loop_id: A)       ← origin loop
│   ├── LoopRecord (loop_id: B)   ← continuation of A
│   └── LoopRecord (loop_id: C)   ← another continuation of A
│       ├── LoopRecord (loop_id: D)  ← parallel branch
│       └── LoopRecord (loop_id: E)  ← parallel branch (selected)
└── child_loop_refs → Session (sub-agent session)
```

---

## Overview

| Concept | Description |
|---|---|
| `Session` | Container for all loops belonging to one `session_id` |
| `LoopRecord` | Complete record of one `agent_loop` / `agent_loop_continue` execution |
| `LoopEvent` | One event in a loop's ordered event stream |
| `SessionRecorder` | Stateful consumer that builds sessions from `AgentEvent` streams |

### Relationship to loops

One session contains many loops. Loops within a session form a **tree** via
`parent_loop_id` / `children_loop_ids` links. Parallel-evaluation branches
form a sibling group linked by `ParallelGroupRecord`. Sub-agent loops are
cross-session (different `session_id`) and connected via `ChildLoopRef` /
`SpawnRef` instead.

---

## Session Formation

A new `Session` is opened when `SessionRecorder` first encounters a `session_id`
it has not seen before. Three scenarios produce a new session:

### `PerSessionId` (default)

One `Session` per `session_id`. Maps naturally onto `BasicAgent` lifetime —
one `BasicAgent` instance = one session for its entire lifetime.

```rust
let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
// Every event from a single BasicAgent feeds into one Session.
```

**When to use:** The default for most applications. No infrastructure needed.

### `InactivityTimeout`

Opens a new session when the agent has been idle for longer than a configured
threshold. Requires the caller to rotate `session_id` beforehand — the recorder
detects the new `session_id` on the next `AgentStart`.

```rust
// In your agent orchestrator, before prompting:
if agent.check_and_rotate(Duration::from_secs(1800)).is_some() {
    println!("Started new session after 30 minutes idle");
}
```

**When to use:** Long-running assistants where each "conversation" should be a
distinct session even if the `BasicAgent` object persists.

### Explicit rotation

Call `BasicAgent::new_session()` directly to rotate immediately.

```rust
let new_id = agent.new_session();
// All subsequent loops belong to the new session.
```

**When to use:** At conversation boundaries you control explicitly (e.g. "clear
chat" button, new document context).

---

## LoopRecord Anatomy

### Field table

| Field | Type | Description |
|---|---|---|
| `loop_id` | `String` | Unique id for this execution |
| `session_id` | `String` | Session this loop belongs to |
| `agent_id` | `String` | Agent that ran this loop |
| `parent_loop_id` | `Option<String>` | Preceding loop (same or different session) |
| `continuation_kind` | `ContinuationKind` | How this loop relates to its parent (`Initial` for first loops) |
| `started_at` | `DateTime<Utc>` | Timestamp from `AgentStart` |
| `ended_at` | `Option<DateTime<Utc>>` | Timestamp from `AgentEnd` |
| `status` | `LoopStatus` | Lifecycle state |
| `rejection` | `Option<String>` | Input-filter rejection reason (if any) |
| `config` | `Option<LoopConfigSnapshot>` | Model/provider that ran this loop |
| `messages` | `Vec<AgentMessage>` | All new messages produced (from `AgentEnd`) |
| `turns` | `Vec<Turn>` | Materialized turn records (one per LLM call-response cycle). Built from `TurnStart`/`TurnEnd` event pairs. Empty for old sessions or loops that ended before any turn completed. |
| `usage` | `Usage` | Token usage for this loop |
| `metadata` | `Option<Value>` | Caller-supplied metadata from `AgentStart` |
| `events` | `Vec<LoopEvent>` | Full ordered event stream |
| `children_loop_ids` | `Vec<String>` | Same-session child loops (parent→children) |
| `child_loop_refs` | `Vec<ChildLoopRef>` | Cross-session sub-agent spawn links |
| `compaction_block` | `Option<CompactionBlock>` | Non-destructive compaction overlay (see below) |
| `parallel_group` | `Option<ParallelGroupRecord>` | Parallel-evaluation group metadata |

### `LoopStatus` lifecycle

```
                              AgentEnd (no rejection)
┌─────────┐  AgentStart  ┌─────────┐ ───────────────────────► ┌───────────┐
│ Pending ├─────────────►│ Running │                           │ Completed │
└─────────┘              └────┬────┘ AgentEnd (rejection Some) └───────────┘
                               │ ────────────────────────────► ┌──────────┐
                               │                               │ Rejected │
                               │ flush() before AgentEnd       └──────────┘
                               └─────────────────────────────► ┌─────────┐
                                                               │ Aborted  │
                                                               └─────────┘
```

`Pending` is only used for parallel-evaluation branches: they are pre-registered
when `ParallelLoopStart` arrives, before their individual `AgentStart` fires.

### `continuation_kind` classification

| `parent_loop_id` | `continuation_kind` | Meaning |
|---|---|---|
| `None` | `Initial` | Fresh origin loop (`agent_loop`) |
| Same-session parent | `Default` | Regular continuation |
| Same-session parent | `Rerun { tag }` | Retry / error recovery |
| Same-session parent | `Branch { tag }` | Branch exploration |
| Different-session parent | `Initial` | Sub-agent loop (spawned by a tool) |

### `LoopConfigSnapshot`

`LoopConfigSnapshot` captures model identity and key configuration from
the `AgentLoopConfig` that ran the loop:

```rust
pub struct LoopConfigSnapshot {
    pub model: String,                    // e.g. "claude-opus-4-6"
    pub provider: String,                 // e.g. "anthropic"
    pub config_id: Option<String>,        // from AgentLoopConfig.config_id
    pub name: Option<String>,             // model display name
    pub api: Option<ApiProtocol>,         // which API protocol was used
    pub base_url: Option<String>,         // provider base URL
    pub reasoning: Option<bool>,          // whether model supports reasoning/thinking
    pub context_window: Option<u32>,      // model context window size
    pub max_tokens: Option<u32>,          // max output tokens
    pub thinking_level: Option<ThinkingLevel>, // reasoning depth for this loop
    pub temperature: Option<f32>,         // sampling temperature
}
```

The first three fields (`model`, `provider`, `config_id`) are always populated.
The remaining fields use `Option` with `#[serde(skip_serializing_if = "Option::is_none")]`
so they only appear when set, keeping serialized output compact.

**Why not store the full `AgentLoopConfig`?** The full config contains API keys
(in `ModelConfig.api_key`) and non-serialisable hook closures. Storing it would
require stripping secrets and skipping closures for little extra value.
`LoopConfigSnapshot` is sufficient for cost attribution, replay (the caller
reconstructs the config), and identifying parallel branches (e.g. "haiku vs. opus").

> **Note:** `thinking_level` and `temperature` were previously stored on the
> `Session` struct. They are now tracked per-loop in `LoopConfigSnapshot`,
> which more accurately reflects that these settings can vary between loops
> (e.g. across parallel evaluation branches with different configs).

### `events` field

`LoopRecord.events` contains every `AgentEvent` emitted during the loop, in
order, tagged with a monotonic `sequence` counter.

`MessageUpdate` (streaming delta) events are **excluded by default** — they are
100–1 000× more numerous than final messages and are not needed for replay.
Enable them with `SessionRecorderConfig { include_streaming_events: true, .. }`.

`AgentEnd.messages` is the **authoritative message source** for a loop.
`LoopRecord.messages` is populated directly from it. Reconstructing messages
from `MessageStart`/`MessageEnd` events would be fragile.

### `compaction_block` field

`LoopRecord.compaction_block` holds a non-destructive compaction overlay. When
present, the context loader uses this block instead of the raw `messages` field
to reconstruct the agent's working context. The original `messages` remain
authoritative for replay and branching — they are never mutated or discarded.
This overlay model means compaction is always reversible: removing or replacing
the `CompactionBlock` restores the original conversation without data loss.

### Bidirectional parent↔child links

Both directions of the loop tree are maintained:

- `LoopRecord.parent_loop_id` — child → parent (set at loop creation)
- `LoopRecord.children_loop_ids` — parent → children (appended at `AgentEnd`)

This allows O(1) traversal in either direction without scanning the full
`loops` vec.

---

## Loop Tree Navigation

`Session` provides four navigation methods:

```rust
// Root loops — no parent in this session.
session.root_loops();

// Direct same-session children of a loop.
session.children_of("loop-id-A");

// All parallel siblings (including the loop itself).
session.parallel_siblings("loop-id-branch-1");

// Lookup by id.
session.get_loop("loop-id-X");

// Cumulative token usage for the whole session.
session.total_usage();
```

### Reconstructing a conversation thread

Follow the parent→child chain from a root:

```rust
fn print_thread(session: &Session, loop_id: &str, indent: usize) {
    if let Some(lr) = session.get_loop(loop_id) {
        println!("{:indent$}{loop_id}: {:?}", "", lr.status, indent = indent);
        for child_id in &lr.children_loop_ids {
            print_thread(session, child_id, indent + 2);
        }
    }
}

for root in session.root_loops() {
    print_thread(&session, &root.loop_id, 0);
}
```

### Identifying branches

Branches share the same `parent_loop_id` and each has `parallel_group` set:

```rust
let branches: Vec<_> = session.parallel_siblings("branch-loop-id").collect();
let winner = branches.iter().find(|l| {
    l.parallel_group.as_ref().map(|pg| pg.is_selected).unwrap_or(false)
});
```

---

## Cross-Session Sub-Agent Tracking

Sub-agents run with their own `session_id`. phi-core maintains bidirectional
links between the parent session and the child session:

```
Parent Session                         Child Session
──────────────────────                 ──────────────────────────
LoopRecord (loop-P)                    Session
  child_loop_refs:                       parent_spawn_ref:
    ChildLoopRef {                         SpawnRef {
      tool_call_id: "call-1"               parent_session_id: "sess-P"
      tool_name: "sub_agent"               parent_loop_id: "loop-P"
      child_loop_id: "loop-C"              tool_call_id: "call-1"
      child_session_id: "sess-C"           tool_name: "sub_agent"
    }                                    }
```

### Tracing a full spawn chain

```rust
// Load parent and child sessions from disk.
let parent = load_session("sess-P", dir)?;
let child = load_session("sess-C", dir)?;

// From parent: find all sub-agent spawns.
for lr in &parent.loops {
    for child_ref in &lr.child_loop_refs {
        println!("Tool {} spawned sub-agent loop {}",
            child_ref.tool_name, child_ref.child_loop_id);
    }
}

// From child: find the parent that triggered it.
if let Some(ref sr) = child.parent_spawn_ref {
    println!("This session was spawned by {} in session {}",
        sr.tool_name, sr.parent_session_id);
}
```

### Why sub-agents get separate sessions

Sub-agents have clean identity boundaries — they can be loaded and analyzed
independently of their parent. Embedding child data inside the parent session
would bloat the parent record and couple two independent execution traces.
The bidirectional `ChildLoopRef` / `SpawnRef` pair provides a complete spawn
graph without that coupling.

---

## Parallel Evaluation Groups

When `agent_loop_parallel` runs N branches, each branch gets its own
`LoopRecord`. All N records are linked by `ParallelGroupRecord`:

```rust
pub struct ParallelGroupRecord {
    pub all_loop_ids: Vec<String>,       // all branch loop_ids in config order
    pub selected_loop_id: String,        // winner chosen by EvaluationStrategy
    pub selected_config_index: usize,    // 0-based index into original configs
    pub evaluation_usage: Usage,         // judge LLM tokens (zero if no judge)
    pub is_selected: bool,               // true only on the winner's record
}
```

`LoopStatus::Pending` is used before `AgentStart` arrives for each branch.
`ParallelLoopStart` announces all `loop_id`s in advance, so the group can be
registered immediately without retroactive wiring.

---

## SessionRecorder Usage

Wire the recorder to your agent's event channel:

```rust
use phi_core::session::{SessionRecorder, SessionRecorderConfig, save_session};
use phi_core::AgentEvent;
use std::path::Path;
use tokio::sync::mpsc;

let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());

// Spawn a task to consume events from the channel.
tokio::spawn(async move {
    while let Some(event) = rx.recv().await {
        recorder.on_event(event);
    }
    // Channel closed — flush and persist.
    recorder.flush();
    for session in recorder.drain_completed() {
        save_session(&session, Path::new("./sessions")).unwrap();
    }
});

// Pass tx to agent_loop / agent_loop_continue / BasicAgent.
```

### `include_streaming_events`

Enable only when you need to replay or audit the raw token stream:

```rust
SessionRecorderConfig {
    include_streaming_events: true,
    ..Default::default()
}
```

Storage implications: a single turn with extended thinking may produce thousands
of `MessageUpdate` events. Each is a full clone of the accumulated message plus
the delta.

---

## Session Lifecycle Callbacks

`SessionRecorderConfig` supports two session-level callbacks for billing, audit, and metrics:

| Field | Type | Description |
|---|---|---|
| `before_task` | `Option<BeforeTaskFn>` | `Arc<dyn Fn(&Session) -> bool>`. Fires on the first `AgentStart` with a new `session_id`. Return `false` to reject. Use for session initialization, billing setup, or audit logging. |
| `after_task` | `Option<AfterTaskFn>` | `Arc<dyn Fn(&Session)>`. Fires in `flush()` when the session is finalized. Use for billing finalization, metrics emission, or cleanup. |

These are **session-level** (not loop-level) hooks. Unlike `before_loop`/`after_loop` on `AgentLoopConfig` which fire around each individual agent loop, `before_task` and `after_task` fire once per session lifecycle:

```rust
use phi_core::session::{SessionRecorder, SessionRecorderConfig};

let config = SessionRecorderConfig {
    before_task: Some(Arc::new(|session: &phi_core::session::Session| -> bool {
        println!("Session started: {}", session.session_id);
        // Initialize billing, start audit trail, etc.
        true // return false to reject the session
    })),
    after_task: Some(Arc::new(|session| {
        println!("Session finalized: {} ({} loops)", session.session_id, session.loops.len());
        // Finalize billing, emit metrics, etc.
    })),
    ..Default::default()
};

let mut recorder = SessionRecorder::new(config);
```

---

## Persistence API

| Function | Description |
|---|---|
| `save_session(session, dir)` | Write `{dir}/{session_id}.json` (atomic via tmp + rename) |
| `load_session(session_id, dir)` | Read `{dir}/{session_id}.json` |
| `list_session_ids(dir)` | List all `.json` filenames, newest first |
| `load_sessions_for_agent(agent_id, dir)` | Load all sessions matching `agent_id` |
| `delete_session(session_id, dir)` | Remove `{dir}/{session_id}.json` |

File format: pretty-printed JSON (`serde_json::to_writer_pretty`).
Directory layout: flat — `{dir}/{session_id}.json`, no sub-directories, no index.
Writes are atomic: the implementation writes to a temp file then renames over the
target, so readers never observe a partially-written file.

### Pluggable store trait *(0.7.0+)*

For callers that want to swap the persistence backend (e.g. S3, SQLite, an
in-memory fake for tests) or that need concurrent-writer safety, phi-core
exposes a `SessionStore` async trait alongside the free functions:

```rust
use phi_core::session::{SessionStore, FileSystemSessionStore};

let store = FileSystemSessionStore::new("./sessions");
store.save(&session).await?;          // acquires fs2 exclusive lock
let loaded = store.load("sess-1").await?;
let ids    = store.list_ids().await?;
store.delete("sess-1").await?;
```

`FileSystemSessionStore::save()` takes an advisory `fs2` exclusive lock on the
target file before the atomic rename. Concurrent writers to the same
`session_id` get back `SessionError::Locked { session_id }` instead of silently
producing a corrupt file. Readers take a shared lock and so coexist with
themselves.

The free `save_session()` / `load_session()` / etc. functions remain available
and unchanged — use the trait when you need pluggability or contention safety.

### When to call `flush()`

Call `flush()` before saving to finalize any loops that have not received
`AgentEnd` yet (e.g. on process shutdown). Flushed loops get status `Aborted`.

```rust
recorder.flush();
let sessions = recorder.drain_completed();
for s in &sessions {
    save_session(s, Path::new("./sessions"))?;
}
```

---

## Design Decisions

### 1. `loop_id` on every `AgentEvent` variant

**Decision:** Add `loop_id: String` to all 11 `AgentEvent` variants that lacked it.

**Why:** `agent_loop_parallel` interleaves branch events on one `tx` channel.
Without `loop_id` on every event, `TurnStart`, `ToolExecutionEnd`, etc. cannot
be reliably attributed to the correct branch `LoopRecord`. The only alternative
— heuristically assigning events to the last-opened loop — produces incorrect
records when two branches overlap.

**Rejected alternative:** Last-opened-loop heuristic. Rejected because parallel
branches genuinely interleave; the heuristic would silently misattribute events.

---

### 2. `LoopStatus::Pending` for parallel branches

**Decision:** Pre-register `LoopRecord { status: Pending }` entries when
`ParallelLoopStart` arrives, before their `AgentStart` events fire.

**Why:** `ParallelLoopStart` announces all `loop_id`s in advance.
Pre-creating records lets the `ParallelGroupRecord` be registered immediately,
so no retroactive wiring is needed when each branch's `AgentStart` arrives later.

**Rejected alternative:** Create `LoopRecord`s only on `AgentStart` and
retroactively set `ParallelGroupRecord` on `ParallelLoopEnd`. Rejected because
it requires a second pass over all records and makes the group state inconsistent
during the parallel execution window.

---

### 3. Messages from `AgentEnd`, not reconstructed from events

**Decision:** `LoopRecord.messages` is populated directly from `AgentEnd.messages`.

**Why:** `AgentEnd.messages` is the authoritative, ordered list of all messages
produced by a loop. The LLM loop already assembles this — there is no value in
re-assembling it from `MessageStart`/`MessageEnd` events in the recorder.

**Rejected alternative:** Reconstruct messages from streaming events. Rejected
because it duplicates work, is fragile (missed events, ordering edge cases), and
requires special handling for partial messages.

---

### 4. Bidirectional parent↔child within a session

**Decision:** Maintain both `parent_loop_id` (child→parent) and
`children_loop_ids` (parent→children) on every `LoopRecord`.

**Why:** O(1) traversal in both directions without scanning the full `loops` vec.
The recorder appends to `parent.children_loop_ids` when a loop's `AgentEnd`
arrives and its `parent_loop_id` is in the same session.

**Rejected alternative:** Single-direction links + O(N) scan. Rejected because
deep continuation trees (10+ loops) would incur O(N²) cost for common tree
operations.

---

### 5. `continuation_kind` classifies loop origin

**Decision:** Reuse the existing `ContinuationKind` enum (`Initial`, `Default`, `Rerun`,
`Branch`, `Compaction`) to classify loop relationships, supplemented by the
`parent_loop_id`/`session_id` cross-session check.

**Why:** `ContinuationKind` is already threaded through `AgentStart` — no new
enum is needed. The full classification table (origin / continuation / retry /
branch / sub-agent) is derivable from `(parent_loop_id, session_id, continuation_kind)`.

**Rejected alternative:** A dedicated `LoopOrigin` enum on `LoopRecord`. Rejected
because it would duplicate information already present in the existing fields and
require an additional mapping step in the recorder.

---

### 6. Sub-agents are separate sessions with bidirectional cross-session links

**Decision:** Sub-agents always get their own `session_id`. The parent records
`ChildLoopRef` (outbound); the child `Session` records `SpawnRef` (inbound).

**Why:** Clean agent identity boundaries — sub-agent sessions can be loaded
and analyzed independently. The bidirectional link pair provides a complete
spawn graph without coupling the parent and child session records.

**Rejected alternative:** Embed sub-agent loops inside the parent session.
Rejected because a sub-agent may have many of its own continuations, parallel
branches, and even nested sub-agents — treating it as a flat loop inside the
parent session would obscure this structure.

---

### 7. `SpawnRef` on Session (not on LoopRecord)

**Decision:** The inbound cross-session spawn reference lives on `Session.parent_spawn_ref`,
not on an individual `LoopRecord`.

**Why:** Sub-agent spawning is a session-level concern. The entire child session
was triggered by one parent loop — the reference belongs at the session level,
not on individual loop records within it. Placing it on a `LoopRecord` would
require choosing *which* loop gets the ref (the first? the origin?) arbitrarily.

**Rejected alternative:** `LoopRecord.parent_spawn_ref`. Rejected because a
sub-agent session may have multiple origin loops (e.g. after `new_session()`)
and the spawn ref would be duplicated or placed inconsistently.

---

### 8. `include_streaming_events: bool` (default false)

**Decision:** `MessageUpdate` (streaming delta) events are excluded from
`LoopRecord.events` by default.

**Why:** Streaming deltas are 100–1 000× more numerous than final messages and
are not needed for replay or branching. The final message content in `AgentEnd.messages`
is authoritative. Opt-in ensures that session files stay compact by default.

**Rejected alternative:** Always store all events. Rejected because a single
session with a few extended-thinking turns could easily produce megabytes of
delta events.

---

### 9. Flat file layout: `{dir}/{session_id}.json`

**Decision:** One JSON file per session. No index file, no sub-directories.

**Why:** Simplest observable format — files can be inspected directly with
any JSON tool. `list_session_ids` is a directory listing. No index to maintain
or synchronize.

**Rejected alternative:** Indexed layout (e.g. `sessions/index.json` + `sessions/{id}.json`).
Rejected because the index requires atomic updates (write to two files) and can
fall out of sync. An indexed layout can be added in a future iteration when
query patterns (filtering, pagination) are clearer.
