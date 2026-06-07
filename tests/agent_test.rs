//! Tests for the Agent struct (stateful wrapper).

use phi_core::provider::mock::*;
use phi_core::provider::{MockProvider, ModelConfig};
use phi_core::BasicAgent;
use phi_core::{LlmMessage, *};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::test]
async fn test_agent_simple_prompt() {
    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("You are helpful.");

    let rx = agent.prompt("Hi there").await;

    // Drain events
    let mut events = Vec::new();
    let mut rx = rx;
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }

    assert!(!events.is_empty());
    assert_eq!(agent.messages().len(), 2); // user + assistant
}

#[tokio::test]
async fn test_agent_reset() {
    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    let _ = agent.prompt("Hi").await;
    assert!(!agent.messages().is_empty());

    agent.reset();
    assert!(agent.messages().is_empty());
    assert!(!agent.is_streaming());
}

#[tokio::test]
async fn test_agent_with_tools() {
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
            "Echoes input"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"text": {"type": "string"}}})
        }
        async fn execute(
            &self,
            params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            let text = params["text"].as_str().unwrap_or("").to_string();
            Ok(ToolResult {
                content: vec![Content::Text { text }],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    let provider = MockProvider::new(vec![
        MockResponse::ToolCalls(vec![MockToolCall {
            name: "echo".into(),
            arguments: serde_json::json!({"text": "hello"}),
        }]),
        MockResponse::Text("Echoed: hello".into()),
    ]);

    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test")
        .with_tools(vec![Arc::new(EchoTool)]);

    let _ = agent.prompt("Echo hello").await;

    // user + assistant(tool_call) + toolResult + assistant(text)
    assert_eq!(agent.messages().len(), 4);
}

#[tokio::test]
async fn test_agent_builder_pattern() {
    let provider = MockProvider::text("ok");
    let agent = BasicAgent::new(ModelConfig::anthropic("test-model", "test-model", "key123"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("sys")
        .with_thinking(ThinkingLevel::Medium)
        .with_max_tokens(4096);

    assert_eq!(agent.system_prompt, "sys");
    assert_eq!(agent.model_config.id, "test-model");
    assert_eq!(agent.model_config.api_key, "key123");
    assert_eq!(agent.thinking_level, ThinkingLevel::Medium);
    assert_eq!(agent.max_tokens, Some(4096));
}

// ---------------------------------------------------------------------------
// State persistence tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_with_messages_builder() {
    let saved = vec![
        AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))),
        AgentMessage::Llm(LlmMessage::new(Message::Assistant {
            content: vec![Content::Text {
                text: "Hi there!".into(),
            }],
            stop_reason: StopReason::Stop,
            model: "mock".into(),
            provider: "mock".into(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        })),
    ];

    let provider = MockProvider::text("ok");
    let agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_messages(saved.clone());

    assert_eq!(agent.messages().len(), 2);
    assert_eq!(*agent.messages(), saved[..]);
}

#[tokio::test]
async fn test_save_and_restore_messages() {
    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    let _ = agent.prompt("Hi").await;
    let json = agent.save_messages().expect("save should succeed");

    // Create a fresh agent and restore
    let provider2 = MockProvider::text("ok");
    let mut agent2 = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider2))
        .with_system_prompt("test");

    agent2
        .restore_messages(&json)
        .expect("restore should succeed");
    assert_eq!(agent.messages(), agent2.messages());
}

#[tokio::test]
async fn test_agent_continues_after_restore() {
    // First agent: prompt → get response → save
    let provider1 = MockProvider::text("First response");
    let mut agent1 = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider1))
        .with_system_prompt("test");

    let _ = agent1.prompt("Hello").await;
    let json = agent1.save_messages().expect("save");

    // Second agent: restore → prompt again
    // The MockProvider will receive the full restored history + new prompt
    let provider2 = MockProvider::text("Second response");
    let mut agent2 = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider2))
        .with_system_prompt("test");

    agent2.restore_messages(&json).expect("restore");
    let _ = agent2.prompt("Follow up").await;

    // Should have: original user + original assistant + follow-up user + new assistant
    assert_eq!(agent2.messages().len(), 4);
    assert_eq!(agent2.messages()[0].role(), "user");
    assert_eq!(agent2.messages()[1].role(), "assistant");
    assert_eq!(agent2.messages()[2].role(), "user");
    assert_eq!(agent2.messages()[3].role(), "assistant");
}

// ---------------------------------------------------------------------------
// Real-time streaming tests (prompt_with_sender / prompt_messages_with_sender)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_prompt_with_sender_streams_events() {
    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let event_count = Arc::new(AtomicUsize::new(0));
    let count_clone = event_count.clone();

    let consumer = tokio::spawn(async move {
        while let Some(_event) = rx.recv().await {
            count_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    agent.prompt_with_sender("Hi there", tx).await;

    // tx is dropped when prompt_with_sender returns, so consumer will finish
    consumer.await.unwrap();

    assert!(event_count.load(Ordering::SeqCst) > 0);
    assert_eq!(agent.messages().len(), 2); // user + assistant
    assert!(!agent.is_streaming());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_prompt_with_sender_real_time_streaming() {
    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    let (tx, mut rx) = mpsc::unbounded_channel();
    let received_during = Arc::new(AtomicUsize::new(0));
    let received_clone = received_during.clone();

    // On a multi-threaded runtime, the consumer can run concurrently
    let consumer = tokio::spawn(async move {
        while let Some(_event) = rx.recv().await {
            received_clone.fetch_add(1, Ordering::SeqCst);
        }
    });

    agent.prompt_with_sender("Hello", tx).await;
    consumer.await.unwrap();

    // Events were consumed by the concurrent task
    assert!(received_during.load(Ordering::SeqCst) > 0);
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn test_prompt_messages_with_sender() {
    let provider = MockProvider::text("Response");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    let (tx, mut rx) = mpsc::unbounded_channel();

    let consumer = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    });

    let msgs = vec![AgentMessage::Llm(LlmMessage::new(Message::user("Hello")))];
    agent.prompt_messages_with_sender(msgs, tx).await;

    let events = consumer.await.unwrap();
    assert!(!events.is_empty());
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn test_continue_loop_with_sender() {
    let provider = MockProvider::text("Continued response");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test");

    // First, add some messages to continue from (last must not be assistant)
    agent.append_message(AgentMessage::Llm(LlmMessage::new(Message::user("Hello"))));
    agent.append_message(AgentMessage::Llm(LlmMessage::new(Message::Assistant {
        content: vec![Content::Text { text: "Hi!".into() }],
        stop_reason: StopReason::Error,
        model: "mock".into(),
        provider: "mock".into(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: Some("rate limited".into()),
    })));
    agent.append_message(AgentMessage::Llm(LlmMessage::new(Message::user(
        "Please try again",
    ))));

    let (tx, mut rx) = mpsc::unbounded_channel();

    let consumer = tokio::spawn(async move {
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        events
    });

    agent
        .continue_loop_with_sender(tx, ContinuationKind::Default)
        .await;

    let events = consumer.await.unwrap();
    assert!(!events.is_empty());
    assert!(!agent.is_streaming());
}

#[tokio::test]
async fn test_prompt_with_sender_tools_restored() {
    struct DummyTool;

    #[async_trait::async_trait]
    impl AgentTool for DummyTool {
        fn name(&self) -> &str {
            "dummy"
        }
        fn label(&self) -> &str {
            "Dummy"
        }
        fn description(&self) -> &str {
            "A dummy tool"
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
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

    let provider = MockProvider::text("Hello!");
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(provider))
        .with_system_prompt("test")
        .with_tools(vec![Arc::new(DummyTool)]);

    let (tx, mut rx) = mpsc::unbounded_channel();
    let consumer = tokio::spawn(async move { while rx.recv().await.is_some() {} });

    agent.prompt_with_sender("Hi", tx).await;
    consumer.await.unwrap();

    // Tools should be restored after the call
    assert!(!agent.is_streaming());
    // Agent should still work for another prompt
    let rx2 = agent.prompt("Follow up").await;
    let mut rx2 = rx2;
    while rx2.try_recv().is_ok() {}
    assert_eq!(agent.messages().len(), 4); // 2 from first + 2 from second
}

#[tokio::test]
async fn test_repeated_prompt_renders_prior_history_to_provider() {
    // Regression guard for the `agent_loop` / `agent_loop_continue`
    // inconsistency: a SECOND `prompt()` on the same agent must render the
    // prior turn TO THE PROVIDER, not just the new message. Before `agent_loop`
    // classified pre-seeded messages into the working-context streams (mirroring
    // `agent_loop_continue`), `build_working_context` rendered only the new
    // `user_context` entry and silently dropped the conversation — the
    // accumulator (`messages().len()`) stayed correct, so the older
    // length-only assertion above did NOT catch it. The mock provider emits a
    // `RawWire::Request` whose body carries `"messages":N` = the rendered
    // `llm_messages.len()`, so a counting wire sink observes the actual sent
    // context size per call.
    use std::sync::Mutex;

    use phi_core::provider::{ProviderWireSink, RawWire};

    #[derive(Default)]
    struct CountingSink {
        counts: Mutex<Vec<usize>>,
    }
    impl ProviderWireSink for CountingSink {
        fn on_wire(&self, ev: &RawWire) {
            if let RawWire::Request { body, .. } = ev {
                let key = "\"messages\":";
                if let Some(i) = body.find(key) {
                    let digits: String = body[i + key.len()..]
                        .chars()
                        .take_while(|c| c.is_ascii_digit())
                        .collect();
                    if let Ok(n) = digits.parse::<usize>() {
                        self.counts.lock().unwrap().push(n);
                    }
                }
            }
        }
    }

    let sink = Arc::new(CountingSink::default());
    let mut agent = BasicAgent::new(ModelConfig::anthropic("mock", "mock", "test"))
        .with_provider_override(Arc::new(MockProvider::texts(vec!["first", "second"])))
        .with_system_prompt("test")
        .with_provider_wire_sink(sink.clone());

    {
        let mut rx = agent.prompt("Remember the value X.").await;
        while rx.recv().await.is_some() {}
    }
    {
        let mut rx = agent.prompt("What was the value?").await;
        while rx.recv().await.is_some() {}
    }

    let counts = sink.counts.lock().unwrap().clone();
    assert_eq!(counts.len(), 2, "expected two provider calls, got {counts:?}");
    // Call 1 renders just the first user message (1). Call 2 (the follow-up)
    // must carry the prior user+assistant pair + the new user message (>= 3),
    // not collapse back to the lone new turn (1).
    assert!(
        counts[1] > counts[0] && counts[1] >= 3,
        "second prompt() must render prior history to the provider; got {counts:?} \
         (call-2 == 1 means the prior turn was dropped)"
    );
}
