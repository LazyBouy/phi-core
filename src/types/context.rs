use super::agent_message::AgentMessage;
use super::event::ContinuationKind;
use super::node_tag::NodeId;
use super::tool::AgentTool;
use std::sync::Arc;

/// Composition I — prepend a braking marker (`[n<id>] …`) onto a message's
/// content so it reaches the provider. Prefixes the leading text block when one
/// exists (the common case for user prompts + tool results); otherwise inserts a
/// fresh leading text block (e.g. an assistant message whose first block is a
/// `ToolCall`). Used only by [`AgentContext::weave_braking_annotations`].
fn prepend_marker_to_message(message: &mut super::content::Message, marker: &str) {
    use super::content::{Content, Message};
    let content = match message {
        Message::User { content, .. }
        | Message::Assistant { content, .. }
        | Message::ToolResult { content, .. } => content,
    };
    match content.first_mut() {
        Some(Content::Text { text }) => *text = format!("{marker} {text}"),
        _ => content.insert(
            0,
            Content::Text {
                text: marker.to_string(),
            },
        ),
    }
}

/// Literal marker that prefixes every synthetic post-revert continue-forward
/// steering message ([`AgentContext::inject_continue_after_revert`]).
///
/// These messages are **model-steering signals**, NOT real user input: after a
/// revert, the surviving tip carries the revert breadcrumb but a weaker model
/// can loop on the re-presented task (re-doing already-completed steps). A
/// separate synthetic `Message::User` placed right after the tip steers it
/// onward. The `[continue_after_revert]` prefix lets consumers (channels, UIs,
/// transcript renderers) identify + filter these synthetic steering messages
/// out of the real conversation stream.
pub const CONTINUE_AFTER_REVERT_MARKER: &str = "[continue_after_revert]";

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
        let base = self.build_trunk_context();
        Self::decay_tags_by_policy(base, policy, current_turn)
    }

    /// Composition I — apply the [`RevertRenderPolicy`] tag-decay to an already-
    /// assembled trunk (CC-18 iter-2 extraction).
    ///
    /// This is the pure tag-decay second pass that
    /// [`build_trunk_context_with_policy`](Self::build_trunk_context_with_policy)
    /// historically inlined. It is now a free helper so the streaming render path
    /// can apply tag-decay to the **already-collapsed** trunk (collapse runs
    /// FIRST per #73 / F-persist-order.a, so the heavy content it must clear is
    /// visible). Decay-able tags (`Lesson`/`Finding`) outside the policy window
    /// are stripped from each message; the `lesson_window_count` cap is enforced
    /// globally per `TagKind`. Pinned tags always render. The lone-breadcrumb
    /// NODE-drop is NOT here — it is owned by `collapse_abandon_class_cluster`
    /// (the only pass that knows per-cluster tag-window state AND owns content).
    pub(crate) fn decay_tags_by_policy(
        mut base: Vec<AgentMessage>,
        policy: &super::node_tag::RevertRenderPolicy,
        current_turn: u32,
    ) -> Vec<AgentMessage> {
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
        //
        // CC-18 iter-2 (#73): the lone-breadcrumb NODE-drop (F-collapsed-node-
        // decay.a) has MOVED OUT of this pass into the policy-aware
        // `collapse_abandon_class_cluster` scan. In iter-1 the node-drop lived
        // here, ran BEFORE collapse, and read the immutable heavy `messages`, so
        // it never saw a lone breadcrumb to drop in production (#73 §2.3). This
        // pass now ONLY decays out-of-window *tags* (its original pre-iter-1
        // contract) — it runs on the already-collapsed trunk, and the
        // content-less node drop is owned by collapse, which is the only pass
        // that both knows the per-cluster tag-window state AND owns the content.
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

    /// Composition I — render-time reclamation of EVERY abandon-class
    /// tool-cluster a revert landed on, **persistently across turns**.
    ///
    /// When a `failure`/`tangent` (abandon-class) revert targets a node that is
    /// (or splits) a heavy `(assistant-tool_call, tool_result)` cluster, the
    /// kept node ends up carrying the abandoned tool-call's heavy `arguments`
    /// (e.g. an 80-line `write_file` body) while its matching tool-result has
    /// been dropped off-trunk by the parent-chain walk — leaving the rendered
    /// trunk both **larger** (the abandoned body survives) and **malformed** (a
    /// tool-call with no matching result). This pass fixes both, at render time
    /// only.
    ///
    /// **CC-18 iter-2 — PERSISTENT scan (GitHub #73 / D-TEST-0068).** The iter-1
    /// version acted ONLY on the trunk tip, so the reclamation evaporated the
    /// moment the model continued forward (the reverted cluster slid mid-trunk
    /// and its heavy body re-rendered every subsequent turn). This version scans
    /// **every** node-bearing trunk message and collapses **every** cluster
    /// carrying a live abandon-class (decay-able `Lesson`/`Finding`) revert tag —
    /// so the abandoned body stays reclaimed on EVERY render, not just the one
    /// right after the revert. The tag persists on the target node in
    /// `self.messages` (`apply_revert`'s `add_tag`), so each tagged cluster is
    /// re-discoverable on every render.
    ///
    /// Per tagged cluster the disposition is identical to iter-1:
    ///
    /// - **Abandon-class, IN-window** (the cluster's decay-able tag still renders
    ///   per `policy.renders_by_turn(tag, current_turn)`): the whole cluster is
    ///   collapsed atomically to a single clean assistant node carrying ONLY the
    ///   breadcrumb — its ENTIRE content (the heavy `ToolCall` AND any
    ///   accompanying reasoning `Text`) is replaced. The single-axis rule (CC-18,
    ///   GitHub #72 v2) branches purely on `is_decayable(category)`: a
    ///   `failure`/`tangent` revert scraps the abandoned work, so the surviving
    ///   node must carry nothing the model can re-execute. The CC-17 predecessor
    ///   stripped only the `ToolCall`, leaving the misleading "Starting with Step
    ///   1…" text that drove weak-model looping (CC-17 close-gate-results.md:29);
    ///   CC-18 replaces the whole content. The one-line breadcrumb (composed by
    ///   `apply_revert`) is woven into content by `weave_braking_annotations`
    ///   downstream, so the model reads the breadcrumb where the ~80-line body
    ///   used to be. Two cluster shapes are handled: a **call-node** (the tagged
    ///   node IS the assistant tool-call node) clears that node's content — its
    ///   matching tool-result is off-trunk (a child excluded by the parent-chain
    ///   walk), so nothing dangles; a **result-node** (the tagged node is the
    ///   matching tool-result, the #59 shape) finds the parent assistant-call
    ///   node on the trunk, clears ITS content, MOVES the tagged node's
    ///   `NodeTag`(s) onto that parent, and REMOVES the tool-result node from the
    ///   trunk Vec — so there is no orphaned result (a `tool_call_id` with no
    ///   matching call would be rejected by OpenAI-compat providers).
    /// - **Abandon-class, OUT-of-window** (the cluster's decay-able tag has
    ///   decayed past `policy.renders_by_turn`): the cluster is collapsed exactly
    ///   as above AND then the now-content-less node is **DROPPED** from the
    ///   trunk entirely — the breadcrumb itself is reclaimed once its lesson is no
    ///   longer rendered (GitHub #72 v2 principle 4: "eventually reclaim even the
    ///   breadcrumb"). This is the **reachable decay-drop** (F-collapsed-node-
    ///   decay.a): in iter-1 the drop lived in `build_trunk_context_with_policy`,
    ///   ran BEFORE the collapse, and read the immutable heavy `messages`, so it
    ///   never saw a lone breadcrumb to drop (#73 §2.3). Moving it INTO this
    ///   policy-aware collapse — the only pass that both knows the per-cluster
    ///   tag-window state AND owns the content — makes it fire in production.
    /// - **Pinned cluster** (`Outcome`/`Checkpoint`, NOT decay-able): the cluster
    ///   is load-bearing (a sealed result the model may re-read), so it is kept
    ///   WHOLE everywhere on the trunk — if the node is a tool-call whose matching
    ///   tool-result is off-trunk, the result is re-appended right after it so the
    ///   kept call is never left dangling. Pinned clusters are never collapsed and
    ///   never dropped.
    ///
    /// The **atomic-cluster invariant** therefore holds for ALL categories: the
    /// rendered trunk never carries a tool-call without its matching result
    /// (abandon-class drops both; pinned keeps both).
    ///
    /// Operates over the **cloned** trunk that `build_trunk_context` returns by
    /// value; `self.messages` (the forensic log) is **never** mutated — the
    /// persistent scan still operates only on the cloned trunk. Caller contract:
    /// invoke ONLY on the revert-mode trunk path (`active_node_id.is_some()`),
    /// FIRST in the render path (on the raw `build_trunk_context()` output, so it
    /// sees the raw heavy content it must clear), with the tag-decay rebuild
    /// (`build_trunk_context_with_policy`) applied to the already-collapsed trunk
    /// afterwards, and before `weave_braking_annotations`. General braking-
    /// machinery merit: any consumer reverting onto/across a heavy tool-cluster
    /// reclaims that context persistently with no log mutation.
    pub fn collapse_abandon_class_cluster(
        &self,
        mut trunk: Vec<AgentMessage>,
        policy: &super::node_tag::RevertRenderPolicy,
        current_turn: u32,
    ) -> Vec<AgentMessage> {
        use super::content::{Content, Message};

        // PERSISTENT SCAN (CC-18 iter-2 / #73). Walk EVERY node-bearing trunk
        // message — not just the tip — and act on each cluster whose first tag
        // is a revert tag. A node carries tags of one decay-class per revert; we
        // read the first tag's `kind` as the gate (abandon-class = decay-able
        // Lesson/Finding; pinned = Outcome/Checkpoint). Nodes without a tag are
        // skipped (no revert landed on them).
        //
        // We process node-by-node. Because abandon-class collapse on a
        // result-node REMOVES that node from the Vec (shifting later indices), we
        // re-scan from a running cursor rather than caching indices: each
        // iteration locates the NEXT unprocessed tagged node at-or-after the
        // cursor, acts on its cluster, then advances the cursor past the node
        // that now occupies the surviving slot. `processed` tracks node_ids we
        // have already collapsed/kept so a tag moved onto a parent (result-node
        // shape) is not re-collapsed when the scan reaches that parent.
        use std::collections::HashSet;
        let mut processed: HashSet<NodeId> = HashSet::new();
        let mut cursor = 0usize;

        while cursor < trunk.len() {
            // Locate the next node-bearing message at-or-after `cursor` that
            // carries a revert tag and has not been processed yet.
            let found = trunk[cursor..].iter().enumerate().find_map(|(off, m)| {
                if let AgentMessage::Llm(lm) = m {
                    if let Some(node_id) = lm.node_id {
                        if !lm.tags.is_empty() && !processed.contains(&node_id) {
                            return Some((cursor + off, node_id));
                        }
                    }
                }
                None
            });
            let Some((idx, node_id)) = found else {
                break; // no further tagged clusters
            };

            let AgentMessage::Llm(node) = &trunk[idx] else {
                cursor = idx + 1;
                continue;
            };
            let Some(tag) = node.tags.first() else {
                cursor = idx + 1;
                continue;
            };
            let abandon_class = tag.kind.is_decayable();
            // For abandon-class clusters: is the (newest) tag still in the decay
            // window? In-window → collapse to breadcrumb; out-of-window → collapse
            // AND drop the now-content-less node (the reachable decay-drop).
            let in_window = node
                .tags
                .iter()
                .max_by_key(|t| t.created_at_turn)
                .map(|t| policy.renders_by_turn(t, current_turn))
                .unwrap_or(true);

            // Classify the node's message shape (call-node vs result-node) and
            // recover the cluster's tool_call_id (see iter-1 doc-comment).
            let node_is_call = matches!(node.message, Message::Assistant { .. });
            let node_call_id = match &node.message {
                Message::Assistant { content, .. } => content.iter().find_map(|b| match b {
                    Content::ToolCall { id, .. } => Some(id.clone()),
                    _ => None,
                }),
                Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
                _ => None,
            };
            let Some(node_call_id) = node_call_id else {
                // Tagged node that is neither a tool-call nor a tool-result (e.g.
                // a tagged assistant text node). Mark processed; if it is an
                // abandon-class lone node out of window, drop it.
                processed.insert(node_id);
                if abandon_class && !in_window {
                    let only_breadcrumb = matches!(&node.message, Message::Assistant { content, .. }
                    if !content.iter().any(|b| match b {
                        Content::ToolCall { .. } => true,
                        Content::Text { text } => !text.trim().is_empty(),
                        _ => true,
                    }));
                    if only_breadcrumb {
                        trunk.remove(idx);
                        continue; // keep cursor — the next node shifted into `idx`
                    }
                }
                cursor = idx + 1;
                continue;
            };

            if abandon_class {
                // The surviving/collapsed node's slot (so the decay-drop can
                // target it). For a call-node it is `idx`; for a result-node it
                // is the parent call's index (after the result node is removed).
                let surviving_idx: Option<usize>;

                if node_is_call {
                    // Call-node shape: replace its ENTIRE content with the
                    // breadcrumb (drop the heavy ToolCall AND any reasoning Text).
                    if let AgentMessage::Llm(lm) = &mut trunk[idx] {
                        if let Message::Assistant { content, .. } = &mut lm.message {
                            content.clear();
                        }
                    }
                    processed.insert(node_id);
                    surviving_idx = Some(idx);
                } else {
                    // Result-node shape (#59-canonical): the heavy body lives in
                    // the PARENT assistant-call node. Find it, clear its content,
                    // MOVE this node's tags onto it, and REMOVE this result node
                    // from the trunk so no orphaned result survives.
                    if let Some(parent_idx) = trunk.iter().position(|m| match m {
                        AgentMessage::Llm(lm) => match &lm.message {
                            Message::Assistant { content, .. } => content.iter().any(
                                |b| matches!(b, Content::ToolCall { id, .. } if *id == node_call_id),
                            ),
                            _ => false,
                        },
                        _ => false,
                    }) {
                        let moved_tags = match &trunk[idx] {
                            AgentMessage::Llm(lm) => lm.tags.clone(),
                            _ => Vec::new(),
                        };
                        let parent_node_id = match &trunk[parent_idx] {
                            AgentMessage::Llm(lm) => lm.node_id,
                            _ => None,
                        };
                        if let AgentMessage::Llm(lm) = &mut trunk[parent_idx] {
                            if let Message::Assistant { content, .. } = &mut lm.message {
                                content.clear();
                            }
                            lm.tags.extend(moved_tags);
                        }
                        // Mark BOTH the result node and the parent as processed so
                        // the scan does not re-collapse the parent when it reaches
                        // it (it now carries the moved tag).
                        processed.insert(node_id);
                        if let Some(pid) = parent_node_id {
                            processed.insert(pid);
                        }
                        // Remove the result node; the parent's index shifts if it
                        // was after the result (it is always before, so it does
                        // not — but compute the surviving index defensively).
                        trunk.remove(idx);
                        surviving_idx = Some(if parent_idx > idx {
                            parent_idx - 1
                        } else {
                            parent_idx
                        });
                    } else {
                        // No parent on-trunk (degenerate) — just drop the orphan
                        // result tag node from consideration.
                        processed.insert(node_id);
                        surviving_idx = None;
                    }
                }

                // Reachable decay-drop: if the cluster's tag is OUT of the decay
                // window, the surviving node is now a lone content-less breadcrumb
                // → DROP it entirely (the breadcrumb is reclaimed once its lesson
                // no longer renders). In-window → keep the collapsed node; the tag
                // is woven into content downstream.
                if !in_window {
                    if let Some(s) = surviving_idx {
                        trunk.remove(s);
                        // Do NOT advance the cursor — the next node shifted into
                        // the freed slot (which is <= idx, so re-scan from cursor
                        // is still correct).
                        continue;
                    }
                }
                // In-window collapse keeps the node in place; advance the cursor
                // past it (or, for the removed result-node, the freed slot now
                // holds the next message — but the parent we collapsed sits
                // BEFORE idx and is already `processed`, so advancing past `idx`
                // is correct only when the surviving node is at/after idx).
                cursor = idx; // re-scan from idx; `processed` prevents re-work
            } else {
                // Pinned cluster (Outcome/Checkpoint): keep WHOLE. If the node is
                // a tool-call whose matching tool-result is off-trunk, re-append
                // the result (read from the forensic log) right after it so the
                // kept call never dangles. Never collapsed, never dropped.
                if node_is_call {
                    let already_present = trunk.iter().any(|m| match m {
                        AgentMessage::Llm(lm) => matches!(
                            &lm.message,
                            Message::ToolResult { tool_call_id, .. } if *tool_call_id == node_call_id
                        ),
                        _ => false,
                    });
                    if !already_present {
                        if let Some(result) = self.messages.iter().find(|m| match m {
                            AgentMessage::Llm(lm) => matches!(
                                &lm.message,
                                Message::ToolResult { tool_call_id, .. } if *tool_call_id == node_call_id
                            ),
                            _ => false,
                        }) {
                            trunk.insert(idx + 1, result.clone());
                        }
                    }
                }
                // Pinned + result-node: the cluster (parent call + result) is
                // already whole on-trunk — no action.
                processed.insert(node_id);
                cursor = idx + 1;
            }
        }

        trunk
    }

    /// Composition I — weave node markers + surviving tag annotations into the
    /// message **content** the model actually sees.
    ///
    /// `build_trunk_context[_with_policy]` indexes nodes and filters tags by the
    /// decay policy, but `node_id` + `tags` live as METADATA on
    /// [`LlmMessage`](super::agent_message::LlmMessage) that the `convert_to_llm`
    /// step strips before the provider call. So historically the model saw
    /// neither the `n<id>` it must echo into `revert_to_state(step=…)` nor the
    /// lesson/finding summary the tool promises "the next turn sees" — making the
    /// revert tool unusable from a cold start (it could not know a valid `step`).
    ///
    /// This bakes both into the content, in place, so they survive to the wire:
    /// - each `Llm` message carrying a `node_id` gets a leading `[n<id>]` marker;
    /// - each surviving [`NodeTag`](super::node_tag::NodeTag) renders right after
    ///   the marker as `[<kind>: <text>]` (e.g. `[lesson: …]` / `[checkpoint: …]`).
    ///
    /// Caller contract: invoke ONLY on the revert-mode trunk path
    /// (`active_node_id.is_some()`), after `build_trunk_context_with_policy` has
    /// already decayed out-of-window tags. Messages without a `node_id` pass
    /// through untouched, so non-revert consumers are byte-identical.
    pub fn weave_braking_annotations(messages: Vec<AgentMessage>) -> Vec<AgentMessage> {
        use super::node_tag::TagKind;
        use std::collections::HashSet;

        // Single pass: weave the per-node `[n<id>]` markers + surviving
        // lesson/finding tags into each node's content. The continue-forward
        // steering is NOT folded here — it is emitted as a separate, decaying,
        // tagged synthetic `Message::User` by
        // [`inject_continue_after_revert`](Self::inject_continue_after_revert),
        // wired in downstream (after this weave) at the streaming call site.
        let woven: Vec<AgentMessage> = messages
            .into_iter()
            .map(|m| match m {
                AgentMessage::Llm(mut lm) => {
                    let Some(node_id) = lm.node_id else {
                        return AgentMessage::Llm(lm);
                    };
                    let mut marker = format!("[{}]", node_id.render());
                    // Dedup ALL identical lesson/finding tags (same kind + same
                    // text) on the same node — not just consecutive runs.
                    // Repeated reverts to the same node stack duplicate tags,
                    // and other tags can interleave between the repeats; a
                    // seen-set renders each `(kind, text)` once regardless of
                    // ordering, removing the wall of noise that can confuse
                    // weaker models into looping.
                    let mut seen: HashSet<(TagKind, &str)> = HashSet::new();
                    for tag in &lm.tags {
                        if !seen.insert((tag.kind, tag.text.as_str())) {
                            continue; // identical tag already rendered on this node
                        }
                        let kind = match tag.kind {
                            TagKind::Lesson => "lesson",
                            TagKind::Finding => "finding",
                            TagKind::Outcome => "outcome",
                            TagKind::Checkpoint => "checkpoint",
                        };
                        marker.push_str(&format!(" [{}: {}]", kind, tag.text));
                    }
                    prepend_marker_to_message(&mut lm.message, &marker);
                    AgentMessage::Llm(lm)
                }
                other => other,
            })
            .collect();

        woven
    }

    /// Composition I — emit a decaying, tagged, all-category post-revert
    /// continue-forward steering message.
    ///
    /// After a revert, the surviving trunk tip carries the revert breadcrumb tag
    /// (composed by `apply_revert`), but a weaker model can loop on the
    /// re-presented task — re-doing already-completed steps instead of advancing.
    /// This inserts a **separate synthetic `Message::User`** right after the tip,
    /// prefixed with [`CONTINUE_AFTER_REVERT_MARKER`] so consumers can identify +
    /// filter it out of the real conversation stream (it is a model-steering
    /// signal, NOT real user input).
    ///
    /// Properties:
    /// - **All categories** — emitted irrespective of the revert tag kind
    ///   (failure→`Lesson`, tangent→`Finding`, completion→`Outcome`,
    ///   step-summary→`Checkpoint`). Any revert tag on the tip triggers it.
    /// - **Decays** — emitted ONLY while the tip's most-recent revert tag is
    ///   within the decay window, i.e. iff
    ///   `current_turn - tag.created_at_turn <= decay_window`. Past the window
    ///   the message is NOT emitted — even for pinned (`Outcome`/`Checkpoint`)
    ///   tags whose TAG itself persists on the trunk. The steering is a
    ///   transient nudge, not a permanent fixture.
    /// - **Breadcrumb echo** — the most-recent tag's text is echoed into the
    ///   message (as the old standalone note did) so the model concretely knows
    ///   what was just reverted from.
    ///
    /// Caller contract: invoke ONLY on the revert-mode trunk path
    /// (`active_node_id.is_some()`), AFTER `collapse_abandon_class_cluster` (so
    /// the tip is the surviving node carrying the moved tag) and AFTER
    /// `weave_braking_annotations`. `current_turn` is the agent-loop turn index;
    /// `decay_window` is the consumer's `lesson_window_turns`. Messages with no
    /// node-bearing tip, or a tip whose newest revert tag is past the window,
    /// pass through untouched — so non-revert / decayed consumers are
    /// byte-identical.
    pub fn inject_continue_after_revert(
        messages: Vec<AgentMessage>,
        current_turn: u32,
        decay_window: u32,
    ) -> Vec<AgentMessage> {
        use super::content::{Content, Message};

        // Locate the trunk tip (the LAST node-bearing message) — the active /
        // reverted-to node. After the collapse this is the surviving node
        // carrying the (possibly moved) revert tag.
        let Some(tip_idx) = messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, m)| match m {
                AgentMessage::Llm(lm) if lm.node_id.is_some() => Some(i),
                _ => None,
            })
        else {
            return messages; // no node-bearing tip → not a revert-landed trunk
        };

        // Read the tip's MOST-RECENT revert tag (highest `created_at_turn`).
        // A node can accrue multiple tags across repeated reverts; the newest
        // governs the decay gate + supplies the breadcrumb echo. No tag → no
        // revert landed here → nothing to steer.
        let AgentMessage::Llm(tip) = &messages[tip_idx] else {
            return messages;
        };
        let Some(newest) = tip.tags.iter().max_by_key(|t| t.created_at_turn) else {
            return messages; // no revert tag on the tip → no steering
        };

        // Decay gate — emit iff the newest tag is within the decay window. This
        // fires for ALL categories (decay-able Lesson/Finding AND pinned
        // Outcome/Checkpoint) while in window; past the window it is suppressed
        // even though a pinned tag's TAG persists on the trunk.
        if current_turn.saturating_sub(newest.created_at_turn) > decay_window {
            return messages;
        }

        // Compose the steering text: `[continue_after_revert]` marker + an
        // optional breadcrumb echo (the newest tag's text) + the concrete
        // continue-forward directive.
        let trimmed = newest.text.trim();
        let crumb = if trimmed.is_empty() {
            String::new()
        } else {
            format!(" {trimmed}.")
        };
        let text = format!(
            "{CONTINUE_AFTER_REVERT_MARKER}{crumb} You just reverted to this \
             node. Continue forward: do the next uncompleted step; do NOT redo \
             completed steps; do NOT stop."
        );
        let note = AgentMessage::Llm(super::agent_message::LlmMessage::new(Message::User {
            content: vec![Content::Text { text }],
            timestamp: 0,
        }));

        let mut out = messages;
        out.insert(tip_idx + 1, note);
        out
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

    fn tool_result(text: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: "tc-1".to_string(),
                tool_name: "write_file".to_string(),
                content: vec![Content::Text {
                    text: text.to_string(),
                }],
                is_error: false,
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
        // Window 3 (0.11 default). Evaluate at current_turn=4 so all 4 findings
        // (turns 1..4) fall within the 3-turn window (distances 3,2,1,0); the
        // count cap is irrelevant while everything is in-window.
        let policy = RevertRenderPolicy::default();
        let built = ctx.build_trunk_context_with_policy(&policy, 4);
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
    fn decay_count_cap_retains_three_newest_out_of_window() {
        // Regression guard: the marker-rework + breadcrumb + all-identical
        // dedup must NOT perturb the count-cap behaviour. Default policy =
        // 3-turn window (0.11), count cap 3. 4 lessons at turns 0..3 evaluated
        // at current_turn=10: all fall outside the window, the count cap retains
        // the 3 newest (turns 1,2,3) — the count-cap contract is unchanged by
        // the window default-change.
        let ctx = build_ctx_with_tags(vec![
            (NodeId(0), tag(TagKind::Lesson, 0, "L0")),
            (NodeId(1), tag(TagKind::Lesson, 1, "L1")),
            (NodeId(2), tag(TagKind::Lesson, 2, "L2")),
            (NodeId(3), tag(TagKind::Lesson, 3, "L3")),
        ]);
        let policy = RevertRenderPolicy::default();
        assert_eq!(policy.lesson_window_turns, 3);
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
        assert_eq!(
            kept_turns,
            vec![1, 2, 3],
            "count cap must retain the 3 newest tags out of window"
        );
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

    // ── braking annotations woven into model-visible content ──

    fn first_text(m: &AgentMessage) -> String {
        match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::User { content, .. }
                | Message::Assistant { content, .. }
                | Message::ToolResult { content, .. } => content
                    .iter()
                    .find_map(|c| match c {
                        Content::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default(),
            },
            _ => String::new(),
        }
    }

    #[test]
    fn weave_braking_annotations_renders_marker_and_tags_into_content() {
        // A trunk node with a Lesson tag → content must carry BOTH the `[n7]`
        // marker (so the model can target it via revert step) AND the lesson
        // text (so "the next turn sees the lesson" — the tool's promise).
        let mut am = assistant("explored the npm path", 1, NodeId(7), None);
        if let AgentMessage::Llm(lm) = &mut am {
            lm.tags
                .push(tag(TagKind::Lesson, 0, "npm is denied; do not retry"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let text = first_text(&woven[0]);
        assert!(text.starts_with("[n7]"), "marker missing: {text}");
        assert!(
            text.contains("[lesson: npm is denied; do not retry]"),
            "lesson annotation missing: {text}"
        );
        assert!(
            text.contains("explored the npm path"),
            "original content lost: {text}"
        );
    }

    #[test]
    fn weave_braking_annotations_marks_untagged_nodes() {
        // No tags → still gets the `[n3]` marker (cold-start addressability).
        let am = user("state your name", 1, NodeId(3), None);
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let text = first_text(&woven[0]);
        assert_eq!(text, "[n3] state your name");
    }

    #[test]
    fn weave_braking_annotations_passes_through_nodeless_messages() {
        // A message with no node_id (non-revert metadata) is byte-identical.
        let am = AgentMessage::Llm(LlmMessage::new(Message::User {
            content: vec![Content::Text {
                text: "no node here".into(),
            }],
            timestamp: 1,
        }));
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        assert_eq!(first_text(&woven[0]), "no node here");
    }

    // ── dedup all-identical lessons + tip marker ──

    #[test]
    fn weave_braking_annotations_dedups_consecutive_identical_lessons() {
        // Repeated reverts to the same node stack the SAME lesson 6×. The
        // weave must render the run ONCE, not 6×.
        let mut am = assistant("re-read the task", 1, NodeId(0), None);
        if let AgentMessage::Llm(lm) = &mut am {
            for _ in 0..6 {
                lm.tags.push(tag(
                    TagKind::Lesson,
                    0,
                    "approach v1 was tangential, restarting",
                ));
            }
        }
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let text = first_text(&woven[0]);
        let occurrences = text
            .matches("[lesson: approach v1 was tangential, restarting]")
            .count();
        assert_eq!(
            occurrences, 1,
            "6 identical lessons must render once: {text}"
        );
    }

    #[test]
    fn dedups_all_identical_lessons_even_when_interleaved() {
        // A `finding` interleaved between two identical `lesson` tags
        // defeated the old consecutive-only dedup. The all-identical seen-set
        // renders the repeated lesson ONCE regardless of ordering, while the
        // distinct finding still renders.
        let mut am = assistant("explored", 1, NodeId(0), None);
        if let AgentMessage::Llm(lm) = &mut am {
            lm.tags.push(tag(TagKind::Lesson, 0, "npm denied"));
            lm.tags.push(tag(TagKind::Finding, 0, "registry is slow"));
            lm.tags.push(tag(TagKind::Lesson, 0, "npm denied")); // non-consecutive repeat
        }
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let text = first_text(&woven[0]);
        assert_eq!(
            text.matches("[lesson: npm denied]").count(),
            1,
            "interleaved identical lesson must render once: {text}"
        );
        assert!(
            text.contains("[finding: registry is slow]"),
            "the interleaved distinct finding must still render: {text}"
        );
    }

    #[test]
    fn weave_braking_annotations_keeps_distinct_lessons() {
        // Dedup is all-identical — distinct lessons must all still render.
        let mut am = assistant("explored two paths", 1, NodeId(0), None);
        if let AgentMessage::Llm(lm) = &mut am {
            lm.tags.push(tag(TagKind::Lesson, 0, "npm denied"));
            lm.tags.push(tag(TagKind::Lesson, 1, "yarn denied"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let text = first_text(&woven[0]);
        assert!(text.contains("[lesson: npm denied]"), "{text}");
        assert!(text.contains("[lesson: yarn denied]"), "{text}");
    }

    #[test]
    fn weave_braking_annotations_renders_tags_without_steering_message() {
        // The weave step renders markers + tags ONLY — it no longer folds the
        // continue-forward directive (that is now a separate, decaying synthetic
        // `[continue_after_revert]` user message emitted by
        // `inject_continue_after_revert`). The weave output length is unchanged
        // and carries no steering text.
        let root = user("write a plan", 1, NodeId(0), None);
        let mut tip = assistant("reverted here", 2, NodeId(1), Some(NodeId(0)));
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags
                .push(tag(TagKind::Lesson, 1, "v1 was tangential, restarting"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
        // No standalone note is inserted by the weave — length is unchanged.
        assert_eq!(
            woven.len(),
            2,
            "weave must not insert any standalone message"
        );
        // The tip keeps its marker + lesson tag, but carries NO steering text.
        let tip_text = first_text(&woven[1]);
        assert!(
            tip_text.starts_with("[n1]")
                && tip_text.contains("[lesson: v1 was tangential, restarting]"),
            "tip keeps its marker + lesson tag: {tip_text}"
        );
        assert!(
            !tip_text.contains("continue forward")
                && !tip_text.contains(CONTINUE_AFTER_REVERT_MARKER),
            "weave must NOT fold any continue-forward steering into the tip: {tip_text}"
        );
        // No `[continue_after_revert]` user message exists after the weave alone.
        assert!(
            !woven
                .iter()
                .any(|m| first_text(m).contains(CONTINUE_AFTER_REVERT_MARKER)),
            "weave alone emits no [continue_after_revert] message"
        );
    }

    #[test]
    fn continue_after_revert_emitted_for_all_categories_within_window() {
        // The decaying `[continue_after_revert]` synthetic user message is
        // emitted after the tip for ALL FOUR revert categories — failure→Lesson,
        // tangent→Finding, completion→Outcome, step-summary→Checkpoint — while
        // the tip's newest tag is within the decay window.
        for kind in [
            TagKind::Lesson,
            TagKind::Finding,
            TagKind::Outcome,
            TagKind::Checkpoint,
        ] {
            let root = user("write a plan", 1, NodeId(0), None);
            let mut tip = assistant("reverted here", 2, NodeId(1), Some(NodeId(0)));
            if let AgentMessage::Llm(lm) = &mut tip {
                lm.tags.push(tag(kind, 5, "v1 abandoned"));
            }
            let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
            // current_turn = 5, created_at_turn = 5, window = 3 → distance 0 ≤ 3.
            let out = AgentContext::inject_continue_after_revert(woven, 5, 3);
            // A new entry was inserted right after the tip.
            assert_eq!(
                out.len(),
                3,
                "{kind:?}: the steering message must be inserted after the tip"
            );
            // The inserted message is a nodeless Message::User immediately after
            // the tip (index 2 — root at 0, tip at 1, steering at 2).
            let note = &out[2];
            assert!(
                matches!(
                    note,
                    AgentMessage::Llm(lm) if lm.node_id.is_none()
                        && matches!(&lm.message, Message::User { .. })
                ),
                "{kind:?}: the steering message must be a nodeless Message::User"
            );
            // Its text starts with the marker, echoes the breadcrumb, and carries
            // the continue-forward directive.
            let text = first_text(note);
            assert!(
                text.starts_with(CONTINUE_AFTER_REVERT_MARKER),
                "{kind:?}: text must start with the marker: {text}"
            );
            assert!(
                text.contains("v1 abandoned")
                    && text.contains("Continue forward")
                    && text.contains("next uncompleted step")
                    && text.contains("do NOT redo completed steps")
                    && text.contains("do NOT stop"),
                "{kind:?}: text must echo the breadcrumb + carry the directive: {text}"
            );
        }
    }

    #[test]
    fn continue_after_revert_suppressed_past_decay_window_all_categories() {
        // Past the decay window the `[continue_after_revert]` message is NOT
        // emitted — for ANY category, INCLUDING pinned Outcome/Checkpoint whose
        // TAG itself persists on the trunk (the steering nudge is transient even
        // though the pinned tag is not).
        for kind in [
            TagKind::Lesson,
            TagKind::Finding,
            TagKind::Outcome,
            TagKind::Checkpoint,
        ] {
            let root = user("write a plan", 1, NodeId(0), None);
            let mut tip = assistant("reverted here", 2, NodeId(1), Some(NodeId(0)));
            if let AgentMessage::Llm(lm) = &mut tip {
                lm.tags.push(tag(kind, 5, "v1 abandoned"));
            }
            let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
            // current_turn = 9, created_at_turn = 5, window = 3 → distance 4 > 3.
            let out = AgentContext::inject_continue_after_revert(woven, 9, 3);
            assert_eq!(
                out.len(),
                2,
                "{kind:?}: no steering message past the decay window"
            );
            assert!(
                !out.iter()
                    .any(|m| first_text(m).contains(CONTINUE_AFTER_REVERT_MARKER)),
                "{kind:?}: no [continue_after_revert] message past the window"
            );
        }
    }

    #[test]
    fn continue_after_revert_decays_at_exact_window_boundary() {
        // Boundary: distance == window renders (within); distance == window + 1
        // does not. Mirrors `renders_by_turn`'s `<=` semantics.
        let make = || {
            let mut tip = assistant("reverted here", 2, NodeId(1), None);
            if let AgentMessage::Llm(lm) = &mut tip {
                lm.tags.push(tag(TagKind::Lesson, 10, "abandoned"));
            }
            AgentContext::weave_braking_annotations(vec![tip])
        };
        // distance 3 == window 3 → emitted.
        let at = AgentContext::inject_continue_after_revert(make(), 13, 3);
        assert_eq!(at.len(), 2, "exactly at window must still emit");
        // distance 4 > window 3 → suppressed.
        let past = AgentContext::inject_continue_after_revert(make(), 14, 3);
        assert_eq!(past.len(), 1, "just past window must suppress");
    }

    #[test]
    fn continue_after_revert_newest_tag_governs_decay_and_breadcrumb() {
        // A node accruing multiple revert tags uses the NEWEST (highest
        // created_at_turn) to gate decay AND to supply the breadcrumb echo.
        let mut tip = assistant("reverted here", 2, NodeId(1), None);
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags.push(tag(TagKind::Lesson, 2, "old breadcrumb"));
            lm.tags.push(tag(TagKind::Lesson, 8, "newest breadcrumb"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![tip]);
        // current_turn = 9, newest created_at_turn = 8, window = 3 → distance 1.
        let out = AgentContext::inject_continue_after_revert(woven, 9, 3);
        assert_eq!(out.len(), 2, "in-window via the newest tag → emit");
        let text = first_text(&out[1]);
        assert!(
            text.contains("newest breadcrumb") && !text.contains("old breadcrumb"),
            "the breadcrumb echo must use the newest tag: {text}"
        );
    }

    #[test]
    fn continue_after_revert_emitted_after_tool_result_tip() {
        // When the trunk tip is a `Message::ToolResult` carrying a revert tag,
        // the steering message is still emitted immediately after it.
        let root = user("write a plan", 1, NodeId(0), None);
        let mut tip = tool_result("file written", 2, NodeId(1), Some(NodeId(0)));
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags
                .push(tag(TagKind::Lesson, 2, "v1 was tangential, restarting"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
        let out = AgentContext::inject_continue_after_revert(woven, 2, 3);
        assert_eq!(
            out.len(),
            3,
            "steering message inserted after the tool-result tip"
        );
        // Steering message is at index 2 (right after the tip at index 1).
        let text = first_text(&out[2]);
        assert!(
            text.starts_with(CONTINUE_AFTER_REVERT_MARKER)
                && text.contains("v1 was tangential, restarting"),
            "the steering message follows the tool-result tip: {text}"
        );
        // The tool-result tip itself keeps its woven marker + content untouched.
        let tip_text = first_text(&out[1]);
        assert!(
            tip_text.starts_with("[n1]") && tip_text.contains("file written"),
            "tool-result tip keeps its marker + content: {tip_text}"
        );
    }

    #[test]
    fn continue_after_revert_no_message_without_revert_tag() {
        // A tip node with NO revert tag is not a fresh-revert tip → no steering
        // message (avoids spamming every trunk build).
        let am = assistant("ordinary tip", 1, NodeId(5), None);
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        let out = AgentContext::inject_continue_after_revert(woven, 1, 3);
        assert_eq!(out.len(), 1, "no steering without a revert tag on the tip");
        assert!(
            !out.iter()
                .any(|m| first_text(m).contains(CONTINUE_AFTER_REVERT_MARKER)),
            "no [continue_after_revert] message without a revert tag"
        );
    }
}

#[cfg(test)]
mod collapse_abandon_class_cluster_tests {
    //! 0.11 — render-time reclamation of the abandon-class tool-cluster a revert
    //! lands on. Locks in: (a) abandon-class collapses the heavy ToolCall body
    //! into the breadcrumb (the body is gone from the rendered trunk; the
    //! breadcrumb tag survives); (b) pinned keeps the cluster WHOLE (re-includes
    //! the matching tool-result so the kept call never dangles); (c)
    //! `messages` is byte-identical before/after (forensic log immutable);
    //! (d) multi-turn abandon-to-ancestor: the whole off-trunk tail is excluded
    //! and the breadcrumb names the abandoned work.
    use super::super::agent_message::LlmMessage;
    use super::super::content::{Content, Message, StopReason};
    use super::super::node_tag::{NodeTag, TagKind};
    use super::super::usage::Usage;
    use super::*;

    /// An assistant node carrying a leading reasoning `Text` block PLUS a single
    /// heavy `write_file` ToolCall. The reasoning text models the CC-17 bug
    /// shape: the live wire carried "I'll follow your plan sequentially.
    /// Starting with Step 1…" alongside the call, and the CC-17 partial-strip
    /// (ToolCall-only) left that text behind, driving weak-model looping
    /// (CC-17 close-gate-results.md:29). CC-18 replaces the WHOLE content.
    fn tool_call_node(
        call_id: &str,
        heavy_args: &str,
        ts: u64,
        node: NodeId,
        parent: Option<NodeId>,
    ) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![
                    Content::Text {
                        text: "I'll follow your plan sequentially. Starting with Step 1…"
                            .to_string(),
                    },
                    Content::ToolCall {
                        id: call_id.to_string(),
                        name: "write_file".to_string(),
                        arguments: serde_json::json!({ "path": "plan-v1.md", "content": heavy_args }),
                    },
                ],
                stop_reason: StopReason::ToolUse,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: ts,
                error_message: None,
            })
            .with_node_identity(node, parent),
        )
    }

    /// A tool-result node matching `call_id`.
    fn result_node(call_id: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::ToolResult {
                tool_call_id: call_id.to_string(),
                tool_name: "write_file".to_string(),
                content: vec![Content::Text {
                    text: "Wrote 356 bytes".to_string(),
                }],
                is_error: false,
                timestamp: ts,
            })
            .with_node_identity(node, parent),
        )
    }

    fn user_node(text: &str, ts: u64, node: NodeId, parent: Option<NodeId>) -> AgentMessage {
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

    /// Attach a NodeTag of `kind` carrying `breadcrumb` to the message at `idx`.
    fn tag_message(msgs: &mut [AgentMessage], idx: usize, kind: TagKind, breadcrumb: &str) {
        if let AgentMessage::Llm(lm) = &mut msgs[idx] {
            lm.add_tag(NodeTag::new(kind, breadcrumb.to_string(), 1, vec![]));
        }
    }

    use super::super::node_tag::RevertRenderPolicy;

    /// The default render policy, evaluated at a turn where the fixtures' tags
    /// (`created_at_turn = 1`) are still IN the decay window — so the collapse
    /// produces the breadcrumb (not the out-of-window decay-drop). CC-18 iter-2
    /// signature: `collapse_abandon_class_cluster(trunk, policy, current_turn)`.
    const IN_WINDOW_TURN: u32 = 1;
    fn in_window_policy() -> RevertRenderPolicy {
        RevertRenderPolicy::default()
    }

    /// First text block of a message (empty string if none).
    fn first_text(m: &AgentMessage) -> String {
        match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::User { content, .. }
                | Message::Assistant { content, .. }
                | Message::ToolResult { content, .. } => content
                    .iter()
                    .find_map(|c| match c {
                        Content::Text { text } => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_default(),
            },
            _ => String::new(),
        }
    }

    /// Does the rendered trunk contain the heavy `write_file` arguments anywhere?
    fn trunk_has_heavy_args(trunk: &[AgentMessage], needle: &str) -> bool {
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::Assistant { content, .. } => content.iter().any(|b| match b {
                    Content::ToolCall { arguments, .. } => arguments.to_string().contains(needle),
                    _ => false,
                }),
                _ => false,
            },
            _ => false,
        })
    }

    fn trunk_has_breadcrumb(trunk: &[AgentMessage], needle: &str) -> bool {
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => lm.tags.iter().any(|t| t.text.contains(needle)),
            _ => false,
        })
    }

    /// Does ANY assistant node in the trunk still carry a non-blank `Content::
    /// Text` block? CC-18: after a `failure`/`tangent` collapse the surviving
    /// n0 must carry NO reasoning text — only the breadcrumb (woven downstream).
    fn trunk_has_surviving_assistant_text(trunk: &[AgentMessage], needle: &str) -> bool {
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::Assistant { content, .. } => content.iter().any(|b| match b {
                    Content::Text { text } => text.contains(needle),
                    _ => false,
                }),
                _ => false,
            },
            _ => false,
        })
    }

    /// Does the trunk carry ANY assistant `Content::ToolCall` at all?
    fn trunk_has_any_tool_call(trunk: &[AgentMessage]) -> bool {
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::Assistant { content, .. } => content
                    .iter()
                    .any(|b| matches!(b, Content::ToolCall { .. })),
                _ => false,
            },
            _ => false,
        })
    }

    fn trunk_has_dangling_call(trunk: &[AgentMessage]) -> bool {
        // A tool-call whose matching result is NOT present anywhere in the trunk.
        let result_ids: std::collections::HashSet<&str> = trunk
            .iter()
            .filter_map(|m| match m {
                AgentMessage::Llm(lm) => match &lm.message {
                    Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.as_str()),
                    _ => None,
                },
                _ => None,
            })
            .collect();
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => match &lm.message {
                Message::Assistant { content, .. } => content.iter().any(|b| match b {
                    Content::ToolCall { id, .. } => !result_ids.contains(id.as_str()),
                    _ => false,
                }),
                _ => false,
            },
            _ => false,
        })
    }

    /// The #59 shape: user task (n0) → assistant write_file call (n1) → result
    /// (n2). `revert_to_state(failure, step="1")` targets the CALL node n1; the
    /// result n2 is its child, dropped off-trunk by the parent-chain walk.
    fn fixture_59_shape() -> AgentContext {
        AgentContext {
            messages: vec![
                user_node("write a plan", 1, NodeId(0), None),
                tool_call_node(
                    "call_abc",
                    "PLAN-V1-HEAVY-BODY",
                    2,
                    NodeId(1),
                    Some(NodeId(0)),
                ),
                result_node("call_abc", 3, NodeId(2), Some(NodeId(1))),
            ],
            next_node_id: 3,
            active_node_id: Some(NodeId(1)), // reverted onto the call node
            ..Default::default()
        }
    }

    #[test]
    fn abandon_class_collapses_heavy_cluster_to_breadcrumb() {
        let mut ctx = fixture_59_shape();
        // apply_revert would have attached an abandon-class (Lesson) breadcrumb
        // on the target call node naming the abandoned work.
        tag_message(
            &mut ctx.messages,
            1,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        // Trunk = parent-chain from n1 = [n0, n1]; n2 (result) is off-trunk.
        let trunk = ctx.build_trunk_context();
        assert!(
            trunk_has_heavy_args(&trunk, "PLAN-V1-HEAVY-BODY"),
            "precondition: the kept tip still carries the heavy body before collapse"
        );

        // Precondition: the kept tip ALSO carries the misleading reasoning text
        // before collapse (the CC-17 bug shape).
        assert!(
            trunk_has_surviving_assistant_text(&trunk, "Starting with Step 1"),
            "precondition: the kept tip carries the reasoning text before collapse"
        );

        let collapsed =
            ctx.collapse_abandon_class_cluster(trunk, &in_window_policy(), IN_WINDOW_TURN);

        // (a) the heavy ToolCall body is GONE from the rendered trunk.
        assert!(
            !trunk_has_heavy_args(&collapsed, "PLAN-V1-HEAVY-BODY"),
            "abandon-class collapse must strip the heavy tool-call body"
        );
        // (a') CC-18 — the surviving n0 carries NO reasoning text either. The
        // CC-17 partial-strip left "Starting with Step 1…" behind, driving the
        // weak-model loop; CC-18 replaces the ENTIRE content.
        assert!(
            !trunk_has_surviving_assistant_text(&collapsed, "Starting with Step 1"),
            "abandon-class collapse must strip the surviving n0 reasoning text (CC-18)"
        );
        // (a'') no tool_call args of any kind survive on the trunk.
        assert!(
            !trunk_has_any_tool_call(&collapsed),
            "abandon-class collapse must leave no tool_call on the trunk"
        );
        // (b) the breadcrumb tag survives (woven into content downstream).
        assert!(
            trunk_has_breadcrumb(&collapsed, "write_file abandoned"),
            "the breadcrumb naming the abandoned work must survive"
        );
        // (c) no dangling tool-call (the call was stripped, not left orphaned).
        assert!(
            !trunk_has_dangling_call(&collapsed),
            "abandon-class collapse must not leave a dangling tool-call"
        );
    }

    /// The #59-canonical (minimax) shape: user task (n0) → assistant write_file
    /// CALL (n1, heavy) → write_file RESULT (n2). `revert_to_state(failure,
    /// step="n2")` targets the tool-RESULT node n2; n1 (its parent call) stays
    /// on-trunk and carries the heavy plan-v1 body.
    fn fixture_59_tool_result_tip() -> AgentContext {
        AgentContext {
            messages: vec![
                user_node("write a plan", 1, NodeId(0), None),
                tool_call_node(
                    "call_abc",
                    "PLAN-V1-HEAVY-BODY",
                    2,
                    NodeId(1),
                    Some(NodeId(0)),
                ),
                result_node("call_abc", 3, NodeId(2), Some(NodeId(1))),
            ],
            next_node_id: 3,
            active_node_id: Some(NodeId(2)), // reverted onto the tool-RESULT node
            ..Default::default()
        }
    }

    /// Does the rendered trunk contain ANY tool-result for `call_id`?
    fn trunk_has_tool_result(trunk: &[AgentMessage], call_id: &str) -> bool {
        trunk.iter().any(|m| match m {
            AgentMessage::Llm(lm) => matches!(
                &lm.message,
                Message::ToolResult { tool_call_id, .. } if tool_call_id == call_id
            ),
            _ => false,
        })
    }

    #[test]
    fn abandon_class_collapses_when_tip_is_tool_result_collapses_whole_cluster() {
        // The #59-canonical shape the live close-gate caught: the model reverts
        // onto the tool-RESULT node (minimax `step="n1"`). The trunk tip is a
        // `Message::ToolResult`; the heavy body lives in its PARENT
        // assistant-call node (still on-trunk). The WHOLE cluster must collapse
        // atomically — the previous fix stripped the parent call but KEPT the
        // tool-result tip, leaving an ORPHANED result (a tool_call_id with no
        // matching call → OpenAI-compat providers reject it).
        let mut ctx = fixture_59_tool_result_tip();
        // apply_revert attaches the abandon-class (Lesson) breadcrumb on the
        // target tool-result node naming the abandoned write_file work.
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        // Trunk = parent-chain from n2 = [n0, n1, n2]; the parent call n1 carries
        // the heavy body, the tip n2 is the tool-result.
        let trunk = ctx.build_trunk_context();
        assert!(
            trunk_has_heavy_args(&trunk, "PLAN-V1-HEAVY-BODY"),
            "precondition: the parent call still carries the heavy body before collapse"
        );
        assert!(
            trunk_has_tool_result(&trunk, "call_abc"),
            "precondition: the tool-result tip is on the trunk before collapse"
        );

        let before = serde_json::to_string(&ctx.messages).unwrap();
        let collapsed =
            ctx.collapse_abandon_class_cluster(trunk, &in_window_policy(), IN_WINDOW_TURN);

        // (a) the parent call's heavy ToolCall args are GONE.
        assert!(
            !trunk_has_heavy_args(&collapsed, "PLAN-V1-HEAVY-BODY"),
            "tool-result-tip collapse must strip the parent call's heavy body"
        );
        // (a') CC-18 — the surviving parent assistant node carries NO reasoning
        // text either (the whole {n0,n1} cluster content is reclaimed).
        assert!(
            !trunk_has_surviving_assistant_text(&collapsed, "Starting with Step 1"),
            "tool-result-tip collapse must strip the parent's reasoning text (CC-18)"
        );
        assert!(
            !trunk_has_any_tool_call(&collapsed),
            "tool-result-tip collapse must leave no tool_call on the trunk"
        );
        // (b) there is NO Message::ToolResult left for this cluster — the
        // tool-result tip is removed, so no orphaned result survives.
        assert!(
            !trunk_has_tool_result(&collapsed, "call_abc"),
            "the tool-result tip must be removed — no orphaned result"
        );
        // (c) the breadcrumb tag renders on the SURVIVING assistant node (moved
        // from the removed tool-result tip onto the parent call node).
        assert!(
            trunk_has_breadcrumb(&collapsed, "write_file abandoned"),
            "the breadcrumb must survive on the surviving assistant node"
        );
        let breadcrumb_on_assistant = collapsed.iter().any(|m| match m {
            AgentMessage::Llm(lm) => {
                matches!(&lm.message, Message::Assistant { .. })
                    && lm
                        .tags
                        .iter()
                        .any(|t| t.text.contains("write_file abandoned"))
            }
            _ => false,
        });
        assert!(
            breadcrumb_on_assistant,
            "the breadcrumb tag must live on the surviving assistant node"
        );
        // (d) no dangling tool-call AND no orphaned tool-result.
        assert!(
            !trunk_has_dangling_call(&collapsed),
            "collapse must not leave a dangling tool-call"
        );
        // (e) self.messages byte-identical (collapse is render-only).
        let after = serde_json::to_string(&ctx.messages).unwrap();
        assert_eq!(before, after, "collapse must never mutate context.messages");
    }

    #[test]
    fn pinned_class_keeps_cluster_whole_no_dangling_call() {
        let mut ctx = fixture_59_shape();
        // A completion (pinned/Outcome) revert onto the same call node.
        tag_message(
            &mut ctx.messages,
            1,
            TagKind::Outcome,
            "reverted past: sub-task sealed (write_file)",
        );
        let trunk = ctx.build_trunk_context(); // = [n0, n1]; result n2 off-trunk
        assert!(
            trunk_has_dangling_call(&trunk),
            "precondition: before collapse the kept call dangles (its result is off-trunk)"
        );

        let collapsed =
            ctx.collapse_abandon_class_cluster(trunk, &in_window_policy(), IN_WINDOW_TURN);

        // Pinned keeps the heavy body whole...
        assert!(
            trunk_has_heavy_args(&collapsed, "PLAN-V1-HEAVY-BODY"),
            "pinned category keeps the sealed tool-call body whole"
        );
        // ...AND re-includes the matching result so the call never dangles.
        assert!(
            !trunk_has_dangling_call(&collapsed),
            "pinned keep-whole must re-include the matching tool-result"
        );
        let has_result = collapsed.iter().any(|m| {
            matches!(
                m,
                AgentMessage::Llm(lm) if matches!(
                    &lm.message,
                    Message::ToolResult { tool_call_id, .. } if tool_call_id == "call_abc"
                )
            )
        });
        assert!(
            has_result,
            "the matching tool-result must be present on the trunk"
        );
    }

    #[test]
    fn collapse_keeps_messages_byte_identical() {
        let mut ctx = fixture_59_shape();
        tag_message(
            &mut ctx.messages,
            1,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        // Snapshot the forensic log via its serialized form (the canonical
        // byte-identity check — collapse is render-only over the cloned trunk).
        let before = serde_json::to_string(&ctx.messages).unwrap();
        let trunk = ctx.build_trunk_context();
        let _collapsed =
            ctx.collapse_abandon_class_cluster(trunk, &in_window_policy(), IN_WINDOW_TURN);
        let after = serde_json::to_string(&ctx.messages).unwrap();
        assert_eq!(before, after, "collapse must never mutate context.messages");
    }

    #[test]
    fn multi_turn_abandon_to_ancestor_excludes_tail_and_names_work() {
        // n0 user → n1 read_file CALL → n2 read result → n3 grep CALL →
        // n4 grep result, all on one chain; revert (tangent) to n1's RESULT
        // (n2) drops the n3/n4 lexer detour. The parent-chain walk excludes the
        // whole tail; the breadcrumb (composed by apply_revert from the
        // strictly-after span) names read_file/grep.
        let mut ctx = AgentContext {
            messages: vec![
                user_node("fix the parser", 1, NodeId(0), None),
                tool_call_node("c1", "PARSER-SRC", 2, NodeId(1), Some(NodeId(0))),
                result_node("c1", 3, NodeId(2), Some(NodeId(1))),
                tool_call_node("c2", "LEXER-SRC", 4, NodeId(3), Some(NodeId(2))),
                result_node("c2", 5, NodeId(4), Some(NodeId(3))),
            ],
            next_node_id: 5,
            active_node_id: Some(NodeId(2)), // reverted to the parser READ result
            ..Default::default()
        };
        // The tip n2 is a tool-result (not a call node) carrying the breadcrumb.
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Finding,
            "reverted past: bug is in parser (read_file, grep abandoned)",
        );
        let trunk = ctx.build_trunk_context();
        let collapsed =
            ctx.collapse_abandon_class_cluster(trunk, &in_window_policy(), IN_WINDOW_TURN);

        // The whole off-trunk tail (lexer detour) is absent.
        assert!(
            !trunk_has_heavy_args(&collapsed, "LEXER-SRC"),
            "the abandoned lexer detour must be off-trunk"
        );
        // The breadcrumb naming the abandoned work survives. The tip n2 is a
        // tool-result carrying an abandon-class breadcrumb → the c1 cluster
        // (parent call n1 + result tip n2) collapses atomically.
        assert!(
            trunk_has_breadcrumb(&collapsed, "read_file, grep abandoned"),
            "the breadcrumb must name all abandoned tools across the span"
        );
        // The c1 tool-result tip is removed (no orphaned result) and its tag
        // moves onto the surviving assistant call node n1 (whose ToolCall body
        // is stripped). The parser cluster collapses to a single clean node.
        assert!(
            !trunk_has_tool_result(&collapsed, "c1"),
            "the c1 tool-result tip must be removed — no orphaned result"
        );
        assert!(
            !trunk_has_heavy_args(&collapsed, "PARSER-SRC"),
            "the parent call's heavy body must be stripped"
        );
        assert!(
            !trunk_has_dangling_call(&collapsed),
            "the atomic collapse must leave no dangling tool-call"
        );
        let breadcrumb_on_assistant = collapsed.iter().any(|m| match m {
            AgentMessage::Llm(lm) => {
                matches!(&lm.message, Message::Assistant { .. })
                    && lm
                        .tags
                        .iter()
                        .any(|t| t.text.contains("read_file, grep abandoned"))
            }
            _ => false,
        });
        assert!(
            breadcrumb_on_assistant,
            "the breadcrumb tag must live on the surviving assistant node"
        );
    }

    #[test]
    fn end_to_end_collapse_then_weave_minimax_tool_result_tip() {
        // The minimax shape end-to-end (collapse → weave → inject): the model
        // reverts onto a tool-RESULT tip carrying an abandon-class Lesson
        // breadcrumb. After the atomic collapse + weave + inject, the rendered
        // output is a single surviving assistant node carrying `[lesson: …
        // write_file …]` (NO `Message::ToolResult`) FOLLOWED BY a separate
        // decaying `[continue_after_revert]` synthetic user message.
        let mut ctx = fixture_59_tool_result_tip();
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        let policy = super::super::node_tag::RevertRenderPolicy::default();
        // CC-18 iter-2 render-path order (mirrors streaming.rs): collapse FIRST
        // on the raw trunk, then decay tags on the already-collapsed trunk.
        let raw = ctx.build_trunk_context();
        let collapsed = ctx.collapse_abandon_class_cluster(raw, &policy, 1);
        let trunk = AgentContext::decay_tags_by_policy(collapsed, &policy, 1);
        let woven = AgentContext::weave_braking_annotations(trunk);
        // current_turn = 1, tag created_at_turn = 1, window = 3 → within window.
        let out = AgentContext::inject_continue_after_revert(woven, 1, policy.lesson_window_turns);

        // No ToolResult anywhere — the cluster collapsed atomically.
        assert!(
            !out.iter().any(|m| matches!(
                m,
                AgentMessage::Llm(lm) if matches!(&lm.message, Message::ToolResult { .. })
            )),
            "no Message::ToolResult must survive the atomic collapse"
        );
        // The surviving assistant node carries the breadcrumb in its content but
        // NOT the steering directive (that is now a separate message).
        fn assistant_text(m: &AgentMessage) -> Option<String> {
            match m {
                AgentMessage::Llm(lm) => match &lm.message {
                    Message::Assistant { content, .. } => content.iter().find_map(|c| match c {
                        Content::Text { text } => Some(text.clone()),
                        _ => None,
                    }),
                    _ => None,
                },
                _ => None,
            }
        }
        let tip_text = out
            .iter()
            .find_map(assistant_text)
            .expect("a surviving assistant node must carry woven content");
        assert!(
            tip_text.contains("[lesson:") && tip_text.contains("write_file abandoned"),
            "the surviving assistant node must carry the lesson breadcrumb: {tip_text}"
        );
        assert!(
            !tip_text.contains("continue forward")
                && !tip_text.contains(CONTINUE_AFTER_REVERT_MARKER),
            "the steering directive must NOT be folded into the assistant node: {tip_text}"
        );
        // And the heavy body is gone.
        assert!(
            !tip_text.contains("PLAN-V1-HEAVY-BODY"),
            "the heavy tool-call body must be gone: {tip_text}"
        );
        // CC-18 — and the misleading reasoning text is gone too (this is the
        // residual the CC-17 close-gate flagged at :29; the weak model re-read
        // "Starting with Step 1…" and re-executed it).
        assert!(
            !tip_text.contains("Starting with Step 1"),
            "the surviving n0 must carry NO reasoning text (CC-18): {tip_text}"
        );
        // A separate `[continue_after_revert]` synthetic user message follows the
        // tip, echoing the breadcrumb + carrying the continue-forward directive.
        let steer = out
            .iter()
            .find(|m| first_text(m).contains(CONTINUE_AFTER_REVERT_MARKER))
            .expect("a [continue_after_revert] steering message must be emitted");
        assert!(
            matches!(
                steer,
                AgentMessage::Llm(lm) if lm.node_id.is_none()
                    && matches!(&lm.message, Message::User { .. })
            ),
            "the steering message must be a nodeless Message::User"
        );
        let steer_text = first_text(steer);
        assert!(
            steer_text.starts_with(CONTINUE_AFTER_REVERT_MARKER)
                && steer_text.contains("write_file abandoned")
                && steer_text.contains("next uncompleted step")
                && steer_text.contains("do NOT stop"),
            "the steering message must carry the marker + breadcrumb + directive: {steer_text}"
        );
    }

    /// CC-18 §6 Tier A (e) — ATOMICITY: naming `step="n0"` (the assistant CALL
    /// node) vs `step="n1"` (its tool-RESULT node) points at the SAME indivisible
    /// {n0,n1} cluster, so the collapse disposition is identical. Both yield a
    /// single surviving clean assistant node carrying the breadcrumb, with NO
    /// tool_call, NO surviving reasoning text, and NO tool_result.
    #[test]
    fn collapse_atomicity_step_n0_and_n1_same_cluster_disposition() {
        // A reusable summariser of "what the collapsed trunk looks like" as a
        // disposition tuple: (has_tool_call, has_surviving_text, has_result,
        // has_breadcrumb).
        fn disposition(collapsed: &[AgentMessage]) -> (bool, bool, bool, bool) {
            (
                trunk_has_any_tool_call(collapsed),
                trunk_has_surviving_assistant_text(collapsed, "Starting with Step 1"),
                trunk_has_tool_result(collapsed, "call_abc"),
                trunk_has_breadcrumb(collapsed, "write_file abandoned"),
            )
        }

        // step="n0": revert onto the CALL node (call-tip shape).
        let mut ctx_n0 = fixture_59_shape();
        tag_message(
            &mut ctx_n0.messages,
            1,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        let trunk_n0 = ctx_n0.build_trunk_context();
        let collapsed_n0 =
            ctx_n0.collapse_abandon_class_cluster(trunk_n0, &in_window_policy(), IN_WINDOW_TURN);

        // step="n1": revert onto the tool-RESULT node (result-tip shape).
        let mut ctx_n1 = fixture_59_tool_result_tip();
        tag_message(
            &mut ctx_n1.messages,
            2,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        let trunk_n1 = ctx_n1.build_trunk_context();
        let collapsed_n1 =
            ctx_n1.collapse_abandon_class_cluster(trunk_n1, &in_window_policy(), IN_WINDOW_TURN);

        // Both dispositions are identical: no tool_call, no surviving text, no
        // tool_result, breadcrumb present.
        let d0 = disposition(&collapsed_n0);
        let d1 = disposition(&collapsed_n1);
        assert_eq!(
            d0, d1,
            "step=n0 and step=n1 must yield the SAME cluster disposition (atomicity)"
        );
        assert_eq!(
            d0,
            (false, false, false, true),
            "the atomic collapse must leave only the breadcrumb"
        );
        // And neither leaves a dangling tool-call.
        assert!(!trunk_has_dangling_call(&collapsed_n0));
        assert!(!trunk_has_dangling_call(&collapsed_n1));
    }

    /// The full production render path used by `streaming.rs`: collapse FIRST on
    /// the raw trunk (policy-aware), THEN decay tags. Mirrors
    /// `streaming.rs:build_trunk_context → collapse_abandon_class_cluster →
    /// decay_tags_by_policy`. This is the REAL pipeline the close-gate exercises.
    fn render_pipeline(
        ctx: &AgentContext,
        policy: &super::super::node_tag::RevertRenderPolicy,
        current_turn: u32,
    ) -> Vec<AgentMessage> {
        let raw = ctx.build_trunk_context();
        let collapsed = ctx.collapse_abandon_class_cluster(raw, policy, current_turn);
        AgentContext::decay_tags_by_policy(collapsed, policy, current_turn)
    }

    /// CC-18 iter-2 F-collapsed-node-decay.a — REACHABLE decay-drop (REAL, not a
    /// hand-built lone-breadcrumb fixture). A real heavy `(call, result)` cluster
    /// is reverted (abandon-class tag) and the model continues forward; the
    /// PRODUCTION render pipeline (collapse-first, policy-aware) collapses the
    /// heavy body to the breadcrumb WHILE the lesson is in-window, and DROPS the
    /// now-content-less node entirely once the lesson is out-of-window (GitHub
    /// #72 v2 principle 4 + #73 §2.3 — the iter-1 decay-drop was unreachable
    /// because it ran before the collapse and read the immutable heavy
    /// `messages`; moving it INTO the policy-aware collapse makes it fire).
    #[test]
    fn reachable_decay_drop_via_real_collapse_pipeline() {
        use super::super::node_tag::RevertRenderPolicy;
        // A real #59-shape heavy cluster: user → write_file CALL (heavy) → result.
        // The tag is attached to the CALL node (n1) at turn 1, exactly as
        // apply_revert would after a `failure` revert onto the abandoned write.
        let mut ctx = AgentContext {
            messages: vec![
                user_node("write a plan", 1, NodeId(0), None),
                tool_call_node(
                    "call_x",
                    "PLAN-V1-HEAVY-BODY",
                    2,
                    NodeId(1),
                    Some(NodeId(0)),
                ),
                result_node("call_x", 3, NodeId(2), Some(NodeId(1))),
            ],
            next_node_id: 3,
            active_node_id: Some(NodeId(1)), // reverted onto the call node
            ..Default::default()
        };
        tag_message(
            &mut ctx.messages,
            1,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );

        // Window 3, count-cap 0 so the lone lesson genuinely decays out past the
        // turn window (the default count-cap of 3 would force-keep it forever).
        let policy = RevertRenderPolicy {
            lesson_window_turns: 3,
            lesson_window_count: 0,
        };

        // (a) IN-window (turn 1, distance 0 ≤ 3): the heavy body is collapsed to
        // the breadcrumb, the node STAYS, and the heavy args are GONE.
        let in_window = render_pipeline(&ctx, &policy, 1);
        let has_n1_in_window = in_window
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))));
        assert!(
            has_n1_in_window,
            "in-window: the collapsed node renders (breadcrumb live)"
        );
        assert!(
            !trunk_has_heavy_args(&in_window, "PLAN-V1-HEAVY-BODY"),
            "in-window: the heavy body is reclaimed (collapsed to breadcrumb)"
        );

        // (b) OUT-of-window (turn 10, distance 9 > 3): the cluster is collapsed
        // AND the now-content-less node is DROPPED from the trunk entirely. The
        // user root (real content) survives. NO heavy body anywhere.
        let decayed = render_pipeline(&ctx, &policy, 10);
        let has_n1_decayed = decayed
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))));
        assert!(
            !has_n1_decayed,
            "out-of-window: the collapsed lone-breadcrumb node is dropped (reachable decay-drop)"
        );
        assert!(
            !trunk_has_heavy_args(&decayed, "PLAN-V1-HEAVY-BODY"),
            "out-of-window: the heavy body never re-appears"
        );
        let has_root = decayed
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(0))));
        assert!(has_root, "the user root (real content) is never dropped");
        // The reverted cluster's tool-result was removed by the result-tip
        // collapse path? Here the tag is on the CALL node so the result n2 is
        // off-trunk anyway (parent-chain from n1 = [n0, n1]); no orphan.
        assert!(
            !trunk_has_dangling_call(&decayed),
            "no dangling call survives the decay-drop"
        );
    }

    /// CC-18 iter-2 #73 CORE ASSERTION — PERSISTENT masking across turns. After a
    /// revert the model continues forward for several more turns; the abandoned
    /// heavy body must be GONE from the collapsed trunk on EVERY render, not just
    /// the one turn right after the revert (the iter-1 tip-only gap). The reverted
    /// cluster slides mid-trunk as new nodes are appended — the persistent scan
    /// must still find + mask it.
    #[test]
    fn persistent_masking_across_turns_mid_trunk_cluster() {
        use super::super::node_tag::RevertRenderPolicy;
        // Turn 1-2: user → heavy write_file CALL (n1) → result (n2), reverted
        // (failure) onto n2. Turns 3-5 (continue forward): the model writes
        // plan-v2 (n3 call + n4 result) then a notes file (n5 call + n6 result).
        // The active tip is n6, so the reverted n1/n2 cluster is now MID-trunk.
        let mut ctx = AgentContext {
            messages: vec![
                user_node("write a plan", 1, NodeId(0), None),
                tool_call_node("c1", "PLAN-V1-HEAVY-BODY", 2, NodeId(1), Some(NodeId(0))),
                result_node("c1", 3, NodeId(2), Some(NodeId(1))),
                tool_call_node("c2", "PLAN-V2-BODY", 4, NodeId(3), Some(NodeId(2))),
                result_node("c2", 5, NodeId(4), Some(NodeId(3))),
                tool_call_node("c3", "NOTES-BODY", 6, NodeId(5), Some(NodeId(4))),
                result_node("c3", 7, NodeId(6), Some(NodeId(5))),
            ],
            next_node_id: 7,
            active_node_id: Some(NodeId(6)), // tip is the LATEST node — cluster mid-trunk
            ..Default::default()
        };
        // The abandon-class (Lesson) tag landed on the RESULT node n2 at turn 1
        // (the #59 result-tip shape), exactly as apply_revert attaches it.
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );

        let policy = RevertRenderPolicy::default();

        // Render at SEVERAL subsequent turns (the model kept going). On EVERY one
        // the abandoned plan-v1 heavy body must be absent — this is the #73 core
        // assertion (iter-1 only masked on turn 1, the tip-render).
        for turn in [1u32, 2, 3] {
            let rendered = render_pipeline(&ctx, &policy, turn);
            assert!(
                !trunk_has_heavy_args(&rendered, "PLAN-V1-HEAVY-BODY"),
                "turn {turn}: the abandoned plan-v1 heavy body must stay masked (mid-trunk, persistent)"
            );
            // The non-reverted plan-v2 + notes bodies are NOT abandon-class →
            // they stay verbatim (the model is actively building on them).
            assert!(
                trunk_has_heavy_args(&rendered, "PLAN-V2-BODY"),
                "turn {turn}: the live plan-v2 body is untouched (not reverted)"
            );
            assert!(
                trunk_has_heavy_args(&rendered, "NOTES-BODY"),
                "turn {turn}: the live notes body is untouched (not reverted)"
            );
            // The breadcrumb naming the abandoned work survives in-window.
            assert!(
                trunk_has_breadcrumb(&rendered, "write_file abandoned"),
                "turn {turn}: the breadcrumb survives in-window"
            );
            // No orphaned tool-result for the collapsed c1 cluster.
            assert!(
                !trunk_has_tool_result(&rendered, "c1"),
                "turn {turn}: the collapsed c1 result is removed — no orphan"
            );
            assert!(
                !trunk_has_dangling_call(&rendered),
                "turn {turn}: no dangling call anywhere"
            );
        }
    }

    /// CC-18 iter-2 — MULTI-REVERT: two abandon-class reverts on DIFFERENT
    /// clusters; BOTH masked persistently on every render, and each drops on its
    /// own decay window (one earlier, one later).
    #[test]
    fn multi_revert_both_masked_each_drops_on_own_window() {
        use super::super::node_tag::RevertRenderPolicy;
        // user → c1 CALL (n1, heavy A) → c1 result (n2) → c2 CALL (n3, heavy B)
        // → c2 result (n4) → c3 CALL (n5, live) → c3 result (n6). Revert #1
        // (failure) onto n2 at turn 1; revert #2 (failure) onto n4 at turn 5.
        let mut ctx = AgentContext {
            messages: vec![
                user_node("do the task", 1, NodeId(0), None),
                tool_call_node("c1", "ABANDONED-A-BODY", 2, NodeId(1), Some(NodeId(0))),
                result_node("c1", 3, NodeId(2), Some(NodeId(1))),
                tool_call_node("c2", "ABANDONED-B-BODY", 4, NodeId(3), Some(NodeId(2))),
                result_node("c2", 5, NodeId(4), Some(NodeId(3))),
                tool_call_node("c3", "LIVE-C-BODY", 6, NodeId(5), Some(NodeId(4))),
                result_node("c3", 7, NodeId(6), Some(NodeId(5))),
            ],
            next_node_id: 7,
            active_node_id: Some(NodeId(6)),
            ..Default::default()
        };
        // Revert #1: tag on n2, created at turn 1.
        if let AgentMessage::Llm(lm) = &mut ctx.messages[2] {
            lm.add_tag(NodeTag::new(
                TagKind::Lesson,
                "reverted past: A abandoned (write_file abandoned)".to_string(),
                1,
                vec![],
            ));
        }
        // Revert #2: tag on n4, created at turn 5.
        if let AgentMessage::Llm(lm) = &mut ctx.messages[4] {
            lm.add_tag(NodeTag::new(
                TagKind::Finding,
                "reverted past: B abandoned (write_file abandoned)".to_string(),
                5,
                vec![],
            ));
        }

        let policy = RevertRenderPolicy {
            lesson_window_turns: 3,
            lesson_window_count: 0,
        };

        // At turn 5: revert #1 (turn 1, distance 4 > 3) is OUT-of-window → its
        // node drops; revert #2 (turn 5, distance 0 ≤ 3) is IN-window → masked +
        // node kept. The live C body is untouched.
        let rendered = render_pipeline(&ctx, &policy, 5);
        assert!(
            !trunk_has_heavy_args(&rendered, "ABANDONED-A-BODY"),
            "A: masked (and decay-dropped) on every render"
        );
        assert!(
            !trunk_has_heavy_args(&rendered, "ABANDONED-B-BODY"),
            "B: masked (in-window collapse) on every render"
        );
        assert!(
            trunk_has_heavy_args(&rendered, "LIVE-C-BODY"),
            "C: the live (non-reverted) body is untouched"
        );
        // A is out-of-window → its collapsed node is dropped entirely.
        let has_n1 = rendered
            .iter()
            .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))));
        assert!(
            !has_n1,
            "A's collapsed node drops once its lesson is out-of-window"
        );
        // B is in-window → its collapsed node (the parent call n3) survives with
        // the breadcrumb, no orphaned result.
        assert!(
            trunk_has_breadcrumb(&rendered, "B abandoned"),
            "B's breadcrumb survives in-window"
        );
        assert!(
            !trunk_has_tool_result(&rendered, "c2"),
            "B's collapsed result is removed — no orphan"
        );
        assert!(
            !trunk_has_dangling_call(&rendered),
            "no dangling call anywhere"
        );
    }

    /// CC-18 iter-2 — PINNED cluster mid-trunk is kept verbatim even when the
    /// model has continued past it (it is never collapsed nor dropped — only
    /// abandon-class clusters are).
    #[test]
    fn pinned_cluster_mid_trunk_kept_verbatim() {
        use super::super::node_tag::RevertRenderPolicy;
        // user → c1 CALL (n1, sealed result, heavy) → c1 result (n2) → c2 CALL
        // (n3, live) → c2 result (n4). A `completion` (Outcome, pinned) revert
        // landed on n1 at turn 1; the model continued forward (tip n4), so the
        // pinned cluster is mid-trunk.
        let mut ctx = AgentContext {
            messages: vec![
                user_node("seal the milestone", 1, NodeId(0), None),
                tool_call_node("c1", "SEALED-RESULT-BODY", 2, NodeId(1), Some(NodeId(0))),
                result_node("c1", 3, NodeId(2), Some(NodeId(1))),
                tool_call_node("c2", "LIVE-BODY", 4, NodeId(3), Some(NodeId(2))),
                result_node("c2", 5, NodeId(4), Some(NodeId(3))),
            ],
            next_node_id: 5,
            active_node_id: Some(NodeId(4)),
            ..Default::default()
        };
        tag_message(
            &mut ctx.messages,
            1,
            TagKind::Outcome,
            "milestone sealed (write_file)",
        );

        let policy = RevertRenderPolicy::default();
        // Even far past any decay window, a pinned cluster is kept verbatim.
        for turn in [1u32, 100] {
            let rendered = render_pipeline(&ctx, &policy, turn);
            assert!(
                trunk_has_heavy_args(&rendered, "SEALED-RESULT-BODY"),
                "turn {turn}: the pinned sealed body is kept verbatim (never collapsed)"
            );
            assert!(
                trunk_has_heavy_args(&rendered, "LIVE-BODY"),
                "turn {turn}: the live body is untouched"
            );
            let has_n1 = rendered
                .iter()
                .any(|m| matches!(m, AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))));
            assert!(has_n1, "turn {turn}: the pinned node is never dropped");
        }
    }

    /// CC-18 iter-2 — IMMUTABLE-LOG after a MULTI-TURN persistent scan. The
    /// forensic `self.messages` must stay byte-identical after the persistent
    /// collapse runs over a multi-node, multi-revert trunk (the most load-bearing
    /// carry-forward invariant — the render is a mask, never a mutation).
    #[test]
    fn collapse_keeps_messages_byte_identical_after_multi_turn_scan() {
        use super::super::node_tag::RevertRenderPolicy;
        let mut ctx = AgentContext {
            messages: vec![
                user_node("do the task", 1, NodeId(0), None),
                tool_call_node("c1", "A-BODY", 2, NodeId(1), Some(NodeId(0))),
                result_node("c1", 3, NodeId(2), Some(NodeId(1))),
                tool_call_node("c2", "B-BODY", 4, NodeId(3), Some(NodeId(2))),
                result_node("c2", 5, NodeId(4), Some(NodeId(3))),
            ],
            next_node_id: 5,
            active_node_id: Some(NodeId(4)),
            ..Default::default()
        };
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Lesson,
            "reverted past: A abandoned (write_file abandoned)",
        );

        let before = serde_json::to_string(&ctx.messages).unwrap();
        let policy = RevertRenderPolicy::default();
        // Run the full pipeline at several turns (incl. past-window so the
        // decay-drop fires on the cloned trunk) — none may mutate self.messages.
        for turn in [1u32, 5, 50] {
            let _ = render_pipeline(&ctx, &policy, turn);
        }
        let after = serde_json::to_string(&ctx.messages).unwrap();
        assert_eq!(
            before, after,
            "the persistent multi-turn scan must NEVER mutate context.messages"
        );
    }

    /// CC-18 F-collapsed-node-decay.a guard — a node with REAL surviving content
    /// is NEVER dropped, even when its decayable lesson fully decays. This is the
    /// "nodes with real surviving content are never dropped" half of the rule.
    #[test]
    fn real_content_node_never_dropped_on_lesson_decay() {
        use super::super::node_tag::RevertRenderPolicy;
        let mut real = AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::Text {
                    text: "substantive reasoning output".to_string(),
                }],
                stop_reason: StopReason::Stop,
                model: "test".into(),
                provider: "test".into(),
                usage: Usage::default(),
                timestamp: 2,
                error_message: None,
            })
            .with_node_identity(NodeId(1), Some(NodeId(0))),
        );
        if let AgentMessage::Llm(lm) = &mut real {
            lm.add_tag(NodeTag::new(
                TagKind::Lesson,
                "a decayable lesson".to_string(),
                1,
                vec![],
            ));
        }
        let ctx = AgentContext {
            messages: vec![user_node("task", 1, NodeId(0), None), real],
            next_node_id: 2,
            active_node_id: Some(NodeId(1)),
            ..Default::default()
        };
        // Count-cap 0 so the lone lesson genuinely decays out past the window —
        // proving the node survives because of its REAL content, not because the
        // tag was force-kept.
        let policy = RevertRenderPolicy {
            lesson_window_turns: 3,
            lesson_window_count: 0,
        };
        // Past the window, through the FULL production pipeline (collapse-first):
        // the lesson decays but the node carries real text content, so collapse's
        // decay-drop guard never fires — the node is kept.
        let decayed = render_pipeline(&ctx, &policy, 10);
        let kept = decayed.iter().any(|m| {
            matches!(
                m,
                AgentMessage::Llm(lm) if lm.node_id == Some(NodeId(1))
            )
        });
        assert!(
            kept,
            "a node with real surviving content must NEVER be dropped on lesson decay"
        );
        assert!(
            trunk_has_surviving_assistant_text(&decayed, "substantive reasoning output"),
            "the real content survives (the node is not a lone breadcrumb)"
        );
    }
}
