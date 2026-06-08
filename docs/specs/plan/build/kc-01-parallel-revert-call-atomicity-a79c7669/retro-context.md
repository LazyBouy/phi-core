<!-- Last verified: 2026-06-08 by Claude Code -->

# KC-01 — retro-context (for the next joint-retro)

> Captured at Phase 5→6 (batched-retro default; no retrospector dispatched this cycle). KC-01 is the **first standalone phi-core kernel chunk** (KC namespace); closed GitHub #77 / D-TEST-0072 (S2 parallel-revert call-atomicity). Cycle `a79c7669`.

## Cycle context

- **Scope**: render call-atomicity in the braking/revert machinery (R1–R4 + `(node_id, tool_call_id)` composite join + `streaming.rs` backstop). 2 render functions, 2 files. No public API / persisted-field / migration change.
- **Flow**: investigation (opt-in Phase 0.5) → plan (iter-1, skip-condition) → implement → 2 auditors → gate-4 → seal → live close-gate.
- **Result**: clean cycle — 0 Trivial/Tactical/Architectural patches; 1 plan iter; 1 audit iter (both PASS 8/8); unit + live close-gate PASS; #77 closed.

## Observations worth carrying

**Process — wins**
- The **opt-in P0 investigation paid off concretely**: it reproduced BOTH #77 BROKEN matrix rows on live `dev` before the plan existed, corrected the issue's line numbers, and established the `(node_id, tool_call_id)` composite recommendation from id-provenance evidence (Mock `mock-tool-{i}` collision). The investigation-grounded plan → a single clean iter-1 audit cycle.
- The **phi-core kernel lane worked first-try** end-to-end (lane shipped `e90fe61` the same session). Host-cargo MUST-RUN, no-CI-guards, leverage-check-N/A → kernel-minimality substitution all behaved as designed.
- The **render-transcript close-gate discipline (`[[feedback_render_transcript_close_gate]]`) demonstrably earned its keep**: unit tests proved atomicity, but *reading the live transcript* surfaced #80 / D-TEST-0075 (the completion-onto-call-node keep-first/drop-rest asymmetry) that no unit test flagged. Strongest single data-point yet for the discipline.

**Process — gaps / standards candidates**
- **`chunk-archive-plan` is not phi-core-aware** (only baby-phi/i-phi). Orchestrator minted the cycle folder + `_cycle-index.md` manually (phi-core reuses baby-phi's exact `docs/specs/plan/build/` shape). Logged as cycle-audit §6 D-3. → **Standards candidate #1**: add a `phi-core` PROJECT_ROOT branch to `chunk-archive-plan` (+ `chunk-template-validate-locked-appendix` hard-assertion path) so the next phi-core cycle archives via the skill.
- **Pre-existing `dev` fmt drifts** (CC-22-era `basic_agent.rs`/`agent_test.rs`) surfaced at gate-2 `fmt --check` and blocked the crate gate. Fixed as a **separate scoped commit `2dc2c40`** (kept the KC-01 seal clean). → minor: phi-core `dev` had un-formatted code landed by an earlier cycle; the pre-commit hook may not be installed in the submodule's `.git/hooks`.

**Code observations**
- The **linear node stamping** (parallel results form `n0→n1→n2`, not siblings of `n0`) makes per-call result recovery **asymmetric**: only the first parallel result is a direct child of the call node; the rest are grandchildren. KC-01's direct-child (`parent_id == node_id`) recovery is atomic but, for a *pinned* revert onto the call-node, keeps the first result + drops the siblings → #80 / D-TEST-0075. A future sibling-stamping or by-tool_call_id-within-abandoned-span recovery would change this (ruled out for #77 as too heavy; revisit if #80 routes to implementation).

**Reusable pattern**
- **phi-core kernel close-gate via i-phi HTC**: build i-phi (phi-e2e worktree) against the kernel fix (path-patched dep auto-picks it up), construct an HTC that elicits the exact runtime scenario (here: HTC-0009 parallel-tool fixture + a `revert_to_state` instruction + a post-revert `write_file`), run one live round-trip, read the transcript. → **Standards candidate #2**: consider codifying a phi-core close-gate template (the kernel has no daemon of its own, so it borrows a consumer's harness).

## Standards-update proposals (DRAFTED — not applied; joint-retro decides)

1. **`chunk-archive-plan` v5 — phi-core PROJECT_ROOT branch** (HIGH-value; closes the only KC-01 deviation). phi-core path == baby-phi shape; small.
2. **phi-core close-gate template** (MEDIUM) — codify the "build consumer against kernel fix + HTC + transcript-read" pattern for future KC chunks.
3. **phi-core pre-commit hook install check** (LOW) — the gate-2 fmt surprise suggests the submodule's hook isn't installed; a one-time `scripts/install-hooks.sh` run would prevent un-formatted code landing on `dev`.

## Cycle-folder artifacts

`plan.md` · `p0-investigation.md` · `audit-A-iter1.md` · `audit-B-iter1.md` · `cycle-audit.md` · `close-gate-result.md` · this `retro-context.md`. Live close-gate artifacts in the i-phi phi-e2e worktree at `docs/e2e-test/cycles/kc01-close-gate-a79c7669/`.

## Status

`audited-pending-retro` — joint-retro pending. Follow-up #80 / D-TEST-0075 (S3) open as a KC-02 candidate.
