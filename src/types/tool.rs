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
}
