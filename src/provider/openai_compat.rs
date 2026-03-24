//! OpenAI Chat Completions compatible provider.
//!
//! One implementation covers OpenAI, xAI, Groq, Cerebras, OpenRouter,
//! Mistral, DeepSeek, MiniMax, HuggingFace, Kimi, and any other provider
//! that implements the OpenAI Chat Completions API.
//!
//! Behavioral differences are handled via `OpenAiCompat` flags in ModelConfig.
/*
ARCHITECTURE: OpenAiCompatProvider — one implementation, 15+ providers

The OpenAI Chat Completions API is a de facto industry standard. Rather than
writing separate providers for OpenAI, Groq, DeepSeek, etc., we write ONE provider
that reads `OpenAiCompat` flags from `ModelConfig` to handle per-provider quirks:

  - `supports_developer_role`           → use "developer" instead of "system" role
  - `max_tokens_field`                  → use "max_completion_tokens" vs "max_tokens"
  - `supports_reasoning_effort`         → send `reasoning_effort: "high/medium/low"`
  - `requires_tool_result_name`         → include `name` field in tool results
  - `thinking_format`                   → parse reasoning from `reasoning` or `reasoning_content`

Adding a new OpenAI-compatible provider requires ONLY:
  1. A new `OpenAiCompat::new_provider()` factory in model.rs
  2. A `ModelConfig::new_provider(id, name)` factory in model.rs
  No new provider files needed.

ARCHITECTURE: Tool call buffering vs Anthropic's content_block approach

OpenAI and Anthropic stream tool calls differently:
  Anthropic: explicit start/delta/stop events with `content_index`
  OpenAI:    tool calls appear as `delta.tool_calls[N]` fragments across chunks;
             each chunk may have multiple tool call deltas; `index` identifies which

We buffer OpenAI tool calls in `ToolCallBuffer` (scratch pads), then push them
into `content` as complete `Content::ToolCall` values at stream end.
This two-phase approach avoids partial JSON being visible to the agent loop.

ARCHITECTURE: `[DONE]` sentinel

OpenAI streams end with a special SSE data payload `[DONE]` (not valid JSON).
The provider explicitly checks for this and breaks the loop rather than
trying to parse it as JSON.
*/

use super::model::{MaxTokensField, ModelConfig, OpenAiCompat, ThinkingFormat};
use super::traits::*;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::EventSource;
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Unit struct — no state. All logic in the `StreamProvider` impl.
pub struct OpenAiCompatProvider;

#[async_trait]
impl StreamProvider for OpenAiCompatProvider {
    fn provider_id(&self) -> &str {
        "openai"
    }

    async fn stream(
        &self,
        config: StreamConfig, // REQUEST — model_config.base_url determines which of 15+ providers to hit
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — receives SSE events ([DONE] terminates the stream)
        cancel: tokio_util::sync::CancellationToken, // ABORT — races against SSE stream
    ) -> Result<Message, ProviderError> {
        let model_config = &config.model_config;
        /*
        RUST QUIRK: `.as_ref().cloned().unwrap_or_default()`
        `model_config.compat` is `Option<OpenAiCompat>`.
          `.as_ref()` → `Option<&OpenAiCompat>` (avoid moving out of model_config)
          `.cloned()` → `Option<OpenAiCompat>` (clone the inner value if Some)
          `.unwrap_or_default()` → `OpenAiCompat` (use `Default::default()` if None)
        Result: an owned `OpenAiCompat` with appropriate flags for this provider,
        defaulting to conservative "generic OpenAI-compat" if compat wasn't specified.
        */
        let compat = model_config.compat.as_ref().cloned().unwrap_or_default();

        let base_url = &model_config.base_url;
        // Append the endpoint path — base_url is like "https://api.openai.com/v1" (no trailing slash)
        let url = format!("{}/chat/completions", base_url);

        let body = build_request_body(&config, model_config, &compat);
        debug!(
            "OpenAI compat request: model={} url={}",
            config.model_config.id, url
        );

        let client = reqwest::Client::new();
        let mut request = client
            .post(&url)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!("Bearer {}", config.model_config.api_key),
            );

        // Add any extra headers from model config
        for (k, v) in &model_config.headers {
            request = request.header(k, v);
        }

        let request = request.json(&body);

        let mut es =
            EventSource::new(request).map_err(|e| ProviderError::Network(e.to_string()))?;

        /*
        ARCHITECTURE: Streaming accumulators

        Unlike Anthropic (which has explicit content_block_start events), OpenAI
        "discovers" content blocks dynamically as deltas arrive:
          - First delta with `content: Some(text)` → create a text block
          - First delta with `reasoning_content` → create a thinking block
          - First delta with `tool_calls[N]` → create buffer N in tool_call_buffers

        `tool_call_buffers` accumulates partial tool call JSON (id/name/arguments fragments)
        until the stream ends, then we convert them to `Content::ToolCall` values.
        */
        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;
        let mut tool_call_buffers: Vec<ToolCallBuffer> = Vec::new(); // scratch pads for partial tool calls

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
                            // OpenAI signals stream end with "[DONE]" (not valid JSON — check first)
                            if msg.data == "[DONE]" {
                                break;
                            }

                            /*
                            RUST QUIRK: `match serde_json::from_str(...) { Ok(c) => c, Err => continue }`

                            This is a `match` used as an expression that short-circuits on error.
                            `continue` jumps back to the top of the `loop` (skipping this chunk).
                            Some providers send non-JSON lines (e.g. comments) in their SSE stream;
                            we log and ignore them rather than failing the whole stream.

                            This is more flexible than `?` (which would return from the whole function).
                            */
                            let chunk: OpenAiChunk = match serde_json::from_str(&msg.data) {
                                Ok(c) => c,
                                Err(e) => {
                                    debug!("Failed to parse OpenAI chunk: {} data={}", e, &msg.data);
                                    continue; // skip this event, keep processing the stream
                                }
                            };

                            // Usage appears in some chunks (varies by provider — not always the last)
                            if let Some(u) = &chunk.usage {
                                usage.input = u.prompt_tokens;
                                usage.output = u.completion_tokens;
                                usage.total_tokens = u.total_tokens;
                                if let Some(details) = &u.prompt_tokens_details {
                                    usage.cache_read = details.cached_tokens; // prompt cache hits
                                }
                                if let Some(details) = &u.completion_tokens_details {
                                    usage.reasoning = details.reasoning_tokens; // o-series reasoning
                                }
                            }

                            for choice in &chunk.choices {
                                let delta = &choice.delta;

                                /*
                                ARCHITECTURE: Thinking format dispatch

                                Different providers use different field names for chain-of-thought:
                                  xAI (Grok):     `delta.reasoning`
                                  OpenRouter:     `delta.reasoning_details` (array of {type, text})
                                  OpenAI/others:  `delta.reasoning_content`

                                `ThinkingFormat::Xai`        → read from `delta.reasoning`
                                `ThinkingFormat::OpenRouter` → collect `delta.reasoning_details`
                                                               entries where type == "thinking"
                                all others                   → read from `delta.reasoning_content`

                                RUST QUIRK: `.as_deref()` on `Option<String>` → `Option<&str>`
                                  `delta.reasoning` is `Option<String>`.
                                  `.as_deref()` borrows the inner String as `&str`.
                                  Result: `Option<&str>` — `None` if the field was absent,
                                  `Some("thinking text")` if it had content.
                                */
                                // Owned string is needed for OpenRouter (assembled from array);
                                // other formats borrow directly from delta fields.
                                // `reasoning_owned` anchors the String so `reasoning` (&str) can borrow it.
                                let reasoning_owned = match compat.thinking_format {
                                    ThinkingFormat::OpenRouter => {
                                        delta.reasoning_details.as_ref().map(|details| {
                                            details
                                                .iter()
                                                .filter(|d| d.detail_type == "thinking")
                                                .filter_map(|d| d.text.as_deref())
                                                .collect::<String>()
                                        })
                                    }
                                    _ => None,
                                };
                                let reasoning = match compat.thinking_format {
                                    ThinkingFormat::Xai => delta.reasoning.as_deref(),
                                    ThinkingFormat::OpenRouter => reasoning_owned.as_deref(),
                                    _ => delta.reasoning_content.as_deref(),
                                };
                                if let Some(reasoning_text) = reasoning {
                                    // Find existing thinking block or create a new one
                                    let thinking_idx = content.iter().position(|c| matches!(c, Content::Thinking { .. }));
                                    let idx = match thinking_idx {
                                        Some(i) => i,
                                        None => {
                                            content.push(Content::Thinking { thinking: String::new(), signature: None });
                                            content.len() - 1
                                        }
                                    };
                                    if let Some(Content::Thinking { thinking, .. }) = content.get_mut(idx) {
                                        thinking.push_str(reasoning_text);
                                    }
                                    let _ = tx.send(StreamEvent::ThinkingDelta {
                                        content_index: idx,
                                        delta: reasoning_text.to_string(),
                                    });
                                }

                                // Text content — find or create a text block
                                if let Some(text) = &delta.content {
                                    let text_idx = content.iter().position(|c| matches!(c, Content::Text { .. }));
                                    let idx = match text_idx {
                                        Some(i) => i,
                                        None => {
                                            content.push(Content::Text { text: String::new() });
                                            content.len() - 1
                                        }
                                    };
                                    if let Some(Content::Text { text: t }) = content.get_mut(idx) {
                                        t.push_str(text);
                                    }
                                    let _ = tx.send(StreamEvent::TextDelta {
                                        content_index: idx,
                                        delta: text.clone(),
                                    });
                                }

                                /*
                                ARCHITECTURE: Tool call buffering

                                OpenAI streams tool calls as partial JSON fragments across
                                multiple chunks. Each `tc` (tool call delta) has:
                                  `tc.index` — which parallel tool call this belongs to
                                  `tc.id`    — call ID (only in the first delta for this index)
                                  `tc.function.name` — only in first delta
                                  `tc.function.arguments` — partial JSON, streamed across many chunks

                                We maintain `tool_call_buffers[tc_index]` as scratch pads.
                                `while tool_call_buffers.len() <= tc_index { push empty buffer }`
                                — ensures the buffer exists before indexing.

                                RUST QUIRK: `buf.name.clone_from(name)` vs `buf.name = name.clone()`
                                `.clone_from(&src)` reuses the existing String allocation if possible
                                (it calls `String::replace_range` internally). Slightly more efficient
                                than `.clone()` when the target already has capacity.
                                */
                                if let Some(tool_calls) = &delta.tool_calls {
                                    for tc in tool_calls {
                                        let tc_index = tc.index as usize;
                                        while tool_call_buffers.len() <= tc_index {
                                            tool_call_buffers.push(ToolCallBuffer::default());
                                        }
                                        let buf = &mut tool_call_buffers[tc_index];
                                        if let Some(id) = &tc.id {
                                            buf.id = id.clone();
                                        }
                                        if let Some(f) = &tc.function {
                                            if let Some(name) = &f.name {
                                                buf.name.clone_from(name);
                                                let _ = tx.send(StreamEvent::ToolCallStart {
                                                    content_index: content.len() + tc_index,
                                                    id: buf.id.clone(),
                                                    name: name.clone(),
                                                });
                                            }
                                            if let Some(args) = &f.arguments {
                                                buf.arguments.push_str(args);
                                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                                    content_index: content.len() + tc_index,
                                                    delta: args.clone(),
                                                });
                                            }
                                        }
                                    }
                                }

                                // `finish_reason` signals why the response stopped generating
                                if let Some(reason) = &choice.finish_reason {
                                    stop_reason = match reason.as_str() {
                                        "stop" => StopReason::Stop,
                                        "length" => StopReason::Length,
                                        "tool_calls" => StopReason::ToolUse,
                                        _ => StopReason::Stop,
                                    };
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let err_str = e.to_string();
                            warn!("OpenAI SSE error: {}", err_str);
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

        /*
        ARCHITECTURE: Finalizing tool calls after stream end

        Only after the stream ends do we have complete JSON for each tool call.
        We parse `buf.arguments` (the accumulated raw JSON string) and push a
        `Content::ToolCall` for each buffer.

        RUST QUIRK: `.unwrap_or(serde_json::Value::Object(Default::default()))`
        If `serde_json::from_str` fails (malformed JSON), we fall back to an
        empty JSON object `{}`. This is defensive: the agent loop should receive
        a ToolCall even if it has empty arguments, so it can report the parsing
        failure as a tool execution error rather than crashing the whole stream.
        `Default::default()` for `serde_json::Map<...>` is an empty map — so
        `serde_json::Value::Object(Default::default())` builds `{}`.
        */
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

        if !tool_call_buffers.is_empty() {
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

/// Scratch pad for accumulating a single streaming tool call across many SSE chunks.
/*
RUST QUIRK: `#[derive(Default)]` on a struct with String fields
  `String::default()` is an empty string `""`.
  So `ToolCallBuffer::default()` gives `{ id: "", name: "", arguments: "" }`.
  This lets us use `tool_call_buffers.push(ToolCallBuffer::default())` to
  create new scratch pads without writing out all the fields manually.
*/
#[derive(Default)]
struct ToolCallBuffer {
    id: String,        // tool call ID (arrives once, in the first chunk for this index)
    name: String,      // function name (arrives once)
    arguments: String, // JSON arguments (accumulated across many chunks)
}

/// Builds the JSON request body for the OpenAI Chat Completions API.
/*
ARCHITECTURE: build_request_body — translation layer (yo-core types → OpenAI JSON)

Converts our internal `StreamConfig` into the OpenAI-compatible JSON body.
The `compat` flags control which variant of the API to use:
  - System/developer role
  - `max_tokens` vs `max_completion_tokens`
  - `reasoning_effort` parameter for thinking-capable models
  - Tool result `name` field (required by some providers)
  - Image format (inline base64 vs URL reference)

RUST QUIRK: `let role = if ... { "developer" } else { "system" }` — if as an expression
  Unlike Python/Java where `if` is a statement, Rust's `if` is an EXPRESSION — it
  evaluates to a value. Both branches must have the same type (here: `&str`).
  Python analogy: `role = "developer" if compat.supports_developer_role else "system"`
*/
fn build_request_body(
    config: &StreamConfig, // REQUEST — messages, tools, model, system prompt, cache config
    model_config: &ModelConfig, // ROUTING — carries base_url (which provider) and api_key
    compat: &OpenAiCompat, // QUIRK FLAGS — per-provider behavior switches (store, dev role, reasoning format, etc.)
) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    // System prompt — role depends on whether this provider uses "developer" vs "system"
    if !config.system_prompt.is_empty() {
        let role = if compat.supports_developer_role {
            "developer" // OpenAI o-series models use "developer" for system-level instructions
        } else {
            "system" // Standard role for most other providers
        };
        messages.push(serde_json::json!({
            "role": role,
            "content": config.system_prompt,
        }));
    }

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content_to_openai(content),
                }));
            }
            Message::Assistant { content, .. } => {
                let mut parts: Vec<serde_json::Value> = Vec::new();
                let mut tool_calls: Vec<serde_json::Value> = Vec::new();

                for c in content {
                    match c {
                        Content::Text { text } => {
                            parts.push(serde_json::json!({"type": "text", "text": text}));
                        }
                        Content::ToolCall {
                            id,
                            name,
                            arguments,
                        } => {
                            tool_calls.push(serde_json::json!({
                                "id": id,
                                "type": "function",
                                "function": {"name": name, "arguments": arguments.to_string()},
                            }));
                        }
                        _ => {}
                    }
                }

                let mut msg_obj = serde_json::json!({"role": "assistant"});
                if !parts.is_empty() {
                    msg_obj["content"] = serde_json::json!(parts);
                }
                if !tool_calls.is_empty() {
                    msg_obj["tool_calls"] = serde_json::json!(tool_calls);
                }
                messages.push(msg_obj);
            }
            Message::ToolResult {
                tool_call_id,
                tool_name,
                content,
                ..
            } => {
                let content_val = if content.iter().any(|c| matches!(c, Content::Image { .. })) {
                    // Images present: use array format for multimodal tool results
                    content_to_openai(content)
                } else {
                    // Text-only: use plain string for maximum compat
                    let text = content
                        .iter()
                        .find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    serde_json::json!(text)
                };

                let mut msg_obj = serde_json::json!({
                    "role": "tool",
                    "tool_call_id": tool_call_id,
                    "content": content_val,
                });
                if compat.requires_tool_result_name {
                    msg_obj["name"] = serde_json::json!(tool_name);
                }
                messages.push(msg_obj);
            }
        }
    }

    let max_tokens_val = config.max_tokens.unwrap_or(model_config.max_tokens);
    let mut body = serde_json::json!({
        "model": config.model_config.id,
        "stream": true,
        "stream_options": {"include_usage": true},
        "messages": messages,
    });

    match compat.max_tokens_field {
        MaxTokensField::MaxCompletionTokens => {
            body["max_completion_tokens"] = serde_json::json!(max_tokens_val);
        }
        MaxTokensField::MaxTokens => {
            body["max_tokens"] = serde_json::json!(max_tokens_val);
        }
    }

    if !config.tools.is_empty() {
        let tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "type": "function",
                    "function": {
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.parameters,
                    }
                })
            })
            .collect();
        body["tools"] = serde_json::json!(tools);
    }

    if config.thinking_level != ThinkingLevel::Off && compat.supports_reasoning_effort {
        let effort = match config.thinking_level {
            ThinkingLevel::Minimal | ThinkingLevel::Low => "low",
            ThinkingLevel::Medium => "medium",
            ThinkingLevel::High => "high",
            ThinkingLevel::Off => unreachable!(),
        };
        body["reasoning_effort"] = serde_json::json!(effort);
    }

    if let Some(temp) = config.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    body
}

/// Convert our `Content` blocks to OpenAI's message content format.
/*
ARCHITECTURE: content_to_openai — two output shapes for maximum compat

OpenAI supports two shapes for message content:
  1. A plain string:  "Hello world"       — for text-only, single-block messages
  2. A parts array:  [{"type":"text",...}] — for multi-block or image-containing messages

Using a plain string where possible maximizes compatibility with older providers
and reduces JSON payload size. The array format is required for images.

RUST QUIRK: early return via `return`
  `return serde_json::json!(text)` — exits the function early with a plain string.
  After the `if` block, we fall through to build the array format.
  Python analogy: `return text` for the early-exit case.
*/
fn content_to_openai(
    content: &[Content], // SOURCE — slice of Content variants to convert; single Text → plain string, multiple → array
) -> serde_json::Value {
    // either a plain JSON string (1 text block) or an array of {type,text/image_url} objects
    // Optimization: single text block → plain string (maximum provider compatibility)
    if content.len() == 1 {
        if let Content::Text { text } = &content[0] {
            return serde_json::json!(text);
        }
    }
    let parts: Vec<serde_json::Value> = content
        .iter()
        .filter_map(|c| match c {
            Content::Text { text } => Some(serde_json::json!({"type": "text", "text": text})),
            Content::Image { data, mime_type } => Some(serde_json::json!({
                "type": "image_url",
                "image_url": {"url": format!("data:{};base64,{}", mime_type, data)},
            })),
            _ => None,
        })
        .collect();
    serde_json::json!(parts)
}

// ---------------------------------------------------------------------------
// OpenAI streaming response deserialization types (private to this module)
// ---------------------------------------------------------------------------
/*
ARCHITECTURE: Private deserialization types — mirroring OpenAI's streaming JSON

These structs mirror the shape of OpenAI's streaming response chunks.
A streaming chunk looks like:
  {
    "choices": [{
      "delta": {
        "content": "Hello ",
        "tool_calls": [{"index": 0, "id": "call_abc", "function": {"name": "bash", "arguments": "{"}}]
      },
      "finish_reason": null
    }],
    "usage": null   // only populated in the last chunk (if stream_options.include_usage = true)
  }

Multiple optional fields are marked `#[serde(default)]` so absent fields
deserialize to `None` (for `Option<T>`) or `0` (for `u64`) without error.

RUST QUIRK: `#[derive(Deserialize, Default)]` on `OpenAiDelta`
  `Default` is needed because `OpenAiChoice.delta` is not `Option<OpenAiDelta>` —
  it's always present in the JSON but may have all-None fields. The `#[derive(Default)]`
  provides the "all fields are None" value for `unwrap_or_default()` call sites.
*/

#[derive(Deserialize)]
struct OpenAiChunk {
    #[serde(default)]
    choices: Vec<OpenAiChoice>,
    #[serde(default)]
    usage: Option<OpenAiUsage>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    delta: OpenAiDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

/// A single entry in OpenRouter's `reasoning_details` array.
#[derive(Deserialize)]
struct OpenRouterReasoningDetail {
    #[serde(rename = "type")]
    detail_type: String,
    #[serde(default)]
    text: Option<String>,
}

#[derive(Deserialize, Default)]
struct OpenAiDelta {
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    reasoning_content: Option<String>,
    #[serde(default)]
    reasoning: Option<String>,
    /// OpenRouter extended thinking: array of `{type, text}` objects.
    #[serde(default)]
    reasoning_details: Option<Vec<OpenRouterReasoningDetail>>,
    #[serde(default)]
    tool_calls: Option<Vec<OpenAiToolCallDelta>>,
}

#[derive(Deserialize)]
struct OpenAiToolCallDelta {
    #[serde(default)]
    index: u32,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    function: Option<OpenAiFunctionDelta>,
}

#[derive(Deserialize)]
struct OpenAiFunctionDelta {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    prompt_tokens_details: Option<OpenAiPromptTokensDetails>,
    #[serde(default)]
    completion_tokens_details: Option<OpenAiCompletionTokensDetails>,
}

#[derive(Deserialize)]
struct OpenAiPromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Deserialize)]
struct OpenAiCompletionTokensDetails {
    #[serde(default)]
    reasoning_tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::model::ModelConfig;

    #[test]
    fn test_build_request_body_basic() {
        let model_config = ModelConfig::openai("gpt-4o", "GPT-4o", "test");
        let config = StreamConfig {
            model_config: model_config.clone(),
            system_prompt: "You are helpful.".into(),
            messages: vec![Message::user("Hello")],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            cache_config: CacheConfig::default(),
        };

        let body = build_request_body(&config, &model_config, &OpenAiCompat::openai());
        assert_eq!(body["model"], "gpt-4o");
        assert!(body["stream"].as_bool().unwrap());
        // Developer role for OpenAI
        assert_eq!(body["messages"][0]["role"], "developer");
        assert_eq!(body["messages"][1]["role"], "user");
        // max_completion_tokens for OpenAI
        assert!(body["max_completion_tokens"].is_number());
    }

    #[test]
    fn test_build_request_body_with_tools() {
        let model_config = ModelConfig::openai("gpt-4o", "GPT-4o", "test");
        let compat = OpenAiCompat::openai();
        let config = StreamConfig {
            model_config: model_config.clone(),
            system_prompt: String::new(),
            messages: vec![Message::user("List files")],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run a command".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            thinking_level: ThinkingLevel::Off,
            max_tokens: Some(1024),
            temperature: Some(0.5),
            cache_config: CacheConfig::default(),
        };

        let body = build_request_body(&config, &model_config, &compat);
        assert!(body["tools"].is_array());
        assert_eq!(body["tools"][0]["function"]["name"], "bash");
        assert_eq!(body["temperature"], 0.5);
    }

    #[test]
    fn test_content_to_openai_simple_text() {
        let content = vec![Content::Text {
            text: "hello".into(),
        }];
        let result = content_to_openai(&content);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_content_to_openai_multipart() {
        let content = vec![
            Content::Text {
                text: "look at this".into(),
            },
            Content::Image {
                data: "abc".into(),
                mime_type: "image/png".into(),
            },
        ];
        let result = content_to_openai(&content);
        assert!(result.is_array());
        assert_eq!(result[0]["type"], "text");
        assert_eq!(result[1]["type"], "image_url");
    }

    #[test]
    fn test_tool_result_with_image() {
        let model_config = ModelConfig::openai("gpt-4o", "GPT-4o", "test");
        let compat = OpenAiCompat::openai();
        let config = StreamConfig {
            model_config: model_config.clone(),
            system_prompt: String::new(),
            messages: vec![
                Message::Assistant {
                    content: vec![Content::ToolCall {
                        id: "call-1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "img.png"}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    model: "test".into(),
                    provider: "test".into(),
                    usage: Usage::default(),
                    timestamp: 0,
                    error_message: None,
                },
                Message::ToolResult {
                    tool_call_id: "call-1".into(),
                    tool_name: "read_file".into(),
                    content: vec![Content::Image {
                        data: "aW1hZ2VkYXRh".into(),
                        mime_type: "image/png".into(),
                    }],
                    is_error: false,
                    timestamp: 0,
                },
            ],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            cache_config: CacheConfig::default(),
        };

        let body = build_request_body(&config, &model_config, &compat);
        let msgs = body["messages"].as_array().unwrap();
        // tool result is the last message (after system + assistant)
        let tool_msg = msgs.last().unwrap();
        assert_eq!(tool_msg["role"], "tool");
        // content should be an array with image_url
        let content = tool_msg["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "image_url");
        assert!(content[0]["image_url"]["url"]
            .as_str()
            .unwrap()
            .starts_with("data:image/png;base64,"));
    }

    #[test]
    fn test_tool_result_text_only_uses_string() {
        let model_config = ModelConfig::openai("gpt-4o", "GPT-4o", "test");
        let compat = OpenAiCompat::openai();
        let config = StreamConfig {
            model_config: model_config.clone(),
            system_prompt: String::new(),
            messages: vec![Message::ToolResult {
                tool_call_id: "call-1".into(),
                tool_name: "bash".into(),
                content: vec![Content::Text {
                    text: "hello".into(),
                }],
                is_error: false,
                timestamp: 0,
            }],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            cache_config: CacheConfig::default(),
        };

        let body = build_request_body(&config, &model_config, &compat);
        let msgs = body["messages"].as_array().unwrap();
        let tool_msg = msgs.last().unwrap();
        // Text-only: content should be a plain string
        assert_eq!(tool_msg["content"], "hello");
    }
}
