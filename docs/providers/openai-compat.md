<!-- Last verified: 2026-04-05 by Claude Code -->
# OpenAI Compatible Provider

One implementation (`OpenAiCompatProvider`) covers OpenAI, xAI, Groq, Cerebras, OpenRouter,
Mistral, DeepSeek, and any other OpenAI Chat Completions-compatible API. The provider is
selected automatically when `ModelConfig.api == ApiProtocol::OpenAiCompletions`.

Per-service behavior is controlled by `OpenAiCompat` flags stored in `ModelConfig.compat`.

## Usage

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

// OpenAI
let api_key = std::env::var("OPENAI_API_KEY").unwrap();
let agent = BasicAgent::new(ModelConfig::openai("gpt-4o", "GPT-4o", &api_key));

// OpenRouter
let or_key = std::env::var("OPENROUTER_API_KEY").unwrap();
let agent = BasicAgent::new(ModelConfig::openrouter("anthropic/claude-sonnet-4", &or_key));

// Local server (LM Studio, Ollama, llama.cpp, vLLM)
let agent = BasicAgent::new(ModelConfig::local(
    "http://localhost:1234/v1",
    "my-model",
    "",  // empty string — most local servers don't require auth
));
```

## OpenAiCompat Quirk Flags

Different providers have behavioral differences even though they share the same API:

```rust
pub struct OpenAiCompat {
    pub supports_store: bool,
    pub supports_developer_role: bool,
    pub supports_reasoning_effort: bool,
    pub supports_usage_in_streaming: bool,
    pub max_tokens_field: MaxTokensField,       // MaxTokens or MaxCompletionTokens
    pub requires_tool_result_name: bool,
    pub requires_assistant_after_tool_result: bool,
    pub thinking_format: ThinkingFormat,        // OpenAi, Xai, Qwen, or OpenRouter
}
```

## Provider Presets

| Provider | `ModelConfig` factory | Key Differences |
|----------|-------------|-----------------|
| OpenAI | `ModelConfig::openai(id, name, key)` | `developer` role, `max_completion_tokens`, `store`, `reasoning_effort` |
| OpenRouter | `ModelConfig::openrouter(id, key)` | `developer` role, `max_tokens`, OpenRouter thinking format |
| Local | `ModelConfig::local(url, id, key)` | Generic defaults, empty api_key OK |
| xAI (Grok) | Direct construction with `OpenAiCompat::xai()` | `reasoning` field for thinking |
| Groq | Direct construction with `OpenAiCompat::groq()` | Standard defaults |
| Cerebras | Direct construction with `OpenAiCompat::cerebras()` | Standard defaults |
| Mistral | Direct construction with `OpenAiCompat::mistral()` | `max_tokens` field |
| DeepSeek | Direct construction with `OpenAiCompat::deepseek()` | `max_completion_tokens` |

## Adding a New Compatible Provider

1. Add a constructor to `OpenAiCompat`:

```rust
impl OpenAiCompat {
    pub fn my_provider() -> Self {
        Self {
            supports_usage_in_streaming: true,
            // set flags as needed...
            ..Default::default()
        }
    }
}
```

2. Create a `ModelConfig` that uses it:

```rust
use phi_core::provider::{ModelConfig, ApiProtocol, OpenAiCompat};

let config = ModelConfig {
    id: "my-model".into(),
    name: "My Model".into(),
    api: ApiProtocol::OpenAiCompletions,
    provider: "my-provider".into(),
    base_url: "https://api.myprovider.com/v1".into(),
    api_key: std::env::var("MY_API_KEY").unwrap_or_default(),
    compat: Some(OpenAiCompat::my_provider()),
    ..Default::default()
};
BasicAgent::new(config)
```

## Thinking/Reasoning

The `ThinkingFormat` enum controls how reasoning content is parsed from streams:

- `ThinkingFormat::OpenAi` — Uses `reasoning_content` field (most providers, default)
- `ThinkingFormat::Xai` — Uses `reasoning` field (Grok)
- `ThinkingFormat::Qwen` — Uses `reasoning_content` field (Qwen variant)
- `ThinkingFormat::OpenRouter` — Uses `reasoning_details` array (OpenRouter extended thinking)

## Auth

Uses `Authorization: Bearer {api_key}` header. Extra headers can be added via `ModelConfig.headers`.
