<!-- Last verified: 2026-06-09 by Claude Code (amended at KC-02 #80: §D1.2's M=1 re-append + M=1 keep-whole superseded-in-part by ADR-0002 §D2.1 — pinned-revert disposition is now target-aware) -->
<!-- Last verified: 2026-06-08 by Claude Code (KC-01 #77/D-TEST-0072 — phi-core ADR-0001, first phi-core ADR; call-atomic render) -->

# phi-core ADR-0001 — Parallel-revert call-atomicity (call-atomic render)

**Status: Accepted**

> First phi-core ADR. Closes GitHub #77 / D-TEST-0072 (S2 runtime-correctness: parallel-revert dangling `tool_call`). Cite sub-decisions as `ADR-0001 §D1.<M>`.

## Forks

All three forks are **TECHNICAL** (no user-visible behaviour delta — every option closes the same S2 defect and renders the same call-atomic working context). Decided on engineering merit; locked at gate-1 at planner-rec, grounded by the Phase-0.5 P0 investigation (9 DEFINITIVE findings, 0 unresolved) against live `dev` 0.11.4.

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F1 | ingest-time id-uniqueness shape | **F1.b — `(node_id, tool_call_id)` composite** | rides the already-session-unique `node_id` (`alloc_node_id`, `context.rs:943`); zero new persisted state; collision-proof by construction. Raw `tool_call_id` (F1.a) is NOT session-unique — MockProvider mints `mock-tool-{i}` resetting per message (`mock.rs:219`, P0 F-8) so it cross-joins stale prior-turn results. Monotonic internal re-mint (F1.c) is the heaviest (round-trips a mint↔provider-id map through the wire serializer). |
| F2 | R3 × prun / decay-drop interaction | **F2.document-as-invariant** | provably non-reachable today (P0 F2): `prun` operates only on `inrun_context` and never touches the trunk parent-walk render; the decay-drop fires only inside `if abandon_class`, and an R3-kept node carries a live/pinned sibling so it is never abandon-class-collapsed. Adding a separate guard would be dead code guarding an unreachable interaction. |
| F3 | enforcement placement | **F3.both (per-call retain + declarative backstop)** | R1/R2/R3 MUST land in `collapse_abandon_class_cluster` (the only locus with per-cluster tag/window state + content ownership); a cheap declarative orphan-filter / empty-node backstop on the `streaming.rs` render pipeline is a locus-independent belt-and-suspenders that also covers any FUTURE trunk-assembly path (e.g. `build_trunk_context_with_policy`). |

## Context

GitHub #77 / D-TEST-0072 is an S2 runtime-correctness defect in the braking/revert machinery. The braking model is **single-call-per-node** throughout (P0 §2 surface map):

- `stamp_node_identity` (`run.rs:585`) stamps each new node's parent as the *previous* active node → results of one assistant node form a LINEAR chain `n0 → n1 → n2`, never siblings.
- `build_trunk_context` (`context.rs:860`, the sole `parent_id` consumer) walks a single linear parent chain.
- `collapse_abandon_class_cluster` (`context.rs:440`) recovered the join key for the FIRST tool-call only (former `:507-510`), cleared whole nodes on abandon (former `:546`/`:575`), and re-appended only the first call's result on pinned (former `:628-646`).

A **parallel** assistant node (N tool_calls → N linearly-chained result nodes) breaks the atomic-cluster invariant: reverting INTO the cluster (e.g. a `completion`/`step-summary` pinned revert landing on the first result) keeps both calls on-trunk but only the first call's result, leaving the 2nd..Nth call as a `tool_use` with no `tool_result` → a hard provider 400 (P0 F-4 / F-5, reproduced on live `dev`).

KC-01 makes the revert render **call-atomic**: drop/keep PER CALL keyed on the native `(node_id, tool_call_id)` composite join, localized to the revert render path. No node-model change, no trunk-walk change, no new persisted field, no migration.

## Sub-decisions

### §D1.1 — Join key = `(node_id, tool_call_id)` composite (resolves F1)

*Net-new surface* (no prior behaviour to preserve at this granularity): the native `Content::ToolCall.id` ↔ `Message::ToolResult.tool_call_id` join (`content.rs:143`, populated verbatim at ingest, P0 F-7) existed but was not session-namespaced. KC-01 consumes that native join and namespaces it with the already-session-unique `node_id`: a call's result is "live for call `(node_X, id)`" iff a `ToolResult{tool_call_id == id}` is on the trunk; an off-trunk result is re-appendable iff `result.parent_id == node_X` (direct child of THIS call node). The composite makes the direct-child test collision-proof against an id-reuse provider (MockProvider `mock-tool-{i}`, #77 F-8). An id-less call gets a deterministic synthetic id `format!("synth-{node_id}-{idx}")` so every call carries a join key. NO new persisted field; NO change to provider id semantics — the namespacing is render-internal only.

### §D1.2 — R1 per-call retain

> Superseded-in-part by ADR-0002 §D2.1 (2026-06-09): pinned-revert disposition is target-aware — onto-call-node reclaims (drop, no re-append); into-cluster re-appends siblings by call-node membership (not the direct-child gate). KC-01's M=1-byte-identical no longer holds for the pinned-onto-call-node / into-cluster sub-cases.

**Pre-existing-behaviour:** `collapse_abandon_class_cluster` shipped whole-node `content.clear()` (former `:546`/`:575`) and a first-call-only pinned re-append (former `:628-646`) at CC-18 / #73. KC-01 replaces the pinned re-append with id-keyed PER-CALL removal: a pinned assistant node emits only the calls whose result is live on the trunk (or re-appendable as a direct child); a call whose result was abandoned (off-trunk, not a direct child) is dropped. The M=1 single-call path is preserved byte-identical (the SAFE regression rows guard it — `row_revert_to_last_result_stays_safe`, `pinned_class_keeps_cluster_whole_no_dangling_call`).

### §D1.3 — R2 per-call collapse

**Pre-existing-behaviour:** an abandon-class collapse cleared the whole parent node's content (former `:575`). KC-01 collapses only the matched call (keyed on the collapsing result's id), breadcrumbs it, and leaves sibling calls intact when a live sibling exists (R3). When NO live sibling exists, the whole-node clear is retained (M=1 path byte-identical — `abandon_class_collapses_when_tip_is_tool_result_collapses_whole_cluster` stays green).

### §D1.4 — R3 mixed-class → pin wins (tag-migration site)

**Pre-existing-behaviour:** the tag-migration site (former `:577`) unconditionally migrated the collapsing tag onto the parent and cleared the parent's content. KC-01 keeps a node WHOLE when it carries both an abandon tag (migrated from a collapsed sibling result) AND a pinned/live sibling call (a sibling whose result is on-trunk) — the helper `call_node_has_live_sibling` gates this. Only the specific collapsing call is removed per-call; the live sibling cluster is real context that "can't 400". A kept-whole node is never the decay-drop target.

### §D1.5 — R4 empty-node handling

*Net-new* render rule. After per-call retain, an assistant node left with zero calls and no non-blank text is dropped entirely (an empty assistant turn is itself a provider 400); a node retaining text is kept as plain assistant text. Lives in the declarative backstop (`enforce_call_atomic_backstop`) on the `streaming.rs` render pipeline so it covers any path assembling a trunk.

### §D1.6 — F2 documented non-interaction invariant (resolves F2)

*Net-new (documentation-only; no prior behaviour to preserve and no machinery added).* R3 × prun / decay-drop is provably non-reachable on current `dev` (P0 F2): `prun` (`run.rs:601-655`) operates only on `inrun_context` and never touches the trunk parent-walk render; the reachable decay-drop (`context.rs:608-616`) fires only inside `if abandon_class`, and an R3-kept node carries a pinned/live sibling so it is never abandon-class-collapsed. NO separate mechanism is added — adding one would be dead code guarding an unreachable interaction. The invariant (a future decay-drop of an abandon tag migrated onto an R3-kept node should still keep the node — extra context can't 400) is documented in the `collapse_abandon_class_cluster` doc-comment rather than implemented as machinery.

### §D1.7 — F3 both-loci enforcement (resolves F3)

*Net-new locus (the `streaming.rs` declarative backstop is additive; no prior behaviour to preserve).* R1/R2/R3 land in `collapse_abandon_class_cluster` (the only locus with per-cluster tag/window state). A declarative orphan-filter + empty-node backstop (`enforce_call_atomic_backstop`, `streaming.rs`) runs as the FINAL linear pass on the assembled revert-mode trunk: it drops any on-trunk `tool_use` whose `tool_result` is absent and any empty assistant node, regardless of which path built the trunk. Additive, no state; the well-formed M=1 / non-revert path is byte-identical (no orphan to drop, no empty node to remove).

## Cross-references

- **Concept doc**: `docs/concepts/concept-brake.md:192` (co-keep constraint, now call-atomic) + `src/types/context.rs` `collapse_abandon_class_cluster` doc-comment (atomic-cluster invariant rewritten node-atomic → call-atomic).
- **Closed drifts/issues**: GitHub #77 / D-TEST-0072 (S2, parallel-revert dangling `tool_call`).
- **Prior ADRs**: none (first phi-core ADR).
- **Forward-scope row**: `docs/specs/plan/forward-scope/kc-01-parallel-revert-call-atomicity.md`; plan `docs/specs/plan/build/kc-01-parallel-revert-call-atomicity-a79c7669/plan.md`; P0 investigation `…/_p0-investigations/kc-01-…-p0-investigation.md`.

## Consequences

### For consumers (i-phi / baby-phi / future)

- Removes the S2 hard-fail (provider 400) for any consumer that fires parallel tool calls while braking/revert is active — a kernel-correctness property every consumer benefits from. Zero consumer-specific logic; zero i-phi leakage (`[[feedback_phi_core_kernel_minimal]]` carve-out: genuine general kernel fix).
- Hardens the braking machinery ahead of the MCP / combined-agent cluster work (#67 / #64 / #68), where parallel tool use + revert co-occur more often — forward-routing note.
- No public API signature change, no new persisted field, no migration (P0 F-7). The change is confined to two render functions in two files; the M=1 single-call path is byte-identical.

## Revisit triggers

1. A FUTURE path makes the decay-drop reach an R3-kept node (re-opens §D1.6 — the documented invariant becomes a real fork to lock).
2. A provider id scheme makes the `(node_id, tool_call_id)` composite key insufficient (re-opens §D1.1).
3. A new trunk-assembly path bypasses BOTH F3 loci (re-opens §D1.7 — the backstop must cover it).
4. Per-call retain shows an M=1 regression (re-opens §D1.2 — the SAFE regression rows are the guard).

## Verification

```bash
# Targeted regression (the promoted matrix + R4 + R3): 13 → 20 collapse_abandon tests
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture

# Full crate suite + lint
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check
```

Both BROKEN rows (`row_pinned_revert_to_first_result_now_call_atomic`, `row_pinned_revert_to_call_node_now_call_atomic`) flip to `dangling=[]`; the 3 SAFE rows stay green; R4 + R3 pass; the 13 existing tests stay green (M=1 byte-identical).

**Live close-gate (orchestrator-run, per `[[feedback_render_transcript_close_gate]]`):** an i-phi HTC with a ≥2-parallel-tool turn (e.g. `memory_help` + `skill_help`, CC-23 HTC-0009 shape) + a pinned `completion`/`step-summary` `revert_to_state` landing into the cluster — render + READ the transcript: the provider request carries balanced `tool_use` ↔ `tool_result` counts (or the cluster collapses atomically) and SENDS without an Anthropic/OpenAI 400. Not bootable at phi-core unit level (needs a live provider round-trip).
