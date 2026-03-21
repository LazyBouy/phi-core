//! Mock provider for testing. No real API calls.
/*
ARCHITECTURE: MockProvider — test double for StreamProvider

In tests, we don't want to make real HTTP calls to Anthropic or OpenAI.
`MockProvider` is a "test double" (specifically a "stub"): it has the same
interface as a real provider but returns pre-scripted responses.

Usage pattern in tests:
  let provider = MockProvider::texts(vec!["Hello", "World"]);
  // first agent loop call → "Hello"
  // second agent loop call → "World"
  // third call → "(no more mock responses)" fallback

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
}

impl MockProvider {
    /// Create a provider from a sequence of responses.
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
        }
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
    async fn stream(
        &self,
        _config: StreamConfig, // IGNORED — test double; real config not used (responses are pre-set)
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — receives synthetic events built from the next MockResponse
        cancel: tokio_util::sync::CancellationToken, // ABORT — honored (returns Cancelled if triggered before events are sent)
    ) -> Result<Message, ProviderError> {
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
                responses.remove(0) // pop the front response
            }
            // MutexGuard dropped here — lock released
        };

        if cancel.is_cancelled() {
            return Err(ProviderError::Cancelled);
        }

        let _ = tx.send(StreamEvent::Start);

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
        let _ = tx.send(StreamEvent::Done {
            message: message.clone(),
        });
        Ok(message)
    }
}
