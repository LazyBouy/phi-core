<!-- Last verified: 2026-06-09 by Claude Code (KC-03 #81/D-TEST-0076 — phi-core ADR-0003; the simple revert tail-shrink contract R1/R2/R2a/R3; SUPERSEDES ADR-0002 §D2.1; render-polish follow-up: §D3.2 single-label example uses the `revert-`-prefixed `rendered_label()` form) -->

# phi-core ADR-0003 — The simple revert tail-shrink contract (R1/R2/R2a/R3): the pinned arm converges to the abandon arm

**Status: Accepted**

> Third phi-core ADR. Closes GitHub #81 / D-TEST-0076 (S2 core-tool correctness: the pinned-revert disposition shrinks the post-target tail) and SUBSUMES #80 / D-TEST-0075 (KC-02's (B) "keep-whole-cluster-on-result-node" was a misread of the contract). Cite sub-decisions as `ADR-0003 §D3.<M>`. **SUPERSEDES ADR-0002 §D2.1** (the result-node keep-whole re-append); ADR-0002 §D2.2 / §D2.3 / §D2.4 carry forward unchanged. Render-pass-only; no node-model / public-API / persisted-field / migration / tool-arg-schema change (tool DESCRIPTION text only). Investigation-first: the Phase-0.5 P0 investigation reproduced the verbatim production render pipeline and settled all 9 gating facts (F-1..F-9) before this plan existed.

## Forks

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F1 | tail-shrink locus + the simple contract (**product-semantic** — the on-wire disposition is user-perceivable) | **F1.render-pass-shrink — the pinned arm converges to the abandon arm; R1/R2/R2a/R3** (investigation-forced; the user's locked simple contract #81; SUPERSEDES the KC-02 (B) keep-whole) | P0 F-2/F-3: the trunk walk (`build_trunk_context`) ALREADY shrinks the tail; the render pass RE-INTRODUCES it via `retain_pinned_calls_on_node` clause (iii) — the KC-02 (B) misread. The fix is to STOP the re-introduction (shallower than the #81 "more fundamental change" hypothesis feared — REFUTED by repro). The abandon-class arm is the proven-correct template (F-8); the pinned arm converges to it, differing only by Rule 2 (ADD vs REPLACE). |
| F2 | node-model adequacy (**TECHNICAL** — no user-visible delta) | **F2.linear-chain-suffices** (investigation-forced) | P0 F-2 / §5.C: the existing linear-stamped `(node_id, tool_call_id)` join already excludes the post-target tail AND removes the orphan call by id; a true-sibling node model is a large kernel change (node stamping, persisted parent links, migration) for zero KC-03 benefit. |
| F3 | append-order + label mechanics (**TECHNICAL** — these ARE the F1 render nits) | **F3.append-after-original + single-label + truthful-list** (investigation-forced) | P0 F-6/F-7: the breadcrumb was PREPENDED (sandwiching the body) with a double-label (`[outcome: reverted past: …]`) and named a KEPT tool. The `[n<id>]` marker stays leading (the model relies on it for a valid `step=`, the `[n<id>]`-echo tests); the `[kind: text]` annotation moves AFTER content; collapse the double-label; exclude the kept target's own calls from the abandoned-list. |
| F4 | KC-01/KC-02 carry-forward (**TECHNICAL** — test disposition) | **F4.invert-the-KC02-keep-whole-tests + keep-the-stay-green-set + backstop-stays** (investigation-forced) | The keep-whole assertions encode the wrong spec; they invert to the shrink dispositions. The abandon-class arm + the on-trunk gate + the M=1 path + `enforce_call_atomic_backstop` STAY (P0 F-4/F-8/F-9). `retain_pinned_calls_on_node` is reworked (clause iii removed), not deleted. |
| F5 | KC-04 split decision (**TECHNICAL** — absorb-vs-split routing) | **F5.absorb — one coherent Large chunk** (investigation-forced) | P0 §6/F5 + §5.C: the only separable large piece is the true-sibling node model, which F2 rules out as unnecessary. No other separable large piece surfaced; splitting would manufacture scope. |

## Context

GitHub #81 / D-TEST-0076 is an S2 core-tool correctness defect: a pinned (`completion` / `step-summary`) revert onto a node X does NOT shrink the post-target tail — it re-gathers it. The contract #81 names (R1/R2/R2a/R3 + 2 render nits):

- **Rule 1** — revert to X SHRINKS every node strictly after X (always, any category).
- **Rule 2** — abandon-class (`failure`/`tangent`) REPLACES X's content with the summary; pinned-class (`completion`/`step-summary`) ADDS the summary to X (X content KEPT).
- **Rule 2a** — pinned append order is `original content → revert summary → [continue_after_revert]`.
- **Rule 3** — a removed tool_result's paired tool_call is surgically removed by id from its kept call node.
- **Nits** — a SINGLE `[kind: …]` label (not `[outcome: reverted past: …]`); the abandoned-list names ONLY the shrunk tools.

The P0 investigation reproduced 9 DEFINITIVE findings through the verbatim production render pipeline:

- **F-2 (load-bearing)**: `build_trunk_context` from `active=n2` ALREADY shrinks the tail (raw trunk = `[n0, n1, n2]`; n3 absent). Rule 1 is satisfied at the trunk-walk layer.
- **F-3**: the FULL pipeline RE-INTRODUCES the shrunk tail on the pinned path (`result_b(TAIL should be GONE)=true`). The re-introducer is step 2, `retain_pinned_calls_on_node` clause (iii).
- **F-4**: that clause re-appends the off-trunk sibling result by call-node membership (`context.rs:842–852, 866–868`) — the KC-02 (B) keep-whole heuristic, the misread #81 corrects.
- **F-8**: the abandon-class arm is CORRECT for the same target and does NOT leak the tail — it is the proven shrink template the pinned arm must converge toward.
- **F-9**: `enforce_call_atomic_backstop` could not have caught the defect (clause (iii) re-appended the result, so the call was not an orphan from the backstop's view) — which is exactly why KC-02's no-400 close-gate falsely read PASS (it validated internal pairing, not the disposition).

KC-02's (B) "keep-whole-cluster-on-result-node" (ADR-0002 §D2.1) was the wrong product semantic: it KEPT the gathered tail on into-cluster reverts, defeating the context-budget tool the revert is meant to be.

## Sub-decisions

### §D3.1 — F1 the simple tail-shrink contract; the pinned arm converges to the abandon arm (resolves F1; SUPERSEDES ADR-0002 §D2.1)

**Pre-existing-behaviour:** ADR-0002 §D2.1 (KC-02 (B)) re-appended the off-trunk parallel sibling on a pinned-onto-result revert (`retain_pinned_calls_on_node` clause (iii), former `:839-852`), KEEPING the whole gathered cluster — the misread. KC-03 implements the simple contract:

- **Rule 1** — a revert onto X SHRINKS every node strictly after X (always, any category). The trunk walk already drops the tail (P0 F-2); the pinned arm no longer re-introduces it. In `retain_pinned_calls_on_node`, for each call on the pinned node whose result is OFF-trunk (strictly after X), the call is dropped by id and its result is NEVER re-appended.
- **Rule 2** — abandon-class REPLACES X's content with the summary (the abandon arm clears the body); pinned-class ADDS the summary to X (X content KEPT).
- The pinned arm CONVERGES to the abandon-class arm's already-correct shrink (P0 F-8), differing ONLY by Rule 2 (ADD vs REPLACE).
- **The on-trunk-keep gate (clause i) is PRESERVED** as Rule 1's don't-over-shrink guard: a call whose result is already live on the trunk (the model continued PAST this cluster, so it is NOT strictly after X) is KEPT. Without this gate, a mid-trunk pinned cluster the model continued past would be wrongly collapsed (`matrix_on_trunk_pinned_cluster_kept_always` / `pinned_cluster_mid_trunk_kept_verbatim`).

**Explicitly:** ADR-0002 §D2.1's result-node keep-whole re-append no longer holds. This is **intended** (#81's simple contract), not a regression — it corrects the KC-02 (B) keep-whole heuristic that #81 names as the misread. The 3 KC-02 keep-whole tests + 2 edge cells that encoded the re-append assertion are inverted to the shrink dispositions; the on-trunk gate + the abandon-class arm + the M=1 reclaim path stay green unchanged.

**`tag_on_call_node` parameter removed (tidy-up):** with clause (iii) gone, both off-trunk branches (revert-onto-call-node and revert-into-cluster) do the SAME thing — drop the orphan call by id — so the `tag_on_call_node: bool` parameter that KC-02 threaded to distinguish them is now dead. It is removed; `retain_pinned_calls_on_node` becomes an associated fn (it no longer reads `self.messages`, since the re-append that read the forensic log is gone). The render is forensic-read-only by construction now: the function neither reads nor mutates `self.messages`.

**Pre-existing-behaviour preservation note (variation (b) — KC-03 scope-narrowing carve-out):** this sub-decision SUPERSEDES the KC-02 §D2.1 re-append behaviour rather than preserving it — by design, since §D2.1's keep-whole IS the misread #81 corrects. The preserved invariants are the on-trunk-keep gate (clause i, Rule 1's guard) + the call-node reclaim drop (clause ii, which already shrinks) + the M=1 byte-identical-where-applicable path + the no-400 backstop. The render-pass-only blast radius and the immutable forensic log are unchanged.

### §D3.2 — F3 append-after-original + single-label + truthful-list (resolves F3)

**Pre-existing-behaviour:** the breadcrumb was PREPENDED (`weave_braking_annotations` `prepend_marker_to_message`, sandwiching the body) with a double-label (`[outcome: reverted past: …]`, the `rendered_label()` kind × the `compose_revert_breadcrumb` `reverted past:` prefix), and the abandoned-list named the kept target's own calls (P0 F-6/F-7). KC-03:

- **Append-after-original (Rule 2a)** — the `[n<id>]` marker stays LEADING (it is a node label the model echoes into `revert_to_state(step=…)`, guarded by the `[n<id>]`-echo tests); the `[<kind>: <summary>]` annotation appends AFTER the node's original content via a new `append_annotation_to_message` variant of `prepend_marker_to_message`. A kept pinned node renders `[n<id>] <original body> [<kind>: …]`.
- **Single-label** — the `reverted past: ` prefix is stripped from the breadcrumb text at weave time, so the rendered annotation is `[revert-outcome: <summary>]`, not `[revert-outcome: reverted past: <summary>]`. The kind label is the `revert-`-prefixed `rendered_label()` form (e.g. `revert-outcome` / `revert-lesson`) so the breadcrumb signals it came from `revert_to_state`. The tag's stored `text` is unchanged (the strip is render-only); the steering pointer (`inject_continue_after_revert`) is unaffected (it is always last, order-correct relative to Rule 2a).
- **Truthful-list** — `apply_revert`'s `abandoned_tool_names` seeds the TARGET cluster's own call name(s) FIRST then the strictly-after span. For a PINNED revert ONTO a RESULT node, the target cluster's content is KEPT (Rule 2 ADD), so naming the kept target's own call would mislabel still-visible work as "abandoned" (#81 obs #4). KC-03 EXCLUDES step (i) for that case (`pinned_onto_result = !is_decayable && target_is_result_node`), seeding ONLY from the strictly-after span (the span the contract actually shrinks). The abandon-class arm REPLACES the target content, so its own calls ARE abandoned → step (i) is kept for abandon-class. The active-pointer (`run.rs:781`) + reject (`run.rs:752-763`) rules are UNCHANGED.

### §D3.3 — F2 linear-chain-suffices + F5 absorb, no node-model change (resolves F2 + F5)

*Net-new confirmation — no prior behaviour to preserve (the node model is unchanged):* the contract is fully expressible on the existing linear-stamped chain (`result_a.parent = call_node`, `result_b.parent = result_a`, …). The trunk walk already excludes the post-target tail (P0 F-2 — the off-trunk grandchildren are absent from the raw trunk), and the orphan call is removable by the existing `(node_id, tool_call_id)` join (P0 §5.C). No `build_trunk_context` / sibling-stamping change; no KC-04 split. The true-sibling node model is a deferrable Revisit trigger (it would only let the orphan-call removal key on the direct-child gate instead of the id-membership test), UNNECESSARY for #81.

### §D3.4 — the no-400 invariant is preserved by the backstop (carry-forward of ADR-0002 §D2.4)

*Net-new confirmation — no prior behaviour changed:* `enforce_call_atomic_backstop` (`streaming.rs:130`) STAYS as the locus-independent belt. After the pinned arm shrinks the tail + drops the orphan call by id (Rule 3), the backstop guarantees no orphan result / no dangling call survives → no provider 400. P0 F-8/F-9: the abandon arm already satisfies this; the converged pinned arm inherits it. The NEW contract cells assert `!trunk_has_dangling_call` AND `!trunk_has_orphan_result` across the exercised cells.

## Cross-references

- **(a) concept-doc**: `docs/concepts/concept-brake.md` (verified-header + `:193` co-keep note rewritten to the simple tail-shrink contract); `src/types/context.rs` `retain_pinned_calls_on_node` doc-comment + the in-arm pinned-cluster comment (rewritten to the shrink form); `src/tools/revert.rs` `RevertTool::description()` + `src/tools/tool_help.rs` `REVERT_HELP` (rewritten to R1/R2/R2a/R3).
- **(b) closed drifts/issues**: GitHub #81 / D-TEST-0076 (primary, S2 core-tool correctness) + #80 / D-TEST-0075 (subsumed/corrected — KC-02 closed it on the wrong invariant).
- **(c) prior ADRs**: `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md` §D2.1 (**SUPERSEDED-in-part by §D3.1**) + §D2.2 / §D2.3 / §D2.4 (carried forward) + `docs/decisions/0001-parallel-revert-call-atomicity.md` §D1.5 (R4 empty-node — relied on for breadcrumb keep-as-text in the call-node reclaim path).
- **(d) forward-scope + P0**: `docs/specs/plan/forward-scope/kc-03-revert-tail-shrink-contract.md` + the P0 investigation (`docs/specs/plan/build/kc-03-revert-tail-shrink-contract-24de5309/p0-investigation.md` §3 F-1..F-9 / §4 fix-locus / §6 forks / §7 contract cells / §8 close-gate).

## Consequences

### For consumers (i-phi / baby-phi / future)

Pinned reverts now actually SHRINK the context budget (the tool's whole point): every node after X is dropped, the summary is appended cleanly after X's original content, no orphan call dangles, and a truthful single-label breadcrumb names only the shrunk tools. A weaker model is no longer confused by a sandwiched body, a double label, or a breadcrumb that names work it can still see. This hardens braking ahead of the MCP / combined-agent cluster (#67 / #64 / #68 — forward-routing note) where parallel-tool + revert co-occur heavily. No public-API / persisted-field / migration / tool-arg-schema change — backward-compatible; the only on-wire change is the corrected revert-mode render.

### For KC-02 / ADR-0002

§D2.1's result-node keep-whole is superseded; the 3 KC-02 keep-whole tests + 2 edge cells are inverted to the shrink dispositions. ADR-0002 §D2.2 / §D2.3 / §D2.4 are unchanged. KC-01 §D1.5 R4 (text-bearing node keep-as-text) is consumed unchanged for the call-node reclaim path.

## Revisit triggers

1. A future **true-sibling node model** is adopted (parallel results stamped as real siblings of the call node) → re-opens §D3.3 (the orphan-call removal could then key on the direct-child gate instead of the id-membership test).
2. A product decision that a pinned `completion` MUST preserve gathered tail work (not shrink it) → re-opens §D3.1 (would re-introduce a keep-whole class — the KC-02 (B) direction, now superseded).
3. A new revert-render path bypasses `retain_pinned_calls_on_node` → re-opens §D3.3 (the absorb-vs-split boundary; the shrink would need re-applying at the new locus).
4. The append-after-original weave breaks the `[n<id>]`-echo contract (a future weave/marker change moves or drops the leading marker) → re-opens §D3.2's marker-leading invariant.
5. The all-calls-shrunk pinned call-node stops rendering its breadcrumb (a weave / ADR-0001 §D1.5 R4 change makes it collapse to a genuinely empty assistant node) → re-opens §D3.1's reliance on R4 keep-as-text.
6. A future decay / window change interacts with the on-trunk-keep gate (clause i) → re-opens Rule 1's don't-over-shrink guarantee (an on-trunk result must never be shrunk as if it were strictly after X).

## Verification

```bash
# Build + lint (host cargo, single crate, -j 4, -Dwarnings)
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# The re-authored contract matrix + the inverted KC-02 tests + the STAY-green probes
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon

# On-trunk-keep gate + mid-trunk-verbatim (Rule 1 don't-over-shrink — must stay green)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib matrix_on_trunk_pinned_cluster_kept_always pinned_cluster_mid_trunk_kept_verbatim

# Abandon-class arm (the convergence template) + weave echo tests (marker-leading invariant)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib matrix_abandon_class weave_braking_annotations

# Full crate suite (lib + integration)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
```

Plus the orchestrator-run live DISPOSITION-asserting full-grid close-gate (per `[[feedback_render_transcript_close_gate]]`): RENDER + READ the turn-3 `*.request.json` across both drivers × `completion` (pinned) and `failure` (abandon), asserting the DISPOSITION — post-target sibling result ABSENT (Rule 1), orphan call REMOVED (Rule 3), breadcrumb APPENDED original-body-FIRST (Rule 2a), abandon REPLACES X's content (Rule 2), truthful single-label, no provider 400 — NOT merely no-400 (the KC-02 wrong-invariant lesson).
