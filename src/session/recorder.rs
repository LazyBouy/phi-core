use super::helpers::*;
use super::model::*;
use crate::types::*;
use std::collections::HashMap;

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

/// Partial turn state accumulated between `TurnStart` and `TurnEnd`.
/// Finalized into a [`Turn`] when `TurnEnd` is received.
struct PartialTurn {
    turn_id: TurnId,
    triggered_by: TurnTrigger,
    started_at: chrono::DateTime<chrono::Utc>,
    input_messages: Vec<AgentMessage>,
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

    /// Turns being accumulated between `TurnStart` and `TurnEnd`, keyed by `loop_id`.
    partial_turns: HashMap<String, PartialTurn>,
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
            partial_turns: HashMap::new(),
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
                    turns: Vec::new(),
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
                // Discard any orphaned partial turn for this loop.
                self.partial_turns.remove(loop_id.as_str());
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

            // ── TurnStart — begin accumulating a partial turn ────────────
            AgentEvent::TurnStart {
                loop_id,
                turn_index,
                timestamp,
                triggered_by,
            } => {
                self.partial_turns.insert(
                    loop_id.clone(),
                    PartialTurn {
                        turn_id: TurnId {
                            loop_id: loop_id.clone(),
                            turn_index: *turn_index,
                        },
                        triggered_by: triggered_by.clone(),
                        started_at: *timestamp,
                        input_messages: Vec::new(),
                    },
                );
                self.append_event(loop_id, event.clone());
            }

            // ── MessageEnd — capture non-assistant messages as turn input ─
            AgentEvent::MessageEnd {
                loop_id, message, ..
            } => {
                if message.role() != "assistant" {
                    if let Some(partial) = self.partial_turns.get_mut(loop_id.as_str()) {
                        partial.input_messages.push(message.clone());
                    }
                }
                self.append_event(loop_id, event.clone());
            }

            // ── TurnEnd — finalize turn + extract config snapshot ─────────
            AgentEvent::TurnEnd {
                loop_id,
                message,
                usage,
                timestamp,
                tool_results,
            } => {
                self.append_event(loop_id, event.clone());

                // Finalize the partial turn into a materialized Turn.
                if let Some(partial) = self.partial_turns.remove(loop_id.as_str()) {
                    let tid = Some(partial.turn_id.clone());
                    let turn = Turn {
                        turn_id: partial.turn_id,
                        triggered_by: partial.triggered_by,
                        usage: usage.clone(),
                        input_messages: partial.input_messages,
                        output_message: message.clone(),
                        tool_results: tool_results
                            .iter()
                            .map(|m| AgentMessage::from(m.clone()).with_turn_id(tid.clone()))
                            .collect(),
                        started_at: partial.started_at,
                        ended_at: *timestamp,
                    };
                    if let Some(open) = self.open_loops.get_mut(loop_id.as_str()) {
                        open.record.turns.push(turn);
                    }
                }

                // Extract config snapshot from assistant message.
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
        // Discard orphaned partial turns (TurnStart received but no TurnEnd).
        self.partial_turns.clear();

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
