<!-- Last verified: 2026-05-23 by Claude Code -->
# Composition I — the braking layer

Composition I is phi-core's **opt-in braking layer** [EXISTS]. It lets the
agent abandon failed or finished branches of its own conversation between
turns, keeping the active LLM context lean while the forensic record stays
intact.

> **Braking is NOT compaction.** Composition I sits *above*
> [`BlockCompactionStrategy`](compaction.md) and the in-memory
> `compact_messages` path. It **delays** how often compaction must run; it
> never replaces it. The full design discussion lives in
> [`concept-brake.md`](concept-brake.md).

The braking layer is **dormant unless the consumer opts in** via
`BasicAgent::with_revert_tool()`. An upgrade from 0.7.x to 0.8.0 that does
not call this method sees no behavioural change.

## The three layers

1. **Forensic JSONL** — `LoopRecord.messages` keeps every message ever
   stamped. Reverts never delete from this log. A session replay reconstructs
   the full tree.
2. **Working trunk** — the parent-chain walk in
   [`AgentContext::build_trunk_context`](../reference/api.md) assembles the
   LLM-facing view by following `parent_id` links from the active node back
   to a root. Abandoned spans are simply off-path.
3. **Annotation layer** — `NodeTag`s attached to on-trunk nodes carry the
   one-line summaries the agent supplied at revert-time, plus a kind that
   drives the render policy (lessons decay; outcomes stay pinned).

## The `revert_to_state` tool

When `with_revert_tool()` is enabled, the LLM sees a `revert_to_state` tool
with this schema:

```json
{
  "category": "failure | tangent | completion | step-summary",
  "step":     "n12 | 12",
  "summary":  "(optional) one-line distillation"
}
```

The agent calls it inline whenever it decides to abandon work. Example:

```
[n10] User: write a fast sorting algorithm
[n11] Assistant: I'll try bubble sort
[n12] Tool result: error: execution timed out

→ Assistant calls revert_to_state(
    category="failure",
    step="n10",
    summary="bubble sort (O(n²)) timed out — try a faster algorithm")
```

The tool itself only **enqueues** a `RevertRequest`. The actual revert is
applied between turns by `apply_revert` — synchronous, no LLM call, mirrors
the deferred pattern from `PrunTool` / `apply_prun`.

After the drain runs, the next turn's prompt is rebuilt by walking
`parent_id` from the new active node (n10) — the failed n11/n12 span is
simply absent. The lesson rides along as a `NodeTag` on n10.

## The four categories

| Category | `TagKind` | Render policy |
|---|---|---|
| `failure` | `Lesson` | Decays per [`RevertRenderPolicy`] |
| `tangent` | `Finding` | Decays per [`RevertRenderPolicy`] |
| `completion` | `Outcome` | Pinned while on-trunk |
| `step-summary` | `Checkpoint` | Pinned while on-trunk |

`failure` and `tangent` are about *resetting* — the agent learned something
on a dead-end branch; the lesson is most useful in the immediate window
after it happened. `completion` and `step-summary` are about *condensing* —
the agent finished real work; that outcome is the only surviving
representation of it and must stay visible.

## The opt-in guarantee

Without `BasicAgent::with_revert_tool()`:

- `RevertTool` is **never instantiated**, **never registered**, **never
  converted into a `ToolDefinition`**, and **never advertised to the LLM**
  in the API request.
- `AgentLoopConfig.revert_pending` is `None`, so the `apply_revert` drain
  is gated off — even a manually-injected `RevertRequest` would be ignored.
- `AgentContext.active_node_id` stays `None`, so
  `build_working_context()` takes the byte-identical linear path it took in
  0.7.x.

There is no back-door. This is verified end-to-end by
`tests/revert_test.rs::opt_in_guarantee_without_with_revert_tool`.

## The rejection rules (0.8.0)

`apply_revert` is conservative in 0.8.0; it emits
`RevertApplied { applied: false, reason: …, .. }` and makes no mutation
when:

- The `step` does not resolve to a known node in `context.messages`
  (`"revert target n<N> not found"`).
- The abandoned span (every message strictly after the target node) contains
  a `Message::User` (`"revert refused: abandoned span contains a user
  message"`). The guarantee: **the agent cannot silently pretend you didn't
  speak**. Auto-rebase of the user message onto the new branch is deferred
  to a future release.

## The render policy

`RevertRenderPolicy` controls how decay-able tags age out of the prompt:

```rust
pub struct RevertRenderPolicy {
    pub lesson_window_turns: u32,  // default 5
    pub lesson_window_count: usize, // default 3
}
```

A `Lesson` or `Finding` tag renders into the prompt if **either** condition
holds:

- It was created within the last `lesson_window_turns` turns.
- It is one of the most recent `lesson_window_count` tags of its kind on
  the trunk.

So a recent mistake stays visible long enough to prevent an immediate
repeat; older mistakes (where the task has moved on) stop cluttering the
prompt while remaining in the session log. `Outcome` / `Checkpoint` tags
ignore both gates and always render while on-trunk.

Apply via:

```rust
ctx.build_trunk_context_with_policy(&policy, current_turn)
```

(`build_trunk_context()` returns the raw walk with no filtering, useful
for tests and tooling that want the unredacted view.)

## What 0.8.0 does NOT ship

The 0.8.0 release is the structural core. Several things slot in later:

- **Model-backed summary generation** when the agent omits `summary`.
  Currently the tag's `text` is empty in that case (kind classification is
  still correct).
- **Auto-rebase** of a `Message::User` in an abandoned span onto the new
  branch. 0.8.0 rejects; a future release can lift the user message
  forward.
- **DAG / multi-parent merge.** 0.8.0 is tree-only — `parent_id` is a
  single `Option<NodeId>`.
- **Soft-failure / silent-stall detection.** Composition I covers hard
  failures + agent-elected reverts; soft-failure detection is
  [`concept-brake.md`](concept-brake.md)'s principal residual open
  question.

## See also

- [`docs/concepts/concept-brake.md`](concept-brake.md) — the design
  source-of-truth (Compositions A–I).
- [`docs/concepts/compaction.md`](compaction.md) — the compaction layer
  Composition I sits *above*.
- [`tests/revert_test.rs`](../../tests/revert_test.rs) — opt-in guarantee
  + tool-shape tests.
- `src/agent_loop/run.rs::apply_revert_tests` — between-turn application
  semantics (success / rejection / inrun_context filtering).
- `src/types/context.rs::build_trunk_context_tests` — parent-chain walk
  semantics (cycle guard, fallbacks, render policy).
