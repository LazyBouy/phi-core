//! Block provenance + annotated request payload for per-turn debug capture.
//!
//! Shipped in phi-core 0.9.0 to close the per-turn debug-capture gap. The
//! agent loop assembles each LLM request from many sources — the system
//! prompt, identity layers, memory tiers, prior loop history, current turn
//! input — and historically that fully-assembled payload was never persisted.
//!
//! The types in this module make the assembled payload + per-block provenance
//! visible to consumers:
//!
//! - [`BlockProvenance`] tags each message in the request with its origin
//!   (system prompt, identity block, memory tier, loop turn, etc.).
//! - [`AnnotatedRequestPayload`] bundles the full request payload (system
//!   prompt + messages + tools + model config + provenance) into a single
//!   serializable record. The `messages` and `provenance` vectors are
//!   parallel-indexed.
//! - [`ProvenanceRole`] disambiguates the role of a loop-turn message
//!   (user input / assistant response / tool-call request / tool-call result).
//!
//! Emission: [`crate::AgentEvent::TurnRequest`] fires exactly once per turn
//! carrying an [`AnnotatedRequestPayload`]. Opt-in persistence flows through
//! [`crate::session::SessionRecorderConfig::capture_turn_requests`].

use super::usage::ThinkingLevel;
use crate::provider::{ResponseFormat, ToolDefinition};
use crate::types::content::Message;
use serde::{Deserialize, Serialize};

/// Origin of a single message block in an assembled LLM request.
///
/// Consumers stamp the corresponding hint on [`crate::LlmMessage::provenance_hint`]
/// before emitting the message into the agent loop; phi-core reads the stamp
/// during request assembly and falls back to deriving the tag from `turn_id`
/// when no stamp is set.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
#[non_exhaustive]
pub enum BlockProvenance {
    /// The system prompt block. Always the first entry in an assembled request
    /// when a system prompt is configured.
    SystemPrompt,
    /// An identity-layer block (e.g., agent / user / project layers). `name` is
    /// the layer's stem (e.g. `"iphi"`, `"Agent"`); `order` is the layer's
    /// priority used to sort identity blocks when assembling the request.
    IdentityBlock { name: String, order: u32 },
    /// A memory-tier block (short-term / long-term / episodic). `tier` is the
    /// tier identifier; `record_id` identifies the specific record within the
    /// tier (file path, JSONL row id, etc.).
    MemoryTier { tier: String, record_id: String },
    /// A loop-turn message — user input, assistant response, tool-call request,
    /// or tool-call result. `turn_index` is 0-based; `message_index` orders
    /// messages within that turn.
    LoopTurn {
        turn_index: usize,
        role: ProvenanceRole,
        message_index: usize,
    },
    /// A steering message injected mid-loop by the caller. No turn binding.
    Steering,
    /// A follow-up message queued after the loop's natural end.
    FollowUp,
    /// Fallback when no provenance can be inferred. Consumers SHOULD stamp
    /// `provenance_hint` to avoid this fallback for non-loop-history blocks.
    Unknown,
}

/// Sub-role within a [`BlockProvenance::LoopTurn`] block.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ProvenanceRole {
    /// A user message injected at the start of the turn.
    UserMessage,
    /// The assistant's response for the turn.
    AssistantResponse,
    /// A tool-call request emitted by the assistant (content of `Message::Assistant`
    /// containing `Content::ToolCall`).
    ToolCallRequest,
    /// A tool-call result fed back as `Message::ToolResult`.
    ToolCallResult,
}

/// Fully-annotated snapshot of the request payload sent to the LLM provider.
///
/// This is the exact, post-`convert_to_llm()` payload — the wire-format
/// shape of `StreamConfig.messages` plus the system prompt and tool
/// definitions — paired with parallel-indexed provenance tags so consumers
/// can reconstruct *exactly* what the model saw, and *why* each block was
/// included.
///
/// Carried by [`crate::AgentEvent::TurnRequest`] (one per turn, before the
/// retry loop's first provider call) and optionally persisted on
/// [`crate::session::Turn::request_payload`] when
/// [`crate::session::SessionRecorderConfig::capture_turn_requests`] is true.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnotatedRequestPayload {
    /// System prompt string sent to the provider.
    pub system_prompt: String,
    /// LLM-wire `Message` vector — the exact payload after
    /// `convert_to_llm()`. Parallel-indexed to [`Self::provenance`].
    pub messages: Vec<Message>,
    /// Per-message provenance tags. `provenance[i]` describes `messages[i]`.
    /// Always the same length as [`Self::messages`].
    pub provenance: Vec<BlockProvenance>,
    /// Tool definitions sent to the provider (schema only, no execute fns).
    pub tools: Vec<ToolDefinition>,
    /// Model identifier (mirrors `model_config.id`).
    pub model_id: String,
    /// Thinking level configured for the turn.
    pub thinking_level: ThinkingLevel,
    /// Max-tokens override (when set).
    pub max_tokens: Option<u32>,
    /// Temperature override (when set).
    pub temperature: Option<f32>,
    /// Desired output shape for the turn.
    ///
    /// Forwarded through a serde proxy because `ResponseFormat` does not derive
    /// `Serialize`/`Deserialize` natively.
    #[serde(with = "response_format_serde")]
    pub response_format: ResponseFormat,
}

mod response_format_serde {
    //! Serde proxy for `ResponseFormat`, which does not derive Serialize /
    //! Deserialize natively. Mirrors the variant shape on the wire as a
    //! tagged enum: `{ "kind": "text" }`, `{ "kind": "json-object" }`,
    //! `{ "kind": "json-schema", "schema": ..., "name": ..., "strict": ... }`.
    use crate::provider::ResponseFormat;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "kind", rename_all = "kebab-case")]
    enum Proxy {
        Text,
        JsonObject,
        JsonSchema {
            schema: serde_json::Value,
            name: String,
            strict: bool,
        },
    }

    impl From<&ResponseFormat> for Proxy {
        fn from(value: &ResponseFormat) -> Self {
            match value {
                ResponseFormat::Text => Proxy::Text,
                ResponseFormat::JsonObject => Proxy::JsonObject,
                ResponseFormat::JsonSchema {
                    schema,
                    name,
                    strict,
                } => Proxy::JsonSchema {
                    schema: schema.clone(),
                    name: name.clone(),
                    strict: *strict,
                },
            }
        }
    }

    impl From<Proxy> for ResponseFormat {
        fn from(value: Proxy) -> Self {
            match value {
                Proxy::Text => ResponseFormat::Text,
                Proxy::JsonObject => ResponseFormat::JsonObject,
                Proxy::JsonSchema {
                    schema,
                    name,
                    strict,
                } => ResponseFormat::JsonSchema {
                    schema,
                    name,
                    strict,
                },
            }
        }
    }

    pub fn serialize<S: Serializer>(value: &ResponseFormat, ser: S) -> Result<S::Ok, S::Error> {
        Proxy::from(value).serialize(ser)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(de: D) -> Result<ResponseFormat, D::Error> {
        Proxy::deserialize(de).map(ResponseFormat::from)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_provenance_serializes_kebab_case() {
        let p = BlockProvenance::SystemPrompt;
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"system-prompt\""));
    }

    #[test]
    fn block_provenance_identity_block_round_trip() {
        let p = BlockProvenance::IdentityBlock {
            name: "Agent".to_string(),
            order: 3,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: BlockProvenance = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn block_provenance_loop_turn_round_trip() {
        let p = BlockProvenance::LoopTurn {
            turn_index: 2,
            role: ProvenanceRole::AssistantResponse,
            message_index: 1,
        };
        let s = serde_json::to_string(&p).unwrap();
        let back: BlockProvenance = serde_json::from_str(&s).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn provenance_role_serializes_kebab_case() {
        let r = ProvenanceRole::ToolCallResult;
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(s, "\"tool-call-result\"");
    }

    #[test]
    fn annotated_request_payload_round_trip_with_text_response_format() {
        let payload = AnnotatedRequestPayload {
            system_prompt: "system".to_string(),
            messages: vec![],
            provenance: vec![],
            tools: vec![],
            model_id: "test-model".to_string(),
            thinking_level: ThinkingLevel::Off,
            max_tokens: Some(1024),
            temperature: Some(0.7),
            response_format: ResponseFormat::Text,
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: AnnotatedRequestPayload = serde_json::from_str(&s).unwrap();
        assert_eq!(back.system_prompt, "system");
        assert!(matches!(back.response_format, ResponseFormat::Text));
        assert_eq!(back.max_tokens, Some(1024));
    }

    #[test]
    fn annotated_request_payload_round_trip_with_json_schema_response_format() {
        let payload = AnnotatedRequestPayload {
            system_prompt: String::new(),
            messages: vec![],
            provenance: vec![],
            tools: vec![],
            model_id: "m".to_string(),
            thinking_level: ThinkingLevel::Medium,
            max_tokens: None,
            temperature: None,
            response_format: ResponseFormat::JsonSchema {
                schema: serde_json::json!({"type": "object"}),
                name: "Out".to_string(),
                strict: true,
            },
        };
        let s = serde_json::to_string(&payload).unwrap();
        let back: AnnotatedRequestPayload = serde_json::from_str(&s).unwrap();
        match back.response_format {
            ResponseFormat::JsonSchema {
                ref name, strict, ..
            } => {
                assert_eq!(name, "Out");
                assert!(strict);
            }
            _ => panic!("expected JsonSchema variant"),
        }
    }
}
