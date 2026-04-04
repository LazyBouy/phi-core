# Configuration

## AgentLoopConfig

The main configuration for the agent loop:

```rust
pub struct AgentLoopConfig {
    /// REQUIRED — Complete provider identity: model id, api_key, base_url, protocol, compat flags, cost rates.
    pub model_config: ModelConfig,
    /// Custom provider override. When Some, bypasses ProviderRegistry. Use for MockProvider in tests.
    pub provider_override: Option<Arc<dyn StreamProvider>>,
    /// Stable config identity for loop_id generation.
    pub config_id: Option<String>,
    pub thinking_level: ThinkingLevel,
    pub max_tokens: Option<u32>,
    pub temperature: Option<f32>,
    pub convert_to_llm: Option<ConvertToLlmFn>,
    pub transform_context: Option<TransformContextFn>,
    pub get_steering_messages: Option<GetMessagesFn>,
    pub get_follow_up_messages: Option<GetMessagesFn>,
    /// Context config (includes CompactionConfig with strategies and token counter).
    pub context_config: Option<ContextConfig>,
    pub execution_limits: Option<ExecutionLimits>,
    pub cache_config: CacheConfig,
    pub tool_execution: ToolExecutionStrategy,
    pub retry_config: RetryConfig,
    // ── Lifecycle callbacks ──
    pub before_turn: Option<BeforeTurnFn>,
    pub after_turn: Option<AfterTurnFn>,
    pub before_loop: Option<BeforeLoopFn>,
    pub after_loop: Option<AfterLoopFn>,
    pub before_tool_execution: Option<BeforeToolExecutionFn>,
    pub after_tool_execution: Option<AfterToolExecutionFn>,
    pub before_tool_execution_update: Option<BeforeToolExecutionUpdateFn>,
    pub after_tool_execution_update: Option<AfterToolExecutionUpdateFn>,
    /// Compaction lifecycle callbacks (G1).
    pub before_compaction_start: Option<BeforeCompactionStartFn>,
    pub after_compaction_end: Option<AfterCompactionEndFn>,
    pub on_error: Option<OnErrorFn>,
    pub input_filters: Vec<Arc<dyn InputFilter>>,
    pub first_turn_trigger: TurnTrigger,
    /// Context translation strategy for cross-provider compatibility (G8).
    pub context_translation: Option<Arc<dyn ContextTranslationStrategy>>,
    /// Shared state for PrunTool to communicate pruning requests to the loop.
    pub prun_pending: Option<Arc<Mutex<Vec<PrunRequest>>>>,
}
```

> **Note:** Compaction strategies (`in_memory_strategy`, `block_strategy`) are fields on `CompactionConfig` (inside `ContextConfig`), not on `AgentLoopConfig`. The `token_counter` for pluggable token counting is also on `ContextConfig`.

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

Controls context window management and compaction:

```rust
pub struct ContextConfig {
    pub max_context_tokens: usize,                          // Default: 100,000
    pub system_prompt_tokens: usize,                        // Default: 4,000
    pub compaction: CompactionConfig,                       // Full compaction policy (nested)
    pub token_counter: Option<Arc<dyn TokenCounter>>,       // Pluggable token counting (REQ-162)
    // Legacy fields (backward compat — use compaction.* instead):
    pub keep_recent: usize,                                 // Default: 10
    pub keep_first: usize,                                  // Default: 2
    pub tool_output_max_lines: usize,                       // Default: 50
}
```

### CompactionConfig

```rust
pub struct CompactionConfig {
    // WHEN to compact:
    pub compact_at_pct: f64,                                // Default: 0.90
    pub compact_budget_threshold_pct: f64,                  // Default: 0.05
    pub compaction_scope: CompactionScope,                  // Default: FixedCount(3)
    // HOW to compact:
    pub keep_first_turns: usize,                            // Default: 2
    pub keep_recent_turns: usize,                           // Default: 10
    pub max_summary_tokens: usize,                          // Default: 2,000
    pub tool_output_max_lines: usize,                       // Default: 50
    pub focus_message: Option<String>,                      // Guides summarization focus
    // Strategy objects (G5 — moved from AgentLoopConfig):
    pub in_memory_strategy: Option<Arc<dyn CompactionStrategy>>,
    pub block_strategy: Option<Arc<dyn BlockCompactionStrategy>>,
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
