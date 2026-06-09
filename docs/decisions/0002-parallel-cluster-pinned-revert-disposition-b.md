<!-- Last verified: 2026-06-09 by Claude Code (KC-03 #81/D-TEST-0076 — §D2.1 Superseded-in-part by ADR-0003 §D3.1: the result-node keep-whole re-append is replaced by the simple tail-shrink contract) -->
<!-- Last verified: 2026-06-09 by Claude Code (KC-02 #80/D-TEST-0075 — phi-core ADR-0002; target-aware pinned-revert disposition (B); SUPERSEDES-in-part ADR-0001 §D1.2) -->

# phi-core ADR-0002 — Parallel-cluster pinned-revert disposition (B): reclaim-on-call-node + keep-cluster-on-result-node

**Status: Accepted**

> Second phi-core ADR. Closes GitHub #80 / D-TEST-0075 (S2 runtime-correctness: parallel-cluster pinned-revert silent loss of gathered work). Cite sub-decisions as `ADR-0002 §D2.<M>`. SUPERSEDES-in-part ADR-0001 §D1.2 (the M=1 direct-child re-append + M=1 keep-whole).

## Forks

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F1 | pinned-onto-call-node / into-cluster revert disposition (**product-semantic** — which gathered work survives is user-perceivable) | **F1.B — reclaim-on-call-node + keep-whole-cluster-on-result-node** (USER-LOCKED 2026-06-08; USER-DIVERGENT from the iter-1 (a)-only draft) | (B) is the broader fix the user chose to fully close the silent-loss class: a revert onto the CALL node (sealing past the whole sub-task) reclaims the abandoned cluster uniformly into the durable breadcrumb; a revert INTO the cluster (onto a RESULT node) KEEPS the WHOLE gathered cluster verbatim (every off-trunk sibling re-appended by call-node membership). Eliminates both the silent half-keep / re-fetch loop #80 surfaced AND the keep-first/drop-rest asymmetry, with no provider 400 in any cell. P0 §10 validated (B) end-to-end through the verbatim production pipeline (all 10 LOSS cells fix, 0 regression, 0 new orphan/dangling/400). |
| F2 | abandon-class onto-call-node / onto-result consistency (**TECHNICAL** — no user-visible delta) | **F2.confirm-uniform — no-op** | P0 F-4 / F-5 / §10.4: abandon-class already collapses the cluster fully + symmetrically to the breadcrumb on the load-bearing call-node/first-result axis; the asymmetric mid/last-result cells are benign target-position effects (live siblings legitimately kept atomic; the abandoned tail reclaimed), not leaks. All 16 abandon-class cells are byte-identical pre/post-(B) (the (B) branch lives in the *pinned* arm only). Adding any abandon-path change would be dead code. |
| F3 | KC-03 split decision (**TECHNICAL** — no user-visible delta) | **F3.absorb — render-pass-only, no node-model change** | P0 F-6 / F-8 / §10.6: the fix is render-pass-only at `retain_pinned_calls_on_node` + a 1-token caller signature thread; no `build_trunk_context` / sibling-stamping change. The only split-worthy item — a true-sibling node model — is UNNECESSARY for #80 closure (the linear chain + the call-node-membership re-append fully closes all 10 LOSS cells); splitting would manufacture scope. The true-sibling node model is a deferrable Revisit trigger, not a KC-03 blocker. |

## Context

GitHub #80 / D-TEST-0075 is an S2 runtime-correctness defect in the braking/revert machinery, exposed once KC-01 made the revert render call-atomic (ADR-0001). The Phase-0.5 P0 investigation drove an exhaustive `category × revert-target × arity` keep/drop/balance grid (P0 §3, 9 DEFINITIVE findings) through the verbatim production render pipeline:

- **0 BROKEN-400 cells** — the #80 "hidden hard 400" hunch does NOT realize. `enforce_call_atomic_backstop` (`streaming.rs:130`) orphan-filters every imbalance the per-call logic might leave; balance is `OK` in every cell (P0 F-1).
- **10 LOSS cells** — all are the pinned (`completion` / `step-summary`) keep-first/drop-rest cells: `completion`/`step-summary` × {`call-node`, `first-result`} × {arity-2, arity-3} = 8, PLUS × `mid-result` × arity-3 = 2 (the §3 prose under-counted as 8; the matrix table marks 10 bolded LOSS rows — P0 §10.3 / §10.8 surprise #2). Gathered sibling work is silently dropped and only a *decaying* breadcrumb remains — genuine lost work for a PINNED category whose contract is "keep the span" (P0 F-2 / F-3).

The defect is **silent loss, not provider rejection.** The single-call-per-node braking model (`build_trunk_context` is the sole linear `parent_id` consumer, `context.rs:1074`) stamps a parallel cluster's N results as a LINEAR chain (`result_a.parent = call_node`, `result_b.parent = result_a`, …), so only `result_a` is a *direct child* of the call node; the KC-01 §D1.2 direct-child re-append therefore keeps `result_a` and drops the off-trunk grandchildren `result_b`/`result_c`. A second P0 pass (P0 §10) DESIGNED + VALIDATED the broader fix (B) against the full 32-cell matrix + 5 edge fixtures via a throwaway implementation through the verbatim pipeline (now reverted; tree clean). The matrix forced ONE design refinement — the on-trunk keep-verbatim gate (P0 §10.6 / §10.8 surprise #1) — which is part of the mechanism, not an option.

## Sub-decisions

### §D2.1 — F1(B) reclaim-on-call-node + keep-whole-cluster-on-result-node (resolves F1; SUPERSEDES ADR-0001 §D1.2 M=1 re-append + M=1 keep-whole)

> **Superseded-in-part by ADR-0003 §D3.1 (2026-06-09, KC-03 / #81 / D-TEST-0076):** clause (iii) — the result-node keep-whole re-append (the off-trunk parallel sibling re-materialised by call-node membership) — is REPLACED by the simple tail-shrink contract: a pinned revert onto X SHRINKS every node strictly after X uniformly across categories (Rule 1), so the pinned arm converges to the abandon arm; the gathered tail is NOT re-appended. The on-trunk-keep gate (clause i) is PRESERVED as Rule 1's don't-over-shrink guard. KC-02's (B) keep-whole was the misread #81 corrects. §D2.2 / §D2.3 / §D2.4 carry forward unchanged.

**Pre-existing-behaviour:** ADR-0001 §D1.2 (KC-01) re-appended the off-trunk-direct-child result and kept the M=1 cluster whole (former `retain_pinned_calls_on_node` direct-child gate `lm.parent_id == Some(this_node_id) && tool_call_id == call_id`). KC-02 threads the caller's already-computed `node_is_call` as a new `tag_on_call_node: bool` parameter and makes the per-call disposition TARGET-AWARE:

- **(i) result on-trunk → KEEP always** — the load-bearing mid-trunk gate: a pinned cluster the model continued PAST (its results on-trunk) is kept verbatim, never reclaimed. Without this gate, `pinned_cluster_mid_trunk_kept_verbatim` REGRESSES (P0 §10.6 / §10.8 surprise #1).
- **(ii) result off-trunk + `tag_on_call_node` → DROP, reclaim to breadcrumb** — the model sealed past the whole sub-task; the abandoned cluster collapses uniformly into the durable breadcrumb (the (a) semantic for the call-node case).
- **(iii) result off-trunk + tag-on-result-node → RE-APPEND by CALL-NODE MEMBERSHIP** (`tool_call_id == call_id` against the forensic log, reaching the off-trunk parallel grandchildren the direct-child gate could not). A genuine post-cluster follow-on whose `tool_call_id ∉ this node's calls` never matches → stays dropped/sealed-past.

**Explicitly:** ADR-0001 §D1.2's "M=1 byte-identical" and M=1-keep-whole claims no longer hold for the pinned-onto-call-node / into-cluster sub-cases. This is **intended** (full silent-loss-class closure), not a regression. All 10 LOSS cells flip: 4 call-node → `[DROP,…]` + breadcrumb (symmetric reclaim); 6 result-node (first-result a2+a3, mid-result a3, both pinned categories) → `[PAIR,…]` whole-cluster (P0 §10.2 / §10.3). Render-pass-only: `self.messages` (forensic log) is `.iter().find()` + `.clone()` reads ONLY, never mutated (the only `self.messages` diff is the find-predicate rename `direct_child` → `cluster_result`). `build_trunk_context` / the node model / `enforce_call_atomic_backstop` (`streaming.rs:130`) untouched.

### §D2.2 — F2 abandon-class confirmed-uniform (no-op) (resolves F2)

*Net-new confirmation — no prior behaviour changed.* Abandon-class (`failure` → Lesson, `tangent` → Finding) onto-call-node + first-result already fully + symmetrically collapse the cluster to the breadcrumb (`[DROP,…]`, no surviving call — P0 F-5 probe `[ABANDON call-node a3] any_call=false`); the result-node path moves the tag to the parent and clears identically. The asymmetric abandon mid/last-result cells are benign target-position effects (the surviving siblings are LIVE on-trunk work legitimately kept atomic; the abandoned tip IS reclaimed, balance OK — P0 F-4), not the #80 keep-first/drop-rest leak. All 16 abandon-class cells are byte-identical pre/post-(B) (P0 §10.4); the (B) branch lives in the *pinned* arm only and abandon-class never reaches `retain_pinned_calls_on_node`. No abandon-path code change; the confirmation is captured here and in the promoted regression rows (the benign abandon-asymmetry cells ship as permanent coverage so a future change cannot silently regress them).

### §D2.3 — F3 absorb, no node-model change (resolves F3)

*Net-new confirmation — no prior behaviour changed.* The fix is render-pass-only: `retain_pinned_calls_on_node` gains the `tag_on_call_node` branch + the call-node-membership re-append, plus a 1-token caller signature thread; no `build_trunk_context` / sibling-stamping change, no KC-03 split (P0 F-6 / F-8 / §10.6). Production delta `+32 / −13` LOC in the fn + 1 caller token. A future true-sibling node model (stamp parallel results as real siblings of the call node) is the deeper-correctness option but is UNNECESSARY for #80 closure — it would only let the result-node re-append use the direct-child gate instead of call-node membership — and is surfaced as a deferrable Revisit trigger, not a KC-03 blocker.

### §D2.4 — the no-400 invariant is preserved by the backstop

*Net-new confirmation — no prior behaviour changed.* P0 F-1 + §10.4 proved 0 BROKEN-400 cells across all 37 cells (32 matrix + 5 edges). `enforce_call_atomic_backstop` (`streaming.rs:130`) is the final declarative pass: it orphan-filters any dangling call (a `tool_use` whose `tool_result` is absent on the trunk) and drops empty assistant nodes, AFTER the per-call disposition runs. F1(B) both DROPS calls (call-node case) AND RE-APPENDS results (result-node case); the backstop guarantees no orphan result and no dangling call survives → no provider 400. The promoted regression asserts `trunk_has_dangling_call` AND `trunk_has_orphan_result` both false in every exercised cell.

## Cross-references

- **Concept doc**: `docs/concepts/concept-brake.md:192` (co-keep constraint — pinned-revert disposition now target-aware) + `src/types/context.rs` `retain_pinned_calls_on_node` doc-comments (the `:738-754` fn comment + the `:691-709` mirror at the pinned branch, rewritten to the (B) 3-case target-aware form).
- **Closed drifts/issues**: GitHub #80 / D-TEST-0075 (S2, parallel-cluster pinned-revert silent loss of gathered work).
- **Prior ADRs**: ADR-0001 §D1.2 (R1 per-call retain — **SUPERSEDED-in-part by §D2.1**: the M=1 direct-child re-append + M=1 keep-whole) + ADR-0001 §D1.5 (R4 empty-node handling — relied on here for the breadcrumb keep-as-text on an all-calls-dropped call-node-reclaimed cluster).
- **Forward-scope row**: `docs/specs/plan/forward-scope/kc-02-parallel-cluster-revert-matrix.md`; plan `docs/specs/plan/build/kc-02-parallel-cluster-revert-matrix-d835a363/plan.md`; P0 investigation `…/_p0-investigations/kc-02-…-p0-investigation.md` §3 (matrix) + §10 (the (B) design + validation).

## Consequences

### For consumers (i-phi / baby-phi / future)

- Pinned reverts now have a complete, target-aware disposition: a revert onto the CALL node reclaims the abandoned parallel tail uniformly into the breadcrumb; a revert INTO the cluster (onto any RESULT node) keeps the WHOLE gathered cluster verbatim. No silent half-keep / re-fetch loop, no provider 400 in any cell — a kernel-correctness property every consumer that fires parallel tool calls while braking benefits from. Zero consumer-specific logic; zero i-phi leakage (`[[feedback_phi_core_kernel_minimal]]` carve-out: genuine general kernel fix).
- Hardens the braking machinery ahead of the MCP / combined-agent cluster work (#67 / #64 / #68), where parallel tool use + revert co-occur heavily and gathered parallel work is expensive to re-fetch — forward-routing note.

### For KC-01 / ADR-0001

- ADR-0001 §D1.2's off-trunk-direct-child re-append + M=1 keep-whole are superseded-in-part by §D2.1 (an inline note is added at ADR-0001 §D1.2 + its verified-header bumped). KC-01's "M=1 byte-identical" no longer holds for the pinned-onto-call-node / into-cluster sub-cases (intended). **3** KC-01 tests are updated to the new (B) dispositions (`pinned_class_keeps_cluster_whole_no_dangling_call`, `row_pinned_revert_to_call_node_now_call_atomic`, `row_pinned_revert_to_first_result_now_call_atomic`); `pinned_cluster_mid_trunk_kept_verbatim` stays GREEN unchanged (the on-trunk-keep gate). No public API signature change, no new persisted field, no migration.

## Revisit triggers

1. A future **true-sibling node model** (stamp parallel results as real siblings of the call node) is adopted → re-opens §D2.3 (the result-node re-append could then use the direct-child gate instead of call-node membership). Deferrable; NOT needed for #80 closure.
2. A product decision that a pinned `completion` onto-call-node MUST also preserve ALL gathered work verbatim (not reclaim) → re-opens §D2.1 clause (ii).
3. A new revert-render path bypasses `retain_pinned_calls_on_node` → re-opens §D2.3 (the absorb-vs-split boundary; the call-node-membership re-append must cover it).
4. The breadcrumb-reclaim path stops firing for an all-calls-dropped call-node (e.g. a weave/R4 change makes a reclaimed pinned node render empty) → re-opens §D2.1's reliance on the weave + R4 keep-as-text path.
5. The on-trunk-keep gate (clause (i)) interacts badly with a future decay / window change → re-opens the mid-trunk-kept-verbatim guarantee (`pinned_cluster_mid_trunk_kept_verbatim` is the guard).

## Verification

```bash
# Targeted regression (the promoted 32-cell matrix + 5 edges + the 3 updated KC-01
# flips + the on-trunk-gate probe): 20 → 30 collapse_abandon tests.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture

# Full crate suite + lint + format.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check
```

The 10 LOSS cells flip (4 call-node → `[DROP,…]` + breadcrumb; 6 result-node → `[PAIR,…]` whole-cluster); the 3 KC-01 tests flip to the new (B) dispositions; `pinned_cluster_mid_trunk_kept_verbatim` stays green; the no-400 invariant (`trunk_has_dangling_call` + `trunk_has_orphan_result` both false) holds across every exercised cell; the abandon-class + no-loss cells stay unchanged.

**Live close-gate (orchestrator-run, per `[[feedback_render_transcript_close_gate]]`):** the FULL live grid — both drivers (`deepseek-chat-v3-0324` + `minimax-m2.7`) × the 4 categories — transcript-read, asserting correct disposition end-to-end: **call-node reverts reclaim to the breadcrumb** (sealed-past cluster summarised, no half-keep) AND **result-node reverts KEEP the whole cluster** (no sibling lost, no re-fetch loop); no 400 in any cell. Not bootable at phi-core unit level (needs a live provider round-trip).
