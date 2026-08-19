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
use std::sync::{Arc, Mutex};

/// One entry in a [`ToolCatalogSnapshot`]: a tool's detailed manual (if any) plus
/// its full parameters JSON-Schema, captured at agent-build time (KC-05, #109).
#[derive(Debug, Clone)]
pub struct ToolCatalogEntry {
    /// The tool's [`detailed_description`](AgentTool::detailed_description) at
    /// snapshot time (`None` if the tool has no separate detailed body).
    pub detailed: Option<String>,
    /// The tool's full [`parameters_schema`](AgentTool::parameters_schema) at
    /// snapshot time — including an MCP tool's complete `inputSchema`.
    pub schema: serde_json::Value,
}

/// A build-time snapshot of `tool_name → (detailed manual, full schema)` for the
/// tools an agent has installed (KC-05, #109).
///
/// Sourced from the folded tool Vec at `build_config` time via
/// [`build_tool_catalog_snapshot`] — cycle-free, since it holds owned data rather
/// than tool `Arc`s. It is what lets `tool_help(<name>)` serve a tool's full
/// schema + detailed body on demand even when the reduced turn-1 catalog carried
/// only a `{"type":"object"}` stub.
pub type ToolCatalogSnapshot = BTreeMap<String, ToolCatalogEntry>;

/// Build a [`ToolCatalogSnapshot`] from a folded tool set (KC-05, #109).
///
/// Captures each tool's [`detailed_description`](AgentTool::detailed_description)
/// together with its full [`parameters_schema`](AgentTool::parameters_schema)
/// (incl. MCP `inputSchema`). Called once the full tool Vec is known
/// (`BasicAgent::build_config`). Cycle-free: the returned snapshot owns its data
/// and holds no reference back to the tools.
pub fn build_tool_catalog_snapshot(tools: &[Arc<dyn AgentTool>]) -> ToolCatalogSnapshot {
    tools
        .iter()
        .map(|t| {
            (
                t.name().to_string(),
                ToolCatalogEntry {
                    detailed: t.detailed_description().map(|s| s.to_string()),
                    schema: t.parameters_schema(),
                },
            )
        })
        .collect()
}

/// Built-in tool that returns a named tool's extended manual text.
///
/// Construction:
/// - [`ToolHelpTool::with_default_help`] — the kernel's short canonical manuals
///   for its own built-in tools (consumer-agnostic).
/// - [`ToolHelpTool::new`] — a consumer-supplied `tool_name → help_text` map
///   (e.g. a downstream that wants richer worked-examples bodies).
/// - [`ToolHelpTool::with_catalog`] — attach a build-time
///   [`ToolCatalogSnapshot`] (KC-05) so `tool_help` serves each tool's full
///   schema + detailed body on demand (the progressive-catalog path).
///
/// Register on a `BasicAgent` via
/// [`with_tool_help`](crate::agents::BasicAgent::with_tool_help).
pub struct ToolHelpTool {
    /// `tool_name → help_text`. A `BTreeMap` keeps the available-tools list in
    /// stable sorted order for the unknown-tool fallback message.
    help: BTreeMap<String, String>,
    /// KC-05 (#109): shared slot for the build-time catalog snapshot. Empty until
    /// installed. `BasicAgent::build_config` fills this (via interior mutability)
    /// once the full tool set is folded; `execute()` prefers a catalog entry over
    /// the static `help` map. A shared `Arc<Mutex<…>>` avoids downcasting the
    /// type-erased tool back out of the agent's `Vec<Arc<dyn AgentTool>>`; it is
    /// cycle-free because it carries snapshot DATA, not tool `Arc`s.
    catalog: Arc<Mutex<Option<ToolCatalogSnapshot>>>,
}

impl ToolHelpTool {
    /// Construct with an explicit `tool_name → help_text` map.
    pub fn new(help: BTreeMap<String, String>) -> Self {
        Self {
            help,
            catalog: Arc::new(Mutex::new(None)),
        }
    }

    /// Construct with the kernel's short canonical manuals for its built-in
    /// tools. Consumer-agnostic — no downstream path or requirement is baked in.
    pub fn with_default_help() -> Self {
        let mut help = BTreeMap::new();
        help.insert("revert_to_state".to_string(), REVERT_HELP.to_string());
        help.insert("prun".to_string(), PRUN_HELP.to_string());
        help.insert("prun_with_memo".to_string(), PRUN_HELP.to_string());
        Self {
            help,
            catalog: Arc::new(Mutex::new(None)),
        }
    }

    /// Add or override a single tool's help text (builder-style). Lets a
    /// consumer extend the default map with its own richer manuals.
    pub fn with_help(mut self, tool_name: impl Into<String>, text: impl Into<String>) -> Self {
        self.help.insert(tool_name.into(), text.into());
        self
    }

    /// Pre-fill the build-time catalog snapshot (builder-style; KC-05, #109).
    ///
    /// After this, `tool_help(<name>)` serves the named tool's full
    /// `parameters_schema()` + detailed body from the snapshot, falling back to
    /// the static help map for the manual body when a tool has no
    /// `detailed_description()`. Primarily for direct construction + tests; the
    /// `BasicAgent` path fills the same slot at `build_config` time via
    /// [`catalog_slot`](Self::catalog_slot).
    pub fn with_catalog(self, snapshot: ToolCatalogSnapshot) -> Self {
        *self.catalog.lock().unwrap() = Some(snapshot);
        self
    }

    /// Return a clone of the shared catalog slot handle (KC-05, #109).
    ///
    /// `BasicAgent::with_tool_help` grabs this before type-erasing the tool into
    /// its `Vec<Arc<dyn AgentTool>>`, then `build_config` writes the freshly-built
    /// [`ToolCatalogSnapshot`] into it once the full tool set is folded — so the
    /// tool serves catalog-backed help without ever being downcast back out.
    pub fn catalog_slot(&self) -> Arc<Mutex<Option<ToolCatalogSnapshot>>> {
        self.catalog.clone()
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

        // KC-05 (#109): prefer the build-time catalog snapshot when installed —
        // serve the tool's full parameters schema + its detailed body (falling
        // back to the static manual for the body when the tool has no
        // detailed_description). This is what makes a reduced turn-1 catalog safe:
        // the model fetches the real schema on demand here.
        let catalog_entry = self
            .catalog
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|snapshot| snapshot.get(tool_name).cloned());

        let text = if let Some(entry) = catalog_entry {
            let body = entry.detailed.or_else(|| self.help.get(tool_name).cloned());
            let mut out = String::new();
            if let Some(b) = body {
                out.push_str(&b);
                out.push_str("\n\n");
            }
            out.push_str("PARAMETERS (JSON Schema):\n");
            out.push_str(
                &serde_json::to_string_pretty(&entry.schema)
                    .unwrap_or_else(|_| entry.schema.to_string()),
            );
            out
        } else {
            match self.help.get(tool_name) {
                Some(manual) => manual.clone(),
                None => {
                    // Available list = static-map keys ∪ catalog keys (sorted).
                    let mut available: std::collections::BTreeSet<String> =
                        self.help.keys().cloned().collect();
                    if let Some(snapshot) = self.catalog.lock().unwrap().as_ref() {
                        available.extend(snapshot.keys().cloned());
                    }
                    format!(
                        "No extended manual is registered for {:?}. Tools with a manual: {}.",
                        tool_name,
                        if available.is_empty() {
                            "(none)".to_string()
                        } else {
                            available.into_iter().collect::<Vec<_>>().join(", ")
                        }
                    )
                }
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
  tags ARE the nodes — the `step` argument names one of them. The `[nN]` tags
  are assigned by the system as it records each step; you only READ them to
  pick a `step`. Do NOT write `[nN]` tags yourself in your replies — the system
  prepends the next one for you; emitting your own just adds a wrong, stale id.

WHAT IT DOES
  Naming a node X in `step` makes it the new tip. The simple contract:
    - SHRINK: every node strictly after X is dropped from your active context
      (its body — even a heavy parallel tool exchange — leaves your context, so
      your budget gets smaller). The dropped messages stay in the forensic
      session log; only your working context changes.
    - SUMMARY at X: for `completion`/`step-summary` the `summary` is ADDED to X
      AFTER X's original content (X is KEPT); for `failure`/`tangent` the summary
      REPLACES X's content (the abandoned body is gone, the lesson takes its
      place).
    - CLEAN: if a dropped node leaves an orphaned tool-call behind on a kept
      node, that call is removed too, so the rebuilt context never dangles.
  If nothing comes after X (you're sealing the step you just finished), nothing
  shrinks — just continue.

HOW TO CHOOSE THE NODE
  Read the [nN] tags and pick the node JUST BEFORE the branch you want to
  discard. Reverting to n0 discards the entire conversation back to the first
  node — only do that to restart from scratch.

AFTER REVERTING
  CONTINUE FORWARD with your new approach from X. Do NOT repeat the steps you
  just abandoned — that is the loop trap. The rebuilt context shows your pinned
  summary so you know what not to retry.

CATEGORIES
  failure = a dead-end branch to learn from (summary REPLACES X's content);
  tangent = a finished exploration to fold back (summary REPLACES X's content);
  completion = a sealed sub-task (summary ADDED after X's kept content);
  step-summary = a checkpoint on a long ongoing trunk (summary ADDED after X's
  kept content).";

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

    // ── Tier D — catalog-backed schema source (KC-05, #109) ─────────────────

    fn text_of(result: &ToolResult) -> String {
        match &result.content[0] {
            Content::Text { text } => text.clone(),
            _ => panic!("expected text content"),
        }
    }

    struct SchemaTool {
        name: String,
        detailed: Option<String>,
    }

    #[async_trait::async_trait]
    impl AgentTool for SchemaTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn label(&self) -> &str {
            "Schema"
        }
        fn description(&self) -> &str {
            "a short one-liner"
        }
        fn detailed_description(&self) -> Option<&str> {
            self.detailed.as_deref()
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {"path": {"type": "string"}},
                "required": ["path"]
            })
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    #[tokio::test]
    async fn catalog_serves_detailed_and_full_schema() {
        let tool = SchemaTool {
            name: "my_tool".into(),
            detailed: Some("MY DETAILED MANUAL BODY".into()),
        };
        let snapshot = build_tool_catalog_snapshot(&[Arc::new(tool) as Arc<dyn AgentTool>]);
        let help = ToolHelpTool::with_default_help().with_catalog(snapshot);
        let result = help
            .execute(serde_json::json!({ "tool_name": "my_tool" }), ctx())
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(text.contains("MY DETAILED MANUAL BODY"), "{text}");
        assert!(text.contains("PARAMETERS (JSON Schema)"), "{text}");
        // The FULL schema is served on demand (the reduced wire only had a stub).
        assert!(
            text.contains("\"path\""),
            "must include the full schema: {text}"
        );
    }

    #[tokio::test]
    async fn catalog_serves_mcp_full_input_schema() {
        // An MCP-shaped adapter: has_large_schema() = true + a full inputSchema.
        struct McpShaped;
        #[async_trait::async_trait]
        impl AgentTool for McpShaped {
            fn name(&self) -> &str {
                "mcp_search"
            }
            fn label(&self) -> &str {
                "MCP Search"
            }
            fn description(&self) -> &str {
                "remote search"
            }
            fn has_large_schema(&self) -> bool {
                true
            }
            fn parameters_schema(&self) -> serde_json::Value {
                serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer"}
                    },
                    "required": ["query"]
                })
            }
            async fn execute(
                &self,
                _params: serde_json::Value,
                _ctx: ToolContext,
            ) -> Result<ToolResult, ToolError> {
                Ok(ToolResult {
                    content: vec![],
                    details: serde_json::Value::Null,
                    child_loop_id: None,
                })
            }
        }

        let snapshot = build_tool_catalog_snapshot(&[Arc::new(McpShaped) as Arc<dyn AgentTool>]);
        let help = ToolHelpTool::new(BTreeMap::new()).with_catalog(snapshot);
        let result = help
            .execute(serde_json::json!({ "tool_name": "mcp_search" }), ctx())
            .await
            .unwrap();
        let text = text_of(&result);
        // The MCP tool's full inputSchema is served (both properties present).
        assert!(
            text.contains("\"query\"") && text.contains("\"limit\""),
            "MCP full inputSchema must be served: {text}"
        );
    }

    #[tokio::test]
    async fn catalog_miss_falls_back_to_static_map() {
        // A catalog is installed but does NOT contain revert_to_state; the static
        // hand-written REVERT_HELP manual is still served (fallback preserved).
        let mut snapshot = ToolCatalogSnapshot::new();
        snapshot.insert(
            "foo".to_string(),
            ToolCatalogEntry {
                detailed: Some("foo body".into()),
                schema: serde_json::json!({"type": "object"}),
            },
        );
        let help = ToolHelpTool::with_default_help().with_catalog(snapshot);
        let result = help
            .execute(serde_json::json!({ "tool_name": "revert_to_state" }), ctx())
            .await
            .unwrap();
        let text = text_of(&result);
        assert!(
            text.contains("TREE of nodes"),
            "static-map fallback must still serve REVERT_HELP: {text}"
        );
    }
}
