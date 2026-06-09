<!-- Last verified: 2026-06-09 by Claude Code -->

# KC-03 — `revert_to_state`: the simple tail-shrink contract (R1/R2/R2a/R3)

> Third phi-core kernel chunk. Closes GitHub **#81 / D-TEST-0076 (S2)** and **subsumes #80 / D-TEST-0075** (KC-02's (B) was a misread). **Investigation-first** (user-directed): the fix is a more fundamental change than the KC-02 render-pass patch — the post-target tail must actually be SHRUNK (the revert application / trunk walk), not annotated. P0 grounds the current revert surface + the node model + locates the true fix-locus BEFORE any plan.

## §1 — Purpose

Re-implement `revert_to_state` so its rendered disposition matches the **simple contract the user locked** (2026-06-09), replacing the KC-02 (B) "keep-whole-cluster-on-result-node" heuristic that was a misread:

- **Rule 1 — Shrink the tail.** Reverting to node X removes EVERY node strictly after X. Always, regardless of category. (Today: not shrunk — a parallel sibling result after X survives on the wire.)
- **Rule 2 — Summary placement at X.** abandon-class (`failure`/`tangent`) → summary **replaces** X's content; pinned-class (`completion`/`step-summary`) → summary **added** to X (X content kept).
- **Rule 2a — Append order (pinned-class).** The added summary is placed **AFTER** X's original content (order: original content → revert summary → `[continue_after_revert]`). (Today: the `[outcome:…]` breadcrumb is prepended, sandwiching the original body.)
- **Rule 3 — Atomicity by unique id.** When a tool_result is removed (shrunk tail, or abandon-class replace at X), its paired tool_call is **surgically removed by id** from its (kept) call node, so no call/result hangs as an orphan.

Plus the two render nits: collapse the redundant `[outcome: reverted past: …]` double-label to ONE form; make the "abandoned" list name only what was actually shrunk (not a kept tool).

## §2 — Inputs consumed

- **GitHub #81 / D-TEST-0076** — the authoritative contract (R1/R2/R2a/R3 + 4 observations + 2 worked examples). The S2 rationale: core revert correctness; "must work perfectly".
- **KC-02 (cycle `d835a363`)** — ADR-0002 (the (B) disposition being SUPERSEDED), the unit matrix that encodes the wrong spec (`matrix_pinned_into_cluster_keeps_whole_cluster` etc.), `close-gate-result.md` (verdict RETRACTED), the live wire evidence (`KC02-RECLAIM` turn-3 request.json: `n2` survives + breadcrumb prepended).
- **KC-01 (cycle `a79c7669`)** — ADR-0001 (call-atomic render; the `(node_id, tool_call_id)` composite join + the `enforce_call_atomic_backstop`), `p0-investigation.md` (F-1 linear stamping n0→n1→n2; F-8 Mock id collision).
- **Render code** (current `dev`): `src/types/context.rs` `apply_revert` (marks intent), `build_trunk_context` (the parent-walk / trunk derivation — the likely tail-shrink locus), `collapse_abandon_class_cluster`, `retain_pinned_calls_on_node` (the KC-02 (B) fn to be reworked/removed), `weave_braking_annotations` (the breadcrumb composer + order), `inject_continue_after_revert`; `src/agent_loop/streaming.rs` `enforce_call_atomic_backstop`. `src/types/node_tag.rs`, `content.rs` (the id join).
- **Live harness**: the KC-02 close-gate (`i-phi/docs/e2e-test/cycles/kc02-close-gate-d835a363/scripts/`) — reuse the parameterized `IPHI_MODEL_OVERRIDE` runner + the reclaim/keepwhole fixtures, extended to assert the DISPOSITION (tail shrunk, orphan call removed, append order, truthful breadcrumb), not merely no-400.
- **Memories**: `[[feedback_render_transcript_close_gate]]` (read the transcript — and verify the DISPOSITION against the contract, not just no-crash), `[[feedback_phi_core_kernel_minimal]]`, `[[feedback_never_hedge]]`, `[[feedback_htc_cohort_strongest_open_source]]`.

## §3 — Issues / drifts closed

- **#81 / D-TEST-0076 (S2)** — primary. The full R1/R2/R2a/R3 contract + the 2 render nits.
- **Subsumes #80 / D-TEST-0075** — KC-02 closed it on the wrong invariant; KC-03 is the correct closure of the underlying semantics.
- Any NEW defect the investigation surfaces is filed as its own D-TEST drift (GitHub-mirrored).

## §4 — Prerequisites

- KC-01 (call-atomic foundation) + KC-02 (the (B) code to rework) — both landed on `dev` (`615b221`). ✓
- `chunk-archive-plan` v5 (phi-core-aware) — landed. ✓
- None blocking.

## §5 — Deliverables

1. **Re-implemented `revert_to_state` disposition** to the R1/R2/R2a/R3 contract. Fix-locus decided by P0 (likely the revert application / trunk derivation — the tail must be actually removed — plus the breadcrumb composer for append-order + the truthful-abandoned-list; the KC-02 (B) `retain_pinned_calls_on_node` re-append is reworked or removed).
2. **Re-authored unit matrix** — delete/invert the KC-02 keep-whole assertions; assert the contract: tail shrunk after X, orphan calls surgically removed by id, pinned-class append-order (original → summary → continue), abandon-class replace-at-X, truthful abandoned-list. Permanent regression coverage.
3. **`revert_to_state` tool-doc rewrite** — the tool description/docs describe the ACTUAL behavior (R1/R2/R2a/R3); no claim that it works where it doesn't.
4. **phi-core ADR-0003** — the tail-shrink contract decision; SUPERSEDES ADR-0002 (the (B) disposition) + records the KC-01/ADR-0001 interaction.
5. **Newly-found-failure drifts** — file each NEW issue the investigation/fix surfaces.
6. **Live close-gate (disposition-asserting)** — both drivers (deepseek + minimax) × the categories, transcript-read, asserting the DISPOSITION (tail shrunk + orphan call removed + append order + truthful breadcrumb + no 400), not merely no-400.

## §6 — Acceptance criteria

- Every unit-matrix cell GREEN against the R1/R2/R2a/R3 contract.
- The live grid (both drivers, transcript-read) shows: the post-target tail is shrunk; orphaned parallel calls are surgically removed; pinned-class summary appended after original content; abandoned-list truthful; no provider 400.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check`.
- Kernel-minimality: general revert-correctness only; zero consumer leakage; no public-API / persisted-field / migration change (target — confirm in P0; the revert tool's arg schema may need a doc-only change).

## §7 — Forks for the planner (locked only after P0)

- **F1 — Tail-shrink locus**: (a) shrink in the revert application (mutate/derive the trunk so the tail is genuinely removed before render) vs (b) shrink in the render pass (filter the tail at `build_trunk_context`/collapse). THE load-bearing fork — P0 must establish whether the trunk derivation already truncates at the revert target (and the bug is elsewhere) or whether truncation is missing entirely.
- **F2 — Node-model adequacy**: does the simple contract work on the existing linear-stamped chain (n0→n1→n2), or does it require a true-sibling node model (the deferred KC-02 Revisit trigger)? P0 decides; lean (a) linear chain suffices (shrink = truncate the chain after X; surgical call-removal handles the orphan).
- **F3 — Append-order + label mechanics**: where the breadcrumb is composed (`weave_braking_annotations`) and how to (i) append-after-original for pinned-class, (ii) collapse the double-label, (iii) make the abandoned-list truthful.
- **F4 — KC-01/KC-02 carry-forward**: how many KC-01 + KC-02 tests flip; whether `retain_pinned_calls_on_node` is removed or reworked; whether `enforce_call_atomic_backstop` stays as the belt.
- **F5 — split**: if P0 surfaces a separable large piece (e.g. a node-model change), route to KC-04 vs absorb.

## §8 — Audit envelope hint

**Large (3 auditors: A code+matrix / B docs+ADR+tool-doc / C carry-forward-regression + the live disposition-asserting close-gate).** Touches the core revert path + supersedes two prior ADRs + reworks KC-02 code. Confirm at gate-1.

## §9 — Unblocks

- Makes `revert_to_state` — the core context-budget tool — behave to the locked contract across all categories + parallel clusters. Load-bearing for every consumer (i-phi/baby-phi/future) and for the MCP/combined-agent cluster (#67/#64/#68) where parallel-tool + revert co-occur.

## §10 — Risks

- **Fix is more fundamental than a render patch** — if the tail-shrink must move into the revert application / trunk derivation, blast radius grows beyond the single render fn. P0 (investigation-first) is the mitigation: locate the true locus + rule out non-viable approaches with evidence BEFORE planning.
- **Hot-path regression** — touches the working-context render; KC-01 atomicity + the M=1 path + the abandon-class cells are the regression guards.
- **Wrong-invariant trap (the KC-02 lesson)** — the close-gate MUST assert the disposition against the contract, not no-400. Codified in §6 + the close-gate deliverable.

## §11 — Direct-approval criteria fit

Does **NOT** auto-approve (`approval=yes` forced): S2 core-tool correctness; the load-bearing fork (F1 tail-shrink locus) is locked only after P0; supersedes two ADRs; likely Large envelope. User lock at gate-1 required.
