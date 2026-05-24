//! Tests for the 0.9.0 per-turn debug-capture surface:
//! `AgentEvent::TurnRequest` emission, `SessionRecorderConfig::capture_turn_requests`
//! opt-in persistence onto `Turn::request_payload`, and provenance derivation
//! rules covering `BlockProvenance::LoopTurn { turn_index, role, message_index }`.
//!
//! These tests load-bear the canonical contract: a developer enabling
//! `capture_turn_requests: true` MUST be able to reconstruct the exact wire
//! payload the model received, with per-block provenance.

use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
use phi_core::provider::mock::*;
use phi_core::provider::{
    MockProvider, ModelConfig, ProviderError, StreamConfig, StreamEvent, StreamProvider,
};
use phi_core::session::{SessionRecorder, SessionRecorderConfig};
use phi_core::*;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_config(provider: Arc<dyn StreamProvider>) -> AgentLoopConfig {
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
        revert_pending: None,
    }
}

fn fresh_context(system_prompt: &str) -> AgentContext {
    AgentContext {
        system_prompt: system_prompt.into(),
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
        active_node_id: None,
        next_node_id: 0,
    }
}

fn drain_events(mut rx: mpsc::UnboundedReceiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    events
}

// ---------------------------------------------------------------------------
// CapturingMockProvider — wraps MockProvider so tests can inspect what
// StreamConfig.messages the provider actually received (gold-standard
// cross-check for byte-equality against captured TurnRequest payload).
// ---------------------------------------------------------------------------

struct CapturingMockProvider {
    inner: MockProvider,
    captured_inputs: Mutex<Vec<Vec<Message>>>,
    captured_system_prompts: Mutex<Vec<String>>,
}

impl CapturingMockProvider {
    fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            inner: MockProvider::new(responses),
            captured_inputs: Mutex::new(Vec::new()),
            captured_system_prompts: Mutex::new(Vec::new()),
        }
    }

    fn captured_messages(&self) -> Vec<Vec<Message>> {
        self.captured_inputs.lock().unwrap().clone()
    }

    fn captured_system_prompts(&self) -> Vec<String> {
        self.captured_system_prompts.lock().unwrap().clone()
    }
}

#[async_trait::async_trait]
impl StreamProvider for CapturingMockProvider {
    fn provider_id(&self) -> &str {
        "capturing-mock"
    }

    async fn stream(
        &self,
        config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Message, ProviderError> {
        // Capture the exact wire-format input BEFORE delegating to inner.
        self.captured_inputs
            .lock()
            .unwrap()
            .push(config.messages.clone());
        self.captured_system_prompts
            .lock()
            .unwrap()
            .push(config.system_prompt.clone());
        self.inner.stream(config, tx, cancel).await
    }
}

// ---------------------------------------------------------------------------
// 1. turn_request_emitted_once_per_turn
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_request_emitted_once_per_turn() {
    // 2 turns: first turn calls a tool, second turn returns text.
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "echo".into(),
            arguments: serde_json::json!({"text": "hi"}),
        }]),
        MockResponse::Text("done".into()),
    ]);

    struct EchoTool;
    #[async_trait::async_trait]
    impl AgentTool for EchoTool {
        fn name(&self) -> &str {
            "echo"
        }
        fn label(&self) -> &str {
            "Echo"
        }
        fn description(&self) -> &str {
            "echo back input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![Content::Text { text: "hi".into() }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let config = make_config(Arc::new(provider));
    let mut context = fresh_context("system");
    context.tools = vec![Arc::new(EchoTool)];

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("call echo")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = drain_events(rx);

    let turn_request_count = events
        .iter()
        .filter(|e| matches!(e, AgentEvent::TurnRequest { .. }))
        .count();
    assert_eq!(
        turn_request_count, 2,
        "expected exactly one TurnRequest per turn (2 turns)"
    );

    // Ensure turn_index is monotonically increasing starting at 0.
    let turn_indices: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnRequest { turn_index, .. } => Some(*turn_index),
            _ => None,
        })
        .collect();
    assert_eq!(turn_indices, vec![0, 1]);
}

// ---------------------------------------------------------------------------
// 2. turn_request_payload_matches_provider_input
// ---------------------------------------------------------------------------

#[tokio::test]
async fn turn_request_payload_matches_provider_input() {
    let provider = Arc::new(CapturingMockProvider::new(vec![MockResponse::Text(
        "ok".into(),
    )]));
    let config = make_config(provider.clone());
    let mut context = fresh_context("system-prompt-A");

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hello")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = drain_events(rx);

    // Extract the payload from the TurnRequest event.
    let captured_payload = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnRequest { payload, .. } => Some(payload.clone()),
            _ => None,
        })
        .expect("expected at least one TurnRequest event");

    // Cross-check against provider input.
    let provider_inputs = provider.captured_messages();
    let provider_system_prompts = provider.captured_system_prompts();
    assert_eq!(
        provider_inputs.len(),
        1,
        "expected exactly one provider call"
    );

    // Byte-equality: JSON-serialize both sides and compare.
    let payload_messages_json = serde_json::to_value(&captured_payload.messages).unwrap();
    let provider_messages_json = serde_json::to_value(&provider_inputs[0]).unwrap();
    assert_eq!(
        payload_messages_json, provider_messages_json,
        "TurnRequest.payload.messages must match StreamConfig.messages byte-for-byte"
    );

    assert_eq!(
        captured_payload.system_prompt, provider_system_prompts[0],
        "TurnRequest.payload.system_prompt must match StreamConfig.system_prompt"
    );

    // Provenance length matches messages length.
    assert_eq!(
        captured_payload.provenance.len(),
        captured_payload.messages.len(),
        "provenance vec must be parallel-indexed to messages"
    );
}

// ---------------------------------------------------------------------------
// 3. recorder_round_trips_when_capture_enabled
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recorder_round_trips_when_capture_enabled() {
    let provider = MockProvider::text("ok");
    let config = make_config(Arc::new(provider));
    let mut context = fresh_context("system");

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hello")));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    // Feed events into a recorder with capture_turn_requests = true.
    let recorder_handle = tokio::spawn(async move {
        let mut recorder = SessionRecorder::new(SessionRecorderConfig {
            capture_turn_requests: true,
            ..Default::default()
        });
        while let Some(ev) = rx.recv().await {
            recorder.on_event(ev);
        }
        recorder.flush();
        recorder
    });

    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let recorder = recorder_handle.await.unwrap();

    // Find the session and walk to the turn.
    let session = recorder
        .sessions()
        .next()
        .expect("expected at least one session");
    let loop_record = session.loops.first().expect("expected at least one loop");
    let turn = loop_record
        .turns
        .first()
        .expect("expected at least one materialized turn");
    assert!(
        turn.request_payload.is_some(),
        "Turn.request_payload must be Some when capture_turn_requests is true"
    );
    let payload = turn.request_payload.as_ref().unwrap();
    assert_eq!(payload.system_prompt, "system");

    // JSON ser-de round-trip — assert the persisted payload survives unchanged.
    let json = serde_json::to_string(turn).unwrap();
    let back: phi_core::session::Turn = serde_json::from_str(&json).unwrap();
    assert!(
        back.request_payload.is_some(),
        "request_payload must survive JSON round-trip"
    );
    assert_eq!(
        back.request_payload.as_ref().unwrap().system_prompt,
        "system"
    );
}

// ---------------------------------------------------------------------------
// 4. recorder_default_off
// ---------------------------------------------------------------------------

#[tokio::test]
async fn recorder_default_off() {
    let provider = MockProvider::text("ok");
    let config = make_config(Arc::new(provider));
    let mut context = fresh_context("system");

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hello")));
    let (tx, mut rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let recorder_handle = tokio::spawn(async move {
        // Default config — capture_turn_requests is false.
        let mut recorder = SessionRecorder::new(SessionRecorderConfig::default());
        while let Some(ev) = rx.recv().await {
            recorder.on_event(ev);
        }
        recorder.flush();
        recorder
    });

    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let recorder = recorder_handle.await.unwrap();

    let session = recorder
        .sessions()
        .next()
        .expect("expected at least one session");
    let loop_record = session.loops.first().expect("expected at least one loop");
    let turn = loop_record
        .turns
        .first()
        .expect("expected at least one materialized turn");
    assert!(
        turn.request_payload.is_none(),
        "Turn.request_payload must be None by default (capture_turn_requests = false)"
    );
}

// ---------------------------------------------------------------------------
// 5. provenance_tags_loop_turns_correctly
// ---------------------------------------------------------------------------

#[tokio::test]
async fn provenance_tags_loop_turns_correctly() {
    // Multi-turn run that exercises the LoopTurn { turn_index, role, message_index }
    // derivation across User, Assistant (text), Assistant (tool call), and ToolResult
    // message variants.
    //
    // Turn 0 input: user message → response: tool call
    // Turn 1 input: tool result (loop carries it forward) → response: text
    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "noop".into(),
            arguments: serde_json::json!({}),
        }]),
        MockResponse::Text("final answer".into()),
    ]);

    struct NoopTool;
    #[async_trait::async_trait]
    impl AgentTool for NoopTool {
        fn name(&self) -> &str {
            "noop"
        }
        fn label(&self) -> &str {
            "Noop"
        }
        fn description(&self) -> &str {
            "noop"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {}})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![Content::Text {
                    text: "done".into(),
                }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let config = make_config(Arc::new(provider));
    let mut context = fresh_context("system");
    context.tools = vec![Arc::new(NoopTool)];

    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("trigger tool")));
    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let _ = agent_loop(vec![prompt], &mut context, &config, tx, cancel).await;
    let events = drain_events(rx);

    // Collect all TurnRequest payloads in order.
    let payloads: Vec<AnnotatedRequestPayload> = events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TurnRequest { payload, .. } => Some(payload.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(payloads.len(), 2, "expected 2 TurnRequest payloads");

    // Turn 0 payload: should contain the initial user message tagged as
    // LoopTurn { turn_index: 0, role: UserMessage, message_index: 0 } (the
    // current turn's input has been turn-id-stamped by the loop).
    let p0 = &payloads[0];
    assert!(
        !p0.provenance.is_empty(),
        "turn-0 payload should have provenance entries"
    );
    let has_turn0_user = p0.provenance.iter().any(|p| {
        matches!(
            p,
            BlockProvenance::LoopTurn {
                turn_index: 0,
                role: ProvenanceRole::UserMessage,
                ..
            } | BlockProvenance::Steering
        )
    });
    assert!(
        has_turn0_user,
        "turn-0 input must be tagged LoopTurn(turn=0, UserMessage) or Steering; got {:?}",
        p0.provenance
    );

    // Turn 1 payload: should contain history with at least one
    // LoopTurn entry tagged as AssistantResponse / ToolCallRequest /
    // ToolCallResult across the prior turn's content.
    let p1 = &payloads[1];
    assert!(
        p1.provenance.len() > p0.provenance.len(),
        "turn-1 payload should carry more history than turn 0"
    );
    let saw_tool_call_request = p1.provenance.iter().any(|p| {
        matches!(
            p,
            BlockProvenance::LoopTurn {
                role: ProvenanceRole::ToolCallRequest,
                ..
            }
        )
    });
    let saw_tool_call_result = p1.provenance.iter().any(|p| {
        matches!(
            p,
            BlockProvenance::LoopTurn {
                role: ProvenanceRole::ToolCallResult,
                ..
            }
        )
    });
    assert!(
        saw_tool_call_request,
        "turn-1 history must include LoopTurn with ToolCallRequest; got {:?}",
        p1.provenance
    );
    assert!(
        saw_tool_call_result,
        "turn-1 history must include LoopTurn with ToolCallResult; got {:?}",
        p1.provenance
    );
}
