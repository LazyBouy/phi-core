<!-- Last verified: 2026-06-06 by Claude Code (CC-19 revert steering dedup: the [continue_after_revert] steering text now REFERENCES the tip node — tip.node_id.render() ([nN]) + newest.kind.rendered_label() ([lesson]/[finding]/[outcome]/[checkpoint]) + a SHORT head-elided gloss [label: …tail] from the NEW elide_breadcrumb_tail helper — instead of echoing the full breadcrumb. The kind→label match in weave_braking_annotations is extracted to the NEW TagKind::rendered_label() helper (single source of truth; weave output byte-identical). The on-node [label: text] annotation stays the full summary site; the pointer carries only a recognizable cue to the abandoned action (#74 / D-TEST-0069). #74 closes the CC-15→…→CC-19 braking-render arc.) -->
<!-- Last verified: 2026-06-06 by Claude Code (0.11.6 PERSISTENT collapse + render-path order: collapse_abandon_class_cluster reworked from a single-tip act into a policy-aware PERSISTENT SCAN over ALL node-bearing trunk messages (gains &policy, current_turn) — every live abandon-class cluster is collapsed on every render, not just the tip, so reclamation persists across all post-revert turns (was tip-only/single-turn — GitHub #73 / D-TEST-0068). The collapsed-node decay-drop MOVES from build_trunk_context_with_policy INTO the policy-aware collapse (the tag-decay second pass is extracted to pub(crate) decay_tags_by_policy) — making it reachable in production (it previously ran before the collapse + read immutable heavy messages, so it never fired). Render path reordered in agent_loop/streaming.rs: collapse FIRST on the raw build_trunk_context() output, then decay_tags_by_policy on the already-collapsed trunk, then weave, then inject. messages stays the immutable forensic log (byte-identical after a MULTI-TURN scan). Pinned clusters kept verbatim everywhere — never collapsed/dropped.) -->
<!-- Last verified: 2026-06-06 by Claude Code (0.11.5 single-axis category render rule: collapse_abandon_class_cluster now replaces the surviving assistant node's ENTIRE content — text AND tool_call — with the breadcrumb on decayable categories (was a ToolCall-only strip that left reasoning text behind, driving weak-model looping); branch purely on is_decayable(category) (no tail-emptiness / own-action discriminator); apply_revert gates the pinned Outcome/Checkpoint tag on a non-empty abandoned tail (empty-tail summary = no-op no-tag; decayable always attaches); build_trunk_context_with_policy drops a collapsed node whose lone breadcrumb has fully decayed; revert_to_state description gains the tail-reclamation sentence; messages-byte-identical invariant preserved) -->
<!-- Last verified: 2026-06-05 by Claude Code (CC-17 render-time token reclamation: collapse_abandon_class_cluster collapses an abandon-class reverted-onto cluster atomically for BOTH revert-target shapes (call-tip strips the tip; result-tip strips the parent call, moves the tag onto it, and removes the tool-result tip — no orphaned result) into its one-line breadcrumb at render time (messages byte-identical); atomic-cluster invariant for all categories (abandon drops the whole cluster, pinned keeps both); post-revert continue-forward steering is now a SEPARATE decaying, tagged, all-category [continue_after_revert] synthetic Message::User (CONTINUE_AFTER_REVERT_MARKER) inserted after the tip by inject_continue_after_revert — emitted for all four categories while within lesson_window_turns, suppressed past the window incl. pinned outcome/checkpoint; the iter-2 inline annotation fold is removed; weave is markers+tags only; breadcrumb excludes the revert tool's own name; decay-window default lowered 5→3; revert description gains 4-category budget framing) -->
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

The tool's `description()` teaches the model the mental model it needs: the
conversation is a TREE of nodes, the inline `[nN]` markers ARE those nodes,
naming a node in `step` makes it the new tip (dropping everything after), how to
choose the node (just before the branch to discard; `n0` = full restart), and to
CONTINUE FORWARD after reverting rather than repeat the abandoned steps. For the
full manual + worked examples the model can call `tool_help("revert_to_state")`
(the on-demand documentation channel — see [`tools.md`](tools.md)).

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
    pub lesson_window_turns: u32,  // default 3 (lowered from 5 in 0.11)
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

## How node markers + annotations reach the model

`node_id` and `tags` live as **metadata** on `LlmMessage`; the `convert_to_llm`
step strips them before the provider call. So policy-filtering alone does not
put anything in front of the model — a final weave step bakes the survivors into
the message **content**:

```rust
let trunk = ctx.build_trunk_context_with_policy(&policy, current_turn);
let woven = AgentContext::weave_braking_annotations(trunk);
```

`weave_braking_annotations` weaves, per trunk message:

- `[n<id>]` — the node marker, so the model can read a valid `step` for
  `revert_to_state` (the tool's `step` accepts exactly this inline render). Without
  it the model has no way to know a target node from a cold start.
- `[<kind>: <text>]` — each surviving `Lesson` / `Finding` / `Outcome` /
  `Checkpoint` tag, so "the next turn sees the lesson" is literally true.
  **An identical `(kind, text)` lesson/finding tag on a node renders once** even
  when other tags interleave between the repeats — a seen-set, not a
  consecutive-only check, so a `[finding][lesson][lesson]` ordering no longer
  stacks the duplicate. Repeated reverts to the same node could otherwise bury
  weaker models in a wall of duplicated noise.

The weave itself renders **markers + tags only**. The post-revert
continue-forward steering is a SEPARATE step,
`AgentContext::inject_continue_after_revert`, wired in `agent_loop/streaming.rs`
right after the weave:

- When a revert landed on the trunk tip (the reverted-to node carries any revert
  tag), a **decaying, tagged, all-category `[continue_after_revert]` synthetic
  `Message::User`** is inserted immediately AFTER the tip. The text begins with
  the literal marker `pub const CONTINUE_AFTER_REVERT_MARKER = "[continue_after_revert]"`
  so consumers (channels, UIs, transcript renderers) can identify + filter it out
  of the real conversation stream — these are model-steering signals, NOT real
  user input. It **references the tip node by its rendered node-number + rendered
  tag-label + a short head-elided gloss of the breadcrumb** then carries the
  directive:
  `[continue_after_revert] You just reverted to node n1. See [lesson: …approach (write_file abandoned)] at n1. Continue forward: do the next uncompleted step; do NOT redo completed steps; do NOT stop.`
  The node-number is `tip.node_id.render()` (e.g. `n1`); the label is
  `newest.kind.rendered_label()` (`lesson`/`finding`/`outcome`/`checkpoint` — the
  single source of truth the weave shares); the gloss is `elide_breadcrumb_tail`
  applied to the newest tag's text (the trailing `(… abandoned)` parenthetical
  survives, the head is elided to a `…`; a breadcrumb at or under the ~40-char cap
  is glossed verbatim). The **full breadcrumb is no longer echoed** — the adjacent
  on-node `[label: text]` annotation is the full summary site, and the pointer
  references it by node-number so the model can find it without the back-to-back
  full repeat (#74 / D-TEST-0069).
- It fires for **ALL revert categories** — failure→`Lesson`, tangent→`Finding`,
  completion→`Outcome`, step-summary→`Checkpoint` — not just the abandon class.
- It **decays**: emitted ONLY while the tip's most-recent revert tag is within the
  decay window (`current_turn - tag.created_at_turn <= lesson_window_turns`). Past
  the window the steering message is suppressed for **any** category — INCLUDING
  pinned `Outcome`/`Checkpoint` whose TAG itself persists on the trunk (the nudge
  is transient even when the pinned tag is not). The earlier 0.11 iteration folded
  the directive into the tip node's annotation; it did not reliably steer a weaker
  model (which still looped re-doing completed steps), so the steering moved to
  this separate, marked, decaying message.
- The reverted-to node also carries the **rewind breadcrumb** of the abandoned
  branch — a one-line `reverted past: <summary> (<tool-names> abandoned)` thread
  composed by `apply_revert` (see below) — which the weave renders as the node's
  `[lesson: …]` tag. This restores the model's progress thread after a rewind (a
  clean rewind erases it, so a model may stop or loop), mirroring the
  `prun_with_memo` "drop the content, leave a memo" pattern. Only the tool-call
  NAMES are carried; the abandoned content itself is never re-introduced.

The breadcrumb is composed in `apply_revert` (`agent_loop/run.rs`): when a revert
drops the post-target span, `compose_revert_breadcrumb` builds the one-liner from
the agent's revert `summary` plus the tool-call names of the abandoned work, and
rides it on the reverted-to node's summary tag for the weave to render. The
name source-set is the **target cluster's own tool-call(s) PLUS the strictly-after
span** (0.11), with the **revert tool's own name (`revert_to_state`) excluded** —
it is the triggering action, not abandoned work. In the #59/minimax shape the
model reverts onto the tool-RESULT node and its `revert_to_state` call sits in the
strictly-after span; without the exclusion the breadcrumb would mislabel as
`(write_file, revert_to_state abandoned)`. After the exclusion the breadcrumb
names `write_file` only.

The agent loop calls the weave + `inject_continue_after_revert` on the revert-mode
trunk path only (`active_node_id.is_some()`); non-revert consumers are
byte-identical (their messages carry no `node_id`, so the weave is a pass-through
and no steering message is inserted). The decay policy default window is 3 turns
(lowered from 5 in 0.11), and the same window gates the `[continue_after_revert]`
steering message.

## Render-time reclamation of the abandon-class tool-cluster (0.11)

A revert reclaims context by dropping the abandoned tail (the off-trunk span the
parent-chain walk excludes). But there is a shape where the dropped span is NOT
enough: when the model reverts **ONTO** a heavy `(assistant-tool_call, tool_result)`
cluster it got wrong — e.g. `revert_to_state(failure, …)` onto an 80-line
`write_file` it abandoned. The kept tip then carries that heavy `ToolCall` body
forward (context gets *larger*, not smaller).

`AgentContext::collapse_abandon_class_cluster` (wired in `agent_loop/streaming.rs`)
collapses the WHOLE cluster atomically at **render time**, handling BOTH revert-target
shapes so the rendered trunk carries a single clean assistant node with the breadcrumb,
NO heavy `ToolCall`, and NO orphaned tool-result. **(0.11.6 PERSISTENT scan)** — it
scans **every** node-bearing trunk message and collapses **every** cluster carrying a
live abandon-class tag, not just the trunk tip (see "Persistent reclamation" below):

- **Abandon-class cluster** (the node carries a decay-able `Lesson`/`Finding` tag — the
  signal a `failure`/`tangent` revert landed there). The surviving assistant node's
  **ENTIRE content is replaced** by the breadcrumb — both the heavy `ToolCall` AND
  any accompanying reasoning `Text` are dropped (`content.clear()`), leaving only
  the woven breadcrumb. **(0.11.5 single-axis rule)** — the earlier 0.11 mechanism
  stripped only the `ToolCall` and left the reasoning text behind; a weak model
  re-read its own "Starting with Step 1…" and re-executed the abandoned work. The
  whole abandoned turn is scrapped, so nothing survives for the model to redo.
  Two shapes:
  - **call-node** (the node IS the assistant tool-call node): clear its content.
    The breadcrumb already lives on the node's tag; its matching tool-result is
    off-trunk (a child the parent-chain walk excluded), so nothing dangles.
  - **result-node** (the node is the matching tool-result — the #59/minimax shape):
    the heavy body lives in the parent assistant-call node. Clear THAT parent's
    content, MOVE the node's breadcrumb tag onto the parent, and REMOVE the
    tool-result node from the rendered trunk — otherwise a `tool_call_id` with no
    matching call survives (an **orphaned tool-result**, which OpenAI-compat
    providers reject). The surviving parent assistant node carries the breadcrumb,
    the weave renders it, and `inject_continue_after_revert` places the decaying
    `[continue_after_revert]` steering message right after the surviving tip.
- **Pinned cluster** (`Outcome`/`Checkpoint`): the cluster is load-bearing (a sealed
  result the model may re-read), so it is kept **whole** everywhere on the trunk — if
  the matching tool-result is off-trunk, it is re-appended right after the call so the
  kept call never dangles. Pinned clusters are never collapsed and never dropped.

**Single-axis rule (0.11.5)** — the branch is purely `TagKind::is_decayable()`. There
is no tail-emptiness discriminator and no own-action heuristic. Naming `step="n0"`
(the assistant call) vs `step="n1"` (its result) points at the same indivisible
`{n0,n1}` cluster (**atomicity**), so the disposition is identical either way.

**Reclamation model + empty-tail no-op** — token reclamation has two sources:
(1) collapse the cluster (decayable only) + (2) drop the abandoned tail (both
categories). Pinned categories keep the cluster, so their only saving is the tail.
`apply_revert` therefore gates the pinned (`Outcome`/`Checkpoint`) tag on a
non-empty `abandoned_node_ids`: a `completion`/`step-summary` revert with **no
tail** attaches NO tag (it would merely restate still-visible content), though the
active-pointer move + `RevertApplied` event still fire. Decayable (`Lesson`/
`Finding`) tags **always** attach — the lesson *replaces* removed content, so it is
meaningful even with an empty tail (the #59 shape).

**Persistent reclamation + render-path order (0.11.6)** — the collapse acts on EVERY
live abandon-class cluster on the trunk, on every render, not just the tip. The earlier
mechanism collapsed only the tip cluster, so reclamation lasted exactly the single turn
right after the revert; once the model continued forward, the reverted cluster slid
mid-trunk, the collapse walked past it, and the abandoned heavy body re-rendered every
subsequent turn (GitHub #73). To make this persistent the render path is reordered: the
policy-aware `collapse_abandon_class_cluster(&policy, current_turn)` runs **FIRST** on
the raw `build_trunk_context()` output (it needs the raw heavy content to know what to
clear), then `decay_tags_by_policy` decays out-of-window *tags* on the already-collapsed
trunk, then weave, then inject. The revert tag persists on the target node in
`messages` (`apply_revert`'s `add_tag`), so every tagged cluster is re-discoverable on
every render.

**Reachable collapsed-node decay-drop (0.11.6)** — the collapse is policy-aware: a
cluster whose decayable tag is IN-window collapses to the breadcrumb; a cluster whose
tag is OUT-of-window has its now-content-less node **dropped from the trunk entirely**
(the breadcrumb is reclaimed once its lesson no longer renders). This decay-drop USED to
live in `build_trunk_context_with_policy` — but it ran *before* the collapse and read
the immutable heavy `messages`, so it never saw a lone breadcrumb to drop in production
(it only passed a synthetic unit fixture). Moving it INTO the policy-aware collapse — the
only pass that both knows the per-cluster tag-window state AND owns the content — makes
it fire. Nodes with real surviving content are never dropped.

The **atomic-cluster invariant** therefore holds for ALL categories: the rendered
trunk never carries a tool-call without its matching result, nor a tool-result
without its matching call. The collapse is **render-only** — it operates on the
cloned trunk that `build_trunk_context` returns by value, so `context.messages`
(the forensic log) stays byte-identical (verified after a MULTI-TURN persistent scan);
replay, audit, and multi-pod see the full record.

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
