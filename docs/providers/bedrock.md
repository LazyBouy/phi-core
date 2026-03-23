# Amazon Bedrock Provider

Handles the AWS Bedrock ConverseStream API. Selected automatically when
`ModelConfig.api == ApiProtocol::BedrockConverseStream`.

## Usage

```rust
use phi_core::BasicAgent;
use phi_core::provider::{ModelConfig, ApiProtocol};

// With static credentials in api_key: "ACCESS_KEY:SECRET_KEY" or "ACCESS_KEY:SECRET_KEY:SESSION_TOKEN"
let creds = std::env::var("AWS_BEDROCK_CREDENTIALS").unwrap_or_default();
let agent = BasicAgent::new(ModelConfig {
    id: "anthropic.claude-3-sonnet-20240229-v1:0".into(),
    name: "Claude Sonnet (Bedrock)".into(),
    api: ApiProtocol::BedrockConverseStream,
    provider: "bedrock".into(),
    base_url: "https://bedrock-runtime.us-east-1.amazonaws.com".into(),
    api_key: creds, // "access_key:secret_key[:session_token]", or "" for IAM roles
    ..Default::default()
});
```

## Authentication

The `api_key` field uses a colon-separated format:

```
{access_key_id}:{secret_access_key}
{access_key_id}:{secret_access_key}:{session_token}
```

For IAM roles (e.g., EC2 instance profiles, ECS task roles), pass an empty `api_key` and provide
pre-computed `Authorization` headers via `ModelConfig.headers`.

## API Details

- **Endpoint**: `{base_url}/model/{model}/converse-stream`
- **Default base URL**: `https://bedrock-runtime.us-east-1.amazonaws.com`
- **Protocol**: `ApiProtocol::BedrockConverseStream`

## Message Format

Bedrock uses its own content block format:

| phi-core | Bedrock API |
|----------|-------------|
| `Content::Text` | `{"text": "..."}` |
| `Content::Image` | `{"image": {"format": "...", "source": {"bytes": "..."}}}` |
| `Content::ToolCall` | `{"toolUse": {"toolUseId": "...", "name": "...", "input": ...}}` |
| `Message::ToolResult` | `{"toolResult": {"toolUseId": "...", "content": [...], "status": "success"}}` |
| System prompt | `system` array of text blocks |
| Tools | `toolConfig.tools[].toolSpec` |
| Max tokens | `inferenceConfig.maxTokens` |

## Stream Events

Bedrock's ConverseStream returns these event types:

- `contentBlockStart` — New content block (text or tool use)
- `contentBlockDelta` — Text or tool use input delta
- `contentBlockStop` — Block complete
- `messageStop` — Stop reason (`end_turn`, `max_tokens`, `tool_use`)
- `metadata` — Token usage
