//! Tests for per-tool execution timeout (MEDIUM-5).
//!
//! Verifies that `AgentLoopConfig.tool_timeout` and the per-tool `AgentTool::timeout()`
//! override bound a misbehaving tool's execution time, surface `ToolError::Timeout` as
//! a structured tool result, and keep the agent loop alive for the next turn.

use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
use phi_core::provider::mock::*;
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::*;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// A test tool that sleeps for a configurable duration before returning success.
struct SleepyTool {
    name: String,
    sleep: Duration,
    /// Per-tool override returned by `AgentTool::timeout()`. None to defer to config.
    override_timeout: Option<Duration>,
}

#[async_trait::async_trait]
impl AgentTool for SleepyTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        "Sleepy"
    }
    fn description(&self) -> &str {
        "Test tool that sleeps."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        tokio::time::sleep(self.sleep).await;
        Ok(ToolResult {
            content: vec![Content::Text {
                text: "slept ok".into(),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
    fn timeout(&self) -> Option<Duration> {
        self.override_timeout
    }
}

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
    }
}

fn make_context(tool: Arc<dyn AgentTool>) -> AgentContext {
    AgentContext {
        system_prompt: "You are helpful.".into(),
        messages: Vec::new(),
        tools: vec![tool],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    }
}

fn find_tool_result(msgs: &[AgentMessage]) -> Option<(&Vec<Content>, bool)> {
    msgs.iter().find_map(|m| match m.as_llm() {
        Some(Message::ToolResult {
            content, is_error, ..
        }) => Some((content, *is_error)),
        _ => None,
    })
}

#[tokio::test]
async fn tool_exceeding_config_timeout_returns_error_result() {
    // Provider returns ONE tool call, then a final text. After the timeout the LLM gets the
    // synthetic error result back and produces its concluding turn.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "sleepy".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("recovered from timeout".into()),
    ]);

    let tool = SleepyTool {
        name: "sleepy".into(),
        sleep: Duration::from_secs(30),
        override_timeout: None,
    };
    let mut config = make_config(Arc::new(provider));
    config.tool_timeout = Some(Duration::from_millis(100));

    let mut ctx = make_context(Arc::new(tool));
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("run sleepy")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let start = Instant::now();
    let new_messages = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;
    let elapsed = start.elapsed();

    // Wall-clock must reflect the timeout — not the 30s tool sleep.
    assert!(
        elapsed < Duration::from_secs(2),
        "agent_loop should have returned promptly after tool timeout; took {:?}",
        elapsed
    );

    // The ToolResult message should carry is_error=true with the "Timeout" string.
    let (content, is_error) =
        find_tool_result(&new_messages).expect("a ToolResult message must be present");
    assert!(is_error, "tool result must be flagged as error on timeout");
    let text = match &content[0] {
        Content::Text { text } => text.clone(),
        other => panic!("expected Content::Text, got {:?}", other),
    };
    assert!(
        text.contains("timeout") || text.contains("Timeout"),
        "tool result text should mention timeout, got {:?}",
        text
    );

    // Agent loop must continue and produce the final assistant text turn after the timeout.
    let last = new_messages.last().expect("at least one final message");
    assert_eq!(last.role(), "assistant");
}

#[tokio::test]
async fn tool_override_timeout_beats_config_default() {
    // Config: 5s. Tool: 50ms. Tool override should win and the timeout fires fast.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "sleepy".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("done".into()),
    ]);

    let tool = SleepyTool {
        name: "sleepy".into(),
        sleep: Duration::from_secs(10),
        override_timeout: Some(Duration::from_millis(50)),
    };
    let mut config = make_config(Arc::new(provider));
    config.tool_timeout = Some(Duration::from_secs(5));

    let mut ctx = make_context(Arc::new(tool));
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("run sleepy")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let start = Instant::now();
    let _ = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;
    let elapsed = start.elapsed();

    // Per-tool override (50ms) must dominate over config-level 5s.
    assert!(
        elapsed < Duration::from_secs(1),
        "per-tool override should have fired in ~50ms; took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn tool_within_timeout_succeeds_normally() {
    // Config: 1s. Tool sleeps 10ms. Result should be the success path.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "sleepy".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("done".into()),
    ]);

    let tool = SleepyTool {
        name: "sleepy".into(),
        sleep: Duration::from_millis(10),
        override_timeout: None,
    };
    let mut config = make_config(Arc::new(provider));
    config.tool_timeout = Some(Duration::from_secs(1));

    let mut ctx = make_context(Arc::new(tool));
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("run sleepy")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;
    let (content, is_error) =
        find_tool_result(&new_messages).expect("a ToolResult message must be present");
    assert!(
        !is_error,
        "tool result should NOT be an error on fast success"
    );
    match &content[0] {
        Content::Text { text } => assert_eq!(text, "slept ok"),
        other => panic!("expected Content::Text, got {:?}", other),
    }
}

#[tokio::test]
async fn tool_no_timeout_preserves_legacy_unbounded_behavior() {
    // tool_timeout = None AND no per-tool override → tool runs to completion (no timeout).
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "sleepy".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("done".into()),
    ]);

    let tool = SleepyTool {
        name: "sleepy".into(),
        sleep: Duration::from_millis(20),
        override_timeout: None,
    };
    let config = make_config(Arc::new(provider)); // tool_timeout: None by default

    let mut ctx = make_context(Arc::new(tool));
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("run sleepy")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;
    let (_, is_error) =
        find_tool_result(&new_messages).expect("a ToolResult message must be present");
    assert!(!is_error);
}
