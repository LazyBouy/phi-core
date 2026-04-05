<!-- Last verified: 2026-04-05 by Claude Code -->
# Google Gemini Provider

Two providers for Google's Gemini models:

- `GoogleProvider` — Google AI Studio (Generative AI API) via `ApiProtocol::GoogleGenerativeAi`
- `GoogleVertexProvider` — Google Cloud Vertex AI via `ApiProtocol::GoogleVertex`

## Google AI Studio

```rust
use phi_core::BasicAgent;
use phi_core::provider::ModelConfig;

let api_key = std::env::var("GOOGLE_API_KEY").unwrap();
let agent = BasicAgent::new(ModelConfig::google(
    "gemini-2.0-flash",
    "Gemini 2.0 Flash",
    &api_key,
));
```

### API Details

- **Endpoint**: `{base_url}/v1beta/models/{model}:streamGenerateContent?alt=sse&key={api_key}`
- **Auth**: API key as query parameter
- **Default base URL**: `https://generativelanguage.googleapis.com`
- **Default context window**: 1,000,000 tokens

### Message Format

Google uses a different message format than OpenAI/Anthropic:

| phi-core | Google API |
|----------|-----------|
| `user` role | `user` role |
| `assistant` role | `model` role |
| `Content::Text` | `{"text": "..."}` |
| `Content::Image` | `{"inlineData": {...}}` |
| `Content::ToolCall` | `{"functionCall": {...}}` |
| `Message::ToolResult` | `{"functionResponse": {...}}` |
| System prompt | `systemInstruction` field |
| Tools | `tools[].functionDeclarations[]` |

### Streaming

Uses SSE format (`alt=sse`). Each chunk contains `candidates` with `content.parts` and optional `usageMetadata`.

## Google Vertex AI

`GoogleVertexProvider` uses the same message format but with Vertex AI authentication and endpoints.

```rust
use phi_core::BasicAgent;
use phi_core::provider::{ModelConfig, ApiProtocol};

// Vertex AI uses OAuth2 Bearer tokens as the api_key
let access_token = get_access_token(); // your OAuth2 helper
let agent = BasicAgent::new(ModelConfig {
    id: "gemini-2.0-flash".into(),
    name: "Gemini 2.0 Flash (Vertex)".into(),
    api: ApiProtocol::GoogleVertex,
    provider: "google_vertex".into(),
    base_url: "https://us-central1-aiplatform.googleapis.com".into(),
    api_key: access_token,
    ..Default::default()
});
```

- **Protocol**: `ApiProtocol::GoogleVertex`
- **Auth**: OAuth2 / service account credentials (Bearer token in `api_key`)
- **Endpoint pattern**: `https://{region}-aiplatform.googleapis.com/v1/projects/{project}/locations/{region}/publishers/google/models/{model}:streamGenerateContent`
