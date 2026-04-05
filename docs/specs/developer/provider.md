<!-- Last verified: 2026-04-05 by Claude Code -->
# Provider System

The provider system abstracts all LLM backends behind a single `StreamProvider` trait. The caller constructs a `ModelConfig` (the model's "identity card"), and the `ProviderRegistry` dispatches to the correct concrete provider at runtime. This design allows seamless switching between Anthropic, OpenAI, Google, Bedrock, Azure, and 15+ OpenAI-compatible providers without changing application code.

## Concept Overview

```
Provider [EXISTS]
├── ModelConfig [EXISTS] — id, name, api, provider, base_url, api_key, cost
├── ApiProtocol [EXISTS] — 7 variants (Anthropic, OpenAI, Google, Bedrock, Azure, etc.)
├── CostConfig [EXISTS] — per-million rates
├── StreamProvider trait [EXISTS] — stream() method
├── ProviderRegistry [EXISTS] — dispatch by ApiProtocol
├── OpenAiCompat [EXISTS] — quirk flags for 15+ providers
└── ContextTranslationStrategy [EXISTS] — cross-provider content translation (G8, src/provider/context_translation.rs)
```

---

## ModelConfig [EXISTS]

The single source of truth for a model's identity. Bundles everything a provider needs to make API calls.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `id` | `String` | [EXISTS] | Model identifier sent to the API (e.g. `"gpt-4o"`, `"claude-sonnet-4-20250514"`) |
| `name` | `String` | [EXISTS] | Human-friendly display name (logging/UI; not sent to API) |
| `api` | `ApiProtocol` | [EXISTS] | Which wire protocol to use (dispatch key for `ProviderRegistry`) |
| `provider` | `String` | [EXISTS] | Provider name for logging (e.g. `"openai"`, `"anthropic"`) |
| `base_url` | `String` | [EXISTS] | Base URL for API requests (supports private deployments, proxies) |
| `api_key` | `String` | [EXISTS] | Authentication credential; defaults to empty string so configs can omit it |
| `reasoning` | `bool` | [EXISTS] | Whether this model supports extended thinking/reasoning |
| `context_window` | `u32` | [EXISTS] | Max input tokens (used for compaction decisions) |
| `max_tokens` | `u32` | [EXISTS] | Default max output tokens |
| `cost` | `CostConfig` | [EXISTS] | Token pricing for cost tracking (defaults to zero) |
| `headers` | `HashMap<String, String>` | [EXISTS] | Additional HTTP headers (e.g. API-version headers) |
| `compat` | `Option<OpenAiCompat>` | [EXISTS] | OpenAI quirk flags; `None` for non-OpenAI providers |

---

## ApiProtocol [EXISTS]

The dispatch key that maps a model to its concrete `StreamProvider` implementation. Seven variants covering all supported backends.

| Variant | Provider File | Status | Covers |
|---------|--------------|--------|--------|
| `AnthropicMessages` | `anthropic.rs` | [EXISTS] | Claude models |
| `OpenAiCompletions` | `openai_compat.rs` | [EXISTS] | OpenAI, Groq, Together, DeepSeek, Fireworks, Mistral, xAI, OpenRouter, etc. (15+) |
| `OpenAiResponses` | `openai_responses.rs` | [EXISTS] | OpenAI Responses API |
| `AzureOpenAiResponses` | `azure_openai.rs` | [EXISTS] | Azure OpenAI |
| `GoogleGenerativeAi` | `google.rs` | [EXISTS] | Gemini (Google AI Studio) |
| `GoogleVertex` | `google_vertex.rs` | [EXISTS] | Vertex AI |
| `BedrockConverseStream` | `bedrock.rs` | [EXISTS] | Amazon Bedrock (ConverseStream) |

---

## CostConfig [EXISTS]

Token pricing per million tokens. Embedded in `ModelConfig` with `#[serde(default)]` fields, so callers who don't need cost tracking can omit it.

| Field | Type | Status | Description |
|-------|------|--------|-------------|
| `input_per_million` | `f64` | [EXISTS] | Cost per million input tokens |
| `output_per_million` | `f64` | [EXISTS] | Cost per million output tokens |
| `cache_read_per_million` | `f64` | [EXISTS] | Cost per million cache-read tokens (default: 0.0) |
| `cache_write_per_million` | `f64` | [EXISTS] | Cost per million cache-write tokens (default: 0.0) |

---

## StreamProvider Trait [EXISTS]

The core abstraction every LLM backend implements. The rest of the codebase interacts only with `&dyn StreamProvider` -- it never knows which concrete backend is used at runtime.

| Method | Signature | Status | Description |
|--------|-----------|--------|-------------|
| `provider_id()` | `-> &str` | [EXISTS] | Short stable identifier (e.g. `"anthropic"`); used in `loop_id` construction |
| `stream()` | `(config, tx, cancel) -> Result<Message, ProviderError>` | [EXISTS] | Stream a completion; sends `StreamEvent`s through `tx` in real time; returns final assembled `Message` |

**Dual-output contract**: The `tx` channel carries partial deltas for real-time UI updates. The return value carries the complete message after the stream ends. The loop cannot read its own output from the channel -- the return value is the protocol, the channel is the live feed.

---

## ProviderRegistry [EXISTS]

Maps `ApiProtocol` to `StreamProvider` implementations. Factory + router.

| Method | Status | Description |
|--------|--------|-------------|
| `new()` | [EXISTS] | Empty registry (no providers) |
| `default()` | [EXISTS] | All 7 built-in providers registered |
| `register(protocol, provider)` | [EXISTS] | Register a provider for a protocol (overwrites if exists) |
| `get(protocol)` | [EXISTS] | Look up provider by protocol |
| `has(protocol)` | [EXISTS] | Check if a provider is registered |
| `protocols()` | [EXISTS] | List all registered protocols |
| `stream(model, config, tx, cancel)` | [EXISTS] | Dispatch: looks up provider by `model.api`, delegates to `provider.stream()` |

**Design**: `model` (routing key) is separate from `config` (request payload). The registry routes on `model.api`, then passes `config` through unchanged.

---

## OpenAiCompat Quirk Flags [EXISTS]

The "quirk matrix" for 15+ OpenAI-compatible providers. One `openai_compat.rs` provider reads these flags at runtime and branches accordingly, instead of maintaining separate provider files per quirk combination.

| Flag | Type | Status | Description |
|------|------|--------|-------------|
| `supports_store` | `bool` | [EXISTS] | Supports the `store` parameter for conversation persistence |
| `supports_developer_role` | `bool` | [EXISTS] | Supports `developer` role (system-level instructions) |
| `supports_reasoning_effort` | `bool` | [EXISTS] | Supports `reasoning_effort` parameter |
| `supports_usage_in_streaming` | `bool` | [EXISTS] | Includes usage data in streaming responses (default: `true`) |
| `max_tokens_field` | `MaxTokensField` | [EXISTS] | Which field name to use: `MaxTokens` or `MaxCompletionTokens` |
| `requires_tool_result_name` | `bool` | [EXISTS] | Tool results must include a `name` field |
| `requires_assistant_after_tool_result` | `bool` | [EXISTS] | Must insert assistant message after tool results |
| `thinking_format` | `ThinkingFormat` | [EXISTS] | How thinking/reasoning content is formatted: `OpenAi`, `Xai`, `Qwen`, `OpenRouter` |

**Factory methods** for provider-specific flag combinations:

| Method | Status | Notes |
|--------|--------|-------|
| `OpenAiCompat::openai()` | [EXISTS] | store, developer role, reasoning effort, MaxCompletionTokens |
| `OpenAiCompat::xai()` | [EXISTS] | Grok thinking format |
| `OpenAiCompat::groq()` | [EXISTS] | Default with streaming usage |
| `OpenAiCompat::cerebras()` | [EXISTS] | Pure default (no deviations) |
| `OpenAiCompat::openrouter()` | [EXISTS] | Developer role, OpenRouter thinking format |
| `OpenAiCompat::mistral()` | [EXISTS] | MaxTokens field |
| `OpenAiCompat::deepseek()` | [EXISTS] | MaxCompletionTokens |

---

## Factory Methods on ModelConfig [EXISTS]

Convenience constructors for common providers.

| Method | Status | Protocol | Default context_window |
|--------|--------|----------|----------------------|
| `ModelConfig::anthropic(id, name, api_key)` | [EXISTS] | `AnthropicMessages` | 200,000 |
| `ModelConfig::openai(id, name, api_key)` | [EXISTS] | `OpenAiCompletions` | 128,000 |
| `ModelConfig::google(id, name, api_key)` | [EXISTS] | `GoogleGenerativeAi` | 1,000,000 |
| `ModelConfig::local(base_url, model_id, api_key)` | [EXISTS] | `OpenAiCompletions` | 128,000 |
| `ModelConfig::openrouter(model_id, api_key)` | [EXISTS] | `OpenAiCompletions` | 200,000 |

---

## ProviderError [EXISTS]

Error taxonomy for provider failures. The agent loop uses this for retry/recovery decisions.

| Variant | Status | Retryable | Description |
|---------|--------|-----------|-------------|
| `Api(String)` | [EXISTS] | No | Non-transient API error (bad request, server error) |
| `Network(String)` | [EXISTS] | Yes | Transport failure (connection refused, timeout, TLS) |
| `Auth(String)` | [EXISTS] | No | 401/403 -- bad or missing API key |
| `RateLimited { retry_after_ms }` | [EXISTS] | Yes | 429 -- too many requests |
| `ContextOverflow { message }` | [EXISTS] | No (compact) | Input exceeds context window; caller should compact and retry |
| `Cancelled` | [EXISTS] | No | `CancellationToken` triggered |
| `Other(String)` | [EXISTS] | No | Catch-all |

**Context overflow detection**: Centralized in `OVERFLOW_PHRASES` covering 15+ provider-specific error strings. Both HTTP errors and SSE-embedded errors are classified.

---

## Code Reference

| Concept | File |
|---------|------|
| `ModelConfig`, `ApiProtocol`, `CostConfig`, `OpenAiCompat`, `MaxTokensField`, `ThinkingFormat` | `src/provider/model.rs` |
| `StreamProvider` trait, `StreamConfig`, `StreamEvent`, `ToolDefinition`, `ProviderError` | `src/provider/traits.rs` |
| `ProviderRegistry` | `src/provider/registry.rs` |
| `AnthropicProvider` | `src/provider/anthropic.rs` |
| `OpenAiCompatProvider` | `src/provider/openai_compat.rs` |
| `OpenAiResponsesProvider` | `src/provider/openai_responses.rs` |
| `AzureOpenAiProvider` | `src/provider/azure_openai.rs` |
| `GoogleProvider` | `src/provider/google.rs` |
| `GoogleVertexProvider` | `src/provider/google_vertex.rs` |
| `BedrockProvider` | `src/provider/bedrock.rs` |
| `RetryConfig` | `src/provider/retry.rs` |
| `MockProvider` (testing) | `src/provider/mock.rs` |

---

## Conceptual Notes

- **ContextTranslationStrategy** [EXISTS] -- Trait in `src/provider/context_translation.rs` (G8). `DefaultContextTranslation` handles cross-provider content translation: Anthropic keeps Thinking blocks, OpenAI converts to Text with `[Reasoning]` prefix, Google/Bedrock drops Thinking. Set on `AgentLoopConfig.context_translation`.
- **Model fallback chain** -- Model resolution follows: Loop (`AgentLoopConfig.model_config`) -> Session model override [EXISTS] (`Session.model_config: Option<ModelConfig>`) -> Agent default (`BasicAgent.model_config`).
- **provider_override** -- `AgentLoopConfig.provider_override: Option<Arc<dyn StreamProvider>>` bypasses `ProviderRegistry` dispatch entirely. Used for testing with `MockProvider` or injecting custom provider implementations.
