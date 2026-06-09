<!-- Last verified: 2026-06-09 by Claude Code -->

# KC-03 cycle-audit — `revert_to_state` simple tail-shrink contract (R1/R2/R2a/R3)

**Cycle hex:** `24de5309` · **Project:** phi-core (kernel lane) · **Branch:** `dev` · **Crate:** 0.11.4
**Closes:** GitHub #81 / D-TEST-0076 (S2); **corrects** #80 / D-TEST-0075 (KC-02 (B) misread)
**Fork lock:** F1 render-pass-shrink (investigation-forced); F2–F5 TECHNICAL at investigation-rec · **Envelope:** Large (3 auditors A/B/C) · **Phases:** 4 · **Plan iterations:** 1 (investigation-first; evidence-forced forks)

---

## §1 — Verdict

**PASS — ready for seal + live close-gate.** Investigation-first (P0 reproduced 9 DEFINITIVE facts, orchestrator-verified); the fix is render-pass-only and a net production simplification (it removes the KC-02 (B) clause-(iii) re-append and converges the pinned arm onto the already-correct abandon arm). All gate-4 MUST-RUN green by orchestrator execution; 3 auditors PASS (A 7/7, B 8/8, C 7/7), zero FAIL/PARTIAL, all ran cargo in-audit matching gate-2. The remaining close steps are orchestrator-run: the live **disposition-asserting** close-gate (the explicit KC-02 lesson — assert the disposition, not no-400) + the #81 closure.

## §2 — Gate-4 MUST-RUN (orchestrator-executed; authoritative)

| Check | Command | Result |
|---|---|---|
| fmt | `cargo fmt -- --check` | exit 0 (clean) |
| full suite | `cargo test -j 4` | 28 binaries `ok`, **0 failed / 0 errors** |
| lib | (within above) | **251 passed / 0 failed** (244 baseline + 7 NEW) |
| `collapse_abandon` | `cargo test --lib collapse_abandon` | **36 passed / 0 failed** (30 baseline; +6; band [36,41]) |
| integration | (within full suite) | green (agent_loop / agent / session etc.) |
| clippy | `RUSTFLAGS="-Dwarnings" cargo clippy --all-targets` | Finished clean (0 warnings; removing `tag_on_call_node` enabled a `ptr_arg` improvement `&mut Vec`→`&mut [..]`) |

No `scripts/check-*.sh` exist (kernel lane) — the MUST-RUN list IS the gate. Cargo-clean honored (gate-2 + gate-4 ~10 GiB reclaimed; auditors self-cleaned).

## §3 — Kernel-minimality surface-discipline (substitutes phi-core-leverage-check — N/A kernel)

- **Blast radius:** 3 render-path functions — `retain_pinned_calls_on_node` (`context.rs:789`, now an associated fn `&mut [AgentMessage]`), `weave_braking_annotations` (`:914`) + the new `append_annotation_to_message` (`:38`), `apply_revert`'s breadcrumb name-set (`run.rs:841-888`). All gated by `active_node_id.is_some()` (revert-mode render).
- **Net production = a SIMPLIFICATION:** `retain_pinned_calls_on_node` shrank ~−35 body LOC (clause-(iii) re-append + the forensic-log re-fetch removed); +~30 weave append-variant; +~12 name-set guard.
- **No change to:** `build_trunk_context` (P0 F-2: already shrinks the tail), `apply_revert` active-pointer/reject (P0 F-1/F-3), the node model (P0 F-2), `enforce_call_atomic_backstop` (P0 F-8, stays as the belt — absent from diff). Forensic `self.messages` never read or mutated by the reworked fn.
- **No public-API / persisted-field / migration / tool-arg-schema change.** The `tag_on_call_node` param was REMOVED as vestigial (private fn, zero external callers). Tool DESCRIPTION text changed (not the schema).
- **Consumer-leakage grep:** `collapse_abandon_class_cluster|retain_pinned_calls_on_node` in `src/` minus `context.rs` → only `streaming.rs:237` (prod) + `run.rs` (test/doc). **Zero leakage.** Genuine general kernel fix.
- k8s N/A (library).

## §4 — Orchestrator gate-2 direct verification (read every production change)

- **`retain_pinned_calls_on_node`** (`:789-854`): clause (iii) gone; per call, on-trunk → keep (clause i, `:838`), off-trunk → drop the call by id via `ids_to_drop` (`:841-852`) — no re-append (R1) + orphan removed (R3). Converges to the abandon arm. **Correct.**
- **`weave_braking_annotations`** (`:914-977`): `[n<id>]` marker prepended (leading, `:966`); `[<kind>: …]` annotation appended after content (`:968`) → `[n<id>] <body> [<kind>: …]` (R2a); `reverted past: ` prefix stripped (`:956-959`) → single label. **Correct.**
- **`apply_revert`** (`run.rs:841-888`): `pinned_onto_result = !is_decayable && target_is_result_node`; step (i) (target cluster's own calls) skipped when `pinned_onto_result` (`:877`); strictly-after span always seeds. **Correct — truthful list.**
- **User-complaint-specific tests spot-read:** `pinned_revert_summary_appended_after_original_content` asserts `body_at < annot_at` + marker-leads + single-label-present + double-label-absent (complaints #2/#3); `pinned_revert_onto_result_breadcrumb_names_only_shrunk_tools` (run.rs) asserts the breadcrumb names skill_help (shrunk) but NOT memory_help (kept) on the exact #80 shape (complaint #4). **Rigorous, not weakened.**

## §5 — Paperwork ledger

| Artifact | State |
|---|---|
| plan.md (iter-1, user-approved) | archived; header 2026-06-09 |
| p0-investigation.md | archived beside plan.md |
| ADR-0003 `0003-revert-tail-shrink-contract.md` | NEW, Accepted, 7 sections, §D3.1–§D3.4, 6 Revisit triggers (Audit B PASS); SUPERSEDES ADR-0002 §D2.1 |
| ADR-0002 §D2.1 | superseded-in-part inline note + verified-header bump (Audit B claim 6 PASS) |
| `concept-brake.md` | tail-shrink note + header refresh (Audit B claim 3 PASS) |
| `retain_pinned_calls_on_node` doc-comments + module comment (`:3554`) | rewritten to the shrink form (the stale `:3555-3564` KC-02 (B) module comment refreshed at gate-3 per Audit C nit) |
| `RevertTool::description()` + `tool_help.rs` REVERT_HELP | rewritten to R1/R2/R2a/R3 (Audit B claim 2 PASS) |
| `_cycle-index.md` KC-03 row | appended (in-flight / pending) |
| audit-A/B/C-iter1.md | written, all PASS |
| #81 closure / #80 corrected-note | **orchestrator-pending** (post-seal + post-close-gate) |

## §6 — Deviations

| D-N | Class | Where | Description | Routing |
|---|---|---|---|---|
| D-1 | Plan-narrative incompleteness (Code-class, authorized) | plan §6/§7.0 inversion list | Plan listed **3** KC-02 tests to invert; P0 grounding found **5** (the planner missed 3 `edge_*` tests — `edge_post_cluster_followon_dropped`, `edge_mixed_on_off_trunk`, `edge_two_cluster_isolation` — that assert the same clause-(iii) keep-whole re-append) AND mis-flagged `matrix_pinned_last_result_and_post_cluster_no_loss` as needing a "split" (it reverts onto the tip → nothing after X → stays green). The implementer paused at P0; orchestrator verified all 4 claims by reading the tests + authorized the corrected scope (invert 5; last_result stays green). Direct consequence of the already-locked Rule 1 — no fork/contract/envelope change. Audit C verified the 5 inversions are intended + the only keep-whole inversions. |
| D-2 | Superseded-literal test updates (Code-class, in-scope) | `abandoned_branch_content_not_reintroduced` + `description_teaches_four_categories_as_budget_levers` | Two additional in-place test updates required by the authorized single-label (F3) + tool-doc deliverables — both asserted superseded render/doc literals (`reverted past: ` prefix; "do NOT shrink the kept span"). Same class as the 5 inversions; Audit C claim 5 verified. The sibling test asserting the STORED `tag.text` stays green (prefix-strip is render-only). |
| D-3 | Stale module comment (Trivial-multi, P7 direct-verification) | `context.rs:3555-3564` | A test-module COMMENT still narrated the KC-02 (B) keep-whole disposition (Audit C non-blocking nit). Orchestrator refreshed it to the KC-03 contract at gate-3 + verified via fmt + full-suite re-run (no auditor re-spawn; comment-only). |
| — | Audit-prompt-authoring miss | — | None. Audit C was widened to the 5-inversion scope at dispatch (the orchestrator carried the authorized correction into the prompt), so no false-FAIL. |

## §7 — Metrics

- **Diff:** `src/types/context.rs` +575 region (prod fix + doc-comments + 5 inversions + 7 new cells); `src/agent_loop/run.rs` +143 (name-set + truthful-list cell); `revert.rs` +38, `tool_help.rs` +31 (tool-doc); `concept-brake.md` +3/−1; ADR-0002 +3; ADR-0003 NEW. Total src+docs: +591/−202.
- **Test delta:** lib 244→251 (+7); `collapse_abandon` 30→36 (+6, the 7th NEW cell is the truthful-list test in `apply_revert_tests`); 5 KC-02 keep-whole tests inverted in place (net count 0); 2 superseded-literal tests updated.
- **Disk reclaimed (cargo-clean):** gate-2 ~5 GiB + gate-4 ~10 GiB; ~240 GB free maintained; 0 disk-pressure incidents.

## §8 — Remaining close steps (orchestrator-run)

1. **Seal commit** (implementer left the tree uncommitted) — subject prepends `D-TEST-0076:`.
2. **Live DISPOSITION-asserting close-gate** (per `[[feedback_render_transcript_close_gate]]`; the explicit KC-02 lesson): reuse the KC-02 close-gate runner + fixtures, both drivers (deepseek + minimax) × completion (pinned) + failure (abandon), transcript-read, asserting **the disposition** — tail shrunk (R1), orphan call removed (R3), append-order original-FIRST (R2a), abandon replaces (R2), truthful single-label, no-400. NOT merely no-400.
3. **#81 closure** + a corrected-note on #80, citing the contract matrix + the live transcript result.
4. **Cycle-index transition** → audited-pending-retro at close.
5. **Pause before chunk-retrospector** per `[[feedback_pause_before_retrospector]]` (KC-01 + KC-02 + KC-03 joint-retro candidate).
