<!-- Last verified: 2026-06-09 by Claude Code -->
<!-- KC-03 cycle plan — ARCHIVED at cycle hex 24de5309 (2026-06-09); user-approved at gate-1.5. iter-1. project=phi-core (kernel lane). Closes GitHub #81 / D-TEST-0076 (S2); subsumes #80. GROUNDED on the orchestrator-verified P0 investigation (p0-investigation.md beside this file); the 5 forks are EVIDENCE-FORCED (P0 §3/§4/§6) — locked-at-investigation-rec. F1 product-semantic; F2-F5 TECHNICAL. -->

# KC-03 — `revert_to_state`: the simple tail-shrink contract (R1/R2/R2a/R3) — converge the pinned arm onto the abandon arm's already-correct shrink

> **Third phi-core kernel chunk.** Closes GitHub **#81 / D-TEST-0076 (S2 core-tool correctness)** and **subsumes #80 / D-TEST-0075** (KC-02's (B) "keep-whole-cluster-on-result-node" was a misread of the contract). **Investigation-first** (user-directed, completed): the P0 investigation reproduced the production render pipeline VERBATIM and settled all 9 gating facts — the fix is **render-pass-only + shallower than feared** (the trunk walk ALREADY shrinks the tail; the render pass re-introduces it). This plan FORMALISES the implementation against the 9 DEFINITIVE P0 findings (F-1..F-9) — phases, file-level deliverables, the matrix re-author + the KC-02 test-flips, ADR-0003 (SUPERSEDES ADR-0002), the tool-doc rewrite, the audit envelope, the §1 locked-fork bodies, the live disposition-asserting close-gate — and does **NOT** re-investigate or re-decide. The forks are EVIDENCE-FORCED (P0 §6); §1 presents the resolution the reproduction forces.
>
> **Project = phi-core** (NOT baby-phi, NOT i-phi). Root `/root/projects/phi/phi-core`, branch `dev`, crate 0.11.4, HEAD `9d0e46b`. HOST cargo (`/root/rust-env/cargo/bin/cargo`), single crate (no `--workspace`), cap `-j 4`. MUST-RUN = clippy `--all-targets` (`RUSTFLAGS="-Dwarnings"`) + `test` + `fmt --check`. **NO `scripts/check-*.sh` CI guards** (phi-core ships only a `scripts/pre-commit` fmt+clippy hook). **phi-core-leverage-check + k8s-readiness-check are N/A** (kernel does not consume itself / library not daemon). Verified-headers APPLY (today 2026-06-09).

---

## §1 — Locked fork details (per chunk-planner v32 P-plan-1-v32; planner-rec bodies pre-filled at iter-1)

> The 5 forks are **EVIDENCE-FORCED by reproduction** (P0 §3/§4/§6) — the user lock at gate-1 is a formality given the reproduced disproofs. **F1 is the product-semantic / load-bearing fork** (which disposition the model reads on the wire is user-perceivable) — framed with User-visible / Product-trajectory per chunk-planner v26 P-plan-1-v26. **F2–F5 are TECHNICAL** (no user-visible delta — node-model adequacy, append-mechanics, carry-forward, split). Each subsection is `*(pre-lock draft; finalizes at gate-1)*`.

#### F1 = F1.render-pass-shrink — the tail-shrink locus is the RENDER PASS; the pinned arm must CONVERGE to the abandon arm's shrink (R1/R2/R2a/R3) *(pre-lock draft; finalizes at gate-1)*

- **User-visible.** A model that fires ≥ 2 parallel tool calls in one node and then pins-reverts (`completion` / `step-summary`) onto an earlier node X now reads a **correctly-shrunk, correctly-ordered revert disposition** on the wire, turn after turn: EVERY node strictly after X is gone (Rule 1), the pinned summary is ADDED to X **after** X's original content (`original body → revert summary → [continue_after_revert]`, Rule 2/2a) rather than prepended-and-sandwiching it, the orphaned parallel tool_call left behind in X's call node is surgically removed by id (Rule 3), and the "abandoned" breadcrumb names ONLY the tools that were actually shrunk with a SINGLE label (no `outcome: reverted past:` double-label, no naming a tool that was kept). This eliminates the silent-loss / re-fetch loop #80 surfaced AND the half-kept asymmetry KC-02's (B) shipped, with no provider 400.
- **Code-level binding.** Locus is **render-pass-only** (P0 §4): (1) **rework `retain_pinned_calls_on_node`** (`context.rs:769`) — REMOVE clause (iii) (`:839-852`, the off-trunk-sibling call-node-membership re-append that KC-02 (B) added) and replace it so the pinned arm SHRINKS the post-target tail + surgically drops the orphan call by id — i.e. for off-trunk + `!tag_on_call_node` the result is DROPPED (not re-appended) and its paired call is removed from the kept node (the existing `ids_to_drop` per-call retain at `:854-862`), CONVERGING to the abandon-class arm's already-correct behavior (P0 F-8) and differing only by Rule 2 (pinned ADDS the summary to X; abandon REPLACES X's content); clause (i) on-trunk-keep STAYS (the don't-over-shrink guard, Rule 1 "no node after X ⇒ nothing shrinks"), clause (ii) call-node reclaim-drop STAYS (already shrinks). (2) **append-after-original** for pinned-kept nodes in `weave_braking_annotations` (`context.rs:929`) — the `[n<id>]` marker stays leading (it is a node label, not the summary), but the `[<kind>: <breadcrumb>]` annotation moves AFTER the original content via an append variant of `prepend_marker_to_message` (`context.rs:12`); collapse the `[outcome: reverted past: …]` double-label (the `kind` from `rendered_label()` at `:959` × the `reverted past:` prefix from `compose_revert_breadcrumb` at `run.rs:670`) to ONE label. (3) **truthful abandoned-list** in `apply_revert` (`run.rs:832-870`) — for a pinned revert onto a RESULT node, EXCLUDE the kept target cluster's own tool-call name(s) (the `push_call_names` at `:860-863` that seeds the target node's own calls FIRST) from the breadcrumb when the target node's content is KEPT (only the shrunk strictly-after span is "abandoned"). NO change to `build_trunk_context` (P0 F-2: trunk walk already shrinks), `apply_revert`'s active-pointer/reject rules (P0 F-1/F-3), the node model (P0 F-2), `enforce_call_atomic_backstop` (P0 F-8: stays as the belt), or any public-API/persisted-field/migration/tool-arg schema (P0 §4). Forensic `self.messages` stays read-only (`.iter().find()` + `.clone()` ONLY).
- **Rationale.** P0 reproduced through the verbatim production pipeline that the defect is **the render pass re-introducing a tail the trunk walk already shrank** (F-2: `raw build_trunk_context: result_b(TAIL)=false`; F-3: `FULL pipeline: result_b(TAIL should be GONE)=true`), and that the re-introducer is exactly `retain_pinned_calls_on_node`'s off-trunk-sibling re-append (F-4) — the KC-02 (B) misread (`retain_pinned_calls_on_node:839-852`). The abandon-class arm is the **proven-correct template** (F-8: `ABANDON FULL: result_b(TAIL)=false call_b_in_n1=false`) — it already shrinks the tail + surgically drops the orphan call; the pinned arm must converge to it, differing only by Rule 2 (ADD vs REPLACE the summary). This is shallower than the forward-scope §10 risk + the #81 "more fundamental change" hypothesis feared — REFUTED by repro (P0 §0/F-3); it is a genuine general revert-correctness fix in the kernel render path with zero consumer-specific logic (`[[feedback_phi_core_kernel_minimal]]` carve-out passes cleanly).
- **Product trajectory.** A correct, simple, target-uniform tail-shrink contract is the durable foundation for every consumer (i-phi / baby-phi / future) and for the MCP / combined-agent cluster (#67 / #64 / #68) where parallel-tool + revert co-occur heavily: the model's context budget actually SHRINKS on a pinned revert (the contract's whole point), the wire stays clean, and a weaker model is not confused by a sandwiched body / a double label / a breadcrumb that names work it can still see. KC-02's (B) keep-whole heuristic was the wrong product semantic (it KEPT the gathered tail on into-cluster reverts, defeating the budget tool); KC-03 corrects it to the locked simple contract.
- **Defers (if chosen).** Defers NOTHING for the #81 closure — R1/R2/R2a/R3 + the 2 render nits all ship in this chunk on the existing linear chain (F2). The deeper true-sibling node model is explicitly NOT required (P0 §5.C / F-2) and is surfaced as an ADR-0003 Revisit trigger, not a KC-04 blocker (F5). KC-02's (B) keep-whole-cluster-on-result-node disposition (ADR-0002 §D2.1) is SUPERSEDED by ADR-0003 — intended (it was the misread #81 corrects), not a deferral.

#### F2 = F2.linear-chain-suffices — no true-sibling node model; the existing `(node_id, tool_call_id)` join is sufficient *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** NO node-model change. The contract is fully expressible on the existing linear-stamped chain (`result_a.parent = call_node`, `result_b.parent = result_a`, …): the trunk walk ALREADY excludes the post-target tail (P0 F-2 — n3 absent from the raw trunk), and the orphan call is removable by the existing `(node_id, tool_call_id)` join (P0 §5.C repro: `n1 carries calls ["pcall_a","pcall_b"]` with `n3.tool_call_id == pcall_b`). The per-call `ids_to_drop` retain at `retain_pinned_calls_on_node:854-862` already does surgical removal by id — KC-03 reuses it for the off-trunk + result-node case (which KC-02 (B) re-appended instead).
- **Rationale.** P0 §5.C reproduced that a true-sibling node model is UNNECESSARY for the contract (the linear chain already shrinks + the id join already removes the orphan) and is a LARGE kernel change (node stamping, persisted parent links, migration) for zero KC-03 benefit — violating kernel-minimality. Lean (a) linear-chain-suffices is evidence-forced.
- **Defers (if chosen).** Defers the true-sibling node model as an ADR-0003 Revisit trigger (deferrable, NOT needed for #81 — it would only let the orphan-call removal key on the direct-child gate instead of the id join). Defers nothing else — the full contract closes on the linear chain in this chunk.

#### F3 = F3.append-after-original + single-label + truthful-list — the breadcrumb composer mechanics *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta beyond F1's — these ARE the F1 render nits, surfaced as a separate mechanics fork per P0 §6).

- **Code-level binding.** Three coupled mechanics in the breadcrumb composer: (i) **append-after-original** — `weave_braking_annotations` (`context.rs:929`) currently weaves the `[n<id>]` marker AND the `[<kind>: <text>]` tag in ONE prepend pass (`:946-962`); split them so the `[n<id>]` marker stays LEADING (the model relies on it for a valid `step=`, guarded by the `weave_braking_annotations_*` echo tests at `:1605+`) while the `[<kind>: <breadcrumb>]` annotation appends AFTER the node's original content for pinned-KEPT nodes (an append variant of `prepend_marker_to_message:12`); (ii) **single-label** — collapse `[outcome: reverted past: …]` to one label (the `rendered_label()` kind at `:959` × the `compose_revert_breadcrumb` `reverted past:` prefix at `run.rs:670`); (iii) **truthful-list** — `apply_revert` (`run.rs:832-870`) excludes the kept target cluster's own calls from `abandoned_tool_names` on a pinned-onto-result revert.
- **Rationale.** P0 F-6 reproduced the PREPEND (`APPEND-ORDER at result_a: "[n2] [outcome: reverted past: …] Wrote 356 bytes"` — breadcrumb BEFORE the body) violating Rule 2a; P0 F-7 reproduced the double-label + the untruthful list (`(memory_help, skill_help abandoned)` while memory_help is KEPT) matching the live wire. The `[n<id>]`-echo tests at `:1605+` are the constraint the split must honor (marker stays leading). These are render-correctness only, zero consumer logic.
- **Defers (if chosen).** Defers nothing — all three mechanics ship in this chunk as part of F1's render nits. The `[continue_after_revert]` injection (`inject_continue_after_revert:1010`) is unaffected (it is always last, order-correct relative to Rule 2a; P0 §2).

#### F4 = F4.invert-KC02-tests + keep-the-stay-green-set + backstop-stays *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — carry-forward test disposition).

- **Code-level binding.** The KC-02 keep-whole assertions INVERT (they encode the wrong spec): **`row_pinned_revert_to_first_result_now_call_atomic`** (`context.rs:3286`) — its `:3304-3307` assertion `trunk_has_tool_result("pcall_b")` ("(B) the off-trunk call_b sibling is re-appended (whole cluster kept)") INVERTS to `!trunk_has_tool_result("pcall_b")` (the tail is SHRUNK) + `!trunk_has_dangling_call` (orphan pcall_b call removed by id); **`matrix_pinned_into_cluster_keeps_whole_cluster`** (`context.rs:3737`) — the first-result a2/a3 + mid-result a3 keep-whole assertions (`:3742-3769`) INVERT to tail-shrunk-after-X; **`matrix_pinned_last_result_and_post_cluster_no_loss`** (`context.rs:3876`) — SPLIT by target position: the revert-onto-LAST-result sub-cases STAY (all results on-trunk → nothing after X → nothing shrinks), only the re-append-past-tip sub-cases invert. STAY-GREEN: **`matrix_on_trunk_pinned_cluster_kept_always`** (`context.rs:3778`, the don't-over-shrink Rule-1 guard — revert onto the last result, all on-trunk → kept verbatim), all abandon-class cells (`matrix_abandon_class_*`, F-8 — abandon arm untouched), the M=1 cells, and `enforce_call_atomic_backstop` (stays as the belt). `retain_pinned_calls_on_node` is **reworked** (clause iii removed/replaced, clauses i+ii kept), NOT deleted.
- **Rationale.** P0 §6/F4 enumerated (via grep, all currently green on `dev` HEAD `9d0e46b`) exactly which KC-02 cells encode the keep-whole misread (the re-append assertions) vs which encode the correct don't-over-shrink guard (on-trunk-keep). The abandon-class arm is the proven template (F-8), so its cells STAY green as the regression-lock for the convergence target. The backstop could NOT have caught the defect (F-9: clause iii re-appended the result so the call was not an orphan) — it stays as the locus-independent belt that, after the pinned arm shrinks + drops the orphan, guarantees no orphan survives.
- **Defers (if chosen).** Defers nothing. The 3 inverted KC-02 tests are UPDATED in place (the inversion IS the intended fix); the STAY-green set is preserved unchanged; `enforce_call_atomic_backstop` is unchanged. Per chunk-planner v19 P2, this is the canonical amend-carry-forward-test path (the keep-whole assertions are the thing being superseded — not (b) preserve-scaffold nor (c) defer/ignore).

#### F5 = F5.absorb — no KC-04 split; one coherent Large chunk *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — absorb-vs-split routing).

- **Code-level binding.** Absorb the entire fix in KC-03; NO KC-04 split. The fix is one render-pass rework (`retain_pinned_calls_on_node` + `weave_braking_annotations` append-variant + `apply_revert` name-set) + the matrix re-author + the tool-doc rewrite + ADR-0003 — a single coherent Large chunk.
- **Rationale.** P0 §6/F5 + §5.C: the only separable large piece would be a true-sibling node-model change, which F2 rules out as unnecessary (no contract benefit, large migration). No other separable large piece surfaced; splitting would manufacture scope.
- **Defers (if chosen).** Defers the true-sibling node model as an ADR-0003 Revisit trigger (deferrable, NOT needed for #81). No KC-04 chunk is opened by this cycle.

**§1 self-check (chunk-planner v32 P-plan-1-v32):** §1 carries a populated `#### F<N> = F<N>.<rec>` subsection for all 5 forks (F1–F5) — F1 with User-visible / Code-level binding / Rationale / Product trajectory / Defers (product-semantic, per v26); F2–F5 (TECHNICAL) with Code-level binding / Rationale / Defers. Each block ≥ 3 sentences. Invoke `chunk-template-validate-locked-appendix` at end-of-draft → expect PASS (5 subsections ≥ 5-lock count, each ≥ 3 sentences).

---

## §2 — Concept alignment

| Concept doc | Line(s) | Current statement | KC-03 alignment | Action |
|---|---|---|---|---|
| `docs/concepts/concept-brake.md` | `:1` (verified-header), `:192` (co-keep constraint; amended at KC-01 to "call-atomic", at KC-02 to "target-aware reclaim/keep") | States the KC-02 (B) target-aware disposition (call-node reclaims; result-node KEEPS the whole cluster) | KC-03 SUPERSEDES the keep-whole disposition: a pinned revert onto X SHRINKS every node after X (Rule 1, uniform across categories); the summary is ADDED to X after its content (Rule 2/2a); the orphan call is removed by id (Rule 3); abandon REPLACES X's content | Refresh verified-header + REWRITE the `:192` note: pinned-revert disposition is the simple tail-shrink contract — shrink every node after X always; pinned ADDS the summary after X's content, abandon REPLACES; orphan call removed by id (cite ADR-0003, which SUPERSEDES ADR-0002 §D2.1) |
| `src/types/context.rs` `retain_pinned_calls_on_node` doc-comment | `:743-768` (the KC-02 (B) 3-case bullet list: keep-if-on-trunk / off-trunk+tag-on-call-node reclaim / **off-trunk+tag-on-result-node RE-APPEND-by-membership**) + the mirror at `:691-714` | Documents the **re-append-off-trunk-sibling** path (the KC-02 (B) keep-whole) — the misread | KC-03 rewrites to the shrink form: keep-if-on-trunk (Rule 1 don't-over-shrink) / off-trunk → DROP the result + surgically remove the paired call by id (uniform; converges to the abandon arm); the summary placement (ADD vs REPLACE) is the only pinned-vs-abandon difference | Rewrite both doc-comment regions to the shrink form keyed on on-trunk vs off-trunk (NO `tag_on_call_node` re-append branch); cite ADR-0003 §D3.1 + the supersession of ADR-0002 §D2.1 |
| `src/tools/revert.rs` `RevertTool::description()` | `:107-109` | Says "completion/step-summary still drop the abandoned tail … they do NOT shrink the kept span you reverted to. completion/step-summary reclaim the abandoned tail" — partly correct intent BUT the code (KC-02 (B)) does NOT drop the tail on into-cluster reverts (it re-appends the whole cluster) | KC-03 makes the code match: the tail IS dropped on EVERY revert (Rule 1); pinned ADDS the summary after X's content (Rule 2a); abandon REPLACES | Rewrite the description (+ the `tool_help.rs` `REVERT_HELP` body `:129-156`) to state R1/R2/R2a/R3 precisely; per CLAUDE.md "no forward references / code is source of truth", the doc must not claim behavior it doesn't have |
| `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md` (ADR-0002) | `§D2.1` (the keep-whole-cluster-on-result-node disposition) + `## Verification` | ADR-0002 §D2.1 asserts the result-node re-append KEEPS the whole gathered cluster (KC-02 (B)) | KC-03 ADR-0003 **SUPERSEDES** §D2.1: the simple contract SHRINKS the tail on every revert (no result-node re-append); §D2.2/§D2.3/§D2.4 (abandon confirm / absorb / no-400) carry forward unchanged | Add an inline `> Superseded-in-part by ADR-0003 §D3.1 (2026-06-09): the result-node keep-whole re-append is replaced by the simple tail-shrink contract — every node after X is shrunk, uniformly across categories.` note at ADR-0002 §D2.1 + bump ADR-0002 verified-header |
| `docs/decisions/0001-parallel-revert-call-atomicity.md` (ADR-0001) | `§D1.5` (R4 empty-node — relied on for breadcrumb keep-as-text) | ADR-0001 §D1.5 R4 keeps text-bearing nodes (an all-calls-shrunk pinned node still renders its breadcrumb, not an empty-assistant 400) | KC-03 relies on R4 unchanged for the reclaim path (a pinned call-node whose every call shrank keeps its breadcrumb tag-text) | Confirmatory; consumed unchanged. ADR-0001 §D1.2 (already superseded-in-part by ADR-0002) — ADR-0003 notes the chain but does not re-amend it |
| `src/types/content.rs` | `:143` (native `id ↔ tool_call_id` join) | Documents the join; KC-03 consumes it for the surgical orphan-call removal by id (Rule 3) | Confirmatory; consumed unchanged | No change |

**Concept-fidelity note (KC-02 supersession — load-bearing):** KC-03 is the **second phi-core cross-ADR supersession**. ADR-0003 §D3.1 SUPERSEDES ADR-0002 §D2.1's result-node keep-whole-cluster re-append (the KC-02 (B) misread that #81 corrects). Under the simple contract: a pinned revert onto X SHRINKS every node strictly after X (Rule 1, uniform across categories), so the pinned arm CONVERGES to the abandon arm's already-correct shrink (P0 F-8), differing only by Rule 2 (pinned ADDS the summary to X; abandon REPLACES X's content). This is **intended, not a regression**: it is the locked simple contract (#81), correcting the KC-02 (B) keep-whole heuristic. The on-trunk-keep gate (clause i) is PRESERVED — it is Rule 1's "no node after X ⇒ nothing shrinks" guard (the load-bearing don't-over-shrink rule), NOT the keep-whole re-append being removed.

---

## §3 — phi-core leverage map → **kernel-minimality surface-discipline** (project=phi-core)

> phi-core-leverage-check is **N/A** — the kernel does not consume itself. Per `[[feedback_phi_core_kernel_minimal]]` + the task directive, §3 carries the **kernel-minimality surface-discipline analysis** (the P0 §4 fix-locus carve-out, cited + ratified). The KC-02 leverage map established the carve-out shape; KC-03 inherits it.

### §3.A — Kernel-minimality carve-out (cite P0 §4)

| Change (locked fork) | Side | General-primitive? | Consumer-specific leakage? | Blast radius |
|---|---|---|---|---|
| F1: rework `retain_pinned_calls_on_node` (`:769`) — REMOVE clause (iii) re-append (`:839-852`), shrink the off-trunk result + drop the paired call by id (converge to the abandon arm); keep clauses (i) on-trunk + (ii) call-node-reclaim | **kernel render-pass** | YES — the simple tail-shrink contract is a general revert-correctness invariant every consumer needs; zero consumer logic; the current re-append is a kernel bug (the KC-02 misread), not an i-phi requirement | NONE | `retain_pinned_calls_on_node` (`context.rs:769`) ONLY (no caller signature change — `tag_on_call_node` already threaded; KC-03 narrows its use, may even render it unused → review) |
| F1: append-after-original + single-label in `weave_braking_annotations` (`:929`) — split the marker (leading) from the `[kind: text]` annotation (appended after content for pinned-kept nodes); collapse the double-label | **kernel render-pass** | YES — Rule 2a ordering correctness; no consumer logic | `weave_braking_annotations` (`:929`) + a new append variant of `prepend_marker_to_message` (`:12`); the `[n<id>]`-echo tests (`:1605+`) constrain it |
| F1: truthful abandoned-list in `apply_revert` (`run.rs:832-870`) — exclude the kept target cluster's own calls on a pinned-onto-result revert | **kernel + apply_revert** | YES — render correctness (name only what was shrunk) | the `abandoned_tool_names` name source-set (`run.rs:832-870`) ONLY; the active-pointer/reject rules untouched |
| F2/F5: no node-model / `build_trunk_context` / KC-04 split | n/a — not required (P0 F-2 / §5.C) | n/a | n/a | none |
| Doc-comment (`:743-768` + `:691-714`) + `concept-brake.md:192` + `revert.rs` description + `tool_help.rs` REVERT_HELP + ADR-0002 §D2.1 supersession note + ADR-0003 | kernel docs | doc-code alignment for the simple tail-shrink contract | NONE | docs only |

**Verdict (cite P0 F-2 / F-3 / F-8 / §4):** fix-locus is **render-pass-only**, across THREE functions (`retain_pinned_calls_on_node` + `weave_braking_annotations` + `apply_revert`'s breadcrumb name-set), all on the revert-mode render path gated by `active_node_id.is_some()`. `collapse_abandon_class_cluster`'s sole production call-site is `streaming.rs:237` (revert-mode only); `run.rs:1757` is a `#[cfg(test)]` harness (`persistent_collapse_end_to_end_via_real_apply_revert`, VERIFIED). **Consumer-leakage grep (VERIFIED at HEAD `9d0e46b`)**: `grep -rn 'collapse_abandon_class_cluster|retain_pinned_calls_on_node' src/ | grep -v context.rs` returns ONLY `streaming.rs:237` (production) + `run.rs:1757` (test) + doc-comment cross-refs in `node_tag.rs` — `sub_agent.rs` / `agent_loop/parallel.rs` / `evaluation.rs` / `types/parallel.rs` do **NOT** invoke the collapse/retain path → **zero blast radius outside the brake render**. `build_trunk_context` (P0 F-2, correct, untouched) + `apply_revert` active-pointer/reject (P0 F-1/F-3, correct, untouched except the breadcrumb name-set) + `enforce_call_atomic_backstop` (P0 F-8, stays as the belt) all unchanged. Forensic `self.messages` `.iter().find()` + `.clone()` only — **never mutated**. No public-API / persisted-field / migration / tool-arg-schema change (tool DESCRIPTION text change only — not the schema). **Zero i-phi leakage.** Genuine general kernel fix per the `[[feedback_phi_core_kernel_minimal]]` carve-out.

### §3.B — Build / LOC caps + pause-discipline (phi-core; K8s axes N/A — library)

> **K8s readiness: N/A — phi-core is a library, not a daemon** (k8s-readiness-check returns N/A for all 7 axes; no `m7b` deferred-ledger; no `CHK8S-D-NN` entry). The table is the per-file LOC cap + pause-trigger (chunk-planner v23 P-plan-1 functional-scope-derivation; v33 P-plan-1-v33 framework-boilerplate allowance for inline tests). KC-03 is a **net SIMPLIFICATION** of the production code (removing clause (iii) re-append is a deletion; the shrink reuses the existing `ids_to_drop` retain) PLUS an append-variant in weave + a name-set filter.

| File | Surface | LOC-cap derivation (per-axis) | Cap | Pause @ (1.5×) |
|---|---|---|---|---|
| `src/types/context.rs` (`retain_pinned_calls_on_node` `:769`) | REMOVE clause (iii) re-append (`:839-852`, ~−14 LOC); route off-trunk + result-node into the existing `ids_to_drop` drop (~+2 LOC); on-trunk + call-node clauses unchanged; review whether `tag_on_call_node` / `results_to_append` become unused (~−4 LOC). Net body ≈ **−16 to −10 LOC** (a simplification) | per-axis ≈ `−16/+4` net body production (deletion-dominant) | **≤ 20 LOC net delta (production simplification + headroom)** | 30 |
| `src/types/context.rs` (`weave_braking_annotations` `:929` + `prepend_marker_to_message` `:12`) | add an append-after-content variant + thread the pinned-kept-node decision (~+18 LOC); collapse the double-label (~−2/+2 LOC) | per-axis ≈ +18 net body | **≤ 30 LOC** | 45 |
| `src/agent_loop/run.rs` (`apply_revert` name-set `:832-870`) | exclude the kept target cluster's own calls on a pinned-onto-result revert (a guard on the `:860-863` seed; ~+8 LOC) | per-axis ≈ +8 net body | **≤ 15 LOC** | 22 |
| `src/types/context.rs` (inline tests `collapse_abandon_class_cluster_tests` `:2106`) | INVERT the 3 KC-02 keep-whole tests (`row_pinned_revert_to_first_result_now_call_atomic:3286`, `matrix_pinned_into_cluster_keeps_whole_cluster:3737`, the re-append sub-cases of `matrix_pinned_last_result_and_post_cluster_no_loss:3876`); ADD the contract cells (tail-shrunk-after-X, orphan-removed-by-id, append-order, abandon-replace-at-X, truthful-list, single-label) via grouped assertions; STAY-green probes preserved | invert 3 in place (~+0 net count) + 6-10 NEW grouped/edge `#[test]` fns × ~30 LOC + reuse existing fixtures (`parallel_fixture:3225`, `parallel_fixture_3`, `multi_call_node`, `trunk_has_*` helpers) + framework allowance (chunk-planner v33 P-plan-1-v33: `AgentContext` construction boilerplate ~+40 LOC) ≈ 250-380 LOC | **≤ 420 LOC inline-test** | 630 |

**Pause-discipline (cascade-band):** F1 is a **render-pass-only change with NO struct/trait/signature cascade** (P0 §4: the `tag_on_call_node` param already exists; KC-03 narrows its use). Production net is a **simplification** (`retain_pinned_calls_on_node` shrinks) plus two small additive surfaces (weave append-variant, name-set filter). Production band `[−16, +30]` across the 3 functions. **PAUSE if** any of: (a) `retain_pinned_calls_on_node` net production exceeds **30 LOC** (the shrink should be a deletion, not an addition — an overrun signals a wrong approach); (b) the append-after-original weave needs to touch `build_trunk_context` or the node model (would exceed the render-pass-only bound — P0 F-2 says it must not); (c) the truthful-list change requires `apply_revert`'s active-pointer/reject logic to change (P0 F-1/F-3 say it must not); (d) inline-test > 630 LOC. Surface via AskUserQuestion; do NOT push through. **No K8s; no migration; no public API change.**

### §3.C — Inline-test cardinality (chunk-planner v27 P-plan-1-v27 + v30 P-plan-10-v30)

The matrix is a `category × revert-target × arity` coverage surface (NOT enum-dispatch). The P0 §7 contract cells promote to a manageable in-tree set via grouped assertions (one `#[test]` per coherent cell-group). Inline tolerance derivation (per-Tier breakdown in §8): **6-10 NEW grouped/edge tests** + **3 INVERTED KC-02 tests** (the keep-whole flips, UPDATED in place). Band `[6, 10]` NEW allowing ≤ 3 incidental edge tests the implementer adds for clarity. Baseline: **30 existing `collapse_abandon` lib tests** (VERIFIED via `cargo test --lib collapse_abandon -- --list` → 30 at HEAD `9d0e46b`); **244 total lib tests** (VERIFIED). The 3 inverted tests are UPDATED (not added/removed — net count change 0); the STAY-green probes (`matrix_on_trunk_pinned_cluster_kept_always:3778`, the abandon-class cells, M=1 cells) stay GREEN unchanged.

### §3.D — Forward-scope ↔ concept-doc precedence

No closed-set / fixed-order / frozen-schema contradiction. The fix changes a render rule (shrink the tail uniformly + append-after-content + truthful-list) and corrects a doc that overstated behavior; it does not extend a closed vocabulary or alter a migration order. `N/A — no contradiction to surface`.

### §3.E — Anticipated gate-2.5 candidates (chunk-planner v13)

- **Append-after-original interaction with the `[n<id>]`-echo tests.** The weave split (marker stays leading; `[kind: text]` appends after content) must keep `weave_braking_annotations_renders_marker_and_tags_into_content` (`:1605`) + the 5 sibling echo tests (`:1628`/`:1637`/`:1652`/`:1702`/`:1716`) GREEN — the model relies on the leading `[n<id>]` for a valid `step=`. **Planner-rec: confirm at P0** the split keeps the marker leading; if the echo tests require the tag adjacent to the marker (not separable), surface via AskUserQuestion at gate-2.5 (a small weave-shape adjustment, do NOT push through). [v0 scope: fully-behavioral — append-after-content for pinned-kept nodes; the marker-leading invariant is preserved.]
- **All-calls-shrunk pinned call-node keeps renderable breadcrumb (R4 reliance).** When a pinned revert onto the call node shrinks every call (the reclaim case, clause ii), the node retains only its pinned-tag breadcrumb text → `weave_braking_annotations` + ADR-0001 §D1.5 R4 keep it as text (not an empty-assistant 400). **Planner-rec: confirm at P0** the reclaim path still renders the breadcrumb (P0 F-8 confirms the abandon arm does this; the pinned-reclaim converges to it). If the node collapses to genuinely empty → small additive weave/R4 fix; surface at gate-2.5.
- **`tag_on_call_node` param fate after clause-(iii) removal.** Removing the result-node re-append may leave `tag_on_call_node` used only to distinguish reclaim (clause ii) from the shrink-drop (now uniform) — review whether it stays meaningful or becomes removable (a tidy-up). **Planner-rec: keep it if it still selects the breadcrumb-reclaim vs the plain-drop semantics for the call-node case; remove if dead. Implementer's choice at P1** — document the choice in the doc-comment + ADR-0003 §D3.1.

---

## §4 — Drifts / issues closed

| Item | Status flip at this cycle | Routing |
|---|---|---|
| **GitHub #81 / D-TEST-0076 (S2)** — the R1/R2/R2a/R3 tail-shrink contract + the 2 render nits | **Status flip at this cycle** — moves to closed at chunk-seal (orchestrator-run live disposition-asserting close-gate + unit regression green). The GitHub issue IS the tracking artifact (no phi-core drift-file ledger). | P-SEAL: post the closure comment on #81 citing the re-authored contract matrix + the live transcript-read close-gate result. Per the commit-subject discipline (outer CLAUDE.md), the P-SEAL commit subject prepends `D-TEST-0076:`. |
| **#80 / D-TEST-0075 — SUBSUMED** | **Status flip at this cycle** — KC-02 closed #80 on the WRONG invariant (the (B) keep-whole misread); KC-03 is the correct closure of the underlying semantics. | P-SEAL: post a note on #80 confirming KC-03 corrects the KC-02 (B) disposition; reference ADR-0003 SUPERSEDES ADR-0002 §D2.1. |
| **NEW investigation-surfaced defect drift** | **No-op confirmation** — P0 surfaced NO new defect beyond the keep-whole misread (the 9 findings are all DEFINITIVE; §9 "Open: NONE"). | The forward-scope §5.5 "file each NEW failure as its own D-TEST drift" closes as a no-op (no new hard failure found). |

**Prior-cycle ratification (chunk-planner v34 P-plan-2-v34 — this-cycle vs prior-cycle distinction):** KC-01 (`46f7854`) + KC-02 (`d835a363`, on `dev` at `9d0e46b`) are **prior-cycle landed** prereqs (no state transition at this cycle); KC-03 SUPERSEDES ADR-0002 §D2.1 in-part (an ADR amendment at this cycle, not a drift — see §5).

---

## §5 — ADR draft

> phi-core `docs/decisions/` exists (KC-01 minted ADR-0001; KC-02 added ADR-0002). KC-03 adds **ADR-0003**: `docs/decisions/0003-revert-tail-shrink-contract.md`. Numbering continues at `0003`. Cite by `ADR-0003 §D3.<M>`. **ADR-location lookup (chunk-planner v22 P7, VERIFIED):** prior ADRs live at `docs/decisions/000{1,2}-*.md` (NOT `docs/specs/decisions/`); KC-03 uses the matching path.

**Proposed: phi-core ADR-0003 — The simple revert tail-shrink contract (R1/R2/R2a/R3): the pinned arm converges to the abandon arm; SUPERSEDES ADR-0002 §D2.1.** Status: Accepted. The implementer authors all **7 canonical ADR top-level sections** (chunk-planner v17 explicit-enumeration — listed so the implementer omits NONE, especially `## Revisit triggers` + the full `## Consequences`):

1. `## Forks` — header table: **F1** (tail-shrink locus + the simple contract → **F1.render-pass-shrink**, product-semantic, investigation-forced; notes #81 supersedes the KC-02 (B) keep-whole) / **F2** (node-model adequacy → linear-chain-suffices, TECHNICAL) / **F3** (append-mechanics → append-after-original+single-label+truthful-list, TECHNICAL) / **F4** (carry-forward → invert-the-3-KC02-keep-whole-tests, TECHNICAL) / **F5** (split → absorb, TECHNICAL).
2. `## Context` — #81 / D-TEST-0076 S2 contract (R1/R2/R2a/R3 + the 2 render nits + the 2 worked examples); the P0 9 DEFINITIVE findings (F-2: trunk walk already shrinks; F-3: render pass re-introduces; F-4: the re-introducer is `retain_pinned_calls_on_node` clause (iii) — the KC-02 (B) misread; F-8: the abandon arm is the proven shrink template); why KC-02's (B) keep-whole was the misread #81 corrects.
3. `## Sub-decisions` — (each ends with a Pre-existing-behaviour preservation note; chunk-planner v11 + v24 P-plan-2-v24):
   - `### §D3.1 — F1 the simple tail-shrink contract; the pinned arm converges to the abandon arm (resolves F1; SUPERSEDES ADR-0002 §D2.1)`. **Pre-existing-behaviour:** ADR-0002 §D2.1 (KC-02 (B)) re-appended the off-trunk parallel sibling on a pinned-onto-result revert (`retain_pinned_calls_on_node:839-852`), KEEPING the whole gathered cluster — the misread. KC-03: **Rule 1** — revert to X SHRINKS every node strictly after X (always, any category); **Rule 2** — abandon-class REPLACES X's content with the summary, pinned-class ADDS the summary to X (X content kept); **Rule 2a** — pinned append order is `original content → revert summary → [continue_after_revert]`; **Rule 3** — a removed tool_result's paired tool_call is surgically removed by id from its kept call node. The pinned arm CONVERGES to the abandon arm's already-correct shrink (P0 F-8), differing only by Rule 2 (ADD vs REPLACE). **Explicitly state**: ADR-0002 §D2.1's result-node keep-whole no longer holds; this is intended (#81's simple contract), not a regression. The on-trunk-keep gate (clause i) is PRESERVED as Rule 1's don't-over-shrink guard (no node after X ⇒ nothing shrinks).
   - `### §D3.2 — F3 append-after-original + single-label + truthful-list (resolves F3)`. **Pre-existing-behaviour:** the breadcrumb was PREPENDED (`weave_braking_annotations:962` sandwiching the body) with a double-label (`[outcome: reverted past: …]`) and the abandoned-list named the kept target's own calls (P0 F-6/F-7). KC-03: marker `[n<id>]` stays leading; the `[kind: text]` annotation appends after content; collapse the double-label to one; exclude the kept target's own calls from the abandoned-list.
   - `### §D3.3 — F2 linear-chain-suffices + F5 absorb, no node-model change (resolves F2 + F5)`. *Net-new confirmation — no prior behaviour to preserve (the node model is unchanged):* the contract is fully expressible on the existing linear chain (P0 F-2 / §5.C); no `build_trunk_context` / sibling-stamping change; no KC-04 split. The true-sibling node model is a deferrable Revisit trigger.
   - `### §D3.4 — the no-400 invariant is preserved by the backstop (carry-forward of ADR-0002 §D2.4)`. *Net-new confirmation:* `enforce_call_atomic_backstop` (`streaming.rs:130`) STAYS as the belt; after the pinned arm shrinks the tail + drops the orphan call by id (Rule 3), the backstop guarantees no orphan result / no dangling call survives → no 400. P0 F-8/F-9: the abandon arm already satisfies this; the converged pinned arm inherits it.
4. `## Cross-references` — **(a) concept-doc**: `concept-brake.md:192` + `context.rs:743-768`/`:691-714` doc-comments + `revert.rs:107-109` description + `tool_help.rs:129-156` REVERT_HELP; **(b) closed drifts/issues**: #81 / D-TEST-0076 (primary) + #80 / D-TEST-0075 (subsumed/corrected); **(c) prior ADRs (milestone-path-explicit per chunk-planner v6)**: `docs/decisions/0002-…md` §D2.1 (**SUPERSEDED-in-part by §D3.1**) + §D2.2/§D2.3/§D2.4 (carried forward) + `docs/decisions/0001-…md` §D1.5 (R4 empty-node — relied on for breadcrumb keep-as-text); **(d) forward-scope row**: `docs/specs/plan/forward-scope/kc-03-revert-tail-shrink-contract.md` + P0 investigation `…/_p0-investigations/kc-03-…-p0-investigation.md` §3 (F-1..F-9) + §4 (fix-locus) + §6 (forks) + §7 (contract cells).
5. `## Consequences` — `### For consumers (i-phi / baby-phi / future)`: pinned reverts now actually SHRINK the context budget (the tool's whole point) — every node after X is dropped, the summary is appended cleanly after X's content, no orphan call, a truthful single-label breadcrumb; hardens braking ahead of the MCP / combined-agent cluster (#67 / #64 / #68 — forward-routing note). `### For KC-02 / ADR-0002`: §D2.1 keep-whole superseded; the 3 KC-02 keep-whole tests inverted; ADR-0002 §D2.2/§D2.3/§D2.4 unchanged. No public API / persisted-field / migration change.
6. `## Revisit triggers` (≥ 5 — chunk-planner v17 mandatory section): (i) a future **true-sibling node model** is adopted → re-opens §D3.3 (the orphan-call removal could use the direct-child gate instead of the id join). (ii) A product decision that a pinned `completion` MUST preserve gathered tail work (not shrink) → re-opens §D3.1 (would re-introduce a keep-whole class — the KC-02 (B) direction, now superseded). (iii) A new revert-render path bypasses `retain_pinned_calls_on_node` → re-opens §D3.3 (the absorb-vs-split boundary). (iv) The append-after-original weave breaks the `[n<id>]`-echo contract (a future weave/marker change) → re-opens §D3.2's marker-leading invariant. (v) The all-calls-shrunk pinned call-node stops rendering its breadcrumb (a weave/R4 change) → re-opens §D3.1's reliance on ADR-0001 §D1.5 R4 keep-as-text. (vi) A future decay/window change interacts with the on-trunk-keep gate (clause i) → re-opens Rule 1's don't-over-shrink guarantee.
7. `## Verification` — `cargo test -j 4 --manifest-path … --lib collapse_abandon` (the re-authored contract matrix + the 3 inverted KC-02 tests + the STAY-green probes) + clippy `-Dwarnings` + `fmt --check` + the live transcript-read disposition-asserting full-grid close-gate (§10).

**ADR-0002 amendment (P-SEAL paperwork):** add the `> Superseded-in-part by ADR-0003 §D3.1` inline note at ADR-0002 §D2.1 + bump ADR-0002's verified-header to cite the KC-03 amendment (per outer CLAUDE.md ADR-inline-amendment verified-header discipline).

**Tool-doc rewrite (P-SEAL — §5 forward-scope deliverable 3):** rewrite `RevertTool::description()` (`revert.rs:107-109`) + `tool_help.rs` `REVERT_HELP` (`:129-156`) to describe the ACTUAL R1/R2/R2a/R3 behavior: every revert shrinks the tail after X; `failure`/`tangent` REPLACE X's content with the summary; `completion`/`step-summary` ADD the summary AFTER X's content (X kept); the orphaned paired call is removed. The doc MUST NOT claim behavior the code does not have (CLAUDE.md "code is source of truth / no forward references").

---

## §6 — Prior-chunk regression / carry-forward invariants

KC-02 (`d835a363`, on `dev` HEAD `9d0e46b`) is the immediate prereq (it shipped the (B) code KC-03 reworks); KC-01 (`46f7854`) is the foundation. The invariants the F1 render change must honor (or intentionally supersede):

| Invariant | Verifying command | Expected |
|---|---|---|
| 30 existing `collapse_abandon` lib tests stay green **EXCEPT** the 3 KC-02 keep-whole tests INVERTED in place | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon` | 27 of 30 unchanged-green; the 3 inverted tests UPDATED to the shrink dispositions; total after +NEW contract tests (see §8) = 33-37 |
| **On-trunk-keep gate (load-bearing, Rule 1 don't-over-shrink)**: `matrix_on_trunk_pinned_cluster_kept_always` (`context.rs:3778`) + `pinned_cluster_mid_trunk_kept_verbatim` (`:3061`) stay GREEN unchanged — a pinned cluster the model continued PAST (results on-trunk) is kept verbatim (no node after X ⇒ nothing shrinks) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib matrix_on_trunk_pinned_cluster_kept_always pinned_cluster_mid_trunk_kept_verbatim` | GREEN (clause (i) preserves them; this is Rule 1, NOT the keep-whole being removed) |
| **Abandon-class arm unchanged (the convergence template, P0 F-8)**: all `matrix_abandon_class_*` cells stay GREEN | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib matrix_abandon_class` | GREEN (the pinned arm converges TO this; abandon arm not touched) |
| `build_trunk_context` linear parent-walk unchanged (P0 F-2; sole `parent_id` consumer) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib build_trunk_context` | green (untouched) |
| `apply_revert` active-pointer + reject rules unchanged (P0 F-1/F-3); forensic `self.messages` never mutated | grep `self.messages` mutation in `retain_pinned_calls_on_node` = none (`.iter().find()` + `.clone()` reads only); `apply_revert` change is the breadcrumb name-set ONLY | render-pass-only confirmed |
| `enforce_call_atomic_backstop` (`streaming.rs:130`) unchanged; no-400 invariant holds (P0 F-8/F-9 / §D3.4) | read diff; `streaming.rs` untouched | order intact; 0 BROKEN-400 |
| `weave_braking_annotations` `[n<id>]`-echo tests stay green (the marker stays leading after the append-after-original split) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib weave_braking_annotations` | green (6 echo tests `:1605`/`:1628`/`:1637`/`:1652`/`:1702`/`:1716`) |
| Full phi-core suite green (blast radius = 3 render fns, all `active_node_id.is_some()`-gated) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path …` | green (244 lib baseline + NEW contract tests; integration suites green) |

**KC-02 test-inversion (load-bearing — chunk-planner v13 R1 closed-set verification, grep-verified against live HEAD `9d0e46b`):** the keep-whole assertions that encode the WRONG spec, all currently green on `dev`:

- **`row_pinned_revert_to_first_result_now_call_atomic`** (`context.rs:3286`) — revert onto result_a (n2); pcall_a on-trunk, pcall_b off-trunk (grandchild). **Today** asserts `trunk_has_tool_result("pcall_b")` (`:3304-3307`, "(B) re-appended, whole cluster kept"). **Under the contract** pcall_b's result is SHRUNK (Rule 1) + the orphan pcall_b call removed by id (Rule 3). **INVERT**: assert `trunk_has_tool_result("pcall_a")` (on-trunk, kept) + `!trunk_has_tool_result("pcall_b")` (shrunk) + `!trunk_has_dangling_call` (orphan call removed).
- **`matrix_pinned_into_cluster_keeps_whole_cluster`** (`context.rs:3737`) — first-result a2/a3 + mid-result a3 keep-whole (`:3742-3769`). **INVERT** to tail-shrunk-after-X: the off-trunk siblings are shrunk + their orphan calls removed; `!trunk_has_dangling_call`, `assert_no_400`. (Likely RENAME to `matrix_pinned_into_cluster_shrinks_tail`.)
- **`matrix_pinned_last_result_and_post_cluster_no_loss`** (`context.rs:3876`) — **SPLIT by target position**: the revert-onto-LAST-result / fully-on-trunk sub-cases STAY (nothing after X → nothing shrinks); only any re-append-past-tip sub-case inverts to shrink. The planner flags this for the implementer to split at P1.

**Confirmation (grep-verified):** `matrix_on_trunk_pinned_cluster_kept_always` (`:3778`) + `pinned_cluster_mid_trunk_kept_verbatim` (`:3061`) do **NOT** invert — their clusters are fully on-trunk (nothing strictly after X), so Rule 1 shrinks nothing → kept verbatim (the don't-over-shrink guard, clause i preserved). All `matrix_abandon_class_*` cells do NOT change (F-8: abandon arm is the convergence target). **KC-02 keep-whole inversion count = 3 tests** (one is a SPLIT).

**Back-compat decision (chunk-planner v19 P2 — F1 supersedes a KC-02 scaffold):** F1 SUPERSEDES the KC-02 (B) keep-whole disposition (ADR-0002 §D2.1). Decision: **(a) amend the 3 carry-forward test bodies to the shrink semantic** + document the supersession in ADR-0003 §D3.1 + the ADR-0002 §D2.1 inline note. NOT (b) preserve-scaffold (the keep-whole re-append IS the thing being superseded — it was the misread) nor (c) defer/ignore (the inversions ARE the intended fix). Canonical amend-carry-forward-test path.

---

## §7 — Phase plan

> **§7.0 phase-order stress-test (chunk-planner v25 P-plan-4-v25).** 4 phases; F1 investigation-forced (no USER-DIVERGENT lock — the evidence settles it); F2–F5 TECHNICAL at investigation-rec. No cascade-band overlap. **Compile-time:** F1 changes NO signature (the `tag_on_call_node` param already exists; KC-03 narrows its use, may remove it) + adds an append-variant fn in `context.rs` + a guard in `run.rs` — no struct/trait change → no RED window from type churn; the 3 functions compile independently and land in P1. **Runtime-test:** P1 lands the F1 render rework (the `retain_pinned_calls_on_node` shrink + the weave append-after-original + the truthful name-set) which IMMEDIATELY makes the **3** KC-02 keep-whole tests RED — so **P1 MUST also invert those 3 tests in the same phase** to close the window (the inversions are mechanical + co-located; the STAY-green probes `matrix_on_trunk_pinned_cluster_kept_always` / `pinned_cluster_mid_trunk_kept_verbatim` stay GREEN by clause (i)). P2 ADDS the NEW contract cells (GREEN-on-arrival, authored after the fix lands). **Single RED-window mitigation:** P1 lands the fix + the 3-test inversion together → no boundary leaves the suite RED. Diff-coherent for a 3-auditor Large envelope (the carry-forward-regression surface = the 3 KC-02 inversions + the on-trunk-gate + the abandon-arm-unchanged + the no-400-across-cells invariant → Audit C).

### P0 — Read + ground (no code)
- **Goal:** implementer reads §9 reading list; confirms the 9 P0 facts + the exact `retain_pinned_calls_on_node:769` body (esp. the clause-(iii) re-append at `:839-852` being REMOVED), the abandon-arm shrink template (`:557-690`, the convergence target), the `weave_braking_annotations:929` prepend + `prepend_marker_to_message:12`, the `apply_revert` name-set (`run.rs:832-870`) at live HEAD (re-verify if drift since plan-draft); confirms the §3.E candidates (append-after-original keeps the `[n<id>]`-echo tests green; the all-calls-shrunk pinned call-node keeps its breadcrumb via R4; the `tag_on_call_node` param fate).
- **Deliverables:** confirmation note; no diff.
- **Tests:** baseline `cargo test … --lib collapse_abandon` = 30 green; `matrix_on_trunk_pinned_cluster_kept_always` + `pinned_cluster_mid_trunk_kept_verbatim` green; the 6 `weave_braking_annotations_*` echo tests green.
- **Confidence:** 10/10. **Pause-discipline:** if the append-after-original split cannot keep the `[n<id>]` marker leading, OR the all-calls-shrunk call-node renders genuinely empty (no breadcrumb), OR the truthful-list filter needs `apply_revert`'s active-pointer/reject logic → AskUserQuestion before P1.

### P1 — F1 fix (the 3 render functions) + invert the 3 KC-02 keep-whole tests
- **Goal:** implement the simple contract: (1) `retain_pinned_calls_on_node` — REMOVE clause (iii) re-append (`:839-852`); off-trunk results DROP + their paired call removed by id (Rule 1 + Rule 3, via the existing `ids_to_drop` retain); clauses (i) on-trunk-keep + (ii) call-node-reclaim STAY (converge to the abandon arm). (2) `weave_braking_annotations` + a `prepend_marker_to_message` append variant — marker `[n<id>]` leading; `[kind: text]` appended AFTER content for pinned-kept nodes (Rule 2a); collapse the double-label. (3) `apply_revert` name-set — exclude the kept target cluster's own calls on a pinned-onto-result revert. UPDATE the 3 inverted KC-02 tests (`row_pinned_revert_to_first_result_now_call_atomic:3286`, `matrix_pinned_into_cluster_keeps_whole_cluster:3737`, the re-append sub-cases of `matrix_pinned_last_result_and_post_cluster_no_loss:3876`) to the shrink dispositions (§6).
- **Deliverables:** `src/types/context.rs` — the `retain_pinned_calls_on_node` simplification (net ≈ −16/+4, §3.B) + the weave append-variant (≈ +18); `src/agent_loop/run.rs` — the name-set filter (≈ +8); the 3-test inversion (§6). Render-pass only; `streaming.rs` + `build_trunk_context` + the node model untouched; forensic `self.messages` read-only; `apply_revert` active-pointer/reject untouched.
- **Tests:** 27 of 30 existing stay green + `matrix_on_trunk_pinned_cluster_kept_always` + `pinned_cluster_mid_trunk_kept_verbatim` + all `matrix_abandon_class_*` + the 6 weave echo tests green; the 3 updated tests green under the shrink contract.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if `retain_pinned_calls_on_node` net production > 30 LOC (it should SHRINK — an overrun signals a wrong approach), OR the append-after-original needs to touch `build_trunk_context`/node-model, OR the truthful-list needs the active-pointer/reject logic → AskUserQuestion (§3.B).

### P2 — Add the contract cells (tail-shrunk, orphan-removed, append-order, abandon-replace, truthful-list, single-label)
- **Goal:** ADD the NEW in-tree contract cells in `collapse_abandon_class_cluster_tests` (`:2106`) — assert: (a) pinned-onto-X tail shrunk strictly after X (Rule 1) across category × arity; (b) the orphan paired call removed by id, `!trunk_has_dangling_call` (Rule 3); (c) pinned append-order = `[n<id>] <original body> [<kind>: …]` (original FIRST, Rule 2a) — a render-string assertion via `weave_braking_annotations`; (d) abandon-class REPLACES X's content (Rule 2, already green via F-8 — add an explicit assertion); (e) truthful abandoned-list names ONLY the shrunk tools, not the kept target's own (F-7); (f) single-label (ONE of `outcome`/`reverted past`, not both, F-7). Reuse `parallel_fixture:3225` / `parallel_fixture_3` / `multi_call_node` / `trunk_has_tool_result` / `trunk_has_dangling_call` / `trunk_has_breadcrumb`; add a `trunk_render_string`/append-order helper if not present.
- **Deliverables:** 6-10 NEW grouped/edge `#[test]` fns; reuse existing fixtures/helpers; ≤ 1-2 NEW helpers (append-order render assertion).
- **Tests:** all NEW cells GREEN; no-400 invariant (`!trunk_has_dangling_call` + `!trunk_has_orphan_result`) holds across the exercised cells.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if inline-test > 630 LOC (§3.B).

### P-SEAL — Docs + ADR + tool-doc + paperwork
- **Goal:** rewrite the `retain_pinned_calls_on_node` doc-comments (`:743-768` + `:691-714`) to the shrink form; rewrite `RevertTool::description()` (`revert.rs:107-109`) + `tool_help.rs` REVERT_HELP (`:129-156`) to R1/R2/R2a/R3; update `concept-brake.md:192` + verified-header; create `docs/decisions/0003-revert-tail-shrink-contract.md` (7 sections, §D3.1–§D3.4, ≥ 5 Revisit triggers); add the ADR-0002 §D2.1 supersession inline note + ADR-0002 verified-header bump; append the KC-03 `_cycle-index.md` row; post the #81 closure comment + the #80 corrected-closure note.
- **Deliverables:** doc-comment rewrite; tool-doc rewrite (description + REVERT_HELP); concept-brake amendment; ADR-0003 (7 sections); ADR-0002 amendment; cycle-index row (leave `Iterations = pending`, `Status = in-flight` — orchestrator owns transitions per chunk-planner v16 P-SEAL lifecycle: gate-3 → ready-for-audit; gate-4 close → audited-pending-retro; Phase 6/7 → retro-complete + Iterations to final count); #81 closure note + #80 corrected-closure note (commit subject prepends `D-TEST-0076:`).
- **Tests:** `fmt --check` + `clippy -Dwarnings` + full suite green.
- **Confidence:** 9/10. **Pause-discipline:** none.

---

## §8 — Tests summary

> Baseline (VERIFIED via `cargo test --lib collapse_abandon -- --list` → **30** at HEAD `9d0e46b`; total lib **244**): 30 existing `collapse_abandon` lib tests pass on `dev` (KC-02's 30 landed). KC-03 adds NO test file (inline-only in `context.rs`).

**Per-Tier breakdown (chunk-planner v22 P1 + v30 P-plan-9):** binary baseline (integration `tests/`) untouched; the change is inline in `context.rs`. The contract cells promote via **grouped** assertions (one `#[test]` per coherent rule/cell-group).

| Tier | Test (grouped) | Cells / rules covered | Asserts | NEW/UPDATED |
|---|---|---|---|---|
| A (Rule 1 — tail shrunk after X) | `matrix_pinned_revert_shrinks_tail_after_target` (completion + step-summary × arity 2,3 × first/mid-result) | the post-target tail across category × arity | every node strictly after X gone; `!trunk_has_tool_result(<shrunk sibling>)`; `assert_no_400` | NEW |
| A (Rule 3 — orphan call removed by id) | `matrix_pinned_revert_removes_orphan_call_by_id` | the paired call of each shrunk result | `!trunk_has_dangling_call`; the kept call node no longer carries the shrunk call's id | NEW |
| A (Rule 2a — pinned append order) | `pinned_revert_summary_appended_after_original_content` | the kept target node render-string | render = `[n<id>] <original body> [<kind>: <summary>]` (original FIRST, summary AFTER); `[continue_after_revert]` still last | NEW |
| A (Rule 2 — abandon replaces at X) | `abandon_revert_summary_replaces_target_content` | abandon-class onto X | X's content REPLACED by the summary (F-8 already green; explicit assertion) | NEW |
| A (render nit — truthful list) | `pinned_revert_breadcrumb_names_only_shrunk_tools` | the abandoned-list on a pinned-onto-result revert | names ONLY shrunk tools (skill_help), NOT the kept target's own (memory_help) — F-7 obs #4 | NEW |
| A (render nit — single label) | `revert_breadcrumb_single_label_not_double` | the breadcrumb label | ONE of `outcome`/`reverted past`, not the `[outcome: reverted past: …]` double — F-7 obs #3 | NEW |
| A (KC-02 inversions — UPDATED ×3) | `row_pinned_revert_to_first_result_now_call_atomic` + `matrix_pinned_into_cluster_keeps_whole_cluster`(→shrinks_tail) + `matrix_pinned_last_result_and_post_cluster_no_loss`(split) | the 3 keep-whole misread cells | first-result → tail shrunk + orphan removed; into-cluster → shrink-after-X; last-result → STAY (split) | UPDATED |
| A (on-trunk gate — STAYS) | `matrix_on_trunk_pinned_cluster_kept_always` (`:3778`) + `pinned_cluster_mid_trunk_kept_verbatim` (`:3061`) | Rule 1 don't-over-shrink guard | on-trunk cluster kept verbatim (nothing after X) | EXISTING-green |
| B/C (abandon arm — STAYS, the convergence template) | all `matrix_abandon_class_*` cells (F-8) | the abandon-class shrink + replace | unchanged green (regression-lock for the convergence target) | EXISTING-green |
| C (no-400 invariant) | `matrix_no_dangling_or_orphan_after_shrink` | the exercised contract cells | `!trunk_has_dangling_call` + `!trunk_has_orphan_result` every cell (P0 F-8/F-9 / §D3.4) | NEW (or folded into Tier-A) |
| **Total NEW MUST-SHIP** | **6-9 NEW grouped + 3 UPDATED + EXISTING-green preserved** | **R1/R2/R2a/R3 + 2 nits + the STAY-green set** | | |

**Tolerance band:** `[6, 10]` NEW grouped/edge tests + 3 UPDATED (the 3 KC-02 keep-whole inversions; net test-count change from the updates = 0; one is a SPLIT that MAY add 1 fn). Allowing ≤ 3 incidental inline tests the implementer may split for clarity (e.g. separating arity-2/arity-3, or a standalone single-label assertion). **Expected `collapse_abandon` lib test delta: +6 to +10 (30 → 36-40); the 3 UPDATED tests do not change the count (the split MAY add ≤ 1).** Crate delta: same (+6 to +10; no new test file; inline in `context.rs`). Expected total lib: 244 → 250-254. **`collapse_abandon` count delta expressed for orchestrator gate-1.5 cross-check: predicted band `[36, 41]` collapse_abandon tests post-KC-03.**

**Re-run command:** `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture`

---

## §9 — Pre-chunk gate (reading list + carry-forward)

Implementer MUST read before P1:
1. This plan + the forward-scope (`docs/specs/plan/forward-scope/kc-03-revert-tail-shrink-contract.md`).
2. **P0 investigation (authoritative — the 9 DEFINITIVE findings are the load-bearing inputs)**: `docs/specs/plan/build/_p0-investigations/kc-03-revert-tail-shrink-contract-p0-investigation.md` — §2 surface map, §3 F-1..F-9 (esp. F-2 trunk-walk-already-shrinks, F-3 render-pass-re-introduces, F-4 the clause-(iii) re-introducer, F-8 the abandon-arm convergence template, F-9 why the backstop missed it), §4 fix-locus, §5 ruled-out (A/B/C), §6 forks, §7 contract cells, §8 close-gate.
3. `src/types/context.rs` — `retain_pinned_calls_on_node` (`:769`, esp. the `:839-852` clause-(iii) re-append being REMOVED + the `:854-862` `ids_to_drop` retain reused for Rule 3 + the `:743-768` doc-comment + the `:691-714` mirror); `collapse_abandon_class_cluster` (`:462`, the abandon arm `:557-690` = the convergence template, the pinned arm `:691-737`); `weave_braking_annotations` (`:929`, the `:962` PREPEND being split) + `prepend_marker_to_message` (`:12`); the test module (`:2106`) + fixtures (`parallel_fixture:3225`, `parallel_fixture_3`, `multi_call_node`) + helpers (`trunk_has_dangling_call`, `trunk_has_breadcrumb`, `trunk_has_tool_result`); the 3 KC-02 tests that invert (`row_pinned_revert_to_first_result_now_call_atomic:3286`, `matrix_pinned_into_cluster_keeps_whole_cluster:3737`, `matrix_pinned_last_result_and_post_cluster_no_loss:3876`) + the STAY-green probes (`matrix_on_trunk_pinned_cluster_kept_always:3778`, `pinned_cluster_mid_trunk_kept_verbatim:3061`) + the 6 weave echo tests (`:1605`/`:1628`/`:1637`/`:1652`/`:1702`/`:1716`).
4. `src/agent_loop/run.rs` — `apply_revert` (`:709`, the breadcrumb name-set `:832-870` — the truthful-list change; the active-pointer `:781` + reject `:752-763` UNCHANGED) + `compose_revert_breadcrumb` (`:670`, the `reverted past:` prefix) + the test harness `persistent_collapse_end_to_end_via_real_apply_revert` (`:1653`).
5. `src/agent_loop/streaming.rs:130` (`enforce_call_atomic_backstop` — UNCHANGED, the no-400 belt) + `:215-262` (render pipeline order).
6. `src/tools/revert.rs:107-109` (`RevertTool::description()` — the tool-doc to rewrite) + `src/tools/tool_help.rs:129-156` (`REVERT_HELP` — the manual to rewrite).
7. `docs/decisions/0002-…md` §D2.1 (the keep-whole being superseded) + §D2.4 (no-400, carried forward) + `0001-…md` §D1.5 (R4, relied on for breadcrumb keep-as-text) + `docs/concepts/concept-brake.md:192`.

**Carry-forward invariants:** §6 table (30 existing tests minus the 3 inverted + the on-trunk-gate probes + abandon arm + weave echo tests stay green + linear walk + no-log-mutation + active-pointer/reject + backstop unchanged + full suite green).

---

## §10 — Close criteria

**Code-aspect:**
- F1: `retain_pinned_calls_on_node` SHRINKS the post-target tail (clause (iii) re-append removed; off-trunk results dropped + paired call removed by id) — CONVERGED to the abandon arm, differing only by Rule 2; clauses (i) on-trunk-keep + (ii) call-node-reclaim preserved. `weave_braking_annotations` appends the `[kind: text]` summary AFTER the original content (Rule 2a; `[n<id>]` marker stays leading); the double-label collapsed. `apply_revert` names only the shrunk tools in the breadcrumb. `build_trunk_context` / node-model / `apply_revert` active-pointer+reject / `streaming.rs` backstop untouched; forensic log read-only.
- The 3 KC-02 keep-whole tests INVERTED to the shrink dispositions; `matrix_on_trunk_pinned_cluster_kept_always` + `pinned_cluster_mid_trunk_kept_verbatim` + all `matrix_abandon_class_*` + the 6 weave echo tests stay GREEN unchanged; 27 of 30 existing stay green; 6-10 NEW contract tests green.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green (244 lib baseline + NEW + integration) + `fmt --check` clean.
- No `build_trunk_context` / node-model change; no public API / persisted-field / migration / tool-arg-schema change (tool DESCRIPTION text only).

**Docs-aspect:**
- `retain_pinned_calls_on_node` doc-comments (`:743-768` + `:691-714`) rewritten to the shrink form.
- `RevertTool::description()` (`revert.rs`) + `tool_help.rs` REVERT_HELP rewritten to R1/R2/R2a/R3 — the doc claims ONLY behavior the code has (CLAUDE.md source-of-truth).
- `concept-brake.md` verified-header + `:192` note refreshed (the simple tail-shrink contract).
- `docs/decisions/0003-revert-tail-shrink-contract.md` Accepted with §D3.1–§D3.4 + ≥ 5 Revisit triggers.
- ADR-0002 §D2.1 supersession inline note added + ADR-0002 verified-header bumped.
- phi-core `_cycle-index.md` KC-03 row appended.
- #81 closure comment + #80 corrected-closure note posted (commit subject `D-TEST-0076:`).

**Live DISPOSITION-asserting close-gate (orchestrator-run — THE real close, per `[[feedback_render_transcript_close_gate]]`; the explicit KC-02 lesson, P0 §8):** unit-green is necessary but NOT closure. Reuse the KC-02 close-gate parameterized runner + fixtures (`/root/projects/phi/worktrees/phi-e2e/i-phi/docs/e2e-test/cycles/kc02-close-gate-d835a363/scripts/`, `IPHI_MODEL_OVERRIDE` + the KC02-RECLAIM fixture), BOTH drivers (`deepseek-chat-v3-0324` + `minimax-m2.7` / strongest-available open-source per `[[feedback_htc_cohort_strongest_open_source]]`) × the `completion` (pinned) and `failure` (abandon) categories. RENDER + READ the turn-3 `*.request.json` and ASSERT the DISPOSITION (NOT merely no-400 — that is the wrong-invariant trap KC-02 fell into):
1. The post-target sibling result (skill_help / `[n2]`-equiv) is **ABSENT** from the wire (Rule 1 — tail shrunk).
2. The orphaned call (skill_help) is **REMOVED** from the call node `[n0]` (n0 carries memory_help only) (Rule 3).
3. The pinned breadcrumb is **APPENDED** at the kept node (`[n1] <memory body> [<kind>: …] [continue_after_revert]` — original body FIRST) (Rule 2a).
4. The breadcrumb names ONLY the shrunk tool (skill_help) with a SINGLE label (render nits).
5. For the `failure` (abandon) category: the summary REPLACES X's content (Rule 2).
6. No provider 400 in any cell.

**Implementation confidence target: 9/10** (`claims-honored / claims-in-scope` ≥ 9/10).

---

## §11 — Audit plan

**audit-envelope-size:** Phase count = 4 (P0, P1, P2, P-SEAL) → mechanically Medium. **I size this LARGE (3 auditors: A code+matrix / B docs+ADR+tool-doc / C carry-forward-regression + the live disposition-asserting close-gate posture)** per the forward-scope §8 hint + the genuine cross-cutting carry-forward surface: KC-03 (1) reworks 3 render functions, (2) SUPERSEDES a prior ADR (ADR-0002 §D2.1) + inverts 3 KC-02 carry-forward tests (the most consequential being the keep-whole inversion), (3) carries the abandon-arm-unchanged + on-trunk-gate-preserved + no-400 regression posture, (4) rewrites the tool description + manual (a docs-fidelity surface a dedicated reviewer should check against the code), (5) the live 2-driver × 2-category disposition-asserting close-gate. The 3-KC02-inversion + on-trunk-gate + abandon-arm-unchanged + no-400 carry-forward posture is precisely what an Audit C covers; the tool-doc + ADR-supersession is precisely what an Audit B covers — splitting keeps each prompt ≤ 600 words. **Confirm Large at gate-1.**

### Audit A — code-correctness + matrix tests (≤ 600 words)
> Read-only on source. Plan at `…/kc-03-…/plan.md`. Verify each with file:line:
> 1. F1 Rule 1+3: `retain_pinned_calls_on_node` (`context.rs:769`) — clause (iii) re-append (former `:839-852`) REMOVED; off-trunk results are DROPPED + their paired call surgically removed by id (the `ids_to_drop` retain); clause (i) on-trunk-keep + clause (ii) call-node-reclaim PRESERVED. The pinned arm now CONVERGES to the abandon arm's shrink (only Rule 2 differs).
> 2. F1 Rule 2a: `weave_braking_annotations` (`context.rs:929`) appends the `[<kind>: <summary>]` annotation AFTER the kept node's original content (via a `prepend_marker_to_message:12` append variant); the `[n<id>]` marker stays LEADING; the `[outcome: reverted past: …]` double-label collapsed to ONE.
> 3. F1 truthful-list: `apply_revert` (`run.rs:832-870`) excludes the kept target cluster's own calls from `abandoned_tool_names` on a pinned-onto-result revert; the active-pointer (`:781`) + reject (`:752-763`) UNCHANGED.
> 4. NEW contract cells (Tier A, §8) green: tail-shrunk-after-X (Rule 1); orphan-call-removed-by-id, `!trunk_has_dangling_call` (Rule 3); append-order `[n<id>] <body> [<kind>: …]` original-FIRST (Rule 2a); truthful single-label breadcrumb.
> 5. The no-400 invariant holds across the exercised cells (`!trunk_has_dangling_call` + `!trunk_has_orphan_result`); `enforce_call_atomic_backstop` (`streaming.rs:130`) UNCHANGED.
> 6. Kernel-minimality (§3.A): blast radius = 3 render fns, all `active_node_id.is_some()`-gated; `build_trunk_context` / node-model UNCHANGED; forensic `self.messages` never mutated; no public API / persisted-field / migration / tool-arg-schema change; zero consumer leakage (grep `collapse_abandon_class_cluster|retain_pinned_calls_on_node` in src/ minus context.rs returns only `streaming.rs:237` prod + `run.rs:1757` test).
> 7. `RUSTFLAGS="-Dwarnings"` clippy clean + `fmt --check` (mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked; orchestrator closes at gate-4).
> PASS/FAIL each.

### Audit B — docs + ADR + tool-doc (≤ 600 words)
> Read-only. Verify:
> 1. `retain_pinned_calls_on_node` doc-comments (`:743-768` + `:691-714`) rewritten to the SHRINK form (keep-if-on-trunk / off-trunk → drop + remove paired call by id; pinned ADDS the summary, abandon REPLACES); the KC-02 (B) re-append bullet REPLACED.
> 2. `RevertTool::description()` (`revert.rs:107-109`) + `tool_help.rs` `REVERT_HELP` (`:129-156`) rewritten to R1/R2/R2a/R3 — every revert shrinks the tail after X; `failure`/`tangent` REPLACE X's content; `completion`/`step-summary` ADD the summary AFTER X's content; the orphaned call removed. The doc claims ONLY behavior the code has (no overstated "drop the tail" where the old code re-appended).
> 3. `concept-brake.md` verified-header refreshed + `:192` note updated to the simple tail-shrink contract (cite ADR-0003, SUPERSEDES ADR-0002 §D2.1).
> 4. `docs/decisions/0003-revert-tail-shrink-contract.md` exists, Status Accepted, 7 sections (`## Forks`/`## Context`/`## Sub-decisions`/`## Cross-references`/`## Consequences`/`## Revisit triggers`/`## Verification`), §D3.1–§D3.4, ≥ 5 Revisit triggers, Verification commands present.
> 5. ADR-0003 §D3.1 states it SUPERSEDES ADR-0002 §D2.1 (the result-node keep-whole re-append → the simple tail-shrink); §D3.1 documents R1/R2/R2a/R3 + the pinned-arm-converges-to-abandon-arm decision + the on-trunk-keep gate as Rule 1's don't-over-shrink guard (intended, not a regression); Cross-references cite #81/D-TEST-0076 + #80 + ADR-0002 §D2.1 (superseded) + §D2.4 + ADR-0001 §D1.5 + forward-scope + P0 §3/§4/§6.
> 6. ADR-0002 §D2.1 carries the `> Superseded-in-part by ADR-0003 §D3.1` inline note + ADR-0002 verified-header bumped.
> 7. phi-core `_cycle-index.md` KC-03 row appended: `Iterations = pending`, `Status = in-flight` (orchestrator owns transitions); plan archive at `…/kc-03-…/plan.md` exists with cycle hex.
> 8. No K8s ledger (N/A library); no migration; no phi-core CI-guard scripts referenced.
> PASS/FAIL each.

### Audit C — carry-forward regression posture + close-gate (≤ 600 words)
> Read-only. The fix INVERTS 3 KC-02 carry-forward tests + relies on the on-trunk-keep gate + the abandon-arm-unchanged convergence template; this audit verifies the regression posture. Verify each with file:line:
> 1. **The 3 KC-02 keep-whole inversions are the ONLY inversions + are INTENDED (not collateral)**: `row_pinned_revert_to_first_result_now_call_atomic` (`:3286`) UPDATED — pcall_b now SHRUNK (`!trunk_has_tool_result("pcall_b")`) + orphan call removed (`!trunk_has_dangling_call`), pcall_a kept (on-trunk); `matrix_pinned_into_cluster_keeps_whole_cluster` (`:3737`) UPDATED — first/mid-result siblings shrunk-after-X (likely renamed `…_shrinks_tail`); `matrix_pinned_last_result_and_post_cluster_no_loss` (`:3876`) SPLIT — the on-trunk last-result sub-cases STAY, only the re-append-past-tip sub-case inverts.
> 2. **On-trunk-keep gate (load-bearing, Rule 1 don't-over-shrink)**: `matrix_on_trunk_pinned_cluster_kept_always` (`:3778`) + `pinned_cluster_mid_trunk_kept_verbatim` (`:3061`) stay GREEN UNCHANGED — a pinned cluster fully on-trunk (nothing after X) is kept verbatim; confirm the bodies were NOT modified.
> 3. **Abandon-class arm unchanged (the convergence template, P0 F-8)**: all `matrix_abandon_class_*` cells stay GREEN unchanged — the pinned arm converges TO this; confirm no abandon-path code change.
> 4. **Weave `[n<id>]`-echo invariant**: the 6 `weave_braking_annotations_*` echo tests (`:1605`/`:1628`/`:1637`/`:1652`/`:1702`/`:1716`) stay GREEN — the append-after-original split keeps the marker leading.
> 5. No-400 across the exercised cells: `!trunk_has_dangling_call` + `!trunk_has_orphan_result` every cell (P0 F-8/F-9); `enforce_call_atomic_backstop` UNCHANGED. Full phi-core suite green (244 lib baseline minus 3 updated + NEW + integration suites). Mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked; orchestrator closes at gate-4.
> 6. Live DISPOSITION-asserting close-gate posture: the §10 close criteria name ALL of — tail shrunk (Rule 1), orphan call removed (Rule 3), append-order original-FIRST (Rule 2a), abandon replaces (Rule 2), truthful single-label, no-400 — across both drivers × completion+failure (orchestrator-run; auditor confirms the criteria are STATED + assert the DISPOSITION, not merely no-400 — the explicit KC-02 wrong-invariant lesson).
> PASS/FAIL each.

---

## §12 — Verification recipe (copy-paste)

```bash
# Build + lint (host cargo, single crate, -j 4, -Dwarnings)
/root/rust-env/cargo/bin/cargo build -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# Targeted regression (the re-authored contract matrix + the 3 inverted KC-02 tests + the STAY-green probes)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture

# On-trunk-keep gate + mid-trunk-verbatim (must stay green — Rule 1 don't-over-shrink)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib matrix_on_trunk_pinned_cluster_kept_always pinned_cluster_mid_trunk_kept_verbatim

# Abandon-class arm (the convergence template — must stay green) + weave echo tests (marker-leading invariant)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib matrix_abandon_class weave_braking_annotations

# Full crate suite (lib + integration)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# Confirm render-pass-only: no public API / persisted-field change; blast radius = 3 render fns
git -C /root/projects/phi/phi-core diff dev --stat -- src/

# Consumer-leakage grep (must return only streaming.rs prod + run.rs test)
grep -rn 'collapse_abandon_class_cluster\|retain_pinned_calls_on_node' /root/projects/phi/phi-core/src/ | grep -v context.rs

# Gate-5 cargo-clean (orchestrator, at cycle close)
/root/rust-env/cargo/bin/cargo clean --manifest-path /root/projects/phi/phi-core/Cargo.toml
```

**Blocking unresolved: NONE** (P0 §0 / §9 "Open: NONE"). The live DISPOSITION-asserting full-grid close-gate (§10) is orchestrator-run (live provider round-trip; not bootable at phi-core unit level) and is THE real close per `[[feedback_render_transcript_close_gate]]` — asserting the disposition, not no-400 (the KC-02 lesson).

---

## Forks for orchestrator

> ⚠️ **The 5 forks are EVIDENCE-FORCED by reproduction (P0 §3/§4/§6)** — the user lock at gate-1 is a formality given the reproduced disproofs (P0 §5 ruled out the 3 non-viable approaches with evidence). **F1 is the product-semantic / load-bearing fork** (the on-wire disposition is user-perceivable); it is investigation-forced to render-pass-shrink (NOT the "more fundamental" trunk-walk change the forward-scope §10 feared — REFUTED by P0 F-2/F-3). F2–F5 are TECHNICAL at investigation-rec. **NO USER-DIVERGENT lock anticipated** (the evidence settles every fork); this is NOT a P-orch-8 skip-eligible cycle only because forward-scope §11 forces `approval=yes` (S2 core-tool correctness + supersedes a prior ADR) — the gate-1 lock confirms the investigation-forced resolutions. Cross-cycle divergence pattern: phi-core kernel lane (KC-01 planner-rec-clean; KC-02 was USER-DIVERGENT (a)→(B) — KC-03 CORRECTS that (B) lock to the user's simple contract #81, so KC-03 itself is the contract the user locked 2026-06-09, presented investigation-forced).

### F1 — tail-shrink locus + the simple contract (load-bearing, product-semantic; investigation-forced)

| Option | User-visible | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F1.render-pass-shrink — the pinned arm converges to the abandon arm; R1/R2/R2a/R3** (investigation-forced; the user's locked simple contract #81) | **User-visible:** the model reads a correctly-shrunk, correctly-ordered revert disposition turn-after-turn — every node after X gone (R1), summary appended after X's content (R2/2a), orphan call removed (R3), single truthful breadcrumb; context actually shrinks | render-pass-only (P0 F-2/F-3 — shallower than feared); a net production SIMPLIFICATION (remove the clause-(iii) re-append; converge to the proven abandon arm, F-8); zero node-model/API/migration change; closes #81 + corrects #80 | **Product trajectory:** the durable simple contract for every consumer + the MCP/combined-agent cluster (#67/#64/#68); a future true-sibling node model is a deferrable Revisit trigger, NOT needed | **LOCKED (investigation-forced)** |
| F1.trunk-walk-shrink (the forward-scope §10 / #81 "more fundamental" hypothesis) | (same on-wire result IF correct) | — | **Product trajectory:** REFUTED by P0 F-2 (the trunk walk ALREADY shrinks; the render pass re-introduces) — touching it re-touches a CORRECT layer + endangers the abandon path; non-viable (P0 §5.A) | NOT chosen (refuted) |

### F2 — node-model adequacy
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F2.linear-chain-suffices** (investigation-forced) | P0 F-2 / §5.C: the linear chain already shrinks the tail + the `(node_id, tool_call_id)` id join removes the orphan; zero migration | — | **REC / lock** |
| F2.true-sibling-node-model | "more correct" (true siblings) | UNNECESSARY for #81 (P0 §5.C); large kernel change (node stamping, migration); over-scoped | NOT chosen (deferrable Revisit trigger) |

### F3 — append-order + label mechanics
**TECHNICAL FORK** (no user-visible delta beyond F1 — these are the F1 render nits).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F3.append-after-original + single-label + truthful-list** (investigation-forced) | Rule 2a + the 2 render nits; marker `[n<id>]` stays leading (echo-test-safe); render-correctness only | the weave split must honor the `[n<id>]`-echo tests (`:1605+`) — confirmed at P0 | **REC / lock** |

### F4 — KC-01/KC-02 carry-forward
**TECHNICAL FORK** (no user-visible delta — carry-forward test disposition).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F4.invert-the-3-KC02-keep-whole-tests + keep-the-stay-green-set + backstop-stays** (investigation-forced) | grep-verified exactly 3 keep-whole tests encode the misread (invert); abandon arm + on-trunk gate + M=1 + backstop STAY (P0 F-4/F-8/F-9); `retain_pinned_calls_on_node` reworked (clause iii removed), not deleted | one inverted test is a SPLIT (last-result sub-cases stay) — the implementer splits it at P1 | **REC / lock** |

### F5 — KC-04 split decision
**TECHNICAL FORK** (no user-visible delta — absorb-vs-split routing).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F5.absorb** (investigation-forced) | one render-pass rework + matrix re-author + tool-doc + ADR-0003; no separable large piece (P0 §6/F5) | the deeper true-sibling node model stays a deferrable (Revisit trigger) | **REC / lock** |
| F5.split (open KC-04 for a node-model change) | "fixes the root cause" | UNNECESSARY for #81 (P0 F-2 — the linear chain closes the contract); over-scoped | NOT chosen |
