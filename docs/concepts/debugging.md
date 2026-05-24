<!-- Last verified: 2026-05-24 by Claude Code (phi-core 0.9.0 debug capture + async-trait migration) -->
# Debugging an Agent Loop

phi-core ships three layered surfaces for observing what an agent loop is doing
in flight, and a fourth (new in 0.9.0) for reconstructing the *exact* request
payload the model received.

| Surface | Granularity | Persistence | Cost |
|---|---|---|---|
| `tracing` integration | log records (text spans) | depends on subscriber | low |
| `AgentEvent` stream | structured per-event channel | none by default (consumer-driven) | low |
| `SessionRecorder` | materialized `Session` / `LoopRecord` / `Turn` tree | JSON ledger on disk | medium |
| `AgentEvent::TurnRequest` + `Turn::request_payload` (0.9.0) | per-turn assembled wire payload | opt-in via `capture_turn_requests` | medium-high (large JSON) |

The first three were already part of phi-core; the fourth is a 0.9.0 addition
that closes the gap where a developer could see *what* the agent did
(events, messages, tool results) but not *exactly what bytes the provider
received* on each turn.

---

## Existing surfaces (pre-0.9.0)

### `tracing` integration

phi-core uses the `tracing` crate for span-level instrumentation across the
loop. Initialize any compatible subscriber (e.g. `tracing_subscriber::fmt`) in
the consumer; phi-core's internal `tracing::warn!` / `tracing::debug!` calls
flow through automatically. Useful for narrative-style debugging where
correlating a specific log line to source is more important than structured
event capture.

### `AgentEvent` stream

Every `agent_loop()` and `agent_loop_continue()` call accepts a
`tokio::sync::mpsc::UnboundedSender<AgentEvent>`. The agent emits a strict
ordered sequence of events: `AgentStart` → (`TurnStart` →
`TurnRequest` (0.9.0) → [`MessageStart` / `MessageUpdate*` / `MessageEnd`] →
[per tool call: `ToolExecutionStart` → `ToolExecutionUpdate*` →
`ToolExecutionEnd`] → `TurnEnd`)* → `AgentEnd`. The full enum lives in
`phi_core::types::AgentEvent` and is `#[non_exhaustive]` since 0.8.0
(downstream wildcard arms keep compiling across additions).

### `SessionRecorder`

A consumer-side feeder that materializes the `AgentEvent` stream into a
nested `Session` → `LoopRecord` → `Turn` tree, persistable as JSON. See
[`docs/concepts/sessions.md`](sessions.md) and
[`docs/concepts/persistence.md`](persistence.md) for the full data model.
`Turn::input_messages` carries non-assistant messages observed during the
turn; `Turn::output_message` is the assistant's response; `Turn::tool_results`
the executed tool results.

---

## NEW at 0.9.0: per-turn request capture

The pre-0.9.0 `Turn` shape recorded *messages observed during the turn* but
**not** the fully-assembled `StreamConfig` payload sent to the provider —
the post-`convert_to_llm()` `Vec<Message>` array, the system prompt as
shipped, tool definitions, and per-block provenance. 0.9.0 adds:

1. **`AgentEvent::TurnRequest`** — fires exactly once per turn, before the
   retry loop's first `provider.stream()` call. Payload is identical
   across retries.
2. **`AnnotatedRequestPayload`** — a serializable record bundling the system
   prompt, message vec, tool definitions, model identity (`model_id`,
   `thinking_level`, `max_tokens`, `temperature`, `response_format`), and
   a parallel-indexed `Vec<BlockProvenance>`.
3. **`BlockProvenance`** — tags the origin of each message block:
   `SystemPrompt`, `IdentityBlock { name, order }`,
   `MemoryTier { tier, record_id }`,
   `LoopTurn { turn_index, role, message_index }`, `Steering`, `FollowUp`,
   `Unknown`.
4. **`SessionRecorderConfig::capture_turn_requests`** — opt-in flag (default
   `false`); when `true`, the recorder stamps each `Turn::request_payload`
   with the captured `AnnotatedRequestPayload`.

### Reconstructing the exact API request

```rust
use phi_core::session::{SessionRecorder, SessionRecorderConfig};

// 1. Enable capture at the recorder.
let mut recorder = SessionRecorder::new(SessionRecorderConfig {
    capture_turn_requests: true,
    ..Default::default()
});

// 2. Feed events. The recorder attaches AnnotatedRequestPayload to each
//    materialized Turn when this flag is true.

// 3. After flush, walk to a specific turn and reconstruct.
for session in recorder.sessions() {
    for loop_record in &session.loops {
        for turn in &loop_record.turns {
            if let Some(ref payload) = turn.request_payload {
                println!("=== Turn {} of loop {} ===", turn.turn_id.turn_index, loop_record.loop_id);
                println!("System prompt ({} bytes):\n{}", payload.system_prompt.len(), payload.system_prompt);
                for (i, (msg, provenance)) in payload.messages.iter().zip(payload.provenance.iter()).enumerate() {
                    println!("[{i}] {:?} :: {:?}", provenance, msg);
                }
            }
        }
    }
}
```

The `system_prompt` + `messages` together are byte-equivalent to the
`StreamConfig` the provider received (verified by
`tests/turn_request_capture_test.rs::turn_request_payload_matches_provider_input`).

### Provenance derivation rules

The parallel `provenance` vec is built in
`agent_loop::streaming::derive_provenance()` per these rules, in order:

1. **`LlmMessage.provenance_hint.is_some()`** — use the stamped hint
   verbatim. This is how identity loaders and memory stores label their
   blocks (see [Consumer stamping](#consumer-stamping) below).
2. **`turn_id.is_some()` + role-based derivation** — fall back to
   `BlockProvenance::LoopTurn { turn_index, role, message_index }`:
   - `Message::User { .. }` → role `UserMessage`
   - `Message::Assistant` with no `Content::ToolCall` → role `AssistantResponse`
   - `Message::Assistant` with any `Content::ToolCall` → role `ToolCallRequest`
   - `Message::ToolResult { .. }` → role `ToolCallResult`
   - `message_index` ordinals are 0-based within their `turn_index`.
3. **`turn_id.is_none() + Message::User`** — the first such message is
   `Steering`; subsequent are `FollowUp`. This disambiguates initial
   loop entry from later context injection.
4. **Anything else** — `Unknown`.

`IdentityBlock` and `MemoryTier` never appear via derivation — they require
consumer stamping (see below). A common signal that a consumer forgot to
stamp is identity / memory blocks appearing as `Unknown` or `Steering`.

### Consumer stamping

Upstream consumers (identity loaders, memory stores) should stamp
`LlmMessage.provenance_hint` before emitting messages into the agent loop
so the provenance vec carries semantic origins rather than fall-back tags:

```rust
use phi_core::{BlockProvenance, LlmMessage, Message};

// Identity loader (per layer):
let lm = LlmMessage::new(Message::user(layer.body_text()))
    .with_provenance_hint(BlockProvenance::IdentityBlock {
        name: layer.stem.clone(),
        order: layer.priority,
    });

// Memory store (per record):
let lm = LlmMessage::new(Message::user(record.text()))
    .with_provenance_hint(BlockProvenance::MemoryTier {
        tier: "short-term".into(),
        record_id: record.id().to_string(),
    });
```

The `with_provenance_hint(...)` consuming builder is the canonical entry
point. Hints survive serialization via the existing `LlmMessage` custom
serde (key `provenanceHint`, omitted when `None`).

### Trade-offs

- **Payload size.** A single `TurnRequest` payload includes the full system
  prompt + the entire `Vec<Message>` sent to the provider, which can be
  hundreds of KB per turn for long-running agents. Across many turns,
  recorded session JSON can grow into tens of MB. `capture_turn_requests: false`
  (default) suppresses persistence — the event still fires, but the
  recorder ignores it.
- **Opt-in flag.** This is opposite to `include_streaming_events` (which is
  also default-off) but mirrors its shape. Enable on a per-session basis
  for debug runs; leave off for production unless the disk budget allows.
- **Custom `convert_to_llm`.** The parallel-index guarantee
  (`payload.messages.len() == payload.provenance.len()`) holds when
  `convert_to_llm = None` (default behaviour). A custom converter that
  collapses or reorders messages can break the alignment — consumers
  relying on byte-exact alignment should leave `convert_to_llm` unset.

---

## Cross-references

- [`docs/concepts/messages-events.md`](messages-events.md) — full
  `AgentEvent` taxonomy and ordering invariants.
- [`docs/concepts/sessions.md`](sessions.md) — `Session` / `LoopRecord` /
  `Turn` shape.
- [`docs/concepts/persistence.md`](persistence.md) — saving / loading
  session JSON.
- [`docs/concepts/compaction.md`](compaction.md) — how compaction blocks
  interact with the prior loop's recorded turns (and the new async-trait
  shape for custom `BlockCompactionStrategy` impls).
- [`docs/concepts/agent-loop.md`](agent-loop.md) — lifecycle hooks and
  their async-from-0.9.0 signatures.
- `CHANGELOG.md` § `[0.9.0] — 2026-05-24` — the canonical "added at" cite
  for `AgentEvent::TurnRequest`, `BlockProvenance`,
  `AnnotatedRequestPayload`, and `capture_turn_requests`.
