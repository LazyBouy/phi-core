<!-- Last verified: 2026-06-09 by Claude Code -->

# P0 investigation — KC-03 (phi-core kernel lane)

> Closes GitHub **#81 / D-TEST-0076 (S2)**, subsumes #80. Investigation-first (user-directed):
> establish, by READING + REPRODUCING, where the post-target tail must actually be shrunk before any
> plan exists. Throwaway MockProvider-free repros were run against the VERBATIM production render
> pipeline and then reverted (tree is clean; no instrumentation left in source).

## §0 — Verdict summary

- **Findings: 9 definitive, 0 unresolved.** Every claim below carries a reproduced result or a `file:line`.
- **Fix-locus: render-pass-only, CONSUMER-side-of-the-kernel-render-pass — entirely inside
  `collapse_abandon_class_cluster`'s PINNED arm + `weave_braking_annotations` (breadcrumb order) +
  `apply_revert`'s breadcrumb name-set. The trunk derivation (`build_trunk_context`) is CORRECT and
  must NOT change. The revert APPLICATION (`apply_revert`) state-change is CORRECT and must NOT change.
  Zero node-model change, zero persisted-field change, zero public-API change, zero migration.** This is
  a genuine general revert-correctness fix in the kernel render path with zero consumer-specific logic —
  passes the kernel carve-out test cleanly.
- **The fix is NOT "more fundamental than the render pass."** The forward-scope/§10 risk and the issue's
  "likely a more fundamental change than the render-pass patch — the tail must actually be shrunk (the
  trunk walk / revert application)" hypothesis is **REFUTED by repro** (F-2): the trunk walk ALREADY
  shrinks the tail; the render pass then RE-INTRODUCES it. The fix is to STOP the re-introduction, which
  is shallower than feared. This is the load-bearing correction P0 was asked to settle.
- **Forks surfaced for the planner: 5** (F1–F5; F1 resolved-to-evidence, F2 resolved-to-evidence; both
  still presented as user-lockable but with the answer the evidence forces).
- **Non-viable approaches ruled out: 3** (with reproduced disproof each).
- **Unresolved / needs-live-repro: NONE at unit level.** One recommended orchestrator-run close-gate
  (§8) — the disposition-asserting live grid — settles end-to-end per `[[feedback_render_transcript_close_gate]]`.

---

## §1 — Questions that gate planning

1. Does `revert_to_state` set state such that the post-target tail SHOULD be excluded, or is the
   abandoned tail never marked? (Where does the tail get dropped — application or render?)
2. Does `build_trunk_context` actually truncate at the revert target on the linear-stamped
   n0→n1→n2 chain, or does it include nodes after the target?
3. If the trunk truncates correctly, WHAT re-introduces the parallel-sibling tail (n2/skill_help) +
   keeps its call in n0 on the pinned (`completion`) path?
4. Is the breadcrumb prepended or appended, and where is that decided?
5. Does the abandon-class (`failure`/`tangent`) path ALSO leak the tail, or is the defect isolated to
   the pinned class?
6. Does the simple contract require a true-sibling node model, or does the existing linear chain +
   `(node_id, tool_call_id)` join suffice for surgical call-removal?
7. Which KC-01/KC-02 tests flip under the contract; is `retain_pinned_calls_on_node` removed or
   reworked; does `enforce_call_atomic_backstop` stay as the belt?
8. Is any kernel public-API / persisted-field / migration / tool-arg-schema change required?

---

## §2 — Current-surface map (the revert pipeline)

**Render-path call order** (revert mode, `agent_loop/streaming.rs:215–262`):

```
if context.active_node_id.is_some() {
    1. raw_trunk = context.build_trunk_context()                         // streaming.rs:232  (context.rs:1111)
    2. collapsed = context.collapse_abandon_class_cluster(raw, policy, t) // streaming.rs:237  (context.rs:462)
    3. trunk     = decay_tags_by_policy(collapsed, policy, t)            // streaming.rs:243  (context.rs:303)
    4. woven     = weave_braking_annotations(trunk)                      // streaming.rs:245  (context.rs:929)
    5. injected  = inject_continue_after_revert(woven, t, window)        // streaming.rs:251  (context.rs:1010)
    6. enforce_call_atomic_backstop(injected)                           // streaming.rs:259  (streaming.rs:130)
} else { context.build_working_context() }                              // streaming.rs:261
```

**Between-turn application** — `apply_revert` (`agent_loop/run.rs:709`):
- `run.rs:735–748` resolves `target_idx` by `node_id` over `messages`.
- `run.rs:752–763` rejects if any strictly-after message is `Message::User` (D6 conservative rule).
- `run.rs:768–774` collects `abandoned_node_ids` = every Llm strictly after target (used in the breadcrumb).
- `run.rs:781` **`context.active_node_id = Some(request.target)`** — the load-bearing state change. This
  is what `build_trunk_context` later walks from. **CORRECT — no change needed (F-1, F-3).**
- `run.rs:793–809` drops off-trunk `inrun_context` entries (linear-mode pre-Phase-4 leftover; benign).
- `run.rs:832–870` composes `abandoned_tool_names` — seeds the TARGET cluster's own tool-call name(s)
  FIRST, then the strictly-after span. **This is the "untruthful abandoned-list" locus** — for a pinned
  revert onto a result node it adds BOTH the target cluster's calls (memory_help, KEPT) and the
  strictly-after span (skill_help). Names a KEPT tool ⇒ issue observation #4 (F-7).
- `run.rs:898–909` attaches the breadcrumb as a `NodeTag` on the target node.

**`build_trunk_context`** (`context.rs:1111`) — walks `parent_id` from `active_node_id` to root
(`context.rs:1136–1147`), reverses to chronological, merges `user_context`. Off-trunk branches are
simply absent. **This is the tail-shrink locus and it WORKS (F-2).**

**`collapse_abandon_class_cluster`** (`context.rs:462`) — persistent scan over every tagged trunk node.
- Abandon-class arm (`context.rs:557–690`): call-node → clear content (or per-call retain if live
  sibling); result-node → find parent call, clear it, move tags, **REMOVE the result node**. Correct
  per contract (F-5).
- **Pinned arm (`context.rs:691–737`)** → delegates to `retain_pinned_calls_on_node`.

**`retain_pinned_calls_on_node`** (`context.rs:769`) — THE DEFECT LOCUS (F-4). For each call on the
pinned node: (i) result on-trunk → keep; (ii) off-trunk + tag-on-call-node → drop call; **(iii)
off-trunk + tag-on-RESULT-node → RE-APPEND the off-trunk sibling result by call-node membership against
`self.messages` (`context.rs:842–852, 866–868`)**. Clause (iii) is exactly the code that resurrects the
shrunk tail.

**`weave_braking_annotations`** (`context.rs:929`) — builds `marker = "[nN] [kind: text]…"` then
`prepend_marker_to_message(&mut lm.message, &marker)` (`context.rs:962`). **PREPEND is the Rule-2a-order
locus (F-6).** The kind label comes from `tag.kind.rendered_label()` (`context.rs:959`).

**`inject_continue_after_revert`** (`context.rs:1010`) — inserts the `[continue_after_revert]` synthetic
`Message::User` after the tip (`context.rs:1083`). Order-correct relative to Rule 2a (always last);
unaffected by the fix except that the breadcrumb it points to moves.

**`enforce_call_atomic_backstop`** (`streaming.rs:130`) — drops on-trunk calls whose result is absent +
empty assistant nodes. Belt-and-suspenders; STAYS (F-8). Note it could NOT have caught this defect:
clause (iii) re-appends the result, so the call is NOT an orphan from the backstop's view (F-9).

---

## §3 — Findings (each DEFINITIVE; repro = a `#[test]` run against the production pipeline, then reverted)

Repro fixture: `parallel_fixture(active, target, kind)` (`context.rs:3257`) — `n0` user → `n1`
`[pcall_a, pcall_b]` assistant → `n2` result_a (parent n1) → `n3` result_b (parent n2, LINEAR chain).
This is the exact #81 shape (n0-cluster-call / n1-first-result / n2-second-result, renamed n1/n2/n3).
Driven through `render_with_backstop` = verbatim production order (`context.rs:3670`).

- **F-1 (DEFINITIVE).** `apply_revert` sets `context.active_node_id = Some(target)` and nothing else
  shrinks the message log. The tail-drop is delegated entirely to the render-time parent walk.
  *Evidence:* `run.rs:781` is the only state mutation that scopes the trunk; `messages` is never
  truncated. Confirmed by reading + by F-2's raw-trunk repro showing the walk does the truncation.

- **F-2 (DEFINITIVE — the load-bearing fact).** `build_trunk_context` from `active=n2` (revert onto
  result_a) **ALREADY shrinks the tail**: the raw trunk = `[n0 user, n1 assistant, n2 result_a]` —
  **n3 (result_b, the tail) is ABSENT.** *Reproduced output:* `raw build_trunk_context: result_a=true
  result_b(TAIL)=false`; node order `n0 user → n1 assistant → n2 tool_result(pcall_a)`. The trunk
  derivation is CORRECT. **Rule 1 is satisfied at the trunk-walk layer.**

- **F-3 (DEFINITIVE — refutes the "more fundamental" hypothesis).** The FULL production pipeline
  RE-INTRODUCES the shrunk tail on the pinned path. *Reproduced output (completion/Outcome onto n2):*
  `FULL pipeline: result_a=true result_b(TAIL should be GONE)=true dangling=false orphan=false`. The
  tail that F-2 proved absent at step 1 is back by step 6. The re-introduction happens in step 2,
  `retain_pinned_calls_on_node` clause (iii). **The fix-locus is the render pass, NOT the trunk walk /
  revert application** — shallower than the forward-scope §10 risk + issue's "fix scope" feared.

- **F-4 (DEFINITIVE).** The re-introducer is `retain_pinned_calls_on_node` clause (iii)
  (`context.rs:813–852`): off-trunk sibling result whose `tool_call_id` matches a call on the pinned
  node is re-fetched from `self.messages` and `trunk.insert`-ed back (`context.rs:866–868`). This is the
  KC-02 (B) "keep-whole-cluster-on-result-node" heuristic — the misread the issue names. *Evidence:*
  the only code path between raw-trunk and final output that ADDS a `ToolResult` to the trunk is this
  `results_to_append` insert; the abandon-class arm only REMOVES.

- **F-5 (DEFINITIVE).** The orphan call (skill_help / pcall_b) is NOT removed from n1, *because* its
  result was re-appended. *Reproduced output:* `call_b in n1 (TAIL call, should be REMOVED)=true`. Rule
  3 is violated as a direct consequence of F-4: the backstop sees a paired call (F-9), so it leaves it.

- **F-6 (DEFINITIVE).** The pinned-class summary is PREPENDED, not appended. *Reproduced output at the
  kept target node:* `APPEND-ORDER at result_a: "[n2] [outcome: reverted past: parallel cluster] Wrote
  356 bytes"` — breadcrumb BEFORE the original "Wrote 356 bytes" body. Matches the live wire
  (`…turn-3.request.json` idx4: `[n1] [outcome: reverted past: …] [memory … ShortTerm …]`). Locus:
  `weave_braking_annotations` `prepend_marker_to_message` (`context.rs:962`). **Rule 2a violated.**

- **F-7 (DEFINITIVE).** The abandoned-list names a KEPT tool. `apply_revert`'s `abandoned_tool_names`
  seeds the TARGET cluster's own call names first (`run.rs:860–863`) then the strictly-after span
  (`run.rs:864–868`). For a pinned revert onto result_a, that yields `(memory_help, skill_help
  abandoned)` while memory_help's result is KEPT. Matches the live wire breadcrumb exactly. Locus:
  `run.rs:832–870` (the name source-set) + the `[outcome: reverted past: …]` double-label at
  `context.rs:959` × the `compose_revert_breadcrumb` prefix. **Issue observations #3 (double-label) +
  #4 (untruthful list).**

- **F-8 (DEFINITIVE).** The abandon-class (`failure`) path is CORRECT for the same n2 target and does
  NOT leak the tail. *Reproduced output (Lesson onto n2):* `ABANDON FULL: result_a=false
  result_b(TAIL)=false call_a_in_n1=false call_b_in_n1=false dangling=false orphan=false` — whole
  cluster collapses to the breadcrumb (Rule 1 + Rule 2-replace + Rule 3 all satisfied). **The defect is
  ISOLATED to the pinned-class arm.** The abandon-class arm is the working template the pinned arm must
  converge toward (shrink the tail; surgically drop the orphan call).

- **F-9 (DEFINITIVE).** `enforce_call_atomic_backstop` could not have caught this. Because clause (iii)
  re-appends result_b, pcall_b is NOT an orphan from the backstop's perspective (`streaming.rs:155–157`
  retains calls whose result is in `result_ids`). *Reproduced output:* `dangling=false orphan=false` on
  the leaky pinned render. **This is exactly why KC-02's no-400 close-gate falsely read PASS** — it
  validated internal pairing, not the disposition. (Confirms the issue's "validated the wrong invariant.")

---

## §4 — Fix-locus determination (F1)

| Change | kernel|consumer | carve-out-test rationale | blast radius |
|---|---|---|---|
| Stop re-appending the off-trunk sibling on the pinned arm (rework clause (iii) of `retain_pinned_calls_on_node` so a pinned revert ONTO/INTO a cluster SHRINKS the post-target tail instead of re-gathering it) | **kernel render-pass** | General revert-correctness: the simple R1 contract ("shrink the tail, always, any category") is a kernel invariant with zero consumer logic. The current behaviour is a kernel bug, not an i-phi requirement. | `collapse_abandon_class_cluster` pinned arm only; abandon-class arm untouched (F-8 proves it's already correct). Verify `sub_agent.rs`/`parallel.rs`/`evaluation.rs` callers unaffected — they do not call `collapse_*`/`retain_pinned_*` (render-path-only; `active_node_id.is_some()` gated). |
| Make the pinned arm surgically remove the orphan call by id (apply the abandon-class arm's per-call drop to the pinned arm) | **kernel render-pass** | Rule 3 atomicity by id — uses the existing `(node_id, tool_call_id)` join (F2 proves sufficient). | Same arm; reuses `call_node_has_live_sibling`-style id matching already present (`context.rs:876`). |
| Append-after-original for pinned-class (`weave_braking_annotations` order) | **kernel render-pass** | Rule 2a — general ordering correctness; no consumer logic. | `weave_braking_annotations` `prepend_marker_to_message` → an append variant for kept-content nodes; the `[n<id>]` marker can stay leading (it is a node label, not the summary) while the `[kind: text]` tag moves AFTER content. Confirm the `[n<id>]`-echo tests (`weave_braking_annotations_*`, `context.rs:1605+`) still hold. |
| Collapse `[outcome: reverted past: …]` double-label to one; truthful abandoned-list | **kernel render-pass + apply_revert** | Render correctness — one label, name only what was shrunk. | `compose_revert_breadcrumb` (double-label) + `apply_revert` name source-set (`run.rs:832–870`) — for a pinned revert onto a result node, the KEPT target cluster's own calls must be EXCLUDED from "abandoned" (they are kept). |
| `build_trunk_context` | **NO CHANGE** | F-2 proves correct. | none |
| `apply_revert` active-pointer + reject rules | **NO CHANGE** (except breadcrumb name-set) | F-1/F-3 prove the state-change is correct. | none |
| Node model / persisted fields / migration / tool-arg schema | **NO CHANGE** | F2 (linear chain suffices) + the tool description already promises tail-drop (it just isn't honored). | none — kernel-minimal; backward-compatible. |

**Net:** a render-pass-only correctness fix in `collapse_abandon_class_cluster` (pinned arm) +
`weave_braking_annotations` (order) + `apply_revert` (breadcrumb name-set/label). No node-model,
public-API, persisted-field, migration, or tool-arg change. Kernel carve-out test PASSES (general
revert correctness, zero consumer leakage). `self.messages` (forensic log) stays immutable.

---

## §5 — Non-viable approaches ruled out (with reproduced evidence)

- **A. "Move the tail-shrink into the revert application / trunk walk" (the issue's leading hypothesis).**
  *Tempting:* the issue + forward-scope §10 both say "the tail must actually be shrunk (the trunk walk /
  revert application), not annotated." *Disproof:* F-2 reproduced `raw build_trunk_context:
  result_b(TAIL)=false` — the walk ALREADY drops the tail. Changing the walk/application is unnecessary
  and would risk the abandon-class path (F-8 proves it's correct via the SAME walk). The re-introduction
  is downstream, in the render pass. **Non-viable: it fixes a layer that isn't broken.**

- **B. "Just filter the tail in a new render-pass post-step / extend the backstop to drop the
  re-appended sibling."** *Tempting:* leave clause (iii) and add a later filter. *Disproof:* clause
  (iii) re-appends the result AND keeps the call, so after it runs the cluster is internally PAIRED
  (F-9: `dangling=false orphan=false`). A backstop-style orphan filter has nothing to grab — both call
  and result are present and matched. You would have to re-derive "is this node after the revert target"
  in a second pass, duplicating the trunk-walk knowledge the FIRST pass (`build_trunk_context`) already
  had and clause (iii) threw away. **Non-viable: fights the re-append instead of removing it; the clean
  fix is to not re-append.**

- **C. "Introduce a true-sibling node model so n1/n2 are siblings of n0 (not a linear chain), so the
  walk naturally excludes both siblings."** *Tempting:* the deferred KC-02 Revisit trigger; feels
  "more correct." *Disproof:* F2 reproduced that the existing linear chain already excludes the tail
  (n3 absent from raw trunk) AND that `n1` carries both calls joinable by id (`n1 carries calls
  ["pcall_a","pcall_b"]`) with `n3.tool_call_id == pcall_b` — so surgical removal of the orphan call is
  fully expressible on the linear chain via the existing `(node_id, tool_call_id)` join. A sibling node
  model is a large kernel change (node stamping, persisted parent links, migration) for zero contract
  benefit. **Non-viable for KC-03: violates kernel-minimality; the linear chain suffices.** (This is
  the F2 answer: lean (a) confirmed.)

---

## §6 — Design forks surfaced (for the planner — NOT locked)

- **F1 — Tail-shrink locus.** Options: (a) shrink in the revert application / trunk derivation; (b)
  shrink in the render pass. **Evidence forces (b):** F-2 proves the trunk walk already shrinks; F-3
  proves the render pass (pinned arm) re-introduces. Trade-off: (b) is a localized rework of
  `retain_pinned_calls_on_node` clause (iii) + per-call orphan drop, with the abandon-class arm as the
  proven template (F-8); (a) would re-touch a correct layer and endanger the abandon path. The planner
  presents (b); the user lock is a formality given the evidence.

- **F2 — Node-model adequacy.** Options: (a) linear chain suffices; (b) true-sibling node model.
  **Evidence forces (a):** F2 repro — tail already excluded by the walk + orphan call removable by the
  existing id join. Trade-off: (a) is kernel-minimal, zero migration; (b) is a large change deferred to
  a future chunk (the KC-02 Revisit trigger) with no KC-03 benefit. Lean (a) confirmed.

- **F3 — Append-order + label mechanics.** Options for the breadcrumb composer
  (`weave_braking_annotations` + `compose_revert_breadcrumb` + `apply_revert` name-set): (i)
  append-after-original for pinned-class while keeping `[n<id>]` leading (it's a node label, not the
  summary); (ii) collapse `[outcome: reverted past: …]` → single label; (iii) make the abandoned-list
  name only what was shrunk (exclude the kept target cluster's own calls on a pinned-onto-result revert).
  Trade-off the investigation surfaced: the `[n<id>]` marker and the `[kind: text]` tag are woven in one
  pass today (`context.rs:946–962`); F3 must split them — marker stays leading, tag moves after content
  — without breaking the `[n<id>]`-echo tests (`context.rs:1605+`) the model relies on for valid `step=`.

- **F4 — KC-01/KC-02 carry-forward.** Tests that ENCODE the wrong spec and must
  invert/delete (DEFINITIVE list from grep, all currently green on `dev`):
  - `row_pinned_revert_to_first_result_now_call_atomic` (`context.rs:3285`) — asserts `(B) the off-trunk
    call_b sibling is re-appended (whole cluster kept)` (`context.rs:3304–3307`). **INVERTS:** call_b's
    result + call must be SHRUNK/removed.
  - `matrix_pinned_into_cluster_keeps_whole_cluster` (`context.rs:3737`) — asserts whole-cluster-kept for
    first-result a2/a3 + mid-result a3 (`context.rs:3742–3769`). **INVERTS:** post-target siblings shrunk.
  - `matrix_pinned_last_result_and_post_cluster_no_loss` (`context.rs:3876`) re-append assertions at
    `context.rs:3932, 3997`. **REVIEW:** the "all results on-trunk" sub-cases (revert onto the LAST
    result) STAY correct (nothing after the target → nothing to shrink); only the re-append-past-tip
    sub-cases invert. The planner must split these by target position.
  - `matrix_on_trunk_pinned_cluster_kept_always` (`context.rs:3778`) — revert onto the LAST result (all
    results on-trunk). **STAYS GREEN** under the contract (Rule 1: no node after X ⇒ nothing shrinks;
    the kept cluster is the trunk itself). This is the load-bearing "don't over-shrink" guard.
  - `retain_pinned_calls_on_node`: **reworked, not deleted** — clause (i) on-trunk-keep STAYS; clause
    (ii) tag-on-call-node DROP stays (it already shrinks); clause (iii) re-append is the bug → replace
    with tail-shrink + per-call orphan drop.
  - `enforce_call_atomic_backstop`: **STAYS as the belt** (F-8/F-9) — after the pinned arm shrinks the
    tail + drops the orphan call, the backstop is the locus-independent guarantee no orphan survives.
  - KC-01 abandon-class matrix cells + the M=1 SAFE-regression rows: **STAY GREEN** (F-8 — abandon path
    untouched; M=1 path byte-identical).

- **F5 — Split.** Evidence says **DO NOT SPLIT.** No separable large piece surfaced: F2 rules out the
  node-model change (the only candidate for a KC-04 carve-out). The fix is one render-pass rework + the
  matrix re-author + tool-doc + ADR-0003 — a single coherent Large chunk. (Planner confirms at gate-1.)

---

## §7 — Repro assets (for the planner/implementer — the contract cells)

All reuse the existing in-module helpers (`parallel_fixture` `context.rs:3257`, `parallel_fixture_3`
`context.rs:3607`, `render_with_backstop` `context.rs:3670`, `trunk_has_tool_result`,
`trunk_has_dangling_call`, `trunk_has_orphan_result`, `assert_no_400`). The throwaway P0 repros
(`kc03_p0_repro_*`, run + reverted) are the SEED for these permanent cells:

| Cell | Asserts | promote? |
|---|---|---|
| pinned (completion) onto first-result n2, arity 2 | tail (result_b + pcall_b call) GONE; result_a + pcall_a KEPT; no-400 | **promote-to-regression** (the #81 defect cell) |
| pinned (step-summary) onto first-result, arity 2 + 3 | same per arity | promote |
| pinned onto mid-result n3, arity 3 | result_c (past tip) GONE + pcall_c call removed; a,b kept | promote (inverts `matrix_pinned_into_cluster_keeps_whole_cluster` mid-result a3) |
| pinned onto LAST result (all on-trunk) | whole cluster kept verbatim — Rule 1 over-shrink guard | promote (this STAYS green; the don't-over-shrink guard) |
| abandon (failure/tangent) onto first-result, arity 2 + 3 | whole cluster → breadcrumb (already correct, F-8) | promote (regression-lock the working path) |
| append-order (pinned, kept content) | rendered kept node = `[n2] <original body> [outcome: …]` (original FIRST) | promote (Rule 2a) |
| truthful abandoned-list (pinned onto result) | breadcrumb names ONLY shrunk tools (skill_help), NOT the kept memory_help | promote (issue obs #4) |
| single-label | breadcrumb carries ONE of `outcome`/`reverted past`, not both | promote (issue obs #3) |
| M=1 SAFE-regression | byte-identical to pre-KC-03 | keep existing |

Throwaway P0 repro code lived under `phi-core/target/` (gitignored) and as an appended test reverted via
backup; **tree is clean** (`git status` empty). Nothing to re-run — the planner authors the permanent
cells above.

## §8 — Recommended close-gate (live, disposition-asserting — orchestrator-run)

Per `[[feedback_render_transcript_close_gate]]`: unit-green is necessary but NOT closure. Reuse the
KC-02 parameterized runner + fixtures at
`/root/projects/phi/worktrees/phi-e2e/i-phi/docs/e2e-test/cycles/kc02-close-gate-d835a363/scripts/`
(`IPHI_MODEL_OVERRIDE`, KC02-RECLAIM fixture), both drivers (deepseek + minimax / strongest-open-source
per `[[feedback_htc_cohort_strongest_open_source]]`). RENDER + READ the turn-3 `*.request.json` and
ASSERT the DISPOSITION (not no-400):
1. The post-target sibling result (skill_help, idx-equiv of `[n2]`) is **ABSENT** from the wire.
2. The orphaned call (skill_help) is **REMOVED** from the call node `[n0]` (n0 carries memory_help only).
3. The pinned breadcrumb is **APPENDED** at the kept node (`[n1] <memory body> [outcome: …]
   [continue_after_revert]` — original body FIRST).
4. The breadcrumb names ONLY the shrunk tool (skill_help), with a SINGLE label.
5. No provider 400.
Run both the `completion` (pinned, kept-content) and `failure` (abandon, replace-content) categories.

## §9 — Open / unresolved

**NONE at unit level.** All 8 gating questions are answered DEFINITIVELY by reproduced result + file:line
(§3). The only remaining evidence is the live disposition close-gate (§8), which is the chunk's
acceptance gate (orchestrator-run), not an unresolved investigation question — the fix mechanism is fully
settled. No `needs_live_repro` blocker on planning.
