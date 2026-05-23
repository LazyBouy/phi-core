//! Node identity + annotation types for tree-structured conversation state
//! ([Composition I](../../docs/concepts/concept-brake.md)).
//!
//! Composition I is the opt-in "braking" layer that lets the agent abandon
//! failed/finished branches of its own conversation by calling the
//! `revert_to_state` tool. These types are inert when
//! [`BasicAgent::with_revert_tool`](crate::agents::BasicAgent::with_revert_tool)
//! is not called — `LlmMessage`'s identity fields stay `None`, no tags are
//! attached, and the linear `build_working_context` path remains the default.
//!
//! - [`NodeId`] — monotonic per-loop integer identifier rendered inline as `n<N>`.
//! - [`RevertCategory`] — failure / tangent / completion / step-summary — the
//!   classification the agent chooses when it calls `revert_to_state`.
//! - [`TagKind`] — Lesson / Finding / Outcome / Checkpoint — what the resulting
//!   summary tag IS (1:1 derived from the [`RevertCategory`]).
//! - [`NodeTag`] — a model-generated summary attached to a trunk node.
//!
//! - [`RevertRenderPolicy`] — kind-aware decay window: how `Lesson` and
//!   `Finding` tags age out of the prompt while `Outcome` and `Checkpoint`
//!   tags stay pinned.

use serde::{Deserialize, Serialize};

/// A monotonic, globally unique identifier for a conversation node — an
/// [`LlmMessage`](super::agent_message::LlmMessage) on the working trunk.
///
/// Inline render: `n<N>` (e.g. `n12`). Wire format: bare integer. Allocated by
/// [`AgentContext::alloc_node_id`](super::context::AgentContext::alloc_node_id).
///
/// `node_id` is populated only when revert mode is active. When revert mode is
/// off, `LlmMessage::node_id` stays `None` and serde omits the `nodeId` key —
/// old session JSON round-trips unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(pub u64);

impl NodeId {
    /// Construct a `NodeId` directly. Most callers should use
    /// [`AgentContext::alloc_node_id`](super::context::AgentContext::alloc_node_id)
    /// to ensure monotonic, session-seeded allocation.
    pub fn new(value: u64) -> Self {
        Self(value)
    }

    /// The raw underlying counter value.
    pub fn raw(&self) -> u64 {
        self.0
    }

    /// Render as the inline tag string `n<N>` — what the agent sees in its
    /// context and what it echoes back into a `revert_to_state` tool call's
    /// `step` argument.
    pub fn render(&self) -> String {
        format!("n{}", self.0)
    }

    /// Lenient parse: accepts both `"n12"` and `"12"`. Returns `None` on
    /// malformed input (the `revert_to_state` tool's `execute()` uses this for
    /// argument validation).
    pub fn parse(s: &str) -> Option<Self> {
        let trimmed = s.strip_prefix('n').unwrap_or(s);
        if trimmed.is_empty() {
            return None;
        }
        trimmed.parse::<u64>().ok().map(Self)
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "n{}", self.0)
    }
}

/// The four revert categories the agent can request via `revert_to_state`.
///
/// See `docs/concepts/concept-brake.md` §5 (Composition I) for the full
/// semantics. Short version:
/// - `Failure` — a dead-end branch the agent should learn from and not repeat.
/// - `Tangent` — an exploration finished; the finding folds back into the main task.
/// - `Completion` — a sub-task is done; squash to a sealed outcome summary.
/// - `StepSummary` — the current task is ongoing but the trunk got long;
///   checkpoint a span and continue from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RevertCategory {
    Failure,
    Tangent,
    Completion,
    StepSummary,
}

impl RevertCategory {
    /// The [`TagKind`] produced by a revert of this category.
    pub fn tag_kind(self) -> TagKind {
        match self {
            Self::Failure => TagKind::Lesson,
            Self::Tangent => TagKind::Finding,
            Self::Completion => TagKind::Outcome,
            Self::StepSummary => TagKind::Checkpoint,
        }
    }
}

/// The annotation kind that ends up on a node, 1:1 with [`RevertCategory`].
///
/// The kind drives the render policy (added in Phase 5):
/// - `Lesson` / `Finding` — decay-able (sliding window).
/// - `Outcome` / `Checkpoint` — pinned while live work depends on them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TagKind {
    Lesson,
    Finding,
    Outcome,
    Checkpoint,
}

impl TagKind {
    /// `true` for `Lesson` / `Finding` — these decay through a sliding window.
    /// `false` for `Outcome` / `Checkpoint` — these stay pinned while live work
    /// depends on them.
    pub fn is_decayable(self) -> bool {
        matches!(self, Self::Lesson | Self::Finding)
    }
}

/// A model-generated summary attached to a trunk node by `apply_revert`.
///
/// Tags live on
/// [`LlmMessage::tags`](super::agent_message::LlmMessage::tags) — the
/// annotation layer. They are NOT trunk nodes themselves and do not affect the
/// parent-chain walk. The kind-aware render policy (Phase 5) decides when each
/// tag is rendered into the prompt vs. log-only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NodeTag {
    /// Lesson / Finding / Outcome / Checkpoint.
    pub kind: TagKind,
    /// The one-line summary the agent supplied in the `revert_to_state` tool
    /// call (or, in a future release, that a fallback generator produced).
    pub text: String,
    /// The turn index this tag was created at — used by the Phase 5 decay
    /// window for `Lesson` / `Finding` tags.
    pub created_at_turn: u32,
    /// Forensic cross-reference: the `node_id`s of the messages that were on
    /// the abandoned branch this tag distills. Those nodes still exist in the
    /// full `messages` log but are off the active parent-chain.
    pub abandoned_node_ids: Vec<NodeId>,
}

impl NodeTag {
    pub fn new(
        kind: TagKind,
        text: String,
        created_at_turn: u32,
        abandoned_node_ids: Vec<NodeId>,
    ) -> Self {
        Self {
            kind,
            text,
            created_at_turn,
            abandoned_node_ids,
        }
    }
}

/// Configurable thresholds for the kind-aware render policy
/// ([`AgentContext::build_trunk_context`](super::context::AgentContext::build_trunk_context)).
///
/// `Lesson` and `Finding` tags are decay-able: once they fall outside the
/// recent-turn window AND the recent-count window, they remain in the session
/// log but stop being rendered into the LLM prompt. `Outcome` and `Checkpoint`
/// tags stay pinned and always render while on-trunk.
///
/// Defaults match the values in the Composition I plan (5 turns, 3 tags).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevertRenderPolicy {
    /// A decay-able tag is rendered if `current_turn - tag.created_at_turn <= lesson_window_turns`.
    /// Default: `5`.
    pub lesson_window_turns: u32,
    /// In addition to the turn-distance gate, the most-recent
    /// `lesson_window_count` decay-able tags of each kind always render. So a
    /// rapid-fire run of `failure` calls won't drop older ones until newer ones
    /// take their slots. Default: `3`.
    pub lesson_window_count: usize,
}

impl Default for RevertRenderPolicy {
    fn default() -> Self {
        Self {
            lesson_window_turns: 5,
            lesson_window_count: 3,
        }
    }
}

impl RevertRenderPolicy {
    /// Should `tag` render into the prompt at `current_turn`?
    ///
    /// - Pinned kinds (`Outcome`, `Checkpoint`) — always `true`.
    /// - Decay-able kinds (`Lesson`, `Finding`) — `true` iff the tag is
    ///   within `lesson_window_turns` of the current turn. Caller separately
    ///   enforces the `lesson_window_count` cap (it requires global ordering
    ///   that this method does not have).
    pub fn renders_by_turn(&self, tag: &NodeTag, current_turn: u32) -> bool {
        if !tag.kind.is_decayable() {
            return true;
        }
        current_turn.saturating_sub(tag.created_at_turn) <= self.lesson_window_turns
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_id_renders_with_n_prefix() {
        assert_eq!(NodeId(12).render(), "n12");
        assert_eq!(NodeId(0).render(), "n0");
        assert_eq!(format!("{}", NodeId(99)), "n99");
    }

    #[test]
    fn node_id_parse_lenient() {
        assert_eq!(NodeId::parse("n12"), Some(NodeId(12)));
        assert_eq!(NodeId::parse("12"), Some(NodeId(12)));
        assert_eq!(NodeId::parse("0"), Some(NodeId(0)));
        assert_eq!(NodeId::parse("nabc"), None);
        assert_eq!(NodeId::parse(""), None);
        assert_eq!(NodeId::parse("n"), None);
    }

    #[test]
    fn node_id_serializes_as_bare_integer() {
        let json = serde_json::to_string(&NodeId(42)).unwrap();
        assert_eq!(json, "42");
        let back: NodeId = serde_json::from_str("42").unwrap();
        assert_eq!(back, NodeId(42));
    }

    #[test]
    fn revert_category_serializes_kebab_case() {
        assert_eq!(
            serde_json::to_string(&RevertCategory::Failure).unwrap(),
            "\"failure\""
        );
        assert_eq!(
            serde_json::to_string(&RevertCategory::Tangent).unwrap(),
            "\"tangent\""
        );
        assert_eq!(
            serde_json::to_string(&RevertCategory::Completion).unwrap(),
            "\"completion\""
        );
        assert_eq!(
            serde_json::to_string(&RevertCategory::StepSummary).unwrap(),
            "\"step-summary\""
        );
    }

    #[test]
    fn revert_category_tag_kind_mapping() {
        assert_eq!(RevertCategory::Failure.tag_kind(), TagKind::Lesson);
        assert_eq!(RevertCategory::Tangent.tag_kind(), TagKind::Finding);
        assert_eq!(RevertCategory::Completion.tag_kind(), TagKind::Outcome);
        assert_eq!(RevertCategory::StepSummary.tag_kind(), TagKind::Checkpoint);
    }

    #[test]
    fn tag_kind_decay_classification() {
        assert!(TagKind::Lesson.is_decayable());
        assert!(TagKind::Finding.is_decayable());
        assert!(!TagKind::Outcome.is_decayable());
        assert!(!TagKind::Checkpoint.is_decayable());
    }

    #[test]
    fn node_tag_roundtrip() {
        let tag = NodeTag::new(
            TagKind::Lesson,
            "bubble sort timed out".to_string(),
            5,
            vec![NodeId(11), NodeId(12)],
        );
        let json = serde_json::to_string(&tag).unwrap();
        let back: NodeTag = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tag);
    }

    #[test]
    fn revert_render_policy_pinned_kinds_always_render() {
        let policy = RevertRenderPolicy::default();
        for kind in [TagKind::Outcome, TagKind::Checkpoint] {
            let tag = NodeTag::new(kind, "x".into(), 0, vec![]);
            // 1000 turns later: still renders.
            assert!(policy.renders_by_turn(&tag, 1000));
        }
    }

    #[test]
    fn revert_render_policy_decayable_within_window() {
        let policy = RevertRenderPolicy::default(); // 5 turns
        let tag = NodeTag::new(TagKind::Lesson, "x".into(), 10, vec![]);
        assert!(policy.renders_by_turn(&tag, 10));
        assert!(policy.renders_by_turn(&tag, 15)); // exactly at window
        assert!(!policy.renders_by_turn(&tag, 16)); // just past
    }

    #[test]
    fn revert_render_policy_custom_window() {
        let policy = RevertRenderPolicy {
            lesson_window_turns: 2,
            lesson_window_count: 1,
        };
        let tag = NodeTag::new(TagKind::Finding, "x".into(), 5, vec![]);
        assert!(policy.renders_by_turn(&tag, 7));
        assert!(!policy.renders_by_turn(&tag, 8));
    }
}
