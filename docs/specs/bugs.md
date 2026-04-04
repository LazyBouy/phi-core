# Known Bugs

Tracked bugs in phi-core. Mark with ~~strikethrough~~ and `[FIXED]` when resolved.

---

## ~~BUG-001: OpenAI-compat provider swallows HTTP 429 as Ok(Message) instead of Err(RateLimited)~~ `[FIXED]`

**File:** `src/provider/openai_compat.rs` line ~316-329

**Problem:** When the SSE connection receives an HTTP 429 (rate limit) error, the `Some(Err(e))` branch constructs an `Ok(Message)` with `StopReason::Error` and returns it. The retry logic in `agent_loop` only retries on `Err(ProviderError::RateLimited)` — it never sees the 429 because it's wrapped in `Ok`.

**Fix:** SSE errors now classify 429/rate-limit as `Err(ProviderError::RateLimited)` and 502/503/504 as `Err(ProviderError::Network)`, enabling the retry loop to handle them.

**Found:** 2026-04-04 | **Fixed:** 2026-04-04

---

## ~~BUG-002: `file:` prefix not resolved in raw `agent.system_prompt` or `agent.profile.system_prompt`~~ `[FIXED]`

**File:** `src/config/builder.rs` — `resolve_system_prompt()`

**Problem:** `resolve_system_prompt()` only resolved `file:` paths through the 3-entity chain. Direct `system_prompt = "file:prompt.md"` was treated as literal text.

**Fix:** Added `file:` prefix check at the top of `resolve_system_prompt()`, before the `{{...}}` reference check. Relative paths resolve from workspace directory.

**Found:** 2026-04-04 | **Fixed:** 2026-04-04

---

## ~~BUG-003: Profile instance system_prompt not used in resolution chain~~ `[FIXED]`

**File:** `src/config/builder.rs` — `build_basic_agent()` lines 267-275

**Problem:** When `agents_from_config()` resolved a profile instance (via `agent_profile = "{{...}}"`), the instance's `system_prompt` was stored in the `AgentProfile` struct but never read by the system prompt resolution chain. The chain only checked `config.agent.system_prompt` and `config.agent.profile.system_prompt`, skipping the resolved profile instance.

**Fix:** Added `profile_override.and_then(|p| p.system_prompt.as_deref())` to the resolution chain between agent-level and base profile.

Resolution order: agent override > profile instance > base profile > empty.

**Found:** 2026-04-04 | **Fixed:** 2026-04-04

---

## ~~BUG-004: Per-instance workspace not supported~~ `[FIXED]`

**Files:** `src/config/schema.rs`, `src/config/builder.rs`

**Problem:** `AgentInstanceSection` had no `workspace` field. All agent instances shared `config.agent.workspace`, preventing per-agent workspace directories for `file:` resolution.

**Fix:** Added `workspace: Option<String>` to `AgentInstanceSection`. Added `workspace_override: Option<&str>` parameter to `build_basic_agent()`. Resolution: instance workspace > agent workspace > default_workspace > ".".

**Found:** 2026-04-04 | **Fixed:** 2026-04-04
