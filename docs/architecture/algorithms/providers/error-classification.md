<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `ProviderError::classify` *(src/provider/traits.rs)*

**Purpose:** Map an HTTP error response to the correct `ProviderError` variant.

```
FUNCTION ProviderError::classify(status: u16, message: String) -> ProviderError

  IF is_context_overflow(status, message) THEN
    RETURN ContextOverflow { message }
  END IF

  IF status == 429 THEN
    RETURN RateLimited { retry_after_ms: None }
  END IF

  IF status == 401 OR status == 403 THEN
    RETURN Auth(message)
  END IF

  RETURN Api(message)

END FUNCTION

FUNCTION is_context_overflow(status: u16, message: String) -> bool
  // Some providers (Cerebras, Mistral) return 400/413 with empty body
  IF (status == 400 OR status == 413) AND message.trim() is empty THEN
    RETURN true
  END IF
  lower ← message.to_lowercase()
  RETURN any of OVERFLOW_PHRASES is a substring of lower

  // OVERFLOW_PHRASES includes:
  //   "prompt is too long"          (Anthropic)
  //   "input is too long"           (Bedrock)
  //   "exceeds the context window"  (OpenAI)
  //   "exceeds the maximum"         (Google)
  //   "maximum prompt length"       (xAI)
  //   "reduce the length of the messages" (Groq)
  //   "maximum context length"      (OpenRouter)
  //   "context length exceeded"     (generic)
  //   "too many tokens"             (generic)
  //   ... 15 phrases total

END FUNCTION
```

---
