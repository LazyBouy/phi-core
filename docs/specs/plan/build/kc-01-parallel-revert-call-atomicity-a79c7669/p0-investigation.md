<!-- Last verified: 2026-06-08 by Claude Code -->

# P0 investigation — KC-01 (phi-core)

> Parallel-revert call-atomicity (closes GitHub #77 / D-TEST-0072, S2). First phi-core kernel chunk.
> Grounded + reproduced on current `dev` (phi-core **0.11.4**). Design (R1–R4) is already locked on #77;
> this investigation GROUNDS its feasibility against the live surface — it does NOT re-decide it.

## §0 — Verdict summary

- **Findings: 9 definitive, 0 unresolved.** Every #77 root-cause fact and every behaviour-matrix row reproduced on current `dev` with a live render-path repro (5/5 rows matched the claim exactly).
- **Fix-locus: phi-core kernel, render-pass only** — `collapse_abandon_class_cluster` (R1/R2/R3) + a declarative orphan-filter / empty-node pass in the `streaming.rs` render pipeline (R4/F3). `apply_revert` only marks intent; the forensic `messages` log is never mutated. NO viable consumer-side fix (the render machinery is wholly inside phi-core). Zero consumer-specific logic — call-atomic render is a general kernel-correctness property every consumer benefits from.
- **Forks surfaced for the planner: 3** (F1 ingest id-uniqueness shape; F2 R3 × prun/decay-drop; F3 enforcement placement). NOT locked.
- **Non-viable approaches ruled out: 2** (#77 direction A cluster-atomic node model; #77 direction C repair-all-alone) — both with evidence.
- **Unresolved / needs-live-repro: NONE blocking.** All matrix rows settle at unit level. One recommended live close-gate (§8) per `[[feedback_render_transcript_close_gate]]` — parallel-tool + pinned-revert renders + sends without a provider 400.

## §1 — Questions that gate planning

1. Do the three #77 root-cause facts still hold on current `dev`, and at what exact file:line (the issue's numbers may have shifted)?
2. Do the two #77 "BROKEN" matrix rows (pinned revert into a multi-call cluster) ACTUALLY produce a dangling `tool_call` on current `dev`, or has some guard since closed them? Same for the "SAFE"/"incidentally-safe" rows (no regression risk).
3. Is the native join key (`Content::ToolCall.id` ↔ `Message::ToolResult.tool_call_id`) actually present + populated at the render locus so the render code can join call↔result by id?
4. Are `tool_call_id`s session-unique today, or is the locked design's "session-uniqueness namespacing on ingest" genuinely needed? Where are ids assigned (Mock + a real provider)?
5. Is the fix genuinely kernel-side with no viable consumer-side alternative, and does it add only general primitives (no i-phi leakage)?
6. Are #77's other fix directions (A cluster-atomic node model / C repair-all) larger/riskier or regression-prone vs the locked hybrid — with evidence?
7. For the residual forks: what does the id provenance recommend for F1; does pin-wins (R3) govern a prun/decay-drop of an R3-kept node (F2); where should enforcement land (F3)?

## §2 — Current-surface map (file:line, current `dev` / 0.11.4)

**Stamp (Fact 1 — linear, not sibling):**
- `stamp_node_identity` — `agent_loop/run.rs:571-597`. The load-bearing line is **`run.rs:585`**: `let parent = context.active_node_id.or_else(|| …)` then `context.active_node_id = Some(new_id)` (`:595`). Each new node's parent is the *previous* active node → results of one assistant node form a LINEAR chain `n0 → n1 → n2`, never siblings of `n0`. (#77 cited `:584-595` — confirmed, off-by-one on the parent line.)

**Revert intent (Fact 2 — User-only refusal):**
- `apply_revert` — `agent_loop/run.rs:709`. User-message refusal guard at **`run.rs:752-762`** (exactly as #77 cited): refuses ONLY when the strictly-after span contains a `Message::User`. Landing mid-cluster (revert to `n1`/`n0`) is permitted. `active_node_id` is set at `:781`; the forensic `messages` log is never mutated (doc-comment `:685-688`).

**Trunk assembly (the sole `parent_id` consumer):**
- `build_trunk_context` — `types/context.rs:860`. Walks `cur = lm.parent_id` from `active_node_id` to root (`:884-896`), one linear single-parent chain. **This is the ONLY consumer of `parent_id`** (grep-confirmed). Reverting to `n1` walks `n1 → n0` → trunk `[n0, n1]`; `n2` (2nd result) is off-trunk while `n0` still carries BOTH calls — the BROKEN setup.
- `build_working_context` — `types/context.rs:221`; dispatches to the trunk path when `active_node_id.is_some()` (`:222-223`).

**Render collapse / orphan-repair (Fact 3 — first-call-only):**
- `collapse_abandon_class_cluster` — `types/context.rs:440`.
  - **Join-key recovery = FIRST ToolCall only**: `types/context.rs:507-510` — `content.iter().find_map(|b| ToolCall { id, .. } => Some(id.clone()))` returns the FIRST call's id; result-node branch reads `tool_call_id` (`:511`). #77 cited `:507-510` — confirmed (the issue attributed it to `build_trunk_context_with_policy`; on current `dev` it lives inside `collapse_abandon_class_cluster`).
  - **Abandon-class whole-node `content.clear()`**: call-node `types/context.rs:546`; result-node parent `types/context.rs:575`. (#77 cited `:544-548`/`:546`/`:576` — confirmed, shifted by ~1.) Tag-migration onto parent at `:577` (`lm.tags.extend(moved_tags)`).
  - **Pinned re-append = single call_id only**: `types/context.rs:628-646` — re-appends the result matching `node_call_id` (the FIRST call's id only); 2nd..Nth calls invisible. (#77 cited `:628-646` — confirmed.)
  - **Reachable decay-drop**: `types/context.rs:608-616` (inside `if abandon_class`).
- `decay_tags_by_policy` — `types/context.rs:303`; pure tag-decay 2nd pass.
- **Production render pipeline** — `agent_loop/streaming.rs:165-191`: `build_trunk_context` (raw) → `collapse_abandon_class_cluster` → `decay_tags_by_policy` → `weave_braking_annotations` → `inject_continue_after_revert`. This is where an R4 empty-node + F3 declarative orphan-filter backstop would slot.

**Node-tag model:**
- `TagKind` — `types/node_tag.rs:112-117` (`Lesson`/`Finding`/`Outcome`/`Checkpoint`). `is_decayable` — `:123` (`true` for Lesson/Finding = abandon-class; `false` for Outcome/Checkpoint = pinned). `RevertCategory` → `TagKind` mapping `:97-100` (`Failure→Lesson`, `Tangent→Finding`, `Completion→Outcome`, `StepSummary→Checkpoint`).

**Join-key types + ingest:**
- `Content::ToolCall { id: String, name, arguments }` — `types/content.rs:85-89`. `Message::ToolResult { tool_call_id: String, .. }` — `types/content.rs:147-156`; doc explicitly documents the join (`content.rs:143`). `ToolResult.tool_call_id` also at `types/tool.rs:117`.
- **Result-ingest copies the call id verbatim**: `agent_loop/tools.rs:255-261/:283/:355/:378/:387/:490/:502-503/:544/:561` — `tool_call_id: id.to_string()` where `id` is the originating `ToolCall.id`. The join holds by construction.
- **Id provenance**: MockProvider `provider/mock.rs:219` — `id = format!("mock-tool-{}", i)` (`i` = per-message call index). Anthropic `provider/anthropic.rs:276-284` — id = provider `ToolUse.id` (`toolu_*`). openai_compat `provider/openai_compat.rs:425-428` — id = `buf.id` (`call_*`).
- **Session-unique NodeId already exists**: `alloc_node_id` — `types/context.rs:943-947` (monotonic counter); `seed_next_node_id_from_messages` — `:952`. Every stamped node already carries a session-unique `node_id`.

## §3 — Findings

**F-1 (DEFINITIVE) — Fact 1 holds: parallel results are stamped LINEAR, not sibling.**
`stamp_node_identity` (`run.rs:585`) sets `parent = previous active node` then advances. Evidence: `build_trunk_context` (`context.rs:884-896`) is the sole `parent_id` consumer and walks a single linear chain; the repro fixture's chain `u(n0)→assistant(n1)→result_a(n2)→result_b(n3)` rendered exactly as a linear walk (reverting to result_a yielded trunk `[u, assistant, result_a]`, dropping result_b). DEFINITIVE.

**F-2 (DEFINITIVE) — Fact 2 holds: `apply_revert` guards only `Message::User`, permits mid-cluster landing.**
`run.rs:752-762` refuses only on a User message in the strictly-after span; no cluster-atomicity guard. file:line confirmed verbatim. DEFINITIVE.

**F-3 (DEFINITIVE) — Fact 3 holds: render recovers/repairs only the FIRST tool_call per node.**
`context.rs:507-510` `find_map`-first-only id recovery; pinned re-append `:628-646` repairs only `node_call_id` (first call). 2nd..Nth calls have no repair path. file:line confirmed. DEFINITIVE.

**F-4 (DEFINITIVE) — BROKEN row `pinned → first result (n1)` reproduces: `call_b` DANGLES.**
Live repro (`build_trunk_context` → `collapse_abandon_class_cluster`, RevertRenderPolicy::default, in-window turn): revert target = result_a, tag = `Outcome` (completion/pinned). Observed: `dangling=["call_b"]`, `orphans=[]`. The assistant node keeps both calls on-trunk, result_b is off-trunk, pinned re-append recovers only call_a's result → `call_b` is a `tool_use` with no `tool_result` → provider 400. Matches #77 exactly. DEFINITIVE (captured output).

**F-5 (DEFINITIVE) — BROKEN row `pinned → call node (n0)` reproduces: `call_b` DANGLES.**
Revert target = assistant call node, tag = `Outcome`. Both results off-trunk; pinned re-append recovers only call_a's result. Observed: `dangling=["call_b"]`, `orphans=[]`. Matches #77. DEFINITIVE.

**F-6 (DEFINITIVE) — SAFE rows reproduce SAFE (no regression risk in those paths).**
- `abandon → first result (n1)` (Failure/Lesson): observed `dangling=[] orphans=[]` — `content.clear()` removes BOTH calls; incidentally safe (matches #77).
- `abandon → call node (n0)` (Failure/Lesson): observed `dangling=[]` — content cleared.
- `any → last result (n2)` (Outcome): observed `dangling=[] orphans=[]` — both results on-trunk.
All three SAFE exactly as #77 claims. These become the explicit regression-guard rows. DEFINITIVE.

**F-7 (DEFINITIVE) — the native join key is present + populated at the render locus.**
`Content::ToolCall.id` (`content.rs:86`) and `Message::ToolResult.tool_call_id` (`content.rs:149`) are both `String`, both carried on the full `AgentMessage::Llm` the trunk renders, and the result's id is copied verbatim from the call at ingest (`tools.rs`, multiple sites). The render code can join call↔result by string equality TODAY — no new persisted field needed. DEFINITIVE.

**F-8 (DEFINITIVE) — `tool_call_id` is NOT session-unique; the locked "session-uniqueness namespacing" is genuinely needed.**
MockProvider mints `mock-tool-{i}` where `i` resets to 0 for every assistant message (`mock.rs:219`) → `mock-tool-0` recurs every turn → a current call would cross-join a STALE result from a prior turn under raw-`tool_call_id` equality. Real providers give response-unique ids (`toolu_*`/`call_*`) but phi-core enforces no cross-turn uniqueness. CRUCIALLY: `node_id` IS already session-unique (`alloc_node_id` monotonic counter, `context.rs:943`), so a `(node_id, tool_call_id)` composite key rides on existing infrastructure with zero new state. DEFINITIVE — this directly informs F1.

**F-9 (DEFINITIVE) — fix-locus is kernel render-pass; no viable consumer-side fix.**
The entire defect lives in phi-core's render machinery (`build_trunk_context` + `collapse_abandon_class_cluster` + `streaming.rs` pipeline). A consumer (i-phi/baby-phi) has no hook to repair the working-context the kernel renders before it hits the provider — the working context is built inside `agent_loop`/`streaming.rs` and sent directly. The only consumer-visible surface (`transform_context`, `streaming.rs:194`) runs AFTER the broken trunk is already assembled and would force every consumer to re-implement call-atomic repair — a textbook kernel-correctness property. DEFINITIVE (per `[[feedback_phi_core_kernel_minimal]]` carve-out: genuine general kernel fix, zero consumer-specific logic).

## §4 — Fix-locus determination

phi-core is the minimal general-purpose KERNEL (`[[feedback_phi_core_kernel_minimal]]`). Carve-out test applied per change:

| Change (locked R#) | Side | Carve-out rationale | Blast radius |
|---|---|---|---|
| Session-unique join key on ingest (R-prep / F1) | **kernel** | The id↔result join is a kernel render primitive; uniqueness is a correctness property all consumers need. Rides on existing `alloc_node_id`/`node_id` (`context.rs:943`) — additive. | Touches ingest stamping + the render join; verify no change to provider id semantics (ids stay as received; namespacing is render-internal). |
| R1 per-call retain (render only emits calls whose result is live) | **kernel** | General: every consumer's trunk must be call-atomic. Lives in `collapse_abandon_class_cluster` / a render filter. | The hot working-context build (`streaming.rs:165-191`). Single-call (M=1) path must stay byte-identical — F-6 SAFE rows are the regression guard. |
| R2 per-call collapse (replace whole-node `content.clear()` at `:546`/`:575` with id-keyed per-call removal) | **kernel** | General braking-correctness. | `collapse_abandon_class_cluster` only. M=1 path identical. |
| R3 mixed-class → pin wins (don't collapse a node with both an abandon tag and a pinned/live sibling) | **kernel** | General safe-by-construction rule. | `collapse_abandon_class_cluster` tag-migration site (`:577`). |
| R4 empty-node drop / keep-as-text | **kernel** | General empty-assistant-400 avoidance. | A final render pass in `streaming.rs` pipeline. |
| F3 declarative orphan-filter backstop in `build_working_context`/pipeline | **kernel** | General invariant enforcement. | Additive final pass. |

**Other kernel callers verified unaffected**: the change is confined to the revert-mode render path (`active_node_id.is_some()`). `sub_agent.rs` / `parallel.rs` / `evaluation.rs` and all non-revert consumers take the `build_working_context` non-trunk branch (`context.rs:225-266`) which is untouched. No public API signature changes required (the locked design adds no new persisted field — F-7).

## §5 — Non-viable approaches ruled out (with evidence)

| Approach | Why it's tempting | Disproof + evidence |
|---|---|---|
| **#77 (A) Cluster-atomic node model** — stamp all N parallel results as siblings of the assistant node + snap reverts to the cluster boundary. | "Fixes the root cause at the node model — the linear chain is the original sin." | **Larger/riskier, re-architects the load-bearing trunk walk.** `build_trunk_context` (`context.rs:884-896`) is the SOLE `parent_id` consumer (grep-confirmed) and is a single LINEAR single-parent walk. Making n1,n2 both `parent = n0` breaks it: walking from `active=n2` yields `[n0, n2]` and DROPS n1 — siblings are not on each other's ancestor path. Direction (A) therefore requires converting the linear walk into a tree/DAG multi-child assembly AND adding a revert-snap-to-boundary in `apply_revert` — a change to the core braking-state machine, vs the locked hybrid which leaves the linear chain + walk untouched and fixes render only. Blast radius is the entire trunk-assembly core. |
| **#77 (C) Render repair-ALL alone** — make the pinned re-append iterate every `Content::ToolCall` (not first-only) WITHOUT per-call collapse. | "Narrowest fix — just repair every missing result." | **Re-materializes abandoned results; regresses the abandon-class semantics.** F-6 proved `abandon → first result` is SAFE today *precisely because* `content.clear()` (`context.rs:546`/`:575`) drops BOTH calls. A naive repair-all would re-append `call_b`'s result that a failure-revert MEANT to drop — re-introducing the noise wall the revert exists to remove (#77's own note). The locked hybrid (per-call collapse keyed on the *collapsing* result's id + per-call retain) is what preserves BOTH the pinned-no-dangle invariant AND the abandon-drops-the-branch invariant. Evidence: the abandon row's safety depends on selective drop, not blanket repair. |

The locked hybrid (id-join render-locus, per-call retain + per-call collapse + pin-wins) is the minimal change that satisfies all five F-4..F-6 invariants simultaneously without touching the node model or the trunk walk.

## §6 — Design forks surfaced (for the planner — NOT locked)

**F1 — ingest-time id-uniqueness shape · TECHNICAL.**
Options: (a) raw `tool_call_id` + a session-uniqueness *assertion*; (b) `(node_id, tool_call_id)` composite key; (c) monotonic internal mint replacing provider ids.
Factual trade-off surfaced: F-8 proves raw `tool_call_id` is NOT session-unique (Mock `mock-tool-{i}` collides every turn), so (a) would FAIL the assertion in the Mock test path and is a non-starter without a re-mint. (b) rides on the EXISTING session-unique `node_id` (`alloc_node_id`, `context.rs:943`) — zero new state, the join becomes `(node_id, tool_call_id)` and is collision-proof by construction; the synthetic-id fallback handles id-less calls within the same composite. (c) re-minting provider ids is heaviest (must round-trip the mint↔provider-id map for the wire serializer and risks diverging from what the provider echoes back). **Investigation leans (b)** — the design's "namespaced" direction maps cleanly onto the existing monotonic `node_id`.

**F2 — R3 × prun / decay-drop interaction · TECHNICAL (assess-then-likely-defer).**
Question: does pin-wins (R3 keeps a mixed-class node whole) also govern a prun or a decay-drop that targets an R3-kept node? Findings: (i) **prun** (`apply_prun`, `run.rs:601-655`) operates ONLY on `inrun_context` (the non-revert linear path) and never touches the trunk parent-walk render — it cannot target an R3-kept trunk node, so prun × R3 is a non-interaction on current `dev`. (ii) **decay-drop** (`context.rs:608-616`) fires ONLY inside `if abandon_class` — an R3-kept node (which by definition carries a pinned/live sibling) is NOT abandon-class-collapsed, so the decay-drop branch is not reached for it. So pin-wins is self-consistent with both today. The residual sub-question is whether a FUTURE decay-drop of the abandon TAG migrated onto an R3-kept node (`:577`) should still keep the node — R3 says yes (extra context can't 400). Factual trade-off: this is safe-by-construction either way; **likely non-load-bearing for the S2 closure** and can be a documented invariant rather than a fork the user must lock. Planner decides whether to formalize or defer.

**F3 — enforcement placement · TECHNICAL.**
Options: per-call retain inside `collapse_abandon_class_cluster` ONLY, vs ALSO a declarative orphan-filter backstop in the `streaming.rs:165-191` pipeline (belt + suspenders). Factual trade-off: the collapse pass is where the per-cluster tag/window state + content ownership live (so R1/R2/R3 must land there). A final declarative orphan-filter (drop any on-trunk `tool_use` whose `tool_result` is absent + R4 empty-node) in the pipeline is a cheap, locus-independent invariant backstop that also covers any future path that assembles a trunk (e.g. `run.rs:1467` `build_trunk_context_with_policy`). Cost of "both": one extra linear pass over the trunk (negligible). Cost of "single": a future trunk-assembly path could re-introduce an orphan unguarded. Design rec was "both"; planner locks.

## §7 — Repro assets

| Asset | What it proves | Disposition | How to re-run |
|---|---|---|---|
| `tests/zzz_kc01_p0_scratch_repro.rs` (5 tests: `row_pinned_revert_to_first_result_dangles`, `row_pinned_revert_to_call_node_dangles`, `row_abandon_revert_to_first_result_safe`, `row_revert_to_last_result_safe`, `row_abandon_revert_to_call_node_safe`) | The full #77 §3 behaviour matrix on current `dev`: both BROKEN rows produce `dangling=["call_b"]`; all three SAFE rows produce `dangling=[] orphans=[]`. | **THROWAWAY — REVERTED.** File deleted; tree clean (`git status` clean). | **Promote-to-regression-test candidate** for the implementer: re-author equivalently inside `types/context.rs`'s `collapse_abandon_class_cluster_tests` module using the existing `two_call`-style fixture + `trunk_has_dangling_call` helper (already present at `context.rs:2021-2043`). The two BROKEN rows must flip to `dangling=[]` after R1–R4; the three SAFE rows must stay green (regression guard for the M=1 path). |

Repro command (after re-creating the scratch file, or for the implementer's promoted tests):
`/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture`

Baseline confirmed green this investigation: 13 existing `collapse_abandon` lib tests pass on `dev` 0.11.4.

## §8 — Recommended close-gate

Per `[[feedback_render_transcript_close_gate]]` (model-facing fix → render + READ the actual transcript):

1. **Unit (in-tree)**: the promoted §7 regression tests — both #77 BROKEN rows now render a working context with NO dangling `tool_call` and NO orphaned `tool_result`; the three SAFE rows stay SAFE; plus the R4 empty-node case and the R3 mixed-class pin-wins case. Plus `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check`.
2. **Live (orchestrator-run)**: reproduce the CC-23 HTC-0009 shape — a braking scenario where one assistant turn fires ≥ 2 parallel tool_calls (e.g. `memory_help` + `skill_help`), THEN a `completion`/`step-summary` (pinned) `revert_to_state` lands ON or BEFORE the last result of that cluster. Render the working context and READ the transcript: assert the provider request carries exactly N `tool_use` blocks ↔ N `tool_result` blocks (or the cluster collapses atomically) and the turn SENDS without an Anthropic/OpenAI 400. "Unit tests pass" / "wire files produced" is NOT closure — the transcript must be read. This is an i-phi-harness HTC (the discovery surface was HTC-0009); the orchestrator runs it at the chunk close-gate. NOT settleable at phi-core unit level alone (needs a live provider round-trip), so it is surfaced here as the close-gate, not booted unsupervised during P0.

## §9 — Blocking vs non-blocking unresolved

- **Blocking: NONE.** All seven §1 questions settled DEFINITIVELY at static + unit level.
- **Non-blocking (recommended live close-gate, not a P0 blocker)**: the §8 live HTC parallel-tool + pinned-revert round-trip — proves the end-to-end no-400 property a unit repro cannot. Named evidence: an i-phi HTC transcript read (provider request with balanced `tool_use`/`tool_result` counts, no 400).
