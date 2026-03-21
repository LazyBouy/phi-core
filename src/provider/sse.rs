//! Shared SSE (Server-Sent Events) parsing utilities.
//!
//! Factored out from the Anthropic provider so all providers can reuse
//! the same streaming infrastructure.
/*
ARCHITECTURE: sse.rs — the shared HTTP streaming layer

Server-Sent Events (SSE) is the wire protocol used by all major LLM providers
for streaming. It's an HTTP response with Content-Type: text/event-stream where
the server pushes newline-delimited events:

  event: content_block_delta
  data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}

  event: message_stop
  data: {"type":"message_stop"}

`drive_sse()` handles the generic SSE layer (HTTP chunking, event framing,
reconnect, cancellation) and forwards parsed `SseEvent` structs to a channel.
Each provider then reads from that channel and interprets the `data` JSON
according to its own format (Anthropic vs OpenAI vs Google differ in event names
and JSON shapes).

This separation of concerns:
  - `sse.rs`           → knows about SSE wire format (events, data, open, close)
  - `anthropic.rs`     → knows about Anthropic's specific JSON event schema
  - `openai_compat.rs` → knows about OpenAI's specific JSON event schema
  - ...

RUST QUIRK: `reqwest_eventsource` crate — async SSE over HTTP

`reqwest` is Rust's most popular async HTTP client (like Python's `httpx`).
`reqwest_eventsource` extends it with SSE stream handling: it reconnects on
dropped connections, handles SSE framing (multi-line fields, comment lines, etc.),
and presents a clean `Stream` of `Event` items.

`EventSource` implements `futures::Stream<Item = Result<Event, ...>>` — a lazy,
async sequence of values. Think of it as an async generator in Python.
*/

use futures::StreamExt;
use reqwest_eventsource::{Event, EventSource};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::debug;

/// A parsed SSE event with event type and data.
/*
RUST QUIRK: `#[derive(Debug, Clone)]` on a simple named-field struct
  Both fields are `String` (heap-allocated, owned). `Clone` deep-copies both strings.
  `Debug` enables `{:?}` formatting for logging/debugging.
  No `Copy` here because `String` is not `Copy` (it owns heap memory).
*/
#[derive(Debug, Clone)]
pub struct SseEvent {
    /// The SSE event type (e.g. "content_block_delta", "message_stop").
    pub event: String,
    /// The raw JSON data string for this event.
    pub data: String,
}

/// Drives an EventSource, sending parsed events through a channel.
/// Returns when the stream ends, errors, or is cancelled.
///
/// The caller receives `SseEvent`s and can parse them according to
/// provider-specific formats.
/*
ARCHITECTURE: drive_sse — the event pump

`drive_sse()` is a "pump" that pulls events from the HTTP response stream and
pushes them into an mpsc channel. It runs as a concurrent task alongside the
provider's event-processing logic.

Why async? The HTTP response arrives incrementally. If we waited synchronously
for the entire response, we'd have no streaming at all. `async` lets the tokio
runtime interleave the HTTP reads with other work.

Return value: `Result<(), String>`
  `Ok(())`  — stream ended cleanly (HTTP connection closed by server)
  `Err(s)`  — either cancelled ("cancelled") or an HTTP/network error

RUST QUIRK: `tokio::select!` — race multiple async operations

`tokio::select!` runs multiple futures concurrently and returns when the FIRST
one completes. All other futures are DROPPED (cancelled).

Here we race two operations:
  1. `cancel.cancelled()` — waits until the cancellation token is triggered
  2. `es.next()`          — waits for the next SSE event from the HTTP stream

If cancellation fires first: we close the SSE connection and return Err("cancelled").
If a new event arrives first: we process it and loop.

Python analogy:
  done, pending = asyncio.wait(
      {cancel_task, es_next_task},
      return_when=asyncio.FIRST_COMPLETED
  )

The `_` in `_ = cancel.cancelled()` means "I don't care about the return value
of this future — I only care that it completed."

RUST QUIRK: `loop { ... }` with `return` — Rust's infinite loop + early exits

`loop` creates an infinite loop (no condition). The only exits are:
  - `return Ok(())`  when stream ends
  - `return Err(...)` on cancellation or error
  - `break` (not used here)

Unlike `while true` in Python, `loop` in Rust communicates intent: "this loop
runs until an explicit exit, not until some condition becomes false."

RUST QUIRK: `es.next()` on a Stream — `futures::StreamExt::next()`
  `EventSource` implements `futures::Stream`. The `StreamExt` trait (from the
  `futures` crate) extends Stream with ergonomic methods:
    `.next()` → future that resolves to `Option<Item>`:
      `Some(Ok(event))` — got an event
      `Some(Err(e))`    — got an error
      `None`            — stream ended

RUST QUIRK: `tx.send(...).is_err()` — checking if the receiver is gone
  `mpsc::UnboundedSender::send()` returns `Err(value)` if the receiver has been
  dropped. When the receiver (the provider's event-processing task) is gone,
  there's no point continuing — we close the SSE connection and return Ok.
  `.is_err()` returns `true` if the Result is an `Err` variant.
  Python analogy: catching a BrokenPipeError when writing to a closed queue.

RUST QUIRK: `e.to_string()` on an error type
  Most error types implement `Display` (the `{}` formatter). `.to_string()`
  calls `Display` to get a human-readable description. Equivalent to `str(e)` in Python.
  Here we convert the `reqwest_eventsource` error to a plain `String` for our
  simpler `Result<(), String>` return type.
*/
/*
DESIGN: Why `drive_sse` takes `es` by value AND uses a channel `tx`

Same dual-audience pattern as `execute_single_tool`:
  `es`     = HTTP SOURCE — drive_sse owns the connection; it is the only place that calls
             `.close()` and `.next()`. Passing by value (not `&mut`) means this function
             is exclusively responsible for the connection's lifecycle.
  `tx`     = OBSERVER CHANNEL — provider tasks await on the rx end to process events.
             Separation keeps HTTP pumping out of provider-specific JSON parsing logic.
  `cancel` = ABORT — if triggered mid-stream, `.close()` shuts down the HTTP connection
             and the function returns so the caller can clean up.

Why run drive_sse as a concurrent task?
  Providers spawn drive_sse as a tokio task and await the rx channel separately.
  This separates "reading bytes off the wire" from "interpreting the JSON events".
  If interpretation is slow, the HTTP buffer drains independently.
*/
pub async fn drive_sse(
    mut es: EventSource, // HTTP SOURCE — owns the open SSE connection; .close() shuts it down
    tx: mpsc::UnboundedSender<SseEvent>, // OBSERVER CHANNEL — forward parsed events to the provider's processing task
    cancel: CancellationToken, // ABORT — if triggered, closes connection and returns Err("cancelled")
) -> Result<(), String> {
    loop {
        tokio::select! {
            // Branch 1: cancellation requested — close SSE and abort
            _ = cancel.cancelled() => {
                es.close(); // signal the HTTP connection to close
                return Err("cancelled".into());
            }
            // Branch 2: next SSE event arrived (or stream ended/errored)
            event = es.next() => {
                match event {
                    // Stream ended cleanly (server closed the connection)
                    None => return Ok(()),
                    // Connection opened — just log it, no action needed
                    Some(Ok(Event::Open)) => {
                        debug!("SSE connection opened");
                    }
                    // A real event with data — forward to the provider's channel
                    Some(Ok(Event::Message(msg))) => {
                        if tx.send(SseEvent {
                            event: msg.event, // e.g. "content_block_delta"
                            data: msg.data,   // the raw JSON payload
                        }).is_err() {
                            // Receiver dropped — no one is listening, clean up
                            es.close();
                            return Ok(());
                        }
                    }
                    // HTTP/SSE error (network issue, bad status code, etc.)
                    Some(Err(e)) => {
                        es.close();
                        return Err(e.to_string());
                    }
                }
            }
        }
    }
}
