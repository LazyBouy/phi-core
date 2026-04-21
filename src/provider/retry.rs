//! Retry with exponential backoff and jitter for provider calls.
//!
//! ARCHITECTURE NOTE: Why retry at the agent loop level (not the provider level)?
//!
//! Retrying is a cross-cutting concern — all 7 providers share the same retry logic.
//! By handling it in stream_assistant_response() (agent_loop.rs), we avoid duplicating
//! retry logic in every provider. Providers simply return ProviderError::RateLimited
//! or ProviderError::Network, and this module decides what to do.

use crate::provider::ProviderError;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tracing::warn;

/// Configuration for automatic retry of transient provider errors.
///
/// Defaults: 3 retries, 1s initial delay, 2x backoff, 30s max delay.
/// Use `RetryConfig::none()` to disable retries entirely.
/*
ARCHITECTURE: Exponential backoff + jitter — why both?

Exponential backoff (delay doubles each attempt) prevents thundering herd:
if 1000 clients all hit a rate limit at the same time and all retry after
exactly 1s, they'll hit the limit again simultaneously. Doubling adds space.

Jitter (±20% random noise) prevents synchronized retries even with backoff:
two clients with the same delay would still retry at the same moment.
With jitter, their windows are offset, reducing server load spikes.

Attempt 1: 1000ms * (0.8–1.2) = 800–1200ms
Attempt 2: 2000ms * (0.8–1.2) = 1600–2400ms
Attempt 3: 4000ms * (0.8–1.2) = 3200–4800ms → capped at 30s
*/
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryConfig {
    /// Maximum number of retry attempts (0 = no retries, fail immediately).
    pub max_retries: usize,
    /// Delay before the first retry in milliseconds (e.g., 1000 = 1 second).
    pub initial_delay_ms: u64,
    /// Multiplier applied each attempt: delay[n] = initial * multiplier^(n-1).
    /// 2.0 = double each time (standard exponential backoff).
    pub backoff_multiplier: f64,
    /// Maximum delay cap in milliseconds — backoff stops growing beyond this.
    pub max_delay_ms: u64,
}

/*
RUST QUIRK: `impl Default` — explicitly defining the "zero value"

Rust has no constructor syntax. Instead, the `Default` trait provides `::default()`.
We implement it manually here because the defaults are non-trivial constants.

If all fields were 0/false/empty-string, we could use `#[derive(Default)]` to get
it for free. But initial_delay_ms = 1000 and backoff_multiplier = 2.0 are not
the "zero values" of u64 and f64 (which are 0 and 0.0 respectively).

Usage examples:
  let cfg = RetryConfig::default();               // 3 retries, 1s delay, 2x backoff
  let cfg = RetryConfig { max_retries: 5, ..Default::default() }; // override one field
  let cfg = RetryConfig::none();                  // 0 retries (disable retries)
*/
impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            initial_delay_ms: 1000,
            backoff_multiplier: 2.0,
            max_delay_ms: 30_000, // numeric literal underscores: _ is ignored, just a readability separator
        }
    }
}

impl RetryConfig {
    /// No retries — fail immediately on any error.
    /*
    RUST QUIRK: `..Default::default()` — struct update syntax

    `Self { max_retries: 0, ..Default::default() }` means:
      "construct a Self where max_retries = 0,
       and all other fields come from Default::default()"

    It's Rust's equivalent of Python's dataclasses.replace():
      dataclasses.replace(RetryConfig(), max_retries=0)

    This lets RetryConfig::none() reuse the sensible defaults for all other
    fields, and only override what matters (max_retries = 0).
    The order matters: named fields come first, `..expr` must be last.
    */
    pub fn none() -> Self {
        Self {
            max_retries: 0,
            ..Default::default() // fill remaining fields from the default
        }
    }

    /// Calculate the delay for a given attempt (1-indexed).
    /// Uses exponential backoff with ±20% jitter.
    /*
    RUST QUIRK: Mixed numeric types require explicit casting (`as`)

    `self.initial_delay_ms` is u64 (unsigned integer).
    `self.backoff_multiplier` is f64 (floating point).
    Rust won't mix them — you must explicitly cast.

    `self.initial_delay_ms as f64` — widening cast: u64 → f64 (safe, no data loss)
    `(attempt - 1) as i32` — narrowing cast: usize → i32 (safe for small values)
    `(capped_ms * jitter) as u64` — narrowing cast: f64 → u64 (truncates fraction, no panic)

    `powi(n: i32)` — integer power for f64:
      2.0_f64.powi(3) = 8.0
      This is more precise than powf(n as f64) for integer exponents.

    `base_ms.min(max)` — clamp from above:
      Python analogy: min(base_ms, self.max_delay_ms as f64)

    `rand::random::<f64>()` — generate a random f64 in [0.0, 1.0).
      The `::<f64>` is a "turbofish" — explicit type parameter at the call site.
      Needed because random() is generic and Rust can't always infer the type.
      Python analogy: random.random()
    */
    pub fn delay_for_attempt(&self, attempt: usize) -> Duration {
        // base_ms: initial_delay * multiplier^(attempt-1)
        // attempt is 1-indexed: attempt 1 → multiplier^0 = 1.0 → no extra delay
        let base_ms =
            self.initial_delay_ms as f64 * self.backoff_multiplier.powi((attempt - 1) as i32);
        let capped_ms = base_ms.min(self.max_delay_ms as f64); // cap at max_delay_ms

        // Jitter: multiply by a random factor in [0.8, 1.2) = ±20% noise
        let jitter = 0.8 + rand::random::<f64>() * 0.4; // 0.4 range → [0.8, 1.2)
        Duration::from_millis((capped_ms * jitter) as u64) // f64 → u64: truncates, never panics
    }
}

/*
RUST QUIRK: Adding methods to a type defined in another module — `impl OtherType`

`ProviderError` is defined in provider/traits.rs, but we add retry-related methods
to it HERE in retry.rs. Rust allows this as long as either:
  a) The type is defined in THIS crate (ProviderError is — it's in our crate)
  b) You own the trait being implemented

This is different from Python where methods must live in the class definition.
In Rust, you can add methods to your own types from any module in the crate.
The split is intentional: ProviderError is a pure type in provider/traits.rs;
retry logic lives here in retry.rs (separation of concerns).
*/
impl ProviderError {
    /// Whether this error is safe to retry.
    ///
    /// Retryable: rate limits (429) and network/transient errors.
    /// Not retryable: auth errors, API errors (bad request), cancellation.
    /*
    RUST QUIRK: `matches!` macro — compact pattern matching returning bool

    `matches!(self, Pattern1 | Pattern2)` is shorthand for:
      match self {
          Pattern1 | Pattern2 => true,
          _ => false,
      }

    The `..` inside `RateLimited { .. }` means "I don't care about the fields,
    just check that it's this variant." It matches any RateLimited value.

    Python analogy: isinstance(self, (RateLimited, Network))
    */
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::RateLimited { .. } | Self::Network(_))
    }

    /// If this is a rate limit with a server-specified retry delay, return it.
    /*
    ARCHITECTURE: Respecting server-specified retry delays (Retry-After header)

    When an API returns HTTP 429 with a `Retry-After: 60` header, we should
    wait exactly that long — not our computed backoff. The server knows its
    own rate limit windows better than we do.

    `retry_after_ms: Some(ms)` — only matches if the field is Some (not None).
    `*ms` — dereferences the &u64 to get the u64 value.
    `Duration::from_millis(*ms)` — wraps it in a Duration for the caller.

    The caller in agent_loop.rs uses:
      e.retry_after().unwrap_or_else(|| retry.delay_for_attempt(attempt))
    meaning: "use server's delay if available, else compute our own."
    */
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited {
                retry_after_ms: Some(ms), // match guard: only if retry_after_ms is Some
            } => Some(Duration::from_millis(*ms)),
            _ => None, // all other cases (including RateLimited { retry_after_ms: None })
        }
    }
}

/// Log a retry attempt.
/*
RUST QUIRK: `pub(crate)` visibility — "public within this crate only"

`pub(crate)` is between `pub` (visible everywhere) and private (visible only in this module).
It means: "any module in this crate can use this function, but external consumers cannot."

Here, log_retry is called from agent_loop.rs (same crate) but shouldn't be part of
the public API that library users see.

`warn!()` is a logging macro from the `tracing` crate (similar to Python's logging.warning()).
It uses the same syntax as println! but routes to the tracing subscriber configured by the user.
`{:.1}` format: float with 1 decimal place → "2.5" not "2.500000"
*/
pub(crate) fn log_retry(
    attempt: usize, // CURRENT — which attempt just failed (1-indexed; printed as "attempt X/Y")
    max: usize,     // TOTAL   — RetryConfig::max_retries (the denominator in "attempt X/Y")
    delay: &Duration, // WAIT    — computed backoff delay before the next attempt (shown in seconds)
    error: &ProviderError, // CAUSE   — the error that triggered this retry (shown in the log message)
) {
    warn!(
        "Provider error (attempt {}/{}), retrying in {:.1}s: {}",
        attempt,
        max,
        delay.as_secs_f64(), // Duration → f64 seconds (e.g., 1500ms → 1.5)
        error                // uses ProviderError's Display impl
    );
}
