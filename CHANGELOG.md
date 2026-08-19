# Changelog

All notable changes to phi-core are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

---

## [Unreleased]

## [0.12.0] — 2026-08-19

**Progressive tool-catalog disclosure + selectable skill-prompt layout, plus braking UX & on-demand tool documentation (general-merit; surfaced by i-phi e2e testing).**

- **Progressive tool-catalog disclosure + tool-registration contract** (#110 + #109;
  `types/tool.rs` + `agent_loop/{config,streaming}.rs` + `tools/tool_help.rs` +
  `tools/revert.rs`). The `AgentTool` trait gains `short_description()` +
  `detailed_description()` + a `has_large_schema()` default method (all
  backward-compatible — the `timeout()` default-method precedent; existing impls
  unchanged). A new `AgentLoopConfig.progressive_tool_catalog` knob
  (`ProgressiveToolCatalog { enabled, min_tools, engage_on_large_schema }`,
  **default OFF**) gates a reduced turn-1 `tools[]` render: when engaged (tool count
  above `min_tools`, or a large-schema tool such as an MCP/OpenAPI adapter is
  attached), each catalog entry carries the tool's `short_description()` + a
  minimal-valid `{"type":"object"}` `parameters` stub instead of the full schema, and
  the model fetches the full `parameters_schema()` + detailed manual on demand via
  `tool_help` (served from a cycle-free build-time catalog snapshot). `revert_to_state`'s
  long manual is split out of `description()` into `detailed_description()`. A
  `SHORT_DESCRIPTION_MAX_CHARS` const + `validate_tool_registration()` primitive let a
  consumer keep catalogs lean-by-contract (hard-error default; consumer-overridable
  stance). **Default OFF ⇒ every existing turn-1 `tools[]` wire is byte-identical.**
  Also fixes 2 latent `openapi`-feature bugs surfaced under `--all-features` (a
  never-compiling doctest; `derivable_impls`). See ADR-0005.

- **Selectable skill-prompt layout — XML default + opt-in YAML** (#78;
  `context/skills.rs`). A new `SkillPromptFormat { Xml, Yaml }` (default `Xml`) +
  `format_for_prompt_as(format)` let a caller opt into a lighter YAML
  `<available_skills>` block for token-tight deployments; `format_for_prompt()` stays
  byte-for-byte XML so existing callers are untouched. Wired via
  `BasicAgent::with_skills_format(set, format)`. See ADR-0004.

**Braking UX + on-demand tool documentation (general-merit; surfaced by i-phi e2e testing).**

- **Revert steering dedup — the `[continue_after_revert]` message references the
  node + rendered tag-label instead of echoing the full breadcrumb**
  (`types/context.rs` + `types/node_tag.rs`). The post-revert continue-forward
  steering text previously echoed the most-recent tag's full breadcrumb text,
  duplicating the adjacent on-node `[label: text]` annotation back-to-back on the
  first post-revert render (GitHub #74 / D-TEST-0069). Now the steering text
  **references the tip node** by its rendered node-number (`tip.node_id.render()`,
  e.g. `n1`) + the rendered tag-label (`newest.kind.rendered_label()`) + a SHORT
  head-elided gloss of the breadcrumb (`[label: …tail]` via the NEW
  `elide_breadcrumb_tail` helper — the trailing `(… abandoned)` parenthetical
  survives, a breadcrumb at/under the ~40-char cap is glossed verbatim):
  `[continue_after_revert] You just reverted to node n1. See [lesson: …approach (write_file abandoned)] at n1. Continue forward: …`.
  The kind→label mapping is extracted from the inline `match` in
  `weave_braking_annotations` into the NEW `TagKind::rendered_label()` helper (the
  single source of truth, so the label rendered ON the node and the label
  REFERENCED by the pointer are identical by construction; weave output stays
  byte-identical). The on-node `[label: text]` annotation remains the full summary
  site; the steering carries only a recognizable cue to the abandoned action. No
  `NodeTag` shape change. Closes the CC-15→…→CC-19 braking-render arc.

- **PERSISTENT collapse + render-path order — reclamation that persists across turns;
  reachable breadcrumb decay-drop** (`types/context.rs` + `agent_loop/streaming.rs`).
  The render-time collapse below was **tip-only**, so reclamation lasted exactly the
  single turn right after a revert; once the model continued forward, the reverted
  cluster slid mid-trunk, the collapse walked past it, and the abandoned heavy body
  re-rendered on every subsequent turn (GitHub #73 / D-TEST-0068). Now
  `collapse_abandon_class_cluster` is a **policy-aware PERSISTENT SCAN** over ALL
  node-bearing trunk messages (signature gains `policy: &RevertRenderPolicy,
  current_turn: u32`) — every live abandon-class cluster is collapsed on every render,
  so the abandoned body stays reclaimed across all post-revert turns. The
  **collapsed-node decay-drop moves INTO this policy-aware collapse** (a cluster whose
  decayable tag is out-of-window has its now-content-less node dropped) — making it
  reachable in production: in the prior design it lived in
  `build_trunk_context_with_policy`, ran *before* the collapse, and read the immutable
  heavy `messages`, so it never saw a lone breadcrumb to drop. The tag-decay second
  pass is extracted to `pub(crate) decay_tags_by_policy`, and the
  `agent_loop/streaming.rs` render path is reordered: `build_trunk_context()` →
  `collapse_abandon_class_cluster(&policy, turn)` → `decay_tags_by_policy(&policy,
  turn)` → weave → inject. The collapse runs FIRST (it needs the raw heavy content to
  know what to clear); `messages` stays the immutable forensic log (byte-identical
  verified after a MULTI-TURN scan); pinned clusters are kept verbatim everywhere.
  General braking-machinery merit: any consumer that reverts and continues forward
  keeps the abandoned cluster reclaimed persistently with no log mutation.
- **Single-axis category render rule — full-node collapse on decayable; empty-tail
  pinned no-op; collapsed-node decay-drop** (`types/context.rs` +
  `agent_loop/run.rs`). **Supersedes the partial-strip collapse mechanism** below.
  The render rule now branches purely on `TagKind::is_decayable(category)` — no
  tail-emptiness discriminator, no own-action heuristic. The atomic unit is the
  whole `{n0,n1}` cluster; naming `step="n0"` (the assistant call) vs `step="n1"`
  (its result) points at the same cluster, so the disposition is identical.
  - **Full-node collapse on decayable** (`collapse_abandon_class_cluster`) — on a
    `failure`/`tangent` revert the surviving assistant node's **ENTIRE content is
    cleared** (drop the reasoning `Text` AND the `ToolCall`), leaving only the woven
    breadcrumb. The earlier mechanism stripped only the `ToolCall` and left the
    reasoning text behind; a weak model re-read its own "Starting with Step 1…" and
    re-executed the abandoned work. Both tip shapes (call-tip + result-tip) clear the
    surviving node's content; the result-tip path still moves the tag onto the parent
    and removes the orphaned tool-result. Pinned (`completion`/`step-summary`) keeps
    the cluster verbatim (unchanged).
  - **Empty-tail pinned no-op** (`apply_revert`) — the pinned `Outcome`/`Checkpoint`
    tag is attached ONLY when a tail was dropped (`abandoned_node_ids` non-empty).
    Reclamation has two sources — collapse the cluster (decayable only) + drop the
    tail (both) — so a pinned revert with no tail reclaims nothing and adds no tag
    (it would merely restate still-visible content); the active-pointer move +
    `RevertApplied` event still fire. Decayable tags always attach (the lesson
    replaces removed content, meaningful even with an empty tail).
  - **Collapsed-node decay-drop** — once a collapsed node's lone breadcrumb fully
    decays (outside the turn-window), the now-content-less node is dropped from the
    rendered trunk rather than left as an empty `[nN]` stub. Nodes with real surviving
    content are never dropped. *(Relocated into the policy-aware
    `collapse_abandon_class_cluster` by the PERSISTENT-collapse rework above — it was
    originally authored in `build_trunk_context_with_policy` but was unreachable there.)*
  - **`revert_to_state` description — tail-reclamation sentence** (`tools/revert.rs`).
    Appends: *"completion/step-summary reclaim the abandoned tail; if there's no tail
    (you're sealing the step you just finished), nothing shrinks — just continue."* —
    steering the model away from the degenerate empty-tail summary no-op.
  The byte-shrink goal, the atomic-cluster (no-dangling-call) invariant, and the
  `messages`-byte-identical invariant (the collapse stays render-only over the cloned
  trunk) all carry forward unchanged. General braking-machinery merit: any consumer
  reverting onto/across a heavy tool-cluster fully reclaims that context with no log
  mutation.
- **`tool_help` built-in tool** (`tools/tool_help.rs`; `ToolHelpTool`). A
  permission-safe, on-demand documentation channel: `tool_help(tool_name)`
  returns a named tool's full manual (mental model + worked examples + failure
  modes) so a model can self-serve the depth exactly when it needs it, instead
  of every model paying the full token cost of an exhaustive `description()` on
  every turn. Ships the kernel's short canonical manuals for the braking trio
  (`revert_to_state` / `prun` / `prun_with_memo`) consumer-agnostically; a
  downstream can supply richer per-tool bodies via `ToolHelpTool::new(map)`.
  Added to `default_tools()` (now 7 tools) and to `BasicAgent::with_tool_help()`
  (explicit opt-in for hand-built tool sets). Unlike a path-in-description +
  `read_file` channel, it exposes no filesystem and works even when the agent is
  sandboxed/locked down.
- **`revert_to_state` self-description rewrite** (`tools/revert.rs`
  `description()` + `step` param). The description now teaches the model the
  mental model it needs to use the tool correctly: the conversation is a TREE of
  nodes, the inline `[nN]` markers ARE those nodes, naming a node in `step`
  makes it the new tip (dropping everything after), how to choose the node (just
  before the branch to discard; `n0` = full restart), and to CONTINUE FORWARD
  after reverting instead of repeating abandoned steps. Points at
  `tool_help("revert_to_state")` for the full manual. A weaker model could
  previously call `revert_to_state` mechanically but loop (revert to `n0`
  repeatedly) because the description never taught the tree model.
- **`weave_braking_annotations` rewind-with-breadcrumb rework**
  (`types/context.rs`). Three braking-machinery robustness improvements, all
  confined to the weave + the revert-apply step (signatures unchanged; non-revert
  consumers stay byte-identical). (a) **All-identical lesson/finding dedup** — an
  identical `(kind, text)` tag on a node now renders ONCE even when interleaved
  with other tags (was: consecutive-only, so `[finding][lesson][lesson]`
  ordering still stacked the repeat). Repeated reverts to the same node could
  otherwise bury the model in a wall of duplicate lessons. (b) **Post-revert
  continue-forward steering — a decaying, tagged, all-category
  `[continue_after_revert]` synthetic user message**
  (`AgentContext::inject_continue_after_revert`, wired in
  `agent_loop/streaming.rs` after `weave_braking_annotations`). After a revert
  the surviving tip carries the breadcrumb, but a weaker model can still loop on
  the re-presented task (re-doing already-completed steps). The continue-forward
  directive is emitted as a SEPARATE synthetic `Message::User` placed immediately
  after the reverted-to tip node (NOT folded into the tip's annotation, which an
  earlier iteration tried but did not reliably steer the weak model). Properties:
  the text begins with the literal marker
  `pub const CONTINUE_AFTER_REVERT_MARKER = "[continue_after_revert]"` so these
  model-steering messages are identifiable + filterable from channels (they are
  NOT real user input); it echoes the tip breadcrumb then carries the directive
  (`[continue_after_revert] <breadcrumb>. You just reverted to this node.
  Continue forward: do the next uncompleted step; do NOT redo completed steps; do
  NOT stop.`); it is emitted for ALL revert categories (failure→Lesson,
  tangent→Finding, completion→Outcome, step-summary→Checkpoint), not just the
  abandon class; and it **decays** — rendered ONLY while the tip's most-recent
  revert tag is within the decay window (`current_turn - tag.created_at_turn <=
  lesson_window_turns`). Past the window the steering message is suppressed for
  any category — INCLUDING pinned `Outcome`/`Checkpoint` whose TAG itself
  persists on the trunk (the nudge is transient even when the pinned tag is not).
  `weave_braking_annotations` itself is now markers + tags only. (c) **Rewind
  breadcrumb (progress
  thread)** — `apply_revert` (`agent_loop/run.rs`) now composes a one-line
  breadcrumb of the abandoned branch from the agent's revert `summary` plus the
  tool-call NAMES extracted from the abandoned span (`compose_revert_breadcrumb`;
  shapes like `reverted past: wrote plan-v1.md (write_file abandoned)`). The
  revert tool's OWN name (`revert_to_state`) is excluded from the breadcrumb —
  it is the triggering action, not abandoned work — so a revert called from the
  node after its target no longer mislabels as `(write_file, revert_to_state
  abandoned)`. The breadcrumb rides on the reverted-to node's summary tag and the
  weave renders it inline on that node, so the rebuilt trunk keeps a one-line
  progress thread of what was tried-and-abandoned — mirroring the
  `prun_with_memo` "drop the content, leave a memo" pattern. Only tool-call names
  are carried; the abandoned content itself is never re-introduced. The decay
  policy (`build_trunk_context_with_policy`) is unchanged by THIS entry (its
  window default is lowered separately below). (Braking machinery surfaced during
  downstream consumer e2e testing: a clean rewind erased the model's progress
  thread — a strong model reverted then stopped, a weak model looped.)
- **Render-time reclamation of the abandon-class tool-cluster** (`types/context.rs`
  `AgentContext::collapse_abandon_class_cluster`; wired in `agent_loop/streaming.rs`
  between `build_trunk_context_with_policy` and `weave_braking_annotations`).
  When a `failure`/`tangent` (abandon-class) revert targets a node that is a
  heavy `(assistant-tool_call, tool_result)` cluster — e.g. the model reverts
  ONTO an 80-line `write_file` call it got wrong — the kept tip otherwise carries
  that heavy body forward (context gets LARGER, not smaller). This render-time
  pass collapses the WHOLE cluster atomically, handling BOTH revert-target shapes
  so the rendered trunk carries a single clean assistant node with the breadcrumb,
  NO heavy `ToolCall`, and NO orphaned tool-result:
  - **call-tip** (revert lands on the assistant tool-call node): strip the tip's
    heavy `ToolCall` blocks; its matching tool-result is off-trunk (a child the
    parent-chain walk excluded), so nothing dangles.
  - **result-tip** (revert lands on the matching tool-result — the #59/minimax
    shape): the heavy body lives in the parent assistant-call node. Strip THAT
    parent's `ToolCall` blocks, MOVE the tip's breadcrumb tag onto the parent, and
    REMOVE the tool-result tip from the rendered trunk — so there is no orphaned
    result (a `tool_call_id` with no matching call, which OpenAI-compat providers
    reject). The surviving parent assistant node becomes the tip, the weave renders
    the breadcrumb on it, and `inject_continue_after_revert` places the decaying
    `[continue_after_revert]` steering message right after it.
  **Atomic-cluster invariant (ALL categories)**: the rendered trunk never carries
  a tool-call without its matching result, nor a tool-result without its matching
  call — abandon-class drops the whole cluster (call stripped, orphaned result
  removed), pinned (`completion`/`step-summary`) keeps both whole (re-including
  the off-trunk result so the kept call never dangles). The collapse is
  **render-only**: it operates on the cloned trunk that `build_trunk_context`
  returns by value, so `context.messages` (the forensic log) stays byte-identical
  — replay/audit/multi-pod see the full record. General braking-machinery merit:
  any consumer reverting onto/across a heavy tool-cluster reclaims that context
  with no log mutation.
- **`revert_to_state` description — 4-category budget framing** (`tools/revert.rs`
  `description()`). The description now frames `revert_to_state` as the model's
  CONTEXT-BUDGET tool and teaches the four categories as budget levers: `failure`
  (a branch failed; lesson fades) / `tangent` (an exploration is finished; finding
  fades) / `completion` (a sub-task is sealed; outcome pinned) / `step-summary`
  (a long trunk needs a checkpoint; checkpoint pinned). It is honest that
  completion/step-summary still drop the abandoned tail + keep a durable marker
  but do NOT shrink the kept span the model reverted to.
- **Decay-window default lowered 5 → 3 turns** (`types/node_tag.rs`
  `RevertRenderPolicy::default`). Abandon-class breadcrumbs (`Lesson`/`Finding`)
  now fade to log-only after 3 turns instead of 5, so the render-time reclamation
  lands faster; the window remains operator-tunable via the consumer's
  render-policy wiring (`lesson_window_turns`). Tests that set the window
  explicitly are unaffected; default-relying tests are updated to the new value.

---

## [0.11.4] — 2026-06-03

**Patch release (host-controlled session identity; surfaced by i-phi e2e testing).**

- **`BasicAgent::with_session_id(impl Into<String>)`** (`agents/basic_agent.rs`).
  A constructor-time builder setter that seeds the agent's `session_id`, the
  complement to the existing `Agent::session_id` getter and `rotate_session`
  (which only rotates to a fresh random id). By default `BasicAgent::new`
  assigns a random `session_id`; a host that manages session identity
  externally — e.g. a daemon that issues a session id at session-create time and
  uses it as the on-disk record key / wire directory — can now make the agent
  emit its events under THAT id, so the materialized `Session` (and anything
  keyed on the event `session_id`, like a `SessionStore`) lands under the id the
  host already handed to its client. Without this the agent's events carried its
  internal random id and the persisted record was unreachable by the host's
  external id. General capability (any multi-agent host needs to control session
  identity); no behavioural change for callers that don't call it. Closes the
  kernel side of i-phi D-TEST-0020 Bug D.

---

## [0.11.3] — 2026-06-03

**Patch release (two runtime-correctness fixes surfaced by i-phi e2e testing).**

- **Harmony tool-call name sanitization** (`provider/openai_compat.rs`). Some
  providers (observed: OpenRouter's gpt-oss / harmony-format conversion) leak a
  model-format control token into the structured `function.name` field — e.g.
  `"name":"repo_facts<|channel|>commentary"`. The contaminated name then fails
  tool-registry dispatch and any consumer permission-rule match, breaking tool
  calls intermittently for harmony-format models. The streamed tool-call name is
  now truncated at the first `<|` control-token delimiter (`sanitize_tool_name`)
  before it reaches the tool registry / `Content::ToolCall`. Clean names (no
  `<|`) are unchanged. (i-phi D-TEST-0038)

- **Composition I braking annotations reach the model** (`types/context.rs`,
  `agent_loop/streaming.rs`). `revert_to_state`'s `step` requires a node id
  (`n<id>`), and the render policy decides which `Lesson`/`Finding` tags survive
  — but `node_id` and `tags` lived as metadata that `convert_to_llm` stripped, so
  the model saw **neither** the node markers it must echo into `step` **nor** the
  lesson summaries the tool promises "the next turn sees". The revert tool was
  therefore unusable from a cold start. New `AgentContext::weave_braking_annotations`
  bakes `[n<id>]` markers + surviving `[<kind>: <text>]` tag annotations into the
  message **content** on the revert-mode trunk path (`active_node_id.is_some()`).
  Non-revert consumers are byte-identical (their messages carry no `node_id`, so
  the weave is a pass-through). (i-phi D-TEST-0036)

---

## [0.11.2] — 2026-06-03

**Minor release (breaking-but-tiny hook-return flip).** The `before_tool_execution`
hook now returns `ToolGate { Allow, Deny { reason } }` instead of `bool`. A denied
tool's `reason` lands in the model's synthetic `tool_result` instead of a hardcoded
opaque string, so the model is told **why** the tool was blocked and can self-correct
instead of re-trying the denied tool and burning tokens. The Allow path and the
Deny-with-default-reason path are byte-for-byte behaviour-compatible with 0.11.1.

**Migration:** `true → ToolGate::Allow`; `false → ToolGate::Deny { reason }` (use
`ToolGate::deny(reason)`; pass `ToolGate::DEFAULT_DENY_REASON` for the historical
opaque string). Non-installers (`before_tool_execution: None`) are unaffected — `None`
is type-agnostic. **baby-phi ripple is 0 source edits** (it only sets the field to
`None`); installers that previously returned `false` now return
`ToolGate::Deny { reason }`, supplying a real denial reason from their permission/policy layer.

### Added

- **`ToolGate` enum** (`agent_loop::config`) — `Allow` runs the tool; `Deny { reason }`
  skips it and synthesises an error `ToolResult` whose text is `reason`. Intentionally
  additive-friendly: future verdict variants (`Defer`, `AskUser`, …) extend it without
  another signature break.
- **`ToolGate::DEFAULT_DENY_REASON`** const — the historical opaque skip string
  (`"Tool execution skipped by before_tool_execution hook."`), preserved as the
  documented default a caller MAY pass.
- **`ToolGate::deny(reason)`** convenience constructor.

### Changed

- **`BeforeToolExecutionFn` return type `bool → ToolGate`** (`HookFuture<'a, bool>` →
  `HookFuture<'a, ToolGate>`). `BasicAgent::on_before_tool_execution`'s closure bound
  flips from `-> bool` to `-> ToolGate` accordingly.
- **The denied `tool_result` now carries the supplied `Deny` reason** instead of the
  hardcoded opaque string (`agent_loop/tools.rs`). Hook ordering is unchanged: a denied
  call still emits `MessageStart`/`MessageEnd` for the synthetic result and **no**
  `ToolExecutionStart`/`End`.
- **The script-callback bridge** (`config/builder.rs`) maps its `allow: bool` to
  `ToolGate::Allow` / `ToolGate::deny(DEFAULT_DENY_REASON)`. The `.sh`/`.py` hook
  protocol is unchanged (still `{ "allow": bool }`); reason-from-script is a future
  additive extension.

---

## [0.11.1] — 2026-06-02

**Patch release (additive / opt-in).** Threads the 0.11.0 `provider_wire_sink`
capture surface from the consumer down to the provider call so a
`BasicAgent`/daemon can actually install a sink. Previously the loop-construction
site (`streaming.rs`) hardcoded `provider_wire_sink: None`, leaving the 0.11.0
`StreamConfig.provider_wire_sink` field unreachable from any consumer. Additive,
default-`None`; baby-phi is unaffected and needs no source edits.

### Added

- **`AgentLoopConfig.provider_wire_sink: Option<Arc<dyn ProviderWireSink>>`** —
  consumer plumbing so a caller can install a raw-wire capture sink that the
  agent loop threads into every per-attempt `StreamConfig`.
- **`BasicAgent::with_provider_wire_sink(sink)`** builder setter (mirrors
  `with_provider_override` / `with_response_format`), plus the corresponding
  `BasicAgent.provider_wire_sink` field defaulted to `None`. `build_loop_config`
  propagates it into `AgentLoopConfig`; the `Agent::build_config` default path
  and `sub_agent.rs` construction set `None`.

### Changed

- **`streaming.rs` wires the consumer sink.** The per-attempt `StreamConfig`
  build now sets `provider_wire_sink: config.provider_wire_sink.clone()` instead
  of a hardcoded `None`, so a sink installed via `BasicAgent` (or directly on
  `AgentLoopConfig`) reaches the provider and fires `ProviderWireSink::on_wire`.

---

## [0.11.0] — 2026-06-02

**Minor release (additive / opt-in).** Adds an opt-in raw-wire capture surface
and completes reasoning-text mapping across every `ThinkingFormat` arm. Both
changes are backward-compatible: the new `StreamConfig` field defaults to
`None`, so existing callers (including baby-phi) are unaffected and need no
source edits. Plan archive: i-phi
`docs/v0/proposal/plan/build/ch-cc-09a-htc-observability-phi-core-half-7ed50e5e/plan.md`.

### Added

- **Opt-in `provider_wire_sink` raw-wire capture surface.** A new
  `StreamConfig.provider_wire_sink: Option<Arc<dyn ProviderWireSink>>` field
  (default `None`) lets a caller observe the literal provider wire traffic for
  a stream. The `ProviderWireSink` trait receives `RawWire` events:
  `RawWire::Request` (the outbound request body), `RawWire::ResponseFrame`
  (one per arriving SSE frame, in stream order), and `RawWire::ResponseDone`
  (at finalize). Wired across **all 7 providers + `MockProvider`**
  (`openai_compat`, `openai_responses`, `anthropic`, `azure_openai`, `google`,
  `google_vertex`, `bedrock`, `mock`). Capture is **per-SSE-frame**, so the
  observer sees exactly what streamed (including partial/interleaved reasoning
  frames). **Per-auth-shape redaction at the sink boundary** ensures no
  credential reaches a captured value: Bearer tokens, `x-api-key`, URL query
  keys (scrubbed via `scrub_url_query`), and SigV4 credentials are stripped
  before any `RawWire` is emitted. `StreamConfig`'s `Debug` derive is replaced
  with a manual impl (the sink is `Arc<dyn ...>` and not `Debug`); the field
  prints as a placeholder.

### Fixed

- **Reasoning text reaches the `Content::Thinking` block across every
  `ThinkingFormat` arm.** Previously the `ThinkingFormat::OpenRouter` arm
  filtered `delta.reasoning_details` on `type == "thinking"` and never read the
  plain `delta.reasoning` string, so models that emit `type == "reasoning.text"`
  (e.g. gpt-oss-via-OpenRouter) had their reasoning **text** silently dropped
  even though reasoning *tokens* were counted. The OpenRouter arm now prefers
  the `delta.reasoning` string and otherwise assembles from any text-bearing
  `reasoning_details` entry (no longer restricted to `type == "thinking"`),
  with de-dup so the same text mirrored on both fields is not double-appended.
  Additionally, the **google (Gemini 2.5)** provider now deserializes the
  per-part `thought` flag (`GooglePart.thought`) and routes `thought: true`
  parts to `Content::Thinking` + `StreamEvent::ThinkingDelta` instead of the
  visible text block. The `Xai` (`delta.reasoning`), `OpenAi`/default
  (`delta.reasoning_content`), anthropic native thinking, and
  openai_responses reasoning-item arms already mapped reasoning text correctly
  and are unchanged.

---

## [0.10.0] — 2026-05-25

**Minor release.** Closes 5 OPEN downstream consumer drifts surfaced by i-phi:
4 with code changes shipped here and 1 (i-phi `D-CH16b-FOLLOWUP-01`) whose
technical scaffolding was already shipped at 0.9.0 (the LLM-body deferral
remains at the consumer's discretion). All four surface additions are
mechanically additive at the call site; only the async-update-Fn migration is
a Breaking change.

Plan archive: i-phi
`docs/v0/proposal/plan/intermediate-stabilization-36caa39f.md` (Chunk A).

### Breaking

- **`BeforeToolExecutionUpdateFn` + `AfterToolExecutionUpdateFn` are now
  async** (closes the 0.9.0 "Forward markers" deferral). Type aliases flip
  from sync `Arc<dyn Fn(&str, &str, &str) -> bool>` and
  `Arc<dyn Fn(&str, &str, &str)>` to the boxed-future shape used by the 9
  lifecycle Fns async-migrated at 0.9.0:

  ```rust
  pub type BeforeToolExecutionUpdateFn =
      Arc<dyn for<'a> Fn(&'a str, &'a str, &'a str) -> HookFuture<'a, bool> + Send + Sync>;
  pub type AfterToolExecutionUpdateFn =
      Arc<dyn for<'a> Fn(&'a str, &'a str, &'a str) -> HookFuture<'a, ()> + Send + Sync>;
  ```

  The `BasicAgent::on_before_tool_execution_update` and
  `on_after_tool_execution_update` builder methods are unchanged at the
  surface — they continue to accept sync `Fn(...)` closures and wrap the
  body in `Box::pin(async move { ... })` automatically. Direct field-level
  consumers of the type aliases migrate by wrapping the closure body the
  same way:

  ```rust
  let hook: BeforeToolExecutionUpdateFn = Arc::new(|name, id, text| {
      Box::pin(async move {
          // sync logic — same as before
          true
      })
  });
  ```

  The agent loop bridges from the sync `ToolUpdateFn` callback (which tools
  invoke during their async `execute` body via `ctx.on_update(...)`) by
  polling the hook future to completion with `futures::executor::block_on`.
  Sync closure bodies (wrapped in `async move { ... }`) complete without
  suspending. Hooks that need to perform truly async work at update-time
  should dispatch via `tokio::spawn(...)` inside the closure body rather
  than suspending inline.

  Closes i-phi consumer drift `D-CH08-FOLLOWUP-03`.

- **`AgentLoopConfig` gains two new public fields:**
  `revert_render_policy: RevertRenderPolicy` and
  `current_tool: Option<Arc<Mutex<Option<CurrentToolExecution>>>>`.
  Struct-literal construction breaks — add
  `revert_render_policy: RevertRenderPolicy::default(), current_tool: None`.
  Builder-pattern construction via `BasicAgent` is unaffected.

### Added

- **`BasicAgent::with_revert_render_policy(RevertRenderPolicy) -> Self`** —
  builder method (sibling to `with_revert_tool`) that configures the
  kind-aware decay window applied to Composition I trunk context. The
  agent loop's `stream_assistant_response` now dispatches to
  `AgentContext::build_trunk_context_with_policy(&policy, turn_index)`
  whenever `active_node_id.is_some()`; outside revert mode the field has no
  effect and the linear `build_working_context` path is byte-identical to
  pre-0.10 behaviour. Closes i-phi consumer drift `D-CH17-FOLLOWUP-03`.

- **`BasicAgent::current_tool_timeout(&self) -> Option<Duration>`** — new
  introspection method that returns the effective timeout of the tool
  currently executing inside the agent's loop, or `None` when no tool is in
  flight. Backed by a new shared slot
  (`crate::context::CurrentToolExecution`) the agent loop writes around
  every `AgentTool::execute()` invocation. Intended use: a host pausing a
  session that is mid-tool-call needs the upper bound on how long the pause
  may block. Documented single-tool model — under `Parallel` / `Batched`
  execution the slot reflects the most-recently-started tool. Closes i-phi
  consumer drift `D-CH09-FOLLOWUP-04`.

- **`phi_core::agent_loop::script_callback::detect_interpreter`** is now
  `pub` (1-LOC visibility flip). External consumers can adopt the same
  script-extension dispatch table phi-core uses internally for
  `ScriptCallback` rather than re-deriving it. Closes i-phi consumer drift
  `D-CH08-FOLLOWUP-PHICORE-01`.

- **`CurrentToolExecution`** struct re-exported from `crate::context`.
  Carries `name: String` + `timeout: Option<Duration>`. Populated by
  `execute_single_tool` immediately before invocation and cleared on
  return (success / error / timeout).

### Migration

**Direct `AgentLoopConfig { ... }` struct-literal construction** — add
the two new fields:

```rust
let config = AgentLoopConfig {
    // ... existing fields ...
    revert_render_policy: phi_core::RevertRenderPolicy::default(),
    current_tool: None,
};
```

**Direct construction of `BeforeToolExecutionUpdateFn` /
`AfterToolExecutionUpdateFn`** — wrap the body in
`Box::pin(async move { ... })`:

```rust
let before: BeforeToolExecutionUpdateFn = Arc::new(|name, id, text| {
    Box::pin(async move {
        // your veto / observation logic ...
        true
    })
});
let after: AfterToolExecutionUpdateFn = Arc::new(|name, id, text| {
    Box::pin(async move {
        // your after-emit logic ...
    })
});
```

The `BasicAgent` setters (`on_before_tool_execution_update`,
`on_after_tool_execution_update`) accept sync closures and wrap them
automatically — no migration needed for builder-based consumers.

**Code that reads `RevertRenderPolicy` from sub-agents** — the default
trait `Agent::build_config()` impl supplies `RevertRenderPolicy::default()`,
matching pre-0.10 behaviour (which had no policy field; the default-window
was implicit at the consumer level when calling
`build_trunk_context_with_policy` directly).

### Internal

- New `phi-core/src/context/execution.rs::CurrentToolExecution` struct +
  module re-export at `crate::context::CurrentToolExecution`.
- `agent_loop/tools.rs::execute_single_tool` now publishes /
  clears the `config.current_tool` shared slot around `tool.execute()`.
- `agent_loop/streaming.rs::stream_assistant_response` dispatches to
  `build_trunk_context_with_policy` when `active_node_id.is_some()`.
- `agent_loop/tools.rs` bridges async tool-update hooks from the sync
  `ToolUpdateFn` callback via `futures::executor::block_on`.
- Test count: 470 → 475 (+5 across new integration tests in
  `tests/release_0_10_test.rs`: `with_revert_render_policy_propagates_to_loop_config`,
  `revert_render_policy_strips_old_lesson_tags_from_llm_prompt`,
  `current_tool_timeout_visible_during_tool_execution`,
  `async_update_hooks_fire_through_sync_bridge`,
  `detect_interpreter_is_publicly_reachable_and_correct`). Inline unit
  tests in `agents/basic_agent.rs::tests` also cover the new setters
  (`with_revert_render_policy_sets_config_field`,
  `revert_render_policy_defaults_to_phi_core_defaults`,
  `current_tool_timeout_is_none_when_no_tool_in_flight`,
  `current_tool_timeout_reflects_shared_slot`,
  `current_tool_slot_is_shared_between_agent_and_config`).

---

## [0.9.0] — 2026-05-24

**Breaking-change release.** Ships two bundled surfaces:

1. **Per-turn debug capture.** A new `AgentEvent::TurnRequest` variant
   carries the fully-assembled LLM request — system prompt, post-
   `convert_to_llm()` `Vec<Message>` array, tool definitions, and
   parallel-indexed per-block provenance — exactly once per turn. Opt-in
   persistence onto `Turn::request_payload` via the new
   `SessionRecorderConfig::capture_turn_requests` flag (default `false`).
   Closes the gap where the wire-format payload sent to the model was
   never recoverable post-hoc.

2. **Async-trait migration.** `BlockCompactionStrategy` and 9 of the 11
   `AgentLoopConfig` lifecycle Fns + the `InputFilter` trait become async.
   Custom impls and hook closures can now `.await` LLM calls and other
   async work inside compaction bodies and lifecycle hooks without
   `block_in_place` workarounds. Tool-update hooks
   (`BeforeToolExecutionUpdateFn` / `AfterToolExecutionUpdateFn`) remain
   sync — see the migration notes for the rationale.

Concept source: [`docs/concepts/debugging.md`](docs/concepts/debugging.md)
(new — covers existing debugging surfaces plus the 0.9.0 capture). Plan
archive: i-phi
`docs/v0/proposal/plan/build/phi-core-0.9.0/plan.md`.

### Breaking

- **`AgentEvent::TurnRequest` variant added.** `AgentEvent` is already
  `#[non_exhaustive]` since 0.8.0, so wildcard `_ => …` arms keep
  compiling unchanged. Exhaustive matchers without a wildcard must add a
  `TurnRequest { .. } => …` arm.
- **`LlmMessage` gains `provenance_hint: Option<Box<BlockProvenance>>`.**
  `LlmMessage::new(...)`, `LlmMessage::with_turn(...)`,
  `LlmMessage::with_provenance_hint(...)`, and `LlmMessage::with_node_identity(...)`
  fill the field automatically. Direct struct-literal construction breaks —
  add `provenance_hint: None`. Serialization is `#[serde(default)]` /
  omitted-when-`None`, so old session JSON loads cleanly.
- **`BlockCompactionStrategy` is now `#[async_trait]`.** All four methods
  (`keep_first`, `keep_recent`, `keep_compacted`, `compact`) are `async fn`.
  Sync impls migrate mechanically: prepend `#[async_trait::async_trait]`
  to the impl block and `async` to each method signature. Bodies need no
  changes if they don't `.await` anything. `compact_session_loops` (and the
  `BasicAgent::compact_context*` wrappers) is now `async fn` as well.
- **9 of 11 `AgentLoopConfig` lifecycle Fns become async.**
  `BeforeLoopFn`, `AfterLoopFn`, `BeforeTurnFn`, `AfterTurnFn`, `OnErrorFn`,
  `BeforeToolExecutionFn`, `AfterToolExecutionFn`, `BeforeCompactionStartFn`,
  `AfterCompactionEndFn` switch from `Fn(...) -> T` to
  `Fn(...) -> HookFuture<'_, T>` (alias for `Pin<Box<dyn Future<Output = T> + Send>>`).
  Sync hook bodies migrate by wrapping in `Box::pin(async move { ... })`.
  `BeforeToolExecutionUpdateFn` and `AfterToolExecutionUpdateFn` **stay sync**
  — see migration notes.
- **`InputFilter::filter()` is now `async fn`** (via `#[async_trait]`).
  CPU-bound filters should wrap their work in `tokio::task::spawn_blocking`
  to avoid stalling the runtime.
- **`AgentLoopConfig` gains no new fields** — async-fication is contained
  in the existing hook fields' type aliases.

### Added

- **`AgentEvent::TurnRequest`** variant with fields
  `{ loop_id, turn_index, payload: AnnotatedRequestPayload, timestamp }`.
  Emitted exactly once per turn (before the retry loop's first
  `provider.stream()` call) regardless of recorder configuration.
- **`BlockProvenance`** enum (`#[non_exhaustive]`) with variants
  `SystemPrompt`, `IdentityBlock { name, order }`,
  `MemoryTier { tier, record_id }`,
  `LoopTurn { turn_index, role, message_index }`, `Steering`, `FollowUp`,
  `Unknown`.
- **`ProvenanceRole`** enum: `UserMessage`, `AssistantResponse`,
  `ToolCallRequest`, `ToolCallResult`.
- **`AnnotatedRequestPayload`** struct mirroring the provider wire format
  (`system_prompt` + `messages` + `tools` + model identity / thinking_level
  / max_tokens / temperature / response_format) with a parallel-indexed
  `provenance` vec.
- **`SessionRecorderConfig::capture_turn_requests: bool`** (default
  `false`) — opt-in flag that mirrors `include_streaming_events`.
- **`Turn::request_payload: Option<AnnotatedRequestPayload>`** —
  `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing
  session JSON loads unchanged.
- **`LlmMessage::with_provenance_hint(BlockProvenance)`** consuming
  builder — used by upstream consumers (identity loaders, memory stores)
  to stamp non-loop-history provenance before emitting messages.
- **`HookFuture<'a, T>`** type alias for `Pin<Box<dyn Future<Output = T> + Send + 'a>>`
  in `phi_core::agent_loop` — short hand for async hook return types.
- **`phi-core/docs/concepts/debugging.md`** — new concept doc covering all
  debug surfaces (`AgentEvent` stream / `SessionRecorder` JSON / `tracing`
  integration) plus the new per-turn capture flow.

### Migration

**Custom `BlockCompactionStrategy` implementations** — add
`#[async_trait::async_trait]` to the impl block and `async` to each method.
If your bodies don't `.await` anything, no further changes are needed:

```rust
use async_trait::async_trait;
use phi_core::context::{BlockCompactionStrategy, CompactedSection, TurnMap, TurnRange};
use phi_core::session::LoopRecord;

struct MyStrategy;

#[async_trait]
impl BlockCompactionStrategy for MyStrategy {
    async fn keep_first(
        &self,
        record: &LoopRecord,
        turn_map: &TurnMap,
        config: &phi_core::context::CompactionConfig,
    ) -> Option<TurnRange> {
        // Sync body — unchanged. Or await an LLM call here.
        None
    }
    // ... keep_recent / keep_compacted similarly
}
```

**Lifecycle hook closures** — wrap the sync body in
`Box::pin(async move { ... })`:

```rust
use std::sync::Arc;
use phi_core::agent_loop::BeforeTurnFn;

let hook: BeforeTurnFn = Arc::new(|messages, turn_index| {
    Box::pin(async move {
        println!("turn {} starting with {} messages", turn_index, messages.len());
        true // false to abort the turn
    })
});
```

Closures that previously did `tokio::task::block_on(async { llm_call().await })`
can drop the bridge and just `.await` directly inside the `async move` block.

**`LlmMessage` struct-literal construction** — add `provenance_hint: None`:

```rust
let lm = phi_core::LlmMessage {
    message: phi_core::Message::user("hi"),
    turn_id: None,
    node_id: None,
    parent_id: None,
    tags: vec![],
    provenance_hint: None, // <-- 0.9.0 addition
};
```

Or prefer the constructor: `LlmMessage::new(Message::user("hi"))`.

**`InputFilter::filter()` is now `async fn`** — prepend
`#[async_trait::async_trait]` to the impl block + `async` to the method.
For CPU-bound filters:

```rust
async fn filter(&self, input: &str) -> FilterDecision {
    let owned = input.to_string();
    tokio::task::spawn_blocking(move || expensive_sync_scan(&owned))
        .await
        .unwrap_or(FilterDecision::Allow)
}
```

**Exhaustive `match AgentEvent` arms** — `#[non_exhaustive]` shielded the
type since 0.8.0, so wildcard arms compile unchanged. Otherwise add:

```rust
AgentEvent::TurnRequest { loop_id, turn_index, payload, timestamp } => {
    // per-turn debug payload available here
}
```

**Pre-existing-behaviour preservation note —
`BeforeToolExecutionUpdateFn` + `AfterToolExecutionUpdateFn` stay sync.**
Making them async would cascade into the `ToolUpdateFn` callback type and
every `AgentTool::execute` body that invokes `ctx.on_update(...)` —
materially wider than the 0.9.0 cycle's scope. The veto decision in
`BeforeToolExecutionUpdateFn` must be synchronous so the surrounding
emit-gate works without an `.await` suspension point at every streamed
tool-update; consumers that want async work at update-time should
dispatch via `tokio::spawn(...)` inside the sync closure body. Tracked
under the `[Unreleased]` "Forward markers" section for a future release.

### Internal

- New `phi-core/src/types/provenance.rs` (`BlockProvenance` + `ProvenanceRole`
  + `AnnotatedRequestPayload` with `serde` round-trip including a
  `response_format` proxy because `ResponseFormat` does not derive serde
  natively).
- `stream_assistant_response()` derives a parallel `Vec<BlockProvenance>` for
  the wire-format `messages` vec, reading `LlmMessage::provenance_hint`
  when set and falling back to `turn_id` + role-derivation otherwise.
- `compact_session_loops` is `async fn`; in-loop call sites in
  `agent_loop/run.rs` adopt `.await`.
- Test count: 461 → 470 (+9 across new integration tests
  `tests/turn_request_capture_test.rs` + `tests/async_compaction_strategy_test.rs`).

---

## [0.8.0] — 2026-05-23

**Breaking-change release.** Ships **Composition I** — an opt-in tree-structured
"braking" layer on top of the agent's conversation. The agent can now call a
new `revert_to_state` tool to abandon failed or finished branches between
turns; the next prompt is rebuilt by walking parent-id links from the active
node, so abandoned spans drop out of context while the forensic record stays
intact. Composition I sits **above** `BlockCompactionStrategy` /
`compact_messages` / episodic memory — it does NOT replace them; it delays
how often they must run.

The braking layer is **opt-in**. A consumer that upgrades 0.7.1 → 0.8.0
without changing how it constructs its agent sees no behavioural change —
the new tool is not registered, the parent-chain walk does not activate, and
`build_working_context` takes the byte-identical linear path it always did.
Enable via `BasicAgent::with_revert_tool()` (one line on the builder).

Concept source: [`docs/concepts/concept-brake.md`](docs/concepts/concept-brake.md) §5
Composition I. Plan archive: i-phi
`docs/v0/proposal/plan/build/phi-core-revert-tool-27c894f6/plan.md`.

### Breaking

- **`LlmMessage` gains three new public fields** — `node_id: Option<NodeId>`,
  `parent_id: Option<NodeId>`, `tags: Vec<NodeTag>`. Construction via
  `LlmMessage::new(Message::…)` or the `with_turn_*` builders is unaffected
  (the constructors fill the new fields with their defaults). Direct
  `LlmMessage { … }` struct-literal construction breaks — callers must add
  `node_id: None, parent_id: None, tags: vec![]` or switch to the
  constructor. Custom serde is extended so old session JSON loads cleanly
  (the new fields are `#[serde(default)]` optionals — `nodeId` / `parentId` /
  `tags` keys are present only when non-default).
- **`AgentEvent` becomes `#[non_exhaustive]`** and gains the
  `RevertApplied { loop_id, category, target, abandoned_node_ids, summary,
  applied, reason, timestamp }` variant. Every exhaustive `match` against
  `AgentEvent` in a downstream crate now requires either an explicit
  `RevertApplied { … }` arm or a wildcard `_ => …`. The disruption is paid
  once now; subsequent additions to `AgentEvent` are non-breaking.
- **`AgentLoopConfig` gains `revert_pending: Option<Arc<Mutex<Vec<RevertRequest>>>>`**.
  Construction via `BasicAgent::build_config()` / `Agent::build_config()`
  default impl is unaffected (the field is filled automatically). Struct-
  literal construction breaks — callers must add `revert_pending: None`.

### Added

- New module `tools::revert` — `RevertTool`, `RevertRequest`, `RevertRecord`.
  The tool is model-callable with four kebab-case categories: `failure` /
  `tangent` / `completion` / `step-summary`, plus an optional `summary` the
  agent writes inline.
- New `BasicAgent::with_revert_tool()` builder — registers `RevertTool` and
  wires the shared pending queue into `AgentLoopConfig`. The opt-in
  guarantee is enforced end-to-end: the LLM never sees the tool unless this
  method was called.
- New `BasicAgent::tools()` accessor — read-only view of the registered
  tool set. Useful for tests that assert tool-registry shape (e.g. the
  Composition I opt-in regression).
- New `types::node_tag` module — `NodeId`, `NodeTag`, `TagKind`,
  `RevertCategory`, `RevertRenderPolicy`. `NodeId` renders inline as `n<N>`
  and parses leniently from `"n12"` / `"12"`.
- New `AgentContext::active_node_id` and `AgentContext::next_node_id` fields
  + `alloc_node_id()` and `seed_next_node_id_from_messages()` helpers.
- New `AgentContext::build_trunk_context()` — parent-chain walk that
  assembles the LLM-facing context from `active_node_id`. Cycle-guarded
  via a visited set; dangling parents stop gracefully; an unresolved
  active node falls back to `messages.clone()`.
- New `AgentContext::build_trunk_context_with_policy(policy, current_turn)`
  — applies kind-aware filtering: `Lesson` / `Finding` tags drop out of the
  prompt past the policy window (default 5 turns) AND the per-kind count
  cap (default 3); `Outcome` / `Checkpoint` tags stay pinned while
  on-trunk. Policy defaults are tunable per application via
  `RevertRenderPolicy`.
- New `apply_revert` between-turn drain in `agent_loop/run.rs`, mirroring
  `apply_prun`. Synchronous, emits exactly one `RevertApplied` event per
  drained request (success or rejection). Rejection rules: unknown target
  and user-message-in-span (per D6 — auto-rebase deferred to a future
  release).
- New concept doc `docs/concepts/composition-i.md`; companion
  `docs/concepts/concept-brake.md` is the design source-of-truth (promoted
  from i-phi to phi-core in this release).

### Documentation

- Promoted `concept-brake.md` from i-phi to `phi-core/docs/concepts/`.
- New `docs/concepts/composition-i.md` with `[EXISTS]` status tags.
- Bumped `phi-core = "0.8"` in `README.md` and added a Composition I
  feature bullet.

---

## [0.7.1] — 2026-05-16

Documentation-only patch release. No code changes. Bumped so the crates.io
README and rendered docs reflect the 0.7.0 surface accurately.

### Documentation

- Bumped `phi-core = "0.7"` in `README.md` and `docs/getting-started/installation.md`.
- Corrected `build_config()` return type in `docs/guides/configuration.md` to
  `Result<AgentLoopConfig, AgentBuildError>`.
- Extended `docs/specs/architecture.md` SessionStore section to describe the
  `SessionStore` trait + `FileSystemSessionStore` with atomic writes and
  advisory locks.
- Added a "0.7.0 additions" subsection to `docs/reference/api.md` listing
  module-path imports for `SessionStore`, `FileSystemSessionStore`,
  `CredentialProvider`, `StaticCredentialProvider`, `ResponseFormat`,
  `AgentBuildError`, `McpClientConfig`, `DEFAULT_REQUEST_TIMEOUT`.
- Added a "Pluggable store trait" subsection to `docs/concepts/sessions.md`
  documenting the trait API + `fs2` locking contract.
- Added this `CHANGELOG.md` (Keep-a-Changelog format).
- Refreshed verified-headers on all touched doc files.

---

## [0.7.0] — 2026-05-16

Hardening + ergonomics release. Brings phi-core to production-ready for
single-process agent workloads. One small breaking change to the `Agent` trait;
the rest is additive.

### Breaking

- **`Agent::build_config()`** now returns
  `Result<AgentLoopConfig, AgentBuildError>` instead of `AgentLoopConfig`.
  - The default implementation no longer panics when `model_config()` returns
    `None`; it returns `Err(AgentBuildError::MissingModelConfig)`.
  - `BasicAgent`'s override always returns `Ok(...)` because its constructor
    requires a `ModelConfig` — no behavioral change for the common path.
  - **Migration:** any custom `Agent` impl that overrides `build_config()` must
    wrap its return value in `Ok(...)`. Callers of `agent.build_config()` need
    to handle the `Result` (typically with `?` or `.expect()`).

### Added

- **Per-tool timeouts.** New `AgentLoopConfig.tool_timeout: Option<Duration>`
  and `AgentTool::timeout() -> Option<Duration>` default method. Resolution
  order: tool-level override → config-level → no timeout. On timeout, the tool
  call is cancelled and the LLM receives a structured error result (so it can
  self-correct) instead of starving sibling tools. Adds
  `ToolError::Timeout { duration }` variant.
- **Structured-output contract.** `ResponseFormat::{Text, JsonObject, JsonSchema}`
  enum on `StreamConfig`. Each provider maps it to its native JSON mode where
  available; Anthropic and Anthropic-on-Bedrock emulate via a synthetic
  tool-call; unsupported configurations surface `ProviderError::SchemaMismatch`
  instead of silently producing free-form text. New
  `Message::extract_json::<T: DeserializeOwned>()` for ergonomic deserialisation.
- **Credential refresh.** New `CredentialProvider` async trait
  (`current()` / `invalidate()`) attachable via
  `ModelConfig::with_credentials(provider)`. On `ProviderError::Auth`, the
  agent loop invalidates the cached credential and retries once before
  propagating — supports long-running agents on STS / OAuth tokens.
  `StaticCredentialProvider` is provided for testing.
- **`SessionStore` trait.** Async pluggable persistence trait
  (`save` / `load` / `list_ids` / `delete` / `list_for_agent`) alongside the
  existing free functions. In-tree `FileSystemSessionStore` adds advisory
  `fs2` exclusive locking on save: concurrent writers to the same
  `session_id` get `SessionError::Locked` instead of corrupted JSON.
- **MCP transport timeouts.** Both `StdioTransport` and `HttpTransport` now
  take a `request_timeout` (default 30 s). A hung MCP subprocess no longer
  blocks the entire agent loop indefinitely. Configure via the new
  `McpClientConfig` + `McpClient::connect_{stdio,http}_with_config()`. The
  `DEFAULT_REQUEST_TIMEOUT` constant is exported. Adds `McpError::Timeout`.

### Changed

- **Atomic session writes.** `save_session()` and `FileSystemSessionStore`
  now write to a tmp file and rename over the target. Readers no longer
  observe partially-written JSON during a save.
- **Internal:** consolidated `agent_id`/`session_id`/`loop_id` initialisation
  in `agent_loop::core` behind a single `ensure_loop_ids()` helper instead of
  scattered `.unwrap()` calls.

### Fixed

- **Poison-tolerant steering / follow-up queues.** A panic inside a hook or
  tool callback no longer crashes the agent session: the recovery helper logs
  a warning and returns the inner `Vec<AgentMessage>` via
  `PoisonError::into_inner()`.
- **Hot-path `.unwrap()`s removed** from `agent_loop::core`,
  `agent_loop::parallel`, and the Google provider's temperature parsing.
  Non-numeric temperatures now surface `ProviderError::Internal` instead of
  panicking mid-stream.

### Dependencies

- Added `fs2 = "0.4"` for advisory file locking in `FileSystemSessionStore`.

---

## [0.6.x] and earlier

See git history (`git log v0.6.0..v0.7.0` for the full diff).
