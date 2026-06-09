<!-- Last verified: 2026-06-09 by Claude Code -->

# KC-02 live close-gate result — #80 / D-TEST-0075 (B) fix

**Cycle:** `d835a363` · **phi-core seal:** `ba1244c` · **Date:** 2026-06-09
**Harness:** i-phi HTC daemon (phi-e2e worktree `dev-v0-e2e`), release-rebuilt against patched phi-core 0.11.4 (`ba1244c`) via the `[patch.crates-io]` path. Cycle folder: `i-phi/docs/e2e-test/cycles/kc02-close-gate-d835a363/`.
**Per `[[feedback_render_transcript_close_gate]]`:** every cell's rendered per-turn wire context was READ (not just "wire produced"). **Per `[[feedback_htc_cohort_strongest_open_source]]`:** both strongest-available open-source drivers exercised.

## ⚠️ CORRECTION (2026-06-09, user-surfaced) — this PASS validated the WRONG invariant

**Superseded by [D-TEST-0076 / #81].** On user review of `KC02-RECLAIM` turn-3 **request.json**, the (B) semantics this gate "passed" are themselves a misread of the intended revert contract. The wire shows that a `completion` revert onto result-node `n1` does **not** shrink the post-target tail: the sibling result `n2` (skill_help) survives in full, with its tool_call still in `n0`, while the breadcrumb claims `skill_help abandoned`. The intended contract is the simple **Rule 1 shrink-tail / Rule 2 abandon-replace-vs-pinned-add / Rule 3 surgical-call-removal-by-id** model — NOT (B)'s "keep-whole-cluster-on-result-node" re-append. This gate's checks (no-400, no-orphan, facts-present) are all true but do **not** verify the disposition, so they read PASS over a wrong behavior. The fix is a new cycle (re-implement to the simple contract, re-author the unit matrix, update the `revert_to_state` tool docs, re-run a disposition-asserting close-gate). The "Verdict: PASS" below is retained verbatim for the audit trail but is **NOT** authoritative — see D-TEST-0076.

## Verdict: PASS [RETRACTED — see CORRECTION above]

The (B) target-aware pinned-revert disposition holds **live, end-to-end, on real providers**, across both new dispositions and both drivers. **Zero provider 400 across all 27 turns. Zero silent loss. The exact #80 defect is demonstrably fixed.** *(Retained for the audit trail; superseded — the no-400/no-orphan invariant was the wrong bar; the disposition itself is wrong per D-TEST-0076.)*

## Cells

| Cell | Driver | Turns | Reverts | Disposition shown live | 400 / dangling / orphan | Output |
|---|---|---|---|---|---|---|
| RECLAIM (`KC02-RECLAIM`) | deepseek-chat-v3-0324 | 7 | 4 | completion revert onto result-node `n1`: sibling `n2` kept verbatim + `n1` carries the reclaim breadcrumb `[outcome: reverted past: Facts gathered: (1) eu-west-2…; (2) rc-<version>-<yyyymmdd>]` | NONE | both facts correct |
| RECLAIM (`KC02-RECLAIM-MM`) | minimax-m2.7 | 16 | 8 | completion revert reclaims the WHOLE parallel cluster to a breadcrumb naming both abandoned calls: `[outcome: reverted past: Fact 1: eu-west-2 … Fact 2: rc-<version>-<yyyymmdd>. (memory_help, skill_help abandoned)]` | NONE (8 reverts on parallel clusters, all clean) | both facts correct |
| KEEP-WHOLE (`KC02-KEEPWHOLE`) | deepseek-chat-v3-0324 | 4 | 1 | clean single completion revert onto result-node `n1`: **off-trunk grandchild `n2` re-appended by call-node membership** → whole cluster survives ([PAIR,PAIR]) | NONE | both facts correct |

## The #80 defect, fixed (KEEP-WHOLE cell — the load-bearing evidence)

With linear stamping `n0(call) → n1(memory result) → n2(skill result)`, a completion revert onto `n1` makes `n2` an **off-trunk grandchild** of the call node `n0`.

- **Pre-revert (turn-2 wire):** model saw `[n0]` call-node + `[n1]` memory result + `[n2]` skill result — full cluster, balanced.
- **Post-revert (turn-3 wire):** `[n2]` skill result **KEPT verbatim** + `[n1]` reverted-to node with the outcome breadcrumb. **Both siblings present.**
- **Under KC-01's direct-child gate**, `n2` (a grandchild, not a direct child of `n0`) would NOT be re-appended → **silently dropped** = exactly the #80 silent-loss defect.
- **Under (B)'s call-node-membership re-append** (`tool_call_id == call_id`), `n2` survives. The defect is gone, demonstrated on a live provider round-trip.

## Method / evidence locations (per cell, under the cycle folder)

- `events/<ID>.events.jsonl` — AgentEnd reached in every cell; tool-execution + RevertApplied counts.
- `fixtures/<id>/fixture/sessions/<sid>/wire/*.turn-N.request.json` + `*.response.json` — the EXACT HTTP request the model saw + the provider response per turn (the source-of-truth read surface). Scanned for `"error"` / `400` / `invalid_request` / dangling/orphan → **0 matches in every cell**.
- `stdout-stderr/<ID>.daemon.log` — 0 error/400/dangling/orphan markers in every cell.
- `transcripts/<ID>.transcript.txt` — product wire render (443 / 1125 / lines).
- output `release-facts.txt` (in each fixture holder) — both facts correct in all 3 cells (no information lost through any revert).

## Notes

- The automated `assert-htc.py` returns exit 2 (no per-HTC test-case `.md` frontmatter) — expected; the close-gate's authoritative signal is the orchestrator's transcript read + the no-400 wire scan, NOT the HTC frontmatter assert.
- Category coverage: `completion` (Outcome/pinned) exercised on both drivers. `step-summary` (Checkpoint/pinned) is byte-equivalent to `completion` in the (B) branch (both pinned/non-decayable; the unit matrix proved Outcome ≡ Checkpoint across all 32 cells — `matrix_pinned_*` tests run both `TagKind::Outcome` and `TagKind::Checkpoint`). The abandon categories (`failure`→Lesson / `tangent`→Finding) are F2 confirm-uniform (no code change; 16 abandon-class cells byte-identical pre/post-(B)). The live grid is opportunistic acceptance over the exhaustive 37-cell unit net (plan §10/§11).
