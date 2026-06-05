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

    /// Composition I — render-time reclamation of the abandon-class tool-cluster
    /// a revert landed on.
    ///
    /// When a `failure`/`tangent` (abandon-class) revert targets a node that is
    /// (or splits) a heavy `(assistant-tool_call, tool_result)` cluster, the
    /// kept tip ends up carrying the abandoned tool-call's heavy `arguments`
    /// (e.g. an 80-line `write_file` body) while its matching tool-result has
    /// been dropped off-trunk by the parent-chain walk — leaving the rendered
    /// trunk both **larger** (the abandoned body survives) and **malformed** (a
    /// tool-call with no matching result). This pass fixes both, at render time
    /// only:
    ///
    /// - **Abandon-class tip** (the tip carries a decay-able `Lesson`/`Finding`
    ///   tag, the signal that a revert just landed there): the whole cluster is
    ///   collapsed atomically to a single clean assistant node carrying the
    ///   breadcrumb, with NO heavy `ToolCall` and NO orphaned tool-result. The
    ///   one-line breadcrumb (composed by `apply_revert`) is woven into content
    ///   by `weave_braking_annotations` downstream, so the model reads the
    ///   breadcrumb where the ~80-line body used to be. Two tip shapes are
    ///   handled: a **call-tip** (the tip IS the assistant tool-call node) strips
    ///   the tip's heavy `ToolCall` blocks — its matching tool-result is off-trunk
    ///   (a child excluded by the parent-chain walk), so nothing dangles; a
    ///   **result-tip** (the tip is the matching tool-result, the #59 shape) finds
    ///   the parent assistant-call node on the trunk, strips ITS `ToolCall` blocks,
    ///   MOVES the tip's `NodeTag`(s) onto that parent, and REMOVES the tool-result
    ///   tip from the trunk Vec — so there is no orphaned result (a `tool_call_id`
    ///   with no matching call would be rejected by OpenAI-compat providers).
    /// - **Pinned tip** (`Outcome`/`Checkpoint`, NOT decay-able): the cluster is
    ///   load-bearing (a sealed result the model may re-read), so it is kept
    ///   WHOLE — if the tip is a tool-call whose matching tool-result is
    ///   off-trunk, the result is re-appended right after the tip so the kept
    ///   call is never left dangling.
    ///
    /// The **atomic-cluster invariant** therefore holds for ALL categories: the
    /// rendered trunk never carries a tool-call without its matching result
    /// (abandon-class drops both; pinned keeps both).
    ///
    /// Operates over the **cloned** trunk that `build_trunk_context` returns by
    /// value; `self.messages` (the forensic log) is **never** mutated. Caller
    /// contract: invoke ONLY on the revert-mode trunk path
    /// (`active_node_id.is_some()`), after `build_trunk_context_with_policy` and
    /// before `weave_braking_annotations`. General braking-machinery merit: any
    /// consumer reverting onto/across a heavy tool-cluster reclaims that context
    /// with no log mutation.
    pub fn collapse_abandon_class_cluster(
        &self,
        mut trunk: Vec<AgentMessage>,
    ) -> Vec<AgentMessage> {
        use super::content::{Content, Message};

        // Locate the trunk tip (the LAST node-bearing message) — the
        // active / reverted-to node. A revert landed here iff the tip carries a
        // NodeTag (abandon-class = decay-able Lesson/Finding; pinned =
        // Outcome/Checkpoint). No tag → no revert landed on this tip → nothing
        // to collapse.
        let tip_idx = trunk.iter().enumerate().rev().find_map(|(i, m)| match m {
            AgentMessage::Llm(lm) if lm.node_id.is_some() => Some(i),
            _ => None,
        });
        let Some(tip_idx) = tip_idx else {
            return trunk;
        };

        let AgentMessage::Llm(tip) = &trunk[tip_idx] else {
            return trunk;
        };
        // Classify the tip's revert tag (if any). A node can carry tags of only
        // one decay-class per revert; we read the first tag's kind as the gate.
        let Some(tag) = tip.tags.first() else {
            return trunk; // no revert tag on the tip → not a revert-landed cluster
        };
        let abandon_class = tag.kind.is_decayable();

        // Classify the tip's message shape. The cluster mechanics apply to two
        // shapes the revert can land on:
        //
        //  (1) the tip is the assistant TOOL-CALL node itself — its `Content::
        //      ToolCall` carries the heavy `arguments`. (`deepseek` reverted
        //      `step="n0"` onto this shape.)
        //  (2) the tip is the matching TOOL-RESULT node — the heavy `arguments`
        //      live in the tip's PARENT assistant-call node, which stays
        //      on-trunk (the tip's `tool_call_id` names it). This is the
        //      #59-canonical shape: `minimax` reverted `step="n1"` onto the
        //      tool-result of the abandoned `write_file`, leaving the heavy
        //      plan-v1 in the parent call node `n0`.
        //
        // For shape (1) `tip_call_id` is the tip's own ToolCall id; for shape
        // (2) it is the tip's `tool_call_id` linking back to the parent call.
        // Either way, `tip_call_id` identifies the cluster's tool invocation.
        let tip_is_call = matches!(tip.message, Message::Assistant { .. });
        let tip_call_id = match &tip.message {
            Message::Assistant { content, .. } => content.iter().find_map(|b| match b {
                Content::ToolCall { id, .. } => Some(id.clone()),
                _ => None,
            }),
            Message::ToolResult { tool_call_id, .. } => Some(tool_call_id.clone()),
            _ => None,
        };
        let Some(tip_call_id) = tip_call_id else {
            return trunk; // tip is neither a tool-call node nor a tool-result → no cluster
        };

        if abandon_class {
            if tip_is_call {
                // Shape (1): the tip is the abandoned assistant tool-call node.
                // Strip the heavy ToolCall blocks from the tip so the rendered
                // trunk carries the breadcrumb (woven from the tag) instead of
                // the ~80-line abandoned body. Dropping the ToolCall also
                // removes the dangling-call hazard (no call → nothing to
                // dangle). Keep any non-ToolCall blocks (e.g. a leading
                // Thinking/Text the assistant emitted alongside the call).
                if let AgentMessage::Llm(lm) = &mut trunk[tip_idx] {
                    if let Message::Assistant { content, .. } = &mut lm.message {
                        content.retain(|b| !matches!(b, Content::ToolCall { .. }));
                    }
                }
            } else {
                // Shape (2) — the #59-canonical tool-RESULT tip. The heavy body
                // lives in the tip's PARENT assistant-call node (the on-trunk
                // node whose `Content::ToolCall.id` == the tip's `tool_call_id`).
                // Collapse the WHOLE cluster atomically so the rendered trunk
                // carries a single clean assistant node with the breadcrumb tag,
                // NO heavy ToolCall, and NO orphaned tool-result:
                //
                //  (a) find the parent assistant-call node on the trunk;
                //  (b) strip its ToolCall blocks (reclaim the heavy plan-v1 body);
                //  (c) MOVE the tip's NodeTag(s) onto the parent node so the
                //      breadcrumb renders on the surviving assistant node;
                //  (d) REMOVE the tool-result tip from the trunk Vec so there is
                //      no orphaned result (a tool_call_id with no matching call
                //      → OpenAI-compat providers reject it).
                //
                // The parent assistant node becomes the last node-bearing
                // message, carrying the moved tag — so the downstream weave's
                // tip detection finds it and folds the continue-forward directive
                // onto it.
                if let Some(parent_idx) = trunk.iter().position(|m| match m {
                    AgentMessage::Llm(lm) => match &lm.message {
                        Message::Assistant { content, .. } => content.iter().any(
                            |b| matches!(b, Content::ToolCall { id, .. } if *id == tip_call_id),
                        ),
                        _ => false,
                    },
                    _ => false,
                }) {
                    // (c) lift the tip's tags out before we remove it.
                    let moved_tags = match &trunk[tip_idx] {
                        AgentMessage::Llm(lm) => lm.tags.clone(),
                        _ => Vec::new(),
                    };
                    // (b) strip the parent call's heavy ToolCall blocks and (c)
                    // append the moved tags onto the parent node.
                    if let AgentMessage::Llm(lm) = &mut trunk[parent_idx] {
                        if let Message::Assistant { content, .. } = &mut lm.message {
                            content.retain(|b| !matches!(b, Content::ToolCall { .. }));
                        }
                        lm.tags.extend(moved_tags);
                    }
                    // (d) remove the tool-result tip from the trunk so no
                    // orphaned result survives.
                    trunk.remove(tip_idx);
                }
            }
        } else if tip_is_call {
            // Pinned: keep the cluster WHOLE. If the matching tool-result is
            // off-trunk (the #59 shape: revert targeted the call node, so its
            // result child was excluded by the parent-chain walk), re-append it
            // right after the tip so the kept call is never dangling. Read the
            // off-trunk result from `self.messages` (the forensic log) by
            // tool_call_id; clone it (render-only, log untouched).
            let already_present = trunk.iter().any(|m| match m {
                AgentMessage::Llm(lm) => matches!(
                    &lm.message,
                    Message::ToolResult { tool_call_id, .. } if *tool_call_id == tip_call_id
                ),
                _ => false,
            });
            if !already_present {
                if let Some(result) = self.messages.iter().find(|m| match m {
                    AgentMessage::Llm(lm) => matches!(
                        &lm.message,
                        Message::ToolResult { tool_call_id, .. } if *tool_call_id == tip_call_id
                    ),
                    _ => false,
                }) {
                    trunk.insert(tip_idx + 1, result.clone());
                }
            }
        }
        // Pinned + tool-result tip: the cluster (parent call + result) is
        // already whole on-trunk (the parent-chain walk keeps the call as the
        // tip's parent), so nothing dangles and there is no action — the
        // `else if tip_is_call` guard above falls through to here as a no-op.

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

        // Locate the trunk tip (the LAST node-bearing message), which is the
        // active / reverted-to node. When that node carries ≥ 1 decay-able
        // lesson/finding tag — the signal that a revert just landed there
        // (apply_revert attaches the summary tag to the target node, which
        // becomes the active node) — we fold a forward-progress directive
        // DIRECTLY into the tip node's woven annotation (right after its
        // `[lesson: …]` tag) so a weaker model is steered onward instead of
        // looping on the re-presented task. The directive is part of the tip
        // node's content marker — there is NO standalone `Message::User` note.
        // General braking-machinery merit: any consumer reverting repeatedly
        // benefits from an attribution-clean continue-forward signal that does
        // not introduce a phantom user-role message.
        let tip_idx = messages
            .iter()
            .enumerate()
            .rev()
            .find_map(|(i, m)| match m {
                AgentMessage::Llm(lm) if lm.node_id.is_some() => Some(i),
                _ => None,
            });

        // Single pass: weave the per-node `[n<id>]` markers + surviving
        // lesson/finding tags into each node's content. On the abandon-class
        // tip (a tip carrying a lesson/finding tag), the continue-forward
        // directive is appended onto that node's marker right after its tags.
        let woven: Vec<AgentMessage> = messages
            .into_iter()
            .enumerate()
            .map(|(idx, m)| match m {
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
                    let mut tip_has_lesson = false;
                    for tag in &lm.tags {
                        let is_lesson = matches!(tag.kind, TagKind::Lesson | TagKind::Finding);
                        if !seen.insert((tag.kind, tag.text.as_str())) {
                            continue; // identical tag already rendered on this node
                        }
                        // The tip carrying a lesson/finding tag is the
                        // reverted-to node → fold the continue-forward directive
                        // onto it below.
                        if Some(idx) == tip_idx && is_lesson {
                            tip_has_lesson = true;
                        }
                        let kind = match tag.kind {
                            TagKind::Lesson => "lesson",
                            TagKind::Finding => "finding",
                            TagKind::Outcome => "outcome",
                            TagKind::Checkpoint => "checkpoint",
                        };
                        marker.push_str(&format!(" [{}: {}]", kind, tag.text));
                    }
                    // Fold the forward-progress directive into the tip node's
                    // annotation (right after its lesson tag), scoped to the
                    // abandon-class tip where the standalone note fired before.
                    if tip_has_lesson {
                        marker.push_str(
                            " [continue forward: do the next uncompleted step; \
                             do NOT redo completed steps; do NOT stop]",
                        );
                    }
                    prepend_marker_to_message(&mut lm.message, &marker);
                    AgentMessage::Llm(lm)
                }
                other => other,
            })
            .collect();

        woven
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
    fn weave_braking_annotations_weaves_forward_progress_marker_at_tip() {
        // The trunk tip (last node-bearing message) carrying a lesson tag is
        // the reverted-to node → the continue-forward directive must be FOLDED
        // INTO that tip node's woven annotation (right after its `[lesson: …]`
        // tag). There is NO standalone `Message::User` braking note.
        let root = user("write a plan", 1, NodeId(0), None);
        let mut tip = assistant("reverted here", 2, NodeId(1), Some(NodeId(0)));
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags
                .push(tag(TagKind::Lesson, 1, "v1 was tangential, restarting"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
        // No standalone note is inserted — the output length is unchanged.
        assert_eq!(
            woven.len(),
            2,
            "no standalone braking note must be inserted (directive is folded into the tip)"
        );
        // The root node does NOT carry the forward directive.
        assert!(
            !first_text(&woven[0]).contains("continue forward"),
            "root node must not carry the forward directive: {}",
            first_text(&woven[0])
        );
        // The tip node's OWN annotation carries the lesson tag AND the folded
        // continue-forward directive.
        let tip_text = first_text(&woven[1]);
        assert!(
            tip_text.starts_with("[n1]")
                && tip_text.contains("[lesson: v1 was tangential, restarting]"),
            "tip keeps its marker + lesson tag: {tip_text}"
        );
        assert!(
            tip_text.contains("[continue forward:")
                && tip_text.contains("next uncompleted step")
                && tip_text.contains("do NOT stop"),
            "tip annotation must carry the folded continue-forward directive: {tip_text}"
        );
        // No phantom `Message::User` braking-note entry exists in the output.
        assert!(
            !woven.iter().any(|m| matches!(
                m,
                AgentMessage::Llm(lm) if lm.node_id.is_none()
                    && matches!(&lm.message, Message::User { .. })
            )),
            "no standalone Message::User braking note must be present"
        );
    }

    #[test]
    fn forward_directive_folded_onto_tool_result_tip_annotation() {
        // When the trunk tip is a `Message::ToolResult` carrying a lesson tag,
        // the continue-forward directive is folded into the tip's OWN
        // annotation (right after its lesson tag) — there is no separate entry.
        let root = user("write a plan", 1, NodeId(0), None);
        let mut tip = tool_result("file written", 2, NodeId(1), Some(NodeId(0)));
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags
                .push(tag(TagKind::Lesson, 1, "v1 was tangential, restarting"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![root, tip]);
        // No standalone entry is appended.
        assert_eq!(woven.len(), 2, "no standalone note entry must be inserted");
        let tip_text = first_text(&woven[1]);
        assert!(
            tip_text.starts_with("[n1]") && tip_text.contains("file written"),
            "tool-result tip keeps its marker + content: {tip_text}"
        );
        assert!(
            tip_text.contains("[lesson: v1 was tangential, restarting]")
                && tip_text.contains("[continue forward:"),
            "the directive is folded into the tool-result tip's annotation: {tip_text}"
        );
    }

    #[test]
    fn forward_directive_folds_into_tip_annotation() {
        // The continue-forward directive renders folded into the tip node's
        // annotation with concrete, directive phrasing (do the next uncompleted
        // step, do not redo completed steps, do not stop). No standalone note.
        let mut tip = assistant("reverted here", 1, NodeId(0), None);
        if let AgentMessage::Llm(lm) = &mut tip {
            lm.tags
                .push(tag(TagKind::Lesson, 0, "approach v1 abandoned"));
        }
        let woven = AgentContext::weave_braking_annotations(vec![tip]);
        // Only the tip entry — no separate note.
        assert_eq!(woven.len(), 1, "no standalone note must follow the tip");
        let tip_text = first_text(&woven[0]);
        // The lesson breadcrumb renders on the tip's annotation.
        assert!(
            tip_text.contains("[lesson: approach v1 abandoned]"),
            "the lesson breadcrumb must render on the tip: {tip_text}"
        );
        // The folded continue-forward directive carries the concrete phrasing.
        assert!(tip_text.contains("[continue forward:"), "{tip_text}");
        assert!(tip_text.contains("next uncompleted step"), "{tip_text}");
        assert!(
            tip_text.contains("do NOT redo completed steps"),
            "{tip_text}"
        );
        assert!(tip_text.contains("do NOT stop"), "{tip_text}");
        // No `[braking note]` standalone-note phrasing remains anywhere.
        assert!(
            !woven
                .iter()
                .any(|m| first_text(m).contains("[braking note]")),
            "no standalone [braking note] entry must exist"
        );
    }

    #[test]
    fn weave_braking_annotations_no_forward_marker_without_lesson() {
        // A tip node with NO lesson/finding tag is not a fresh-revert tip →
        // no forward-progress directive (avoids spamming every trunk build).
        let am = assistant("ordinary tip", 1, NodeId(5), None);
        let woven = AgentContext::weave_braking_annotations(vec![am]);
        // No lesson/finding tag on the tip → no directive folded, no note added.
        assert_eq!(
            woven.len(),
            1,
            "no note must be inserted without a lesson tag"
        );
        let text = first_text(&woven[0]);
        assert!(
            !text.contains("continue forward"),
            "untagged tip must not carry the forward directive: {text}"
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

    /// An assistant node carrying a single heavy `write_file` ToolCall.
    fn tool_call_node(
        call_id: &str,
        heavy_args: &str,
        ts: u64,
        node: NodeId,
        parent: Option<NodeId>,
    ) -> AgentMessage {
        AgentMessage::Llm(
            LlmMessage::new(Message::Assistant {
                content: vec![Content::ToolCall {
                    id: call_id.to_string(),
                    name: "write_file".to_string(),
                    arguments: serde_json::json!({ "path": "plan-v1.md", "content": heavy_args }),
                }],
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

        let collapsed = ctx.collapse_abandon_class_cluster(trunk);

        // (a) the heavy ToolCall body is GONE from the rendered trunk.
        assert!(
            !trunk_has_heavy_args(&collapsed, "PLAN-V1-HEAVY-BODY"),
            "abandon-class collapse must strip the heavy tool-call body"
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
        let collapsed = ctx.collapse_abandon_class_cluster(trunk);

        // (a) the parent call's heavy ToolCall args are GONE.
        assert!(
            !trunk_has_heavy_args(&collapsed, "PLAN-V1-HEAVY-BODY"),
            "tool-result-tip collapse must strip the parent call's heavy body"
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

        let collapsed = ctx.collapse_abandon_class_cluster(trunk);

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
        let _collapsed = ctx.collapse_abandon_class_cluster(trunk);
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
        let collapsed = ctx.collapse_abandon_class_cluster(trunk);

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
        // The minimax shape end-to-end (collapse → weave): the model reverts
        // onto a tool-RESULT tip carrying an abandon-class Lesson breadcrumb.
        // After the atomic collapse + weave, the rendered output is a SINGLE
        // assistant node carrying `[lesson: … write_file …]` + the folded
        // `[continue forward: …]` directive — with NO `Message::ToolResult` and
        // NO standalone `Message::User` braking note.
        let mut ctx = fixture_59_tool_result_tip();
        tag_message(
            &mut ctx.messages,
            2,
            TagKind::Lesson,
            "reverted past: plan-v1 abandoned (write_file abandoned)",
        );
        let policy = super::super::node_tag::RevertRenderPolicy::default();
        let trunk = ctx.build_trunk_context_with_policy(&policy, 1);
        let collapsed = ctx.collapse_abandon_class_cluster(trunk);
        let woven = AgentContext::weave_braking_annotations(collapsed);

        // No ToolResult anywhere — the cluster collapsed atomically.
        assert!(
            !woven.iter().any(|m| matches!(
                m,
                AgentMessage::Llm(lm) if matches!(&lm.message, Message::ToolResult { .. })
            )),
            "no Message::ToolResult must survive the atomic collapse"
        );
        // No standalone Message::User braking note (nodeless user entry).
        assert!(
            !woven.iter().any(|m| matches!(
                m,
                AgentMessage::Llm(lm) if lm.node_id.is_none()
                    && matches!(&lm.message, Message::User { .. })
            )),
            "no standalone Message::User braking note must be present"
        );
        // The surviving assistant node carries the breadcrumb AND the folded
        // continue-forward directive in its content.
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
        let tip_text = woven
            .iter()
            .rev()
            .find_map(assistant_text)
            .expect("a surviving assistant node must carry woven content");
        assert!(
            tip_text.contains("[lesson:") && tip_text.contains("write_file abandoned"),
            "the surviving assistant node must carry the lesson breadcrumb: {tip_text}"
        );
        assert!(
            tip_text.contains("[continue forward:")
                && tip_text.contains("next uncompleted step")
                && tip_text.contains("do NOT stop"),
            "the surviving assistant node must carry the folded continue-forward directive: {tip_text}"
        );
        // And the heavy body is gone.
        assert!(
            !tip_text.contains("PLAN-V1-HEAVY-BODY"),
            "the heavy tool-call body must be gone: {tip_text}"
        );
    }
}
