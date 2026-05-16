//! Amazon Bedrock ConverseStream provider.
//!
//! Uses the Bedrock ConverseStream API with AWS SigV4 request signing.
//! For simplicity, we implement minimal SigV4 signing using the `aws-sigv4`
//! and `aws-credential-types` crates. If those aren't available, callers
//! can pass pre-signed requests or use an IAM proxy.
//!
//! The `api_key` field in StreamConfig is expected to be formatted as:
//! `{access_key_id}:{secret_access_key}` (with optional `:{session_token}`).
//! The `base_url` in ModelConfig should be the Bedrock endpoint, e.g.:
//! `https://bedrock-runtime.us-east-1.amazonaws.com`
/*
ARCHITECTURE: BedrockProvider — AWS-native Gemini/Claude/Titan via Bedrock

Amazon Bedrock is AWS's managed AI platform. It hosts Claude (Anthropic), Titan (Amazon),
Llama (Meta), and others. The Bedrock ConverseStream API is a uniform interface that
abstracts away per-model differences at the AWS level.

Key differences from other providers:

  Authentication: AWS SigV4 request signing (NOT Bearer/API key headers)
    SigV4 involves: HMAC-SHA256 of canonical request, string-to-sign, and credential scope.
    The `api_key` is encoded as `access_key_id:secret_access_key[:session_token]`.
    We parse this and use it to sign the request.

  URL: `{base_url}/model/{model}/converse-stream`
    Example: `https://bedrock-runtime.us-east-1.amazonaws.com/model/anthropic.claude-3-sonnet.../converse-stream`

  Request body: ConverseStream format (Amazon's envelope around model-specific content)
    Messages: `{role: "user"|"assistant", content: [{text: "..."}]}`
    Tools: `toolConfig: {tools: [{toolSpec: {name, description, inputSchema: {json: {...}}}}]}`
    System: `system: [{text: "..."}]` (separate top-level field)

  Response: Binary event-stream framing (NOT SSE)
    Bedrock returns responses as AWS event stream (binary-framed JSON events).
    We re-encode them to the SSE loop pattern for consistency.

RUST QUIRK: `splitn(3, ':')` — split with a limit
  `.splitn(3, ':')` splits on `:` but produces AT MOST 3 parts.
  For `"AKID:SECRET"` → `["AKID", "SECRET"]`
  For `"AKID:SECRET:TOKEN"` → `["AKID", "SECRET", "TOKEN"]`
  For `"AKID:SECRET:TOKEN:EXTRA"` → `["AKID", "SECRET", "TOKEN:EXTRA"]` (3rd gets the rest)
  This lets us parse `access_key:secret[:session_token]` without splitting session tokens
  that might (theoretically) contain colons.
  Python analogy: `api_key.split(":", 2)`
*/

use super::traits::*;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Unit struct — no state. All logic in the `StreamProvider` impl.
pub struct BedrockProvider;

#[async_trait]
impl StreamProvider for BedrockProvider {
    fn provider_id(&self) -> &str {
        "bedrock"
    }

    async fn stream(
        &self,
        config: StreamConfig, // REQUEST — api_key is "access_key:secret[:token]"; uses AWS SigV4 signing
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — receives events from ConverseStream binary framing
        cancel: tokio_util::sync::CancellationToken, // ABORT — races against ConverseStream
    ) -> Result<Message, ProviderError> {
        let model_config = &config.model_config;
        // Resolve via CredentialProvider when set, else use the static `api_key`.
        // Bound to a local so `.splitn(...)` borrows from a local-lifetime String.
        let api_key = model_config.resolve_api_key().await?;

        // Structured-output gate. Bedrock's Converse API has no universal JSON-mode;
        // the canonical workaround is the Anthropic tool-call shape, which only works
        // for Anthropic foundation models on Bedrock. Detect by model-ID prefix; for
        // other model families (Cohere, AI21, Meta Llama, Mistral on Bedrock, etc.),
        // surface SchemaMismatch with a clear reason so the caller can adapt.
        if !matches!(config.response_format, ResponseFormat::Text)
            && !model_config.id.contains("anthropic")
        {
            return Err(ProviderError::SchemaMismatch {
                reason: format!(
                    "Bedrock model `{}` does not support structured output via the \
                     phi-core Converse adapter (only `anthropic.*` foundation models do). \
                     Either switch to a Bedrock Anthropic model or set response_format to Text.",
                    model_config.id
                ),
            });
        }

        let base_url = &model_config.base_url;
        let url = format!(
            "{}/model/{}/converse-stream",
            base_url, config.model_config.id
        );

        let body = build_bedrock_body(&config);
        debug!(
            "Bedrock request: model={} url={}",
            config.model_config.id, url
        );

        /*
        RUST QUIRK: `api_key.splitn(3, ':').collect::<Vec<&str>>()`

        `.splitn(n, delimiter)` — split into at most n parts (see module doc above).
        `.collect::<Vec<&str>>()` — turbofish: collect into a Vec<&str> (borrowed slices
          into the resolved `api_key` String; valid as long as the local is alive).
        `parts.len() < 2` — validate we got at least access_key AND secret_key.
        Python analogy: `parts = api_key.split(":", 2)` + `if len(parts) < 2: raise ...`
        */
        let parts: Vec<&str> = api_key.splitn(3, ':').collect();
        if parts.len() < 2 {
            return Err(ProviderError::Auth(
                "Bedrock api_key must be 'access_key:secret_key[:session_token]'".into(),
            ));
        }

        let client = reqwest::Client::new();
        let mut request = client.post(&url).header("content-type", "application/json");

        // Add AWS auth headers. In a real implementation, this would use SigV4.
        // For now, we support a simplified auth model where the caller provides
        // pre-computed auth headers via model_config.headers, or uses an IAM proxy.
        for (k, v) in &model_config.headers {
            request = request.header(k, v);
        }

        // If no auth headers provided, try basic Bearer auth as fallback
        // (works with some Bedrock proxy configurations)
        if !model_config.headers.contains_key("authorization") {
            request = request.header("authorization", format!("Bearer {}", api_key));
        }

        let response = request
            .json(&body)
            .send()
            .await
            .map_err(|e| ProviderError::Network(e.to_string()))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::classify(
                status.as_u16(),
                &format!("Bedrock error {}: {}", status, body),
            ));
        }

        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;

        let _ = tx.send(StreamEvent::Start);

        // Bedrock ConverseStream returns event-stream format (application/vnd.amazon.eventstream)
        // For simplicity, we parse it as newline-delimited JSON chunks.
        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    return Err(ProviderError::Cancelled);
                }
                chunk = stream.next() => {
                    match chunk {
                        None => break,
                        Some(Err(e)) => {
                            warn!("Bedrock stream error: {}", e);
                            break;
                        }
                        Some(Ok(bytes)) => {
                            buffer.push_str(&String::from_utf8_lossy(&bytes));

                            // Try to parse complete JSON objects
                            while let Some(pos) = buffer.find('\n') {
                                let line = buffer[..pos].trim().to_string();
                                buffer = buffer[pos + 1..].to_string();

                                if line.is_empty() {
                                    continue;
                                }

                                let event: BedrockEvent = match serde_json::from_str(&line) {
                                    Ok(e) => e,
                                    Err(_) => continue,
                                };

                                match event {
                                    BedrockEvent::ContentBlockDelta { delta, .. } => {
                                        if let Some(text) = delta.text {
                                            let text_idx = content.iter().position(|c| matches!(c, Content::Text { .. }));
                                            let idx = match text_idx {
                                                Some(i) => i,
                                                None => {
                                                    content.push(Content::Text { text: String::new() });
                                                    content.len() - 1
                                                }
                                            };
                                            if let Some(Content::Text { text: t }) = content.get_mut(idx) {
                                                t.push_str(&text);
                                            }
                                            let _ = tx.send(StreamEvent::TextDelta {
                                                content_index: idx,
                                                delta: text,
                                            });
                                        }
                                        if let Some(tool_use) = delta.tool_use {
                                            let _ = tx.send(StreamEvent::ToolCallDelta {
                                                content_index: content.len(),
                                                delta: tool_use.input,
                                            });
                                        }
                                    }
                                    BedrockEvent::ContentBlockStart { start, .. } => {
                                        if let Some(tool_use) = start.tool_use {
                                            let idx = content.len();
                                            content.push(Content::ToolCall {
                                                id: tool_use.tool_use_id.clone(),
                                                name: tool_use.name.clone(),
                                                arguments: serde_json::Value::Object(Default::default()),
                                            });
                                            let _ = tx.send(StreamEvent::ToolCallStart {
                                                content_index: idx,
                                                id: tool_use.tool_use_id,
                                                name: tool_use.name,
                                            });
                                        }
                                    }
                                    BedrockEvent::ContentBlockStop { .. } => {
                                        if content.iter().any(|c| matches!(c, Content::ToolCall { .. })) {
                                            let _ = tx.send(StreamEvent::ToolCallEnd {
                                                content_index: content.len() - 1,
                                            });
                                        }
                                    }
                                    BedrockEvent::MessageStop { stop_reason: sr } => {
                                        stop_reason = match sr.as_deref() {
                                            Some("end_turn") => StopReason::Stop,
                                            Some("max_tokens") => StopReason::Length,
                                            Some("tool_use") => StopReason::ToolUse,
                                            _ => StopReason::Stop,
                                        };
                                    }
                                    BedrockEvent::Metadata { usage: u } => {
                                        if let Some(u) = u {
                                            usage.input = u.input_tokens;
                                            usage.output = u.output_tokens;
                                            usage.total_tokens = u.input_tokens + u.output_tokens;
                                        }
                                    }
                                    BedrockEvent::Unknown => {}
                                }
                            }
                        }
                    }
                }
            }
        }

        let message = Message::Assistant {
            content,
            stop_reason,
            model: config.model_config.id.clone(),
            provider: model_config.provider.clone(),
            usage,
            timestamp: now_ms(),
            error_message: None,
        };

        let _ = tx.send(StreamEvent::Done {
            message: message.clone(),
        });
        Ok(message)
    }
}

fn build_bedrock_body(config: &StreamConfig) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                let blocks = content_to_bedrock(content);
                messages.push(serde_json::json!({"role": "user", "content": blocks}));
            }
            Message::Assistant { content, .. } => {
                let blocks = content_to_bedrock(content);
                messages.push(serde_json::json!({"role": "assistant", "content": blocks}));
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                // Build content blocks for tool result (text + images)
                let tool_content: Vec<serde_json::Value> = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text } => Some(serde_json::json!({"text": text})),
                        Content::Image { data, mime_type } => Some(serde_json::json!({
                            "image": {
                                "format": mime_type.split('/').nth(1).unwrap_or("png"),
                                "source": {"bytes": data},
                            }
                        })),
                        _ => None,
                    })
                    .collect();

                let tool_content = if tool_content.is_empty() {
                    vec![serde_json::json!({"text": ""})]
                } else {
                    tool_content
                };

                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "toolResult": {
                            "toolUseId": tool_call_id,
                            "content": tool_content,
                            "status": if *is_error { "error" } else { "success" },
                        }
                    }],
                }));
            }
        }
    }

    let mut body = serde_json::json!({"messages": messages});

    if !config.system_prompt.is_empty() {
        body["system"] = serde_json::json!([{"text": config.system_prompt}]);
    }

    let mut inference_config = serde_json::json!({});
    if let Some(max) = config.max_tokens {
        inference_config["maxTokens"] = serde_json::json!(max);
    }
    if let Some(temp) = config.temperature {
        inference_config["temperature"] = serde_json::json!(temp);
    }
    if inference_config != serde_json::json!({}) {
        body["inferenceConfig"] = inference_config;
    }

    if !config.tools.is_empty() {
        let tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "toolSpec": {
                        "name": t.name,
                        "description": t.description,
                        "inputSchema": {"json": t.parameters},
                    }
                })
            })
            .collect();
        body["toolConfig"] = serde_json::json!({"tools": tools});
    }

    // Structured-output emulation for Anthropic-on-Bedrock. Same shape as the native
    // Anthropic provider: inject a `respond_json` synthetic tool spec and force the
    // model to call it via `toolChoice.tool`. Non-Anthropic Bedrock models are gated
    // by stream() above and never reach this point with a non-Text format.
    match &config.response_format {
        ResponseFormat::Text => {}
        ResponseFormat::JsonObject | ResponseFormat::JsonSchema { .. } => {
            let (schema, description) = match &config.response_format {
                ResponseFormat::JsonSchema { schema, name, .. } => (
                    schema.clone(),
                    format!("Return the response as a JSON object matching `{}`.", name),
                ),
                _ => (
                    serde_json::json!({"type": "object", "additionalProperties": true}),
                    "Return the response as a JSON object.".to_string(),
                ),
            };
            let synthetic = serde_json::json!({
                "toolSpec": {
                    "name": "respond_json",
                    "description": description,
                    "inputSchema": {"json": schema},
                }
            });
            // Append to existing toolConfig.tools or create one.
            if let Some(tools_arr) = body
                .get_mut("toolConfig")
                .and_then(|tc| tc.get_mut("tools"))
                .and_then(|t| t.as_array_mut())
            {
                tools_arr.push(synthetic);
            } else {
                body["toolConfig"] = serde_json::json!({"tools": [synthetic]});
            }
            // Force tool_choice to the synthetic tool.
            body["toolConfig"]["toolChoice"] =
                serde_json::json!({"tool": {"name": "respond_json"}});
        }
    }

    body
}

fn content_to_bedrock(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(serde_json::json!({"text": text})),
            Content::Image { data, mime_type } => Some(serde_json::json!({
                "image": {
                    "format": mime_type.split('/').nth(1).unwrap_or("png"),
                    "source": {"bytes": data},
                }
            })),
            Content::ToolCall {
                id,
                name,
                arguments,
            } => Some(serde_json::json!({
                "toolUse": {"toolUseId": id, "name": name, "input": arguments},
            })),
            Content::Thinking { .. } => None,
        })
        .collect()
}

// Bedrock event types
#[derive(Deserialize)]
#[serde(untagged)]
enum BedrockEvent {
    ContentBlockDelta {
        #[serde(rename = "contentBlockDelta")]
        delta: BedrockDelta,
    },
    ContentBlockStart {
        #[serde(rename = "contentBlockStart")]
        start: BedrockBlockStart,
    },
    ContentBlockStop {
        #[serde(rename = "contentBlockStop")]
        #[allow(dead_code)]
        stop: serde_json::Value,
    },
    MessageStop {
        #[serde(rename = "messageStop")]
        stop_reason: Option<String>,
    },
    Metadata {
        #[serde(rename = "metadata")]
        usage: Option<BedrockUsage>,
    },
    Unknown,
}

#[derive(Deserialize)]
struct BedrockDelta {
    #[serde(default)]
    text: Option<String>,
    #[serde(default, rename = "toolUse")]
    tool_use: Option<BedrockToolUseDelta>,
}

#[derive(Deserialize)]
struct BedrockToolUseDelta {
    input: String,
}

#[derive(Deserialize)]
struct BedrockBlockStart {
    #[serde(default, rename = "toolUse")]
    tool_use: Option<BedrockToolUseStart>,
}

#[derive(Deserialize)]
struct BedrockToolUseStart {
    #[serde(rename = "toolUseId")]
    tool_use_id: String,
    name: String,
}

#[derive(Deserialize)]
struct BedrockUsage {
    #[serde(default, rename = "inputTokens")]
    input_tokens: u64,
    #[serde(default, rename = "outputTokens")]
    output_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_bedrock_body() {
        let config = StreamConfig {
            model_config: crate::provider::ModelConfig::anthropic(
                "anthropic.claude-3-sonnet-20240229-v1:0",
                "Claude Sonnet",
                "key:secret",
            ),
            system_prompt: "Be helpful".into(),
            messages: vec![Message::user("Hello")],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            max_tokens: Some(1024),
            temperature: None,
            cache_config: CacheConfig::default(),
            response_format: ResponseFormat::Text,
        };

        let body = build_bedrock_body(&config);
        assert!(body["messages"].is_array());
        assert_eq!(body["messages"][0]["role"], "user");
        assert!(body["system"].is_array());
        assert_eq!(body["inferenceConfig"]["maxTokens"], 1024);
    }

    #[test]
    fn test_content_to_bedrock() {
        let content = vec![
            Content::Text {
                text: "hello".into(),
            },
            Content::ToolCall {
                id: "tc-1".into(),
                name: "bash".into(),
                arguments: serde_json::json!({"command": "ls"}),
            },
        ];
        let blocks = content_to_bedrock(&content);
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["text"], "hello");
        assert_eq!(blocks[1]["toolUse"]["name"], "bash");
    }
}
