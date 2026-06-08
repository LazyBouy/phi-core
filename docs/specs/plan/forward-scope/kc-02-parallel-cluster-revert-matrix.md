<!-- Last verified: 2026-06-08 by Claude Code -->

# KC-02 — parallel-cluster revert keep/drop matrix: characterize + fix

> Second phi-core kernel chunk. Closes GitHub #80 / D-TEST-0075 (S2). **Characterize-first** per #80 §4: exhaustively map the per-call keep/drop/balance disposition across the full `category × revert-target × cluster-arity` grid (the hunch is the grid hides more failures, possibly hard 400s), THEN lock + ship the fix grounded on what's found. `revert_to_state` is the core context-budget tool; per-call correctness underpins all four braking categories.

## §1 — Purpose

Make the parallel-cluster revert render **correct and symmetric across all four categories** (`failure`/`tangent`/`completion`/`step-summary`) and all revert targets (onto the call-node, onto any result node, onto a post-cluster node), not just the single `completion`-onto-call-node path KC-01's close-gate happened to hit. Eliminate the silent mid-turn loss of gathered work (dropped sibling results), the keep-first/drop-rest asymmetry (a linear-stamping artifact), and any hard dangling/orphan→400 the grid surfaces.

## §2 — Inputs consumed

- **GitHub #80 / D-TEST-0075** — the §3 three-option table (a reclaim-to-breadcrumb / b pin-whole-cluster / c current), the §4 REQUIRED test matrix (2 drivers × 4 categories × 4 targets = 32 live cells + the unit equivalents), the S2 rationale (silent loss / non-re-fetchable results / unproven grid).
- **KC-01 (cycle `a79c7669`)** — ADR-0001 (R1–R4 + the `(node_id, tool_call_id)` composite join), `p0-investigation.md` (§2 surface map, F-1 linear stamping, F-8 Mock id collision), `close-gate-result.md` (the live evidence + the #80 finding).
- **Render code**: `phi-core/src/types/context.rs` `collapse_abandon_class_cluster` (`:440`) — the abandon-class branches (`:560-650`) + `retain_pinned_calls_on_node` (`:755`) + `call_node_has_live_sibling` (`:839`); `agent_loop/streaming.rs` `enforce_call_atomic_backstop` (`:130`).
- **Live harness base**: the KC-01 close-gate (`i-phi/docs/e2e-test/cycles/kc01-close-gate-a79c7669/scripts/`) — extend to a parameterized category × target sweep; `minimax/minimax-m2.7` braking precedent (HTC-0002, cc16–cc19).
- **Memories**: `[[feedback_render_transcript_close_gate]]`, `[[feedback_htc_cohort_strongest_open_source]]` (two-driver requirement), `[[feedback_phi_core_kernel_minimal]]`, `[[feedback_never_hedge]]`.

## §3 — Issues / drifts closed

- **#80 / D-TEST-0075 (S2)** — primary. **Any NEW hard failure the characterization surfaces** is filed as its own drift; if a found issue is separable + large, it may route to KC-03 (split decision at gate-1).

## §4 — Prerequisites

- **KC-01** (the per-call retain foundation + composite join) — landed (`46f7854`). ✓
- **`chunk-archive-plan` v5** (phi-core-aware) — landed (`6b87f10`); KC-02 archives via the skill. ✓
- None blocking.

## §5 — Deliverables

1. **Exhaustive deterministic UNIT MATRIX** (the characterization workhorse; in the P0 investigation): MockProvider drives a parallel-N-call assistant node, then a programmatic revert across **every `(category ∈ {failure,tangent,completion,step-summary}) × (target ∈ {call-node, first-result, mid-result, last-result, post-cluster})`** cell at arity ≥ 2 (and arity 3 for the asymmetry). Assert per cell: (a) **no dangling call / no orphaned result** (balance); (b) **sibling symmetry** (all N siblings same disposition); (c) **no silent loss** (a needed result is reclaimed-into-summary OR kept, never dropped-without-trace); (d) **category fidelity** (abandon → cluster reclaimed to a decaying breadcrumb; pinned → the chosen semantic with a durable marker). Mock's `mock-tool-{i}` collision (F-8) stresses the composite key.
2. **The FIX** — decided by the matrix findings. Planner-lean: **option (a) reclaim-to-breadcrumb** for pinned-onto-call-node (matches the tool's "reclaim the abandoned tail" wording + symmetric); plus whatever the grid reveals as broken in other cells. Render-pass only; forensic log never mutated.
3. **Newly-found-failure drifts** — file each NEW grid failure (esp. any hard 400) as its own D-TEST drift (GitHub-mirrored).
4. **Promoted regression tests** — the unit matrix lands in-tree (`collapse_abandon_class_cluster_tests`) as permanent regression coverage for every cell.
5. **Concept/ADR docs** — phi-core ADR-0002 (the category × target disposition decision); update the `collapse_abandon_class_cluster` doc-comment + `concept-brake.md` to state the full matrix semantics.
6. **Live close-gate (FULL grid)** — post-fix: the parameterized harness sweep, **both drivers (`deepseek-chat-v3-0324` + `minimax-m2.7`) × the 4 categories**, transcript-read, asserting no 400 + correct disposition end-to-end. (A small pre-fix live subset in P0 confirms ≥ 1 real provider 400 exists today on a worst-case cell.)

## §6 — Acceptance criteria

- Every unit-matrix cell GREEN (balance + symmetry + no-loss + category-fidelity).
- The full live grid (2 drivers × 4 categories, transcript-read) sends without any provider 400 and shows correct keep/drop disposition.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check`.
- KC-01's atomicity (the 13 existing + KC-01's 7 tests) preserved; M=1 path byte-identical.
- Kernel-minimality: general braking-correctness only; zero consumer leakage; no public API / persisted-field / migration change (target — confirm in P0).

## §7 — Forks for the planner (decided by the characterization findings)

- **F1 — the pinned-onto-call-node semantic**: (a) reclaim-to-breadcrumb vs (b) pin-whole-cluster. THE load-bearing fork; lean (a). **Locked only after the P0 matrix shows the full consequence of each across all targets.**
- **F2 — abandon-class onto-call-node / result-node consistency**: confirm/adjust so failure+tangent reclaim the whole parallel cluster uniformly (the matrix may reveal a sibling-leak here too).
- **F3 — split decision**: if the characterization surfaces a separable large issue (e.g. a node-model change to stamp parallel results as true siblings), route it to KC-03 vs absorb in KC-02.

## §8 — Audit envelope hint

**Large (3 auditors: A code+matrix-tests / B docs+ADR / C cross-cutting: the live-grid harness + the category×target interaction + no-regression vs KC-01).** Heavier than KC-01: touches the core render across all four categories + a live grid. Confirm at gate-1.

## §9 — Unblocks

- Hardens `revert_to_state` across **all** categories for parallel tool use — removes the latent S2 for any consumer (i-phi/baby-phi/future) firing parallel tools while braking.
- De-risks the MCP / combined-agent cluster (#67/#64/#68), where parallel tool use + revert co-occur heavily.

## §10 — Risks

- **The hunch realizes**: the matrix surfaces MORE failures (possibly hard 400s in untested cells) → scope expansion / a KC-03 split. The characterize-first structure is the mitigation (find before fix).
- **Hot-path regression**: the fix touches the core working-context render; M=1 + KC-01's parallel-atomic cases are the regression guard.
- **Live-grid nondeterminism**: models may not hit every target spontaneously; the unit matrix is the exhaustive net, the live grid is opportunistic acceptance.

## §11 — Direct-approval criteria fit

Does **NOT** auto-approve (`approval=yes` forced): S2 core-tool correctness; the load-bearing fork (F1) is locked only after the characterization; likely Large envelope; characterize-first means fix scope is genuinely unknown until P0 completes. User lock at gate-1 required.
