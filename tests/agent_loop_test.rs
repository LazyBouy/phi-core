//! Tests for the core agent loop using MockProvider.

use phi_core::agent_loop::evaluation::{
    ElaborateEvaluation, PickFirstEvaluation, TokenEfficientEvaluation, TransparentEvaluation,
};
use phi_core::agent_loop::{agent_loop, agent_loop_continue, agent_loop_parallel, AgentLoopConfig};
use phi_core::provider::mock::*;
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::*;
use std::sync::Arc;
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

fn collect_events(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

#[tokio::test]
async fn test_simple_text_response() {
    let provider = MockProvider::text("Hello, world!");
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);

    // Should have: AgentStart, TurnStart, MessageStart(user), MessageEnd(user),
    //              MessageStart(assistant), MessageEnd(assistant), TurnEnd, AgentEnd
    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            AgentEvent::AgentStart { .. } => "AgentStart",
            AgentEvent::AgentEnd { .. } => "AgentEnd",
            AgentEvent::TurnStart { .. } => "TurnStart",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::MessageStart { .. } => "MessageStart",
            AgentEvent::MessageEnd { .. } => "MessageEnd",
            AgentEvent::MessageUpdate { .. } => "MessageUpdate",
            AgentEvent::ToolExecutionStart { .. } => "ToolExecStart",
            AgentEvent::ToolExecutionUpdate { .. } => "ToolExecUpdate",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecEnd",
            AgentEvent::ProgressMessage { .. } => "ProgressMessage",
            AgentEvent::InputRejected { .. } => "InputRejected",
            AgentEvent::ParallelLoopStart { .. } => "ParallelLoopStart",
            AgentEvent::ParallelLoopEnd { .. } => "ParallelLoopEnd",
            AgentEvent::CompactionStarted { .. } => "CompactionStarted",
            AgentEvent::CompactionEnded { .. } => "CompactionEnded",
            AgentEvent::PrunApplied { .. } => "PrunApplied",
        })
        .collect();

    assert!(event_types.contains(&"AgentStart"));
    assert!(event_types.contains(&"AgentEnd"));
    assert!(event_types.contains(&"TurnStart"));
    assert!(event_types.contains(&"TurnEnd"));

    // new_messages should contain user prompt + assistant response
    assert_eq!(new_messages.len(), 2);
    assert_eq!(new_messages[0].role(), "user");
    assert_eq!(new_messages[1].role(), "assistant");

    // Context should have both messages
    assert_eq!(context.messages.len(), 2);
}

#[tokio::test]
async fn test_tool_call_and_response() {
    // Mock: first call returns tool use, second returns text
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "read_file".into(),
            arguments: serde_json::json!({"path": "test.txt"}),
        }]),
        MockResponse::Text("The file contains: hello".into()),
    ]);

    // Define a simple tool
    struct ReadFileTool;

    #[async_trait::async_trait]
    impl AgentTool for ReadFileTool {
        fn name(&self) -> &str {
            "read_file"
        }
        fn label(&self) -> &str {
            "Read File"
        }
        fn description(&self) -> &str {
            "Read a file"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({
                "type": "object",
                "properties": {
                    "path": {"type": "string"}
                }
            })
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![Content::Text {
                    text: "hello".into(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "You are helpful.".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ReadFileTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Read test.txt")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);

    let event_types: Vec<&str> = events
        .iter()
        .map(|e| match e {
            AgentEvent::AgentStart { .. } => "AgentStart",
            AgentEvent::AgentEnd { .. } => "AgentEnd",
            AgentEvent::TurnStart { .. } => "TurnStart",
            AgentEvent::TurnEnd { .. } => "TurnEnd",
            AgentEvent::MessageStart { .. } => "MessageStart",
            AgentEvent::MessageEnd { .. } => "MessageEnd",
            AgentEvent::MessageUpdate { .. } => "MessageUpdate",
            AgentEvent::ToolExecutionStart { .. } => "ToolExecStart",
            AgentEvent::ToolExecutionUpdate { .. } => "ToolExecUpdate",
            AgentEvent::ToolExecutionEnd { .. } => "ToolExecEnd",
            AgentEvent::ProgressMessage { .. } => "ProgressMessage",
            AgentEvent::InputRejected { .. } => "InputRejected",
            AgentEvent::ParallelLoopStart { .. } => "ParallelLoopStart",
            AgentEvent::ParallelLoopEnd { .. } => "ParallelLoopEnd",
            AgentEvent::CompactionStarted { .. } => "CompactionStarted",
            AgentEvent::CompactionEnded { .. } => "CompactionEnded",
            AgentEvent::PrunApplied { .. } => "PrunApplied",
        })
        .collect();

    // Should have tool execution events
    assert!(event_types.contains(&"ToolExecStart"));
    assert!(event_types.contains(&"ToolExecEnd"));

    // Messages: user, assistant(tool_call), toolResult, assistant(text)
    assert_eq!(new_messages.len(), 4);
    assert_eq!(new_messages[0].role(), "user");
    assert_eq!(new_messages[1].role(), "assistant");
    assert_eq!(new_messages[2].role(), "toolResult");
    assert_eq!(new_messages[3].role(), "assistant");
}

#[tokio::test]
async fn test_abort_cancels_loop() {
    // Provider that returns text — but we cancel before it runs
    let provider = MockProvider::text("Should not appear");
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    // Cancel immediately
    cancel.cancel();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Should have user message but loop should exit early
    // The prompt is added before the loop checks cancellation
    assert!(new_messages.len() <= 2); // user + possibly error
}

#[tokio::test]
async fn test_continue_from_tool_result() {
    let provider = MockProvider::text("Done processing.");
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("do something"))),
            AgentMessage::Llm(LlmMessage::new(Message::ToolResult {
                tool_call_id: "tc-1".into(),
                tool_name: "test_tool".into(),
                content: vec![Content::Text {
                    text: "result".into(),
                }],
                is_error: false,
                timestamp: 0,
            })),
        ],
        tools: Vec::new(),
        // agent_loop_continue requires agent_id and session_id to be set
        agent_id: Some("test-agent".into()),
        session_id: Some("test-session".into()),
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop_continue(&mut context, &config, tx, cancel).await;

    assert!(!new_messages.is_empty());
    assert_eq!(new_messages[0].role(), "assistant");
}

#[tokio::test]
async fn test_tool_error_is_reported() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "failing_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Tool failed, sorry.".into()),
    ]);

    struct FailingTool;

    #[async_trait::async_trait]
    impl AgentTool for FailingTool {
        fn name(&self) -> &str {
            "failing_tool"
        }
        fn label(&self) -> &str {
            "Failing Tool"
        }
        fn description(&self) -> &str {
            "Always fails"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Err(ToolError::Failed("Something went wrong".into()))
        }
    }

    let config = make_config(Arc::new(provider));
    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(FailingTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Use the tool")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);

    // Tool error should be reported
    let tool_end_events: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { is_error: true, .. }))
        .collect();
    assert_eq!(tool_end_events.len(), 1);

    // Should still get a final assistant response
    assert_eq!(new_messages.last().unwrap().role(), "assistant");
}

#[tokio::test]
async fn test_unknown_tool_reports_error() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "nonexistent".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("I couldn't find that tool.".into()),
    ]);

    let config = make_config(Arc::new(provider));
    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(), // No tools registered
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Use nonexistent tool")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let _new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);
    let tool_errors: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { is_error: true, .. }))
        .collect();
    assert_eq!(tool_errors.len(), 1);
}

// ---------------------------------------------------------------------------
// Parallel tool execution tests
// ---------------------------------------------------------------------------

/// A tool that records execution timestamps to verify parallelism.
struct TimedTool {
    name: String,
    delay_ms: u64,
}

#[async_trait::async_trait]
impl AgentTool for TimedTool {
    fn name(&self) -> &str {
        &self.name
    }
    fn label(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        "Timed tool"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        tokio::time::sleep(std::time::Duration::from_millis(self.delay_ms)).await;
        Ok(ToolResult {
            content: vec![Content::Text {
                text: format!("done:{}", self.name),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

#[tokio::test]
async fn test_parallel_tool_execution_faster_than_sequential() {
    // 3 tools each taking 50ms. Sequential = 150ms+, Parallel = ~50ms.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![
            MockToolCall {
                name: "tool_a".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_b".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_c".into(),
                arguments: serde_json::json!({}),
            },
        ]),
        MockResponse::Text("All done.".into()),
    ]);

    let mut config = make_config(Arc::new(provider));
    config.tool_execution = ToolExecutionStrategy::Parallel;

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(TimedTool {
                name: "tool_a".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_b".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_c".into(),
                delay_ms: 50,
            }),
        ],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Run all tools")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let start = std::time::Instant::now();
    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let elapsed = start.elapsed();

    let events = collect_events(rx);

    // All 3 tool results should be present
    let tool_results: Vec<_> = new_messages
        .iter()
        .filter(|m| m.role() == "toolResult")
        .collect();
    assert_eq!(tool_results.len(), 3);

    // Should complete in roughly 50-100ms, not 150ms+
    assert!(
        elapsed.as_millis() < 130,
        "Parallel execution took {}ms, expected <130ms",
        elapsed.as_millis()
    );

    // Should have 3 ToolExecutionStart and 3 ToolExecutionEnd events
    let starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionStart { .. }))
        .count();
    let ends = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ToolExecutionEnd { .. }))
        .count();
    assert_eq!(starts, 3);
    assert_eq!(ends, 3);
}

#[tokio::test]
async fn test_sequential_tool_execution_is_slower() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![
            MockToolCall {
                name: "tool_a".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_b".into(),
                arguments: serde_json::json!({}),
            },
        ]),
        MockResponse::Text("Done.".into()),
    ]);

    let mut config = make_config(Arc::new(provider));
    config.tool_execution = ToolExecutionStrategy::Sequential;

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(TimedTool {
                name: "tool_a".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_b".into(),
                delay_ms: 50,
            }),
        ],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Run tools")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let start = std::time::Instant::now();
    let _new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let elapsed = start.elapsed();

    // Sequential should take 100ms+ (2 × 50ms)
    assert!(
        elapsed.as_millis() >= 95,
        "Sequential execution took {}ms, expected >=95ms",
        elapsed.as_millis()
    );
}

#[tokio::test]
async fn test_batched_tool_execution() {
    // 4 tools, batch size 2: two batches of 2
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![
            MockToolCall {
                name: "tool_a".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_b".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_c".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "tool_d".into(),
                arguments: serde_json::json!({}),
            },
        ]),
        MockResponse::Text("All done.".into()),
    ]);

    let mut config = make_config(Arc::new(provider));
    config.tool_execution = ToolExecutionStrategy::Batched { size: 2 };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(TimedTool {
                name: "tool_a".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_b".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_c".into(),
                delay_ms: 50,
            }),
            Arc::new(TimedTool {
                name: "tool_d".into(),
                delay_ms: 50,
            }),
        ],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Run all tools")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let start = std::time::Instant::now();
    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let elapsed = start.elapsed();

    let _events = collect_events(rx);

    // All 4 results present
    let tool_results: Vec<_> = new_messages
        .iter()
        .filter(|m| m.role() == "toolResult")
        .collect();
    assert_eq!(tool_results.len(), 4);

    // 2 batches × 50ms = ~100ms (not 200ms sequential, not 50ms full parallel)
    assert!(
        elapsed.as_millis() >= 90 && elapsed.as_millis() < 160,
        "Batched execution took {}ms, expected 90-160ms",
        elapsed.as_millis()
    );
}

// ---------------------------------------------------------------------------
// Streaming tool output (on_update callback) tests
// ---------------------------------------------------------------------------

/// A tool that emits progress updates via on_update callback.
struct ProgressTool;

#[async_trait::async_trait]
impl AgentTool for ProgressTool {
    fn name(&self) -> &str {
        "progress_tool"
    }
    fn label(&self) -> &str {
        "Progress"
    }
    fn description(&self) -> &str {
        "A tool that streams progress"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }

    async fn execute(
        &self,
        _params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        for i in 1..=3 {
            if let Some(ref cb) = ctx.on_update {
                cb(ToolResult {
                    content: vec![Content::Text {
                        text: format!("step {}/3", i),
                    }],
                    details: serde_json::Value::Null,
                    child_loop_id: None,
                });
            }
        }
        Ok(ToolResult {
            content: vec![Content::Text {
                text: "done".into(),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

#[tokio::test]
async fn test_tool_execution_update_events_emitted() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("All done.".into()),
    ]);

    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ProgressTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);

    let updates: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionUpdate { partial_result, .. } => {
                if let Some(Content::Text { text }) = partial_result.content.first() {
                    Some(text.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert_eq!(updates, vec!["step 1/3", "step 2/3", "step 3/3"]);
}

// ---------------------------------------------------------------------------
// Retry with backoff tests
// ---------------------------------------------------------------------------

/// A provider that fails N times with a given error, then delegates to a MockProvider.
struct FailThenSucceedProvider {
    fail_count: std::sync::atomic::AtomicUsize,
    max_failures: usize,
    error: ProviderError,
    inner: MockProvider,
}

use phi_core::provider::{ProviderError, StreamConfig, StreamEvent, StreamProvider};

#[async_trait::async_trait]
impl StreamProvider for FailThenSucceedProvider {
    fn provider_id(&self) -> &str {
        "mock"
    }
    async fn stream(
        &self,
        config: StreamConfig,
        tx: tokio::sync::mpsc::UnboundedSender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<phi_core::Message, ProviderError> {
        let attempt = self
            .fail_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if attempt < self.max_failures {
            return Err(match &self.error {
                ProviderError::RateLimited { retry_after_ms } => ProviderError::RateLimited {
                    retry_after_ms: *retry_after_ms,
                },
                ProviderError::Network(msg) => ProviderError::Network(msg.clone()),
                ProviderError::Auth(msg) => ProviderError::Auth(msg.clone()),
                other => ProviderError::Other(other.to_string()),
            });
        }
        self.inner.stream(config, tx, cancel).await
    }
}

#[tokio::test]
async fn test_retry_on_rate_limit_succeeds() {
    let provider = Arc::new(FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicUsize::new(0),
        max_failures: 2,
        error: ProviderError::RateLimited {
            retry_after_ms: Some(10), // 10ms for fast tests
        },
        inner: MockProvider::text("Success after retries"),
    });

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(
            Arc::clone(&provider) as Arc<dyn phi_core::provider::StreamProvider>
        ),
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
        retry_config: phi_core::RetryConfig {
            max_retries: 3,
            initial_delay_ms: 10,
            backoff_multiplier: 2.0,
            max_delay_ms: 100,
        },
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
    };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Should have succeeded after 2 failures + 1 success
    assert_eq!(new_messages.len(), 2); // user + assistant
    let events = collect_events(rx);
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));

    // Verify the provider was called 3 times (2 failures + 1 success)
    assert_eq!(
        provider
            .fail_count
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );
}

#[tokio::test]
async fn test_retry_exhausted_returns_error() {
    let provider = Arc::new(FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicUsize::new(0),
        max_failures: 10, // more failures than retries
        error: ProviderError::Network("connection reset".into()),
        inner: MockProvider::text("never reached"),
    });

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(
            Arc::clone(&provider) as Arc<dyn phi_core::provider::StreamProvider>
        ),
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
        retry_config: phi_core::RetryConfig {
            max_retries: 2,
            initial_delay_ms: 10,
            backoff_multiplier: 2.0,
            max_delay_ms: 100,
        },
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
    };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Should have an error message (StopReason::Error)
    let last = new_messages.last().unwrap();
    if let AgentMessage::Llm(LlmMessage {
        message:
            Message::Assistant {
                stop_reason,
                error_message,
                ..
            },
        ..
    }) = last
    {
        assert_eq!(*stop_reason, StopReason::Error);
        assert!(error_message.as_ref().unwrap().contains("connection reset"));
    } else {
        panic!("Expected error assistant message");
    }

    // 1 initial + 2 retries = 3 attempts
    assert_eq!(
        provider
            .fail_count
            .load(std::sync::atomic::Ordering::SeqCst),
        3
    );
}

#[tokio::test]
async fn test_no_retry_on_auth_error() {
    let provider = Arc::new(FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicUsize::new(0),
        max_failures: 1,
        error: ProviderError::Auth("invalid key".into()),
        inner: MockProvider::text("never reached"),
    });

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(
            Arc::clone(&provider) as Arc<dyn phi_core::provider::StreamProvider>
        ),
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
        retry_config: phi_core::RetryConfig::default(), // 3 retries, but auth is not retryable
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
    };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Should have been called exactly once — no retries for auth errors
    assert_eq!(
        provider
            .fail_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

#[tokio::test]
async fn test_retry_none_disables_retries() {
    let provider = Arc::new(FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicUsize::new(0),
        max_failures: 1,
        error: ProviderError::RateLimited {
            retry_after_ms: None,
        },
        inner: MockProvider::text("never reached"),
    });

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(
            Arc::clone(&provider) as Arc<dyn phi_core::provider::StreamProvider>
        ),
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
        retry_config: phi_core::RetryConfig::none(), // disabled
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
    };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Only 1 attempt — no retries
    assert_eq!(
        provider
            .fail_count
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
}

// ---------------------------------------------------------------------------
// Event streaming bug fix test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_message_update_events_emitted_during_streaming() {
    // This test verifies the fix for: text deltas not emitted because
    // partial_message was None when deltas arrived (MessageStart was only
    // emitted on Done, after all deltas had already been processed).
    let provider = MockProvider::text("Hello, world!");
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let events = collect_events(rx);

    // Collect MessageUpdate text deltas
    let deltas: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageUpdate {
                delta: StreamDelta::Text { delta },
                ..
            } => Some(delta.clone()),
            _ => None,
        })
        .collect();

    // Should have at least one text delta with "Hello, world!"
    assert!(
        !deltas.is_empty(),
        "Expected MessageUpdate events with text deltas, got none"
    );
    let full_text: String = deltas.into_iter().collect();
    assert_eq!(full_text, "Hello, world!");

    // Verify event ordering: MessageStart before MessageUpdate before MessageEnd
    let event_types: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::MessageStart { .. } => Some("Start"),
            AgentEvent::MessageUpdate { .. } => Some("Update"),
            AgentEvent::MessageEnd { .. } => Some("End"),
            _ => None,
        })
        .collect();

    // Should be: Start (user), End (user), Start (assistant), Update(s), End (assistant)
    // Find the assistant sequence
    let assistant_start = event_types.iter().rposition(|&e| e == "Start").unwrap();
    let assistant_end = event_types.iter().rposition(|&e| e == "End").unwrap();

    // All Updates should be between the last Start and last End
    for (i, &et) in event_types.iter().enumerate() {
        if et == "Update" {
            assert!(
                i > assistant_start && i < assistant_end,
                "MessageUpdate at index {} should be between MessageStart ({}) and MessageEnd ({})",
                i,
                assistant_start,
                assistant_end
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Lifecycle callback tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_before_turn_can_abort() {
    // Provider with 5 text responses, but before_turn aborts after 2 turns.
    // We need tool calls to keep the loop going for multiple turns.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        // These should never be reached
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Final".into()),
    ]);

    let turn_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let turn_count_clone = turn_count.clone();

    let mut config = make_config(Arc::new(provider));
    config.before_turn = Some(std::sync::Arc::new(move |_msgs, _turn| {
        let count = turn_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        count < 2 // Allow turns 0 and 1, abort on turn 2
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ProgressTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // before_turn was called 3 times (allowed 0, allowed 1, rejected 2)
    assert_eq!(turn_count.load(std::sync::atomic::Ordering::SeqCst), 3);

    // Only 2 assistant messages should be produced
    let assistant_count = new_messages
        .iter()
        .filter(|m| m.role() == "assistant")
        .count();
    assert_eq!(assistant_count, 2);
}

#[tokio::test]
async fn test_after_turn_receives_messages() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("Done.".into()),
    ]);

    let message_counts: std::sync::Arc<std::sync::Mutex<Vec<usize>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let counts_clone = message_counts.clone();

    let mut config = make_config(Arc::new(provider));
    config.after_turn = Some(std::sync::Arc::new(move |msgs, _usage| {
        counts_clone.lock().unwrap().push(msgs.len());
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ProgressTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let counts = message_counts.lock().unwrap();
    // after_turn called twice (one per LLM response)
    assert_eq!(counts.len(), 2);
    // Message count should increase between calls
    assert!(counts[1] > counts[0], "counts: {:?}", *counts);
}

#[tokio::test]
async fn test_on_error_fires_on_provider_error() {
    let provider = Arc::new(FailThenSucceedProvider {
        fail_count: std::sync::atomic::AtomicUsize::new(0),
        max_failures: 10, // more failures than retries
        error: ProviderError::Network("connection reset".into()),
        inner: MockProvider::text("never reached"),
    });

    let error_msgs: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let error_msgs_clone = error_msgs.clone();

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("mock", "mock", "test"),
        provider_override: Some(
            Arc::clone(&provider) as Arc<dyn phi_core::provider::StreamProvider>
        ),
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
        retry_config: phi_core::RetryConfig::none(),
        before_turn: None,
        after_turn: None,
        on_error: Some(std::sync::Arc::new(move |err| {
            error_msgs_clone.lock().unwrap().push(err.to_string());
        })),
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
    };

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let errors = error_msgs.lock().unwrap();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("connection reset"), "got: {}", errors[0]);
}

#[tokio::test]
async fn test_callbacks_are_optional() {
    // Verify the loop works fine with all callbacks set to None (same as before)
    let provider = MockProvider::text("Hello!");
    let config = make_config(Arc::new(provider));
    // make_config already sets all callbacks to None

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    assert_eq!(new_messages.len(), 2);
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

// ---------------------------------------------------------------------------
// ProgressMessage tests (Addition 1)
// ---------------------------------------------------------------------------

/// A tool that calls on_progress to emit user-facing progress messages.
struct ProgressMessageTool;

#[async_trait::async_trait]
impl AgentTool for ProgressMessageTool {
    fn name(&self) -> &str {
        "progress_msg_tool"
    }
    fn label(&self) -> &str {
        "ProgressMsg"
    }
    fn description(&self) -> &str {
        "Emits progress messages"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if let Some(ref progress) = ctx.on_progress {
            progress("Working...".into());
        }
        Ok(ToolResult {
            content: vec![Content::Text {
                text: "done".into(),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

#[tokio::test]
async fn test_progress_message_event_emitted() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_msg_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("ok".into()),
    ]);
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ProgressMessageTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    let progress_msgs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ProgressMessage {
                tool_call_id,
                tool_name,
                text,
                ..
            } => Some((tool_call_id.clone(), tool_name.clone(), text.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(progress_msgs.len(), 1);
    assert_eq!(progress_msgs[0].1, "progress_msg_tool");
    assert_eq!(progress_msgs[0].2, "Working...");
}

/// A tool that does NOT call on_progress — should cause no panics, no events.
struct SilentTool;

#[async_trait::async_trait]
impl AgentTool for SilentTool {
    fn name(&self) -> &str {
        "silent_tool"
    }
    fn label(&self) -> &str {
        "Silent"
    }
    fn description(&self) -> &str {
        "Does not call progress"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        _ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        // Intentionally ignores on_progress
        Ok(ToolResult {
            content: vec![Content::Text {
                text: "quiet".into(),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

#[tokio::test]
async fn test_tool_ignoring_progress_no_panic() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "silent_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("ok".into()),
    ]);
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(SilentTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    // No ProgressMessage events
    let progress_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::ProgressMessage { .. }))
        .count();
    assert_eq!(progress_count, 0);
}

/// Two parallel tools both emit progress — events are distinguishable by tool_call_id.
struct NamedProgressTool {
    tool_name: String,
}

#[async_trait::async_trait]
impl AgentTool for NamedProgressTool {
    fn name(&self) -> &str {
        &self.tool_name
    }
    fn label(&self) -> &str {
        &self.tool_name
    }
    fn description(&self) -> &str {
        "Named progress tool"
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({})
    }
    async fn execute(
        &self,
        _params: serde_json::Value,
        ctx: ToolContext,
    ) -> Result<ToolResult, ToolError> {
        if let Some(ref progress) = ctx.on_progress {
            progress(format!("progress from {}", self.tool_name));
        }
        Ok(ToolResult {
            content: vec![Content::Text {
                text: format!("done:{}", self.tool_name),
            }],
            details: serde_json::Value::Null,
            child_loop_id: None,
        })
    }
}

#[tokio::test]
async fn test_parallel_tools_progress_distinguishable() {
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![
            MockToolCall {
                name: "pa".into(),
                arguments: serde_json::json!({}),
            },
            MockToolCall {
                name: "pb".into(),
                arguments: serde_json::json!({}),
            },
        ]),
        MockResponse::Text("done".into()),
    ]);
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![
            Arc::new(NamedProgressTool {
                tool_name: "pa".into(),
            }),
            Arc::new(NamedProgressTool {
                tool_name: "pb".into(),
            }),
        ],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    let progress_msgs: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ProgressMessage {
                tool_name, text, ..
            } => Some((tool_name.clone(), text.clone())),
            _ => None,
        })
        .collect();

    assert_eq!(progress_msgs.len(), 2);
    let names: Vec<&str> = progress_msgs.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&"pa"));
    assert!(names.contains(&"pb"));
}

#[tokio::test]
async fn test_on_update_still_works_after_refactor() {
    // Existing ProgressTool uses on_update (not on_progress) — ensure it still works.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "progress_tool".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("ok".into()),
    ]);
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: vec![Arc::new(ProgressTool)],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("go")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    let updates: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::ToolExecutionUpdate { partial_result, .. } => {
                if let Some(Content::Text { text }) = partial_result.content.first() {
                    Some(text.clone())
                } else {
                    None
                }
            }
            _ => None,
        })
        .collect();

    assert_eq!(updates, vec!["step 1/3", "step 2/3", "step 3/3"]);
}

// ---------------------------------------------------------------------------
// InputFilter tests (Addition 2)
// ---------------------------------------------------------------------------

struct PassFilter;
impl InputFilter for PassFilter {
    fn filter(&self, _text: &str) -> FilterResult {
        FilterResult::Pass
    }
}

struct WarnFilter {
    warning: String,
}
impl InputFilter for WarnFilter {
    fn filter(&self, _text: &str) -> FilterResult {
        FilterResult::Warn(self.warning.clone())
    }
}

struct RejectFilter {
    reason: String,
}
impl InputFilter for RejectFilter {
    fn filter(&self, _text: &str) -> FilterResult {
        FilterResult::Reject(self.reason.clone())
    }
}

#[tokio::test]
async fn test_filter_pass_message_goes_through() {
    let provider = MockProvider::text("Hello!");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(PassFilter)];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    // Message went through normally
    assert_eq!(new_messages.len(), 2); // user + assistant
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

#[tokio::test]
async fn test_filter_warn_injects_warning_message() {
    let provider = MockProvider::text("Got it.");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(WarnFilter {
        warning: "danger".into(),
    })];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // user (with appended warning) + assistant = 2
    assert_eq!(new_messages.len(), 2);
    // The warning should be appended to the user message's content
    if let AgentMessage::Llm(LlmMessage {
        message: Message::User { content, .. },
        ..
    }) = &new_messages[0]
    {
        assert_eq!(content.len(), 2, "expected original text + warning");
        let warning = match &content[1] {
            Content::Text { text } => text.as_str(),
            _ => panic!("expected text"),
        };
        assert!(warning.contains("[Warning: danger]"), "got: {}", warning);
    } else {
        panic!("Expected user message at index 0");
    }
}

#[tokio::test]
async fn test_filter_reject_returns_empty() {
    let provider = MockProvider::text("Should not reach");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(RejectFilter {
        reason: "blocked".into(),
    })];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Bad input")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    // Rejected — empty messages returned
    assert!(new_messages.is_empty());
    // Context should NOT contain the rejected prompt
    assert!(
        context.messages.is_empty(),
        "Rejected prompts should not leak into context, got {} messages",
        context.messages.len()
    );
    // InputRejected event should carry the reason
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::InputRejected { reason, .. } if reason == "blocked")));
    // AgentStart + InputRejected + AgentEnd
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentStart { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { messages, .. } if messages.is_empty())));
}

#[tokio::test]
async fn test_filter_chain_first_reject_wins() {
    let provider = MockProvider::text("Should not reach");
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    struct CountingRejectFilter {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl InputFilter for CountingRejectFilter {
        fn filter(&self, _text: &str) -> FilterResult {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            FilterResult::Reject("first rejects".into())
        }
    }

    struct NeverCalledFilter {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }
    impl InputFilter for NeverCalledFilter {
        fn filter(&self, _text: &str) -> FilterResult {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            FilterResult::Pass
        }
    }

    let count2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));

    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![
        Arc::new(CountingRejectFilter {
            counter: call_count.clone(),
        }),
        Arc::new(NeverCalledFilter {
            counter: count2.clone(),
        }),
    ];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Bad")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    assert!(new_messages.is_empty());
    // First filter was called
    assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    // Second filter was NOT called (first reject short-circuits)
    assert_eq!(count2.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test]
async fn test_filter_multiple_warns_accumulate() {
    let provider = MockProvider::text("Got warnings.");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![
        Arc::new(WarnFilter {
            warning: "warn1".into(),
        }),
        Arc::new(WarnFilter {
            warning: "warn2".into(),
        }),
    ];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // user (with appended warnings) + assistant = 2
    assert_eq!(new_messages.len(), 2);
    if let AgentMessage::Llm(LlmMessage {
        message: Message::User { content, .. },
        ..
    }) = &new_messages[0]
    {
        // Original text + appended warning block
        assert!(content.len() >= 2, "expected original text + warning");
        let warning = match content.last().unwrap() {
            Content::Text { text } => text.as_str(),
            _ => panic!("expected text"),
        };
        assert!(warning.contains("[Warning: warn1]"), "got: {}", warning);
        assert!(warning.contains("[Warning: warn2]"), "got: {}", warning);
    } else {
        panic!("Expected user message");
    }
}

#[tokio::test]
async fn test_filter_non_text_content_only_text_extracted() {
    // User message with Image content — filter should receive only text portions
    let provider = MockProvider::text("Ok");

    let call_text = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let call_text_clone = call_text.clone();

    struct CapturingFilter {
        captured: std::sync::Arc<std::sync::Mutex<String>>,
    }
    impl InputFilter for CapturingFilter {
        fn filter(&self, text: &str) -> FilterResult {
            *self.captured.lock().unwrap() = text.to_string();
            FilterResult::Pass
        }
    }

    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(CapturingFilter {
        captured: call_text_clone,
    })];

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![
            Content::Text {
                text: "Check this image".into(),
            },
            Content::Image {
                data: "base64data".into(),
                mime_type: "image/png".into(),
            },
        ],
        timestamp: 0,
    }));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    let captured = call_text.lock().unwrap();
    // Filter should have received only the text portion
    assert_eq!(*captured, "Check this image");
}

// ---------------------------------------------------------------------------
// InputFilter tests — steering and follow-up paths
// ---------------------------------------------------------------------------

// Content-based filter helpers for steering/follow-up tests.
// Unlike the unconditional Pass/Warn/Reject filters above, these check the
// actual message text so the initial prompt can pass while the injected
// steering/follow-up message is caught.

struct ContentRejectFilter {
    keyword: String,
}
impl InputFilter for ContentRejectFilter {
    fn filter(&self, text: &str) -> FilterResult {
        if text.contains(&self.keyword) {
            FilterResult::Reject(format!("blocked: {}", self.keyword))
        } else {
            FilterResult::Pass
        }
    }
}

struct ContentWarnFilter {
    keyword: String,
    warning: String,
}
impl InputFilter for ContentWarnFilter {
    fn filter(&self, text: &str) -> FilterResult {
        if text.contains(&self.keyword) {
            FilterResult::Warn(self.warning.clone())
        } else {
            FilterResult::Pass
        }
    }
}

#[tokio::test]
async fn test_filter_rejects_steering_message() {
    // The filter passes the initial prompt ("hello") but rejects the steering
    // message ("SECRET"). The run should abort before any LLM turn starts.
    let provider = MockProvider::text("Should not reach LLM.");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(ContentRejectFilter {
        keyword: "SECRET".into(),
    })];

    // Steering returns "SECRET" on the first poll, empty thereafter.
    let steered = Arc::new(std::sync::Mutex::new(false));
    let steered_clone = steered.clone();
    config.get_steering_messages = Some(Box::new(move || {
        let mut done = steered_clone.lock().unwrap();
        if !*done {
            *done = true;
            vec![AgentMessage::Llm(LlmMessage::new(Message::user(
                "SECRET content",
            )))]
        } else {
            vec![]
        }
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("hello")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;
    let events = collect_events(rx);

    // InputRejected must have fired
    assert!(
        events.iter().any(
            |e| matches!(e, AgentEvent::InputRejected { reason, .. } if reason.contains("SECRET"))
        ),
        "expected InputRejected; got: {:?}",
        events
    );
    // No LLM turn should have started
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnStart { .. })),
        "unexpected TurnStart; steering was rejected before any turn"
    );
}

#[tokio::test]
async fn test_filter_warns_steering_message() {
    // The filter passes the initial prompt but warns on the steering message.
    // The warning text should be appended to the steering message before injection.
    let provider = MockProvider::texts(vec!["First turn.", "Second turn."]);
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(ContentWarnFilter {
        keyword: "FLAGGED".into(),
        warning: "steer-warn".into(),
    })];

    let steered = Arc::new(std::sync::Mutex::new(false));
    let steered_clone = steered.clone();
    config.get_steering_messages = Some(Box::new(move || {
        let mut done = steered_clone.lock().unwrap();
        if !*done {
            *done = true;
            vec![AgentMessage::Llm(LlmMessage::new(Message::user(
                "FLAGGED content",
            )))]
        } else {
            vec![]
        }
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("hello")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    // The steering message should be in context with the warning appended
    let steering_msg = context.messages.iter().find(|m| {
        if let AgentMessage::Llm(LlmMessage {
            message: Message::User { content, .. },
            ..
        }) = m
        {
            content
                .iter()
                .any(|c| matches!(c, Content::Text { text } if text.contains("FLAGGED")))
        } else {
            false
        }
    });
    assert!(
        steering_msg.is_some(),
        "steering message not found in context"
    );

    if let Some(AgentMessage::Llm(LlmMessage {
        message: Message::User { content, .. },
        ..
    })) = steering_msg
    {
        let has_warning = content
            .iter()
            .any(|c| matches!(c, Content::Text { text } if text.contains("[Warning: steer-warn]")));
        assert!(
            has_warning,
            "warning not appended to steering message; content: {:?}",
            content
        );
    }
}

#[tokio::test]
async fn test_filter_rejects_follow_up_message() {
    // The filter passes the initial prompt but rejects the follow-up message.
    // The first LLM turn completes normally; the run aborts before the follow-up turn.
    let provider = MockProvider::text("Normal response.");
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(ContentRejectFilter {
        keyword: "BLOCKED_FOLLOWUP".into(),
    })];

    let followed = Arc::new(std::sync::Mutex::new(false));
    let followed_clone = followed.clone();
    config.get_follow_up_messages = Some(Box::new(move || {
        let mut done = followed_clone.lock().unwrap();
        if !*done {
            *done = true;
            vec![AgentMessage::Llm(LlmMessage::new(Message::user(
                "BLOCKED_FOLLOWUP content",
            )))]
        } else {
            vec![]
        }
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("hello")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;
    let events = collect_events(rx);

    // First turn completed normally
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::TurnEnd { .. })),
        "expected at least one TurnEnd"
    );
    // Follow-up was rejected
    assert!(
        events.iter().any(|e| matches!(e, AgentEvent::InputRejected { reason, .. } if reason.contains("BLOCKED_FOLLOWUP"))),
        "expected InputRejected for follow-up; got: {:?}", events
    );
    // AgentEnd still fires (run closes cleanly)
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::AgentEnd { .. })),
        "expected AgentEnd"
    );
    // Only one TurnStart: the second turn (follow-up) never started
    let turn_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .count();
    assert_eq!(
        turn_starts, 1,
        "expected exactly 1 TurnStart; follow-up turn was rejected"
    );
}

#[tokio::test]
async fn test_filter_warns_follow_up_message() {
    // The filter warns on the follow-up message; warning text is appended before injection.
    let provider = MockProvider::texts(vec!["First turn.", "Second turn."]);
    let mut config = make_config(Arc::new(provider));
    config.input_filters = vec![Arc::new(ContentWarnFilter {
        keyword: "WARN_FOLLOWUP".into(),
        warning: "follow-warn".into(),
    })];

    let followed = Arc::new(std::sync::Mutex::new(false));
    let followed_clone = followed.clone();
    config.get_follow_up_messages = Some(Box::new(move || {
        let mut done = followed_clone.lock().unwrap();
        if !*done {
            *done = true;
            vec![AgentMessage::Llm(LlmMessage::new(Message::user(
                "WARN_FOLLOWUP content",
            )))]
        } else {
            vec![]
        }
    }));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("hello")))],
        &mut context,
        &config,
        tx,
        cancel,
    )
    .await;

    // The follow-up message should be in context with the warning appended
    let followup_msg = context.messages.iter().find(|m| {
        if let AgentMessage::Llm(LlmMessage {
            message: Message::User { content, .. },
            ..
        }) = m
        {
            content
                .iter()
                .any(|c| matches!(c, Content::Text { text } if text.contains("WARN_FOLLOWUP")))
        } else {
            false
        }
    });
    assert!(
        followup_msg.is_some(),
        "follow-up message not found in context"
    );

    if let Some(AgentMessage::Llm(LlmMessage {
        message: Message::User { content, .. },
        ..
    })) = followup_msg
    {
        let has_warning = content.iter().any(
            |c| matches!(c, Content::Text { text } if text.contains("[Warning: follow-warn]")),
        );
        assert!(
            has_warning,
            "warning not appended to follow-up message; content: {:?}",
            content
        );
    }
    // Both turns ran
    assert_eq!(
        context
            .messages
            .iter()
            .filter(|m| matches!(
                m,
                AgentMessage::Llm(LlmMessage {
                    message: Message::Assistant { .. },
                    ..
                })
            ))
            .count(),
        2
    );
}

// ---------------------------------------------------------------------------
// Usage tracking tests (TurnEnd.usage, AgentEnd.usage, reasoning, budget)
// ---------------------------------------------------------------------------

/// A StreamProvider that wraps MockProvider but injects a specific Usage into the returned message.
struct WithUsageProvider {
    usage: Usage,
    inner: MockProvider,
}

#[async_trait::async_trait]
impl phi_core::provider::StreamProvider for WithUsageProvider {
    fn provider_id(&self) -> &str {
        "mock-with-usage"
    }

    async fn stream(
        &self,
        config: phi_core::provider::StreamConfig,
        tx: tokio::sync::mpsc::UnboundedSender<phi_core::provider::StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<phi_core::Message, phi_core::provider::ProviderError> {
        let mut msg = self.inner.stream(config, tx, cancel).await?;
        if let phi_core::Message::Assistant { ref mut usage, .. } = msg {
            *usage = self.usage.clone();
        }
        Ok(msg)
    }
}

#[tokio::test]
async fn test_turn_end_carries_usage() {
    let expected_usage = Usage {
        input: 200,
        output: 80,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 280,
    };
    let provider = Arc::new(WithUsageProvider {
        usage: expected_usage.clone(),
        inner: MockProvider::text("Hello"),
    });
    let config = make_config(provider);
    let mut context = AgentContext {
        system_prompt: "sys".into(),
        messages: vec![],
        tools: vec![],
        ..Default::default()
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let prompts = vec![AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text { text: "hi".into() }],
        timestamp: 0,
    }))];
    agent_loop(prompts, &mut context, &config, tx, CancellationToken::new()).await;
    let events = collect_events(rx);
    let turn_end = events.iter().find_map(|e| {
        if let AgentEvent::TurnEnd { usage, .. } = e {
            Some(usage.clone())
        } else {
            None
        }
    });
    assert!(turn_end.is_some(), "TurnEnd event not found");
    assert_eq!(turn_end.unwrap(), expected_usage);
}

#[tokio::test]
async fn test_agent_end_carries_accumulated_usage() {
    let turn_usage = Usage {
        input: 100,
        output: 50,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 150,
    };
    // Two-turn run: MockProvider returns two text responses
    let provider = Arc::new(WithUsageProvider {
        usage: turn_usage.clone(),
        inner: MockProvider::texts(vec!["First", "Second"]),
    });
    let mut config = make_config(provider);
    config.get_follow_up_messages = Some(Box::new({
        let called = std::sync::atomic::AtomicBool::new(false);
        let called = Arc::new(called);
        move || {
            if !called.swap(true, std::sync::atomic::Ordering::SeqCst) {
                vec![AgentMessage::Llm(LlmMessage::new(Message::User {
                    content: vec![Content::Text {
                        text: "follow up".into(),
                    }],
                    timestamp: 0,
                }))]
            } else {
                vec![]
            }
        }
    }));
    let mut context = AgentContext {
        system_prompt: "sys".into(),
        messages: vec![],
        tools: vec![],
        ..Default::default()
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let prompts = vec![AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text {
            text: "start".into(),
        }],
        timestamp: 0,
    }))];
    agent_loop(prompts, &mut context, &config, tx, CancellationToken::new()).await;
    let events = collect_events(rx);
    let agent_end_usage = events.iter().find_map(|e| {
        if let AgentEvent::AgentEnd { usage, .. } = e {
            Some(usage.clone())
        } else {
            None
        }
    });
    assert!(agent_end_usage.is_some(), "AgentEnd event not found");
    let total = agent_end_usage.unwrap();
    // Two turns each with input=100, output=50 → total input=200, output=100
    assert_eq!(total.input, 200);
    assert_eq!(total.output, 100);
}

#[tokio::test]
async fn test_reasoning_tokens_accumulated() {
    let turn_usage = Usage {
        input: 300,
        output: 120,
        reasoning: 50,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 420,
    };
    let provider = Arc::new(WithUsageProvider {
        usage: turn_usage.clone(),
        inner: MockProvider::text("Done with reasoning"),
    });
    let config = make_config(provider);
    let mut context = AgentContext {
        system_prompt: "sys".into(),
        messages: vec![],
        tools: vec![],
        ..Default::default()
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let prompts = vec![AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text {
            text: "think hard".into(),
        }],
        timestamp: 0,
    }))];
    agent_loop(prompts, &mut context, &config, tx, CancellationToken::new()).await;
    let events = collect_events(rx);
    let agent_end_usage = events.iter().find_map(|e| {
        if let AgentEvent::AgentEnd { usage, .. } = e {
            Some(usage.clone())
        } else {
            None
        }
    });
    assert!(agent_end_usage.is_some());
    assert_eq!(agent_end_usage.unwrap().reasoning, 50);
}

#[tokio::test]
async fn test_budget_enforcement_stops_loop() {
    use phi_core::context::ExecutionLimits;
    use phi_core::provider::CostConfig;

    // Each turn: output=1 token, priced at $1_000_000 per million = $1.00 per turn.
    // Budget: $0.50 → first turn costs $1.00 which exceeds the budget.
    // The check happens AFTER the first turn, so 1 TurnStart fires then the loop stops.
    let turn_usage = Usage {
        input: 0,
        output: 1,
        reasoning: 0,
        cache_read: 0,
        cache_write: 0,
        total_tokens: 1,
    };
    let provider = Arc::new(WithUsageProvider {
        usage: turn_usage,
        inner: MockProvider::texts(vec!["First", "Second"]),
    });
    let mut config = make_config(provider);
    config.model_config.cost = CostConfig {
        input_per_million: 0.0,
        output_per_million: 1_000_000.0, // $1 per token
        cache_read_per_million: 0.0,
        cache_write_per_million: 0.0,
    };
    config.execution_limits = Some(ExecutionLimits {
        max_turns: 10,
        max_total_tokens: 1_000_000,
        max_duration: std::time::Duration::from_secs(60),
        max_cost: Some(0.50), // $0.50 budget — first turn ($1.00) exceeds it
    });
    let mut context = AgentContext {
        system_prompt: "sys".into(),
        messages: vec![],
        tools: vec![],
        ..Default::default()
    };
    let (tx, rx) = mpsc::unbounded_channel();
    let prompts = vec![AgentMessage::Llm(LlmMessage::new(Message::User {
        content: vec![Content::Text { text: "go".into() }],
        timestamp: 0,
    }))];
    agent_loop(prompts, &mut context, &config, tx, CancellationToken::new()).await;
    let events = collect_events(rx);
    let turn_starts = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnStart { .. }))
        .count();
    // First turn runs, cost check fires after it, loop stops before second turn
    assert_eq!(
        turn_starts, 1,
        "expected 1 TurnStart before budget cut-off, got {}",
        turn_starts
    );
    // AgentEnd must always be emitted
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

// ---------------------------------------------------------------------------
// CompactionStrategy tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_default_compaction_matches_compact_messages() {
    use phi_core::context::{compact_messages, ContextConfig, DefaultCompaction};
    use phi_core::CompactionStrategy;

    let mut messages = Vec::new();
    for i in 0..100 {
        messages.push(AgentMessage::Llm(LlmMessage::new(Message::user(format!(
            "Message {} {}",
            i,
            "x".repeat(200)
        )))));
    }

    let config = ContextConfig {
        max_context_tokens: 500,
        system_prompt_tokens: 100,
        compaction: CompactionConfig::default(),
        token_counter: None,
        keep_recent: 5,
        keep_first: 2,
        tool_output_max_lines: 20,
    };

    let result_direct = compact_messages(messages.clone(), &config);
    let result_trait = DefaultCompaction.compact(messages, &config);

    // Compare lengths and structure, not deep equality — Level 3 compaction
    // inserts marker messages with now_ms() timestamps that differ between calls.
    assert_eq!(result_direct.len(), result_trait.len());
    assert!(
        result_direct.len() < 100,
        "compaction should have reduced messages"
    );
    assert!(
        result_direct.len() >= 2,
        "should keep at least keep_first messages"
    );
}

#[tokio::test]
async fn test_custom_compaction_strategy_is_called() {
    use phi_core::context::ContextConfig;
    use phi_core::CompactionStrategy;

    /// A custom strategy that prepends a marker message, then delegates
    /// to the default compaction.
    struct MarkerCompaction;

    impl CompactionStrategy for MarkerCompaction {
        fn compact(
            &self,
            messages: Vec<AgentMessage>,
            _config: &ContextConfig,
        ) -> Vec<AgentMessage> {
            let mut result = vec![AgentMessage::Llm(LlmMessage::new(Message::user(
                "[compacted]",
            )))];
            // Keep only the last message to prove we ran
            if let Some(last) = messages.last() {
                result.push(last.clone());
            }
            result
        }
    }

    // Provider returns a simple text response
    let provider = MockProvider::text("Got it.");

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("test", "test", "test"),
        provider_override: Some(Arc::new(provider) as Arc<dyn phi_core::provider::StreamProvider>),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: Some(ContextConfig {
            max_context_tokens: 10, // Tiny budget to force compaction
            system_prompt_tokens: 0,
            compaction: CompactionConfig {
                compact_at_pct: 0.01,               // Very aggressive — trigger at 1%
                compact_budget_threshold_pct: 0.99, // Always fire
                in_memory_strategy: Some(std::sync::Arc::new(MarkerCompaction)),
                ..CompactionConfig::default()
            },
            token_counter: None,
            keep_recent: 1,
            keep_first: 1,
            tool_output_max_lines: 10,
        }),
        execution_limits: None,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        retry_config: phi_core::RetryConfig::none(),
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
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hello")));
    let mut context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // The custom strategy should have inserted "[compacted]" as the first message
    assert!(
        context.messages.iter().any(|m| {
            if let AgentMessage::Llm(LlmMessage {
                message: Message::User { content, .. },
                ..
            }) = m
            {
                content
                    .iter()
                    .any(|c| matches!(c, Content::Text { text } if text == "[compacted]"))
            } else {
                false
            }
        }),
        "Custom compaction marker not found in context: {:?}",
        context
            .messages
            .iter()
            .filter_map(|m| {
                if let AgentMessage::Llm(LlmMessage {
                    message: Message::User { content, .. },
                    ..
                }) = m
                {
                    Some(content)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn test_none_compaction_strategy_uses_default() {
    use phi_core::context::ContextConfig;

    // Provider returns a simple text response
    let provider = MockProvider::text("Got it.");

    let config = AgentLoopConfig {
        model_config: ModelConfig::anthropic("test", "test", "test"),
        provider_override: Some(Arc::new(provider) as Arc<dyn phi_core::provider::StreamProvider>),
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        convert_to_llm: None,
        transform_context: None,
        get_steering_messages: None,
        get_follow_up_messages: None,
        context_config: Some(ContextConfig {
            max_context_tokens: 10, // Tiny budget to force compaction
            system_prompt_tokens: 0,
            compaction: CompactionConfig {
                compact_at_pct: 0.01,
                compact_budget_threshold_pct: 0.99,
                ..CompactionConfig::default()
            },
            token_counter: None,
            keep_recent: 1,
            keep_first: 1,
            tool_output_max_lines: 10,
        }),
        execution_limits: None,
        cache_config: CacheConfig::default(),
        tool_execution: ToolExecutionStrategy::default(),
        retry_config: phi_core::RetryConfig::none(),
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
    };

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("Hello")));
    let mut context = AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
        agent_id: None,
        session_id: None,
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    // Should not panic — DefaultCompaction handles everything
    let new_messages = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;

    // Agent should have produced at least the user message + assistant response
    assert!(
        !new_messages.is_empty(),
        "Agent should have produced messages"
    );
}

// ---------------------------------------------------------------------------
// Session & Loop Identity tests
// ---------------------------------------------------------------------------

/// loop_id with explicit config_id uses the format "{session_id}.{config_id}.1"
#[tokio::test]
async fn test_loop_id_explicit_config_id() {
    let provider = MockProvider::text("hello");
    let mut config = make_config(Arc::new(provider));
    config.config_id = Some("anthropic-opus".into());

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: Vec::new(),
        tools: Vec::new(),
        agent_id: Some("agt-test".into()),
        session_id: Some("ses-test".into()),
        loop_id: Some("ses-test.anthropic-opus.1".into()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop(vec![], &mut context, &config, tx, cancel).await;

    // Collect events and find AgentStart
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    let agent_start = events.iter().find_map(|e| {
        if let AgentEvent::AgentStart { loop_id, .. } = e {
            Some(loop_id.clone())
        } else {
            None
        }
    });
    assert_eq!(
        agent_start.as_deref(),
        Some("ses-test.anthropic-opus.1"),
        "loop_id in AgentStart should match the one set in context"
    );
}

/// agent_loop_continue emits AgentStart with parent_loop_id and continuation_kind
#[tokio::test]
async fn test_continuation_kind_in_agent_start() {
    let provider = MockProvider::text("Done processing.");
    let config = make_config(Arc::new(provider));

    let tag = chrono::Utc::now().to_rfc3339();
    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: vec![
            AgentMessage::Llm(LlmMessage::new(Message::user("do something"))),
            AgentMessage::Llm(LlmMessage::new(Message::ToolResult {
                tool_call_id: "tc-1".into(),
                tool_name: "test_tool".into(),
                content: vec![Content::Text {
                    text: "result".into(),
                }],
                is_error: false,
                timestamp: 0,
            })),
        ],
        tools: Vec::new(),
        agent_id: Some("agt-test".into()),
        session_id: Some("ses-test".into()),
        loop_id: Some("ses-test.mock.mock.2".into()),
        parent_loop_id: Some("ses-test.mock.mock.1".into()),
        continuation_kind: Some(ContinuationKind::Rerun { tag: tag.clone() }),
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop_continue(&mut context, &config, tx, cancel).await;

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    let start_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::AgentStart { .. }));
    assert!(start_event.is_some(), "AgentStart must be emitted");

    if let Some(AgentEvent::AgentStart {
        loop_id,
        parent_loop_id,
        continuation_kind,
        ..
    }) = start_event
    {
        assert_eq!(loop_id, "ses-test.mock.mock.2");
        assert_eq!(parent_loop_id.as_deref(), Some("ses-test.mock.mock.1"));
        assert!(
            matches!(continuation_kind, Some(ContinuationKind::Rerun { .. })),
            "continuation_kind should be Rerun"
        );
    }
}

/// Two agent_loop() calls with different config_ids in the same session get independent .1 counters
#[tokio::test]
async fn test_agent_wrapper_independent_counters_per_config() {
    use phi_core::BasicAgent;

    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock-a", "mock-a", "test"))
        .with_provider_override(Arc::new(MockProvider::texts(vec!["first", "second"])));

    // First loop with model "mock-a" (already set above)
    let rx1 = agent.prompt("hello").await;
    let events1: Vec<_> = {
        let mut v = Vec::new();
        let mut rx = rx1;
        while let Ok(e) = rx.try_recv() {
            v.push(e);
        }
        v
    };

    let loop_id_1 = events1.iter().find_map(|e| {
        if let AgentEvent::AgentStart { loop_id, .. } = e {
            Some(loop_id.clone())
        } else {
            None
        }
    });

    // Second loop with a different model "mock-b" — gets its own counter starting at .1
    agent.model_config = ModelConfig::anthropic("mock-b", "mock-b", "test");
    let rx2 = agent.prompt("world").await;
    let events2: Vec<_> = {
        let mut v = Vec::new();
        let mut rx = rx2;
        while let Ok(e) = rx.try_recv() {
            v.push(e);
        }
        v
    };

    let loop_id_2 = events2.iter().find_map(|e| {
        if let AgentEvent::AgentStart { loop_id, .. } = e {
            Some(loop_id.clone())
        } else {
            None
        }
    });

    let id1 = loop_id_1.expect("loop_id_1 missing");
    let id2 = loop_id_2.expect("loop_id_2 missing");

    // Both end in .1 (independent per-config counters)
    assert!(
        id1.ends_with(".1"),
        "first loop should end in .1, got: {}",
        id1
    );
    assert!(
        id2.ends_with(".1"),
        "second loop (different model) should also end in .1, got: {}",
        id2
    );
    // They differ in the config_id segment
    assert_ne!(id1, id2, "loop_ids for different models must differ");
}

/// agent_loop_continue panics when agent_id is None
#[tokio::test]
#[should_panic(expected = "agent_loop_continue requires context.agent_id to be set")]
async fn test_continue_panics_without_agent_id() {
    let provider = MockProvider::text("unreachable");
    let config = make_config(Arc::new(provider));

    let mut context = AgentContext {
        system_prompt: "test".into(),
        messages: vec![AgentMessage::Llm(LlmMessage::new(Message::user("hi")))],
        tools: Vec::new(),
        agent_id: None, // ← intentionally None — should panic
        session_id: Some("ses-test".into()),
        loop_id: None,
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    agent_loop_continue(&mut context, &config, tx, cancel).await;
}

// ── Evaluational parallelism tests ───────────────────────────────────────────

fn make_base_context() -> AgentContext {
    AgentContext {
        system_prompt: String::new(),
        messages: vec![],
        tools: vec![],
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

/// Single config + TransparentEvaluation behaves identically to plain agent_loop.
#[tokio::test]
async fn test_parallel_transparent() {
    let provider = MockProvider::text("transparent response");
    let config = make_config(Arc::new(provider));

    let (tx, rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("hello")))],
        make_base_context(),
        vec![config],
        Arc::new(TransparentEvaluation),
        tx,
        CancellationToken::new(),
    )
    .await;

    assert_eq!(result.selected_index, 0);
    assert!(result.all_outcomes.is_empty()); // selected removed from all_outcomes
    assert!(!result.selected_messages.is_empty());

    let events = collect_events(rx);
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ParallelLoopStart { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ParallelLoopEnd { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentStart { .. })));
}

/// PickFirstEvaluation always selects index 0 regardless of other branches.
#[tokio::test]
async fn test_parallel_pick_first() {
    let config_a = make_config(Arc::new(MockProvider::text("response from A")));
    let config_b = make_config(Arc::new(MockProvider::text("response from B")));

    let (tx, rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("compare")))],
        make_base_context(),
        vec![config_a, config_b],
        Arc::new(PickFirstEvaluation),
        tx,
        CancellationToken::new(),
    )
    .await;

    assert_eq!(result.selected_index, 0);
    // One branch was selected, one remains in all_outcomes.
    assert_eq!(result.all_outcomes.len(), 1);

    // Both branches' AgentStart events should appear (same session_id).
    let events = collect_events(rx);
    let agent_starts: Vec<_> = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::AgentStart { .. }))
        .collect();
    assert_eq!(agent_starts.len(), 2);

    // All AgentStart events share the same session_id.
    let session_ids: Vec<_> = agent_starts
        .iter()
        .filter_map(|e| {
            if let AgentEvent::AgentStart { session_id, .. } = e {
                Some(session_id.clone())
            } else {
                None
            }
        })
        .collect();
    assert!(session_ids.windows(2).all(|w| w[0] == w[1]));
}

/// TokenEfficientEvaluation selects the branch with the fewest output tokens.
/// MockProvider reports usage proportional to response length via MockResponse.
#[tokio::test]
async fn test_parallel_token_efficient() {
    // We can't control MockProvider's reported token counts directly, but we can verify
    // the strategy is wired correctly: all 3 branches run, one is selected.
    let config_a = make_config(Arc::new(MockProvider::text("a")));
    let config_b = make_config(Arc::new(MockProvider::text("b")));
    let config_c = make_config(Arc::new(MockProvider::text("c")));

    let (tx, _rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("query")))],
        make_base_context(),
        vec![config_a, config_b, config_c],
        Arc::new(TokenEfficientEvaluation),
        tx,
        CancellationToken::new(),
    )
    .await;

    // Whatever was selected, the index is in [0, 2].
    assert!(result.selected_index <= 2);
    // 2 non-selected branches remain.
    assert_eq!(result.all_outcomes.len(), 2);
    // ParallelLoopResult has a total_usage field — just verify it exists.
    let _ = &result.total_usage;
}

/// ElaborateEvaluation selects the branch with the most output tokens.
#[tokio::test]
async fn test_parallel_elaborate() {
    let config_a = make_config(Arc::new(MockProvider::text("x")));
    let config_b = make_config(Arc::new(MockProvider::text("y")));

    let (tx, _rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user("query")))],
        make_base_context(),
        vec![config_a, config_b],
        Arc::new(ElaborateEvaluation),
        tx,
        CancellationToken::new(),
    )
    .await;

    assert!(result.selected_index <= 1);
    assert_eq!(result.all_outcomes.len(), 1);
}

/// LlmJudgeEvaluation: a third MockProvider acts as judge and returns "2",
/// so branch index 1 is selected.
#[tokio::test]
async fn test_parallel_llm_judge() {
    use phi_core::agent_loop::evaluation::LlmJudgeEvaluation;

    let config_a = make_config(Arc::new(MockProvider::text("first branch answer")));
    let config_b = make_config(Arc::new(MockProvider::text("second branch answer")));

    // Judge mock replies "2" → selects index 1 (second branch, 0-based)
    let judge_provider = Arc::new(MockProvider::text("2"));
    let judge_config = make_config(judge_provider);

    let (tx, rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![AgentMessage::Llm(LlmMessage::new(Message::user(
            "which is better?",
        )))],
        make_base_context(),
        vec![config_a, config_b],
        Arc::new(LlmJudgeEvaluation {
            judge_config,
            system_prompt: None,
        }),
        tx,
        CancellationToken::new(),
    )
    .await;

    // Judge said "2" → selected_index == 1
    assert_eq!(result.selected_index, 1);
    assert_eq!(result.all_outcomes.len(), 1); // the non-selected branch

    // The judge loop emits its own AgentStart — so we expect 3 AgentStart events
    // (branch A + branch B + judge).
    let events = collect_events(rx);
    let agent_start_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::AgentStart { .. }))
        .count();
    assert_eq!(agent_start_count, 3);

    // Judge's usage is part of total_usage.
    let end_event = events
        .iter()
        .find(|e| matches!(e, AgentEvent::ParallelLoopEnd { .. }));
    assert!(end_event.is_some());
    if let Some(AgentEvent::ParallelLoopEnd {
        selected_config_index,
        ..
    }) = end_event
    {
        assert_eq!(*selected_config_index, 1);
    }
}

/// agent_loop_continue mode: prompts is empty, context already contains the user query.
/// Both branches run via agent_loop_continue and the result is selected normally.
#[tokio::test]
async fn test_parallel_continue_mode() {
    // Both branches respond with text; PickFirst selects branch 0.
    let config_a = make_config(Arc::new(MockProvider::text("branch a response")));
    let config_b = make_config(Arc::new(MockProvider::text("branch b response")));

    // Pre-populate context with the user query (simulating an agent_loop_continue scenario).
    let mut base_ctx = make_base_context();
    base_ctx.agent_id = Some("test-agent".to_string());
    base_ctx.session_id = Some("test-session".to_string());
    base_ctx
        .messages
        .push(AgentMessage::Llm(LlmMessage::new(Message::user(
            "Which answer is better?",
        ))));

    let (tx, rx) = mpsc::unbounded_channel();
    let result = agent_loop_parallel(
        vec![], // empty prompts → agent_loop_continue mode
        base_ctx,
        vec![config_a, config_b],
        Arc::new(PickFirstEvaluation),
        tx,
        CancellationToken::new(),
    )
    .await;

    // PickFirst always selects index 0.
    assert_eq!(result.selected_index, 0);

    // The selected branch should have produced at least one assistant message.
    assert!(!result.selected_messages.is_empty());

    // original_context_len should be 1 (the user message we pre-populated).
    // The non-selected outcome retains this information.
    assert_eq!(result.all_outcomes.len(), 1);
    assert_eq!(result.all_outcomes[0].original_context_len, 1);

    // Both branches emitted AgentStart events.
    let events = collect_events(rx);
    let agent_start_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::AgentStart { .. }))
        .count();
    assert_eq!(agent_start_count, 2);

    // ParallelLoopStart and ParallelLoopEnd bracket the execution.
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ParallelLoopStart { .. })));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::ParallelLoopEnd { .. })));
}

// ═══════════════════════════════════════════════════════════════════════════
// New builder methods and hook tests (Phase 1 invocation layer)
// ═══════════════════════════════════════════════════════════════════════════

#[test]
fn test_new_builder_methods_compile() {
    // Verify all new builder methods chain correctly (compile-time check)
    let _agent = phi_core::BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_system_prompt("test")
        .with_temperature(0.7)
        .with_config_id("test-config")
        .on_before_loop(|_msgs, _n| true)
        .on_after_loop(|_msgs, _usage| {})
        .on_before_tool_execution(|_name, _id, _args| true)
        .on_after_tool_execution(|_name, _id, _error| {})
        .on_before_tool_execution_update(|_name, _id, _text| true)
        .on_after_tool_execution_update(|_name, _id, _text| {})
        .with_convert_to_llm(|msgs| msgs.iter().filter_map(|m| m.as_llm().cloned()).collect())
        .with_transform_context(|msgs| msgs)
        .on_before_compaction_start(|_tokens, _count| true)
        .on_after_compaction_end(|_before, _after, _tok_before, _tok_after| {});
}

#[tokio::test]
async fn test_before_loop_hook_fires() {
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_clone = fired.clone();

    let provider = Arc::new(MockProvider::texts(vec!["hello"]));
    let mut config = make_config(provider);
    config.before_loop = Some(Arc::new(move |_msgs, _n| {
        fired_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        true // allow loop to proceed
    }));

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let msg = AgentMessage::Llm(LlmMessage::new(Message::user("test")));

    let mut context = AgentContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![],
        agent_id: Some("test".to_string()),
        session_id: Some("test".to_string()),
        loop_id: Some("test.loop.1".to_string()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    agent_loop(vec![msg], &mut context, &config, tx, cancel).await;
    let events = collect_events(rx);

    assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(events
        .iter()
        .any(|e| matches!(e, AgentEvent::AgentEnd { .. })));
}

#[tokio::test]
async fn test_after_loop_hook_fires() {
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let fired_clone = fired.clone();

    let provider = Arc::new(MockProvider::texts(vec!["hello"]));
    let mut config = make_config(provider);
    config.after_loop = Some(Arc::new(move |_msgs, _usage| {
        fired_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let msg = AgentMessage::Llm(LlmMessage::new(Message::user("test")));

    let mut context = AgentContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![],
        agent_id: Some("test".to_string()),
        session_id: Some("test".to_string()),
        loop_id: Some("test.loop.1".to_string()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    agent_loop(vec![msg], &mut context, &config, tx, cancel).await;
    let _events = collect_events(rx);

    assert!(fired.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_tool_execution_hooks_fire() {
    let before_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let after_fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let before_clone = before_fired.clone();
    let after_clone = after_fired.clone();

    // Create a mock that returns a tool call, then a final response
    let provider = Arc::new(MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "bash".to_string(),
            arguments: serde_json::json!({"command": "echo hi"}),
        }]),
        MockResponse::Text("done".to_string()),
    ]));
    let mut config = make_config(provider);
    config.before_tool_execution = Some(Arc::new(move |_name, _id, _args| {
        before_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        true
    }));
    config.after_tool_execution = Some(Arc::new(move |_name, _id, _err| {
        after_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    }));

    let tool = phi_core::tools::BashTool::default();
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let msg = AgentMessage::Llm(LlmMessage::new(Message::user("run echo hi")));

    let mut context = AgentContext {
        system_prompt: String::new(),
        messages: Vec::new(),
        tools: vec![Arc::new(tool)],
        agent_id: Some("test".to_string()),
        session_id: Some("test".to_string()),
        loop_id: Some("test.loop.1".to_string()),
        parent_loop_id: None,
        continuation_kind: None,
        session: None,
        user_context: Vec::new(),
        inrun_context: Vec::new(),
    };

    agent_loop(vec![msg], &mut context, &config, tx, cancel).await;
    let _events = collect_events(rx);

    assert!(before_fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(after_fired.load(std::sync::atomic::Ordering::SeqCst));
}

// ---------------------------------------------------------------------------
// G5 — Compaction Strategy Tests
// ---------------------------------------------------------------------------

#[test]
fn test_compaction_strategy_in_compaction_config() {
    use phi_core::context::{CompactionConfig, CompactionStrategy, ContextConfig};
    use std::sync::atomic::{AtomicBool, Ordering};

    // Create a custom CompactionStrategy that sets a flag when accessed.
    struct MarkerStrategy {
        flag: Arc<AtomicBool>,
    }
    impl CompactionStrategy for MarkerStrategy {
        fn compact(
            &self,
            messages: Vec<AgentMessage>,
            _config: &ContextConfig,
        ) -> Vec<AgentMessage> {
            self.flag.store(true, Ordering::SeqCst);
            messages
        }
    }

    let flag = Arc::new(AtomicBool::new(false));
    let strategy = Arc::new(MarkerStrategy { flag: flag.clone() });

    let compaction = CompactionConfig {
        in_memory_strategy: Some(strategy),
        ..Default::default()
    };

    // Verify the field is Some.
    assert!(compaction.in_memory_strategy.is_some());

    // Build a ContextConfig using this CompactionConfig.
    let ctx_config = ContextConfig {
        compaction,
        ..Default::default()
    };

    // Verify it's wired through.
    assert!(ctx_config.compaction.in_memory_strategy.is_some());

    // Call compact to verify the strategy is reachable.
    let strategy_ref = ctx_config.compaction.in_memory_strategy.as_ref().unwrap();
    strategy_ref.compact(vec![], &ctx_config);
    assert!(
        flag.load(Ordering::SeqCst),
        "strategy should have been called"
    );
}

#[test]
fn test_block_strategy_in_compaction_config() {
    use phi_core::context::{CompactionConfig, DefaultBlockCompaction};

    let compaction = CompactionConfig {
        block_strategy: Some(Arc::new(DefaultBlockCompaction)),
        ..Default::default()
    };

    // Verify the field is Some and round-trips correctly.
    assert!(compaction.block_strategy.is_some());

    // Clone to verify it survives Arc cloning (CompactionConfig is Clone).
    let cloned = compaction.clone();
    assert!(cloned.block_strategy.is_some());
}
