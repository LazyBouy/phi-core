//! Persistent session layer for phi-core agents.
//!
//! A [`Session`] is a named container (keyed by `session_id`) that groups all
//! [`LoopRecord`]s belonging to one agent session. Loops within a session form a
//! tree via [`LoopRecord::parent_loop_id`] / [`LoopRecord::children_loop_ids`]
//! links; parallel-evaluation branches form sibling groups via
//! [`ParallelGroupRecord`].
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use phi_core::session::{SessionRecorder, SessionRecorderConfig, save_session};
//! use phi_core::AgentEvent;
//! use std::path::Path;
//!
//! let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
//!
//! // Feed every event from your agent into the recorder:
//! // recorder.on_event(event);
//!
//! // When done, flush open loops and persist:
//! recorder.flush();
//! for session in recorder.drain_completed() {
//!     save_session(&session, Path::new("./sessions")).ok();
//! }
//! ```

use crate::types::*;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
// SessionRecorderConfig
// ---------------------------------------------------------------------------

/// Configuration for [`SessionRecorder`].
#[derive(Debug, Clone, Default)]
pub struct SessionRecorderConfig {
    /// Store `MessageUpdate` (streaming delta) events in [`LoopRecord::events`].
    ///
    /// Default: `false`. Streaming deltas are 100–1 000× more numerous than
    /// final messages and are not needed for replay or branching. Enable only
    /// for debugging or playback use cases.
    pub include_streaming_events: bool,
}

// ---------------------------------------------------------------------------
// SessionRecorder internals
// ---------------------------------------------------------------------------

/// Partial state for a parallel-evaluation group, accumulated as `ParallelLoopStart`
/// arrives before `ParallelLoopEnd`.
struct PartialParallelGroup {
    all_loop_ids: Vec<String>,
}

/// An open (in-progress) loop record stored inside the recorder.
struct OpenLoop {
    record: LoopRecord,
    /// Monotonic event counter for this loop.
    next_seq: u64,
}

// ---------------------------------------------------------------------------
// SessionRecorder
// ---------------------------------------------------------------------------

/// Records every [`AgentEvent`] into a structured tree of [`Session`]s and
/// [`LoopRecord`]s.
///
/// Call [`on_event`][Self::on_event] for every event emitted on the agent's
/// `tx` channel, then [`flush`][Self::flush] before shutdown or saving.
///
/// ## Session grouping
///
/// Sessions are keyed by `session_id`. Every `AgentStart` event that carries a
/// `session_id` the recorder has not seen before opens a new [`Session`]; all
/// subsequent loops with the same `session_id` are appended to that session.
///
/// **The recorder never rotates sessions on its own.** If you want a new session
/// to start after a period of inactivity, call
/// [`BasicAgent::check_and_rotate`][crate::BasicAgent::check_and_rotate] (or
/// [`BasicAgent::new_session`][crate::BasicAgent::new_session]) before the next
/// prompt. The next `AgentStart` will carry the new `session_id` and the recorder
/// will open a fresh [`Session`] automatically, with
/// [`SessionFormation::InactivityTimeout`] or [`SessionFormation::FirstLoop`]
/// as the recorded reason.
///
/// ## Example
///
/// ```rust,no_run
/// use phi_core::session::{SessionRecorder, SessionRecorderConfig};
/// use phi_core::AgentEvent;
///
/// let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
/// // Feed events as they arrive:
/// // recorder.on_event(event);
/// recorder.flush();
/// ```
pub struct SessionRecorder {
    config: SessionRecorderConfig,

    /// Completed sessions (all their loops are closed).
    completed: Vec<Session>,

    /// Sessions that still have open loops.
    open_sessions: HashMap<String, Session>,

    /// Loops currently executing (between AgentStart and AgentEnd).
    open_loops: HashMap<String, OpenLoop>,

    /// Parallel groups announced by ParallelLoopStart but not yet closed.
    partial_groups: HashMap<String, PartialParallelGroup>,
}

impl SessionRecorder {
    /// Create a new recorder with the given configuration.
    pub fn new(config: SessionRecorderConfig) -> Self {
        SessionRecorder {
            config,
            completed: Vec::new(),
            open_sessions: HashMap::new(),
            open_loops: HashMap::new(),
            partial_groups: HashMap::new(),
        }
    }

    /// Feed one event into the recorder.
    ///
    /// Must be called for every event emitted on the agent's `tx` channel.
    pub fn on_event(&mut self, event: AgentEvent) {
        match &event {
            // ── ParallelLoopStart ─────────────────────────────────────────
            AgentEvent::ParallelLoopStart { loop_ids, .. } => {
                for lid in loop_ids {
                    // Pre-register a Pending record; will be promoted to Running when AgentStart arrives.
                    let group_key = lid.clone();
                    self.partial_groups
                        .entry(group_key)
                        .or_insert_with(|| PartialParallelGroup {
                            all_loop_ids: loop_ids.clone(),
                        });
                    // We don't have agent_id / session_id yet — those arrive in AgentStart.
                }
            }

            // ── AgentStart ────────────────────────────────────────────────
            AgentEvent::AgentStart {
                agent_id,
                session_id,
                loop_id,
                parent_loop_id,
                continuation_kind,
                timestamp,
                metadata,
            } => {
                // Ensure the session exists.
                let now = *timestamp;
                let session = self
                    .open_sessions
                    .entry(session_id.clone())
                    .or_insert_with(|| Session {
                        session_id: session_id.clone(),
                        agent_id: agent_id.clone(),
                        created_at: now,
                        last_active_at: now,
                        formation: SessionFormation::FirstLoop { timestamp: now },
                        parent_spawn_ref: None,
                        loops: Vec::new(),
                    });
                session.last_active_at = now;

                // If parent_loop_id is set and belongs to a DIFFERENT session, this is a
                // sub-agent spawn — record the inbound SpawnRef on this session.
                if let Some(ref plid) = parent_loop_id {
                    let parent_session_id = session_id_from_loop_id(plid);
                    if parent_session_id != *session_id && session.parent_spawn_ref.is_none() {
                        // We don't have the tool_call_id / tool_name here; those come from the parent's
                        // ToolExecutionEnd. Set what we know; callers can enrich later if needed.
                        session.parent_spawn_ref = Some(SpawnRef {
                            parent_session_id,
                            parent_loop_id: plid.clone(),
                            tool_call_id: String::new(), // enriched when ChildLoopRef is processed
                            tool_name: String::new(),
                        });
                    }
                }

                // Create the LoopRecord (Pending → Running).
                let record = LoopRecord {
                    loop_id: loop_id.clone(),
                    session_id: session_id.clone(),
                    agent_id: agent_id.clone(),
                    parent_loop_id: parent_loop_id.clone(),
                    continuation_kind: continuation_kind.clone(),
                    started_at: now,
                    ended_at: None,
                    status: LoopStatus::Running,
                    rejection: None,
                    config: None,
                    messages: Vec::new(),
                    usage: Usage::default(),
                    metadata: metadata.clone(),
                    events: Vec::new(),
                    children_loop_ids: Vec::new(),
                    child_loop_refs: Vec::new(),
                    parallel_group: None,
                    compaction_block: None,
                };
                let open = OpenLoop {
                    record,
                    next_seq: 0,
                };
                self.open_loops.insert(loop_id.clone(), open);
                // Append AgentStart to event stream.
                self.append_event(loop_id, event.clone());
            }

            // ── AgentEnd ──────────────────────────────────────────────────
            AgentEvent::AgentEnd {
                loop_id,
                messages,
                usage,
                timestamp,
                rejection,
            } => {
                self.append_event(loop_id, event.clone());
                if let Some(mut open) = self.open_loops.remove(loop_id) {
                    open.record.ended_at = Some(*timestamp);
                    open.record.status = if rejection.is_some() {
                        LoopStatus::Rejected
                    } else {
                        LoopStatus::Completed
                    };
                    open.record.rejection = rejection.clone();
                    open.record.messages = messages.clone();
                    open.record.usage = usage.clone();

                    // Extract config snapshot from first assistant message.
                    if open.record.config.is_none() {
                        open.record.config = extract_config_snapshot(messages, loop_id);
                    }

                    let session_id = open.record.session_id.clone();
                    let parent_loop_id = open.record.parent_loop_id.clone();

                    // Link parent → child within same session.
                    if let Some(ref plid) = parent_loop_id {
                        // Check if parent is in the same session.
                        let parent_in_session = self
                            .open_sessions
                            .get(&session_id)
                            .map(|s| s.loops.iter().any(|l| &l.loop_id == plid))
                            .unwrap_or(false);
                        let parent_in_open = self.open_loops.contains_key(plid.as_str());

                        if parent_in_session {
                            if let Some(s) = self.open_sessions.get_mut(&session_id) {
                                if let Some(p) = s.loops.iter_mut().find(|l| &l.loop_id == plid) {
                                    if !p.children_loop_ids.contains(loop_id) {
                                        p.children_loop_ids.push(loop_id.clone());
                                    }
                                }
                            }
                        } else if parent_in_open {
                            if let Some(p) = self.open_loops.get_mut(plid.as_str()) {
                                // Only link same-session children. Cross-session sub-agent
                                // children are tracked via child_loop_refs / SpawnRef.
                                if p.record.session_id == session_id
                                    && !p.record.children_loop_ids.contains(loop_id)
                                {
                                    p.record.children_loop_ids.push(loop_id.clone());
                                }
                            }
                        }
                    }

                    // Move into session.
                    if let Some(session) = self.open_sessions.get_mut(&session_id) {
                        session.loops.push(open.record);
                    }
                }
            }

            // ── TurnEnd — extract config snapshot ─────────────────────────
            AgentEvent::TurnEnd {
                loop_id, message, ..
            } => {
                self.append_event(loop_id, event.clone());
                if let Some(open) = self.open_loops.get_mut(loop_id.as_str()) {
                    if open.record.config.is_none() {
                        open.record.config =
                            extract_config_snapshot(std::slice::from_ref(message), loop_id);
                    }
                }
            }

            // ── ToolExecutionEnd — record child loop ref ──────────────────
            AgentEvent::ToolExecutionEnd {
                loop_id,
                tool_call_id,
                tool_name,
                result,
                // child_loop_id is also a top-level field on ToolExecutionEnd (mirrors
                // result.child_loop_id for ergonomic pattern matching). We read from
                // result.child_loop_id here so the ChildLoopRef is populated from the
                // same authoritative source as ToolResult.
                ..
            } => {
                self.append_event(loop_id, event.clone());
                if let Some(child_lid) = &result.child_loop_id {
                    if let Some(open) = self.open_loops.get_mut(loop_id.as_str()) {
                        let child_session_id = session_id_from_loop_id(child_lid);
                        open.record.child_loop_refs.push(ChildLoopRef {
                            tool_call_id: tool_call_id.clone(),
                            tool_name: tool_name.clone(),
                            child_loop_id: child_lid.clone(),
                            child_session_id: child_session_id.clone(),
                        });

                        // Enrich child session's parent_spawn_ref with the tool details we now know.
                        // The child may still be in open_sessions (common case) or already in
                        // completed (if flush() was called between child AgentEnd and this event).
                        // We check both to avoid a silent enrichment skip.
                        let parent_session_id = open.record.session_id.clone();
                        let parent_lid = loop_id.clone();
                        let tc_id = tool_call_id.clone();
                        let tn = tool_name.clone();
                        let csl = child_session_id.clone();
                        let enrich = move |session: &mut Session| {
                            if let Some(ref mut sr) = session.parent_spawn_ref {
                                if sr.tool_call_id.is_empty() {
                                    sr.parent_session_id = parent_session_id;
                                    sr.parent_loop_id = parent_lid;
                                    sr.tool_call_id = tc_id;
                                    sr.tool_name = tn;
                                }
                            }
                        };
                        if let Some(child_sess) = self.open_sessions.get_mut(&csl) {
                            enrich(child_sess);
                        } else if let Some(child_sess) =
                            self.completed.iter_mut().find(|s| s.session_id == csl)
                        {
                            enrich(child_sess);
                        }
                    }
                }
            }

            // ── ParallelLoopEnd ───────────────────────────────────────────
            AgentEvent::ParallelLoopEnd {
                selected_loop_id,
                selected_config_index,
                evaluation_usage,
                ..
            } => {
                // Recover all_loop_ids from the partial_groups registered at ParallelLoopStart.
                let all_loop_ids = self
                    .partial_groups
                    .get(selected_loop_id.as_str())
                    .map(|pg| pg.all_loop_ids.clone())
                    .unwrap_or_else(|| vec![selected_loop_id.clone()]);
                let group = ParallelGroupRecord {
                    all_loop_ids: all_loop_ids.clone(),
                    selected_loop_id: selected_loop_id.clone(),
                    selected_config_index: *selected_config_index,
                    evaluation_usage: evaluation_usage.clone(),
                    is_selected: false, // will be set per-record below
                };

                // Retroactively set ParallelGroupRecord on all branch LoopRecords.
                for lid in &all_loop_ids {
                    let is_selected = lid == selected_loop_id;
                    let pg = ParallelGroupRecord {
                        is_selected,
                        ..group.clone()
                    };

                    // Check open_loops first (loop may not be closed yet).
                    if let Some(open) = self.open_loops.get_mut(lid.as_str()) {
                        open.record.parallel_group = Some(pg.clone());
                    }

                    // Also retroactively update already-closed loops in sessions.
                    for session in self.open_sessions.values_mut() {
                        if let Some(lr) = session.loops.iter_mut().find(|l| &l.loop_id == lid) {
                            lr.parallel_group = Some(pg.clone());
                        }
                    }
                    for session in self.completed.iter_mut() {
                        if let Some(lr) = session.loops.iter_mut().find(|l| &l.loop_id == lid) {
                            lr.parallel_group = Some(pg.clone());
                        }
                    }
                }

                // Clean up partial group entries.
                for lid in &all_loop_ids {
                    self.partial_groups.remove(lid.as_str());
                }
            }

            // ── MessageUpdate — optional streaming events ─────────────────
            AgentEvent::MessageUpdate { loop_id, .. } => {
                if self.config.include_streaming_events {
                    self.append_event(loop_id, event.clone());
                }
            }

            // ── All other events — append to loop stream ──────────────────
            other => {
                if let Some(lid) = loop_id_of(other) {
                    self.append_event(lid, event.clone());
                }
            }
        }
    }

    /// Finalize all open [`LoopRecord`]s (status → [`LoopStatus::Aborted`]) and
    /// move them into their sessions.
    ///
    /// Call before saving or on process shutdown.
    pub fn flush(&mut self) {
        let loop_ids: Vec<String> = self.open_loops.keys().cloned().collect();
        for lid in loop_ids {
            if let Some(mut open) = self.open_loops.remove(&lid) {
                open.record.status = LoopStatus::Aborted;
                let session_id = open.record.session_id.clone();
                if let Some(session) = self.open_sessions.get_mut(&session_id) {
                    session.loops.push(open.record);
                }
            }
        }
        // Move fully-closed sessions from open_sessions to completed.
        let session_ids: Vec<String> = self.open_sessions.keys().cloned().collect();
        for sid in session_ids {
            // A session is "complete" when all its loops have ended.
            // Since we just flushed all open loops, every session is complete.
            if let Some(session) = self.open_sessions.remove(&sid) {
                self.completed.push(session);
            }
        }
    }

    /// Promote sessions that have no remaining open loops to the completed list,
    /// without aborting any running loops.
    ///
    /// A session is eligible when every loop belonging to it has already received
    /// an [`AgentEnd`][crate::AgentEvent::AgentEnd] event (i.e. it has no entry in
    /// the internal open-loops map). Sessions that still have active loops are left
    /// in place.
    ///
    /// This is intended for **periodic checkpointing** in production: save finished
    /// sessions to disk while leaving in-flight agent runs untouched. In contrast,
    /// [`flush`][Self::flush] first aborts all open loops and then promotes
    /// everything.
    ///
    /// Returns the number of sessions that were promoted.
    pub fn checkpoint(&mut self) -> usize {
        // Collect session_ids that still have open loops.
        let sessions_with_open_loops: Vec<String> = self
            .open_loops
            .values()
            .map(|l| l.record.session_id.clone())
            .collect();
        // Promote sessions whose id is not in that set.
        let promotable: Vec<String> = self
            .open_sessions
            .keys()
            .filter(|sid| !sessions_with_open_loops.contains(sid))
            .cloned()
            .collect();
        let count = promotable.len();
        for sid in promotable {
            if let Some(session) = self.open_sessions.remove(&sid) {
                self.completed.push(session);
            }
        }
        count
    }

    /// Drain all completed sessions out of the recorder (consuming them).
    ///
    /// Useful for periodic checkpointing. Call [`flush`][Self::flush] first
    /// if you want to include in-progress sessions, or [`checkpoint`][Self::checkpoint]
    /// to drain only fully-finished sessions without aborting active loops.
    pub fn drain_completed(&mut self) -> Vec<Session> {
        std::mem::take(&mut self.completed)
    }

    /// All sessions known to this recorder (completed and in-progress).
    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.completed.iter().chain(self.open_sessions.values())
    }

    /// Look up a session by `session_id`.
    pub fn get_session(&self, session_id: &str) -> Option<&Session> {
        self.completed
            .iter()
            .find(|s| s.session_id == session_id)
            .or_else(|| self.open_sessions.get(session_id))
    }

    /// Look up an in-progress [`LoopRecord`] by `loop_id`.
    pub fn current_loop(&self, loop_id: &str) -> Option<&LoopRecord> {
        self.open_loops.get(loop_id).map(|o| &o.record)
    }

    // ── Private helpers ───────────────────────────────────────────────────

    fn append_event(&mut self, loop_id: &str, event: AgentEvent) {
        if let Some(open) = self.open_loops.get_mut(loop_id) {
            let seq = open.next_seq;
            open.next_seq += 1;
            open.record.events.push(LoopEvent {
                sequence: seq,
                event,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Persistence API
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

/// Save a session to `{dir}/{session_id}.json`, creating `dir` if necessary.
///
/// Returns the path the file was written to.
pub fn save_session(session: &Session, dir: &Path) -> Result<PathBuf, SessionError> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", session.session_id));
    let file = std::fs::File::create(&path)?;
    serde_json::to_writer_pretty(file, session)?;
    Ok(path)
}

/// Load a session from `{dir}/{session_id}.json`.
pub fn load_session(session_id: &str, dir: &Path) -> Result<Session, SessionError> {
    let path = dir.join(format!("{}.json", session_id));
    if !path.exists() {
        return Err(SessionError::NotFound {
            session_id: session_id.to_string(),
        });
    }
    let file = std::fs::File::open(path)?;
    let session: Session = serde_json::from_reader(file)?;
    Ok(session)
}

/// List all session IDs in `dir`, sorted by file modification time (newest first).
pub fn list_session_ids(dir: &Path) -> Result<Vec<String>, SessionError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries: Vec<(std::time::SystemTime, String)> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|x| x == "json")
                .unwrap_or(false)
        })
        .filter_map(|e| {
            let stem = e
                .path()
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())?;
            let mtime = e.metadata().ok()?.modified().ok()?;
            Some((mtime, stem))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
    Ok(entries.into_iter().map(|(_, id)| id).collect())
}

/// Load all sessions in `dir` that belong to `agent_id`.
pub fn load_sessions_for_agent(agent_id: &str, dir: &Path) -> Result<Vec<Session>, SessionError> {
    let ids = list_session_ids(dir)?;
    let mut sessions = Vec::new();
    for id in ids {
        match load_session(&id, dir) {
            Ok(s) if s.agent_id == agent_id => sessions.push(s),
            Ok(_) => {}
            Err(SessionError::NotFound { .. }) => {}
            Err(e) => return Err(e),
        }
    }
    Ok(sessions)
}

/// Delete `{dir}/{session_id}.json`.
pub fn delete_session(session_id: &str, dir: &Path) -> Result<(), SessionError> {
    let path = dir.join(format!("{}.json", session_id));
    if !path.exists() {
        return Err(SessionError::NotFound {
            session_id: session_id.to_string(),
        });
    }
    std::fs::remove_file(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract `session_id` from a `loop_id`.
///
/// Loop ids follow the format `{session_id}.{config_segment}.{N}`. `session_id`
/// is a UUID (e.g. `550e8400-e29b-41d4-a716-446655440000`) — it contains hyphens
/// but no dots. The first `.` in the `loop_id` is always the boundary between the
/// UUID and the rest.
fn session_id_from_loop_id(loop_id: &str) -> String {
    match loop_id.find('.') {
        Some(pos) => loop_id[..pos].to_string(),
        None => loop_id.to_string(),
    }
}

/// Return the `loop_id` from events that carry one but are handled by the catch-all arm.
fn loop_id_of(event: &AgentEvent) -> Option<&str> {
    match event {
        AgentEvent::TurnStart { loop_id, .. } => Some(loop_id),
        AgentEvent::MessageStart { loop_id, .. } => Some(loop_id),
        AgentEvent::MessageEnd { loop_id, .. } => Some(loop_id),
        AgentEvent::ToolExecutionStart { loop_id, .. } => Some(loop_id),
        AgentEvent::ToolExecutionUpdate { loop_id, .. } => Some(loop_id),
        AgentEvent::ProgressMessage { loop_id, .. } if !loop_id.is_empty() => Some(loop_id),
        AgentEvent::InputRejected { loop_id, .. } if !loop_id.is_empty() => Some(loop_id),
        AgentEvent::CompactionStarted { loop_id, .. } => Some(loop_id),
        AgentEvent::CompactionEnded { loop_id, .. } => Some(loop_id),
        _ => None,
    }
}

/// Extract the config segment from a `loop_id` of the form
/// `{session_id}.{config_segment}.{N}`.
///
/// Returns `None` if the `loop_id` does not contain at least two `.` separators.
fn config_segment_from_loop_id(loop_id: &str) -> Option<String> {
    let first = loop_id.find('.')?;
    let after = &loop_id[first + 1..];
    let last = after.rfind('.')?;
    Some(after[..last].to_string())
}

/// Extract a [`LoopConfigSnapshot`] from a slice of messages, using the first
/// `Message::Assistant` found.
///
/// `loop_id` is used to populate [`LoopConfigSnapshot::config_id`] by parsing
/// the `config_segment` component of the `{session_id}.{config_segment}.{N}` format.
fn extract_config_snapshot(messages: &[AgentMessage], loop_id: &str) -> Option<LoopConfigSnapshot> {
    messages.iter().find_map(|m| {
        if let AgentMessage::Llm(LlmMessage {
            message: Message::Assistant {
                model, provider, ..
            },
            ..
        }) = m
        {
            Some(LoopConfigSnapshot {
                model: model.clone(),
                provider: provider.clone(),
                config_id: config_segment_from_loop_id(loop_id),
            })
        } else {
            None
        }
    })
}
