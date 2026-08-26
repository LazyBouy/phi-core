//! Mock provider for testing. No real API calls.
/*
ARCHITECTURE: MockProvider — test double for StreamProvider

In tests, we don't want to make real HTTP calls to Anthropic or OpenAI.
`MockProvider` is a "test double" (specifically a "stub"): it has the same
interface as a real provider but returns pre-scripted responses.

Usage pattern in tests (default one-shot):
  let provider = MockProvider::texts(vec!["Hello", "World"]);
  // first agent loop call → "Hello"
  // second agent loop call → "World"
  // third call → "(no more mock responses)" fallback

OPT-IN REPEAT/CYCLE MODE (`with_repeat` / `repeating`):
  let provider = MockProvider::repeating(vec![tool_call, terminal_text]);
  // the queue cycles VERBATIM instead of exhausting: each consumed response is
  // re-queued to the back, so a single scripted session re-emits across turns.
  // `new`/`text`/`texts` keep `repeat` OFF → byte-behaviour-identical one-shot.

CONSUMER CADENCE CONTRACT (read before enabling repeat):
  The kernel cycles the supplied `Vec<MockResponse>` VERBATIM — it never reorders
  the queue and never synthesizes a terminal response. So a one-tool-call-per-turn
  cadence is the CONSUMER's responsibility: the repeating unit must be exactly
  `[ToolCalls, Text]` (tool call FIRST, terminal non-tool `Text` SECOND), because
  one interactive turn consumes exactly two queue items (the tool round + a
  terminal round that returns a non-tool text to end the turn).
    - `[ToolCalls, Text]` → one tool call re-emitted every turn (the target cadence).
    - A LEADING text (`[Text, ToolCalls]`) makes the first turn a dud, then self-corrects.
    - A `[ToolCalls]`-ONLY repeating queue is an UNBOUNDED tool loop: under repeat the
      empty→`"(no more mock responses)"` safety-net is bypassed, so no terminal round
      ever ends the turn — the loop is halted only by `max_turns`. Always include a
      terminal `Text` in the repeating unit.

RUST QUIRK: `std::sync::Mutex<Vec<MockResponse>>` — interior mutability for shared state

`MockProvider` must implement `StreamProvider` which requires `Sync` (shareable
between threads). But `stream()` takes `&self` (shared reference) and needs to
MUTATE the response queue (remove the next response).

The problem: `&self` is a shared reference — by default it's read-only.
Solution: wrap the queue in `Mutex<T>`, which provides "interior mutability":
  - `Mutex::lock()` gives an exclusive `MutexGuard<T>` — exclusive borrow at runtime
  - Other threads must wait for the lock before accessing the queue
  - This satisfies `Sync` because all accesses are serialized through the Mutex

Python analogy: `threading.Lock()` protecting a shared list.

Why `std::sync::Mutex` (not `tokio::sync::Mutex`)?
  `std::sync::Mutex` is a blocking mutex — it uses the OS thread scheduler.
  `tokio::sync::Mutex` is an async-aware mutex — it yields the tokio task instead.
  Here we lock only briefly (just to pop from the Vec), so blocking is fine.
  If we held the lock across an `await` point, we'd need `tokio::sync::Mutex`.
*/

use super::traits::*;
use crate::types::*;
use async_trait::async_trait;
use tokio::sync::mpsc;

/// A mock response: either plain text or a set of tool calls.
/*
RUST QUIRK: Tuple variant vs struct variant
  `MockResponse::Text(String)` — tuple variant with one unnamed field
  `MockResponse::ToolCalls(Vec<MockToolCall>)` — tuple variant with one unnamed field
  Both hold their value by ownership (the String / Vec is moved into the enum).

  Access the inner value with pattern matching:
    `match response { MockResponse::Text(text) => ... }`
  Python analogy: tagged unions / sum types via dataclasses.
*/
#[derive(Debug, Clone)]
pub enum MockResponse {
    /// The LLM replies with this text string.
    Text(String),
    /// The LLM calls these tools (arguments are pre-specified, not generated).
    ToolCalls(Vec<MockToolCall>),
}

/// A single mock tool call (pre-scripted name + arguments).
#[derive(Debug, Clone)]
pub struct MockToolCall {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Mock LLM provider for tests. Supply a sequence of responses.
pub struct MockProvider {
    /// Queue of responses to return, in order. Protected by a Mutex for interior mutability.
    responses: std::sync::Mutex<Vec<MockResponse>>,
    /// Opt-in repeat/cycle mode. When `false` (the default via `new`/`text`/`texts`) the
    /// queue is consumed destructively and exhausts to the `"(no more mock responses)"`
    /// fallback. When `true` (via `with_repeat`/`repeating`) each consumed response is
    /// re-queued to the back, so the flat `Vec<MockResponse>` cycles verbatim and never
    /// exhausts while non-empty. See the module docs for the CONSUMER cadence contract.
    repeat: bool,
}

impl MockProvider {
    /// Create a provider from a sequence of responses (one-shot; exhausts to the fallback).
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            repeat: false,
        }
    }

    /// Create a provider from a sequence of responses, optionally cycling the queue.
    ///
    /// With `repeat == false` this is byte-behaviour-identical to [`MockProvider::new`]
    /// (one-shot destructive consume → `"(no more mock responses)"` fallback once the
    /// queue empties). With `repeat == true` the queue cycles verbatim: each consumed
    /// response is re-queued to the back, so a single scripted session can re-emit its
    /// responses across many turns without ever hitting the fallback.
    ///
    /// The queue is cycled VERBATIM (order-agnostic; the kernel never reorders it), so a
    /// correct one-tool-call-per-turn cadence is the CONSUMER's responsibility — the
    /// repeating unit must be exactly `[ToolCalls, Text]` (tool call FIRST, terminal
    /// non-tool `Text` SECOND). A `[ToolCalls]`-only repeating queue is an UNBOUNDED tool
    /// loop the kernel will NOT auto-terminate (see the module-level CONSUMER contract).
    pub fn with_repeat(responses: Vec<MockResponse>, repeat: bool) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            repeat,
        }
    }

    /// Convenience: a repeating provider whose queue cycles verbatim (= `with_repeat(responses, true)`).
    ///
    /// Mirrors the `text`/`texts` convenience precedent. The CONSUMER cadence contract of
    /// [`MockProvider::with_repeat`] applies — supply `[ToolCalls, Text]` with a terminal
    /// text to sustain a one-tool-call-per-turn session.
    pub fn repeating(responses: Vec<MockResponse>) -> Self {
        Self::with_repeat(responses, true)
    }

    /// Convenience: provider that always returns the same text.
    pub fn text(text: impl Into<String>) -> Self {
        Self::new(vec![MockResponse::Text(text.into())])
    }

    /// Convenience: provider that returns a sequence of text responses, one per call.
    pub fn texts(texts: Vec<impl Into<String>>) -> Self {
        /*
        RUST QUIRK: `.into_iter().map(...).collect()` — consuming iterator chain

        `texts.into_iter()` — MOVES the Vec into an iterator (transfers ownership)
        `.map(|t| MockResponse::Text(t.into()))` — transforms each item
          `t.into()` converts `impl Into<String>` → `String` (calls `Into::into()`)
          Then wraps in `MockResponse::Text(...)`
        `.collect()` — gathers into a `Vec<MockResponse>`
        The return type is inferred from `Self::new(responses: Vec<MockResponse>)`.
        */
        Self::new(
            texts
                .into_iter()
                .map(|t| MockResponse::Text(t.into()))
                .collect(),
        )
    }
}

#[async_trait]
impl StreamProvider for MockProvider {
    fn provider_id(&self) -> &str {
        "mock"
    }

    async fn stream(
        &self,
        config: StreamConfig, // responses are pre-set; `config` is used only for opt-in raw-wire capture
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — receives synthetic events built from the next MockResponse
        cancel: tokio_util::sync::CancellationToken, // ABORT — honored (returns Cancelled if triggered before events are sent)
    ) -> Result<Message, ProviderError> {
        // Opt-in raw-wire capture. The mock has no real HTTP body; emit a synthetic,
        // credential-free request summary so deterministic no-network sink tests can
        // exercise the full Request → ResponseFrame(s) → ResponseDone sequence.
        let wire_sink = config.provider_wire_sink.clone();
        let provider_id = self.provider_id().to_string();
        let model_id = config.model_config.id.clone();
        let mut frame_index: usize = 0;
        if let Some(sink) = wire_sink.as_ref() {
            sink.on_wire(&RawWire::Request {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                body: format!("{{\"mock\":true,\"messages\":{}}}", config.messages.len()),
            });
        }
        /*
        RUST QUIRK: `{ let mut guard = self.responses.lock().unwrap(); ... }`
        The block `{ ... }` creates a scope. The `MutexGuard` (returned by `.lock()`)
        is dropped when the block ends — releasing the lock.

        `.lock()` returns `Result<MutexGuard, PoisonError>`. A Mutex is "poisoned"
        if another thread panicked while holding the lock. `.unwrap()` propagates
        the panic in that unlikely scenario.

        `.remove(0)` removes and returns the first element, shifting everything else
        left. O(n) but fine for small test queues.
        This is the standard Mutex pattern: lock briefly, extract data, drop lock.
        Python analogy: `with lock: response = queue.pop(0)`
        */
        let response = {
            let mut responses = self.responses.lock().unwrap(); // acquire lock
            if responses.is_empty() {
                // Fallback: tests that run more turns than responses get a safe default
                MockResponse::Text("(no more mock responses)".into())
            } else {
                let front = responses.remove(0); // pop the front response
                if self.repeat {
                    // Repeat/cycle mode: re-queue the popped response to the back so the
                    // flat queue cycles verbatim (never exhausts while non-empty). The
                    // `repeat == false` path above is byte-identical to `responses.remove(0)`.
                    responses.push(front.clone());
                }
                front
            }
            // MutexGuard dropped here — lock released
        };

        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        let _ = tx.send(StreamEvent::Start);
        if let Some(sink) = wire_sink.as_ref() {
            sink.on_wire(&RawWire::ResponseFrame {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                frame_index,
                raw_frame: "event: start".to_string(),
            });
            frame_index += 1;
        }

        /*
        RUST QUIRK: `match response { ... }` — consuming a moved value

        `response` was moved out of the Mutex (via `.remove(0)`).
        This `match` consumes it: each arm can move fields out of the variant.
        After the match, `response` is gone. The `message` local is the result.
        */
        let message = match response {
            MockResponse::Text(text) => {
                // Emit a single TextDelta for the full text (simplified vs real streaming)
                let _ = tx.send(StreamEvent::TextDelta {
                    content_index: 0,
                    delta: text.clone(),
                });
                if let Some(sink) = wire_sink.as_ref() {
                    sink.on_wire(&RawWire::ResponseFrame {
                        provider_id: provider_id.clone(),
                        model_id: model_id.clone(),
                        frame_index,
                        raw_frame: format!("data: {{\"text\":{:?}}}", text),
                    });
                    frame_index += 1;
                }
                Message::Assistant {
                    content: vec![Content::Text { text }],
                    stop_reason: StopReason::Stop,
                    model: "mock".into(),
                    provider: "mock".into(),
                    usage: Usage::default(),
                    timestamp: now_ms(),
                    error_message: None,
                }
            }
            MockResponse::ToolCalls(calls) => {
                /*
                RUST QUIRK: `.enumerate()` — pairing each element with its index

                `.iter().enumerate()` transforms `Iterator<Item=T>` →
                `Iterator<Item=(usize, &T)>`, providing the index alongside each item.
                Here: `(i, call)` where `i` is 0, 1, 2, ... and `call` is `&MockToolCall`.
                Python analogy: `for i, call in enumerate(calls):`
                */
                let content: Vec<Content> = calls
                    .iter()
                    .enumerate()
                    .map(|(i, call)| {
                        let id = format!("mock-tool-{}", i);
                        // Notify the channel that a tool call started and immediately ended
                        // (mock: no streaming of arguments — they're fully known upfront)
                        let _ = tx.send(StreamEvent::ToolCallStart {
                            content_index: i,
                            id: id.clone(),
                            name: call.name.clone(),
                        });
                        let _ = tx.send(StreamEvent::ToolCallEnd { content_index: i });
                        Content::ToolCall {
                            id,
                            name: call.name.clone(),
                            arguments: call.arguments.clone(),
                        }
                    })
                    .collect();

                Message::Assistant {
                    content,
                    stop_reason: StopReason::ToolUse,
                    model: "mock".into(),
                    provider: "mock".into(),
                    usage: Usage::default(),
                    timestamp: now_ms(),
                    error_message: None,
                }
            }
        };

        // Signal stream completion — both on the channel and as the return value
        if let Some(sink) = wire_sink.as_ref() {
            let summary = match &message {
                Message::Assistant {
                    content,
                    stop_reason,
                    ..
                } => format!(
                    "{:?} ({} block(s), {} frame(s))",
                    stop_reason,
                    content.len(),
                    frame_index
                ),
                _ => format!("non-assistant ({} frame(s))", frame_index),
            };
            sink.on_wire(&RawWire::ResponseDone {
                provider_id: provider_id.clone(),
                model_id: model_id.clone(),
                message_summary: summary,
            });
        }
        let _ = tx.send(StreamEvent::Done {
            message: message.clone(),
        });
        Ok(message)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ModelConfig;
    use tokio_util::sync::CancellationToken;

    fn cfg() -> StreamConfig {
        StreamConfig {
            model_config: ModelConfig::anthropic("mock", "mock", "test"),
            system_prompt: String::new(),
            messages: vec![Message::user("hi")],
            tools: vec![],
            thinking_level: ThinkingLevel::Off,
            max_tokens: None,
            temperature: None,
            cache_config: CacheConfig::default(),
            response_format: ResponseFormat::Text,
            provider_wire_sink: None,
        }
    }

    /// Drive `stream()` once and return the leading assistant text.
    async fn next_text(p: &MockProvider) -> String {
        let (tx, _rx) = mpsc::unbounded_channel();
        let msg = p
            .stream(cfg(), tx, CancellationToken::new())
            .await
            .expect("mock stream never errors without cancellation");
        match msg {
            Message::Assistant { content, .. } => match content.first() {
                Some(Content::Text { text }) => text.clone(),
                other => panic!("expected leading text content, got {other:?}"),
            },
            other => panic!("expected assistant message, got {other:?}"),
        }
    }

    /// Collect the assistant text of `n` successive `stream()` calls.
    async fn texts_of(p: &MockProvider, n: usize) -> Vec<String> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            out.push(next_text(p).await);
        }
        out
    }

    // Tier A — back-compat: `new()` consumes destructively then exhausts to the fallback
    // (byte-behaviour-identical to the pre-KC-06 provider; repeat defaults OFF).
    #[tokio::test]
    async fn mock_new_is_one_shot_and_exhausts() {
        let p = MockProvider::new(vec![
            MockResponse::Text("a".into()),
            MockResponse::Text("b".into()),
        ]);
        assert_eq!(
            texts_of(&p, 3).await,
            vec!["a", "b", "(no more mock responses)"],
        );
    }

    // Tier A — back-compat: `with_repeat(_, false)` is byte-behaviour-identical to `new()`.
    #[tokio::test]
    async fn mock_with_repeat_false_matches_new_one_shot() {
        let p = MockProvider::with_repeat(
            vec![
                MockResponse::Text("a".into()),
                MockResponse::Text("b".into()),
            ],
            false,
        );
        assert_eq!(
            texts_of(&p, 3).await,
            vec!["a", "b", "(no more mock responses)"],
        );
    }

    // Tier F — API equivalence: `repeating(r)` ≡ `with_repeat(r, true)` (cycle verbatim);
    // `with_repeat(r, false)` ≡ `new(r)` (one-shot then exhaust).
    #[tokio::test]
    async fn mock_repeat_api_equivalence() {
        // repeating(r) cycles verbatim, identical to with_repeat(r, true).
        let repeating = MockProvider::repeating(vec![MockResponse::Text("x".into())]);
        let with_true = MockProvider::with_repeat(vec![MockResponse::Text("x".into())], true);
        let rep_seq = texts_of(&repeating, 3).await;
        assert_eq!(rep_seq, texts_of(&with_true, 3).await);
        assert_eq!(rep_seq, vec!["x", "x", "x"]);

        // with_repeat(r, false) one-shots then exhausts, identical to new(r).
        let with_false = MockProvider::with_repeat(vec![MockResponse::Text("x".into())], false);
        let plain_new = MockProvider::new(vec![MockResponse::Text("x".into())]);
        let false_seq = texts_of(&with_false, 3).await;
        assert_eq!(false_seq, texts_of(&plain_new, 3).await);
        assert_eq!(
            false_seq,
            vec!["x", "(no more mock responses)", "(no more mock responses)"],
        );
    }
}
