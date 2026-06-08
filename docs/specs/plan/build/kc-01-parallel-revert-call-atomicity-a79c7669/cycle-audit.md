<!-- Last verified: 2026-06-08 by Claude Code -->

# KC-01 — cycle-audit (gate-4 final re-audit)

> phi-core kernel chunk · cycle `a79c7669` · closes GitHub #77 / D-TEST-0072 (S2 parallel-revert call-atomicity). First phi-core cycle (KC namespace).

## §0 — Critical findings

| Check | Result |
|---|---|
| Unit gate (clippy + full suite + fmt) | **GREEN** |
| Both #77 BROKEN matrix rows flip to call-atomic (`dangling=[]`) | **YES** (`row_pinned_revert_to_first_result_now_call_atomic`, `row_pinned_revert_to_call_node_now_call_atomic`) |
| M=1 / 3 SAFE rows regression | **no regression** (13 existing + 3 SAFE rows green) |
| Live transcript close-gate (parallel-tool + pinned revert, no provider 400) | **PASS** (HTC `KC01-CLOSEGATE`, deepseek; pinned revert onto the F-5 cluster → post-revert request accepted, no 400, clean `AgentEnd`; transcript read — see `close-gate-result.md`) |
| Overall (code) | **structural-pass + unit-pass + live-pass**; #77 / D-TEST-0072 **closed**. Adjacent semantic finding filed #80 / D-TEST-0075 (S3; does not reopen #77) |

## §1 — Audit-pipeline summary

| Auditor | Iter | Focus | Verdict | Claims |
|---|---|---|---|---|
| A | 1 | code-correctness + tests | **PASS** | 8/8 PASS, 0 FAIL |
| B | 1 | docs + ADR + paperwork | **PASS** | 8/8 PASS, 0 FAIL |

Clean iter-1 on both lanes; no re-spawn. Orchestrator spot-checked the load-bearing claims by personally reading the production logic (`retain_pinned_calls_on_node`, `call_node_has_live_sibling`, the gated `content.clear()`, `enforce_call_atomic_backstop`) + tracing both BROKEN rows by hand at gate-2, and re-verified the ADR-0001 structure at gate-4.

## §2 — Diff review

Production (render-pass only; forensic `messages` log never mutated):
- `src/types/context.rs` — `collapse_abandon_class_cluster` rewritten to **call-atomic**: per-call retain (`retain_pinned_calls_on_node`, F1.b `(node_id, tool_call_id)` composite join via `parent_id == this_node_id`, synthetic `synth-{node_id}-{idx}` fallback); R2 per-call collapse (the whole-node `content.clear()` at both the call-node + result-node branches is now gated on `call_node_has_live_sibling` → R3 pin-wins keeps a mixed-class node whole, removing only the collapsing call); atomic-cluster doc-comment rewritten node-atomic → call-atomic. +190 production / +315 inline-test (net).
- `src/agent_loop/streaming.rs` — `enforce_call_atomic_backstop` (R4 empty-node + F3 declarative orphan-filter) as the final pass on the revert-mode pipeline; `pub(crate)`. +~50 logic.
- `src/agent_loop/mod.rs` — `pub(crate)` re-export (+5).

Docs:
- `docs/concepts/concept-brake.md:192` co-keep note → call-atomic; verified-header refreshed.
- `docs/decisions/0001-parallel-revert-call-atomicity.md` — **NEW** (phi-core ADR-0001; the `docs/decisions/` home is net-new). 7 sections, §D1.1–§D1.7, 4 Revisit triggers, Accepted.

No public API signature change, no new persisted field, no migration (P0 F-7). Change confined to 2 render functions in 2 files + a re-export.

## §3 — MUST-RUN evidence (authoritative, orchestrator-run)

| Command | Result |
|---|---|
| `RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --manifest-path … --all-targets` | **exit 0, clean** |
| `cargo test -j 4 --manifest-path …` (full crate) | **GREEN** — lib **234 passed / 0 failed**; all integration binaries + doc-tests pass; 0 failures anywhere |
| `cargo test … --lib collapse_abandon` | **20 passed / 0 failed** (was 13; +7 NEW) |
| `cargo fmt --manifest-path … -- --check` | **clean** (after the pre-existing-drift hygiene fix `2dc2c40`) |
| CI guards | **N/A** — phi-core ships no `check-*.sh` (only `scripts/pre-commit`); the MUST-RUN list IS the gate |

## §4 — Iteration accounting

- Trivial-1L: 0 · Trivial-multi: 0 · Tactical: 0 · Architectural: 0.
- Clean iter-1: both auditors PASS first pass; zero orchestrator patches required. Skip-condition fired at gate-1.5 (all 3 forks planner-rec + §1 populated → no iter-2 plan re-spawn).

## §5 — Paperwork ledger

| Artifact | Status |
|---|---|
| Forward-scope (`forward-scope/kc-01-…md`) | ✅ committed `89b6089` (prerequisite) |
| Plan archive (`…-a79c7669/plan.md`) | ✅ committed `008193f` |
| P0 investigation (`…/p0-investigation.md`) | ✅ committed `008193f` (moved from `_p0-investigations/` at archive) |
| Cycle-index (`_cycle-index.md`, first phi-core row) | ✅ minted `008193f` (`Iterations`/`Status` updated at this gate) |
| ADR-0001 | ✅ in seal (7 sections, §D1.1–§D1.7) |
| concept-brake.md amendment + verified-header | ✅ in seal |
| `context.rs` atomic-cluster doc-comment (node→call atomic) | ✅ in seal |
| #77 GitHub closure comment | ⏳ deferred to post-live-close-gate (correct) |

## §6 — Deviations

| # | Class | Note |
|---|---|---|
| D-1 | Pre-existing-drift hygiene | 3 `dev` fmt drifts (CC-22-era `basic_agent.rs`/`agent_test.rs`, unrelated to KC-01) surfaced by gate-2 `fmt --check`; fixed as a **separate scoped commit `2dc2c40`** (not in the KC-01 seal) to keep the seal clean + the crate gate green. Semantically null. |
| D-2 | LOC Band-2 (no pause) | `streaming.rs` +71 vs cap 60 (1.18×, < 90 pause) — backstop doc-comment + invariant logic; `context.rs` inline-test +315 vs cap 240 (1.31×, < 360 pause) — NEW parallel fixture + thoroughly-documented tests. Both within pause thresholds; logged per chunk-implementer v15 P-impl-2. |
| D-3 | Archive done manually | `chunk-archive-plan` skill is baby-phi/i-phi-aware only (not phi-core); orchestrator minted the cycle folder + `_cycle-index.md` by hand (phi-core uses baby-phi's exact `docs/specs/plan/build/` shape). v4 hard-assertion satisfied by direct read of §1 (heading + 3 subsections + 3-sentence bodies). |
| D-4 | Live close-gate post-seal | The §10 live transcript-read close-gate ran AFTER the phi-core seal (so i-phi built against the landed fix). **PASS** — #77 closed. |
| D-5 | Transcript-surfaced follow-up | Reading the close-gate transcript surfaced that `completion` onto a parallel call-node keeps the first result + drops siblings (atomic but lossy). Filed **#80 / D-TEST-0075** (S3 semantic; KC-02 candidate). Does NOT reopen #77 (atomicity holds + validated). |

No phi-core-specific skill/agent gaps beyond D-3 (the lane shipped 2026-06-08; first exercise).

## §7 — Metrics

- Test delta: **+7** (`collapse_abandon` 13 → 20; full lib 234, 0 failed).
- Production LOC: ~+245 net (context.rs ~190 + streaming.rs ~50 + mod.rs 5); inline-test ~+315.
- phi-core import baseline: **N/A** (kernel does not consume itself; kernel-minimality surface-discipline applied per `[[feedback_phi_core_kernel_minimal]]` — zero consumer leakage, no public API change).
- Disk reclaimed (placement-1/gate-5 cargo-clean): **5.6 GiB** (target 4.9 GB → 0; `/root` 36% used, 243 G free).
- Plan iterations: 1 (skip-condition). Audit iterations: 1 (both PASS).
