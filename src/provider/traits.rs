use crate::types::*;
use async_trait::async_trait;
use tokio::sync::mpsc;

use super::model::ModelConfig;

/*
ARCHITECTURE: The Provider Layer

This module defines the core abstraction for ALL LLM providers:

  StreamProvider trait  — the interface every provider must implement
  StreamEvent enum      — the event protocol sent through the channel
  StreamConfig struct   — the input to every provider call
  ProviderError enum    — the error taxonomy

Why streaming via a channel instead of returning a Vec of events?
Because streaming gives real-time UI updates. The user sees tokens as they arrive,
not after the entire response. An mpsc channel is the natural async Rust primitive
for this producer-consumer split.

The dual-output pattern:
  provider.stream(config, tx, cancel) → Future<Result<Message, Error>>
                          ↑                     ↑
               sends StreamEvents        returns final Message
               in real-time            after stream completes

The channel carries partial deltas; the return value carries the complete message.
*/

/// Events emitted during LLM streaming.
/*
ARCHITECTURE: `content_index` in delta events

LLM responses can contain MULTIPLE content blocks in one message:
  [Thinking("..."), Text("Hello"), ToolCall({id: "x", name: "bash", args: {...}})]

`content_index` identifies WHICH block a delta belongs to.
Without it, interleaved deltas from parallel content blocks would be ambiguous.

Example for an extended-thinking response:
  ThinkingDelta { content_index: 0, delta: "Let me " }
  ThinkingDelta { content_index: 0, delta: "think..." }
  TextDelta     { content_index: 1, delta: "Here's " }
  TextDelta     { content_index: 1, delta: "my answer." }
  ToolCallStart { content_index: 2, id: "call_1", name: "bash" }
  ToolCallDelta { content_index: 2, delta: "{\"cmd\":" }
  ToolCallEnd   { content_index: 2 }
  Done          { message: (complete Message) }
*/
#[derive(Debug, Clone)]
pub enum StreamEvent {
    /// Stream started — the LLM has begun generating. Consumers should create a placeholder.
    Start,
    /// A text token from the response text.
    TextDelta { content_index: usize, delta: String },
    /// A chunk from the model's chain-of-thought (extended thinking mode only).
    ThinkingDelta { content_index: usize, delta: String },
    /// The LLM began a tool call — id and name are now known.
    ToolCallStart {
        content_index: usize,
        id: String,
        name: String,
    },
    /// A JSON fragment for a tool call's arguments (accumulate until ToolCallEnd).
    ToolCallDelta { content_index: usize, delta: String },
    /// The tool call's argument JSON is complete.
    ToolCallEnd { content_index: usize },
    /// Stream completed successfully. `message` is the final complete Message.
    Done { message: Message },
    /// Stream failed. `message` is a synthetic error Message with stop_reason=Error.
    Error { message: Message },
}

/// Configuration for a streaming LLM call
/*
ARCHITECTURE: StreamConfig — the "envelope" passed into every provider call

Every `StreamProvider::stream()` call receives exactly one `StreamConfig`.
It bundles everything the provider needs to make one API request:
  - model_config — the complete provider identity: id, api_key, base_url, compat flags
  - messages / system_prompt / tools — the conversation payload
  - thinking_level / max_tokens / temperature — per-call generation overrides
  - cache_config — whether to send prompt-caching headers

`model_config` is required (non-optional). Every provider reads at minimum
`model_config.id` (model name) and `model_config.api_key` (auth credential).
Providers with custom endpoints also read `model_config.base_url`, `model_config.headers`,
and (for OpenAI-compat) `model_config.compat`.

Why not pass individual arguments?
  If `stream()` took 10 positional parameters it would be unergonomic and break
  callers every time we added a field. A config struct is extensible: adding a
  field is backward-compatible if the caller can use `Default::default()` for it.
  Python analogy: kwargs dict passed to a function, or a dataclass payload.

RUST QUIRK: `Option<u32>` and `Option<f32>` — "nullable" fields
  Rust has no null. `Option<T>` is an explicit "maybe absent" wrapper:
    `None`    → caller didn't set a value; provider uses its own default
    `Some(v)` → caller explicitly overrides the value
  Python analogy: `max_tokens: int | None = None`
*/
#[derive(Debug, Clone)]
pub struct StreamConfig {
    /// Complete provider identity: model id, api_key, base_url, compat flags, cost rates.
    /// All providers read `model_config.id` and `model_config.api_key`; most also read
    /// `model_config.base_url` and `model_config.headers`.
    pub model_config: ModelConfig,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>, // overrides model_config.max_tokens when Some
    pub temperature: Option<f32>,
    /// Prompt caching configuration. Default: enabled with auto strategy.
    pub cache_config: CacheConfig,
}

/// Tool definition sent to the LLM (schema only, no execute fn)
/*
ARCHITECTURE: ToolDefinition — the schema half of a tool

Every tool has two sides:
  1. `AgentTool` (types.rs) — the Rust struct that EXECUTES the tool (has code)
  2. `ToolDefinition` (here)  — the JSON schema that gets SENT TO THE LLM

When we call `provider.stream(config, ...)`, only `ToolDefinition` goes to the API.
The LLM never sees executable code — it only sees name/description/parameters so it
can decide whether to call the tool and how to format the arguments.

The separation exists because:
  - The provider layer is pure I/O; it doesn't execute tools
  - ToolDefinition is serializable (goes over the wire); AgentTool is not
  - `agent_loop.rs` bridges them: it converts AgentTool → ToolDefinition before
    calling stream(), then receives ToolCall content and finds the matching AgentTool

RUST QUIRK: `serde_json::Value` — a dynamically typed JSON tree
  JSON doesn't map to a fixed Rust type. `serde_json::Value` is an enum that
  can hold any valid JSON structure:
    Value::Object(Map<String, Value>)
    Value::Array(Vec<Value>)
    Value::String(String)
    Value::Number(Number)  — wraps i64/u64/f64
    Value::Bool(bool)
    Value::Null

  Tool parameters are represented as a JSON Schema object — a dynamic shape
  that varies per tool — so `serde_json::Value` is the right type here.

RUST QUIRK: `#[derive(Serialize, Deserialize)]`
  Requires the `serde` + `serde_json` crates.
  `Serialize`   → can convert this struct TO JSON (for sending to APIs)
  `Deserialize` → can reconstruct this struct FROM JSON (for round-tripping)
  Python analogy: combining json.dumps() and json.loads() support automatically.
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema object describing the tool's parameters.
    /// LLMs use this schema to know what arguments to pass when calling the tool.
    pub parameters: serde_json::Value,
}

use serde::{Deserialize, Serialize};

/// The core provider trait. Implement this for each LLM backend.
/*
ARCHITECTURE: StreamProvider — the single extension point for ALL LLM backends

Every LLM backend (Anthropic, OpenAI, Google, Bedrock, Azure, ...) implements
this one trait. The rest of the codebase interacts only with `&dyn StreamProvider`
— it never knows which concrete backend is being used at runtime.

This is the "Strategy" pattern: swap the provider, keep everything else constant.

The dual-output contract:
  1. `tx` (mpsc channel) — sends StreamEvents in real time as they arrive
     Consumers subscribe to this channel to update the UI with partial tokens.
  2. Return value `Result<Message, ProviderError>` — the fully assembled Message
     Only available after the stream completes. Contains the complete response.

Why both? Because `Message` is only complete when the stream ends, but the UI
needs to show tokens as they arrive (low latency). The channel handles the
"streaming display" concern; the return value handles the "final record" concern.

RUST QUIRK: `Send + Sync` trait bounds — thread safety requirements
  `Send`  → values of this type can be transferred across thread boundaries
  `Sync`  → references (&T) can be shared across thread boundaries simultaneously

  Why required on StreamProvider?
    The provider is stored as `Arc<dyn StreamProvider>` and accessed from
    async tasks that may run on different OS threads in the tokio thread pool.
    Without `Send + Sync`, the compiler would reject this as unsafe.

  What do they PREVENT?
    `Rc<T>` is not `Send` (non-atomic reference count, unsafe to move between threads)
    `RefCell<T>` is not `Sync` (non-atomic borrow flag, unsafe to share between threads)
    The bounds ensure implementations can't accidentally use these.

RUST QUIRK: `#[async_trait]` — async methods in traits
  Rust's native trait system doesn't support `async fn` in traits (as of stable Rust)
  because `async fn` returns an anonymous `impl Future<Output=T>` — each
  implementation would return a DIFFERENT type, violating the uniform vtable layout
  required by `dyn Trait`.

  `#[async_trait]` is a procedural macro from the `async-trait` crate that desugars:
    async fn stream(&self, ...) -> Result<...>
  into:
    fn stream(&self, ...) -> Pin<Box<dyn Future<Output=Result<...>> + Send + '_>>

  The `Pin<Box<dyn Future...>>` is a heap-allocated, type-erased future — same type
  for every implementation, so the vtable works. The `Send` bound ensures the future
  itself is thread-safe (can be awaited on any tokio thread).
  Python analogy: an abstract async method that subclasses override.
*/
#[async_trait]
pub trait StreamProvider: Send + Sync {
    /// Short, stable identifier for this provider type.
    ///
    /// Used as the `provider_id` component of auto-derived `loop_id` signatures:
    ///   `loop_id = "{session_id}.{provider_id}.{model_slug}.{N}"`
    ///
    /// Return a lowercase ASCII string with no spaces (e.g. `"anthropic"`, `"openai"`, `"google"`).
    /// Custom providers should return a unique, stable string.
    fn provider_id(&self) -> &str;

    /// Stream a completion. Send events through `tx` in real time.
    /// Returns the final, fully-assembled assistant `Message` after the stream ends.
    ///
    /// Implementors must:
    /// - Send `StreamEvent::Start` when the stream begins
    /// - Send `StreamEvent::TextDelta` / `ThinkingDelta` / `ToolCall*` as tokens arrive
    /// - Send `StreamEvent::Done { message }` or `StreamEvent::Error { message }` at the end
    /// - Honor `cancel` — stop early and return `Err(ProviderError::Cancelled)`
    async fn stream(
        &self,
        config: StreamConfig, // ALL REQUEST PARAMS — model, messages, tools, auth (bundled to avoid 10-arg signature)
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — push StreamEvents here in real-time as tokens arrive
        cancel: tokio_util::sync::CancellationToken, // ABORT — check this; return Err(Cancelled) if triggered
    ) -> Result<Message, ProviderError>; // final fully-assembled Message (only available after stream ends)
}

/*
RUST QUIRK: `thiserror::Error` derive — auto-implementing `std::error::Error`

`std::error::Error` is the standard Rust error trait. Manually implementing it
requires also implementing `Display` and optionally `source()`. Boilerplate.

`thiserror` is a macro crate that generates all three from annotations:
  `#[error("API error: {0}")]` on a tuple variant:
    → Display impl: format!("API error: {}", self.0)
    → The {0} refers to the first (unnamed) field of the tuple variant.

  `#[error("Rate limited, retry after {retry_after_ms:?}ms")]` on a struct variant:
    → Display impl using the named field `retry_after_ms`
    → {:?} uses Debug formatting on the Option<u64> → "Some(60000)" or "None"

  `#[derive(thiserror::Error)]` also requires `#[derive(Debug)]` (already present).

Python analogy:
  class ProviderError(Exception):
      pass
  class ApiError(ProviderError):
      def __str__(self): return f"API error: {self.message}"

ARCHITECTURE: ProviderError variants — the error taxonomy

Variants map to HTTP status codes + semantic categories:
  `Api`            — 4xx/5xx errors that are NOT special (bad request, server error)
  `Network`        — Transport failures: connection refused, timeout, TLS error
  `Auth`           — 401/403 — bad or missing API key
  `RateLimited`    — 429 — too many requests; includes optional server-specified delay
  `ContextOverflow`— input too long for the model's context window
  `Cancelled`      — CancellationToken was triggered by the caller
  `Other`          — catch-all for anything that doesn't fit

Why a flat enum rather than a hierarchy?
  The agent loop has a simple decision tree:
    is_retryable() → retry (RateLimited, Network)
    is_context_overflow() → try compaction, then give up
    is Cancelled → clean shutdown
    everything else → surface to caller as failure
  A flat enum with methods makes this dispatch cheap and exhaustive.
*/
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// A non-transient API error (bad request, server error, etc.).
    #[error("API error: {0}")]
    Api(String),
    /// Network/transport failure — connection refused, timeout, TLS error, etc.
    #[error("Network error: {0}")]
    Network(String),
    /// Authentication failure — bad or missing API key (HTTP 401/403).
    #[error("Auth error: {0}")]
    Auth(String),
    /// Rate limit hit (HTTP 429). `retry_after_ms` is the server-specified delay if present.
    #[error("Rate limited, retry after {retry_after_ms:?}ms")]
    RateLimited { retry_after_ms: Option<u64> },
    /// Input exceeds the model's context window. Caller should compact and retry.
    #[error("Context overflow: {message}")]
    ContextOverflow { message: String },
    /// The caller cancelled the request via `CancellationToken`.
    #[error("Cancelled")]
    Cancelled,
    /// Catch-all for errors that don't fit another category.
    #[error("{0}")]
    Other(String),
}

impl ProviderError {
    /// Classify an HTTP error response into the appropriate error variant.
    ///
    /// Detects context overflow, rate limits, auth errors, and general API errors
    /// from the HTTP status code and response body.
    pub fn classify(
        status: u16,   // HTTP status code — 429, 401, 403, 400, 413, 5xx
        message: &str, // response body text — checked for overflow phrases; may be empty (Cerebras quirk)
    ) -> Self {
        if is_context_overflow(status, message) {
            Self::ContextOverflow {
                message: message.to_string(),
            }
        } else if status == 429 {
            Self::RateLimited {
                retry_after_ms: None,
            }
        } else if status == 401 || status == 403 {
            Self::Auth(message.to_string())
        } else {
            Self::Api(message.to_string())
        }
    }

    /// Returns true if this error indicates a context overflow.
    pub fn is_context_overflow(&self) -> bool {
        matches!(self, Self::ContextOverflow { .. })
    }
}

/// Known phrases that indicate context overflow across LLM providers.
///
/// Covers: Anthropic, OpenAI, Google Gemini, AWS Bedrock, xAI, Groq,
/// OpenRouter, llama.cpp, LM Studio, MiniMax, Kimi, GitHub Copilot,
/// and generic patterns.
/*
ARCHITECTURE: Centralised overflow detection — one place, all providers

Context overflow is a universal problem: every LLM has a finite token window.
But every provider expresses overflow differently:
  Anthropic: "prompt is too long: 213462 tokens > 200000 maximum"
  OpenAI:    "Your input exceeds the context window of this model"
  Gemini:    "The input token count (1196265) exceeds the maximum number of tokens allowed"
  Groq:      "Please reduce the length of the messages or completion"
  ...

Centralising these phrases in ONE constant means:
  1. Every provider uses `ProviderError::classify()` — no duplication
  2. Adding a new provider = adding one phrase to this array
  3. The agent loop only checks `is_context_overflow()` — doesn't know which provider

RUST QUIRK: `const OVERFLOW_PHRASES: &[&str]` — a compile-time constant

`const` — value is inlined at compile time (not a runtime allocation).
  The array lives in the binary's read-only data segment (`.rodata`).
  Python analogy: a module-level tuple of strings, but truly immutable.

`&[&str]` — a slice of string slices (two levels of reference):
  `&str`  — a reference to a string (UTF-8 bytes, stored somewhere)
  `&[T]`  — a "fat pointer" to a contiguous sequence of T (pointer + length)
  `&[&str]` — a reference to a sequence of `&str` items

  The string literals ("prompt is too long") are `&'static str` — they live
  forever in the binary, so no allocation, no lifetime issues.

Why not `Vec<String>`?
  `Vec<String>` is heap-allocated and built at runtime. A `const &[&str]` is
  zero runtime cost — the data is baked into the binary at compile time.

RUST QUIRK: `&[&str]` as the type for array literals
  You might expect `const X: [&str; 14] = [...]` (fixed-size array), but
  `&[&str]` (slice reference) is more ergonomic — the length is encoded in the
  fat pointer, not the type. Functions that iterate over it don't need to be
  generic over the array length.
*/
const OVERFLOW_PHRASES: &[&str] = &[
    "prompt is too long",                 // Anthropic
    "input is too long",                  // AWS Bedrock
    "exceeds the context window",         // OpenAI (Completions & Responses)
    "exceeds the maximum",                // Google Gemini ("input token count exceeds the maximum")
    "maximum prompt length",              // xAI
    "reduce the length of the messages",  // Groq
    "maximum context length",             // OpenRouter
    "exceeds the limit of",               // GitHub Copilot
    "exceeds the available context size", // llama.cpp
    "greater than the context length",    // LM Studio
    "context window exceeds limit",       // MiniMax
    "exceeded model token limit",         // Kimi
    "context length exceeded",            // Generic
    "context_length_exceeded",            // Generic (underscore variant)
    "too many tokens",                    // Generic
    "token limit exceeded",               // Generic
];

/// Check if an error message indicates context overflow (for use by types.rs).
/*
RUST QUIRK: `pub(crate)` — "public within this crate only"

`pub(crate)` sits between fully public (`pub`) and private (default).
  - `pub`         → anyone importing this crate can call it
  - `pub(crate)`  → only modules within THIS crate can call it
  - (no modifier) → only this module can call it

`is_context_overflow_message` is needed by `types.rs` (to classify SSE errors
embedded in the stream — not just HTTP status errors) but shouldn't be part of
the public library API. `pub(crate)` is the right scope.

RUST QUIRK: `.iter().any(|phrase| lower.contains(phrase))`
  `.iter()` — returns an iterator over `&&&str` (references to &str elements)
  `.any(predicate)` — short-circuits: returns `true` as soon as predicate is true
  `lower.contains(phrase)` — substring search (case-sensitive, but `lower` is already
    lowercased so we get case-insensitive matching for free)
  Python analogy: `any(phrase in lower for phrase in OVERFLOW_PHRASES)`
*/
pub(crate) fn is_context_overflow_message(message: &str) -> bool {
    let lower = message.to_lowercase(); // normalize to lowercase for case-insensitive matching
    OVERFLOW_PHRASES.iter().any(|phrase| lower.contains(phrase))
}

/// Check if an HTTP error response indicates context overflow.
/*
ARCHITECTURE: Two-path overflow detection

Path 1 — Empty body (Cerebras, Mistral quirk):
  Some providers return HTTP 400/413 with an EMPTY body when the input is too long.
  We can't match a phrase, so we infer overflow from (status=400|413) + empty body.

Path 2 — Phrase matching:
  All other providers include a descriptive message. Delegate to is_context_overflow_message().

The two paths are checked in order: empty-body first (cheaper), phrase-match second.

RUST QUIRK: `message.trim().is_empty()`
  `.trim()` removes leading/trailing whitespace, returning a `&str` slice of the original.
  `.is_empty()` returns true if the slice has length 0.
  Together: "is this message blank (or just whitespace)?"
  Python analogy: `not message.strip()`
*/
fn is_context_overflow(
    status: u16,   // HTTP status — 400/413 with empty body → overflow even without a phrase
    message: &str, // response body — matched against OVERFLOW_PHRASES; may be empty
) -> bool {
    // Some providers (Cerebras, Mistral) return 400/413 with empty body on overflow
    if (status == 400 || status == 413) && message.trim().is_empty() {
        return true;
    }
    is_context_overflow_message(message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_anthropic_overflow() {
        let err =
            ProviderError::classify(400, "prompt is too long: 213462 tokens > 200000 maximum");
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_openai_overflow() {
        let err =
            ProviderError::classify(400, "Your input exceeds the context window of this model");
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_google_overflow() {
        let err = ProviderError::classify(
            400,
            "The input token count (1196265) exceeds the maximum number of tokens allowed",
        );
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_bedrock_overflow() {
        let err = ProviderError::classify(400, "input is too long for requested model");
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_xai_overflow() {
        let err = ProviderError::classify(
            400,
            "This model's maximum prompt length is 131072 but request contains 537812 tokens",
        );
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_groq_overflow() {
        let err = ProviderError::classify(
            400,
            "Please reduce the length of the messages or completion",
        );
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_empty_body_overflow() {
        // Cerebras/Mistral return 400/413 with empty body
        let err = ProviderError::classify(413, "");
        assert!(err.is_context_overflow());
        let err = ProviderError::classify(400, "  ");
        assert!(err.is_context_overflow());
    }

    #[test]
    fn classify_rate_limit() {
        let err = ProviderError::classify(429, "rate limit exceeded");
        assert!(matches!(err, ProviderError::RateLimited { .. }));
    }

    #[test]
    fn classify_auth_error() {
        let err = ProviderError::classify(401, "invalid api key");
        assert!(matches!(err, ProviderError::Auth(_)));
        let err = ProviderError::classify(403, "forbidden");
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[test]
    fn classify_regular_api_error() {
        let err = ProviderError::classify(400, "invalid request format");
        assert!(matches!(err, ProviderError::Api(_)));
        assert!(!err.is_context_overflow());
    }

    #[test]
    fn overflow_message_case_insensitive() {
        assert!(is_context_overflow_message("PROMPT IS TOO LONG"));
        assert!(is_context_overflow_message("Too Many Tokens in request"));
    }

    #[test]
    fn non_overflow_messages() {
        assert!(!is_context_overflow_message("invalid api key"));
        assert!(!is_context_overflow_message("internal server error"));
        assert!(!is_context_overflow_message(""));
    }
}
