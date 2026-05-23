//! Tests for the CredentialProvider trait and Auth-error refresh-and-retry path (MEDIUM-4).

use async_trait::async_trait;
use phi_core::agent_loop::{agent_loop, AgentLoopConfig};
use phi_core::provider::{
    CredentialProvider, ModelConfig, ProviderError, StaticCredentialProvider, StreamConfig,
    StreamEvent, StreamProvider,
};
use phi_core::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// CredentialProvider that records every `current()` / `invalidate()` call and
/// returns a fixed key. Used for asserting the streaming-retry behaviour around
/// `ProviderError::Auth`.
#[derive(Debug)]
struct CountingCredentials {
    key: String,
    current_calls: AtomicUsize,
    invalidate_calls: AtomicUsize,
}

impl CountingCredentials {
    fn new(key: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            current_calls: AtomicUsize::new(0),
            invalidate_calls: AtomicUsize::new(0),
        }
    }
    fn current(&self) -> usize {
        self.current_calls.load(Ordering::SeqCst)
    }
    fn invalidates(&self) -> usize {
        self.invalidate_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl CredentialProvider for CountingCredentials {
    async fn current(&self) -> Result<String, ProviderError> {
        self.current_calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.key.clone())
    }
    async fn invalidate(&self) -> Result<(), ProviderError> {
        self.invalidate_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// `StreamProvider` that returns `ProviderError::Auth` on the first call and a
/// successful text response on subsequent calls. Records the call count so tests
/// can assert exactly N attempts were made.
struct FlakyAuthProvider {
    calls: AtomicUsize,
    text_after_auth: String,
}

impl FlakyAuthProvider {
    fn new(text: impl Into<String>) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            text_after_auth: text.into(),
        }
    }
    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl StreamProvider for FlakyAuthProvider {
    fn provider_id(&self) -> &str {
        "flaky-auth"
    }
    async fn stream(
        &self,
        config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        _cancel: CancellationToken,
    ) -> Result<Message, ProviderError> {
        // Mirror what real providers do: resolve the API key via the CredentialProvider
        // (when set) so the test asserts the integrated call path, not just the retry-loop
        // bookkeeping.
        let _api_key = config.model_config.resolve_api_key().await?;
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        if n == 0 {
            return Err(ProviderError::Auth(
                "synthetic auth failure for test".into(),
            ));
        }
        // Subsequent calls succeed.
        let _ = tx.send(StreamEvent::Start);
        let _ = tx.send(StreamEvent::TextDelta {
            content_index: 0,
            delta: self.text_after_auth.clone(),
        });
        Ok(Message::Assistant {
            content: vec![Content::Text {
                text: self.text_after_auth.clone(),
            }],
            stop_reason: StopReason::Stop,
            model: "flaky".into(),
            provider: "flaky".into(),
            usage: Usage::default(),
            timestamp: 0,
            error_message: None,
        })
    }
}

fn make_config(
    provider: Arc<dyn StreamProvider>,
    creds: Option<Arc<dyn CredentialProvider>>,
) -> AgentLoopConfig {
    let mut model_config = ModelConfig::anthropic("mock", "mock", "static-fallback");
    model_config.credentials = creds;

    AgentLoopConfig {
        model_config,
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
        retry_config: phi_core::RetryConfig::none(), // exclude RateLimited/Network retries from the count
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

fn make_context() -> AgentContext {
    AgentContext {
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
        active_node_id: None,
        next_node_id: 0,
    }
}

#[tokio::test]
async fn credentials_provider_current_is_called_when_set() {
    // Sanity: when `credentials` is set on `ModelConfig`, `resolve_api_key` delegates.
    let creds = Arc::new(CountingCredentials::new("rotated-key-1"));
    let resolved = {
        let mut mc = ModelConfig::anthropic("m", "n", "static");
        mc.credentials = Some(creds.clone());
        mc.resolve_api_key().await.unwrap()
    };
    assert_eq!(resolved, "rotated-key-1");
    assert_eq!(creds.current(), 1);
    assert_eq!(creds.invalidates(), 0);
}

#[tokio::test]
async fn static_credential_provider_returns_fixed_key() {
    let creds: Arc<dyn CredentialProvider> = Arc::new(StaticCredentialProvider::new("fixed-key"));
    assert_eq!(creds.current().await.unwrap(), "fixed-key");
    // Default invalidate() is a no-op; should not error.
    creds.invalidate().await.unwrap();
}

#[tokio::test]
async fn auth_error_triggers_single_refresh_and_retry_with_credentials() {
    let creds = Arc::new(CountingCredentials::new("rotated-key"));
    let provider = Arc::new(FlakyAuthProvider::new("recovered after refresh"));
    let config = make_config(provider.clone(), Some(creds.clone()));

    let mut ctx = make_context();
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;

    // Provider was called exactly twice: initial Auth, then post-refresh success.
    assert_eq!(provider.calls(), 2, "provider must be called exactly twice");
    // CredentialProvider::invalidate was called exactly once.
    assert_eq!(creds.invalidates(), 1, "invalidate must fire exactly once");
    // current() was called for both stream attempts (resolve_api_key on each).
    assert_eq!(
        creds.current(),
        2,
        "current must be called for both attempts"
    );

    // The final assistant message must be the post-refresh success text.
    let last = new_messages
        .iter()
        .rev()
        .find_map(|m| match m.as_llm() {
            Some(Message::Assistant { content, .. }) => Some(content.clone()),
            _ => None,
        })
        .expect("expected at least one assistant message");
    let text = match &last[0] {
        Content::Text { text } => text.clone(),
        other => panic!("expected Content::Text, got {:?}", other),
    };
    assert_eq!(text, "recovered after refresh");
}

#[tokio::test]
async fn auth_error_without_credentials_does_not_retry() {
    // Same flaky provider but `credentials = None` — Auth must propagate immediately.
    let provider = Arc::new(FlakyAuthProvider::new("should-not-appear"));
    let config = make_config(provider.clone(), None);

    let mut ctx = make_context();
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();

    let new_messages = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;

    // The provider was called exactly once: the Auth error propagated without retry.
    assert_eq!(
        provider.calls(),
        1,
        "without credentials, Auth must propagate after a single attempt"
    );

    // The loop surfaces a synthetic error Message::Assistant — verify it carries
    // the Auth failure text rather than the recovered string.
    let final_text: String = new_messages
        .iter()
        .find_map(|m| match m.as_llm() {
            Some(Message::Assistant {
                content,
                error_message,
                ..
            }) => Some(
                error_message
                    .clone()
                    .or_else(|| {
                        content.iter().find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                    })
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    assert!(
        !final_text.contains("recovered after refresh"),
        "without credentials, the recovery path must not run; got {:?}",
        final_text
    );
}

#[tokio::test]
async fn second_auth_error_propagates_after_refresh() {
    // Provider that ALWAYS returns Auth — refresh fires once, second Auth propagates.
    struct AlwaysAuthFails {
        calls: AtomicUsize,
    }
    #[async_trait]
    impl StreamProvider for AlwaysAuthFails {
        fn provider_id(&self) -> &str {
            "always-auth-fails"
        }
        async fn stream(
            &self,
            _c: StreamConfig,
            _tx: mpsc::UnboundedSender<StreamEvent>,
            _x: CancellationToken,
        ) -> Result<Message, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(ProviderError::Auth("always".into()))
        }
    }

    let provider = Arc::new(AlwaysAuthFails {
        calls: AtomicUsize::new(0),
    });
    let creds = Arc::new(CountingCredentials::new("rotated"));
    let config = make_config(provider.clone(), Some(creds.clone()));

    let mut ctx = make_context();
    let prompt = AgentMessage::Llm(LlmMessage::new(Message::user("hi")));
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let _ = agent_loop(vec![prompt], &mut ctx, &config, tx, cancel).await;

    // Exactly two attempts: original + one post-refresh retry; no further retries.
    assert_eq!(provider.calls.load(Ordering::SeqCst), 2);
    assert_eq!(creds.invalidates(), 1);
}
