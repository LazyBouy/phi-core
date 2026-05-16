//! LLM streaming — the core assistant response function.
//!
//! Extracted from `agent_loop.rs`. Contains [`stream_assistant_response`], which
//! prepares the payload, calls the provider in a retry loop, and re-emits SSE
//! events as [`AgentEvent`]s for the caller.

use super::config::*;
use super::helpers::default_convert_to_llm;
use crate::provider::{ProviderError, ProviderRegistry, StreamConfig, StreamEvent, StreamProvider};
use crate::types::*;
use tokio::sync::mpsc;
use tracing::warn;

/*
stream_assistant_response — the core LLM call.

This function does three things:
  1. Prepares the payload (context transform → LLM message conversion → tool definitions)
  2. Calls provider.stream() in a retry loop for transient failures
  3. Drains the event channel and re-emits events as AgentEvents for the UI

ARCHITECTURE NOTE: Dual-output design of provider.stream()

provider.stream() has an unusual dual-output pattern:
  - It takes a `stream_tx: mpsc::UnboundedSender<StreamEvent>` (push-based, fires during streaming)
  - It returns `Result<Message, ProviderError>` (pull-based, available after await completes)

Why both? Because SSE streaming and HTTP completion are sequential:
  a) SSE events arrive token-by-token (we push them into stream_tx for the UI)
  b) The final complete Message is only available when the stream ends (returned as Result)

The UI reads from stream_rx (the receiving end of the channel) while the provider
pushes into stream_tx. This decouples the UI rendering from the HTTP layer.

*/
/// Stream an assistant response from the LLM.
pub(super) async fn stream_assistant_response(
    context: &AgentContext, // READ-ONLY — converts messages for LLM but never mutates context
    config: &AgentLoopConfig, // SETTINGS — model, system prompt, cache; used to build StreamConfig
    tx: &mpsc::UnboundedSender<AgentEvent>, // OBSERVER — re-emits StreamEvents as AgentEvents for the caller
    cancel: &tokio_util::sync::CancellationToken, // ABORT — forwarded to provider.stream(); cloned as provider_cancel
    loop_id: &str,
) -> Message {
    // complete LLM response (all content blocks assembled); synthetic error Message on failure
    // Build working context: if prun streams are populated, merge them; otherwise use messages as-is.
    let base_messages = context.build_working_context();

    // Apply context transform (optional hook to prune/reshape messages before LLM sees them)
    let messages = if let Some(transform) = &config.transform_context {
        transform(base_messages)
    } else {
        base_messages
    };

    // Convert AgentMessage[] → Message[]: strip Extension messages, keep only LLM-visible ones.
    // This is the "context filter" — Extension messages are UI-only and must never enter the prompt.
    let convert = config.convert_to_llm.as_ref();
    let llm_messages = match convert {
        Some(f) => f(&messages),
        None => default_convert_to_llm(&messages), // default: keep only Llm(Message) variants
    };

    // Build tool definitions — the JSON Schema descriptions the LLM uses to decide which tool to call.
    // `.iter().map(...).collect()` is the idiomatic Rust "transform a collection" pattern.
    // Python analogy: [ToolDefinition(name=t.name(), ...) for t in context.tools]
    let tool_defs: Vec<crate::provider::ToolDefinition> = context
        .tools
        .iter()
        .map(|t| crate::provider::ToolDefinition {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: t.parameters_schema(),
        })
        .collect();

    /*
    RETRY LOOP — loop { ... break value } returning a value

    RUST QUIRK: `loop` can return a value via `break expr`.
    This is unique to Rust — loops are expressions, not just statements.

      let result = loop {
          if condition { break some_value; }  // ← breaks out AND returns some_value
          // otherwise keep looping
      };

    Here we break with a tuple `(result, stream_rx)` — Rust allows breaking with
    any expression, including tuples and structs. The destructuring on the left
    `let (result, mut stream_rx) = loop { ... };` unpacks it immediately.

    MATCH GUARD: `Err(e) if e.is_retryable() && ...`
    The `if` after a match pattern is a "match guard" — an extra condition that must
    be true for that arm to fire. Without it, all Err variants would match the arm.
    Python analogy:
      if isinstance(result, Err) and result.is_retryable() and attempt < max:
          ...
    */
    // Resolve provider: use override if set, else dispatch via registry.
    // ProviderRegistry is built inline — all 7 built-in providers are ZSTs, so this is near-zero cost.
    let registry = ProviderRegistry::default();
    let provider: &dyn StreamProvider = match config.provider_override.as_deref() {
        Some(p) => p,
        None => match registry.get(&config.model_config.api) {
            Some(p) => p,
            None => {
                return Message::Assistant {
                    content: vec![Content::Text {
                        text: String::new(),
                    }],
                    stop_reason: StopReason::Error,
                    model: config.model_config.id.clone(),
                    provider: String::new(),
                    usage: Usage::default(),
                    timestamp: now_ms(),
                    error_message: Some(format!(
                        "No provider registered for protocol: {}",
                        config.model_config.api
                    )),
                };
            }
        },
    };

    let retry = &config.retry_config;
    let mut attempt = 0;
    // Track whether we have already attempted a credential refresh in response to an
    // Auth error. Per the MEDIUM-4 spec, we refresh + retry exactly once per
    // `stream_assistant_response` call before propagating the Auth failure.
    let mut auth_refreshed = false;
    let (result, mut stream_rx) = loop {
        let stream_config = StreamConfig {
            model_config: config.model_config.clone(),
            system_prompt: context.system_prompt.clone(),
            messages: llm_messages.clone(),
            tools: tool_defs.clone(),
            thinking_level: config.thinking_level,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            cache_config: config.cache_config.clone(),
            response_format: config.response_format.clone(),
        };

        // Create a fresh channel per attempt — previous stream_rx is dropped when loop continues.
        // stream_tx is given to the provider; stream_rx stays here for event draining below.
        let (stream_tx, stream_rx) = mpsc::unbounded_channel();
        let provider_cancel = cancel.clone();

        let result = provider
            .stream(stream_config, stream_tx, provider_cancel)
            .await; // .await suspends here until the SSE stream completes

        match &result {
            // Match guard: only retry if retryable, under the limit, and not cancelled
            Err(e) if e.is_retryable() && attempt < retry.max_retries && !cancel.is_cancelled() => {
                attempt += 1;
                // Use the provider's Retry-After header if present, else use exponential backoff
                let delay = e
                    .retry_after()
                    .unwrap_or_else(|| retry.delay_for_attempt(attempt));
                // unwrap_or_else takes a CLOSURE (lazy evaluation) — the delay is only computed
                // if retry_after() returns None. Saves computing an unused value.
                crate::provider::retry::log_retry(attempt, retry.max_retries, &delay, e);
                tokio::time::sleep(delay).await;
                continue; // jump back to top of loop
            }
            // Auth error with a CredentialProvider attached: invalidate the cached
            // credential and retry exactly once. If the second attempt also fails Auth,
            // we propagate (auth_refreshed gates the recursion).
            Err(ProviderError::Auth(_))
                if config.model_config.credentials.is_some()
                    && !auth_refreshed
                    && !cancel.is_cancelled() =>
            {
                auth_refreshed = true;
                tracing::warn!(
                    "Provider returned Auth error; refreshing credentials and retrying once."
                );
                // Best-effort: if invalidate itself errors, propagate the original Auth
                // (the new error from invalidate would be misleading).
                if let Err(e) = config.model_config.invalidate_credentials().await {
                    tracing::warn!("CredentialProvider::invalidate failed: {}", e);
                }
                continue;
            }
            _ => break (result, stream_rx), // success or non-retryable error — exit loop with tuple
        }
    };

    /*
    Drain the event channel and re-emit as AgentEvents.

    stream_rx is a tokio mpsc receiver. The provider sent StreamEvents into stream_tx
    during the `.await` above. Now we drain them all with `try_recv()`:

    RUST QUIRK: `while let Ok(event) = stream_rx.try_recv()`
    `try_recv()` returns:
      Ok(event)  — got an event
      Err(_)     — channel empty OR closed
    `while let Ok(event) = ...` loops as long as we get Ok values. When empty → stops.
    This is non-blocking: it drains all buffered events synchronously.

    `.ok()` on `tx.send(...)`:
    `tx.send()` returns Result<(), SendError> — it fails only if the receiver is dropped.
    `.ok()` converts the Result to Option and silently discards the error.
    Pattern: "fire-and-forget" — we don't care if the subscriber dropped.
    */
    let mut partial_message: Option<AgentMessage> = None;
    while let Ok(event) = stream_rx.try_recv() {
        match &event {
            StreamEvent::Start => {
                // Create a placeholder so deltas have a message to attach to.
                // It will be replaced by the real message on Done.
                let placeholder = AgentMessage::Llm(LlmMessage::new(Message::Assistant {
                    content: Vec::new(),
                    stop_reason: StopReason::Stop,
                    model: config.model_config.id.clone(),
                    provider: String::new(),
                    usage: Usage::default(),
                    timestamp: now_ms(),
                    error_message: None,
                }));
                partial_message = Some(placeholder.clone());
                tx.send(AgentEvent::MessageStart {
                    loop_id: loop_id.to_string(),
                    message: placeholder,
                })
                .ok(); // .ok() = discard Result — receiver being dropped is non-fatal
            }
            StreamEvent::TextDelta { delta, .. } => {
                // `if let Some(ref msg) = partial_message` — borrow the inner value without moving.
                // `ref msg` means: bind msg as &AgentMessage (a reference), not as AgentMessage (moved).
                // Without `ref`, the match would try to MOVE partial_message out, leaving it unusable.
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        loop_id: loop_id.to_string(),
                        message: msg.clone(),
                        delta: StreamDelta::Text {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::ThinkingDelta { delta, .. } => {
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        loop_id: loop_id.to_string(),
                        message: msg.clone(),
                        delta: StreamDelta::Thinking {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::ToolCallDelta { delta, .. } => {
                if let Some(ref msg) = partial_message {
                    tx.send(AgentEvent::MessageUpdate {
                        loop_id: loop_id.to_string(),
                        message: msg.clone(),
                        delta: StreamDelta::ToolCallDelta {
                            delta: delta.clone(),
                        },
                    })
                    .ok();
                }
            }
            StreamEvent::Done { message } => {
                // message.clone().into() — uses the `From<Message> for AgentMessage` impl
                // defined in types.rs to wrap the Message in AgentMessage::Llm(LlmMessage::new(..)) automatically.
                let am: AgentMessage = message.clone().into();
                partial_message = Some(am.clone());
                // MessageStart was already emitted on StreamEvent::Start
                tx.send(AgentEvent::MessageEnd {
                    loop_id: loop_id.to_string(),
                    message: am,
                })
                .ok();
            }
            StreamEvent::Error { message } => {
                let am: AgentMessage = message.clone().into();
                // Only emit MessageStart if Start wasn't received
                // (error before stream opened → no Start event was sent)
                if partial_message.is_none() {
                    tx.send(AgentEvent::MessageStart {
                        loop_id: loop_id.to_string(),
                        message: am.clone(),
                    })
                    .ok();
                }
                partial_message = Some(am.clone());
                tx.send(AgentEvent::MessageEnd {
                    loop_id: loop_id.to_string(),
                    message: am,
                })
                .ok();
            }
            _ => {} // catch-all: ignore any future StreamEvent variants we don't handle here
        }
    }

    // Return the final result: the complete Message from the provider (or a synthetic error Message)
    match result {
        Ok(msg) => msg,
        Err(e) => {
            // Non-retryable error or retries exhausted. Build a synthetic error Message so the
            // agent loop can record it and fire on_error callbacks. We never panic — errors are
            // part of the protocol, not exceptional conditions.
            warn!("Provider error: {}", e);
            Message::Assistant {
                content: vec![Content::Text {
                    text: String::new(), // empty — the error lives in error_message
                }],
                stop_reason: StopReason::Error,
                model: config.model_config.id.clone(),
                provider: "unknown".into(), // .into() converts &str → String
                usage: Usage::default(),
                timestamp: now_ms(),
                error_message: Some(e.to_string()), // Display trait → String via to_string()
            }
        }
    }
}
