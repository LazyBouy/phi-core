//! KC-05 (#109) — progressive tool-catalog disclosure.
//!
//! Tier B — the golden OFF-default wire is byte-identical to the historical full
//! render; the progressive-ON wire carries `short_description()` + a
//! `{"type":"object"}` `parameters` stub with a measurable byte drop.
//! Tier C — the engage predicate (count threshold / MCP trigger / OFF-default).
//!
//! The golden-OFF test is the #1 back-compat guard: the reduced branch is
//! unreachable unless `enabled` AND the count/MCP threshold engage, so every
//! existing agent's turn-1 `tools[]` stays byte-for-byte unchanged.

use phi_core::agent_loop::{agent_loop, AgentLoopConfig, ProgressiveToolCatalog};
use phi_core::provider::{MockProvider, ModelConfig, StreamProvider, ToolDefinition};
use phi_core::tools::RevertTool;
use phi_core::*;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A tool with a real short/detailed split and a fat parameters schema — so the
/// reduced render produces a measurable byte drop vs the full schema.
struct FatTool {
    name: String,
}

#[async_trait::async_trait]
impl AgentTool for FatTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        "Fat"
    }
    fn description(&self) -> &str {
        "This is the FULL, long-form description carrying the entire mental model, \
         worked examples, and failure modes that the model would otherwise pay for \
         on every single turn even though it rarely needs the depth up front."
    }
    fn short_description(&self) -> &str {
        "Lean one-liner; call tool_help for the full manual."
    }
    fn detailed_description(&self) -> Option<&str> {
        Some("The full manual body lives here and is served on demand.")
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "alpha": {"type": "string", "description": "a fairly long property description to add bytes"},
                "beta": {"type": "integer", "description": "another verbose property description here"},
                "gamma": {"type": "boolean", "description": "yet another property to fatten the schema up"},
                "delta": {"type": "array", "items": {"type": "string"}, "description": "an array property"}
            },
            "required": ["alpha", "beta"]
        })
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        Ok(ToolResult {
            content: vec![Content::Text { text: "ok".into() }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

/// A tool that reports itself as an MCP adapter (for the `engage_on_large_schema` path).
struct FakeMcpTool;

#[async_trait::async_trait]
impl AgentTool for FakeMcpTool {
    fn name(&self) -> &str {
        "mcp_tool"
    }
    fn label(&self) -> &str {
        "MCP"
    }
    fn description(&self) -> &str {
        "a remote MCP tool with a large inputSchema"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object", "properties": {}})
    }
    fn has_large_schema(&self) -> bool {
        true
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

fn make_config(
    provider: Arc<dyn StreamProvider>,
    progressive: ProgressiveToolCatalog,
) -> AgentLoopConfig {
    AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(provider),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: None,
        execution_limits: None,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        tool_timeout: None,
        response_format: phi_core::provider::ResponseFormat::Text,
        provider_wire_sink: None,
        progressive_tool_catalog: progressive,
        retry_config: phi_core::RetryConfig::default(),
        before_turn: None,
        after_turn: None,
        on_error: None,
        before_loop: None,
        after_loop: None,
        before_tool_execution: None,
        after_tool_execution: None,
        before_tool_execution_update: None,
        after_tool_execution_update: None,
        before_compaction_start: None,
        after_compaction_end: None,
        input_filters: vec![],
        first_turn_trigger: TurnTrigger::User,
        config_id: None,
        context_translation: None,
        prun_pending: None,
        revert_pending: None,
        current_tool: None,
        revert_render_policy: phi_core::RevertRenderPolicy::default(),
    }
}

fn fresh_context(tools: Vec<Arc<dyn AgentTool>>) -> AgentContext {
    AgentContext {
        system_prompt: "system".into(),
        messages: Vec::new(),
        tools,
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
        active_node_id: None,
        next_node_id: 0,
    }
}

fn fat_tools(n: usize) -> Vec<Arc<dyn AgentTool>> {
    (0..n)
        .map(|i| {
            Arc::new(FatTool {
                name: format!("tool_{i}"),
            }) as Arc<dyn AgentTool>
        })
        .collect()
}

/// Run one turn and return the turn-0 `TurnRequest.payload.tools` (the exact
/// wire tool catalog the model would receive).
async fn capture_wire_tools(
    tools: Vec<Arc<dyn AgentTool>>,
    progressive: ProgressiveToolCatalog,
) -> Vec<ToolDefinition> {
    let provider = Arc::new(MockProvider::text("ok"));
    let config = make_config(provider, progressive);
    let mut context = fresh_context(tools);
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let mut payload_tools = None;
    while let Ok(e) = rx.try_recv() {
        if let AgentEvent::TurnRequest { payload, .. } = e {
            payload_tools = Some(payload.tools.clone());
            break;
        }
    }
    payload_tools.expect("expected a TurnRequest event with tool defs")
}

/// The historical full-render form, computed directly from the tools — the
/// golden reference for byte-identity.
fn expected_full_tools(tools: &[Arc<dyn AgentTool>]) -> Vec<ToolDefinition> {
    tools
        .iter()
        .map(|t| ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tier B — golden OFF + reduced ON + byte drop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn golden_off_wire_is_byte_identical() {
    // Default (OFF) config: even with 10 tools (> min_tools), the reduced branch
    // is unreachable, so the wire must equal the historical full render exactly.
    let tools = fat_tools(10);
    let expected = expected_full_tools(&tools);
    let wire = capture_wire_tools(tools, ProgressiveToolCatalog::default()).await;

    let wire_json = serde_json::to_value(&wire).unwrap();
    let expected_json = serde_json::to_value(&expected).unwrap();
    assert_eq!(
        wire_json, expected_json,
        "OFF-default turn-1 tools[] must be byte-identical to the full render"
    );
}

#[tokio::test]
async fn progressive_on_reduces_render() {
    // Enabled + 10 tools (> min_tools 8) → engaged. Each entry carries the
    // short_description + a {"type":"object"} stub instead of the full schema.
    let tools = fat_tools(10);
    let wire = capture_wire_tools(
        tools,
        ProgressiveToolCatalog {
            enabled: true,
            min_tools: 8,
            engage_on_large_schema: true,
        },
    )
    .await;

    let stub = serde_json::json!({"type": "object"});
    for def in &wire {
        assert_eq!(
            def.description, "Lean one-liner; call tool_help for the full manual.",
            "reduced entry must use short_description()"
        );
        assert_eq!(
            def.parameters, stub,
            "reduced entry must carry the minimal-valid parameters stub"
        );
    }
}

#[tokio::test]
async fn progressive_on_drops_bytes() {
    let tools = fat_tools(10);
    let full = capture_wire_tools(tools.clone(), ProgressiveToolCatalog::default()).await;
    let reduced = capture_wire_tools(
        tools,
        ProgressiveToolCatalog {
            enabled: true,
            min_tools: 8,
            engage_on_large_schema: true,
        },
    )
    .await;

    let full_bytes = serde_json::to_string(&full).unwrap().len();
    let reduced_bytes = serde_json::to_string(&reduced).unwrap().len();
    assert!(
        reduced_bytes < full_bytes,
        "reduced wire ({reduced_bytes} bytes) must be smaller than full ({full_bytes} bytes)"
    );
}

// ---------------------------------------------------------------------------
// Tier C — engage predicate (count threshold / MCP trigger / OFF-default)
// ---------------------------------------------------------------------------

#[test]
fn engages_below_threshold_is_false() {
    // Enabled but only 4 tools (<= min_tools 8) and no MCP → inert (full render).
    let cfg = ProgressiveToolCatalog {
        enabled: true,
        min_tools: 8,
        engage_on_large_schema: true,
    };
    let tools = fat_tools(4);
    assert!(!cfg.engages(&tools));
}

#[test]
fn engages_above_threshold_is_true() {
    // Enabled + 10 tools (> min_tools 8) → engaged.
    let cfg = ProgressiveToolCatalog {
        enabled: true,
        min_tools: 8,
        engage_on_large_schema: true,
    };
    let tools = fat_tools(10);
    assert!(cfg.engages(&tools));
}

#[test]
fn engages_on_large_schema_below_count_and_disabled_never() {
    // 2 tools (below count) but one is an MCP adapter → engages via engage_on_large_schema.
    let with_mcp: Vec<Arc<dyn AgentTool>> = vec![
        Arc::new(FatTool { name: "t0".into() }),
        Arc::new(FakeMcpTool),
    ];
    let mcp_on = ProgressiveToolCatalog {
        enabled: true,
        min_tools: 8,
        engage_on_large_schema: true,
    };
    assert!(
        mcp_on.engages(&with_mcp),
        "MCP tool below count must engage"
    );

    // engage_on_large_schema: false → the MCP tool no longer triggers below the count.
    let mcp_off = ProgressiveToolCatalog {
        enabled: true,
        min_tools: 8,
        engage_on_large_schema: false,
    };
    assert!(!mcp_off.engages(&with_mcp));

    // Disabled → never engages, even above the count / with MCP.
    let disabled = ProgressiveToolCatalog {
        enabled: false,
        ..Default::default()
    };
    assert!(!disabled.engages(&fat_tools(20)));
    assert!(!disabled.engages(&with_mcp));
}

// ---------------------------------------------------------------------------
// Close-gate — representative render (KC-05 disposition on a REAL agent turn).
//
// phi-core is daemon-less and no consumer can enable progressive yet, so the
// ON-path is exercised at the kernel level here. This renders the actual turn-1
// `tools[]` wire (`AgentEvent::TurnRequest.payload.tools`) for a representative
// agent carrying the REAL `RevertTool` (whose 1870-char manual is the headline
// magnification case) + a large-schema MCP tool + fat tools, and ASSERTS THE
// DISPOSITION — not merely that it renders. The live-model wire read (a real
// provider driving the reduced catalog end-to-end) is OWNED by the i-phi consumer
// follow-on that flips the flag on the product daemon
// (`[[feedback_render_transcript_close_gate]]` Rule 6). See close-gate-result.md.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn close_gate_real_revert_manual_absent_and_wire_reduced() {
    // Representative agent: the REAL RevertTool + a large-schema MCP tool + enough
    // fat tools to clear the count threshold.
    let revert: Arc<dyn AgentTool> = Arc::new(RevertTool::new(Arc::new(Mutex::new(Vec::new()))));
    let mut tools: Vec<Arc<dyn AgentTool>> = vec![revert, Arc::new(FakeMcpTool)];
    for i in 0..8 {
        tools.push(Arc::new(FatTool {
            name: format!("tool{i}"),
        }));
    }

    // A fragment unique to REVERT_DETAILED (the full manual) — never in the short
    // one-liner. On PRE-KC-05 code this WAS `description()` and would ride the wire.
    let manual_needle = "The conversation is a TREE of nodes";

    let off = capture_wire_tools(tools.clone(), ProgressiveToolCatalog::default()).await;
    let off_json = serde_json::to_string(&off).unwrap();

    let on = capture_wire_tools(
        tools,
        ProgressiveToolCatalog {
            enabled: true,
            min_tools: 8,
            engage_on_large_schema: true,
        },
    )
    .await;
    let on_json = serde_json::to_string(&on).unwrap();

    // R1 — the full revert manual is NOT eager on the wire (the fix's headline).
    assert!(
        !on_json.contains(manual_needle),
        "revert_to_state's full manual must not be on the progressive wire"
    );
    // R2 — the revert_to_state entry carries the SHORT one-liner + the stub params.
    let revert_def = on
        .iter()
        .find(|d| d.name == "revert_to_state")
        .expect("revert_to_state must be on the wire");
    assert!(
        revert_def.description.chars().count() <= 256,
        "revert wire description must be the short one-liner (got {} chars)",
        revert_def.description.chars().count()
    );
    assert!(
        revert_def
            .description
            .contains("tool_help(\"revert_to_state\")"),
        "the short one-liner must point the model at tool_help"
    );
    // R3 — every catalog entry is reduced to the stub on the wire.
    let stub = serde_json::json!({"type": "object"});
    for d in &on {
        assert_eq!(d.parameters, stub, "{} not reduced to the stub", d.name);
    }
    // R4 — measurable byte drop vs the full render.
    assert!(
        on_json.len() < off_json.len(),
        "progressive wire ({} bytes) must be smaller than full ({} bytes)",
        on_json.len(),
        off_json.len()
    );
}
