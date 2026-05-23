//! Model-invocable tool for Composition I "braking": tree-structured revert
//! between turns. See `docs/concepts/concept-brake.md` §5.
//
// ARCHITECTURE: RevertTool — model-directed branch abandonment via deferred execution
//
// Mirrors the deferred-apply pattern from `PrunTool` (see `tools/prun.rs`): the
// tool's `execute()` validates input and enqueues a `RevertRequest` on a shared
// `Arc<Mutex<Vec<_>>>`; the agent loop drains the queue between turns and applies
// each request via `apply_revert` (Phase 3). The tool itself does NOT mutate
// `AgentContext`.
//
// Why deferred (same reasoning as `PrunTool`):
//   1. Ownership — tools see `&self`; the active-node-id and message tree are
//      owned by the agent loop. Threading `&mut AgentContext` through
//      `ToolContext` would defeat parallel tool execution.
//   2. Timing — mid-turn mutation while the LLM stream is open would corrupt
//      content_index counters in `StreamEvent` deltas. Between-turn application
//      is the only safe window.
//   3. Auditing — every drained `RevertRequest` produces an `AgentEvent::RevertApplied`
//      (Phase 3) which the session recorder auto-persists; the abandoned span
//      lives forever in the forensic `messages` log and is only off-trunk.
//
// Opt-in guarantee: `RevertTool` is registered exclusively by
// `BasicAgent::with_revert_tool()`. The LLM never sees the tool unless the
// builder explicitly enables it; the apply_revert drain is gated on
// `AgentLoopConfig.revert_pending.is_some()`, which is set only by the same
// builder method. There is no other registration path.

use crate::types::{
    AgentTool, Content, NodeId, RevertCategory, ToolContext, ToolError, ToolResult,
};
use std::sync::{Arc, Mutex};

/// A pending revert request the LLM submitted via `revert_to_state`.
///
/// Lifecycle:
/// 1. [`RevertTool::execute`] pushes one of these onto the shared queue.
/// 2. The agent loop drains the queue between turns and calls `apply_revert`
///    on each (Phase 3).
/// 3. `apply_revert` validates the target, moves `AgentContext.active_node_id`,
///    attaches a [`NodeTag`](crate::types::NodeTag) carrying `summary`, and emits
///    `AgentEvent::RevertApplied` with the structured outcome.
#[derive(Debug, Clone)]
pub struct RevertRequest {
    /// Which of the four categories the agent chose — drives the resulting
    /// [`TagKind`](crate::types::TagKind) and the kind-aware render policy.
    pub category: RevertCategory,
    /// The [`NodeId`] the agent wants to revert to. The abandoned span is
    /// everything strictly after this node on the current trunk.
    pub target: NodeId,
    /// Agent-supplied one-line summary that becomes the
    /// [`NodeTag::text`](crate::types::NodeTag::text) attached to the target
    /// node. `None` is structurally valid — `apply_revert` attaches an empty
    /// tag — and reserved for a future fallback generator.
    pub summary: Option<String>,
}

/// Structured metadata persisted in the `details` field of the synthetic
/// `revert_to_state` `ToolResult`, and (mirroring `PrunRecord`) carried into
/// the `AgentEvent::RevertApplied` payload by `apply_revert`.
///
/// Source-of-truth for revert observability: a session replay can reconstruct
/// exactly which branch was abandoned, what category the agent assigned, and
/// what summary it wrote.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RevertRecord {
    /// Category the agent assigned at call time.
    pub category: RevertCategory,
    /// Target node — the new active-node-id after the revert.
    pub target: NodeId,
    /// The `node_id`s of every message that fell off-trunk as a result of the
    /// revert. Populated by `apply_revert` (Phase 3); empty at enqueue time.
    pub abandoned_node_ids: Vec<NodeId>,
    /// Echo of `RevertRequest.summary`.
    pub summary: Option<String>,
}

/// Model-invocable tool that enqueues a revert request between turns.
///
/// Construction is gated by [`BasicAgent::with_revert_tool`](crate::agents::BasicAgent::with_revert_tool);
/// the tool struct itself is `pub` so that custom agents (e.g. embedded
/// downstream wrappers) can wire it manually if they share the same
/// `Arc<Mutex<Vec<RevertRequest>>>` with [`AgentLoopConfig::revert_pending`](crate::agent_loop::AgentLoopConfig).
pub struct RevertTool {
    /// Shared queue read by the agent-loop drain. The Arc + Mutex pattern is
    /// identical to `PrunTool::pending`.
    pending: Arc<Mutex<Vec<RevertRequest>>>,
}

impl RevertTool {
    /// Bind a new `RevertTool` to a shared pending queue.
    pub fn new(pending: Arc<Mutex<Vec<RevertRequest>>>) -> Self {
        Self { pending }
    }
}

#[async_trait::async_trait]
impl AgentTool for RevertTool {
    fn name(&self) -> &str {
        "revert_to_state"
    }

    fn label(&self) -> &str {
        "Revert to State"
    }

    fn description(&self) -> &str {
        "Abandon the current branch and return the conversation trunk to an earlier node. Use when a branch failed (failure), an exploration is finished (tangent), a sub-task is sealed (completion), or the trunk is long enough that a checkpoint helps (step-summary). Supply a one-line `summary` distilling what to remember; it is attached as an annotation on the target node so the next turn sees the lesson without the abandoned chatter. Abandoned messages stay in the forensic session log; only the active conversation context is rebuilt."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "category": {
                    "type": "string",
                    "enum": ["failure", "tangent", "completion", "step-summary"],
                    "description": "failure = dead-end branch to learn from; tangent = finished exploration to fold back; completion = sealed sub-task outcome; step-summary = checkpoint a long ongoing trunk."
                },
                "step": {
                    "type": "string",
                    "description": "Node identifier to revert to. Accepts the inline render (e.g. \"n10\") or a bare integer (\"10\")."
                },
                "summary": {
                    "type": "string",
                    "description": "Optional one-line summary attached as an annotation on the target node — what to remember about the abandoned branch."
                }
            },
            "required": ["category", "step"]
        })
    }

    /*
    DESIGN: execute() enqueues; apply_revert (Phase 3) mutates.

    Three responsibilities:
      1. Parse + validate `category` (must be one of the four kebab-case values).
      2. Parse + validate `step` (lenient — `NodeId::parse` accepts both `"n12"`
         and `"12"`).
      3. Optionally lift `summary` (any non-string value is treated as absent).

    On success: push a `RevertRequest` and return a synthetic ack so the LLM
    sees the call was accepted. The real work — moving the active pointer,
    attaching the NodeTag, emitting the event, rejecting unsafe targets —
    happens in `apply_revert`.

    `_ctx` is unused: there is no I/O, no cancellation budget to honour, no
    streaming output. Same shape as `PrunTool::execute`.
    */
    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let category_raw = params
            .get("category")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("category is required".to_string()))?;
        let category: RevertCategory = serde_json::from_value(serde_json::Value::String(
            category_raw.to_string(),
        ))
        .map_err(|_| {
            ToolError::InvalidArgs(format!(
                "category must be one of failure | tangent | completion | step-summary; got {:?}",
                category_raw
            ))
        })?;

        let step_raw = params
            .get("step")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("step is required".to_string()))?;
        let target = NodeId::parse(step_raw).ok_or_else(|| {
            ToolError::InvalidArgs(format!(
                "step must be a node identifier like \"n12\" or \"12\"; got {:?}",
                step_raw
            ))
        })?;

        let summary = params
            .get("summary")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

        self.pending.lock().unwrap().push(RevertRequest {
            category,
            target,
            summary: summary.clone(),
        });

        let ack_text = match summary.as_deref() {
            Some(s) => format!(
                "Revert request recorded: category={:?}, target={}, summary={:?}. The trunk will be rebuilt from this node before the next turn.",
                category, target, s
            ),
            None => format!(
                "Revert request recorded: category={:?}, target={}. The trunk will be rebuilt from this node before the next turn.",
                category, target
            ),
        };
        Ok(ToolResult {
            content: vec![Content::Text { text: ack_text }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}
