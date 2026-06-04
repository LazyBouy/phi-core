//! Model-invocable tool that returns a named tool's extended manual on demand.
//
// ARCHITECTURE: ToolHelpTool — an on-demand, permission-safe tool-documentation
// channel.
//
// Why a kernel tool (general merit, not a single-consumer requirement):
//   - Self-descriptions (`AgentTool::description()`) are the ONLY channel a model
//     has to learn a tool's mental model, and they are paid on EVERY turn. Tools
//     with a non-trivial model (e.g. `revert_to_state`'s tree/node model) face a
//     dilemma: under-describe (weak models loop/misuse) or over-pack (every model
//     pays the full token cost every turn).
//   - `tool_help(tool_name)` lets the model self-serve the FULL manual exactly
//     when it needs the depth — paying the cost on demand, not on every turn.
//   - It is permission-safe: unlike embedding a file path + `read_file`, it
//     exposes no filesystem and works even when the agent is sandboxed/locked
//     down (the lockdown scenarios that most need guidance).
//
// Consumer-agnostic content: the kernel ships the SHORT canonical manual for its
// own built-in tools via [`ToolHelpTool::with_default_help`]. A consumer that
// wants a richer per-tool body supplies its own map via [`ToolHelpTool::new`] —
// NO consumer-specific path or requirement leaks into the kernel.

use crate::types::{AgentTool, Content, ToolContext, ToolError, ToolResult};
use std::collections::BTreeMap;

/// Built-in tool that returns a named tool's extended manual text.
///
/// Construction:
/// - [`ToolHelpTool::with_default_help`] — the kernel's short canonical manuals
///   for its own built-in tools (consumer-agnostic).
/// - [`ToolHelpTool::new`] — a consumer-supplied `tool_name → help_text` map
///   (e.g. a downstream that wants richer worked-examples bodies).
///
/// Register on a `BasicAgent` via
/// [`with_tool_help`](crate::agents::BasicAgent::with_tool_help).
pub struct ToolHelpTool {
    /// `tool_name → help_text`. A `BTreeMap` keeps the available-tools list in
    /// stable sorted order for the unknown-tool fallback message.
    help: BTreeMap<String, String>,
}

impl ToolHelpTool {
    /// Construct with an explicit `tool_name → help_text` map.
    pub fn new(help: BTreeMap<String, String>) -> Self {
        Self { help }
    }

    /// Construct with the kernel's short canonical manuals for its built-in
    /// tools. Consumer-agnostic — no downstream path or requirement is baked in.
    pub fn with_default_help() -> Self {
        let mut help = BTreeMap::new();
        help.insert("revert_to_state".to_string(), REVERT_HELP.to_string());
        help.insert("prun".to_string(), PRUN_HELP.to_string());
        help.insert("prun_with_memo".to_string(), PRUN_HELP.to_string());
        Self { help }
    }

    /// Add or override a single tool's help text (builder-style). Lets a
    /// consumer extend the default map with its own richer manuals.
    pub fn with_help(mut self, tool_name: impl Into<String>, text: impl Into<String>) -> Self {
        self.help.insert(tool_name.into(), text.into());
        self
    }
}

#[async_trait::async_trait]
impl AgentTool for ToolHelpTool {
    fn name(&self) -> &str {
        "tool_help"
    }

    fn label(&self) -> &str {
        "Tool Help"
    }

    fn description(&self) -> &str {
        "Fetch the full manual for another tool on demand — its mental model, worked examples, and failure modes. Call this when a tool's short description is not enough to use it correctly (e.g. revert_to_state). Pass the tool's exact name in `tool_name`."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "tool_name": {
                    "type": "string",
                    "description": "The exact name of the tool whose manual you want (e.g. \"revert_to_state\")."
                }
            },
            "required": ["tool_name"]
        })
    }

    async fn execute(
        &self,
        params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        let tool_name = params
            .get("tool_name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidArgs("tool_name is required".to_string()))?;

        let text = match self.help.get(tool_name) {
            Some(manual) => manual.clone(),
            None => {
                let available: Vec<&str> = self.help.keys().map(|s| s.as_str()).collect();
                format!(
                    "No extended manual is registered for {:?}. Tools with a manual: {}.",
                    tool_name,
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                )
            }
        };

        Ok(ToolResult {
            content: vec![Content::Text { text }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

/// Short canonical manual for `revert_to_state` (kernel-side; consumers may
/// supply a richer extended body via [`ToolHelpTool::new`]).
const REVERT_HELP: &str = "\
revert_to_state — rewind the conversation tree and resume forward.

MENTAL MODEL
  The conversation is a TREE of nodes. Each assistant/tool step is a node,
  tagged inline in the messages as [n0], [n1], [n2], … in order. Those inline
  tags ARE the nodes — the `step` argument names one of them.

WHAT IT DOES
  Naming a node in `step` makes it the new tip: every node AFTER it is dropped
  from your active context. The dropped messages stay in the forensic session
  log; only your working context changes. The `summary` you supply is pinned to
  the target node as a lesson so the next turn remembers what was tried.

HOW TO CHOOSE THE NODE
  Read the [nN] tags and pick the node JUST BEFORE the branch you want to
  discard. Reverting to n0 discards the entire conversation back to the first
  node — only do that to restart from scratch.

AFTER REVERTING
  CONTINUE FORWARD with your new approach from that node. Do NOT repeat the
  steps you just abandoned — that is the loop trap. The rebuilt context shows
  your pinned lesson so you know what not to retry.

CATEGORIES
  failure = a dead-end branch to learn from; tangent = a finished exploration to
  fold back; completion = a sealed sub-task; step-summary = a checkpoint on a
  long ongoing trunk.";

/// Short canonical manual for the `prun` / `prun_with_memo` variants.
const PRUN_HELP: &str = "\
prun / prun_with_memo — model-directed context pruning.

WHAT IT DOES
  prun(tokens) silently removes the oldest in-run context entries to free up
  roughly `tokens` of budget. prun_with_memo(tokens, memo) does the same but
  leaves your `memo` summary in place of what was removed, so the gist survives.

GUARANTEES
  User messages are NEVER pruned. Pruned content is preserved in the forensic
  session log (a PrunRecord) — only your active working context shrinks.

WHEN TO USE
  Use when older turns are no longer load-bearing and the context is getting
  long. Prefer prun_with_memo when the removed span carries a conclusion you
  still need to remember; use plain prun when the span is pure noise.";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ToolContext;

    fn ctx() -> ToolContext {
        ToolContext {
            tool_call_id: "call-1".into(),
            tool_name: "tool_help".into(),
            cancel: tokio_util::sync::CancellationToken::new(),
            on_update: None,
            on_progress: None,
        }
    }

    #[tokio::test]
    async fn returns_manual_for_a_named_tool() {
        let tool = ToolHelpTool::with_default_help();
        let result = tool
            .execute(serde_json::json!({ "tool_name": "revert_to_state" }), ctx())
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.clone(),
            _ => panic!("expected text content"),
        };
        assert!(
            text.contains("TREE of nodes"),
            "manual must teach the tree model: {text}"
        );
        assert!(
            text.contains("CONTINUE FORWARD"),
            "manual must teach forward-continuation: {text}"
        );
    }

    #[tokio::test]
    async fn unknown_tool_lists_available_tools() {
        let tool = ToolHelpTool::with_default_help();
        let result = tool
            .execute(serde_json::json!({ "tool_name": "does_not_exist" }), ctx())
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.clone(),
            _ => panic!("expected text content"),
        };
        assert!(text.contains("No extended manual is registered"), "{text}");
        // Lists the available tools (sorted via BTreeMap).
        assert!(text.contains("revert_to_state"), "{text}");
        assert!(text.contains("prun"), "{text}");
    }

    #[tokio::test]
    async fn missing_tool_name_is_invalid_args() {
        let tool = ToolHelpTool::with_default_help();
        let err = tool
            .execute(serde_json::json!({}), ctx())
            .await
            .unwrap_err();
        assert!(matches!(err, ToolError::InvalidArgs(_)));
    }

    #[tokio::test]
    async fn consumer_supplied_map_is_used() {
        let mut map = BTreeMap::new();
        map.insert("my_tool".to_string(), "my custom manual".to_string());
        let tool = ToolHelpTool::new(map);
        let result = tool
            .execute(serde_json::json!({ "tool_name": "my_tool" }), ctx())
            .await
            .unwrap();
        let text = match &result.content[0] {
            Content::Text { text } => text.clone(),
            _ => panic!("expected text content"),
        };
        assert_eq!(text, "my custom manual");
    }
}
