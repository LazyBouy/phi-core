//! Provider registry — maps ApiProtocol to StreamProvider implementations.
/*
ARCHITECTURE: ProviderRegistry — the "factory + router" for LLM providers

The registry solves two problems:
  1. Factory — owns one instance of each `StreamProvider` implementation
  2. Router  — dispatches a stream() call to the right provider based on `ApiProtocol`

Usage pattern:
  let registry = ProviderRegistry::default(); // registers all 7 built-in providers
  registry.stream(&model_config, stream_config, tx, cancel).await

The caller never touches individual providers directly. It holds a `ProviderRegistry`
and lets the registry pick the right backend. This makes it trivial to:
  - Add a new provider: register it in `Default::default()`
  - Use a custom provider: build a custom registry with `new()` + `register()`

RUST QUIRK: `HashMap<ApiProtocol, Box<dyn StreamProvider>>` — heterogeneous collection

The registry must hold DIFFERENT concrete types (AnthropicProvider, GoogleProvider, ...)
behind a SINGLE common interface (`StreamProvider`). This is classic polymorphism.

`Box<dyn StreamProvider>` is a "trait object" — a heap-allocated pointer to a value
whose concrete type is erased. The `dyn` keyword means "dynamic dispatch" — method
calls go through a vtable (a table of function pointers) at runtime.

`ApiProtocol` is the key (an enum, implements Hash + Eq). For each protocol,
there's exactly one `Box<dyn StreamProvider>` value.
Python analogy:
  registry: dict[ApiProtocol, StreamProvider] = {
      ApiProtocol.ANTHROPIC: AnthropicProvider(),
      ...
  }
*/

use super::model::{ApiProtocol, ModelConfig};
use super::traits::*;
use crate::types::*;
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Registry of all available stream providers, keyed by API protocol.
pub struct ProviderRegistry {
    providers: HashMap<ApiProtocol, Box<dyn StreamProvider>>,
}

impl ProviderRegistry {
    /// Create an empty registry (no providers registered).
    /// Use `ProviderRegistry::default()` to get all built-in providers.
    pub fn new() -> Self {
        Self {
            providers: HashMap::new(),
        }
    }

    /// Register a provider for a given protocol.
    /*
    RUST QUIRK: `impl StreamProvider + 'static` — a generic bound with a lifetime constraint

    `impl StreamProvider` means "any type that implements StreamProvider" — the
    compiler generates a specific version of this method for each concrete type
    passed in (monomorphization). No virtual dispatch here; `Box::new(provider)` then
    erases the concrete type into `Box<dyn StreamProvider>`.

    `+ 'static` means "the type must not contain any borrowed references that could
    dangle." In practice: no `&'a str` fields. All the built-in providers (structs
    with no fields, or fields that own their data) satisfy this naturally.

    Why required? `Box<dyn StreamProvider>` is stored in `self.providers` which
    may outlive the current stack frame. Rust requires `'static` to guarantee the
    boxed value remains valid for as long as it's stored.

    `self.providers.insert(protocol, Box::new(provider))` — boxes the value onto the
    heap and inserts it. If a provider was already registered for this protocol,
    `insert` overwrites it (and the old Box is dropped).
    */
    pub fn register(&mut self, protocol: ApiProtocol, provider: impl StreamProvider + 'static) {
        self.providers.insert(protocol, Box::new(provider));
    }

    /// Get a reference to the provider for a given protocol, if registered.
    /*
    RUST QUIRK: `.map(|p| p.as_ref())` — `Box<dyn T>` → `&dyn T`

    `self.providers.get(protocol)` returns `Option<&Box<dyn StreamProvider>>`.
    We want to return `Option<&dyn StreamProvider>` (a reference to the trait object,
    not a reference to the Box that contains it).

    `p.as_ref()` on a `Box<T>` returns `&T` — it "peels off" the Box layer.
    Here: `Box<dyn StreamProvider>.as_ref()` → `&dyn StreamProvider`.

    Python analogy: just returning the value from the dict — no Box layer exists in Python.
    */
    pub fn get(&self, protocol: &ApiProtocol) -> Option<&dyn StreamProvider> {
        self.providers.get(protocol).map(|p| p.as_ref())
    }

    /// Returns true if a provider is registered for the given protocol.
    pub fn has(&self, protocol: &ApiProtocol) -> bool {
        self.providers.contains_key(protocol)
    }

    /// List all protocols that have a registered provider.
    /*
    RUST QUIRK: `.keys().copied().collect()` — iterator chain on HashMap keys

    `.keys()` — returns an iterator over `&ApiProtocol` (references to the keys)
    `.copied()` — converts `&ApiProtocol` to `ApiProtocol` (valid because ApiProtocol
                  implements Copy — it's a small enum with no heap data)
    `.collect()` — consumes the iterator and builds a `Vec<ApiProtocol>`
                   Rust infers the collection type from the return type `Vec<ApiProtocol>`

    Python analogy: list(registry.keys())
    */
    pub fn protocols(&self) -> Vec<ApiProtocol> {
        self.providers.keys().copied().collect()
    }

    /// Stream using the appropriate provider for the model's API protocol.
    /*
    ARCHITECTURE: The dispatch method — routes to the right backend

    This is the primary entry point. It:
      1. Looks up the provider by `model.api` (the ApiProtocol enum variant)
      2. Returns `ProviderError::Other` if no provider is registered for that protocol
      3. Delegates to `provider.stream()` — the actual HTTP+SSE call

    `ok_or_else(|| ...)` converts `Option<&dyn StreamProvider>` → `Result<...>`:
      `Some(provider)` → `Ok(provider)`
      `None`           → `Err(ProviderError::Other("No provider registered for..."))`
    The `?` then propagates the Err early.

    `provider.stream(config, tx, cancel).await` — async call through the trait object.
    The vtable dispatches to the concrete method (AnthropicProvider::stream, etc.)
    at runtime. The `.await` suspends this task until the stream completes.
    */
    /*
    DESIGN: Why `model` is separate from `config`
      `model`  = ROUTING KEY — tells the registry WHICH provider to dispatch to (via model.api)
      `config` = REQUEST PAYLOAD — forwarded unchanged to the selected provider
    The registry itself never reads config; it just routes based on model.api, then passes
    config through. Separating them makes the routing logic clear and config unopinionated.
    */
    pub async fn stream(
        &self,
        model: &ModelConfig, // ROUTER — model.api selects the provider; also carries base_url, headers
        config: StreamConfig, // PAYLOAD — forwarded as-is to the selected provider
        tx: mpsc::UnboundedSender<StreamEvent>, // OBSERVER — passed through to provider.stream()
        cancel: tokio_util::sync::CancellationToken, // ABORT — passed through to provider.stream()
    ) -> Result<Message, ProviderError> {
        let provider = self.providers.get(&model.api).ok_or_else(|| {
            ProviderError::Other(format!(
                "No provider registered for protocol: {}",
                model.api
            ))
        })?;

        provider.stream(config, tx, cancel).await
    }
}

impl Default for ProviderRegistry {
    /// Create a registry with all 7 built-in providers registered.
    /*
    ARCHITECTURE: `Default` as a convenient "all batteries included" factory

    `ProviderRegistry::default()` is the recommended way to get a working registry.
    It registers every built-in provider. Custom apps that need to restrict which
    providers are available can use `ProviderRegistry::new()` and register selectively.

    RUST QUIRK: `use` inside a function — scoped imports
    `use crate::provider::{ AnthropicProvider, ... }` inside the function body
    is a scoped import. The names are only available within this block, which avoids
    polluting the module's namespace with 7 provider names. Especially useful when
    the providers are large dependencies only needed in one place.
    */
    fn default() -> Self {
        use crate::provider::{
            AnthropicProvider, AzureOpenAiProvider, BedrockProvider, GoogleProvider,
            GoogleVertexProvider, OpenAiCompatProvider, OpenAiResponsesProvider,
        };

        let mut registry = Self::new();
        registry.register(ApiProtocol::AnthropicMessages, AnthropicProvider);
        registry.register(ApiProtocol::OpenAiCompletions, OpenAiCompatProvider);
        registry.register(ApiProtocol::OpenAiResponses, OpenAiResponsesProvider);
        registry.register(ApiProtocol::GoogleGenerativeAi, GoogleProvider);
        registry.register(ApiProtocol::GoogleVertex, GoogleVertexProvider);
        registry.register(ApiProtocol::BedrockConverseStream, BedrockProvider);
        registry.register(ApiProtocol::AzureOpenAiResponses, AzureOpenAiProvider);

        registry
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_registry_has_all_providers() {
        let registry = ProviderRegistry::default();

        assert!(registry.has(&ApiProtocol::AnthropicMessages));
        assert!(registry.has(&ApiProtocol::OpenAiCompletions));
        assert!(registry.has(&ApiProtocol::OpenAiResponses));
        assert!(registry.has(&ApiProtocol::GoogleGenerativeAi));
        assert!(registry.has(&ApiProtocol::GoogleVertex));
        assert!(registry.has(&ApiProtocol::BedrockConverseStream));
        assert!(registry.has(&ApiProtocol::AzureOpenAiResponses));
    }

    #[test]
    fn test_registry_protocols() {
        let registry = ProviderRegistry::default();
        let protocols = registry.protocols();
        assert_eq!(protocols.len(), 7);
    }

    #[test]
    fn test_custom_registry() {
        let mut registry = ProviderRegistry::new();
        assert!(!registry.has(&ApiProtocol::AnthropicMessages));

        registry.register(
            ApiProtocol::AnthropicMessages,
            crate::provider::AnthropicProvider,
        );
        assert!(registry.has(&ApiProtocol::AnthropicMessages));
    }
}
