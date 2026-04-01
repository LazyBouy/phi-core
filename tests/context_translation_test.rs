//! Tests for G8: ContextTranslation — cross-provider message compatibility.

use phi_core::provider::context_translation::{
    ContextTranslationStrategy, DefaultContextTranslation,
};
use phi_core::provider::model::ApiProtocol;
use phi_core::types::content::{Content, Message, StopReason};
use phi_core::types::usage::Usage;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn make_assistant_with_thinking() -> Message {
    Message::Assistant {
        content: vec![
            Content::Thinking {
                thinking: "Let me think...".to_string(),
                signature: None,
            },
            Content::Text {
                text: "Here is my answer.".to_string(),
            },
        ],
        stop_reason: StopReason::Stop,
        model: "test".to_string(),
        provider: "test".to_string(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    }
}

fn make_assistant_with_thinking_and_signature() -> Message {
    Message::Assistant {
        content: vec![
            Content::Thinking {
                thinking: "Deep reasoning here".to_string(),
                signature: Some("sig-abc-123".to_string()),
            },
            Content::Text {
                text: "Final answer.".to_string(),
            },
        ],
        stop_reason: StopReason::Stop,
        model: "test".to_string(),
        provider: "test".to_string(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    }
}

fn make_assistant_with_text_and_toolcall() -> Message {
    Message::Assistant {
        content: vec![
            Content::Text {
                text: "I will read that file.".to_string(),
            },
            Content::ToolCall {
                id: "call_1".to_string(),
                name: "read_file".to_string(),
                arguments: serde_json::json!({"path": "src/main.rs"}),
            },
        ],
        stop_reason: StopReason::ToolUse,
        model: "test".to_string(),
        provider: "test".to_string(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    }
}

fn make_assistant_with_image() -> Message {
    Message::Assistant {
        content: vec![Content::Image {
            data: "iVBORw0KGgo=".to_string(),
            mime_type: "image/png".to_string(),
        }],
        stop_reason: StopReason::Stop,
        model: "test".to_string(),
        provider: "test".to_string(),
        usage: Usage::default(),
        timestamp: 0,
        error_message: None,
    }
}

// ---------------------------------------------------------------------------
// 5. test_default_translation_noop_same_provider
// ---------------------------------------------------------------------------

#[test]
fn test_default_translation_noop_same_provider() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_thinking()];
    let result = strategy.translate_for_provider(&msgs, ApiProtocol::AnthropicMessages);

    assert_eq!(result.len(), 1);
    if let Message::Assistant { content, .. } = &result[0] {
        assert_eq!(
            content.len(),
            2,
            "Anthropic target should keep all content blocks"
        );
        assert!(
            matches!(&content[0], Content::Thinking { .. }),
            "first block should still be Thinking"
        );
        assert!(
            matches!(&content[1], Content::Text { .. }),
            "second block should still be Text"
        );
    } else {
        panic!("Expected assistant message");
    }
}

// ---------------------------------------------------------------------------
// 6. test_translation_thinking_to_text_for_openai
// ---------------------------------------------------------------------------

#[test]
fn test_translation_thinking_to_text_for_openai() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_thinking()];
    let result = strategy.translate_for_provider(&msgs, ApiProtocol::OpenAiCompletions);

    assert_eq!(result.len(), 1);
    if let Message::Assistant { content, .. } = &result[0] {
        assert_eq!(content.len(), 2, "should have 2 content blocks");
        match &content[0] {
            Content::Text { text } => {
                assert!(
                    text.starts_with("[Reasoning]"),
                    "thinking should be converted to text with [Reasoning] prefix, got: {text}"
                );
                assert!(
                    text.contains("Let me think..."),
                    "should contain original thinking text"
                );
            }
            other => panic!("Expected Text, got {:?}", other),
        }
    } else {
        panic!("Expected assistant message");
    }
}

// ---------------------------------------------------------------------------
// 7. test_translation_drops_signature_for_openai
// ---------------------------------------------------------------------------

#[test]
fn test_translation_drops_signature_for_openai() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_thinking_and_signature()];
    let result = strategy.translate_for_provider(&msgs, ApiProtocol::OpenAiCompletions);

    if let Message::Assistant { content, .. } = &result[0] {
        // The thinking block is converted to Text — no signature field exists on Content::Text
        match &content[0] {
            Content::Text { text } => {
                assert!(
                    !text.contains("sig-abc-123"),
                    "signature should not appear in translated text"
                );
            }
            Content::Thinking { signature, .. } => {
                panic!("Thinking should have been converted to Text, but got Thinking with signature={signature:?}");
            }
            other => panic!("Expected Text, got {:?}", other),
        }
    } else {
        panic!("Expected assistant message");
    }
}

// ---------------------------------------------------------------------------
// 8. test_translation_drops_thinking_for_google
// ---------------------------------------------------------------------------

#[test]
fn test_translation_drops_thinking_for_google() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_thinking()];
    let result = strategy.translate_for_provider(&msgs, ApiProtocol::GoogleGenerativeAi);

    if let Message::Assistant { content, .. } = &result[0] {
        assert_eq!(
            content.len(),
            1,
            "Google should drop thinking, leaving only Text"
        );
        assert!(
            matches!(&content[0], Content::Text { .. }),
            "remaining block should be Text"
        );
    } else {
        panic!("Expected assistant message");
    }
}

// ---------------------------------------------------------------------------
// 9. test_translation_preserves_text_and_toolcall
// ---------------------------------------------------------------------------

#[test]
fn test_translation_preserves_text_and_toolcall() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_text_and_toolcall()];

    // Test for multiple providers
    for target in [
        ApiProtocol::AnthropicMessages,
        ApiProtocol::OpenAiCompletions,
        ApiProtocol::GoogleGenerativeAi,
    ] {
        let result = strategy.translate_for_provider(&msgs, target);
        if let Message::Assistant { content, .. } = &result[0] {
            assert_eq!(
                content.len(),
                2,
                "Text + ToolCall should both be preserved for {target:?}"
            );
            assert!(
                matches!(&content[0], Content::Text { .. }),
                "first block should be Text for {target:?}"
            );
            assert!(
                matches!(&content[1], Content::ToolCall { .. }),
                "second block should be ToolCall for {target:?}"
            );
        } else {
            panic!("Expected assistant message");
        }
    }
}

// ---------------------------------------------------------------------------
// 10. test_translation_preserves_images
// ---------------------------------------------------------------------------

#[test]
fn test_translation_preserves_images() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![make_assistant_with_image()];
    let result = strategy.translate_for_provider(&msgs, ApiProtocol::OpenAiCompletions);

    if let Message::Assistant { content, .. } = &result[0] {
        assert_eq!(content.len(), 1, "image should be preserved");
        match &content[0] {
            Content::Image { data, mime_type } => {
                assert_eq!(data, "iVBORw0KGgo=");
                assert_eq!(mime_type, "image/png");
            }
            other => panic!("Expected Image, got {:?}", other),
        }
    } else {
        panic!("Expected assistant message");
    }
}

// ---------------------------------------------------------------------------
// 11. test_lossless_roundtrip
// ---------------------------------------------------------------------------

#[test]
fn test_lossless_roundtrip() {
    let strategy = DefaultContextTranslation;
    let original = vec![make_assistant_with_thinking()];
    let original_clone = original.clone();

    // Translate for OpenAI (which converts Thinking → Text)
    let _translated = strategy.translate_for_provider(&original, ApiProtocol::OpenAiCompletions);

    // Original messages should be unchanged (translate is read-only)
    assert_eq!(
        original, original_clone,
        "original messages should be unchanged after translation"
    );

    // Verify the original still has Thinking content
    if let Message::Assistant { content, .. } = &original[0] {
        assert!(
            matches!(&content[0], Content::Thinking { .. }),
            "original should still have Thinking block"
        );
    }
}

// ---------------------------------------------------------------------------
// 12. test_translation_user_messages_pass_through
// ---------------------------------------------------------------------------

#[test]
fn test_translation_user_messages_pass_through() {
    let strategy = DefaultContextTranslation;
    let msgs = vec![Message::user("Hello, world!")];

    for target in [
        ApiProtocol::AnthropicMessages,
        ApiProtocol::OpenAiCompletions,
        ApiProtocol::GoogleGenerativeAi,
        ApiProtocol::BedrockConverseStream,
    ] {
        let result = strategy.translate_for_provider(&msgs, target);
        assert_eq!(
            result.len(),
            1,
            "user message should pass through for {target:?}"
        );
        assert!(
            matches!(&result[0], Message::User { .. }),
            "should still be a User message for {target:?}"
        );
    }
}
