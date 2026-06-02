//! CC-09a P2 — raw-wire `ProviderWireSink` per-provider wiring + per-auth-shape redaction.
//!
//! These tests drive the real `StreamProvider::stream()` code paths against a local
//! `wiremock` server (NO live network) and assert that, when a `ProviderWireSink` is
//! installed on `StreamConfig`, each provider:
//!   1. emits a `RawWire::Request` with a credential-free body / scrubbed URL,
//!   2. emits one `RawWire::ResponseFrame` per arriving SSE frame (in order), and
//!   3. emits a `RawWire::ResponseDone` at finalize.
//!
//! The redaction tier is load-bearing: no Bearer token / x-api-key / api-key / URL
//! query key / SigV4 credential may appear in any captured `RawWire` value.

use std::sync::{Arc, Mutex};

use phi_core::provider::{
    scrub_url_query, AnthropicProvider, ApiProtocol, AzureOpenAiProvider, BedrockProvider,
    GoogleProvider, GoogleVertexProvider, MockProvider, ModelConfig, OpenAiCompatProvider,
    OpenAiResponsesProvider, ProviderWireSink, RawWire, ResponseFormat, StreamConfig, StreamEvent,
    StreamProvider,
};
use phi_core::types::{CacheConfig, Message, ThinkingLevel};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A sink that records every `RawWire` event it receives (clones into owned form).
#[derive(Default)]
struct CollectingSink {
    events: Mutex<Vec<RawWire>>,
}

impl ProviderWireSink for CollectingSink {
    fn on_wire(&self, ev: &RawWire) {
        self.events.lock().unwrap().push(ev.clone());
    }
}

impl CollectingSink {
    fn snapshot(&self) -> Vec<RawWire> {
        self.events.lock().unwrap().clone()
    }
    fn request_bodies(&self) -> Vec<String> {
        self.snapshot()
            .into_iter()
            .filter_map(|e| match e {
                RawWire::Request { body, .. } => Some(body),
                _ => None,
            })
            .collect()
    }
    fn frame_count(&self) -> usize {
        self.snapshot()
            .iter()
            .filter(|e| matches!(e, RawWire::ResponseFrame { .. }))
            .count()
    }
    fn done_count(&self) -> usize {
        self.snapshot()
            .iter()
            .filter(|e| matches!(e, RawWire::ResponseDone { .. }))
            .count()
    }
    /// All captured strings across every variant — the redaction net.
    fn all_captured_text(&self) -> String {
        self.snapshot()
            .into_iter()
            .map(|e| match e {
                RawWire::Request {
                    provider_id,
                    model_id,
                    body,
                } => format!("{provider_id}|{model_id}|{body}"),
                RawWire::ResponseFrame {
                    provider_id,
                    model_id,
                    raw_frame,
                    ..
                } => format!("{provider_id}|{model_id}|{raw_frame}"),
                RawWire::ResponseDone {
                    provider_id,
                    model_id,
                    message_summary,
                } => format!("{provider_id}|{model_id}|{message_summary}"),
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

fn base_config(model_config: ModelConfig, sink: &Arc<CollectingSink>) -> StreamConfig {
    StreamConfig {
        model_config,
        system_prompt: "You are helpful.".into(),
        messages: vec![Message::user("Add 17 + 25")],
        tools: vec![],
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        cache_config: CacheConfig::default(),
        response_format: ResponseFormat::Text,
        provider_wire_sink: Some(sink.clone() as Arc<dyn ProviderWireSink>),
    }
}

/// Drain the StreamEvent channel to completion (we only assert on the sink).
async fn drain(mut rx: mpsc::UnboundedReceiver<StreamEvent>) {
    while rx.recv().await.is_some() {}
}

// ===========================================================================
// Tier F1-providers
// ===========================================================================

#[tokio::test]
async fn test_mock_wire_sink_invoked_on_request_and_frame() {
    // mock is fully deterministic with NO network.
    let sink = Arc::new(CollectingSink::default());
    let provider = MockProvider::text("42");
    let config = base_config(ModelConfig::anthropic("mock", "Mock", "k"), &sink);

    let (tx, rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let drainer = tokio::spawn(drain(rx));
    provider.stream(config, tx, cancel).await.unwrap();
    drainer.await.unwrap();

    assert_eq!(sink.request_bodies().len(), 1, "exactly one Request");
    assert!(sink.frame_count() >= 1, "at least one ResponseFrame");
    assert_eq!(sink.done_count(), 1, "exactly one ResponseDone");
    // The default (no-sink) path must be inert: a fresh config with None emits nothing.
    let inert_sink = Arc::new(CollectingSink::default());
    let mut inert = base_config(ModelConfig::anthropic("mock", "Mock", "k"), &inert_sink);
    inert.provider_wire_sink = None;
    let provider2 = MockProvider::text("42");
    let (tx2, rx2) = mpsc::unbounded_channel();
    let drainer2 = tokio::spawn(drain(rx2));
    provider2
        .stream(inert, tx2, CancellationToken::new())
        .await
        .unwrap();
    drainer2.await.unwrap();
    assert_eq!(inert_sink.snapshot().len(), 0, "None sink is inert");
}

/// Build an OpenAI-compat SSE body (two reasoning/text frames + [DONE]).
fn openai_sse_body() -> String {
    [
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"4\"},\"finish_reason\":null}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"2\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat()
}

async fn openai_compat_against(server: &MockServer, sink: &Arc<CollectingSink>) {
    Mock::given(method("POST"))
        .respond_with(
            // `set_body_raw` sets the content-type alongside the body; `set_body_string`
            // would force `text/plain`, which `reqwest-eventsource` rejects.
            ResponseTemplate::new(200)
                .set_body_raw(openai_sse_body().into_bytes(), "text/event-stream"),
        )
        .mount(server)
        .await;

    let mut model = ModelConfig::openai("gpt-4o", "GPT-4o", "sk-SECRET-BEARER-TOKEN");
    model.base_url = server.uri();
    let config = base_config(model, sink);

    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    OpenAiCompatProvider
        .stream(config, tx, CancellationToken::new())
        .await
        .unwrap();
    drainer.await.unwrap();
}

#[tokio::test]
async fn test_openai_compat_wire_sink_invoked_on_request_and_frame() {
    let server = MockServer::start().await;
    let sink = Arc::new(CollectingSink::default());
    openai_compat_against(&server, &sink).await;

    assert_eq!(sink.request_bodies().len(), 1);
    assert!(
        sink.frame_count() >= 2,
        "expected >=2 frames, got {}",
        sink.frame_count()
    );
    assert_eq!(sink.done_count(), 1);
}

// ===========================================================================
// Tier F1-redaction (MUST-SHIP) — per auth shape
// ===========================================================================

#[tokio::test]
async fn test_wire_sink_redacts_no_bearer_token() {
    // openai_compat / openai_responses: Bearer rides the `authorization` header; the
    // captured Request body must NOT contain the secret token.
    let server = MockServer::start().await;
    let sink = Arc::new(CollectingSink::default());
    openai_compat_against(&server, &sink).await;

    let captured = sink.all_captured_text();
    assert!(
        !captured.contains("sk-SECRET-BEARER-TOKEN"),
        "Bearer token leaked into captured wire: {captured}"
    );
    assert!(
        !captured.to_lowercase().contains("bearer "),
        "Authorization Bearer header leaked into captured wire"
    );
    // The request body should still be present (non-empty) and carry the model id.
    assert!(sink.request_bodies()[0].contains("gpt-4o"));
}

#[tokio::test]
async fn test_wire_sink_redacts_no_x_api_key() {
    // anthropic: x-api-key rides a header; body is token-free. We don't redirect the
    // anthropic const API_URL, so the request fails fast at the network layer — but the
    // RawWire::Request is emitted BEFORE the transport, so we can still assert redaction.
    let sink = Arc::new(CollectingSink::default());
    let model = ModelConfig::anthropic("claude-x", "Claude", "sk-ant-SECRET-XAPIKEY");
    // anthropic uses a const API_URL; the RawWire::Request is emitted pre-transport so we
    // cancel immediately and bound the call with a timeout to keep the test hermetic.
    let config = base_config(model, &sink);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        AnthropicProvider.stream(config, tx, cancel),
    )
    .await;
    drainer.await.unwrap();

    assert_eq!(
        sink.request_bodies().len(),
        1,
        "Request emitted pre-transport"
    );
    let captured = sink.all_captured_text();
    assert!(
        !captured.contains("sk-ant-SECRET-XAPIKEY"),
        "x-api-key leaked into captured wire: {captured}"
    );
}

#[tokio::test]
async fn test_wire_sink_redacts_url_key() {
    // google: the API key rides the URL query (`?...&key=<KEY>`). The captured Request
    // must carry a scrubbed URL with NO key, and the body must be credential-free.
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::google("gemini-2.5-flash", "Gemini", "AIzaSECRETURLKEY");
    // Point at an unroutable base_url so no live network call; Request is emitted first.
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = GoogleProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();

    assert_eq!(sink.request_bodies().len(), 1);
    let captured = sink.all_captured_text();
    assert!(
        !captured.contains("AIzaSECRETURLKEY"),
        "URL API key leaked into captured wire: {captured}"
    );
    assert!(
        captured.contains("<redacted>"),
        "expected scrubbed URL marker in captured google Request"
    );

    // Helper-level guarantee: scrub_url_query strips the entire query (incl. key=).
    let scrubbed = scrub_url_query(
        "https://generativelanguage.googleapis.com/v1beta/models/x:streamGenerateContent?alt=sse&key=AIzaSECRETURLKEY",
    );
    assert!(!scrubbed.contains("AIzaSECRETURLKEY"));
    assert!(!scrubbed.contains("key="));
    assert!(scrubbed.ends_with("?<redacted>"));
}

#[tokio::test]
async fn test_wire_sink_redacts_sigv4_creds() {
    // bedrock: api_key is "access_key:secret[:session_token]"; SigV4 signing happens
    // AFTER the sink fires. No access-key / secret / session-token / authorization may
    // appear in any captured RawWire value.
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::anthropic(
        "anthropic.claude-3",
        "Bedrock Claude",
        "AKIAACCESSKEY:SECRETSIGNINGKEY:SESSIONTOKEN123",
    );
    model.api = ApiProtocol::BedrockConverseStream;
    model.provider = "bedrock".into();
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = BedrockProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();

    assert_eq!(sink.request_bodies().len(), 1, "Request emitted pre-SigV4");
    let captured = sink.all_captured_text();
    for secret in ["AKIAACCESSKEY", "SECRETSIGNINGKEY", "SESSIONTOKEN123"] {
        assert!(
            !captured.contains(secret),
            "SigV4 credential `{secret}` leaked into captured wire: {captured}"
        );
    }
    assert!(
        !captured.to_lowercase().contains("bearer "),
        "Bearer fallback credential leaked into captured wire"
    );
}

// ===========================================================================
// Tier F1-providers (remaining 5 network providers) — request-emit + redaction
// reachable without live network (Request fires pre-transport; SSE-decode covered
// for the EventSource providers via wiremock above for openai_compat).
// ===========================================================================

#[tokio::test]
async fn test_anthropic_wire_sink_invoked_on_request_and_frame() {
    // anthropic uses a const API_URL we cannot redirect; the Request is emitted
    // pre-transport. Pre-cancel so the select! loop returns before any connection,
    // and bound with a timeout for hermeticity.
    let sink = Arc::new(CollectingSink::default());
    let model = ModelConfig::anthropic("claude-x", "Claude", "sk-ant-KEY");
    let config = base_config(model, &sink);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        AnthropicProvider.stream(config, tx, cancel),
    )
    .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
}

#[tokio::test]
async fn test_openai_responses_wire_sink_invoked_on_request_and_frame() {
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::openai("gpt-5", "GPT-5", "sk-SECRET");
    model.api = ApiProtocol::OpenAiResponses;
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = OpenAiResponsesProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
    assert!(!sink.all_captured_text().contains("sk-SECRET"));
}

#[tokio::test]
async fn test_azure_openai_wire_sink_invoked_on_request_and_frame() {
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::openai("gpt-4o", "Azure GPT", "AZURE-SECRET-APIKEY");
    model.api = ApiProtocol::AzureOpenAiResponses;
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = AzureOpenAiProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
    assert!(!sink.all_captured_text().contains("AZURE-SECRET-APIKEY"));
}

#[tokio::test]
async fn test_google_wire_sink_invoked_on_request_and_frame() {
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::google("gemini-2.5-flash", "Gemini", "AIzaKEY");
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = GoogleProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
    assert!(!sink.all_captured_text().contains("AIzaKEY"));
}

#[tokio::test]
async fn test_google_vertex_wire_sink_invoked_on_request_and_frame() {
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::google("gemini-2.5-pro", "Vertex Gemini", "ya29.OAUTHTOKEN");
    model.api = ApiProtocol::GoogleVertex;
    model.provider = "vertex".into();
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = GoogleVertexProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
    assert!(
        !sink.all_captured_text().contains("ya29.OAUTHTOKEN"),
        "Vertex OAuth token leaked into captured wire"
    );
}

#[tokio::test]
async fn test_bedrock_wire_sink_invoked_on_request_and_frame() {
    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::anthropic("anthropic.claude-3", "Bedrock", "AK:SECRET:TOKEN");
    model.api = ApiProtocol::BedrockConverseStream;
    model.provider = "bedrock".into();
    model.base_url = "http://127.0.0.1:1".into();
    let config = base_config(model, &sink);
    let (tx, rx) = mpsc::unbounded_channel();
    let drainer = tokio::spawn(drain(rx));
    let _ = BedrockProvider
        .stream(config, tx, CancellationToken::new())
        .await;
    drainer.await.unwrap();
    assert_eq!(sink.request_bodies().len(), 1);
}

// ===========================================================================
// Tier F2 — reasoning-text mapping across thinking arms (CC-09a P3)
//
// These drive the real provider `stream()` decode loop against a wiremock SSE
// body and collect `StreamEvent::ThinkingDelta` / `StreamEvent::TextDelta` to
// assert reasoning TEXT (not just the count) reaches the Thinking block.
// ===========================================================================

/// Drain the StreamEvent channel and return (thinking_text, visible_text)
/// reconstructed from ThinkingDelta / TextDelta events.
async fn collect_thinking_and_text(
    mut rx: mpsc::UnboundedReceiver<StreamEvent>,
) -> (String, String) {
    let mut thinking = String::new();
    let mut text = String::new();
    while let Some(ev) = rx.recv().await {
        match ev {
            StreamEvent::ThinkingDelta { delta, .. } => thinking.push_str(&delta),
            StreamEvent::TextDelta { delta, .. } => text.push_str(&delta),
            _ => {}
        }
    }
    (thinking, text)
}

/// gpt-oss-via-OpenRouter captured-shape SSE: each reasoning frame carries the
/// SAME text on BOTH `delta.reasoning` (string) AND
/// `delta.reasoning_details[{type:"reasoning.text",text,...}]` — the de-dup case.
/// Frame shapes are modelled on the verbatim p0-findings raw-frame sample.
fn gpt_oss_openrouter_sse_body() -> String {
    [
        // Reasoning frame 1 — reasoning text mirrored on both fields, empty content.
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"\",\"role\":\"assistant\",\
          \"reasoning\":\"We just need to solve 17 + 25.\",\
          \"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\"We just need to solve 17 + 25.\",\"format\":\"unknown\",\"index\":0}]},\"finish_reason\":null}]}\n\n",
        // Reasoning frame 2 — same mirror.
        "data: {\"choices\":[{\"index\":0,\"delta\":{\
          \"reasoning\":\" This is 42.\",\
          \"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\" This is 42.\",\"format\":\"unknown\",\"index\":0}]},\"finish_reason\":null}]}\n\n",
        // Visible answer streams on delta.content.
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"42\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    ]
    .concat()
}

#[tokio::test]
async fn test_openrouter_gpt_oss_reasoning_text_mapped() {
    // The OpenRouter arm must capture gpt-oss reasoning text into the Thinking
    // block — and must NOT double-append (it arrives on both `delta.reasoning`
    // and `reasoning_details[].text` per frame).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            gpt_oss_openrouter_sse_body().into_bytes(),
            "text/event-stream",
        ))
        .mount(&server)
        .await;

    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::openrouter("openai/gpt-oss-20b", "sk-or-SECRET");
    model.base_url = server.uri();
    let config = base_config(model, &sink);

    let (tx, rx) = mpsc::unbounded_channel();
    let collector = tokio::spawn(collect_thinking_and_text(rx));
    OpenAiCompatProvider
        .stream(config, tx, CancellationToken::new())
        .await
        .unwrap();
    let (thinking, text) = collector.await.unwrap();

    // Reasoning text reached the Thinking block (the T12 symptom is closed).
    assert_eq!(
        thinking, "We just need to solve 17 + 25. This is 42.",
        "gpt-oss reasoning text must reach Thinking exactly once (de-dup)"
    );
    // De-dup: the text must appear EXACTLY ONCE, not twice (once from
    // delta.reasoning + once from reasoning_details).
    assert_eq!(
        thinking.matches("This is 42.").count(),
        1,
        "reasoning text double-appended (de-dup failed): {thinking}"
    );
    // The visible answer is unaffected.
    assert_eq!(text, "42");
}

/// Per-arm regression: feed each at-risk thinking arm its native reasoning shape
/// and assert reasoning text reaches the Thinking block. Covers the OpenRouter
/// arm (reasoning_details `reasoning.text`-only, no `delta.reasoning`), the Xai
/// arm (`delta.reasoning` string), and the OpenAi/default arm
/// (`delta.reasoning_content`).
#[tokio::test]
async fn test_all_thinking_arms_capture_reasoning_text() {
    // Arm 1 — OpenRouter, reasoning_details ONLY (no top-level delta.reasoning):
    // exercises the broadened filter (any text-bearing entry, not just type=="thinking").
    {
        let server = MockServer::start().await;
        let body = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\
              \"reasoning_details\":[{\"type\":\"reasoning.text\",\"text\":\"deliberating\",\"index\":0}]},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat();
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let sink = Arc::new(CollectingSink::default());
        let mut model = ModelConfig::openrouter("some/model", "sk-or-K");
        model.base_url = server.uri();
        let config = base_config(model, &sink);
        let (tx, rx) = mpsc::unbounded_channel();
        let collector = tokio::spawn(collect_thinking_and_text(rx));
        OpenAiCompatProvider
            .stream(config, tx, CancellationToken::new())
            .await
            .unwrap();
        let (thinking, text) = collector.await.unwrap();
        assert_eq!(thinking, "deliberating", "OpenRouter reasoning_details arm");
        assert_eq!(text, "ok");
    }

    // Arm 2 — Xai, top-level `delta.reasoning` string. MUST stay green (unchanged).
    {
        let server = MockServer::start().await;
        let body = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning\":\"grok-thinks\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"done\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat();
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let sink = Arc::new(CollectingSink::default());
        // xAI factory shape: openai-compat with ThinkingFormat::Xai.
        let mut model = ModelConfig::openai("grok-2", "Grok", "xai-K");
        model.provider = "xai".into();
        model.compat = Some(phi_core::provider::OpenAiCompat::xai());
        model.base_url = server.uri();
        let config = base_config(model, &sink);
        let (tx, rx) = mpsc::unbounded_channel();
        let collector = tokio::spawn(collect_thinking_and_text(rx));
        OpenAiCompatProvider
            .stream(config, tx, CancellationToken::new())
            .await
            .unwrap();
        let (thinking, text) = collector.await.unwrap();
        assert_eq!(thinking, "grok-thinks", "Xai delta.reasoning arm");
        assert_eq!(text, "done");
    }

    // Arm 3 — OpenAi/default, `delta.reasoning_content`. MUST stay green (unchanged).
    {
        let server = MockServer::start().await;
        let body = [
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"reasoning_content\":\"o-series-cot\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\n",
            "data: [DONE]\n\n",
        ]
        .concat();
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream"),
            )
            .mount(&server)
            .await;
        let sink = Arc::new(CollectingSink::default());
        let mut model = ModelConfig::openai("o3", "O3", "sk-K");
        model.base_url = server.uri();
        let config = base_config(model, &sink);
        let (tx, rx) = mpsc::unbounded_channel();
        let collector = tokio::spawn(collect_thinking_and_text(rx));
        OpenAiCompatProvider
            .stream(config, tx, CancellationToken::new())
            .await
            .unwrap();
        let (thinking, text) = collector.await.unwrap();
        assert_eq!(thinking, "o-series-cot", "OpenAi reasoning_content arm");
        assert_eq!(text, "answer");
    }
}

/// google (Gemini 2.5): a part flagged `thought: true` is reasoning and must
/// route to the Thinking block; a non-thought text part is the visible answer.
#[tokio::test]
async fn test_google_thought_part_maps_to_thinking() {
    let server = MockServer::start().await;
    // One Gemini SSE frame with two parts: a thought part + a visible part.
    let body = ["data: {\"candidates\":[{\"content\":{\"parts\":[\
          {\"text\":\"Let me add the numbers\",\"thought\":true},\
          {\"text\":\"42\"}]},\"finishReason\":\"STOP\"}]}\n\n"]
    .concat();
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(body.into_bytes(), "text/event-stream"),
        )
        .mount(&server)
        .await;

    let sink = Arc::new(CollectingSink::default());
    let mut model = ModelConfig::google("gemini-2.5-flash", "Gemini", "AIzaKEY");
    model.base_url = server.uri();
    let config = base_config(model, &sink);

    let (tx, rx) = mpsc::unbounded_channel();
    let collector = tokio::spawn(collect_thinking_and_text(rx));
    GoogleProvider
        .stream(config, tx, CancellationToken::new())
        .await
        .unwrap();
    let (thinking, text) = collector.await.unwrap();

    assert_eq!(
        thinking, "Let me add the numbers",
        "Gemini thought:true part must route to Thinking"
    );
    assert_eq!(text, "42", "non-thought part stays the visible answer");
}
