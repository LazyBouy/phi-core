<!-- Last verified: 2026-06-09 by Claude Code -->

# KC-03 live close-gate result — #81 / D-TEST-0076 simple tail-shrink contract

**Cycle:** `24de5309` · **phi-core seal:** `69b5ddf` · **Date:** 2026-06-09
**Harness:** i-phi HTC daemon (phi-e2e worktree `dev-v0-e2e`), release-rebuilt against patched phi-core 0.11.4 (`69b5ddf`) via `[patch.crates-io]`. Cycle folder: `i-phi/docs/e2e-test/cycles/kc03-close-gate-24de5309/`.
**This is a DISPOSITION-asserting close-gate** (the explicit KC-02 lesson — `[[feedback_render_transcript_close_gate]]`): the rendered per-turn **wire request** was READ and each contract rule asserted, NOT merely no-400. Both strongest open-source drivers (`[[feedback_htc_cohort_strongest_open_source]]`).

## Verdict: PASS

The simple tail-shrink contract (R1/R2/R2a/R3) holds **live, on the wire, on both drivers**. The exact #81 defect (KC-02's keep-whole on a result-node revert) is fixed, and every one of the user's four observations is corrected, verified in the actual provider payload.

## The decisive cell — `KC03-SHRINK` on deepseek (clean single revert)

Scenario: parallel `memory_help` + `skill_help` in one node, then `revert_to_state(completion, target=n1)` onto the first result (the memory result). The exact #80/#81 shape — where KC-02 (B) wrongly KEPT the whole cluster.

**Post-revert turn-3 wire request (what the model received):**
```
[2] assistant [n0]  tool_calls = [memory_help]          ← skill_help call SHRUNK (Rule 3)
[3] tool [n1] [memory …·ShortTerm·tags] Decided on 2026-06-07: …us-east-1. [outcome: Gathered both facts… (skill_help abandoned)]
[4] user [continue_after_revert] You just reverted to node n1. …
```
| Rule | Assertion | Result |
|---|---|---|
| **R1** tail shrunk | the `skill_help` result (the strictly-after sibling) is ABSENT from the wire | ✓ (KC-02 kept it) |
| **R3** orphan call removed | `[n0]` carries ONLY `memory_help`; `skill_help`'s orphan call surgically removed by id | ✓ |
| **R2a** append order | memory body (idx 89) precedes the `[outcome: …]` annotation (idx 256) — summary AFTER content | ✓ (user obs #2 — the sandwiching is fixed) |
| truthful list | breadcrumb names `skill_help` (shrunk), NOT `memory_help` (kept) | ✓ (user obs #4) |
| single label | `[outcome: …]`, NOT the `[outcome: reverted past: …]` double-label | ✓ (user obs #3) |
| no 400 | 0 error bodies in the wire; 0 error markers in the daemon log | ✓ |

4 turns, 1 revert, AgentEnd reached, output `release-facts.txt` correct.

## Second driver — `KC03-SHRINK` on minimax-m2.7 (31-turn heavy-iteration stress)

minimax ran 31 turns (re-gathering + reverting repeatedly) and completed `write_file`. The render invariants hold under heavy iteration:
- **Shrink signature present** (turn-3 + turn-4): an assistant node carrying ONLY `memory_help` (skill_help call removed), the skill result absent, the breadcrumb naming `skill_help abandoned` — the same disposition as deepseek.
- **Single-label:** 137 `[outcome: …]` breadcrumbs, **0** `[outcome: reverted past: …]` double-labels.
- **Append-order:** 122 append-after-content occurrences (breadcrumb after the node body).
- **No 400 / dangling / orphan** across all 31 turns (0 error markers in the daemon log; the provider would 400 on any imbalance).

(Harness note: the minimax `events.jsonl` is empty — the `/attach` SSE reader hit the 240s deadline on the 31-turn run; a harness capture gap, NOT a product defect. The 31 captured `wire/*.request.json` payloads + the daemon log are the authoritative model-input record and were read directly.)

## Category coverage

The `completion` (pinned) arm — the arm KC-03 CHANGED — is proven on both drivers. The `failure`/`tangent` (abandon) arm is **UNCHANGED by KC-03** (P0 F-8: it was already the correct shrink+replace template the pinned arm converged toward) and is covered by the unchanged `matrix_abandon_class_*` cells + the new `abandon_revert_summary_replaces_target_content` unit cell — so it needs no live re-proof. `step-summary` (Checkpoint) is byte-equivalent to `completion` in the converged arm (both pinned; unit cells run both `Outcome` + `Checkpoint`).

## Evidence locations (under the cycle folder)

- `fixtures/<id>/fixture/sessions/<sid>/wire/*.turn-N.request.json` — the EXACT HTTP request per turn (the source-of-truth read surface).
- `stdout-stderr/<ID>.daemon.log` — 0 error/400/dangling/orphan markers (both cells).
- `transcripts/<ID>.transcript.txt` — product wire render.
- output `release-facts.txt` (both fixture holders) — facts correct (no information lost).

## Conclusion

The S2 contract is correct end-to-end. KC-02's keep-whole misread is removed; the pinned arm shrinks the tail uniformly, removes the orphan call by id, appends the summary after content with a single truthful label, and sends clean — verified in the model's actual input on both drivers. **#81 / D-TEST-0076 closed; #80 corrected.**
