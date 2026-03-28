use crate::provider::CostConfig;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Usage metrics for cost tracking and cache hit rate calculation.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    /*
    #[serde(default)] means: if this field is missing during deserialization, use its default value instead of erroring.
    To set a custom default, you point serde to a custom function:

    fn default_cache_read() -> u64 { 100 }  // some custom default
    #[serde(default = "default_cache_read")]
    pub cache_read: u64,
    */
    /// Reasoning / thinking tokens — a subset of `output`.
    /// Non-zero only for providers that report reasoning tokens separately
    /// (OpenAI o-series via `completion_tokens_details.reasoning_tokens`,
    /// OpenAI Responses API via `output_token_details.reasoning_tokens`).
    /// Defaults to 0 for all other providers.
    #[serde(default)]
    pub reasoning: u64,
    #[serde(default)]
    pub cache_read: u64,
    #[serde(default)]
    pub cache_write: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

impl Usage {
    /// Estimated dollar cost for this usage given per-million-token rates.
    ///
    /// `reasoning` tokens are already counted in `output`, so they are not
    /// double-charged — the breakdown is purely informational.
    pub fn estimated_cost(&self, cost: &CostConfig) -> f64 {
        (self.input as f64 / 1_000_000.0) * cost.input_per_million
            + (self.output as f64 / 1_000_000.0) * cost.output_per_million
            + (self.cache_read as f64 / 1_000_000.0) * cost.cache_read_per_million
            + (self.cache_write as f64 / 1_000_000.0) * cost.cache_write_per_million
    }

    /// Add two `Usage` values together (e.g., sum across parallel branches or multi-step loops).
    pub fn combine(&self, other: &Usage) -> Usage {
        Usage {
            input: self.input + other.input,
            output: self.output + other.output,
            reasoning: self.reasoning + other.reasoning,
            cache_read: self.cache_read + other.cache_read,
            cache_write: self.cache_write + other.cache_write,
            total_tokens: self.total_tokens + other.total_tokens,
        }
    }

    /// Fraction of input tokens served from cache (0.0–1.0).
    /// Returns 0.0 if no input tokens were processed.
    pub fn cache_hit_rate(&self) -> f64 {
        let total_input = self.input + self.cache_read + self.cache_write;
        if total_input == 0 {
            return 0.0; // early return — guard against division by zero
        }
        /*
        RUST QUIRK: Explicit numeric casting with `as`

        Unlike Python, Rust never silently promotes integer types to float.
        self.cache_read and total_input are u64 (unsigned 64-bit integer).
        Division of u64 / u64 = u64 (integer division, truncates decimals).
        To get a fractional result, we must explicitly cast to f64 first.

          self.cache_read as f64   ← safe widening cast: u64 → f64, no data loss
          total_input as f64       ← same

        Python equivalent: float(self.cache_read) / float(total_input)

        `as` in Rust is the casting keyword. For widening casts (small → large type)
        it is always safe. For narrowing casts (large → small), it silently truncates
        (no panic), so be careful when casting from f64 → u64.
        */
        self.cache_read as f64 / total_input as f64
    }
}

// ---------------------------------------------------------------------------
// Cache configuration
// ---------------------------------------------------------------------------

/// Controls prompt caching behavior for providers that support it.
///
/// By default, caching is enabled with automatic breakpoint placement.
/// This gives optimal cost savings without any user configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CacheConfig {
    /// Master switch — set to false to disable all caching hints.
    /// Default: true.
    pub enabled: bool,
    /// How cache breakpoints are placed.
    pub strategy: CacheStrategy,
}

/*
RUST QUIRK: impl Default

Rust does NOT allow default field values in struct definitions (unlike C++/Python dataclasses).
Instead, you implement the `Default` trait, which defines a single method `default() -> Self`
that constructs the "zeroed/sensible" value.

Python analogy:
    @dataclass
    class CacheConfig:
        enabled: bool = True
        strategy: CacheStrategy = CacheStrategy.Auto

    # In Python, defaults live in the field definition.
    # In Rust, defaults live in `impl Default`.

Usage:
    let cfg = CacheConfig::default();    // explicit
    let cfg: CacheConfig = Default::default(); // via trait
    let cfg = CacheConfig { enabled: false, ..Default::default() }; // spread/override
    // The `..Default::default()` is Rust's "struct update syntax" —
    // fill all unspecified fields from the default value.

When you #[derive(Default)], Rust auto-generates this impl by calling `.default()`
on each field. We implement it manually here because we want non-zero defaults
(enabled: true, strategy: Auto) instead of (false, first-variant).
*/
impl Default for CacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            strategy: CacheStrategy::Auto,
        }
    }
}

// --------------------------------------------------------------------------
// Cache strategies for providers that support EXPLICIT prompt caching
// (e.g., Anthropic)
// --------------------------------------------------------------------------
/// Strategy for placing cache breakpoints (currently Anthropic-specific;
/// other providers handle caching automatically regardless of this setting).
/// In Future, a provider specific implementation of the agent loop
/// can inspect this setting and apply it to any provider that supports
/// explicit caching hints.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum CacheStrategy {
    /// Automatic breakpoint placement (recommended).
    /// Caches: system prompt, tool definitions, and recent conversation history.
    #[default]
    Auto,
    /// Disable caching entirely.
    Disabled,
    /// Fine-grained control over what gets cached.
    Manual {
        // Inline struct variants or just struct variants are used when you want to group multiple related fields together under a single variant.
        // In this case, Manual has three boolean fields that control different aspects of caching, so it makes sense to group them together.
        // Optional fields must be defines as Option<T> — this allows them to be None (not provided) or Some(value).
        /// Cache the system prompt.
        cache_system: bool,
        /// Cache tool definitions.
        cache_tools: bool,
        /// Cache conversation history (second-to-last message).
        cache_messages: bool,
    },
}

// ---------------------------------------------------------------------------
// Thinking level
// ---------------------------------------------------------------------------

/// Controls the depth of model reasoning before responding.
///
/// - `Off`     — No thinking tokens; pure response, fastest & cheapest.
/// - `Minimal` — Lightest reasoning pass; good for simple tool calls.
/// - `Low`     — Shallow chain-of-thought; for moderately complex tasks.
/// - `Medium`  — Balanced reasoning; default for most agentic workflows.
/// - `High`    — Maximum reasoning budget; for complex multi-step planning.
///   Most expensive — use for hard coding/logic problems only.
///
/// The exact token budgets behind each level are mapped per provider in
/// their respective StreamProvider implementations, and may evolve over time
/// as we optimize for cost vs performance.

/*
RUST QUIRK: Copy vs Clone

`Copy` is a marker trait for types that can be duplicated by just copying their bits
(no heap allocation involved). When a type is Copy, assignment does NOT move ownership —
it silently makes a duplicate.

    let level = ThinkingLevel::Off;
    let also_level = level;   // ← if NOT Copy: move (level is gone)
    let also_level = level;   // ← if Copy: silent bitwise copy (both valid)

ThinkingLevel is a simple enum with no heap data — all variants are just tags.
So it gets `Copy`. This means you can pass it around freely without `clone()` calls.

`Clone` (also derived here) is the explicit, potentially expensive version:
  .clone()  ←  always works, may allocate
  Copy      ←  implicit, zero-cost, only for stack-only types

Python has no equivalent — everything is a reference by default. The closest
analogy is an IntEnum: copying an IntEnum gives you an independent value,
not a shared reference.

`#[default]` on `Off` tells `#[derive(Default)]` which variant to use
when Default::default() is called. Without it, Default would not compile
on an enum (there's no obvious "zero value" for arbitrary enums).
*/
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingLevel {
    /// No chain-of-thought tokens. Fastest and cheapest.
    #[default]
    Off,
    Minimal,
    Low,
    Medium,
    High,
}
