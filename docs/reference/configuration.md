# Configuration

## AgentLoopConfig

The main configuration for the agent loop:

```rust
pub struct AgentLoopConfig {
    /// REQUIRED — Complete provider identity: model id, api_key, base_url, protocol, compat flags, cost rates.
    /// Provider is resolved from model_config.api via ProviderRegistry.
    pub model_config: ModelConfig,
    /// Custom provider override. When Some, bypasses ProviderRegistry. Use for MockProvider in tests.
    pub provider_override: Option<Arc<dyn StreamProvider>>,
    pub config_id: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub get_steering_messages: Option<GetMessagesFn>,
    pub get_follow_up_messages: Option<GetMessagesFn>,
    pub context_config: Option<ContextConfig>,
    pub compaction_strategy: Option<Arc<dyn CompactionStrategy>>,
    pub execution_limits: Option<ExecutionLimits>,
    pub cache_config: CacheConfig,
    pub tool_execution: ToolExecutionStrategy,
    pub retry_config: RetryConfig,
    pub before_loop: Option<BeforeLoopFn>,
    pub after_loop: Option<AfterLoopFn>,
    pub before_turn: Option<BeforeTurnFn>,
    pub after_turn: Option<AfterTurnFn>,
    pub on_error: Option<OnErrorFn>,
    pub before_tool_execution: Option<BeforeToolExecutionFn>,
    pub after_tool_execution: Option<AfterToolExecutionFn>,
    pub before_tool_execution_update: Option<BeforeToolExecutionUpdateFn>,
    pub after_tool_execution_update: Option<AfterToolExecutionUpdateFn>,
    pub input_filters: Vec<Arc<dyn InputFilter>>,
    pub first_turn_trigger: TurnTrigger,
}
```

## StreamConfig

Internal config passed to `StreamProvider::stream()`. All provider identity comes from `model_config`:

```rust
pub struct StreamConfig {
    /// REQUIRED — full provider identity: id, api_key, base_url, compat, cost.
    pub model_config: ModelConfig,
    pub system_prompt: String,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDefinition>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,  // overrides model_config.max_tokens when Some
    pub temperature: Option<f32>,
    pub cache_config: CacheConfig,
}
```

## ContextConfig

Controls context window compaction:

```rust
pub struct ContextConfig {
    pub max_context_tokens: usize,      // Default: 100,000
    pub system_prompt_tokens: usize,    // Default: 4,000
    pub keep_recent: usize,             // Default: 10
    pub keep_first: usize,             // Default: 2
    pub tool_output_max_lines: usize,   // Default: 50
}
```

## ExecutionLimits

Prevents runaway agents:

```rust
pub struct ExecutionLimits {
    pub max_turns: usize,              // Default: 50
    pub max_total_tokens: usize,       // Default: 1,000,000
    pub max_duration: Duration,        // Default: 600s
}
```

## ThinkingLevel

```rust
pub enum ThinkingLevel {
    Off,        // No thinking (default)
    Minimal,    // 128 tokens (Anthropic budget)
    Low,        // 512 tokens
    Medium,     // 2,048 tokens
    High,       // 8,192 tokens
}
```

## CostConfig

Token pricing per million:

```rust
pub struct CostConfig {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_write_per_million: f64,
}
```
