//! Tests for `ResponseFormat`, `Message::extract_json`, and per-provider body wiring
//! (MEDIUM-7).

use phi_core::provider::{ModelConfig, ProviderError, ResponseFormat, StreamConfig};
use phi_core::*;

fn json_msg(text: impl Into<String>) -> Message {
    Message::Assistant {
        content: vec![Content::Text { text: text.into() }],
        stop_reason: StopReason::Stop,
        model: "test".into(),
        provider: "test".into(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    }
}

#[derive(Debug, serde::Deserialize, PartialEq)]
struct Answer {
    score: i32,
    label: String,
}

#[test]
fn extract_json_from_assistant_text_succeeds() {
    let m = json_msg(r#"{"score": 42, "label": "ok"}"#);
    let parsed: Answer = m.extract_json().unwrap();
    assert_eq!(
        parsed,
        Answer {
            score: 42,
            label: "ok".into()
        }
    );
}

#[test]
fn extract_json_concatenates_multiple_text_blocks() {
    // Some providers may emit JSON across two text blocks under streaming.
    let m = Message::Assistant {
        content: vec![
            Content::Text {
                text: r#"{"score": 1, "#.into(),
            },
            Content::Text {
                text: r#""label": "split"}"#.into(),
            },
        ],
        stop_reason: StopReason::Stop,
        model: "test".into(),
        provider: "test".into(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    };
    let parsed: Answer = m.extract_json().unwrap();
    assert_eq!(parsed.score, 1);
    assert_eq!(parsed.label, "split");
}

#[test]
fn extract_json_from_respond_json_tool_call_succeeds() {
    // Anthropic / Bedrock-Anthropic emulation: JSON lives in a `respond_json`
    // tool-call arguments value, not in the text content.
    let m = Message::Assistant {
        content: vec![Content::ToolCall {
            id: "call-1".into(),
            name: "respond_json".into(),
            arguments: serde_json::json!({"score": 7, "label": "via-tool"}),
        }],
        stop_reason: StopReason::ToolUse,
        model: "test".into(),
        provider: "test".into(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    };
    let parsed: Answer = m.extract_json().unwrap();
    assert_eq!(parsed.score, 7);
    assert_eq!(parsed.label, "via-tool");
}

#[test]
fn extract_json_on_invalid_json_returns_schema_mismatch() {
    let m = json_msg("not json at all");
    match m.extract_json::<Answer>() {
        Err(ProviderError::SchemaMismatch { reason }) => {
            assert!(reason.contains("not valid JSON"), "got reason: {}", reason);
        }
        other => panic!("expected SchemaMismatch, got {:?}", other),
    }
}

#[test]
fn extract_json_on_non_assistant_message_returns_schema_mismatch() {
    let m = Message::user("hi");
    match m.extract_json::<Answer>() {
        Err(ProviderError::SchemaMismatch { reason }) => {
            assert!(reason.contains("Assistant"), "got reason: {}", reason);
        }
        other => panic!("expected SchemaMismatch, got {:?}", other),
    }
}

#[test]
fn extract_json_on_empty_assistant_returns_schema_mismatch() {
    let m = Message::Assistant {
        content: vec![],
        stop_reason: StopReason::Stop,
        model: "test".into(),
        provider: "test".into(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    };
    match m.extract_json::<Answer>() {
        Err(ProviderError::SchemaMismatch { .. }) => {}
        other => panic!("expected SchemaMismatch, got {:?}", other),
    }
}

// ── Per-provider body wiring sanity ─────────────────────────────────────────
//
// These tests use private helpers via crate-public surface — we exercise the body
// builders indirectly by constructing a StreamConfig and asking each provider to
// build its body via a public path. Since `build_request_body` is `fn` (not pub),
// we settle for behavioural tests: confirm the bedrock gate fires + extract_json
// round-trips through tool-call emulation. Per-provider native wiring is covered
// by integration tests against live APIs in `tests/integration_anthropic.rs`.

fn make_stream_config(format: ResponseFormat) -> StreamConfig {
    StreamConfig {
        model_config: ModelConfig::anthropic("test", "test", "test"),
        system_prompt: String::new(),
        messages: vec![Message::user("hi")],
        tools: vec![],
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        cache_config: CacheConfig::default(),
        response_format: format,
    }
}

#[test]
fn response_format_default_is_text() {
    let rf = ResponseFormat::default();
    assert!(matches!(rf, ResponseFormat::Text));
}

#[test]
fn stream_config_carries_response_format() {
    let sc = make_stream_config(ResponseFormat::JsonObject);
    assert!(matches!(sc.response_format, ResponseFormat::JsonObject));
}

#[tokio::test]
async fn bedrock_rejects_structured_output_on_non_anthropic_model() {
    use phi_core::provider::{ApiProtocol, BedrockProvider, StreamProvider};
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    // Construct a Bedrock config for a non-Anthropic model (e.g., Meta Llama on Bedrock).
    let mut model_config = ModelConfig::anthropic("meta.llama3-8b-instruct-v1:0", "Llama 3", "k");
    model_config.api = ApiProtocol::BedrockConverseStream;
    model_config.provider = "bedrock".into();

    let config = StreamConfig {
        model_config,
        system_prompt: String::new(),
        messages: vec![Message::user("hi")],
        tools: vec![],
        thinking_level: ThinkingLevel::Off,
        max_tokens: None,
        temperature: None,
        cache_config: CacheConfig::default(),
        response_format: ResponseFormat::JsonObject,
    };

    let provider = BedrockProvider;
    let (tx, _rx) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let result = provider.stream(config, tx, cancel).await;

    match result {
        Err(ProviderError::SchemaMismatch { reason }) => {
            assert!(
                reason.contains("does not support structured output")
                    && reason.contains("meta.llama3-8b-instruct"),
                "reason should name the rejected model; got: {}",
                reason
            );
        }
        other => panic!("expected SchemaMismatch, got {:?}", other),
    }
}

#[test]
fn response_format_json_schema_carries_payload() {
    let rf = ResponseFormat::JsonSchema {
        schema: serde_json::json!({"type": "object"}),
        name: "Foo".into(),
        strict: true,
    };
    match rf {
        ResponseFormat::JsonSchema {
            schema,
            name,
            strict,
        } => {
            assert_eq!(name, "Foo");
            assert!(strict);
            assert!(schema.is_object());
        }
        _ => unreachable!(),
    }
}
