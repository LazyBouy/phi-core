<!-- Last verified: 2026-06-08 by Claude Code -->

# KC-01 — parallel-revert call-atomicity (phi-core braking)

> **First phi-core kernel chunk.** Closes GitHub #77 (D-TEST-0072), S2 runtime-correctness defect in the braking/revert machinery. Design is **already locked** (R1–R4) via the #77 design-discussion comment 2026-06-08 — this forward-scope carries that locked direction; the Phase-0.5 investigation grounds it against the live surface and the planner formalises the implementation plan.

## §1 — Purpose

Make the revert/working-context render **call-atomic** so that reverting into a parallel (multi-tool-call) cluster never leaves a dangling `tool_call` (a `tool_use` with no matching `tool_result`) or an orphaned `tool_result` (a `tool_call_id` with no matching call). Today the braking model is **single-call-per-node** throughout; a parallel assistant node (N tool_calls → N linearly-chained result nodes) breaks the atomic-cluster invariant and can hard-fail a turn with a provider 400.

## §2 — Inputs consumed

- **GitHub #77 / D-TEST-0072** — issue body (§1 gap, §2 three grounded root-cause facts, §3 behaviour matrix, §5 fix directions, §6 acceptance) + the **resolved-design comment** (call-atomic design, R1–R4, three worked examples, the pin-wins tie-break, the guiding principle).
- **phi-core current surface** (`dev`): `agent_loop/run.rs` (`stamp_node_identity:584`, `apply_revert:709`), `types/context.rs` (`build_working_context:221`, `collapse_abandon_class_cluster:440`, orphan repair `:507`/`:628`), `types/node_tag.rs` (`TagKind`), `types/content.rs:149` (`ToolResult.tool_call_id` ↔ `Content::ToolCall.id` join), `types/tool.rs:117`.
- **Memories**: `[[feedback_phi_core_kernel_minimal]]` (kernel-minimal surface discipline), `[[feedback_never_hedge]]`, `[[feedback_render_transcript_close_gate]]` (close-gate = render + read an actual transcript).

## §3 — Issues / drifts closed

- **#77 / D-TEST-0072** (S2) — parallel-revert dangling tool_call. Primary + only deliverable target.

## §4 — Prerequisites

- None. The fix is self-contained in phi-core `dev`. The `tool_call_id` ↔ `ToolCall.id` join key already exists natively; no new persisted field is required (an internal session-unique namespacing of that id is the only ingest-time addition).
- phi-core kernel lane in `chunk-initiate` (shipped 2026-06-08, commit `e90fe61`).

## §5 — Deliverables (locked design R1–R4)

1. **Join-key threading** — adopt the native `tool_call_id` ↔ `Content::ToolCall.id` pairing as the call↔result link (NO new "t-tag"). Add: (a) a **session-uniqueness guarantee on ingest** (namespacing so a provider that reuses an id cannot cross-join), (b) a **synthetic id fallback** for id-less / MockProvider calls so every call carries a join key.
2. **R1 — per-call retain** in the working-context render: an assistant node emits only the `ToolCall`s whose matching result is live on the trunk; a call is dropped iff its result was abandoned. Drop only the specific collapsing result, never live siblings. → no dangling call, no orphaned result, in either revert direction.
3. **R2 — per-call collapse**: replace the whole-node `content.clear()` in `collapse_abandon_class_cluster` (`context.rs:546`/`:576`) with per-call removal keyed on the collapsing result's id — breadcrumb the matched call, leave sibling calls intact.
4. **R3 — mixed-class collision → pin wins**: if an assistant node carries both an abandon tag (migrated from a collapsed sibling result) and a must-keep (pinned/live) sibling call, do NOT collapse — keep the node whole (accept the extra context; safe-by-construction).
5. **R4 — empty-node handling**: per-call retain leaving zero calls + no text → drop the node (avoid empty-assistant-turn 400); has text → keep as plain assistant text.
6. **Locus discipline**: render-pass only (`collapse_abandon_class_cluster` + a declarative orphan-filter in `build_working_context`); forensic log never mutated; `apply_revert` only marks intent.
7. **Tests**: unit coverage for each revert × category × cluster-arity case in §3's matrix (the pinned-into-cluster BROKEN cases must produce a clean working context with no dangling/orphan); the empty-node case; the mixed-class pin-wins case.
8. **Close-gate (per `[[feedback_render_transcript_close_gate]]`)**: a live braking scenario with a parallel-tool turn + a pinned revert into the cluster that renders + sends without a provider 400 (transcript read).
9. **Docs**: update the braking/revert concept doc + the `collapse_abandon_class_cluster` doc-comment's atomic-cluster invariant to state call-atomicity (was node-atomic); verified-headers refreshed.

## §6 — Acceptance criteria

- Reverting to `n1`/`n0` of a 2-tool-call cluster with a `completion`/`step-summary` (pinned) category produces a working context with **no dangling tool_call** and **no orphaned tool_result** (#77 §6).
- The §3 behaviour matrix's BROKEN rows become SAFE; the SAFE rows stay SAFE (no regression).
- Live parallel-tool + pinned-revert HTC renders + sends without a 400 (transcript read).
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core test suite green + `fmt --check` clean.
- Kernel-minimality: the change adds only general primitives (call-atomic render is a kernel-correctness property every consumer benefits from), no i-phi-specific leakage.

## §7 — Forks for the planner

Most forks were **resolved** in the #77 design discussion (carried as locked direction in §5). Genuine residual forks for the planner to surface + the user to lock:

- **F1 — ingest-time id-uniqueness shape**: raw `tool_call_id` + a uniqueness assertion vs. a `(node_id, tool_call_id)` composite key vs. a monotonic internal mint. (Design leans composite/namespaced; planner picks the concrete shape grounded on the investigation.)
- **F2 — R3 × prun / decay-drop interaction**: does pin-wins also govern a `prun` or decay-drop that targets a node R3 kept whole? (Open sub-question flagged on #77; may be a TECHNICAL FORK or deferred if non-load-bearing for the S2 closure.)
- **F3 — enforcement placement**: per-call retain in the collapse pass **plus** a declarative orphan-filter backstop in `build_working_context` (design rec: both — belt + suspenders), vs. a single locus. (Likely TECHNICAL FORK.)

## §8 — Audit envelope hint

**Medium (2 auditors: A code-correctness + tests; B docs/paperwork/ADR)**. Single-subsystem kernel fix with locked design; no cross-crate surface, no migration. Bump to Large only if the investigation surfaces an unexpectedly wide call-site cascade in the render path.

## §9 — Unblocks

- Removes the S2 hard-fail risk for any consumer (i-phi, baby-phi, future) that fires parallel tool calls while braking/revert is active.
- Hardens the braking machinery ahead of the MCP / combined-agent cluster (#67/#64/#68), where parallel tool use + revert co-occur more often.

## §10 — Risks

- **Render-path cascade**: per-call retain touches the hot working-context build; risk of subtle regression in the single-call (M=1) path. Mitigation: the §3 matrix's SAFE rows are explicit regression tests.
- **Pin/abandon tag migration subtlety** (`context.rs:577`): the mixed-class collision (R3) is the trickiest state; pin-wins is the conservative resolution (extra context can't 400).
- **Provider serializer assumptions**: no provider currently strips orphans — the fix must guarantee the invariant at render, not rely on serialization.

## §11 — Direct-approval criteria fit

Does **NOT** auto-approve (`approval=yes` forced): it is the first phi-core kernel chunk (new lane), touches load-bearing braking machinery, and carries residual forks (F1/F2). User lock at gate-1 is required.
