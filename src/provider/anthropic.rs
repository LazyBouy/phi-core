//! Anthropic Claude provider (Messages API with streaming)
/*
ARCHITECTURE: AnthropicProvider — one struct, one job

`AnthropicProvider` is a zero-field unit struct (no state). All behaviour
lives in the `StreamProvider::stream()` method. The provider is stateless:
it doesn't cache connections or store conversation history — that's the
agent loop's responsibility.

The overall flow of `stream()`:
  1. Build the JSON request body (messages → Anthropic format, prompt caching)
  2. Start an HTTP POST with streaming enabled (reqwest EventSource)
  3. Process SSE events in a loop until "message_stop" or error
  4. Assemble the complete `Message::Assistant` from accumulated content + usage
  5. Send `StreamEvent::Done` on the channel; return the `Message`

ARCHITECTURE: Anthropic SSE event sequence

The Anthropic streaming API emits events in this order:
  message_start         — response metadata, initial input token usage
  content_block_start   — a new content block begins (text / thinking / tool_use)
  content_block_delta*  — incremental content for the current block
  content_block_stop    — content block complete
  message_delta         — final stop_reason and output token usage
  message_stop          — stream ended

Multiple content blocks may interleave (text + tool_use simultaneously):
  content_block_start(0, text)
  content_block_delta(0, text_delta: "Hello ")
  content_block_start(1, tool_use: {id, name})
  content_block_delta(1, input_json_delta: "{\"cmd")
  content_block_delta(0, text_delta: "world")
  content_block_delta(1, input_json_delta: "\": \"ls\"}")
  content_block_stop(0)
  content_block_stop(1)
  message_delta(stop_reason: "tool_use")
  message_stop

ARCHITECTURE: Prompt caching

Anthropic supports prompt caching with `"cache_control": {"type": "ephemeral"}`
markers on system/tools/messages. The provider places up to 3 cache breakpoints
(see `build_request_body` comments). On cache hits, the API returns reduced
`cache_read_input_tokens` billing and lower latency.
*/

use super::traits::*;
use crate::types::*;
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use serde::Deserialize;
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Default Anthropic Messages API endpoint.
const API_URL: &str = "https://api.anthropic.com/v1/messages";
/// Required `anthropic-version` header value.
const API_VERSION: &str = "2023-06-01";

/// Unit struct — no fields, no state. All logic is in the `StreamProvider` impl.
pub struct AnthropicProvider;

#[async_trait]
impl StreamProvider for AnthropicProvider {
    fn provider_id(&self) -> &str {
        "anthropic"
    }

    async fn stream(
        &self,
        config: StreamConfig, // REQUEST — includes api_key (sk-ant-* or sk-ant-oat* for OAuth)
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — receives SSE events as they arrive
        cancel: tokio_util::sync::CancellationToken, // ABORT — races against SSE stream in tokio::select!
    ) -> Result<Message, ProviderError> {
        /*
        ARCHITECTURE: OAuth vs API key auth

        Anthropic supports two auth modes:
          1. API key (`sk-ant-...`)  — simple `x-api-key` header; for direct API access
          2. OAuth token (`sk-ant-oat...`) — `Authorization: Bearer ...` header;
             used by Claude Code CLI (authenticated via claude.ai). Adds extra identity
             headers so Anthropic can attribute usage to the Claude Code product.

        We detect OAuth by prefix: "sk-ant-oat" in the key. This is fragile but matches
        the real-world key format Anthropic uses for its own OAuth tokens.
        */
        let is_oauth = config.api_key.contains("sk-ant-oat");
        let body = build_request_body(&config, is_oauth);
        debug!(
            "Anthropic request: model={}, oauth={}",
            config.model, is_oauth
        );

        /*
        RUST QUIRK: Builder pattern on `reqwest::Client`

        `reqwest::Client::new()` creates a reusable HTTP client (connection pool, TLS).
        In production you'd store this in the provider, but for simplicity we create
        one per call here.

        `client.post(URL).header(...).header(...).json(&body)` is a fluent builder:
          each method takes `self` by value and returns a new `RequestBuilder`.
          `.json(&body)` serializes `body` (a `serde_json::Value`) to JSON bytes and
          sets Content-Type: application/json.

        RUST QUIRK: `let mut builder = ...` — reassigning via `mut`
        We conditionally add different headers based on `is_oauth`.
        `builder = builder.header(...)` — the old builder is consumed, the new one with
        the extra header is returned. This is builder chaining with conditional branches.
        */
        let client = reqwest::Client::new();
        let mut builder = client
            .post(API_URL)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json");

        if is_oauth {
            // OAuth token — Bearer auth with Claude Code identity headers
            builder = builder
                .header("authorization", format!("Bearer {}", config.api_key))
                .header(
                    "anthropic-beta",
                    "claude-code-20250219,oauth-2025-04-20,fine-grained-tool-streaming-2025-05-14",
                )
                .header("anthropic-dangerous-direct-browser-access", "true")
                .header("user-agent", "claude-cli/2.1.2 (external, cli)")
                .header("x-app", "cli");
        } else {
            // Standard API key auth
            builder = builder.header("x-api-key", &config.api_key);
        }

        let request = builder.json(&body);

        /*
        RUST QUIRK: `EventSource::new(request).map_err(|e| ProviderError::Network(...))?`

        `EventSource::new(request)` returns `Result<EventSource, Error>`.
        `.map_err(|e| ProviderError::Network(e.to_string()))` transforms the error type:
          `Error` (reqwest_eventsource) → `ProviderError::Network(String)`
          This is needed because `stream()` must return `Result<Message, ProviderError>`,
          not `Result<Message, reqwest_eventsource::Error>`.

        `.map_err` is the standard "transform the Err variant, leave Ok untouched" combinator.
        Python analogy: re-raising as a different exception type.

        The `?` then unwraps the `Ok` or returns the `Err` early.
        */
        let mut es =
            EventSource::new(request).map_err(|e| ProviderError::Network(e.to_string()))?;

        /*
        ARCHITECTURE: Streaming state — accumulator variables

        We accumulate the full response in these variables across many SSE events:
          `content`     — grows as content_block_start/delta events arrive
                         starts empty; we push new Content variants for each new block
          `usage`       — filled by message_start (input tokens) and message_delta (output tokens)
          `stop_reason` — updated by message_delta; default is Stop until we know better

        At stream end, these three are assembled into `Message::Assistant { ... }`.

        RUST QUIRK: `Vec<Content>` — a growable array on the heap
          `Vec::new()` creates an empty vector with no allocation.
          We `.push()` items as blocks are discovered.
          `content.get_mut(idx)` — returns `Option<&mut Content>` at index `idx`.
            Returns None if idx >= len, so we protect with `while content.len() <= idx { push }`.

        RUST QUIRK: `let _ = tx.send(...)` — intentionally ignoring the Result
          `UnboundedSender::send()` returns `Err` if the receiver is dropped.
          We don't care — if the UI isn't listening, we still want to continue.
          `let _ = ` explicitly discards the value, silencing the "unused Result" compiler lint.
        */
        let mut content: Vec<Content> = Vec::new();
        let mut usage = Usage::default();
        let mut stop_reason = StopReason::Stop;

        let _ = tx.send(StreamEvent::Start); // notify UI that streaming has begun

        /*
        ARCHITECTURE: The SSE event loop — a streaming state machine

        This `loop` processes Anthropic SSE events one by one. It runs until:
          - "message_stop" arrives → break and assemble the final Message
          - "error" event          → return an error Message
          - SSE error              → return an error Message
          - Cancellation           → return Err(Cancelled)

        `tokio::select!` races two futures each iteration:
          1. `cancel.cancelled()` — user pressed Ctrl-C or the agent was aborted
          2. `es.next()`          — the next SSE event from Anthropic's HTTP stream

        The first to complete "wins" and its branch runs. The loser is dropped.
        This is the idiomatic Rust pattern for "process events but stay interruptible."
        */
        loop {
            tokio::select! {
                _ = cancel.cancelled() => {
                    es.close();
                    return Err(ProviderError::Cancelled);
                }
                event = es.next() => {
                    match event {
                        None => break,
                        Some(Ok(Event::Open)) => {}
                        Some(Ok(Event::Message(msg))) => {
                            /*
                            RUST QUIRK: `msg.event.as_str()` for pattern matching
                            `msg.event` is a `String`. We can't match on `String` directly
                            because Rust's match requires compile-time known sizes.
                            `.as_str()` converts `String` → `&str`, which CAN be matched.
                            Each arm is a string literal (a `&'static str`).
                            Python analogy: `match msg.event:` with `case "message_start":` etc.
                            */
                            match msg.event.as_str() {
                                "message_start" => {
                                    /*
                                    RUST QUIRK: `if let Ok(data) = serde_json::from_str::<T>(&s)`
                                    `serde_json::from_str::<AnthropicMessageStart>(&msg.data)` tries
                                    to deserialize the JSON string into our Rust struct.
                                    If deserialization succeeds → `Ok(data)`, and `if let Ok(data)` binds it.
                                    If it fails → `Err(e)`, and we silently skip this event (no panic).
                                    We tolerate partial / unknown events gracefully — the stream continues.
                                    `::<AnthropicMessageStart>` is a "turbofish" — explicit type parameter.
                                    */
                                    if let Ok(data) = serde_json::from_str::<AnthropicMessageStart>(&msg.data) {
                                        usage.input = data.message.usage.input_tokens;
                                        usage.cache_read = data.message.usage.cache_read_input_tokens;
                                        usage.cache_write = data.message.usage.cache_creation_input_tokens;
                                    }
                                }
                                "content_block_start" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicContentBlockStart>(&msg.data) {
                                        let idx = data.index as usize; // u64 → usize (safe: index is tiny)
                                        match data.content_block {
                                            AnthropicContentBlock::Text { .. } => {
                                                // Pad the content Vec with empty Text blocks up to this index
                                                while content.len() <= idx {
                                                    content.push(Content::Text { text: String::new() });
                                                }
                                            }
                                            AnthropicContentBlock::Thinking { .. } => {
                                                while content.len() <= idx {
                                                    content.push(Content::Thinking { thinking: String::new(), signature: None });
                                                }
                                            }
                                            AnthropicContentBlock::ToolUse { id, name, .. } => {
                                                while content.len() <= idx {
                                                    content.push(Content::ToolCall {
                                                        id: id.clone(),
                                                        name: name.clone(),
                                                        // Placeholder — will hold accumulated JSON fragments
                                                        arguments: serde_json::Value::Object(Default::default()),
                                                    });
                                                }
                                                // Notify the UI that a tool call has started
                                                let _ = tx.send(StreamEvent::ToolCallStart {
                                                    content_index: idx,
                                                    id,
                                                    name,
                                                });
                                            }
                                        }
                                    }
                                }
                                "content_block_delta" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicContentBlockDelta>(&msg.data) {
                                        let idx = data.index as usize;
                                        match data.delta {
                                            AnthropicDelta::TextDelta { text } => {
                                                /*
                                                RUST QUIRK: `if let Some(Content::Text { text: ref mut t }) = content.get_mut(idx)`
                                                Pattern match with multiple levels at once:
                                                  - `content.get_mut(idx)` → `Option<&mut Content>`
                                                  - `Some(...)` unpacks the Option
                                                  - `Content::Text { text: ref mut t }` destructures the enum variant,
                                                    binding `t` as a mutable reference to the `text` field
                                                  - `ref mut` means "bind this field BY mutable reference, don't move it"
                                                  - `t.push_str(&text)` appends to the string IN PLACE

                                                Python analogy:
                                                  block = content[idx]
                                                  if isinstance(block, TextContent):
                                                      block.text += text
                                                */
                                                if let Some(Content::Text { text: ref mut t }) = content.get_mut(idx) {
                                                    t.push_str(&text);
                                                }
                                                let _ = tx.send(StreamEvent::TextDelta {
                                                    content_index: idx,
                                                    delta: text,
                                                });
                                            }
                                            AnthropicDelta::ThinkingDelta { thinking } => {
                                                if let Some(Content::Thinking { thinking: ref mut t, .. }) = content.get_mut(idx) {
                                                    t.push_str(&thinking);
                                                }
                                                let _ = tx.send(StreamEvent::ThinkingDelta {
                                                    content_index: idx,
                                                    delta: thinking,
                                                });
                                            }
                                            AnthropicDelta::InputJsonDelta { partial_json } => {
                                                /*
                                                ARCHITECTURE: Tool argument JSON accumulation
                                                Anthropic streams tool arguments as partial JSON fragments:
                                                  chunk 1: "{\"cmd\":"
                                                  chunk 2: " \"ls -la\"}"
                                                We can't parse partial JSON, so we buffer in a hidden
                                                `__partial_json` key inside the arguments object.
                                                At `content_block_stop`, we parse the full accumulated string.

                                                Why store it in `arguments` itself? To avoid a separate HashMap
                                                of scratch buffers indexed by content_block index.
                                                */
                                                if let Some(Content::ToolCall { ref mut arguments, .. }) = content.get_mut(idx) {
                                                    // Append to string buffer stored in arguments
                                                    // We accumulate the raw JSON string and parse it at content_block_stop
                                                    let buf = arguments
                                                        .as_object_mut()
                                                        .and_then(|o| o.get_mut("__partial_json"))
                                                        .and_then(|v| v.as_str().map(|s| s.to_string()));
                                                    let new_buf = format!("{}{}", buf.unwrap_or_default(), partial_json);
                                                    if let Some(obj) = arguments.as_object_mut() {
                                                        obj.insert("__partial_json".into(), serde_json::Value::String(new_buf));
                                                    }
                                                }
                                                let _ = tx.send(StreamEvent::ToolCallDelta {
                                                    content_index: idx,
                                                    delta: partial_json,
                                                });
                                            }
                                            AnthropicDelta::SignatureDelta { signature } => {
                                                // Extended thinking: the signature authenticates the thinking block
                                                if let Some(Content::Thinking { signature: ref mut s, .. }) = content.get_mut(idx) {
                                                    *s = Some(signature); // `*s` dereferences the &mut Option<String>
                                                }
                                            }
                                        }
                                    }
                                }
                                "content_block_stop" => {
                                    if let Ok(data) = serde_json::from_str::<serde_json::Value>(&msg.data) {
                                        let idx = data["index"].as_u64().unwrap_or(0) as usize;
                                        // Parse accumulated JSON for tool calls
                                        if let Some(Content::ToolCall { ref mut arguments, .. }) = content.get_mut(idx) {
                                            if let Some(partial) = arguments.as_object()
                                                .and_then(|o| o.get("__partial_json"))
                                                .and_then(|v| v.as_str())
                                                .map(|s| s.to_string())
                                            {
                                                if let Ok(parsed) = serde_json::from_str(&partial) {
                                                    *arguments = parsed; // replace placeholder with real parsed JSON
                                                } else {
                                                    warn!("Failed to parse tool call JSON: {}", partial);
                                                    *arguments = serde_json::Value::Object(Default::default());
                                                }
                                            }
                                        }
                                        let _ = tx.send(StreamEvent::ToolCallEnd { content_index: idx });
                                    }
                                }
                                "message_delta" => {
                                    if let Ok(data) = serde_json::from_str::<AnthropicMessageDelta>(&msg.data) {
                                        /*
                                        RUST QUIRK: `as_deref()` — converting Option<String> to Option<&str>
                                        `data.delta.stop_reason` is `Option<String>`.
                                        `.as_deref()` converts it to `Option<&str>` — borrowing the inner string.
                                        This lets us match with `Some("tool_use")` etc. without cloning.
                                        `match data.delta.stop_reason.as_deref()`:
                                          Some("tool_use")   → StopReason::ToolUse
                                          Some("max_tokens") → StopReason::Length
                                          Some("end_turn") | None | Some(_) → StopReason::Stop
                                        */
                                        stop_reason = match data.delta.stop_reason.as_deref() {
                                            Some("tool_use") => StopReason::ToolUse,
                                            Some("max_tokens") => StopReason::Length,
                                            _ => StopReason::Stop,
                                        };
                                        usage.output = data.usage.output_tokens;
                                    }
                                }
                                "message_stop" => break, // stream complete — exit the loop
                                "ping" => {}             // Anthropic sends periodic pings; ignore them
                                "error" => {
                                    warn!("Anthropic stream error: {}", msg.data);
                                    let err_msg = Message::Assistant {
                                        content: vec![Content::Text { text: String::new() }],
                                        stop_reason: StopReason::Error,
                                        model: config.model.clone(),
                                        provider: "anthropic".into(),
                                        usage: usage.clone(),
                                        timestamp: now_ms(),
                                        error_message: Some(msg.data),
                                    };
                                    let _ = tx.send(StreamEvent::Error { message: err_msg.clone() });
                                    return Ok(err_msg);
                                }
                                other => {
                                    debug!("Unknown Anthropic event: {}", other);
                                }
                            }
                        }
                        Some(Err(e)) => {
                            let err_str = e.to_string();
                            warn!("SSE error: {}", err_str);
                            let err_msg = Message::Assistant {
                                content: vec![Content::Text { text: String::new() }],
                                stop_reason: StopReason::Error,
                                model: config.model.clone(),
                                provider: "anthropic".into(),
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

        let has_tool_calls = content
            .iter()
            .any(|c| matches!(c, Content::ToolCall { .. }));
        if has_tool_calls {
            stop_reason = StopReason::ToolUse;
        }

        let message = Message::Assistant {
            content,
            stop_reason,
            model: config.model.clone(),
            provider: "anthropic".into(),
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

// ---------------------------------------------------------------------------
// Anthropic API request/response types
// ---------------------------------------------------------------------------

/// Builds the JSON request body for the Anthropic Messages API.
/*
ARCHITECTURE: build_request_body — translation layer (yo-core types → Anthropic JSON)

The Anthropic API expects a specific JSON format. This function converts:
  - `Message::User/Assistant/ToolResult` → Anthropic message objects
  - `Content::Text/Image/Thinking/ToolCall` → Anthropic content blocks
  - `ThinkingLevel` → `"thinking": { "type": "enabled", "budget_tokens": N }`
  - `CacheConfig` → `"cache_control": {"type": "ephemeral"}` markers

RUST QUIRK: `serde_json::json!({...})` — macro for inline JSON construction
  `json!` is a macro that converts a Rust literal into a `serde_json::Value`.
  It supports Rust expressions inline: `json!({"model": config.model})` → the
  string value of `config.model` is embedded at the "model" key.
  Python analogy: dict literals like `{"model": config.model}`.

RUST QUIRK: `&[Content]` — a slice reference as a function parameter
  `content_to_anthropic(content)` takes `&[Content]`.
  When called with `content` (a `Vec<Content>`), Rust auto-coerces `Vec<T>` → `&[T]`.
  The function receives a read-only view of the contents without any allocation.
*/
fn build_request_body(
    config: &StreamConfig, // REQUEST — messages, tools, model, system prompt, cache config
    is_oauth: bool, // AUTH MODE — true = OAuth (adds claude-code product headers); false = API key only
) -> serde_json::Value {
    let mut messages: Vec<serde_json::Value> = Vec::new();

    for msg in &config.messages {
        match msg {
            Message::User { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "user",
                    "content": content_to_anthropic(content),
                }));
            }
            Message::Assistant { content, .. } => {
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": content_to_anthropic(content),
                }));
            }
            Message::ToolResult {
                tool_call_id,
                content,
                is_error,
                ..
            } => {
                let result_content = if content.iter().any(|c| matches!(c, Content::Image { .. })) {
                    // Multi-content with images: use array format
                    serde_json::json!(content_to_anthropic(content))
                } else {
                    // Text-only: use string shorthand
                    let text = content
                        .iter()
                        .find_map(|c| match c {
                            Content::Text { text } => Some(text.clone()),
                            _ => None,
                        })
                        .unwrap_or_default();
                    serde_json::json!(text)
                };

                messages.push(serde_json::json!({
                    "role": "user",
                    "content": [{
                        "type": "tool_result",
                        "tool_use_id": tool_call_id,
                        "content": result_content,
                        "is_error": is_error,
                    }],
                }));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Prompt caching — place cache_control breakpoints based on CacheConfig.
    //
    // Anthropic caches the full prefix (tools → system → messages) up to each
    // breakpoint. We use up to 3 breakpoints:
    //   1. System prompt (stable across turns)
    //   2. Last tool definition (tools rarely change)
    //   3. Second-to-last message (conversation history grows, cache the prefix)
    //
    // When caching is disabled or strategy is Disabled, no markers are added.
    // -----------------------------------------------------------------------
    let cache = &config.cache_config;
    let caching_enabled = cache.enabled && cache.strategy != CacheStrategy::Disabled;
    let (cache_system, cache_tools, cache_messages) = match &cache.strategy {
        CacheStrategy::Auto => (true, true, true),
        CacheStrategy::Disabled => (false, false, false),
        CacheStrategy::Manual {
            cache_system,
            cache_tools,
            cache_messages,
        } => (*cache_system, *cache_tools, *cache_messages),
    };

    // Breakpoint 3: second-to-last message (cache conversation prefix)
    if caching_enabled && cache_messages && messages.len() >= 2 {
        let cache_idx = messages.len() - 2;
        if let Some(content) = messages[cache_idx]["content"].as_array_mut() {
            if let Some(last_block) = content.last_mut() {
                last_block["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
        }
    }

    let mut body = serde_json::json!({
        "model": config.model,
        "max_tokens": config.max_tokens.unwrap_or(8192),
        "stream": true,
        "messages": messages,
    });

    // Breakpoint 1: system prompt
    if is_oauth {
        let mut system_blocks = vec![serde_json::json!({
            "type": "text",
            "text": "You are Claude Code, Anthropic's official CLI for Claude.",
        })];
        if !config.system_prompt.is_empty() {
            system_blocks.push(serde_json::json!({
                "type": "text",
                "text": config.system_prompt,
            }));
        }
        // Cache the last system block
        if caching_enabled && cache_system {
            if let Some(last) = system_blocks.last_mut() {
                last["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
        }
        body["system"] = serde_json::json!(system_blocks);
    } else if !config.system_prompt.is_empty() {
        let mut block = serde_json::json!({
            "type": "text",
            "text": config.system_prompt,
        });
        if caching_enabled && cache_system {
            block["cache_control"] = serde_json::json!({"type": "ephemeral"});
        }
        body["system"] = serde_json::json!([block]);
    }

    // Breakpoint 2: last tool definition (tools are stable between turns)
    if !config.tools.is_empty() {
        let mut tools: Vec<serde_json::Value> = config
            .tools
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": t.parameters,
                })
            })
            .collect();
        if caching_enabled && cache_tools {
            if let Some(last_tool) = tools.last_mut() {
                last_tool["cache_control"] = serde_json::json!({"type": "ephemeral"});
            }
        }
        body["tools"] = serde_json::json!(tools);
    }

    if config.thinking_level != ThinkingLevel::Off {
        let budget = match config.thinking_level {
            ThinkingLevel::Minimal => 128,
            ThinkingLevel::Low => 512,
            ThinkingLevel::Medium => 2048,
            ThinkingLevel::High => 8192,
            ThinkingLevel::Off => 0,
        };
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "budget_tokens": budget,
        });
    }

    if let Some(temp) = config.temperature {
        body["temperature"] = serde_json::json!(temp);
    }

    body
}

fn content_to_anthropic(content: &[Content]) -> Vec<serde_json::Value> {
    content
        .iter()
        .map(|c| match c {
            Content::Text { text } => serde_json::json!({"type": "text", "text": text}),
            Content::Image { data, mime_type } => serde_json::json!({
                "type": "image",
                "source": {"type": "base64", "media_type": mime_type, "data": data},
            }),
            Content::Thinking {
                thinking,
                signature,
            } => serde_json::json!({
                "type": "thinking",
                "thinking": thinking,
                "signature": signature.as_deref().unwrap_or(""),
            }),
            Content::ToolCall {
                id,
                name,
                arguments,
            } => serde_json::json!({
                "type": "tool_use",
                "id": id,
                "name": name,
                "input": arguments,
            }),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Anthropic SSE event deserialization types (private to this module)
// ---------------------------------------------------------------------------
/*
ARCHITECTURE: Private deserialization types — the "decoder ring" for Anthropic's JSON

These structs mirror Anthropic's SSE event JSON shapes exactly. They exist only
to deserialize event data — they're never stored or returned to callers.
Using dedicated structs (vs parsing `serde_json::Value` fields manually) means:
  - Compile-time field names (typos caught at build time)
  - Automatic error handling (`serde_json::from_str` returns Err on shape mismatch)
  - Self-documenting: the struct shows exactly what fields we expect

RUST QUIRK: `#[derive(Deserialize)]` — serde auto-generates deserialization
  The `Deserialize` derive reads field names from the struct definition.
  It maps JSON key names to field names. If they don't match, use `#[serde(rename = "...")]`.

RUST QUIRK: `#[serde(tag = "type")]` — "externally tagged" enum deserialization
  When deserializing `AnthropicContentBlock`, serde looks at the JSON's "type" field
  to decide which variant to construct:
    {"type": "text", "text": "Hello"}         → Text { text: "Hello" }
    {"type": "tool_use", "id": ..., "name":...} → ToolUse { id: ..., name: ... }
  This is the "internally tagged" enum pattern — the discriminant ("type") is a
  field inside the JSON object, not wrapping the whole thing.

RUST QUIRK: `#[allow(dead_code)]` — suppress "field never read" warnings
  The `text` field of `AnthropicContentBlock::Text` is present in the JSON but
  we don't need it (we initialize the content block with an empty string and fill
  it via deltas). `#[allow(dead_code)]` tells the compiler "yes, I know, I don't care."

RUST QUIRK: `#[serde(default)]` on struct fields
  If a field is absent in JSON, serde uses `Default::default()` instead of failing.
  For `u64`, `Default::default()` = `0`. This handles older API responses that
  don't include all usage fields.
*/

#[derive(Deserialize)]
struct AnthropicMessageStart {
    message: AnthropicMessageInfo,
}

#[derive(Deserialize)]
struct AnthropicMessageInfo {
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
}

#[derive(Deserialize)]
struct AnthropicContentBlockStart {
    index: u64,
    content_block: AnthropicContentBlock,
}

/// Anthropic content block type (text, thinking, or tool_use).
/// Dispatched by the "type" field in the JSON.
#[derive(Deserialize)]
#[serde(tag = "type")]
enum AnthropicContentBlock {
    #[serde(rename = "text")]
    Text {
        #[allow(dead_code)]
        text: String, // initial text (empty in streaming; filled via TextDelta events)
    },
    #[serde(rename = "thinking")]
    Thinking {
        #[allow(dead_code)]
        thinking: String, // initial thinking (empty in streaming)
    },
    #[serde(rename = "tool_use")]
    ToolUse { id: String, name: String },
}

#[derive(Deserialize)]
struct AnthropicContentBlockDelta {
    index: u64,
    delta: AnthropicDelta,
}

/// Delta variants for incremental content within a content block.
/*
RUST QUIRK: `#[allow(clippy::enum_variant_names)]` — suppress a clippy lint

Clippy warns when all variants of an enum end with the same suffix
(here: `TextDelta`, `ThinkingDelta`, `InputJsonDelta`, `SignatureDelta` all end in `Delta`).
Clippy suggests removing the common suffix, but `Delta` is part of the Anthropic API
terminology, and removing it would make the variants less clear.
`#[allow(...)]` silences this specific lint for this item only.
*/
#[derive(Deserialize)]
#[serde(tag = "type")]
#[allow(clippy::enum_variant_names)]
enum AnthropicDelta {
    #[serde(rename = "text_delta")]
    TextDelta { text: String },
    #[serde(rename = "thinking_delta")]
    ThinkingDelta { thinking: String },
    #[serde(rename = "input_json_delta")]
    InputJsonDelta { partial_json: String },
    #[serde(rename = "signature_delta")]
    SignatureDelta { signature: String },
}

#[derive(Deserialize)]
struct AnthropicMessageDelta {
    delta: AnthropicMessageDeltaInner,
    usage: AnthropicUsage,
}

#[derive(Deserialize)]
struct AnthropicMessageDeltaInner {
    stop_reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::traits::ToolDefinition;

    fn make_config(cache: CacheConfig) -> StreamConfig {
        StreamConfig {
            model: "claude-sonnet-4-20250514".into(),
            system_prompt: "You are helpful.".into(),
            messages: vec![
                Message::user("Hello"),
                Message::User {
                    content: vec![Content::Text {
                        text: "What is 2+2?".into(),
                    }],
                    timestamp: 0,
                },
            ],
            tools: vec![ToolDefinition {
                name: "bash".into(),
                description: "Run commands".into(),
                parameters: serde_json::json!({"type": "object"}),
            }],
            thinking_level: ThinkingLevel::Off,
            api_key: "test-key".into(),
            max_tokens: Some(1024),
            temperature: None,
            model_config: None,
            cache_config: cache,
        }
    }

    #[test]
    fn test_cache_auto_places_all_breakpoints() {
        let body = build_request_body(&make_config(CacheConfig::default()), false);

        // System prompt should have cache_control
        let system = &body["system"][0];
        assert_eq!(system["cache_control"]["type"], "ephemeral");

        // Last tool should have cache_control
        let tools = body["tools"].as_array().unwrap();
        let last_tool = tools.last().unwrap();
        assert_eq!(last_tool["cache_control"]["type"], "ephemeral");

        // Second-to-last message should have cache_control
        let msgs = body["messages"].as_array().unwrap();
        let second_to_last = &msgs[msgs.len() - 2];
        let content = second_to_last["content"].as_array().unwrap();
        let last_block = content.last().unwrap();
        assert_eq!(last_block["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn test_cache_disabled_no_breakpoints() {
        let config = CacheConfig {
            enabled: false,
            strategy: CacheStrategy::Auto,
        };
        let body = build_request_body(&make_config(config), false);

        // System prompt should NOT have cache_control
        let system = &body["system"][0];
        assert!(system.get("cache_control").is_none());

        // Tools should NOT have cache_control
        let tools = body["tools"].as_array().unwrap();
        assert!(tools.last().unwrap().get("cache_control").is_none());

        // Messages should NOT have cache_control on any block
        let msgs = body["messages"].as_array().unwrap();
        for msg in msgs {
            if let Some(content) = msg["content"].as_array() {
                for block in content {
                    assert!(block.get("cache_control").is_none());
                }
            }
        }
    }

    #[test]
    fn test_cache_manual_system_only() {
        let config = CacheConfig {
            enabled: true,
            strategy: CacheStrategy::Manual {
                cache_system: true,
                cache_tools: false,
                cache_messages: false,
            },
        };
        let body = build_request_body(&make_config(config), false);

        // System: cached
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        // Tools: not cached
        assert!(body["tools"]
            .as_array()
            .unwrap()
            .last()
            .unwrap()
            .get("cache_control")
            .is_none());
        // Messages: not cached
        let msgs = body["messages"].as_array().unwrap();
        let second = &msgs[msgs.len() - 2];
        let content = second["content"].as_array().unwrap();
        assert!(content.last().unwrap().get("cache_control").is_none());
    }

    #[test]
    fn test_usage_cache_hit_rate() {
        let usage = Usage {
            input: 100,
            output: 50,
            cache_read: 900,
            cache_write: 0,
            total_tokens: 1050,
        };
        let rate = usage.cache_hit_rate();
        assert!((rate - 0.9).abs() < 0.001); // 900 / (100 + 900 + 0) = 0.9

        let empty = Usage::default();
        assert_eq!(empty.cache_hit_rate(), 0.0);
    }

    #[test]
    fn test_tool_result_with_image() {
        let config = StreamConfig {
            model: "claude-sonnet-4-20250514".into(),
            system_prompt: "".into(),
            messages: vec![
                Message::Assistant {
                    content: vec![Content::ToolCall {
                        id: "tc-1".into(),
                        name: "read_file".into(),
                        arguments: serde_json::json!({"path": "test.png"}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    model: "test".into(),
                    provider: "test".into(),
                    usage: Usage::default(),
                    timestamp: 0,
                    error_message: None,
                },
                Message::ToolResult {
                    tool_call_id: "tc-1".into(),
                    tool_name: "read_file".into(),
                    content: vec![
                        Content::Text {
                            text: "screenshot".into(),
                        },
                        Content::Image {
                            data: "aW1hZ2VkYXRh".into(),
                            mime_type: "image/png".into(),
                        },
                    ],
                    is_error: false,
                    timestamp: 0,
                },
            ],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            api_key: "test-key".into(),
            max_tokens: Some(1024),
            temperature: None,
            model_config: None,
            cache_config: CacheConfig {
                enabled: false,
                strategy: CacheStrategy::Disabled,
            },
        };

        let body = build_request_body(&config, false);
        let msgs = body["messages"].as_array().unwrap();
        // The ToolResult message (second message)
        let tool_msg = &msgs[1];
        let tool_result = &tool_msg["content"][0];
        assert_eq!(tool_result["type"], "tool_result");
        // content should be an array (not a string) since it has images
        let content = tool_result["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[1]["type"], "image");
        assert_eq!(content[1]["source"]["media_type"], "image/png");
    }

    #[test]
    fn test_tool_result_text_only_uses_string() {
        let config = StreamConfig {
            model: "claude-sonnet-4-20250514".into(),
            system_prompt: "".into(),
            messages: vec![
                Message::Assistant {
                    content: vec![Content::ToolCall {
                        id: "tc-1".into(),
                        name: "bash".into(),
                        arguments: serde_json::json!({"command": "echo hi"}),
                    }],
                    stop_reason: StopReason::ToolUse,
                    model: "test".into(),
                    provider: "test".into(),
                    usage: Usage::default(),
                    timestamp: 0,
                    error_message: None,
                },
                Message::ToolResult {
                    tool_call_id: "tc-1".into(),
                    tool_name: "bash".into(),
                    content: vec![Content::Text {
                        text: "hello".into(),
                    }],
                    is_error: false,
                    timestamp: 0,
                },
            ],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            api_key: "test-key".into(),
            max_tokens: Some(1024),
            temperature: None,
            model_config: None,
            cache_config: CacheConfig {
                enabled: false,
                strategy: CacheStrategy::Disabled,
            },
        };

        let body = build_request_body(&config, false);
        let msgs = body["messages"].as_array().unwrap();
        let tool_result = &msgs[1]["content"][0];
        // Text-only: content should be a plain string
        assert_eq!(tool_result["content"], "hello");
    }
}
