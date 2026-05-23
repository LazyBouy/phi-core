use super::agent_message::AgentMessage;
use super::event::ContinuationKind;
use super::node_tag::NodeId;
use super::tool::AgentTool;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// In-run context entry (2-stream architecture)
// ---------------------------------------------------------------------------

/// An entry in the in-run context — either a live message or a pruned replacement.
//
// `Live(AgentMessage)` is intentionally larger than the pruned variants — the
// loop holds `Vec<InRunEntry>` and reads `Live` on every turn; an extra heap
// indirection per access would be a hot-path regression. Composition I's
// addition of `node_id`/`parent_id`/`tags` on `LlmMessage` tipped this enum
// past clippy's 200-byte threshold; the size difference is by design.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone)]
pub enum InRunEntry {
    /// Live message — sent to LLM as-is.
    Live(AgentMessage),
    /// Pruned with memo — the original is in the session log; the LLM sees the memo.
    PrunedMemo {
        memo: String,
        tokens_removed: usize,
        timestamp: u64,
    },
    /// Pruned without memo — the original is in the session log; the LLM sees nothing.
    PrunedSilent {
        tokens_removed: usize,
        timestamp: u64,
    },
}

// ---------------------------------------------------------------------------
// Agent context (passed to the loop)
// ---------------------------------------------------------------------------

#[derive(Default, Clone)]
pub struct AgentContext {
    /*
    AgentContext vs Agent Session
    AgentContext is a snapshot — the minimal state needed to execute one agent loop call:
    system prompt + message history + available tools . It's stateless and passed around
    functionally.

    An Agent Session (your extension target) would be the lifecycle wrapper —
    tracking session ID, branch tree, timestamps, and multiple AgentContext snapshots over time.
    Think of it as: AgentContext is a single frame, a Session is the whole film reel.
    */
    pub system_prompt: String, // PROMPT — injected at the top of every LLM call (role="system")
    pub messages: Vec<AgentMessage>, // HISTORY — full conversation; grows each turn; includes Extension messages

    // Arc<dyn AgentTool>: shared ownership of type-erased tools. Arc (atomic reference-counted
    // pointer) allows AgentContext to be Clone — cloning a context for parallel branches is a
    // cheap reference-count increment on each tool, not a deep copy. Tools are immutable during
    // execution (execute takes &self), so sharing via Arc is semantically correct.
    pub tools: Vec<Arc<dyn AgentTool>>, // REGISTRY — available tool implementations; converted to ToolDefinition for LLM

    // ── Identity ─────────────────────────────────────────────────────────────
    // Set by callers (e.g. Agent wrapper) before each loop call.
    // agent_loop() auto-generates UUIDs if None and writes them back to context.
    // agent_loop_continue() asserts both are Some — continuations require stable identity.
    /// Stable identifier for the agent instance. Auto-generated UUID v4 if None on first call;
    /// written back to context so continuations inherit it.
    pub agent_id: Option<String>,

    /// Groups related loop calls under one logical session (evaluational parallelism, reruns,
    /// branches). Auto-generated UUID v4 if None on first call; written back to context.
    /// Required (Some) for all `agent_loop_continue()` calls.
    pub session_id: Option<String>,

    /// Unique identifier for this specific loop call: `"{session_id}.{config_id}.{N}"`.
    /// Set by Agent wrapper via `next_loop_id()`; direct callers may supply their own.
    /// Falls back to a UUID at loop entry if still None.
    pub loop_id: Option<String>,

    /// The `loop_id` of the loop this was continued from. None for origin calls.
    /// Set by Agent wrapper for `agent_loop_continue()` calls to enable ancestry tracking.
    pub parent_loop_id: Option<String>,

    /// How this loop relates to prior loops. None for origin calls.
    /// Some(Default|Rerun|Branch) for agent_loop_continue() calls.
    pub continuation_kind: Option<ContinuationKind>,

    /// Optional session for block-based compaction. When `Some`, the agent loop
    /// uses `compact_session_loops()` / `build_context_from_session()` instead of
    /// in-memory `CompactionStrategy::compact()`.
    ///
    /// When `None` (sub-agents, tests, direct callers), the loop falls back to
    /// the existing in-memory compaction path.
    pub session: Option<crate::session::Session>,

    /// User-injected context (prompts, steering, follow-ups). NEVER pruned.
    pub user_context: Vec<AgentMessage>,
    /// In-run context (model-generated). Can be pruned surgically.
    pub inrun_context: Vec<InRunEntry>,

    // ── Composition I (opt-in tree braking) ─────────────────────────────────
    /// The "active" node in the conversation tree — the head of the working
    /// trunk. Set by `apply_revert` (Phase 3). `None` unless a revert has
    /// occurred.
    ///
    /// When `Some`, [`build_working_context`](Self::build_working_context)
    /// dispatches to the parent-chain walk (`build_trunk_context`, Phase 4)
    /// instead of the linear merge path.
    pub active_node_id: Option<NodeId>,

    /// The next [`NodeId`] to allocate. Seeded at loop entry to ensure
    /// continuations don't collide with existing IDs in `messages`. Plain
    /// field (no serde) — `AgentContext` is a runtime snapshot, not persisted.
    pub next_node_id: u64,
}

impl AgentContext {
    /// Build the working context for LLM calls by merging user_context + live inrun entries.
    ///
    /// If both streams are empty (old code path, no PrunTool), falls back to `self.messages`
    /// for backward compatibility.
    ///
    /// Composition I — when [`active_node_id`](Self::active_node_id) is `Some`,
    /// dispatch to [`build_trunk_context`](Self::build_trunk_context) instead.
    /// The active pointer is set only by `apply_revert` (Phase 3) and is only
    /// set after the loop opts in via
    /// [`BasicAgent::with_revert_tool`](crate::agents::BasicAgent::with_revert_tool),
    /// so non-revert consumers take this branch never.
    pub fn build_working_context(&self) -> Vec<AgentMessage> {
        if self.active_node_id.is_some() {
            return self.build_trunk_context();
        }
        if self.user_context.is_empty() && self.inrun_context.is_empty() {
            return self.messages.clone();
        }

        let mut entries: Vec<(u64, AgentMessage)> = Vec::new();

        // Add all user_context entries with their timestamps
        for msg in &self.user_context {
            entries.push((msg.timestamp(), msg.clone()));
        }

        // Add live inrun entries and individual memo messages at their original timestamps
        for entry in &self.inrun_context {
            match entry {
                InRunEntry::Live(msg) => {
                    entries.push((msg.timestamp(), msg.clone()));
                }
                InRunEntry::PrunedMemo {
                    memo, timestamp, ..
                } => {
                    let memo_text = format!("[Pruned context summary: {}]", memo);
                    let memo_msg = AgentMessage::Llm(super::agent_message::LlmMessage::new(
                        super::content::Message::User {
                            content: vec![super::content::Content::Text { text: memo_text }],
                            timestamp: *timestamp,
                        },
                    ));
                    entries.push((*timestamp, memo_msg));
                }
                InRunEntry::PrunedSilent { .. } => {}
            }
        }

        // If the merge produced nothing, fall back
        if entries.is_empty() {
            return self.messages.clone();
        }

        // Sort by timestamp to preserve chronological order
        entries.sort_by_key(|(ts, _)| *ts);

        entries.into_iter().map(|(_, msg)| msg).collect()
    }

    /// Composition I — parent-chain assembly with kind-aware tag filtering.
    ///
    /// Variant of [`build_trunk_context`](Self::build_trunk_context) that
    /// applies a [`RevertRenderPolicy`] to each trunk node's tags. Decay-able
    /// tags (`Lesson`/`Finding`) outside the policy window are stripped from
    /// the returned message (they remain in `self.messages` — log-only after
    /// they decay). The `lesson_window_count` cap is enforced globally per
    /// `TagKind` over the whole trunk (most recent N decay-able tags of each
    /// kind always render even if some are outside the turn window).
    ///
    /// `current_turn` is the integer the caller stamps `NodeTag.created_at_turn`
    /// against — typically the agent-loop's `turn: usize` counter.
    pub fn build_trunk_context_with_policy(
        &self,
        policy: &super::node_tag::RevertRenderPolicy,
        current_turn: u32,
    ) -> Vec<AgentMessage> {
        let mut base = self.build_trunk_context();
        // First pass: collect (kind, created_at_turn, msg_index, tag_index) for
        // every decay-able tag on the trunk so we can apply the count cap.
        use super::node_tag::TagKind;
        let mut decayable: Vec<(TagKind, u32, usize, usize)> = Vec::new();
        for (mi, m) in base.iter().enumerate() {
            if let AgentMessage::Llm(lm) = m {
                for (ti, t) in lm.tags.iter().enumerate() {
                    if t.kind.is_decayable() {
                        decayable.push((t.kind, t.created_at_turn, mi, ti));
                    }
                }
            }
        }
        // Per-kind keep set: the most-recent `lesson_window_count` by
        // created_at_turn (descending). Stored as (msg_index, tag_index) pairs.
        use std::collections::HashSet;
        let mut force_keep: HashSet<(usize, usize)> = HashSet::new();
        for kind in [TagKind::Lesson, TagKind::Finding] {
            let mut of_kind: Vec<&(TagKind, u32, usize, usize)> =
                decayable.iter().filter(|(k, ..)| *k == kind).collect();
            of_kind.sort_by_key(|e| std::cmp::Reverse(e.1)); // descending by turn
            for entry in of_kind.iter().take(policy.lesson_window_count) {
                force_keep.insert((entry.2, entry.3));
            }
        }
        // Second pass: rebuild each Llm message's tags vector, dropping
        // decay-able tags outside the window AND not in the force-keep set.
        for (mi, m) in base.iter_mut().enumerate() {
            if let AgentMessage::Llm(lm) = m {
                let mut kept_tags = Vec::with_capacity(lm.tags.len());
                for (ti, tag) in lm.tags.iter().enumerate() {
                    let in_window = policy.renders_by_turn(tag, current_turn);
                    let forced = force_keep.contains(&(mi, ti));
                    if in_window || forced {
                        kept_tags.push(tag.clone());
                    }
                }
                lm.tags = kept_tags;
            }
        }
        base
    }

    /// Composition I — parent-chain assembly.
    ///
    /// Indexes every [`AgentMessage::Llm`] with a [`NodeId`] in `messages`,
    /// then walks `parent_id` links from [`active_node_id`](Self::active_node_id)
    /// up to a root. The resulting list is reversed back into chronological
    /// order. Messages off this trunk (the abandoned branches) are simply
    /// absent — the forensic record stays intact in `messages`.
    ///
    /// `user_context` entries are merged in at their timestamp position so
    /// the user's prompts always appear regardless of stamping state. Live
    /// `inrun_context` entries are NOT merged here — they already exist in
    /// `messages`; adding them again would duplicate them.
    ///
    /// Robustness:
    /// - **Cycle guard** — the walk tracks visited `NodeId`s; a corrupt
    ///   `parent_id` cycle stops at the second visit and never loops.
    /// - **Dangling parent** — when a `parent_id` does not resolve, the walk
    ///   stops at the last reachable node (graceful degradation, never panic).
    /// - **Missing active node** — when `active_node_id` does not resolve in
    ///   the index, falls back to `messages.clone()` so the LLM still sees
    ///   _something_ rather than an empty prompt.
    ///
    /// Extension messages (`AgentMessage::Extension`) pass through unchanged
    /// — they don't carry `node_id` and are not part of the LLM context anyway.
    pub fn build_trunk_context(&self) -> Vec<AgentMessage> {
        use std::collections::HashMap;

        let Some(active) = self.active_node_id else {
            return self.messages.clone();
        };

        // Index every Llm message that has a node_id, by its NodeId. Carry
        // the index into `messages` so we can recover chronological order
        // without reading the message body twice.
        let mut by_id: HashMap<NodeId, (usize, &super::agent_message::LlmMessage)> = HashMap::new();
        for (idx, m) in self.messages.iter().enumerate() {
            if let AgentMessage::Llm(lm) = m {
                if let Some(id) = lm.node_id {
                    by_id.insert(id, (idx, lm));
                }
            }
        }

        // Walk parent_id from active up to root. Cycle-guard via a visited
        // set so a corrupt chain can never loop forever (and we don't render
        // a node twice).
        let mut visited: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
        let mut trunk_indices: Vec<usize> = Vec::new();
        let mut cur = Some(active);
        while let Some(id) = cur {
            if !visited.insert(id) {
                break; // cycle detected
            }
            match by_id.get(&id) {
                Some((idx, lm)) => {
                    trunk_indices.push(*idx);
                    cur = lm.parent_id;
                }
                None => break, // dangling — stop gracefully
            }
        }

        if trunk_indices.is_empty() {
            // active_node_id did not resolve — fall back rather than emit nothing.
            return self.messages.clone();
        }

        // Reverse to chronological order.
        trunk_indices.reverse();

        // Assemble: trunk Llm messages + user_context entries merged at their
        // timestamps + non-Llm extension messages preserved by their original
        // index. We carry a `(sort_key, message)` pair where sort_key is the
        // index in `messages` for trunk entries and a synthetic post-position
        // for user_context entries based on timestamp.
        let mut out: Vec<(u64, AgentMessage)> = Vec::new();
        for idx in trunk_indices {
            let m = &self.messages[idx];
            out.push((m.timestamp(), m.clone()));
        }
        for um in &self.user_context {
            // Skip if this exact user message is already represented in trunk
            // (avoids double-rendering when user_context shadowed messages).
            let ts = um.timestamp();
            let already = out.iter().any(|(t, m)| {
                *t == ts
                    && matches!(
                        (um, m),
                        (AgentMessage::Llm(a), AgentMessage::Llm(b)) if a.message == b.message
                    )
            });
            if !already {
                out.push((ts, um.clone()));
            }
        }

        // Stable sort by timestamp to preserve chronological order across
        // merged streams.
        out.sort_by_key(|(ts, _)| *ts);
        out.into_iter().map(|(_, m)| m).collect()
    }

    // ── Composition I helpers ───────────────────────────────────────────────

    /// Allocate the next monotonic [`NodeId`] and increment the counter.
    /// Use this — not `NodeId::new` directly — when stamping a new message in
    /// the agent loop (Phase 4 wiring), so IDs stay unique within a session.
    pub fn alloc_node_id(&mut self) -> NodeId {
        let id = NodeId(self.next_node_id);
        self.next_node_id = self.next_node_id.saturating_add(1);
        id
    }

    /// Seed `next_node_id` from the maximum [`NodeId`] present in `messages`.
    /// Call at loop entry (Phase 4 wiring) so continuations resume the counter
    /// past any existing IDs. Idempotent — never decreases `next_node_id`.
    pub fn seed_next_node_id_from_messages(&mut self) {
        let max = self
            .messages
            .iter()
            .filter_map(|m| m.node_id())
            .map(|nid| nid.raw())
            .max();
        if let Some(m) = max {
            self.next_node_id = self.next_node_id.max(m.saturating_add(1));
        }
    }
}

#[cfg(test)]
mod node_id_alloc_tests {
    //! Composition I Phase 1 tests for [`AgentContext::alloc_node_id`] and
    //! [`AgentContext::seed_next_node_id_from_messages`].
    use super::*;
    use crate::types::content::Message;

    #[test]
    fn alloc_node_id_is_monotonic_from_zero() {
        let mut ctx = AgentContext::default();
        assert_eq!(ctx.alloc_node_id(), NodeId(0));
        assert_eq!(ctx.alloc_node_id(), NodeId(1));
        assert_eq!(ctx.alloc_node_id(), NodeId(2));
        assert_eq!(ctx.next_node_id, 3);
    }

    #[test]
    fn seed_resumes_from_max_existing_node_id() {
        let mut ctx = AgentContext::default();
        ctx.messages.push(
            AgentMessage::from(Message::User {
                content: vec![],
                timestamp: 1,
            })
            .with_node_identity(NodeId(5), None),
        );
        ctx.messages.push(
            AgentMessage::from(Message::User {
                content: vec![],
                timestamp: 2,
            })
            .with_node_identity(NodeId(7), Some(NodeId(5))),
        );
        ctx.seed_next_node_id_from_messages();
        // First alloc after seeding picks up at max+1 = 8.
        assert_eq!(ctx.alloc_node_id(), NodeId(8));
        assert_eq!(ctx.alloc_node_id(), NodeId(9));
    }

    #[test]
    fn seed_no_op_when_no_existing_node_ids() {
        let mut ctx = AgentContext::default();
        ctx.messages.push(AgentMessage::from(Message::User {
            content: vec![],
            timestamp: 1,
        }));
        ctx.seed_next_node_id_from_messages();
        // No stamped node_ids ⇒ counter stays at 0.
        assert_eq!(ctx.alloc_node_id(), NodeId(0));
    }

    #[test]
    fn seed_is_idempotent_and_never_decreases() {
        let mut ctx = AgentContext::default();
        ctx.messages.push(
            AgentMessage::from(Message::User {
                content: vec![],
                timestamp: 1,
            })
            .with_node_identity(NodeId(3), None),
        );
        ctx.seed_next_node_id_from_messages();
        let _ = ctx.alloc_node_id(); // n4
        let _ = ctx.alloc_node_id(); // n5
                                     // Re-seeding must not roll back; messages still only have max=n3, but
                                     // the counter is already at 6.
        ctx.seed_next_node_id_from_messages();
        assert_eq!(ctx.alloc_node_id(), NodeId(6));
    }

    #[test]
    fn active_node_id_defaults_to_none() {
        let ctx = AgentContext::default();
        assert!(ctx.active_node_id.is_none());
    }
}

#[cfg(test)]
mod build_trunk_context_tests {
    //! Composition I Phase 4 — opt-in parent-chain assembly.
    use super::super::agent_message::LlmMessage;
    use super::super::content::{Content, Message, StopReason};
    use super::super::usage::Usage;
    use super::*;

    fn assistant(text: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: ts,
                error_message: None,
            })
            .with_node_identity(node, parent),
        )
    }

    fn user(text: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::User {
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                timestamp: ts,
            })
            .with_node_identity(node, parent),
        )
    }

    #[test]
    fn linear_path_when_active_pointer_is_none() {
        // Critical opt-in regression test: byte-identical to pre-0.8.0 path
        // when revert mode never engaged.
        let ctx = AgentContext {
            messages: vec![
                user("hi", 1, NodeId(0), None),
                assistant("hello", 2, NodeId(1), Some(NodeId(0))),
            ],
            ..Default::default()
        };
        assert!(ctx.active_node_id.is_none());
        let built = ctx.build_working_context();
        // No user_context, no inrun_context, active=None → returns messages clone.
        assert_eq!(built.len(), 2);
    }

    #[test]
    fn trunk_walk_omits_abandoned_branch() {
        // n10 ← n11 (abandoned) ← n12 (abandoned)
        //        \_ n13 ← n14 (new active)
        let ctx = AgentContext {
            messages: vec![
                user("write a sort", 1, NodeId(10), None),
                assistant("trying bubble", 2, NodeId(11), Some(NodeId(10))),
                assistant("timed out", 3, NodeId(12), Some(NodeId(11))),
                assistant("trying quick", 4, NodeId(13), Some(NodeId(10))),
                assistant("works", 5, NodeId(14), Some(NodeId(13))),
            ],
            active_node_id: Some(NodeId(14)),
            ..Default::default()
        };
        let built = ctx.build_working_context();
        let texts: Vec<String> = built
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => match &lm.message {
                    Message::User { content, .. } | Message::Assistant { content, .. } => {
                        content.iter().find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect();
        assert_eq!(texts, vec!["write a sort", "trying quick", "works"]);
    }

    #[test]
    fn cycle_in_parent_id_does_not_loop_forever() {
        // n10 → parent n11 → parent n10 (cycle)
        let ctx = AgentContext {
            messages: vec![
                assistant("a", 1, NodeId(10), Some(NodeId(11))),
                assistant("b", 2, NodeId(11), Some(NodeId(10))),
            ],
            active_node_id: Some(NodeId(11)),
            ..Default::default()
        };
        let built = ctx.build_working_context();
        // Walk is capped at messages.len()+1 = 3; doesn't panic, doesn't hang.
        assert!(built.len() <= 2);
    }

    #[test]
    fn dangling_parent_stops_walk_gracefully() {
        // n10 → parent n99 (doesn't exist)
        let ctx = AgentContext {
            messages: vec![assistant("a", 1, NodeId(10), Some(NodeId(99)))],
            active_node_id: Some(NodeId(10)),
            ..Default::default()
        };
        let built = ctx.build_working_context();
        assert_eq!(built.len(), 1);
    }

    #[test]
    fn unresolved_active_node_falls_back_to_messages() {
        let ctx = AgentContext {
            messages: vec![assistant("a", 1, NodeId(10), None)],
            active_node_id: Some(NodeId(99)),
            ..Default::default()
        };
        let built = ctx.build_working_context();
        // Non-empty fallback rather than an empty prompt.
        assert_eq!(built.len(), 1);
    }

    // ── Phase 5 — render policy ────────────────────────────────────────────

    use super::super::node_tag::{NodeTag, RevertRenderPolicy, TagKind};

    fn tag(kind: TagKind, turn: u32, text: &str) -> NodeTag {
        NodeTag::new(kind, text.to_string(), turn, vec![])
    }

    fn build_ctx_with_tags(tags: Vec<(NodeId, NodeTag)>) -> AgentContext {
        let mut msgs: Vec<AgentMessage> = Vec::new();
        for (i, (id, t)) in tags.iter().enumerate() {
            let parent = if i == 0 { None } else { Some(tags[i - 1].0) };
            let mut am = assistant("body", (i + 1) as u64, *id, parent);
            if let AgentMessage::Llm(lm) = &mut am {
                lm.tags.push(t.clone());
            }
            msgs.push(am);
        }
        let last = tags.last().map(|(id, _)| *id);
        AgentContext {
            messages: msgs,
            active_node_id: last,
            ..Default::default()
        }
    }

    #[test]
    fn render_policy_keeps_pinned_tags_indefinitely() {
        let ctx = build_ctx_with_tags(vec![(NodeId(0), tag(TagKind::Outcome, 0, "sealed"))]);
        let policy = RevertRenderPolicy::default();
        let built = ctx.build_trunk_context_with_policy(&policy, 1000);
        let kept_tags: Vec<&NodeTag> = built
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => Some(lm.tags.iter()),
                _ => None,
            })
            .flatten()
            .collect();
        assert_eq!(kept_tags.len(), 1);
        assert_eq!(kept_tags[0].kind, TagKind::Outcome);
    }

    #[test]
    fn render_policy_drops_old_decayable_tags() {
        // 4 lessons at turns 0, 1, 2, 3. With default policy (5 turn window,
        // count cap 3), at current_turn=10 ALL fall outside the window and
        // the count cap retains only the 3 newest: turns 1, 2, 3.
        let ctx = build_ctx_with_tags(vec![
            (NodeId(0), tag(TagKind::Lesson, 0, "L0")),
            (NodeId(1), tag(TagKind::Lesson, 1, "L1")),
            (NodeId(2), tag(TagKind::Lesson, 2, "L2")),
            (NodeId(3), tag(TagKind::Lesson, 3, "L3")),
        ]);
        let policy = RevertRenderPolicy::default();
        let built = ctx.build_trunk_context_with_policy(&policy, 10);
        let mut kept_turns: Vec<u32> = built
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => Some(lm.tags.iter()),
                _ => None,
            })
            .flatten()
            .map(|t| t.created_at_turn)
            .collect();
        kept_turns.sort();
        assert_eq!(kept_turns, vec![1, 2, 3]); // count cap retains the 3 newest
    }

    #[test]
    fn render_policy_in_window_decayable_renders_regardless_of_count() {
        // 4 findings, ALL within window — should all render even though
        // count cap is 3 (the cap only matters once tags fall out of window).
        let ctx = build_ctx_with_tags(vec![
            (NodeId(0), tag(TagKind::Finding, 1, "F1")),
            (NodeId(1), tag(TagKind::Finding, 2, "F2")),
            (NodeId(2), tag(TagKind::Finding, 3, "F3")),
            (NodeId(3), tag(TagKind::Finding, 4, "F4")),
        ]);
        let policy = RevertRenderPolicy::default(); // window 5
        let built = ctx.build_trunk_context_with_policy(&policy, 5);
        let kept_count = built
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => Some(lm.tags.len()),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(kept_count, 4);
    }

    #[test]
    fn render_policy_does_not_mutate_original_context() {
        let ctx = build_ctx_with_tags(vec![(NodeId(0), tag(TagKind::Lesson, 0, "L0"))]);
        let policy = RevertRenderPolicy {
            lesson_window_turns: 0,
            lesson_window_count: 0,
        };
        let _ = ctx.build_trunk_context_with_policy(&policy, 100);
        // Original tag is still present in self.messages (build_trunk_*
        // returns clones; the forensic record is intact).
        let original_tags = ctx
            .messages
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => Some(lm.tags.len()),
                _ => None,
            })
            .sum::<usize>();
        assert_eq!(original_tags, 1);
    }
}
