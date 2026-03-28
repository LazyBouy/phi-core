use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Compaction scope
// ---------------------------------------------------------------------------

/// Controls how many earlier loops are included in compaction and context loading.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompactionScope {
    /// Compact a fixed number of earlier loops on the active chain.
    FixedCount(usize),
    /// Walk the chain backward, accumulating per-loop token estimates,
    /// and stop when `max_context_tokens` would be exceeded.
    ///
    /// NOTE: The scope can include loops whose raw messages EXCEED
    /// `max_context_tokens`. This is intentional — the compacted summaries
    /// of those loops will fit in the window, even though the originals
    /// did not. This enables richer context for summarisation strategies
    /// (e.g. LLM summarisers) that compress large loops into compact
    /// representations that then fit within the budget.
    TokenBudget,
}

impl Default for CompactionScope {
    fn default() -> Self {
        Self::FixedCount(3)
    }
}

// ---------------------------------------------------------------------------
// Compaction configuration
// ---------------------------------------------------------------------------

/// Full compaction policy — controls both WHEN and HOW to compact.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactionConfig {
    // ── WHEN to compact ──
    /// Fraction of `max_context_tokens` below which headroom is measured.
    /// Compaction fires when headroom drops below `compact_budget_threshold_pct`.
    /// Default: 0.90 (90%).
    pub compact_at_pct: f64,
    /// Minimum remaining headroom fraction before compaction fires.
    /// Default: 0.05 (5%). With defaults at 100k/4k: fires at ~81k tokens.
    pub compact_budget_threshold_pct: f64,
    /// Scope controlling how many earlier loops to compact and load.
    /// Default: `FixedCount(3)`.
    pub compaction_scope: CompactionScope,

    // ── HOW to compact ──
    /// Turns to keep verbatim from the start (most recent loop only). Default: 2.
    pub keep_first_turns: usize,
    /// Minimum turns to keep from the end (most recent loop only).
    /// Extended to turn boundary so ToolCall/ToolResult pairs are never split.
    /// Default: 10.
    pub keep_recent_turns: usize,
    /// Token budget for the summarised middle section. Default: 2_000.
    ///
    /// This is a budget, not a per-turn limit. Implementations of
    /// `BlockCompactionStrategy::keep_compacted()` should aim to summarise
    /// ALL turns in the range within this budget — e.g. by producing shorter
    /// per-turn summaries or an LLM-generated holistic digest.
    /// `DefaultBlockCompaction` is a basic implementation that generates
    /// per-turn one-liners and drops remaining turns when the budget runs out.
    pub max_summary_tokens: usize,
    /// Max lines per tool output in the keep_recent section. Default: 50.
    pub tool_output_max_lines: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            compact_at_pct: 0.90,
            compact_budget_threshold_pct: 0.05,
            compaction_scope: CompactionScope::default(),
            keep_first_turns: 2,
            keep_recent_turns: 10,
            max_summary_tokens: 2_000,
            tool_output_max_lines: 50,
        }
    }
}

// ---------------------------------------------------------------------------
// Context configuration
// ---------------------------------------------------------------------------

/// Configuration for context management — model constraints + compaction policy.
///
/// `CompactionConfig` is a required field: if you set a context limit,
/// compaction is always ready with sensible defaults. Compaction as a whole
/// is disabled by setting `context_config: None` on `AgentLoopConfig`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextConfig {
    /// Maximum context tokens (the model's context window).
    pub max_context_tokens: usize,
    /// Tokens reserved for the system prompt.
    pub system_prompt_tokens: usize,
    /// Compaction policy — always present when context limits are set.
    pub compaction: CompactionConfig,

    // Legacy fields — kept for backward compatibility with existing configs.
    // New code should use `compaction.*` instead.
    #[serde(default)]
    pub keep_recent: usize,
    #[serde(default)]
    pub keep_first: usize,
    #[serde(default)]
    pub tool_output_max_lines: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_context_tokens: 100_000,
            system_prompt_tokens: 4_000,
            compaction: CompactionConfig::default(),
            keep_recent: 10,
            keep_first: 2,
            tool_output_max_lines: 50,
        }
    }
}
