# Context Translation

Context translation solves a fundamental problem in multi-provider agent systems: when an agent switches providers mid-session, content types from the original provider may be silently dropped or cause errors on the new provider. The `ContextTranslationStrategy` trait provides a read-only translation layer that produces temporary copies of messages, never modifying the canonical history.

## Why it is needed

Different LLM providers support different content types. For example:

- **Anthropic** emits `Content::Thinking` blocks (chain-of-thought reasoning)
- **OpenAI** has no native thinking block format
- **Google/Bedrock** do not support thinking blocks at all

Without translation, switching from Anthropic to OpenAI mid-session would cause thinking blocks to be silently dropped or rejected. The agent loses reasoning context it previously produced.

## Design principles

### The canonical Message format IS the master layout

phi-core's `Message` enum (`User`, `Assistant`, `ToolResult`) and `Content` enum (`Text`, `Image`, `Thinking`, `ToolCall`) define the canonical format. All providers parse into this format and all session history is stored in it. Translation happens only at the boundary, right before messages are sent to a provider.

### Read-only translation

Translation produces **temporary copies** of the message slice. The original messages in `LoopRecord.messages` are never modified. This means:

- Session persistence always stores the full-fidelity canonical format
- Multiple providers can read the same history with different translations
- No information is permanently lost

### Lossless round-trip guarantee

Consider this scenario:

```
Turn 1-3: Anthropic (produces Content::Thinking blocks)
Turn 4:   Switch to OpenAI
Turn 5-6: Switch back to Anthropic
```

Here is what happens:

1. **Turns 1-3** are stored with full `Content::Thinking` blocks in canonical format.
2. **Turn 4**: Before calling OpenAI, the translator converts `Content::Thinking` to `Content::Text` prefixed with `[Reasoning]`. OpenAI sees text, not thinking blocks. The canonical history is untouched.
3. **Turns 5-6**: Back on Anthropic. The translator passes `Content::Thinking` through unchanged. Anthropic sees the original thinking blocks from turns 1-3 exactly as they were produced.

The original thinking blocks from turns 1-3 are never lost. They remain in the canonical history and are available whenever the session returns to a provider that supports them.

---

## Content type translation rules

The `DefaultContextTranslation` implementation applies these rules per target provider:

### Content::Thinking

| Target Provider | Translation |
|----------------|-------------|
| Anthropic | Kept as-is |
| OpenAI Completions | Converted to `Content::Text` with `[Reasoning]` prefix |
| OpenAI Responses | Converted to `Content::Text` with `[Reasoning]` prefix |
| Azure OpenAI | Converted to `Content::Text` with `[Reasoning]` prefix |
| Google Gemini | Dropped (unsupported) |
| Google Vertex | Dropped (unsupported) |
| Amazon Bedrock | Dropped (unsupported) |

### All other content types

`Content::Text`, `Content::Image`, and `Content::ToolCall` pass through unchanged for all providers.

### Message-level behavior

Only `Message::Assistant` messages are translated (since they are the only ones that carry provider-specific content types). `Message::User` and `Message::ToolResult` pass through unchanged.

---

## The ContextTranslationStrategy trait

```rust
pub trait ContextTranslationStrategy: Send + Sync {
    /// Translate a slice of messages for the given target provider protocol.
    fn translate_for_provider(&self, messages: &[Message], target: ApiProtocol) -> Vec<Message>;
}
```

The trait receives the full message slice and the target `ApiProtocol` enum variant. It returns a new `Vec<Message>` with translations applied.

### DefaultContextTranslation

The built-in implementation applies the content type rules described above. It is the default when no custom strategy is provided.

### Custom strategies

Implement the trait to define custom translation logic:

```rust
use phi_core::provider::context_translation::{ContextTranslationStrategy, DefaultContextTranslation};
use phi_core::provider::model::ApiProtocol;
use phi_core::types::content::Message;

struct MyTranslation;

impl ContextTranslationStrategy for MyTranslation {
    fn translate_for_provider(&self, messages: &[Message], target: ApiProtocol) -> Vec<Message> {
        // Custom logic here — e.g., strip all images for text-only providers
        // Fall back to default for everything else
        DefaultContextTranslation.translate_for_provider(messages, target)
    }
}
```

---

## Usage

### On AgentLoopConfig

Set the `context_translation` field to inject a strategy into the agent loop:

```rust
use std::sync::Arc;
use phi_core::agent_loop::AgentLoopConfig;
use phi_core::provider::context_translation::DefaultContextTranslation;
use phi_core::provider::ModelConfig;

let config = AgentLoopConfig {
    model_config: ModelConfig::openai("gpt-4o", "GPT-4o", &api_key),
    context_translation: Some(Arc::new(DefaultContextTranslation)),
    ..Default::default()
};
```

When `context_translation` is `Some`, the loop calls `translate_for_provider()` on the message slice before each LLM call. When `None`, messages are passed to the provider as-is.

### When to enable translation

Enable context translation when:

- Your agent may switch providers mid-session (e.g., using different models for different tasks)
- You are loading session history that was produced by a different provider
- You are running parallel sub-agents on different providers that share context

If your agent always uses a single provider, translation is unnecessary.
