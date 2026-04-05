<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `delay_for_attempt` *(src/provider/retry.rs)*

**Purpose:** Compute the sleep duration before a retry attempt using exponential backoff with jitter.

```
FUNCTION delay_for_attempt(config: RetryConfig, attempt: usize) -> Duration
  // attempt is 1-indexed
  base_ms ← config.initial_delay_ms * (config.backoff_multiplier ^ (attempt - 1))
  capped_ms ← min(base_ms, config.max_delay_ms)

  // ±20% uniform jitter: multiply by random value in [0.8, 1.2]
  jitter ← 0.8 + random_float_0_to_1() * 0.4
  delay_ms ← floor(capped_ms * jitter)

  RETURN Duration::from_ms(delay_ms)

  // Examples with defaults (initial=1000ms, multiplier=2.0, max=30000ms):
  //   attempt 1 → base=1000ms  → ~800–1200ms
  //   attempt 2 → base=2000ms  → ~1600–2400ms
  //   attempt 3 → base=4000ms  → ~3200–4800ms
END FUNCTION
```

---
