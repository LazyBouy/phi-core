//! Azure OpenAI provider.
//!
//! Uses the OpenAI Responses API format but with Azure-specific authentication
//! and URL patterns.
//!
//! Base URL format: `https://{resource}.openai.azure.com/openai/deployments/{deployment}`
//! Auth: `api-key` header or Azure AD Bearer token.
/*
ARCHITECTURE: AzureOpenAiProvider — Azure's twist on the OpenAI API

Azure hosts OpenAI models but with several differences from api.openai.com:

URL structure:
  OpenAI:   POST https://api.openai.com/v1/responses
  Azure:    POST https://{resource}.openai.azure.com/openai/deployments/{model}/responses?api-version=...

Authentication:
  OpenAI:   `Authorization: Bearer sk-...`
  Azure:    `api-key: {azure_api_key}` header (NOT Authorization Bearer)
            OR `Authorization: Bearer {azure_ad_token}` for Azure AD auth

The `api-version` query parameter is required for Azure and controls which
version of the API specification to use.

Model name:
  In Azure, the model name in the URL is the "deployment name" (user-chosen),
  not the OpenAI model ID. The body still sends `model` for display, but the
  actual dispatch is done by the URL path.

Since the event format is the same as OpenAI Responses API, we share the same
deserialization types (ResponsesEvent, etc. defined in this file or shared).
*/

use super::traits::*;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::EventSource;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Unit struct — no state. All logic in the `StreamProvider` impl.
pub struct AzureOpenAiProvider;

#[async_trait]
impl StreamProvider for AzureOpenAiProvider {
    fn provider_id(&self) -> &str {
        "azure"
    }

    async fn stream(
        &self,
        config: StreamConfig, // REQUEST — api_key sent as `api-key` header (NOT Bearer); base_url includes api-version
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — delegate to OpenAiCompatProvider::stream internally
        cancel: tokio_util::sync::CancellationToken, // ABORT — forwarded to delegate
    ) -> Result<Message, ProviderError> {
        let model_config = &config.model_config;

        /*
        ARCHITECTURE: Azure URL construction

        Azure requires an `api-version` query parameter. We append it here.
        The `model_config.base_url` is the deployment-specific URL:
          "https://{resource}.openai.azure.com/openai/deployments/{deployment}"
        We append "/responses?api-version=..." to get the full endpoint.

        RUST QUIRK: `format!("{}/responses?api-version=...", base_url)`
          String formatting using positional `{}` placeholders.
          `model_config.base_url` is `String`, which Display-formats as its content.
          Python analogy: f"{base_url}/responses?api-version=2025-01-01-preview"
        */
        let url = format!(
            "{}/responses?api-version=2025-01-01-preview",
            model_config.base_url
        );

        let body = build_azure_request_body(&config);
        debug!("Azure OpenAI request: model={} url={}", config.model_config.id, url);

        let client = reqwest::Client::new();
        let mut request = client
            .post(&url)
            .header("content-type", "application/json")
            .header("api-key", &config.model_config.api_key); // Azure uses `api-key` header, NOT `Authorization: Bearer`

        for (k, v) in &model_config.headers {
            request = request.header(k, v);
        }

        let request = request.json(&body);
        let mut es =
            EventSource::new(request).map_err(|e| ProviderError::Network(e.to_string()))?;

        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;
        let mut tool_call_buffers: Vec<ToolCallBuffer> = Vec::new();

        let _ = tx.send(StreamEvent::Start);

        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    es.close();
                    return Err(ProviderError::Cancelled);
                }
                event = es.next() => {
                    match event {
                        None => break,
                        Some(Ok(reqwest_eventsource::Event::Open)) => {}
                        Some(Ok(reqwest_eventsource::Event::Message(msg))) => {
                            match msg.event.as_str() {
                                "response.output_text.delta" => {
                                    if let Ok(data) = serde_json::from_str::<DeltaEvent>(&msg.data) {
                                        let idx = content.iter().position(|c| matches!(c, Content::Text { .. }));
                                        let idx = match idx {
                                            Some(i) => i,
                                            None => {
                                                content.push(Content::Text { text: String::new() });
                                                content.len() - 1
                                            }
                                        };
                                        if let Some(Content::Text { text }) = content.get_mut(idx) {
                                            text.push_str(&data.delta);
                                        }
                                        let _ = tx.send(StreamEvent::TextDelta {
                                            content_index: idx,
                                            delta: data.delta,
                                        });
                                    }
                                }
                                "response.function_call_arguments.start" => {
                                    if let Ok(data) = serde_json::from_str::<FnCallStartEvent>(&msg.data) {
                                        tool_call_buffers.push(ToolCallBuffer {
                                            id: data.call_id.unwrap_or_default(),
                                            name: data.name.unwrap_or_default(),
                                            arguments: String::new(),
                                        });
                                        let buf = tool_call_buffers.last().unwrap();
                                        let _ = tx.send(StreamEvent::ToolCallStart {
                                            content_index: content.len() + tool_call_buffers.len() - 1,
                                            id: buf.id.clone(),
                                            name: buf.name.clone(),
                                        });
                                    }
                                }
                                "response.function_call_arguments.delta" => {
                                    if let Ok(data) = serde_json::from_str::<DeltaEvent>(&msg.data) {
                                        if let Some(buf) = tool_call_buffers.last_mut() {
                                            buf.arguments.push_str(&data.delta);
                                            let _ = tx.send(StreamEvent::ToolCallDelta {
                                                content_index: content.len() + tool_call_buffers.len() - 1,
                                                delta: data.delta,
                                            });
                                        }
                                    }
                                }
                                "response.completed" => {
                                    if let Ok(data) = serde_json::from_str::<CompletedEvent>(&msg.data) {
                                        if let Some(resp) = data.response {
                                            if let Some(u) = resp.usage {
                                                usage.input = u.input_tokens;
                                                usage.output = u.output_tokens;
                                                usage.total_tokens = u.total_tokens;
                                            }
                                        }
                                    }
                                    break;
                                }
                                "error" => {
                                    warn!("Azure OpenAI error: {}", msg.data);
                                    let err_msg = Message::Assistant {
                                        content: vec![Content::Text { text: String::new() }],
                                        stop_reason: StopReason::Error,
                                        model: config.model_config.id.clone(),
                                        provider: model_config.provider.clone(),
                                        usage: usage.clone(),
                                        timestamp: now_ms(),
                                        error_message: Some(msg.data),
                                    };
                                    let _ = tx.send(StreamEvent::Error { message: err_msg.clone() });
                                    return Ok(err_msg);
                                }
                                _ => {}
                            }
                        }
                        Some(Err(e)) => {
                            let err_str = e.to_string();
                            warn!("Azure SSE error: {}", err_str);
                            let err_msg = Message::Assistant {
                                content: vec![Content::Text { text: String::new() }],
                                stop_reason: StopReason::Error,
                                model: config.model_config.id.clone(),
                                provider: model_config.provider.clone(),
                                usage: usage.clone(),
                                timestamp: now_ms(),
                                error_message: Some(err_str),
                            };
                            let _ = tx.send(StreamEvent::Error { message: err_msg.clone() });
                            return Ok(err_msg);
                        }
                    }
                }
            }
        }

        for buf in &tool_call_buffers {
            let args = serde_json::from_str(&buf.arguments)
                .unwrap_or(serde_json::Value::Object(Default::default()));
            content.push(Content::ToolCall {
                id: buf.id.clone(),
                name: buf.name.clone(),
                arguments: args,
            });
            let _ = tx.send(StreamEvent::ToolCallEnd {
                content_index: content.len() - 1,
            });
        }

        if content
            .iter()
            .any(|c| matches!(c, Content::ToolCall { .. }))
        {
            stop_reason = StopReason::ToolUse;
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

struct ToolCallBuffer {
    id: String,
    name: String,
    arguments: String,
}

fn build_azure_request_body(config: &StreamConfig) -> serde_json::Value {
    // Same format as OpenAI Responses API
    let mut input: Vec<serde_json::Value> = Vec::new();

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                // Build content array for user message (supports text + images)
                let user_content: Vec<serde_json::Value> = content
                    .iter()
                    .filter_map(|c| match c {
                        Content::Text { text } => Some(serde_json::json!({
                            "type": "input_text",
                            "text": text,
                        })),
                        Content::Image { data, mime_type } => Some(serde_json::json!({
                            "type": "input_image",
                            "image_url": format!("data:{};base64,{}", mime_type, data),
                        })),
                        _ => None,
                    })
                    .collect();

                if user_content.len() == 1 && user_content[0]["type"] == "input_text" {
                    // Simple text-only message can use shorthand format
                    input.push(serde_json::json!({
                        "role": "user",
                        "content": user_content[0]["text"].as_str().unwrap_or(""),
                    }));
                } else {
                    // Multi-modal content uses array format
                    input.push(serde_json::json!({
                        "role": "user",
                        "content": user_content,
                    }));
                }
            }
            Message::Assistant { content, .. } => {
                for c in content {
                    match c {
                        Content::Text { text } => {
                            input.push(serde_json::json!({
                                "type": "message",
                                "role": "assistant",
                                "content": [{"type": "output_text", "text": text}],
                            }));
                        }
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            input.push(serde_json::json!({
                                "type": "function_call",
                                "call_id": id,
                                "name": name,
                                "arguments": arguments.to_string(),
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Message::ToolResult {
                tool_call_id,
                content,
                ..
            } => {
                let output_val = if content.iter().any(|c| matches!(c, Content::Image { .. })) {
                    let parts: Vec<serde_json::Value> = content
                        .iter()
                        .filter_map(|c| match c {
                            Content::Text { text } => Some(serde_json::json!({
                                "type": "input_text",
                                "text": text,
                            })),
                            Content::Image { data, mime_type } => Some(serde_json::json!({
                                "type": "input_image",
                                "image_url": format!("data:{};base64,{}", mime_type, data),
                            })),
                            _ => None,
                        })
                        .collect();
                    serde_json::json!(parts)
                } else {
                    let text = content
                        .iter()
                        .find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    serde_json::json!(text)
                };
                input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": output_val,
                }));
            }
        }
    }

    let mut body = serde_json::json!({
        "model": config.model_config.id,
        "stream": true,
        "input": input,
    });

    if !config.system_prompt.is_empty() {
        body["instructions"] = serde_json::json!(config.system_prompt);
    }

    if let Some(max) = config.max_tokens {
        body["max_output_tokens"] = serde_json::json!(max);
    }

    if !config.tools.is_empty() {
        let tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "name": t.name,
                    "description": t.description,
                    "parameters": t.parameters,
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }

    if let Some(temp) = config.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    body
}

// Event types
#[derive(Deserialize)]
struct DeltaEvent {
    delta: String,
}

#[derive(Deserialize)]
struct FnCallStartEvent {
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize)]
struct CompletedEvent {
    #[serde(default)]
    response: Option<ResponseData>,
}

#[derive(Deserialize)]
struct ResponseData {
    #[serde(default)]
    usage: Option<AzureUsage>,
}

#[derive(Deserialize)]
struct AzureUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
}
