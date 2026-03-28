use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Execution limits
// ---------------------------------------------------------------------------

/*
ExecutionLimits — a safety net against runaway agent loops.

Without limits, a poorly-designed tool or a confused LLM could loop forever,
burning tokens and money. These three limits provide defense-in-depth:

  max_turns    — catches infinite tool-call loops
  max_total_tokens — catches token budget overruns (cost control)
  max_duration — catches wall-clock hangs (e.g., a bash tool that blocks)

The agent loop checks these BEFORE each turn (in ExecutionTracker::check_limits).
When a limit is hit, it injects a "[Agent stopped: ...]" user message into the
conversation so the LLM (and user) can see what happened, then returns.

RUST QUIRK: `std::time::Duration`

Duration is Rust's type for a span of time (not a point in time — that's Instant/SystemTime).
Constructors:
  Duration::from_secs(600)   → 10 minutes
  Duration::from_millis(100) → 100ms
  Duration::from_nanos(1)    → 1 nanosecond

Internally, Duration is stored as (seconds: u64, nanoseconds: u32) — no floating point,
no overflow risk for reasonable values.

The full path `std::time::Duration` is used instead of a `use` import because it appears
only in this one struct — no need to pollute the module namespace.
*/
/// Execution limits for the agent loop — guards against infinite loops and budget overruns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionLimits {
    /// Maximum number of LLM turns (one turn = one LLM call + its tool results)
    pub max_turns: usize,
    /// Maximum total tokens consumed across all turns (input + output)
    pub max_total_tokens: usize,
    /// Maximum wall-clock duration. Uses std::time::Duration (not f64 seconds) for precision.
    pub max_duration: std::time::Duration,
    /// Maximum cumulative dollar cost for the run. `None` means no cost cap.
    /// Requires `AgentLoopConfig.cost_config` to be set — without pricing rates the
    /// accumulated cost is always 0.0 and this limit has no effect.
    #[serde(default)]
    pub max_cost: Option<f64>,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            max_turns: 50,
            max_total_tokens: 1_000_000,
            max_duration: std::time::Duration::from_secs(600),
            max_cost: None,
        }
    }
}

/// Tracks execution state against limits
pub struct ExecutionTracker {
    pub limits: ExecutionLimits,
    pub turns: usize,
    pub tokens_used: usize,
    /// Accumulated dollar cost across all turns. Updated via `record_cost()`.
    /// Only non-zero when `AgentLoopConfig.cost_config` is set.
    pub cost_accumulated: f64,
    pub started_at: std::time::Instant,
}

impl ExecutionTracker {
    pub fn new(limits: ExecutionLimits) -> Self {
        Self {
            limits,
            turns: 0,
            tokens_used: 0,
            cost_accumulated: 0.0,
            started_at: std::time::Instant::now(),
        }
    }

    pub fn record_turn(&mut self, tokens: usize) {
        self.turns += 1;
        self.tokens_used += tokens;
    }

    /// Accumulate incremental cost for the current turn.
    pub fn record_cost(&mut self, cost: f64) {
        self.cost_accumulated += cost;
    }

    /// Check if any limit has been exceeded. Returns the reason if so.
    /*
    RUST QUIRK: `Option<String>` as "either an error reason, or nothing"

    `check_limits()` returns:
      Some("Max turns reached (50/50)")  ← a limit was hit
      None                                ← all limits OK

    This is the Rust way to return "optional data" — no exceptions, no sentinel values (-1, ""),
    no separate boolean + string pair. The caller pattern-matches to handle both cases.

    RUST QUIRK: `Instant::elapsed()` for wall-clock timing

    `std::time::Instant` records a moment in time (monotonic clock, not wall clock).
    Monotonic means it never goes backwards — safe to use for durations.
    `started_at.elapsed()` returns a `Duration` = current time - started_at.

    The `>=` comparison between two Durations works because Duration implements PartialOrd.

    RUST QUIRK: `{:.0}` format specifier — zero decimal places for f64

    `format!("Max duration reached ({:.0}s/{:.0}s)", elapsed.as_secs_f64(), ...)`
    `{:.0}` means "format as float with 0 decimal places" → "42" not "42.000000"
    Other examples: {:.2} = 2 decimal places, {:>10.3} = right-aligned, 10 wide, 3 decimal places
    */
    pub fn check_limits(&self) -> Option<String> {
        if self.turns >= self.limits.max_turns {
            return Some(format!(
                "Max turns reached ({}/{})",
                self.turns, self.limits.max_turns
            ));
        }
        if self.tokens_used >= self.limits.max_total_tokens {
            return Some(format!(
                "Max tokens reached ({}/{})",
                self.tokens_used, self.limits.max_total_tokens
            ));
        }
        let elapsed = self.started_at.elapsed(); // Duration since ExecutionTracker::new()
        if elapsed >= self.limits.max_duration {
            return Some(format!(
                "Max duration reached ({:.0}s/{:.0}s)", // {:.0} = 0 decimal places
                elapsed.as_secs_f64(),
                self.limits.max_duration.as_secs_f64()
            ));
        }
        if let Some(max) = self.limits.max_cost {
            if self.cost_accumulated >= max {
                return Some(format!(
                    "Max cost reached (${:.4}/${:.4})",
                    self.cost_accumulated, max
                ));
            }
        }
        None // All limits OK — return None (no reason to stop)
    }
}
