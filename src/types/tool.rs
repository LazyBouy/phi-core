use super::content::Content;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Tool execution strategy
// ---------------------------------------------------------------------------

/// Controls how multiple tool calls from a single LLM response are executed.
///
/// When the LLM returns multiple tool calls (e.g., "read file A, read file B,
/// run bash C"), this determines whether they run sequentially or in parallel.
/// Who Chooses the Strategy?
/// The caller/developer sets it at agent construction time — it's a config on
/// the Agent struct, not a per-turn LLM decision . The LLM has no awareness of it whatsoever.
/// The practical decision rule is straightforward:
/// Parallel — default, stateless tools (file reads, web fetches)
/// Sequential — tools with shared mutable state (DB writes, shell with side effects)
/// Batched — human-in-the-loop flows with periodic steering checkpoints without fully serializing execution
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ToolExecutionStrategy {
    /// Run tools one at a time, check steering between each.
    /// Use for debugging or tools with shared mutable state.
    Sequential,
    /// Run all tool calls concurrently, check steering after all complete.
    /// Default — most tool calls are independent and this gives the best latency.
    #[default]
    Parallel,
    /// Run in batches of N (N tools in parallel, wait for all N to finish),check steering between batches.
    /// Balances speed with human-in-the-loop control.
    //  Steering: steering is the human-in-the-loop interrupt mechanism .
    //  It's a check point where the agent asks: "has the human sent
    //  a new instruction, cancellation, or correction since the last batch finished?"
    Batched { size: usize },
}

// ---------------------------------------------------------------------------
// Tool Usage and execution context
// ---------------------------------------------------------------------------

/// Callback for streaming partial results during tool execution.
///
/// Tools call this to emit progress updates (e.g., partial output, status messages)
/// that are forwarded as `AgentEvent::ToolExecutionUpdate` events for UI consumption.
/// Partial results are **not** sent to the LLM — only the final `ToolResult` is.
/*  dyn → "I don't know the concrete type at compile time"
//  Fn(ToolResult) → "a function (callable like Python Lambda) that takes a ToolResult as input and returns () (nothing)"
//  Arc<...> → (Atomically Reference Counted) A "thread-safe reference-counted pointer to this function, allowing it to be shared across threads safely"
//  Reference counting means the Arc tracks how many owners currently hold a reference to the value .
    Thread A clones Arc  → count = 2
    Thread B clones Arc  → count = 3
    Thread A drops it    → count = 2
    Thread B drops it    → count = 1
    Original drops it    → count = 0 → memory freed
//  In Rust, one can't just share a raw pointer across threads — Arc wraps the value and keeps a reference count, freeing it when the last owner drops it.
//  Send + Sync → "the function can be safely sent to and called from multiple threads"
//  Send = "safe to move to another thread"
//  Sync = "safe to share a reference across threads"
//  In Rust, the Send and Sync are derived "almost" for free.
//  You only lose it when you use inherently non-thread-safe types like Rc<T> (use Arc<T> instead) or RefCell<T> (use Mutex<T> instead).
//  "+" = "must implement ALL of these"
//  */
pub type ToolUpdateFn = Arc<dyn Fn(ToolResult) + Send + Sync>;

/// Callback for emitting user-facing progress messages during tool execution.
///
/// Each invocation emits an `AgentEvent::ProgressMessage` event. Unlike `ToolUpdateFn`,
/// these are simple text messages intended for user-facing display (e.g., status lines,
/// notifications), not structured tool results.
pub type ProgressFn = Arc<dyn Fn(String) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResult {
    /*
    The struct ToolResult is a generic result container (content + freeform details) used at execution time — what a tool hands back to the runtime.
    The Message::ToolResult enum variant is a LLM-protocol record — it adds tool_call_id, tool_name, is_error,
    timestamp which are needed for LLM correlation, not tool execution .
    So the runtime transforms struct ToolResult → Message::ToolResult by enriching it with correlation metadata before it enters the LLM conversation.
    */
    pub content: Vec<Content>,
    #[serde(default)]
    pub details: serde_json::Value,
    /// Set by sub-agent tools to the child loop's `loop_id` after `agent_loop()` returns.
    /// `None` for all regular (non-sub-agent) tools.
    /// Propagated to `AgentEvent::ToolExecutionEnd.child_loop_id` so the parent event stream
    /// can reference the child loop without parsing tool result content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_loop_id: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /*
    {0} means "insert the first (and only) field of this enum variant" —
    so Failed("disk full") would display as "disk full", and NotFound("bash") displays as "Tool not found: bash"
    */
    #[error("{0}")]
    Failed(String),
    #[error("Tool not found: {0}")]
    NotFound(String),
    #[error("Invalid arguments: {0}")]
    InvalidArgs(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("Tool exceeded timeout of {duration:?}")]
    Timeout { duration: std::time::Duration },
}

/// Context passed to tool execution. Bundles all per-invocation state.
///
/// Using a struct instead of individual parameters future-proofs the trait —
/// adding fields to `ToolContext` is non-breaking.
pub struct ToolContext {
    /// The ID of this tool call (for correlation).
    pub tool_call_id: String,
    /// The name of the tool being invoked.
    pub tool_name: String,
    /// Cancellation token — check `is_cancelled()` in long-running tools.
    pub cancel: tokio_util::sync::CancellationToken,
    /// Optional callback for streaming partial `ToolResult`s (UI/logging only).
    /*
    How it works in practice:
    // Somewhere in agent_loop.rs, BEFORE calling the tool:
    let (tx, rx) = channel();  // a pipe

    let on_update = Arc::new(move |partial: ToolResult| {
        tx.send(AgentEvent::ToolExecutionUpdate(partial)); // ← pushes into channel
    });

    // Tool receives the context with on_update already wired:
    let ctx = ToolContext { on_update: Some(on_update), .. };
    tool.execute(params, ctx).await;

    // Meanwhile, the event stream consumer reads from rx → sends to UI

    (tx, rx) is the pipe (the channel)

    ToolUpdateFn is the handle the tool receives — an Arc-wrapped callable that internally calls tx.send(...)

    The tool calls the handle
    → handle pushes into tx
    → wrapping it as AgentEvent::ToolExecutionUpdate(partial)
    → runtime reads from rx
    → dispatches as it wishes (UI, logging, etc.)

    */
    pub on_update: Option<ToolUpdateFn>,
    /// Optional callback for emitting user-facing progress messages.
    pub on_progress: Option<ProgressFn>,
}

impl Clone for ToolContext {
    /*
    In Rust, there is no automatic copying of complex structs — when you assign or pass a struct, ownership moves, meaning the original is gone.
    For ToolContext, we want to be able to clone it (e.g., if multiple tool calls share the same context), so we implement the Clone trait manually.
    When multiple tools run in parallel, each tool must own its own ToolContext instance (its own cancel token, its own on_progress callback) —
    you can't move one context into two threads simultaneously . clone() is what makes that fan-out safe.

    Clone is a trait (like From<Message>) that allows for explicit duplication of values.
    By implementing Clone for ToolContext, we can create independent copies of the context for each tool execution,
    ensuring thread safety and proper ownership semantics in concurrent scenarios.
    A Trait in Rust gets implemented by the "for" preposition — impl TraitName for Type { ... }
    — this is how you say "this type implements this trait, and here are the method definitions that fulfill the trait's contract".

    An example of traits and its usage in Rust:

    // Define the trait (the ABC skeleton)
    trait Summarizable {
        fn summary(&self) -> String;                  // must implement
        fn label(&self) -> String {                   // optional default
            String::from("item")
        }
    }

    // Implement it for a struct
    struct AgentEvent { text: String }

    impl Summarizable for AgentEvent {
        fn summary(&self) -> String {
            format!("Event: {}", self.text)           // must provide this
        }
        // label() is inherited from default
    }

    */
    fn clone(&self) -> Self {
        Self {
            tool_call_id: self.tool_call_id.clone(),
            tool_name: self.tool_name.clone(),
            cancel: self.cancel.clone(),
            on_update: self.on_update.clone(),
            on_progress: self.on_progress.clone(),
        }
    }
}

impl fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        /*
        <'_> — Lifetime Elision Syntax: In Rust, when you see a lifetime annotation like <'_>,
        it means "I want the compiler to infer the appropriate lifetime for this reference."
        "how long a borrow is valid"
         In the context of the Debug implementation for ToolContext, the on_update and on_progress fields are Arc-wrapped function pointers (callbacks).
         These callbacks may capture references to data with specific lifetimes, but since we're only printing a placeholder ("<callback>") instead of the actual function,
         we don't need to worry about their lifetimes here. The <'_> allows us to implement Debug without having to specify or manage those lifetimes explicitly.

        &self.on_update.as_ref().map(|_| "<callback>")
        This line is a clever way to indicate in the debug output that there is a callback present without trying to print the actual function pointer
        (which isn't useful and can be complex).
        on_update is Option<Arc<dyn Fn(...)>> — an optional function pointer wrapped in an Arc (shared pointer) . The logic here is:

            .as_ref() — peek at the Option without consuming it

            .map(|_| "<callback>") — if it exists, replace it with the string "<callback>" for display purposes; if None, stays None

            The |_| is a closure that ignores its argument (the actual Arc) and just returns the label "<callback>" to indicate that a callback is present.

        Why? Because you can't Debug-print a function/closure — so this substitutes a human-readable placeholder.
        "<callback>" is just a string literal they chose; it has no special Rust meaning.
        */
        f.debug_struct("ToolContext")
            .field("tool_call_id", &self.tool_call_id)
            .field("tool_name", &self.tool_name)
            .field("cancel", &self.cancel)
            .field("on_update", &self.on_update.as_ref().map(|_| "<callback>"))
            .field(
                "on_progress",
                &self.on_progress.as_ref().map(|_| "<callback>"),
            )
            .finish()
    }
}

/// A tool the agent can call. Implement this trait for your tools.
/*
JUST A NOTE ON ASYNC TRAITS:
Rust traits natively can't have async fn methods (pre-2024 stable),
so this macro rewrites your async fn execute(...) into a fn execute(...) -> Pin<Box<dyn Future>> under the hood.
*/
#[async_trait::async_trait]
pub trait AgentTool: Send + Sync {
    /// Unique tool name (used in LLM tool_use)
    fn name(&self) -> &str;
    /// Human-readable label for UI
    fn label(&self) -> &str;
    /// Description for the LLM
    fn description(&self) -> &str;
    /// JSON Schema for parameters
    fn parameters_schema(&self) -> serde_json::Value;
    /// Execute the tool.
    ///
    /// The `ctx` parameter provides per-invocation context:
    /// - `ctx.tool_call_id` / `ctx.tool_name` — for correlation and logging
    /// - `ctx.cancel` — cancellation token; check `is_cancelled()` in long-running tools
    /// - `ctx.on_update` — optional callback for streaming partial `ToolResult`s (UI/logging only)
    /// - `ctx.on_progress` — optional callback for user-facing progress text (`ProgressMessage`)
    /*
    DESIGN: Why `params` AND `ctx` are separate parameters — input vs environment
      `params`  = LLM INPUT    — the JSON arguments the LLM chose to pass this invocation;
                                 varies per call (e.g. {"cmd": "ls -la"} one call, {"cmd": "pwd"} next)
      `ctx`     = SYSTEM ENV   — plumbing injected by the agent loop; same shape for every tool:
                                 cancel token, on_update callback, on_progress callback, call identifiers
    The separation keeps tools clean: a tool's execute() only needs to know about its own args
    (params), while the system concerns (how to cancel, how to stream updates) are injected via ctx.
    Python analogy: params ~ **kwargs from the caller; ctx ~ a request context object from the framework.
    */
    async fn execute(
        &self,
        params: serde_json::Value, // LLM INPUT — JSON args chosen by the LLM; validated inside execute()
        ctx: ToolContext, // SYSTEM ENV — cancel token + streaming callbacks from the agent loop
    ) -> Result<ToolResult, ToolError>;

    /// Optional per-tool execution timeout.
    ///
    /// Resolution order at dispatch time:
    /// 1. This per-tool override (if `Some`)
    /// 2. `AgentLoopConfig.tool_timeout` (if `Some`)
    /// 3. `None` — no per-tool timeout (loop-level limits still apply)
    ///
    /// On timeout, the agent loop fires the tool's child cancel token (best-effort
    /// cooperative cancellation) and synthesises a `ToolError::Timeout` result so the
    /// LLM sees the failure and can self-correct without the agent loop aborting.
    fn timeout(&self) -> Option<std::time::Duration> {
        None
    }

    /// Short, model-facing one-liner for the turn-1 tool catalog (KC-05, #110).
    ///
    /// This is the description a progressive-catalog agent sends on the wire — it
    /// must be lean enough to pay on every turn (see
    /// [`SHORT_DESCRIPTION_MAX_CHARS`]). Defaults to [`description`](Self::description)
    /// so every existing tool renders identically until it opts into a real split.
    /// Tools with a large mental model (e.g. `revert_to_state`) override this with a
    /// one-liner and move the full manual to [`detailed_description`](Self::detailed_description)
    /// / `tool_help`.
    fn short_description(&self) -> &str {
        self.description()
    }

    /// Full extended manual, served on demand via `tool_help` (KC-05, #110).
    ///
    /// `None` (the default) means the tool has no separate detailed body — its
    /// [`description`](Self::description) is already complete. A tool that splits its
    /// documentation returns the full manual here (the same body a `tool_help(<name>)`
    /// call surfaces) while keeping [`short_description`](Self::short_description) lean.
    fn detailed_description(&self) -> Option<&str> {
        None
    }

    /// Whether this tool carries a large parameter schema worth deferring — the
    /// token-magnification case the progressive tool-catalog keys on.
    ///
    /// Consumed by the progressive-catalog `engage_on_large_schema` trigger
    /// ([`ProgressiveToolCatalog`](crate::agent_loop::ProgressiveToolCatalog)): an
    /// agent that attaches such tools benefits most from a reduced catalog because
    /// their schemas are the P0 magnification case — MCP `inputSchema`s and
    /// OpenAPI-generated parameter schemas. Defaults to `false`; `McpToolAdapter`
    /// and `OpenApiToolAdapter` override to `true`. This is a general
    /// schema-magnitude signal, not consumer policy.
    fn has_large_schema(&self) -> bool {
        false
    }
}

/// Maximum length (in `char`s) of a tool's [`short_description`](AgentTool::short_description)
/// — the model-facing one-liner in the turn-1 catalog (KC-05, #110).
///
/// A description the model reads is load-bearing, so an over-long short description
/// fails [`validate_tool_registration`] by default (the F1.a hard-error stance) rather
/// than being silently truncated on the wire. Kernel built-ins are 122–196 chars;
/// 256 leaves comfortable headroom above the longest (`edit_file`, 196) and forces
/// only the intended `revert_to_state` split.
pub const SHORT_DESCRIPTION_MAX_CHARS: usize = 256;

/// Error returned by [`validate_tool_registration`] when a tool violates the
/// kernel tool-registration contract (KC-05, #110).
///
/// Hard-error is the kernel DEFAULT stance (F1.a). The STANCE is a
/// consumer-overridable default (F2.b): a consumer that prefers truncate-with-warn
/// may ignore the `Err` and truncate itself — phi-core bakes in no policy beyond
/// exposing this primitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolRegistrationError {
    /// The tool's [`name`](AgentTool::name) is empty.
    MissingName,
    /// The tool's [`short_description`](AgentTool::short_description) is empty.
    MissingShortDescription {
        /// The offending tool's name.
        tool: String,
    },
    /// The tool's [`short_description`](AgentTool::short_description) exceeds
    /// [`SHORT_DESCRIPTION_MAX_CHARS`].
    ShortDescriptionTooLong {
        /// The offending tool's name.
        tool: String,
        /// Actual `short_description` length in `char`s.
        len: usize,
        /// The configured maximum ([`SHORT_DESCRIPTION_MAX_CHARS`]).
        max: usize,
    },
}

impl std::fmt::Display for ToolRegistrationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ToolRegistrationError::MissingName => {
                write!(f, "tool registration invalid: name() is empty")
            }
            ToolRegistrationError::MissingShortDescription { tool } => {
                write!(
                    f,
                    "tool {tool:?} registration invalid: short_description() is empty"
                )
            }
            ToolRegistrationError::ShortDescriptionTooLong { tool, len, max } => {
                write!(
                    f,
                    "tool {tool:?} registration invalid: short_description() is {len} chars, exceeds the {max}-char limit"
                )
            }
        }
    }
}

impl std::error::Error for ToolRegistrationError {}

/// Validate a tool against the kernel tool-registration contract (KC-05, #110).
///
/// The kernel DEFAULT stance is hard-error (F1.a): an empty [`name`](AgentTool::name),
/// an empty [`short_description`](AgentTool::short_description), or a short description
/// exceeding [`SHORT_DESCRIPTION_MAX_CHARS`] returns `Err`. A missing
/// [`detailed_description`](AgentTool::detailed_description) is NOT an error — it is a
/// non-fatal advisory surfaced by [`tool_registration_warning`].
///
/// phi-core exposes this primitive but does not force-invoke it on the hot path, so
/// no policy is baked into the kernel (F2.b): a consumer calls this to opt into the
/// hard-error stance, or ignores it and truncates/warns per its own policy.
pub fn validate_tool_registration(tool: &dyn AgentTool) -> Result<(), ToolRegistrationError> {
    if tool.name().is_empty() {
        return Err(ToolRegistrationError::MissingName);
    }
    let short = tool.short_description();
    if short.is_empty() {
        return Err(ToolRegistrationError::MissingShortDescription {
            tool: tool.name().to_string(),
        });
    }
    let len = short.chars().count();
    if len > SHORT_DESCRIPTION_MAX_CHARS {
        return Err(ToolRegistrationError::ShortDescriptionTooLong {
            tool: tool.name().to_string(),
            len,
            max: SHORT_DESCRIPTION_MAX_CHARS,
        });
    }
    Ok(())
}

/// Non-fatal registration advisory for a tool (KC-05, #110).
///
/// Returns `Some(message)` when the tool has no
/// [`detailed_description`](AgentTool::detailed_description) — `tool_help(<name>)`
/// will fall back to its short description for that tool. This is advisory only
/// (never fails a build); the F1.a hard-error cases are surfaced by
/// [`validate_tool_registration`] instead.
pub fn tool_registration_warning(tool: &dyn AgentTool) -> Option<String> {
    if tool.detailed_description().is_none() {
        Some(format!(
            "tool {:?} has no detailed_description(); tool_help falls back to its short description",
            tool.name()
        ))
    } else {
        None
    }
}

#[cfg(test)]
mod kc05_registration_tests {
    use super::*;

    /// A tool that overrides ONLY the pre-KC-05 surface (name/label/description/
    /// schema/execute) and inherits all three new default methods — the backcompat
    /// shape every one of the 15 existing impls has.
    struct PlainTool {
        name: String,
        desc: String,
    }

    #[async_trait::async_trait]
    impl AgentTool for PlainTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn label(&self) -> &str {
            "Plain"
        }
        fn description(&self) -> &str {
            &self.desc
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    /// A tool that supplies a real short/detailed split.
    struct SplitTool {
        short: String,
        detailed: String,
    }

    #[async_trait::async_trait]
    impl AgentTool for SplitTool {
        fn name(&self) -> &str {
            "split"
        }
        fn label(&self) -> &str {
            "Split"
        }
        fn description(&self) -> &str {
            &self.detailed
        }
        fn short_description(&self) -> &str {
            &self.short
        }
        fn detailed_description(&self) -> Option<&str> {
            Some(&self.detailed)
        }
        fn parameters_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        async fn execute(
            &self,
            _params: serde_json::Value,
            _ctx: ToolContext,
        ) -> Result<ToolResult, ToolError> {
            Ok(ToolResult {
                content: vec![],
                details: serde_json::Value::Null,
                child_loop_id: None,
            })
        }
    }

    // ── Tier A — trait split defaults + backcompat ──────────────────────────

    #[test]
    fn short_description_defaults_to_description() {
        let t = PlainTool {
            name: "plain".into(),
            desc: "does a plain thing".into(),
        };
        // A tool overriding neither renders `short_description()` == `description()`.
        assert_eq!(t.short_description(), t.description());
        assert_eq!(t.short_description(), "does a plain thing");
    }

    #[test]
    fn detailed_description_defaults_to_none() {
        let t = PlainTool {
            name: "plain".into(),
            desc: "does a plain thing".into(),
        };
        assert!(t.detailed_description().is_none());
        // has_large_schema also defaults false (backcompat schema-magnitude signal).
        assert!(!t.has_large_schema());
    }

    #[test]
    fn split_override_supplies_short_and_detailed() {
        let t = SplitTool {
            short: "one-liner".into(),
            detailed: "the full manual with all the depth".into(),
        };
        assert_eq!(t.short_description(), "one-liner");
        assert_eq!(
            t.detailed_description(),
            Some("the full manual with all the depth")
        );
        // description() is the full body; short is the lean wire form.
        assert_ne!(t.short_description(), t.description());
    }

    // ── Tier E — registration validation (F1.a hard-error / F2.b overridable) ─

    #[test]
    fn validation_passes_for_valid_tool() {
        let t = SplitTool {
            short: "a compliant short description".into(),
            detailed: "the full manual".into(),
        };
        assert!(validate_tool_registration(&t).is_ok());
        // Valid + has a detailed body ⇒ no advisory warning.
        assert!(tool_registration_warning(&t).is_none());
    }

    #[test]
    fn validation_errors_on_over_limit_short() {
        let t = SplitTool {
            short: "x".repeat(SHORT_DESCRIPTION_MAX_CHARS + 1),
            detailed: "body".into(),
        };
        match validate_tool_registration(&t) {
            Err(ToolRegistrationError::ShortDescriptionTooLong { len, max, .. }) => {
                assert_eq!(len, SHORT_DESCRIPTION_MAX_CHARS + 1);
                assert_eq!(max, SHORT_DESCRIPTION_MAX_CHARS);
            }
            other => panic!("expected ShortDescriptionTooLong, got {other:?}"),
        }
    }

    #[test]
    fn validation_errors_on_missing_short_or_name() {
        // Empty short_description ⇒ MissingShortDescription.
        let empty_short = SplitTool {
            short: String::new(),
            detailed: "body".into(),
        };
        assert!(matches!(
            validate_tool_registration(&empty_short),
            Err(ToolRegistrationError::MissingShortDescription { .. })
        ));
        // Empty name ⇒ MissingName (short inherits description here).
        let empty_name = PlainTool {
            name: String::new(),
            desc: "non-empty".into(),
        };
        assert!(matches!(
            validate_tool_registration(&empty_name),
            Err(ToolRegistrationError::MissingName)
        ));
    }

    #[test]
    fn validation_warns_on_missing_detailed() {
        // A tool with no detailed_description() passes hard validation but yields
        // an advisory warning (never an error).
        let t = PlainTool {
            name: "plain".into(),
            desc: "does a plain thing".into(),
        };
        assert!(validate_tool_registration(&t).is_ok());
        let warn = tool_registration_warning(&t);
        assert!(warn.is_some(), "expected a missing-detailed advisory");
        assert!(warn.unwrap().contains("detailed_description"));
    }
}
