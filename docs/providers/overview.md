# Providers Overview

phi-core supports multiple LLM providers through the `StreamProvider` trait and `ApiProtocol`
dispatch. Callers never name a provider struct directly — `ModelConfig` is the single
descriptor for every provider connection.

## Supported Protocols

| `ApiProtocol` | Wire Format | Factory Method |
|----------|----------------|------------|
| `AnthropicMessages` | Anthropic Messages API | `ModelConfig::anthropic(id, name, key)` |
| `OpenAiCompletions` | OpenAI Chat Completions (15+ backends) | `ModelConfig::openai(id, name, key)` / `ModelConfig::local(url, id, key)` / `ModelConfig::openrouter(id, key)` |
| `OpenAiResponses` | OpenAI Responses API | Direct struct construction |
| `AzureOpenAiResponses` | Azure OpenAI Responses | Direct struct construction |
| `GoogleGenerativeAi` | Google Gemini API | `ModelConfig::google(id, name, key)` |
| `GoogleVertex` | Google Vertex AI | Direct struct construction |
| `BedrockConverseStream` | AWS Bedrock ConverseStream | Direct struct construction |

## ApiProtocol Enum

```rust
pub enum ApiProtocol {
    AnthropicMessages,
    OpenAiCompletions,
    OpenAiResponses,
    AzureOpenAiResponses,
    GoogleGenerativeAi,
    GoogleVertex,
    BedrockConverseStream,
}
```

## ModelConfig

`ModelConfig` is the single, complete description of a provider connection. Pass it to
`BasicAgent::new()`, `SubAgentTool::new()`, or `AgentLoopConfig.model_config`:

```rust
pub struct ModelConfig {
    pub id: String,              // e.g. "gpt-4o" — model name sent to the API
    pub name: String,            // e.g. "GPT-4o" — display label for logging/UI
    pub api: ApiProtocol,        // Which wire protocol to use (dispatch key)
    pub provider: String,        // e.g. "openai" — logging label
    pub base_url: String,        // API endpoint (no trailing slash)
    pub api_key: String,         // Auth credential (sk-..., or "access_key:secret" for Bedrock)
    pub reasoning: bool,         // Supports thinking/reasoning
    pub context_window: u32,     // Context size in tokens
    pub max_tokens: u32,         // Default max output
    pub cost: CostConfig,        // Pricing per million tokens (0.0 = no tracking)
    pub headers: HashMap<String, String>,  // Extra HTTP headers
    pub compat: Option<OpenAiCompat>,      // Quirk flags (OpenAiCompletions only)
}
```

Factory methods (all accept `api_key` as the auth parameter):

```rust
let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let anthropic = ModelConfig::anthropic("claude-sonnet-4-20250514", "Claude Sonnet 4", &api_key);

let openai_key = std::env::var("OPENAI_API_KEY").unwrap();
let openai = ModelConfig::openai("gpt-4o", "GPT-4o", &openai_key);

let gemini_key = std::env::var("GEMINI_API_KEY").unwrap();
let google = ModelConfig::google("gemini-2.0-flash", "Gemini 2.0 Flash", &gemini_key);

// Local server — pass empty string for api_key if unauthenticated
let local = ModelConfig::local("http://localhost:1234/v1", "my-model", "");

// OpenRouter — dedicated factory with correct compat flags
let or_key = std::env::var("OPENROUTER_API_KEY").unwrap();
let openrouter = ModelConfig::openrouter("anthropic/claude-sonnet-4", &or_key);
```

## ProviderRegistry

Maps `ApiProtocol` → `StreamProvider`. The default registry includes all built-in providers:

```rust
let registry = ProviderRegistry::default();

// Use it to stream with any model
let result = registry.stream(&model_config, stream_config, tx, cancel).await?;
```

Custom registries (advanced — for adding a fully custom `StreamProvider` implementation):

```rust
use phi_core::provider::{ProviderRegistry, ApiProtocol};

let mut registry = ProviderRegistry::new();
registry.register(ApiProtocol::AnthropicMessages, my_custom_provider);
// Then pass to AgentLoopConfig... (most users should use provider_override instead)
```

## StreamProvider Trait

```rust
#[async_trait]
pub trait StreamProvider: Send + Sync {
    async fn stream(
        &self,
        config: StreamConfig,
        tx: mpsc::UnboundedSender<StreamEvent>,
        cancel: CancellationToken,
    ) -> Result<Message, ProviderError>;
}
```

All providers receive a `StreamConfig`, emit `StreamEvent`s through the channel, and return the final `Message`.

## OpenAPI Tool Adapter

In addition to LLM providers, phi-core can auto-generate tools from any OpenAPI 3.0 spec. This is a tool integration (not a provider), but it complements the provider system by letting agents call external APIs.

Enable with `features = ["openapi"]`. See the [OpenAPI Tools guide](../guides/openapi.md) for details.
