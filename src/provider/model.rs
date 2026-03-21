//! Model configuration and provider compatibility flags.
/*
ARCHITECTURE: model.rs — the "model identity card"

Every LLM provider has a different API shape, auth style, field names, and
quirks. This module defines the data structures that capture all of that
variation in a single `ModelConfig` value.

Key types:
  `ApiProtocol`  — which wire protocol to use (Anthropic vs OpenAI vs Gemini vs ...)
  `ModelConfig`  — the full model "identity card": base_url, auth, limits, quirks
  `OpenAiCompat` — per-provider flags for the 15+ OpenAI-compatible providers
  `CostConfig`   — token pricing (optional, used for cost tracking)

How it flows:
  1. Caller builds or loads a `ModelConfig` (factory methods: `ModelConfig::anthropic()`,
     `ModelConfig::openai()`, etc., or deserialize from JSON/YAML)
  2. Sets it on `StreamConfig::model_config`
  3. `ProviderRegistry::for_protocol()` picks the right `StreamProvider` impl
     based on `config.api`
  4. The provider uses `base_url`, `compat`, `headers` etc. from `ModelConfig`
     to customise API calls

Why not hard-code provider details in each provider file?
  ModelConfig externalizes the provider-specific details so users can configure
  custom endpoints, private deployments, or new providers without changing
  provider source code.
*/

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Which API protocol a model uses.
/*
ARCHITECTURE: ApiProtocol — the dispatch key for the provider registry

`ProviderRegistry::for_protocol(api: ApiProtocol)` maps each variant to
a concrete `StreamProvider` implementation:
  AnthropicMessages       → AnthropicProvider
  OpenAiCompletions       → OpenAiCompatProvider (handles 15+ providers)
  OpenAiResponses         → OpenAiResponsesProvider
  AzureOpenAiResponses    → AzureOpenAiProvider
  GoogleGenerativeAi      → GoogleProvider
  GoogleVertex            → GoogleVertexProvider
  BedrockConverseStream   → BedrockProvider

This is the "Strategy via enum dispatch" pattern: the enum variant IS the strategy
selector. The registry (registry.rs) `match`es on this enum and returns the right
provider. At runtime, the caller only holds a `Box<dyn StreamProvider>` and never
needs to know which variant was used.

RUST QUIRK: `Hash` derive — required for use as HashMap keys

`#[derive(Hash)]` enables values of this type to be used as keys in `HashMap<K, V>`.
`Hash` computes an integer hash of the value. Combined with `PartialEq + Eq`
(also derived), this is what HashMap needs:
  - `Hash` to find the bucket
  - `Eq` to confirm the key matches within the bucket (hash collisions)

Why does `ApiProtocol` need to be a HashMap key?
  In `ProviderRegistry`, we may store `HashMap<ApiProtocol, Box<dyn StreamProvider>>`.
  Without `Hash + Eq`, that HashMap would fail to compile.

RUST QUIRK: `Copy` on an enum with no data fields
  All variants of `ApiProtocol` carry no data — they're just tags.
  `Copy` lets the compiler bitwise-copy the value instead of moving it.
  After `let api = model.api;`, `model.api` is STILL valid (Copy semantics).
  Python analogy: Python enums are always by-reference, so no equivalent concept.

RUST QUIRK: `#[serde(rename_all = "snake_case")]`
  When serializing to JSON/YAML, variant names are converted to snake_case:
    `AnthropicMessages` → "anthropic_messages"
    `BedrockConverseStream` → "bedrock_converse_stream"
  This makes config files human-readable without matching Rust's PascalCase convention.
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiProtocol {
    AnthropicMessages,
    OpenAiCompletions,
    OpenAiResponses,
    AzureOpenAiResponses,
    GoogleGenerativeAi,
    GoogleVertex,
    BedrockConverseStream,
}

impl std::fmt::Display for ApiProtocol {
    /*
    RUST QUIRK: Implementing `Display` manually (vs deriving it)

    `Display` (the `{}` formatter) is NOT derivable — you must write it by hand.
    `Debug` (the `{:?}` formatter) IS derivable.

    Why? `Debug` is purely for developers (shows the Rust name), so auto-generated
    is fine. `Display` is for end-users, and you control the string representation.

    Here we return snake_case strings ("anthropic_messages") instead of the
    Rust PascalCase names ("AnthropicMessages") — consistent with the serde rename.

    `write!(f, "...")` — writes into the formatter buffer `f`.
    Returns `fmt::Result` (Ok or Err), required by the trait.
    Python analogy: implementing __str__(self) → return "anthropic_messages"
    */
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AnthropicMessages => write!(f, "anthropic_messages"),
            Self::OpenAiCompletions => write!(f, "openai_completions"),
            Self::OpenAiResponses => write!(f, "openai_responses"),
            Self::AzureOpenAiResponses => write!(f, "azure_openai_responses"),
            Self::GoogleGenerativeAi => write!(f, "google_generative_ai"),
            Self::GoogleVertex => write!(f, "google_vertex"),
            Self::BedrockConverseStream => write!(f, "bedrock_converse_stream"),
        }
    }
}

/// Cost per million tokens (input/output).
/*
ARCHITECTURE: CostConfig — optional cost tracking

LLM providers charge differently for input vs output tokens, and some offer
reduced prices for cache reads and cache writes (Anthropic prompt caching).

`CostConfig` is embedded in `ModelConfig` but has `#[serde(default)]` fields,
meaning callers who don't care about cost tracking don't need to supply them —
they default to 0.0.

RUST QUIRK: `#[serde(default)]` — per-field default during deserialization
  When deserializing a `ModelConfig`, if "cache_read_per_million" is absent in
  the JSON/YAML, serde calls `Default::default()` for that field instead of
  returning an error. This makes the struct forward-compatible: old config files
  (without the cache fields) still deserialize correctly.
  Python analogy: `dataclasses.field(default=0.0)` or `pydantic.Field(default=0.0)`
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostConfig {
    pub input_per_million: f64,
    pub output_per_million: f64,
    #[serde(default)]
    pub cache_read_per_million: f64,
    #[serde(default)]
    pub cache_write_per_million: f64,
}

impl Default for CostConfig {
    fn default() -> Self {
        Self {
            input_per_million: 0.0,
            output_per_million: 0.0,
            cache_read_per_million: 0.0,
            cache_write_per_million: 0.0,
        }
    }
}

/// How a provider handles the `max_tokens` field.
/*
ARCHITECTURE: MaxTokensField — a per-provider API quirk

The OpenAI-compatible API has two field names for the same concept:
  `max_tokens`           — the original field name, used by most providers
  `max_completion_tokens`— new name, required by OpenAI o-series reasoning models

Both control the maximum number of tokens in the response, but OpenAI split
them so reasoning token budgets are counted separately. The provider must use
the correct field name, or the API returns an error.

`MaxTokensField` is a small enum used as a flag inside `OpenAiCompat`, avoiding
a raw `bool` (which would be less self-documenting).

RUST QUIRK: `#[derive(Default)]` + `#[default]` on a variant
  `#[derive(Default)]` auto-generates `Default::default()` for the enum.
  `#[default]` on a specific variant marks it as the default value:
    `MaxTokensField::default()` → `MaxTokensField::MaxTokens`
  Without `#[default]`, the derive macro wouldn't know which variant to pick.
  Python analogy: no direct equivalent; closest is Enum with a class variable for default.
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MaxTokensField {
    #[default]
    MaxTokens,
    MaxCompletionTokens,
}

/// How a provider formats thinking/reasoning output.
/*
ARCHITECTURE: ThinkingFormat — per-provider reasoning output format

Extended thinking / chain-of-thought output is formatted differently by each provider:
  `OpenAi` — reasoning appears in a dedicated `reasoning_content` array
  `Xai`    — Grok's format (slightly different JSON structure)
  `Qwen`   — Qwen's format (another variation)

This flag tells `openai_compat.rs` which parsing branch to use when extracting
thinking deltas from the streaming response. Without this flag, we'd need a
separate provider file for each thinking-capable service.
*/
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingFormat {
    #[default]
    OpenAi,
    Xai,
    Qwen,
}

/// Compatibility flags for OpenAI-compatible providers.
/// Different providers have different quirks even though they share the same base API.
/*
ARCHITECTURE: OpenAiCompat — the "quirk matrix" for 15+ OpenAI-compatible providers

The OpenAI Chat Completions API is a de-facto standard that dozens of providers
implement. But every provider deviates in small ways:
  - OpenAI o-series uses `max_completion_tokens` not `max_tokens`
  - xAI (Grok) uses a different thinking output format
  - Some providers don't include usage data in streaming chunks
  - Some require a `name` field in tool results
  - Some need a dummy assistant message inserted after tool results

Instead of writing a separate provider for each quirk combination, we have ONE
`openai_compat.rs` provider that reads `OpenAiCompat` flags at runtime and
branches accordingly. New providers = new `OpenAiCompat::new_provider()` factory.

The factory methods (`openai()`, `xai()`, `groq()`, ...) use `..Default::default()`
struct update syntax to express only the fields that differ from defaults.
Python analogy: a dataclass with defaults, and factory classmethods that override
only the fields that need to change.
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAiCompat {
    /// Supports the `store` parameter for conversation persistence.
    pub supports_store: bool,
    /// Supports `developer` role (system-level instructions).
    pub supports_developer_role: bool,
    /// Supports `reasoning_effort` parameter.
    pub supports_reasoning_effort: bool,
    /// Includes usage data in streaming responses.
    pub supports_usage_in_streaming: bool,
    /// Which field name to use for max tokens.
    pub max_tokens_field: MaxTokensField,
    /// Tool results must include a `name` field.
    pub requires_tool_result_name: bool,
    /// Must insert an assistant message after tool results.
    pub requires_assistant_after_tool_result: bool,
    /// How thinking/reasoning content is formatted in streaming.
    pub thinking_format: ThinkingFormat,
}

impl Default for OpenAiCompat {
    /*
    RUST QUIRK: `impl Default` manually (rather than `#[derive(Default)]`)

    `#[derive(Default)]` would work only if every field's type implements `Default`
    AND the zero-values are the right defaults. Here, `supports_usage_in_streaming`
    should default to `true`, not `false`. Since `bool` defaults to `false`, we
    must override it manually.

    A manually written `Default` impl is common when some field defaults are
    non-trivial (non-zero numbers, non-empty strings, true booleans, etc.).
    */
    fn default() -> Self {
        Self {
            supports_store: false,
            supports_developer_role: false,
            supports_reasoning_effort: false,
            supports_usage_in_streaming: true, // most OpenAI-compat providers include usage
            max_tokens_field: MaxTokensField::MaxTokens,
            requires_tool_result_name: false,
            requires_assistant_after_tool_result: false,
            thinking_format: ThinkingFormat::OpenAi,
        }
    }
}

impl OpenAiCompat {
    /// Compat flags for native OpenAI.
    /*
    RUST QUIRK: `..Default::default()` — struct update syntax for overriding defaults

    `Self { supports_store: true, ..Default::default() }` means:
      "build a Self where supports_store = true (and supports_developer_role = true,
       supports_reasoning_effort = true, max_tokens_field = MaxCompletionTokens)
       and all OTHER fields come from Default::default()"

    The `..expr` "spreads" the remaining fields from a base value.
    It MUST be last in the struct literal.
    Python analogy: dataclasses.replace(OpenAiCompat(), supports_store=True, ...)

    Why is this better than repeating all fields?
      - Fewer lines to write (only express differences from defaults)
      - If a new field is added with a sensible default, existing factory methods
        automatically get the right value — no manual update needed
    */
    pub fn openai() -> Self {
        Self {
            supports_store: true,
            supports_developer_role: true,
            supports_reasoning_effort: true,
            supports_usage_in_streaming: true,
            max_tokens_field: MaxTokensField::MaxCompletionTokens,
            ..Default::default()
        }
    }

    /// Compat flags for xAI (Grok).
    pub fn xai() -> Self {
        Self {
            supports_usage_in_streaming: true,
            thinking_format: ThinkingFormat::Xai, // Grok uses a different thinking JSON shape
            ..Default::default()
        }
    }

    /// Compat flags for Groq.
    pub fn groq() -> Self {
        Self {
            supports_usage_in_streaming: true,
            ..Default::default()
        }
    }

    /// Compat flags for Cerebras.
    pub fn cerebras() -> Self {
        Self::default() // no deviations from defaults
    }

    /// Compat flags for OpenRouter.
    pub fn openrouter() -> Self {
        Self {
            supports_usage_in_streaming: true,
            max_tokens_field: MaxTokensField::MaxCompletionTokens,
            ..Default::default()
        }
    }

    /// Compat flags for Mistral.
    pub fn mistral() -> Self {
        Self {
            supports_usage_in_streaming: true,
            max_tokens_field: MaxTokensField::MaxTokens,
            ..Default::default()
        }
    }

    /// Compat flags for DeepSeek.
    pub fn deepseek() -> Self {
        Self {
            supports_usage_in_streaming: true,
            max_tokens_field: MaxTokensField::MaxCompletionTokens,
            ..Default::default()
        }
    }
}

/// Full model configuration. Knows everything needed to make API calls.
/*
ARCHITECTURE: ModelConfig — the single source of truth for a model's identity

`ModelConfig` bundles everything a provider needs to make API calls:
  - `id` / `name`    — which model to request (sent in the API body)
  - `api`            — which provider implementation to use (dispatch key)
  - `provider`       — human label for logging/display
  - `base_url`       — the HTTP endpoint (can be a private deployment or proxy)
  - `reasoning`      — whether this model supports extended thinking
  - `context_window` — max input tokens (used for context compaction decisions)
  - `max_tokens`     — default output token limit
  - `cost`           — token pricing for cost tracking
  - `headers`        — additional HTTP headers (e.g., API-version headers)
  - `compat`         — OpenAI quirk flags (only for OpenAiCompletions protocol)

Factory methods (`anthropic()`, `openai()`, `local()`, `google()`) cover common
cases. Custom providers are built by constructing the struct directly.

RUST QUIRK: `HashMap<String, String>` — a key-value dictionary
  `HashMap<K, V>` from `std::collections` — Rust's standard hash map.
  Here it stores additional HTTP headers like `{"X-My-Header": "value"}`.
  Python analogy: `dict[str, str]`.
  `#[serde(default)]` means it deserializes as an empty HashMap if absent in config.

RUST QUIRK: `Option<OpenAiCompat>` — present only for OpenAI-compat providers
  Anthropic/Google/Bedrock have their own provider files that don't use `compat`.
  For them, `compat` is `None`. For OpenAI-compatible providers, `compat` is
  `Some(OpenAiCompat { ... })`. This models "this field only makes sense for
  a subset of configurations." The provider accesses it with `compat.as_ref()?` or
  `compat.unwrap_or_default()`.
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelConfig {
    /// Model identifier sent to the API (e.g. "gpt-4o", "claude-sonnet-4-20250514").
    pub id: String,
    /// Human-friendly name.
    pub name: String,
    /// Which API protocol to use.
    pub api: ApiProtocol,
    /// Provider name (e.g. "openai", "anthropic", "xai").
    pub provider: String,
    /// Base URL for API requests (without trailing slash).
    pub base_url: String,
    /// Whether this model supports reasoning/thinking.
    pub reasoning: bool,
    /// Context window size in tokens.
    pub context_window: u32,
    /// Default max output tokens.
    pub max_tokens: u32,
    /// Cost configuration.
    #[serde(default)]
    pub cost: CostConfig,
    /// Additional headers to send with requests.
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// OpenAI-compat quirk flags (only for OpenAiCompletions protocol).
    #[serde(default)]
    pub compat: Option<OpenAiCompat>,
}

impl ModelConfig {
    /// Create a new Anthropic model config.
    pub fn anthropic(
        id: impl Into<String>, // API ID — model identifier sent in the request body (e.g. "claude-sonnet-4-20250514")
        name: impl Into<String>, // DISPLAY NAME — human-readable label for logging/UI; not sent to the API
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            api: ApiProtocol::AnthropicMessages,
            provider: "anthropic".into(),
            base_url: "https://api.anthropic.com".into(),
            reasoning: false,
            context_window: 200_000,
            max_tokens: 8192,
            cost: CostConfig::default(),
            headers: HashMap::new(),
            compat: None, // Anthropic has its own protocol, no compat flags needed
        }
    }

    /// Create a new OpenAI model config.
    pub fn openai(
        id: impl Into<String>, // API ID — model identifier sent in the request body (e.g. "gpt-4o")
        name: impl Into<String>, // DISPLAY NAME — human-readable label for logging/UI; not sent to the API
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            api: ApiProtocol::OpenAiCompletions,
            provider: "openai".into(),
            base_url: "https://api.openai.com/v1".into(),
            reasoning: false,
            context_window: 128_000,
            max_tokens: 4096,
            cost: CostConfig::default(),
            headers: HashMap::new(),
            compat: Some(OpenAiCompat::openai()), // OpenAI needs compat flags (store, developer role, etc.)
        }
    }

    /// Create a config for a local OpenAI-compatible server (LM Studio, Ollama, etc.).
    /// No API key required — sends an empty Bearer token.
    pub fn local(
        base_url: impl Into<String>, // ENDPOINT — full base URL of the local server (e.g. "http://localhost:1234/v1")
        model_id: impl Into<String>, // API ID — model name expected by the local server (e.g. "llama-3.1-8b")
    ) -> Self {
        Self {
            id: model_id.into(),
            name: "Local Model".into(),
            api: ApiProtocol::OpenAiCompletions,
            provider: "local".into(),
            base_url: base_url.into(), // caller provides e.g. "http://localhost:1234/v1"
            reasoning: false,
            context_window: 128_000,
            max_tokens: 4096,
            cost: CostConfig::default(),
            headers: HashMap::new(),
            compat: Some(OpenAiCompat::default()), // most local servers are generic OpenAI-compat
        }
    }

    /// Create a new Google Generative AI (Gemini) model config.
    pub fn google(
        id: impl Into<String>, // API ID — model identifier sent in the request URL (e.g. "gemini-2.5-pro")
        name: impl Into<String>, // DISPLAY NAME — human-readable label for logging/UI; not sent to the API
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            api: ApiProtocol::GoogleGenerativeAi,
            provider: "google".into(),
            base_url: "https://generativelanguage.googleapis.com".into(),
            reasoning: false,
            context_window: 1_000_000,
            max_tokens: 8192,
            cost: CostConfig::default(),
            headers: HashMap::new(),
            compat: None, // Google has its own protocol, no compat flags needed
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_model_config_anthropic() {
        let config = ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4");
        assert_eq!(config.api, ApiProtocol::AnthropicMessages);
        assert_eq!(config.provider, "anthropic");
        assert!(config.compat.is_none());
    }

    #[test]
    fn test_model_config_openai() {
        let config = ModelConfig::openai("gpt-4o", "GPT-4o");
        assert_eq!(config.api, ApiProtocol::OpenAiCompletions);
        let compat = config.compat.unwrap();
        assert!(compat.supports_store);
        assert!(compat.supports_developer_role);
        assert_eq!(compat.max_tokens_field, MaxTokensField::MaxCompletionTokens);
    }

    #[test]
    fn test_openai_compat_variants() {
        let xai = OpenAiCompat::xai();
        assert_eq!(xai.thinking_format, ThinkingFormat::Xai);
        assert!(!xai.supports_store);

        let groq = OpenAiCompat::groq();
        assert!(groq.supports_usage_in_streaming);
        assert!(!groq.supports_store);

        let deepseek = OpenAiCompat::deepseek();
        assert_eq!(
            deepseek.max_tokens_field,
            MaxTokensField::MaxCompletionTokens
        );
    }

    #[test]
    fn test_api_protocol_display() {
        assert_eq!(
            ApiProtocol::AnthropicMessages.to_string(),
            "anthropic_messages"
        );
        assert_eq!(
            ApiProtocol::OpenAiCompletions.to_string(),
            "openai_completions"
        );
        assert_eq!(
            ApiProtocol::GoogleGenerativeAi.to_string(),
            "google_generative_ai"
        );
    }

    #[test]
    fn test_cost_config_default() {
        let cost = CostConfig::default();
        assert_eq!(cost.input_per_million, 0.0);
        assert_eq!(cost.output_per_million, 0.0);
    }
}
