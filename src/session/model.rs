use crate::provider::ModelConfig;
use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// SessionFormation
// ---------------------------------------------------------------------------

/// How this [`Session`] was initially created.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionFormation {
    /// Created by direct construction — e.g. when the caller manually builds a
    /// [`Session`] value (e.g. in tests or tooling).
    ///
    /// [`SessionRecorder`] never sets this variant; it always writes
    /// [`FirstLoop`][Self::FirstLoop] when it opens a session.
    Explicit { timestamp: DateTime<Utc> },

    /// Created automatically when a new `session_id` first appeared in an `AgentStart`
    /// event (the recorder saw the session_id for the first time).
    FirstLoop { timestamp: DateTime<Utc> },

    /// A new session was opened because the agent had been idle longer than `threshold_secs`.
    ///
    /// Requires the caller to have rotated the `session_id` beforehand — for example
    /// via [`BasicAgent::check_and_rotate`]. The recorder detects the new `session_id`
    /// when the next `AgentStart` arrives.
    InactivityTimeout {
        /// Idle threshold that triggered the new session.
        threshold_secs: u64,
        /// The `session_id` of the session that preceded this one (if known).
        previous_session_id: Option<String>,
        timestamp: DateTime<Utc>,
    },
}

// ---------------------------------------------------------------------------
// LoopStatus
// ---------------------------------------------------------------------------

/// Lifecycle state of a [`LoopRecord`].
///
/// ```text
/// ┌─────────┐  AgentStart  ┌─────────┐  AgentEnd (ok)     ┌───────────┐
/// │ Pending ├─────────────►│ Running ├───────────────────►│ Completed │
/// └─────────┘              └────┬────┘  AgentEnd (reject) └───────────┘
///                               │                          ┌──────────┐
///                               ├─────────────────────────►│ Rejected │
///                               │       flush()            └──────────┘
///                               │                          ┌─────────┐
///                               └─────────────────────────►│ Aborted │
///                                                          └─────────┘
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LoopStatus {
    /// Loop id appeared in `ParallelLoopStart` but `AgentStart` has not yet arrived.
    ///
    /// Only used for parallel-evaluation branches that are pre-registered when
    /// [`AgentEvent::ParallelLoopStart`] is processed, before their individual
    /// `AgentStart` events fire.
    Pending,

    /// `AgentStart` was received; the loop is executing.
    Running,

    /// `AgentEnd` was received and `rejection` is `None`; the loop finished normally.
    Completed,

    /// `AgentEnd` was received with `rejection: Some(_)`; an input filter blocked the run.
    Rejected,

    /// [`SessionRecorder::flush`] was called before `AgentEnd` arrived
    /// (e.g. process shutdown or unclean shutdown of the event channel).
    Aborted,
}

// ---------------------------------------------------------------------------
// LoopConfigSnapshot
// ---------------------------------------------------------------------------

/// A lightweight, serialisable snapshot of the model that ran a loop.
///
/// ## Why not store the full `AgentLoopConfig`?
///
/// `AgentLoopConfig` contains API keys (in `ModelConfig.api_key`) and
/// non-serialisable hook closures (`BeforeTurnFn`, `AfterTurnFn`, etc.).
/// Storing the full config would require stripping secrets and skipping
/// closures, yielding little extra value.
///
/// `LoopConfigSnapshot` captures just enough to:
/// - Identify which model/provider produced the messages (cost attribution,
///   analysis).
/// - Support replay by telling the caller which config to reconstruct.
/// - Distinguish branches in evaluational parallelism (e.g. "haiku vs. opus").
///
/// Populated from the first `Message::Assistant` seen in the loop
/// (`TurnEnd.message` or `AgentEnd.messages`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopConfigSnapshot {
    /// The model id string (e.g. `"claude-opus-4-6"`, `"gpt-4o"`).
    pub model: String,
    /// Provider name (e.g. `"anthropic"`, `"openai"`).
    pub provider: String,
    /// The stable config identity from `AgentLoopConfig.config_id` (if set).
    ///
    /// Matches the `config_segment` component embedded in the `loop_id` format
    /// `{session_id}.{config_segment}.{N}`. Useful to correlate a `LoopRecord`
    /// back to its named configuration.
    pub config_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Cross-session sub-agent references
// ---------------------------------------------------------------------------

/// Outbound cross-session link — recorded on the **parent** [`LoopRecord`] when
/// a tool call in that loop spawned a sub-agent loop.
///
/// Sub-agents run with their own `session_id`. This ref allows the parent session
/// to link outward to the child session for tracing agent-spawning chains.
///
/// The inverse link is [`SpawnRef`] on [`Session::parent_spawn_ref`]
/// (child → parent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChildLoopRef {
    /// The `ToolCall.id` that triggered sub-agent execution.
    pub tool_call_id: String,
    /// The tool name that performed the spawn.
    pub tool_name: String,
    /// The sub-agent's `AgentStart.loop_id`.
    pub child_loop_id: String,
    /// The sub-agent's `AgentStart.session_id`.
    ///
    /// Extracted from the `child_loop_id` prefix — loop ids follow the format
    /// `{session_id}.{config_segment}.{N}` where `session_id` is a UUID
    /// containing hyphens but no dots.
    pub child_session_id: String,
}

/// Inbound cross-session link — recorded on the **child** [`Session`] when the
/// session was spawned by a tool call in a different (parent) session.
///
/// Together with [`ChildLoopRef`] in the parent session this forms a complete
/// bidirectional cross-session spawn graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpawnRef {
    /// The parent session's `session_id`.
    pub parent_session_id: String,
    /// The parent loop's `loop_id` (the loop whose tool call triggered this spawn).
    pub parent_loop_id: String,
    /// The `ToolCall.id` in the parent loop.
    pub tool_call_id: String,
    /// The tool name in the parent loop.
    pub tool_name: String,
}

// ---------------------------------------------------------------------------
// ParallelGroupRecord
// ---------------------------------------------------------------------------

/// Links a [`LoopRecord`] to its evaluational-parallelism group.
///
/// All branches in the same `agent_loop_parallel` call share identical
/// `all_loop_ids` / `selected_loop_id` values — only `is_selected` differs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParallelGroupRecord {
    /// All branch `loop_id`s in config order (matches `ParallelLoopStart.loop_ids`).
    pub all_loop_ids: Vec<String>,
    /// The `loop_id` selected as winner by the evaluation strategy.
    pub selected_loop_id: String,
    /// 0-based index into the original `configs` slice of the winning branch.
    pub selected_config_index: usize,
    /// Token usage incurred by the judge LLM (zero for non-judge strategies).
    pub evaluation_usage: Usage,
    /// `true` if this [`LoopRecord`] is the evaluation winner.
    pub is_selected: bool,
}

// ---------------------------------------------------------------------------
// LoopEvent
// ---------------------------------------------------------------------------

/// One event in a [`LoopRecord`]'s ordered event stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopEvent {
    /// Monotonic counter within this loop (0-based). Gaps indicate filtered events
    /// (e.g. `MessageUpdate` streaming deltas when
    /// `SessionRecorderConfig::include_streaming_events` is `false`).
    pub sequence: u64,
    /// The original event. `event.loop_id()` matches the [`LoopRecord::loop_id`].
    pub event: AgentEvent,
}

// ---------------------------------------------------------------------------
// Turn
// ---------------------------------------------------------------------------

/// A materialized record of one LLM turn within a loop.
///
/// Each turn represents one LLM call-response cycle plus any tool executions
/// that followed. Built by [`SessionRecorder`] from `TurnStart`/`TurnEnd`
/// event pairs.
///
/// ## Message partitioning
///
/// - `input_messages` — user prompts, steering messages, and follow-ups injected
///   at the start of this turn (between `TurnStart` and the assistant response).
/// - `output_message` — the assistant's streamed response (from `TurnEnd.message`).
/// - `tool_results` — tool result messages executed this turn (from `TurnEnd.tool_results`).
///   Empty when no tool calls were made (`StopReason::Stop`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// Identifies this turn: `loop_id` + `turn_index`.
    pub turn_id: TurnId,

    /// What caused this turn to begin.
    pub triggered_by: TurnTrigger,

    /// Per-turn token usage (from `TurnEnd.usage`).
    pub usage: Usage,

    /// Messages injected at the start of this turn (user prompts, steering
    /// messages, follow-ups). Empty for continuation turns that only have
    /// tool results from the prior turn feeding back in.
    pub input_messages: Vec<AgentMessage>,

    /// The assistant message produced by the LLM this turn.
    pub output_message: AgentMessage,

    /// Tool result messages from this turn. Empty when no tool calls were made.
    pub tool_results: Vec<AgentMessage>,

    /// Wall-clock time when this turn began (from `TurnStart.timestamp`).
    pub started_at: DateTime<Utc>,

    /// Wall-clock time when this turn completed (from `TurnEnd.timestamp`).
    pub ended_at: DateTime<Utc>,
}

impl Turn {
    /// The zero-based turn index within its loop.
    pub fn index(&self) -> u32 {
        self.turn_id.turn_index
    }

    /// Duration of this turn.
    pub fn duration(&self) -> chrono::Duration {
        self.ended_at - self.started_at
    }

    /// Whether this turn included tool calls.
    pub fn has_tool_calls(&self) -> bool {
        !self.tool_results.is_empty()
    }

    /// All messages in this turn in chronological order:
    /// input_messages, then output_message, then tool_results.
    pub fn all_messages(&self) -> Vec<&AgentMessage> {
        let mut msgs: Vec<&AgentMessage> = self.input_messages.iter().collect();
        msgs.push(&self.output_message);
        msgs.extend(self.tool_results.iter());
        msgs
    }
}

// ---------------------------------------------------------------------------
// LoopRecord
// ---------------------------------------------------------------------------

/// A complete record of one agent-loop execution.
///
/// ## Loop origin classification
///
/// | `parent_loop_id` | `continuation_kind` | Meaning |
/// |---|---|---|
/// | `None` | `None` | Fresh origin loop (`agent_loop`) |
/// | `Some(p)`, same session | `Some(Default)` | Regular continuation |
/// | `Some(p)`, same session | `Some(Rerun)` | Retry / error recovery |
/// | `Some(p)`, same session | `Some(Branch)` | Branch exploration |
/// | `Some(p)`, different session | `None` | Sub-agent loop (spawned by a tool) |
///
/// ## Tree navigation
///
/// - Parent → children: iterate [`children_loop_ids`][Self::children_loop_ids]
/// - Child → parent: read [`parent_loop_id`][Self::parent_loop_id]
/// - Sub-agent children (cross-session): iterate [`child_loop_refs`][Self::child_loop_refs]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoopRecord {
    // ── Identity ────────────────────────────────────────────────────────────
    /// Unique identifier for this loop execution.
    pub loop_id: String,
    /// Session this loop belongs to.
    pub session_id: String,
    /// Agent that ran this loop.
    pub agent_id: String,

    // ── Loop origin classification ────────────────────────────────────────
    /// `loop_id` of the loop that directly preceded this one (if any).
    ///
    /// - `None` for origin loops (started by `agent_loop`).
    /// - `Some(id)` for continuations started by `agent_loop_continue`.
    /// - For sub-agent loops, `parent_loop_id` refers to the tool call loop
    ///   in a **different** session.
    pub parent_loop_id: Option<String>,

    /// How this loop relates to its parent.
    ///
    /// - `None` if this is an origin loop or a sub-agent loop.
    /// - `Some(Default)` for regular same-session continuations.
    /// - `Some(Rerun)` for retries / error recovery.
    /// - `Some(Branch {..})` for branch explorations.
    pub continuation_kind: Option<ContinuationKind>,

    // ── Timing ────────────────────────────────────────────────────────────
    /// Timestamp from `AgentStart`.
    pub started_at: DateTime<Utc>,
    /// Timestamp from `AgentEnd` (`None` while running or pending).
    pub ended_at: Option<DateTime<Utc>>,

    // ── Status ────────────────────────────────────────────────────────────
    pub status: LoopStatus,
    /// Set when `AgentEnd.rejection` is `Some(_)` (input filter blocked the run).
    pub rejection: Option<String>,

    // ── Model ─────────────────────────────────────────────────────────────
    /// Identifies the model and provider that ran this loop.
    ///
    /// Populated from the first `Message::Assistant` seen in the loop.
    /// `None` if the loop ended before any assistant message was produced.
    pub config: Option<LoopConfigSnapshot>,

    // ── Messages ──────────────────────────────────────────────────────────
    /// All new messages produced by this loop — taken directly from `AgentEnd.messages`.
    ///
    /// These are the authoritative messages for replay and branching. To resume
    /// from a loop, reconstruct an `AgentContext` with the full message history
    /// (prior loop messages + these) and call `agent_loop_continue`.
    pub messages: Vec<AgentMessage>,

    // ── Turns ────────────────────────────────────────────────────────────
    /// Materialized turn records, one per LLM call-response cycle.
    ///
    /// Built by [`SessionRecorder`] from `TurnStart`/`TurnEnd` event pairs.
    /// Empty for old sessions that predate turn materialization, or for loops
    /// that ended before any turn completed (rejected, aborted).
    #[serde(default)]
    pub turns: Vec<Turn>,

    // ── Usage ─────────────────────────────────────────────────────────────
    /// Token usage from `AgentEnd.usage`.
    pub usage: Usage,

    // ── Caller context ────────────────────────────────────────────────────
    /// Opaque metadata passed to `AgentStart` by the caller (e.g. request id).
    pub metadata: Option<serde_json::Value>,

    // ── Full event stream ─────────────────────────────────────────────────
    /// Ordered event stream for this loop.
    ///
    /// `MessageUpdate` (streaming delta) events are included only when
    /// [`SessionRecorderConfig::include_streaming_events`] is `true`.
    pub events: Vec<LoopEvent>,

    // ── Same-session tree ─────────────────────────────────────────────────
    /// `loop_id`s of same-session child loops (continuations / reruns / branches).
    ///
    /// This is the parent→children direction of the bidirectional loop tree.
    /// The inverse (`children → parent`) is [`parent_loop_id`][Self::parent_loop_id].
    ///
    /// Does **not** include cross-session sub-agent children — those are in
    /// [`child_loop_refs`][Self::child_loop_refs].
    pub children_loop_ids: Vec<String>,

    /// Cross-session links to sub-agent loops spawned by tool calls in this loop.
    ///
    /// Each entry corresponds to a `ToolExecutionEnd.child_loop_id` that is
    /// `Some(_)`. Use the `child_session_id` to load the child [`Session`].
    pub child_loop_refs: Vec<ChildLoopRef>,

    // ── Parallel evaluation ───────────────────────────────────────────────
    /// Set when this loop was part of an evaluational-parallelism group.
    pub parallel_group: Option<ParallelGroupRecord>,

    // ── Compaction ──────────────────────────────────────────────────────
    /// Non-destructive compaction overlay. When `Some`, the context loader
    /// uses this block instead of raw `self.messages`. The original messages
    /// remain untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compaction_block: Option<crate::context::CompactionBlock>,
}

impl LoopRecord {
    /// Get a turn by its index. Returns `None` if turns are not materialized
    /// or the index is out of range.
    pub fn get_turn(&self, turn_index: u32) -> Option<&Turn> {
        self.turns.get(turn_index as usize)
    }

    /// Number of materialized turns. Returns 0 if turns are not materialized.
    pub fn turn_count(&self) -> usize {
        self.turns.len()
    }
}

// ---------------------------------------------------------------------------
// SessionScope
// ---------------------------------------------------------------------------

/// Whether session data is kept in memory only or persisted to disk.
///
/// - `Ephemeral` (default): session exists only in memory for the process lifetime.
/// - `Persistent`: session data is written to a store and survives restarts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SessionScope {
    #[default]
    Ephemeral,
    Persistent,
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// A named container grouping all [`LoopRecord`]s for one agent session.
///
/// ## Loop tree structure
///
/// The tree is implicit via `parent_loop_id` / `children_loop_ids` links:
///
/// - **Root loops** — `parent_loop_id` is `None` (or points to a loop in a
///   different session for sub-agent roots).
/// - **Continuation chains** — `parent_loop_id` → `loop_id` within the same
///   session.
/// - **Parallel branches** — siblings sharing the same `parent_loop_id`, each
///   with `parallel_group` set.
/// - **Sub-agent children** — in `child_loop_refs` on the parent loop
///   (cross-session, not in `loops` vec).
///
/// ## Cross-session sub-agent tracking
///
/// When this session was itself spawned as a sub-agent, [`parent_spawn_ref`]
/// points back to the parent session and loop that triggered it. This is the
/// inverse of [`LoopRecord::child_loop_refs`] in the parent session, and together
/// they form a complete bidirectional cross-session spawn graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    /// Stable identifier for this session — matches `AgentStart.session_id`.
    pub session_id: String,
    /// The `agent_id` from the first `AgentStart` seen for this session.
    pub agent_id: String,
    /// Timestamp of the first `AgentStart` event seen for this session.
    pub created_at: DateTime<Utc>,
    /// Timestamp of the most recent `AgentStart` event seen for this session.
    ///
    /// Updated each time a new loop opens (on `AgentStart`), so it reflects
    /// when the last loop _started_, not when it last had activity.
    pub last_active_at: DateTime<Utc>,
    /// Why this session was created.
    pub formation: SessionFormation,

    /// Set when this session was spawned as a sub-agent by a loop in a different
    /// session. Populated by [`SessionRecorder`] when a new session's first
    /// `AgentStart` carries a `parent_loop_id` that belongs to a different
    /// `session_id`.
    pub parent_spawn_ref: Option<SpawnRef>,

    // ── Session-level config overrides (G4, G7, G9) ────────────────────────
    /// Session-level model config override (G4).
    /// When set, this model is used for all loops in this session instead of the
    /// agent's default.
    #[serde(default)]
    pub model_config: Option<ModelConfig>,

    /// Session-level thinking level override (G9).
    /// Takes precedence over the agent profile's thinking_level.
    #[serde(default)]
    pub thinking_level: Option<ThinkingLevel>,

    /// Session-level temperature override (G9).
    /// Takes precedence over the agent profile's temperature.
    #[serde(default)]
    pub temperature: Option<f32>,

    /// Session scope — ephemeral (in-memory only) or persistent (written to store) (G7).
    #[serde(default)]
    pub scope: SessionScope,

    /// All completed and in-progress [`LoopRecord`]s, ordered by [`LoopRecord::started_at`].
    pub loops: Vec<LoopRecord>,
}

impl Session {
    /// Return root loops — those whose `parent_loop_id` is `None` or whose parent
    /// belongs to a different session.
    pub fn root_loops(&self) -> impl Iterator<Item = &LoopRecord> {
        let loop_ids: std::collections::HashSet<&str> =
            self.loops.iter().map(|l| l.loop_id.as_str()).collect();
        self.loops.iter().filter(move |l| {
            l.parent_loop_id
                .as_deref()
                .map(|pid| !loop_ids.contains(pid))
                .unwrap_or(true)
        })
    }

    /// Return all direct same-session children of `loop_id`.
    pub fn children_of<'a>(&'a self, loop_id: &str) -> impl Iterator<Item = &'a LoopRecord> {
        let record = self.loops.iter().find(|l| l.loop_id == loop_id);
        let ids: Vec<&str> = record
            .map(|r| r.children_loop_ids.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default();
        self.loops
            .iter()
            .filter(move |l| ids.contains(&l.loop_id.as_str()))
    }

    /// Return all loops in the same parallel group as `loop_id`.
    pub fn parallel_siblings<'a>(&'a self, loop_id: &str) -> impl Iterator<Item = &'a LoopRecord> {
        let all_ids: Option<Vec<String>> = self
            .loops
            .iter()
            .find(|l| l.loop_id == loop_id)
            .and_then(|l| l.parallel_group.as_ref())
            .map(|pg| pg.all_loop_ids.clone());

        self.loops.iter().filter(move |l| {
            all_ids
                .as_ref()
                .map(|ids| ids.contains(&l.loop_id))
                .unwrap_or(false)
        })
    }

    /// Look up a loop by its `loop_id`.
    pub fn get_loop(&self, loop_id: &str) -> Option<&LoopRecord> {
        self.loops.iter().find(|l| l.loop_id == loop_id)
    }

    /// Mutable look up a loop by its `loop_id`.
    pub fn get_loop_mut(&mut self, loop_id: &str) -> Option<&mut LoopRecord> {
        self.loops.iter_mut().find(|l| l.loop_id == loop_id)
    }

    /// Build the linear chain of loops from root to `target_loop_id`
    /// by walking `parent_loop_id` links backward. Returns loop IDs
    /// in chronological order (root first).
    ///
    /// This naturally handles parallel branches (only the selected path)
    /// and reruns (only the active ancestor chain).
    pub fn loop_chain_to(&self, target_loop_id: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut current = target_loop_id.to_string();
        loop {
            chain.push(current.clone());
            match self
                .get_loop(&current)
                .and_then(|r| r.parent_loop_id.as_ref())
            {
                Some(parent) => current = parent.clone(),
                None => break,
            }
        }
        chain.reverse();
        chain
    }

    /// Cumulative token usage across all loops in this session.
    pub fn total_usage(&self) -> Usage {
        self.loops.iter().fold(Usage::default(), |mut acc, l| {
            acc.input += l.usage.input;
            acc.output += l.usage.output;
            acc.reasoning += l.usage.reasoning;
            acc.cache_read += l.usage.cache_read;
            acc.cache_write += l.usage.cache_write;
            acc.total_tokens += l.usage.total_tokens;
            acc
        })
    }
}

// ---------------------------------------------------------------------------
// SessionError
// ---------------------------------------------------------------------------

/// Errors from session I/O.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Session not found: {session_id}")]
    NotFound { session_id: String },
}

// ---------------------------------------------------------------------------
// OpenLoop
// ---------------------------------------------------------------------------

/// An open (in-progress) loop record stored inside the recorder.
pub(crate) struct OpenLoop {
    pub(crate) record: LoopRecord,
    /// Monotonic event counter for this loop.
    pub(crate) next_seq: u64,
}
