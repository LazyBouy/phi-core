<!-- Last verified: 2026-06-09 by Claude Code -->

# KC-02 cycle-audit — parallel-cluster pinned-revert disposition (B)

**Cycle hex:** `d835a363` · **Project:** phi-core (kernel lane) · **Branch:** `dev` · **Crate:** 0.11.4
**Closes:** GitHub #80 / D-TEST-0075 (S2) · **Fork lock:** F1=F1.B USER-LOCKED (2026-06-08, USER-DIVERGENT from iter-1 (a)); F2/F3 at investigation-rec
**Envelope:** Large (3 auditors A/B/C) · **Phases:** 4 (P0/P1/P2/P-SEAL) · **Plan iterations:** 2 (iter-1 (a) → iter-2 (B) USER-DIVERGENT narrow re-author of F1 only)

---

## §1 — Verdict

**PASS — ready for seal + live close-gate.** The validated (B) mechanism (P0 §10) was implemented verbatim; all gate-4 MUST-RUN checks green by orchestrator execution; 3 independent auditors PASS (A 8/8, B 8/8, C 6/6) with zero FAIL/PARTIAL. The only remaining close steps are orchestrator-run + outside the unit lane: the live transcript-read close-gate (per `[[feedback_render_transcript_close_gate]]`) + the #80 GitHub closure.

## §2 — Gate-4 MUST-RUN (orchestrator-executed; authoritative — sub-agent claims are NOT-EXECUTED-IN-AUDIT corroboration)

| Check | Command | Result |
|---|---|---|
| fmt | `cargo fmt -- --check` | exit 0 (clean) |
| full suite | `cargo test -j 4` (host cargo, single crate) | 28 binaries `ok`, **0 failed / 0 errors** |
| lib | (within above) | **244 passed / 0 failed** (was 234 pre-P2; +10) |
| `collapse_abandon` | `cargo test --lib collapse_abandon` | **30 passed / 0 failed** (was 20; +10 within plan §8 [8,12]) |
| integration | (within full suite) | `agent_loop_test` 56 · `agent_test` 14 · `session_test` 36 — all green |
| clippy | `RUSTFLAGS="-Dwarnings" cargo clippy --all-targets` | Finished clean (0 warnings) |

phi-core has **no `scripts/check-*.sh` CI guards** (only a `scripts/pre-commit` fmt+clippy hook) — the MUST-RUN list above IS the gate (noted per kernel-lane convention). Cargo-clean discipline honored: cleaned after each test invocation (gate-2 5.1 GiB + gate-4 4.9 GiB reclaimed; auditors self-cleaned).

## §3 — Kernel-minimality surface-discipline (substitutes phi-core-leverage-check — N/A, kernel does not consume itself)

- **Blast radius:** ONE private fn `retain_pinned_calls_on_node` (`src/types/context.rs:769`) + ONE caller token (`:733`, threading the existing `node_is_call` from `:527`) + the fn doc-comments (`:743` + the pinned-branch mirror `:692`).
- **General primitive?** YES — target-aware pinned-revert reclaim/keep is a render-correctness property every consumer (i-phi / baby-phi / future MCP) needs identically. `tag_on_call_node: bool` is a general render concept, not a consumer hook.
- **Consumer leakage?** NONE — zero i-phi/baby-phi types or requirements introduced.
- **No public-API / persisted-field / migration change:** `retain_pinned_calls_on_node` is private (`fn`, not `pub fn`); render-pass only; forensic `self.messages` is `.iter().find()` + `.clone()` reads ONLY (never mutated — the only `self.messages` diff is the find-predicate rename). `build_trunk_context` (`:1074`) + the node model + `enforce_call_atomic_backstop` (`streaming.rs:130`) UNTOUCHED (empty diff). **Genuine general kernel fix** per `[[feedback_phi_core_kernel_minimal]]`.
- **k8s-readiness-check:** N/A (phi-core is a library, not a daemon).

## §4 — Orchestrator gate-2 direct verification (read every diff)

- **(B) 3-clause branch** (`context.rs:823-868`): (i) on-trunk → `continue` (load-bearing keep gate, `:831`); (ii) off-trunk + `tag_on_call_node` → drop, no re-append (`:834-838`); (iii) off-trunk + `!tag_on_call_node` → re-append by call-node membership `tool_call_id == call_id` against `self.messages` (`:842-852`), replacing the KC-01 direct-child `parent_id == this_node_id` gate. Synth-id fallback for id-less calls preserved from KC-01 (`:792-795`). **Matches P0 §10.1 validated logic verbatim.**
- **Caller** threads `node_is_call` (`:733`); single signature change, no other call-sites.
- **3 KC-01 flips** spot-read for assertion fidelity (not weakened):
  - `pinned_class_keeps_cluster_whole_no_dangling_call:2501` — INVERTED to `!has_result(call_abc)` + `!dangling` + breadcrumb (`[DROP]`+crumb).
  - `row_pinned_revert_to_call_node_now_call_atomic:3316` — `!pcall_a` + `!pcall_b` + breadcrumb (`[DROP,DROP]`+crumb).
  - `row_pinned_revert_to_first_result_now_call_atomic:3286` — `pcall_a` on-trunk kept + `pcall_b` re-appended by membership (`[PAIR,PAIR]`).
- **On-trunk-gate test** `pinned_cluster_mid_trunk_kept_verbatim` ABSENT from the diff (body untouched → stays green) + a NEW explicit probe `matrix_on_trunk_pinned_cluster_kept_always`.
- **10 NEW** `matrix_`/`edge_` grouped tests using `render_with_backstop` (verbatim pipeline + backstop) + `assert_no_400`, covering arity-2/3 × Outcome/Checkpoint, reclaim/keep-whole/on-trunk/no-400/edges.

## §5 — Paperwork ledger

| Artifact | State |
|---|---|
| plan.md (iter-2 (B)) | archived at cycle folder; verified-header 2026-06-09 |
| p0-investigation.md | moved beside plan.md (was `_p0-investigations/`) |
| ADR-0002 `0002-parallel-cluster-pinned-revert-disposition-b.md` | NEW, Accepted, 7 sections, §D2.1–§D2.4, 5 Revisit triggers, header 2026-06-09 |
| ADR-0001 §D1.2 | superseded-in-part inline note added; verified-header bumped (Audit B claim 5 PASS) |
| `concept-brake.md:192` | target-aware co-keep note + header refresh (Audit B claim 2 PASS) |
| `retain_pinned_calls_on_node` doc-comments | rewritten to (B) 3-case form (Audit B claim 1 PASS) |
| `_cycle-index.md` KC-02 row | appended (`in-flight` / `pending`) |
| audit-A/B/C-iter1.md | written, all PASS |
| #80 GitHub closure | **orchestrator-pending** (post-seal + post-close-gate) |

## §6 — Deviations

- **None of code/test/audit class.** Implementation matched the validated mechanism with zero PAUSE, zero collateral test failures, zero auditor re-spawns. Single-iteration audit cycle (A1·B1·C1, all PASS).
- **Process note (not a defect):** ADR-0001 inline-supersession note dated 2026-06-09 (today) where the implementer-dispatch sample illustratively wrote 2026-06-08 — internally consistent with the verified-header; Audit B flagged + cleared as non-defect.
- **Audit-prompt-authoring miss:** none. The Large-envelope audit prompts were authored in plan §11 by the planner and used verbatim (no orchestrator paraphrase) → zero divergence risk; gate-3 6-axis cross-check N/A (no paraphrase).

## §7 — Metrics

- **Production diff:** `src/types/context.rs` +630/−75 (incl. the 10 NEW matrix tests + 3 flip rewrites + the fn change + doc-comments); fn-region production net well under the 70-LOC pause threshold (P0-validated `+32/−13` + caller token + doc-comment).
- **Docs diff:** `concept-brake.md` +3/−1; ADR-0001 +3; ADR-0002 NEW (14.8 KB).
- **Disk reclaimed (cargo-clean):** gate-2 5.1 GiB + gate-4 4.9 GiB = ~10 GiB across the cycle; 238 GB free maintained; 0 disk-pressure incidents.
- **Test delta:** lib 234→244 (+10); `collapse_abandon` 20→30 (+10); integration unchanged (106 green).

## §8 — Remaining close steps (orchestrator-run, outside the unit lane)

1. **Seal commit** (P-SEAL; implementer left the tree uncommitted for review) — subject prepends `D-TEST-0075:`.
2. **Live close-gate** (per `[[feedback_render_transcript_close_gate]]`): the full grid — `deepseek-chat-v3-0324` + `minimax-m2.7` × the 4 categories — transcript-read, asserting call-node reclaims + result-node keeps-whole + no 400. (Optional pre-fix confirmation is superseded by the matrix proof that no 400 exists + the silent-loss is mechanically established.)
3. **#80 closure** on GitHub citing the promoted 32-cell + 5-edge regression matrix + the live transcript result.
4. **Cycle-index transition** → `audited-pending-retro` at close.
5. **Pause before chunk-retrospector** per `[[feedback_pause_before_retrospector]]`.
