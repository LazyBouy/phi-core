//! CC-10a kernel keystone: `before_tool_execution` returning `ToolGate::Deny { reason }`
//! lands the supplied `reason` in the synthetic `ToolResult` the model receives,
//! instead of the historical opaque skip string — so the model is told *why* a
//! tool was blocked and can self-correct.
//!
//! Also verifies the back-compat default path: a `Deny` carrying
//! `ToolGate::DEFAULT_DENY_REASON` reproduces the historical string byte-for-byte.

use std::sync::Arc;

use phi_core::agent_loop::{agent_loop, AgentLoopConfig, ToolGate};
use phi_core::provider::mock::{MockResponse, MockToolCall};
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::types::{CacheConfig, ThinkingLevel, ToolExecutionStrategy, TurnTrigger};
use phi_core::{AgentContext, AgentEvent, AgentMessage, Content, LlmMessage, Message};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

fn make_config(provider: Arc<dyn phi_core::provider::StreamProvider>) -> AgentLoopConfig {
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
        progressive_tool_catalog: Default::default(),
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

fn collect_events(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

fn fresh_context() -> AgentContext {
    AgentContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(phi_core::tools::BashTool::default())],
        agent_id: Some("test".to_string()),
        session_id: Some("test".to_string()),
        loop_id: Some("test.loop.1".to_string()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
        active_node_id: None,
        next_node_id: 0,
    }
}

/// Returns the text of the FIRST `ToolResult` message (with its `is_error` flag)
/// emitted into the event stream, by inspecting `MessageEnd` events.
fn first_tool_result_text(events: &[AgentEvent]) -> Option<(String, bool)> {
    events.iter().find_map(|e| {
        let AgentEvent::MessageEnd {
            message: AgentMessage::Llm(llm),
            ..
        } = e
        else {
            return None;
        };
        let Message::ToolResult {
            content, is_error, ..
        } = &llm.message
        else {
            return None;
        };
        let text = content.iter().find_map(|c| match c {
            Content::Text { text } => Some(text.clone()),
            _ => None,
        })?;
        Some((text, *is_error))
    })
}

/// A `before_tool_execution` returning `ToolGate::Deny { reason }` puts the supplied
/// reason into the model's `tool_result` (NOT the opaque default), marks it as an
/// error, and does NOT emit a `ToolExecutionStart` event (hook-ordering invariant).
#[tokio::test]
async fn test_deny_reason_reaches_tool_result() {
    const SENTINEL: &str = "permission denied: tool `bash` not in your allow-list";

    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "echo hi"}),
        }]),
        MockResponse::Text("done".to_string()),
    ]));
    let mut config = make_config(provider);
    config.before_tool_execution = Some(Arc::new(move |_name, _id, _args| {
        Box::pin(async move { ToolGate::deny(SENTINEL) })
    }));

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let msg = AgentMessage::Llm(LlmMessage::new(Message::user("run echo hi")));
    let mut context = fresh_context();

    agent_loop(vec![msg], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    // The denied tool_result carries the supplied reason, marked as an error.
    let (text, is_error) =
        first_tool_result_text(&events).expect("a ToolResult message should be emitted on deny");
    assert!(
        text.contains(SENTINEL),
        "denied tool_result must carry the Deny reason; got: {text:?}"
    );
    assert!(is_error, "denied tool_result must be flagged is_error");

    // It must NOT be the historical opaque default string.
    assert!(
        !text.contains(ToolGate::DEFAULT_DENY_REASON),
        "denied tool_result must not fall back to the opaque default when a reason is supplied"
    );

    // Hook-ordering invariant: a denied tool emits no ToolExecutionStart / End.
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionStart { .. })),
        "a denied tool call must not emit ToolExecutionStart"
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. })),
        "a denied tool call must not emit ToolExecutionEnd"
    );

    // MessageStart/MessageEnd for the synthetic result are still emitted.
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::MessageStart { .. })),
        "the synthetic denied ToolResult must still emit MessageStart"
    );
}

/// Back-compat: a `Deny` carrying `ToolGate::DEFAULT_DENY_REASON` reproduces the
/// historical opaque skip string byte-for-byte (the documented default path).
#[tokio::test]
async fn test_deny_default_reason_matches_historical_string() {
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "echo hi"}),
        }]),
        MockResponse::Text("done".to_string()),
    ]));
    let mut config = make_config(provider);
    config.before_tool_execution = Some(Arc::new(move |_name, _id, _args| {
        Box::pin(async move { ToolGate::deny(ToolGate::DEFAULT_DENY_REASON) })
    }));

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let msg = AgentMessage::Llm(LlmMessage::new(Message::user("run echo hi")));
    let mut context = fresh_context();

    agent_loop(vec![msg], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    let (text, is_error) = first_tool_result_text(&events)
        .expect("a ToolResult message should be emitted on default-reason deny");
    assert_eq!(
        text, "Tool execution skipped by before_tool_execution hook.",
        "the default deny reason must reproduce the historical opaque string byte-for-byte"
    );
    assert!(is_error);
}
