<!-- Last verified: 2026-06-09 by Claude Code -->
<!-- KC-02 cycle plan — ARCHIVED at cycle hex d835a363 (2026-06-09). iter-2 — F1 USER-LOCKED at the BROADER (B) fix 2026-06-08 (USER-DIVERGENT from the iter-1 (a)-only draft); F2/F3 at investigation-rec. project=phi-core. -->

# KC-02 — parallel-cluster revert keep/drop matrix: the F1(B) reclaim-on-call-node + keep-cluster-on-result-node fix

> **Second phi-core kernel chunk.** Closes GitHub #80 / D-TEST-0075 (S2 runtime-correctness: parallel-cluster pinned-revert silent loss of gathered work). **Characterization is DONE** — the exhaustive 32-cell `category × revert-target × arity` matrix (P0 §3, 9 DEFINITIVE findings) PLUS the second P0 pass that DESIGNED + VALIDATED the broader fix (P0 §10: throwaway (B) impl + 6 throwaway tests + the full 37-cell re-run, all reverted, tree clean). The load-bearing fork **F1 = (B) reclaim-on-call-node + keep-whole-cluster-on-result-node is USER-LOCKED (2026-06-08)** — the broader fix that fully closes the silent-loss class (preserves gathered work on result-node reverts; reclaims on call-node reverts). This plan FORMALISES the implementation against the VALIDATED (B) mechanism — phases, file-level deliverables, test allocation, the 3 KC-01 test-flips, the on-trunk-keep gate, the audit envelope, the §1 locked-fork bodies — and does **NOT** re-characterize or re-decide.
>
> **Project = phi-core** (NOT baby-phi). Root `/root/projects/phi/phi-core`, branch `dev`, crate 0.11.4. HOST cargo (`/root/rust-env/cargo/bin/cargo`), single crate (no `--workspace`), cap `-j 4`. MUST-RUN = clippy `--all-targets` (`RUSTFLAGS="-Dwarnings"`) + `test` + `fmt --check`. NO `scripts/check-*.sh` CI guards (phi-core ships only a `scripts/pre-commit` fmt+clippy hook). phi-core-leverage-check + k8s-readiness-check are **N/A** (kernel does not consume itself / library not daemon). Verified-headers APPLY.

---

## §1 — Locked fork details (per chunk-planner v32 P-plan-1-v32; planner-rec bodies pre-filled at iter-2 re-author)

> F1 is a **product-semantic** fork (which gathered work survives a pinned revert is user-perceivable) — framed with User-visible / Product-trajectory per chunk-planner v26 P-plan-1-v26. F2 + F3 are **TECHNICAL** (no user-visible delta — F2 is a no-op confirmation, F3 is the absorb-vs-split routing). **F1 is USER-LOCKED at the BROADER (B) 2026-06-08** (USER-DIVERGENT from the iter-1 (a)-only draft — re-authored fully here against the P0 §10-validated mechanism); F2 + F3 are at investigation-rec (preserved verbatim from iter-1).

#### F1 = F1.B — pinned-onto-call-node reclaim-to-breadcrumb + pinned-onto-result-node keep-whole-cluster *(pre-lock draft; USER-LOCKED (B) 2026-06-08 — broader silent-loss-class closure)*

- **User-visible.** When a model fires ≥ 2 parallel tool calls in one node and then pins-reverts (`completion` / `step-summary`), the disposition is now **target-aware + complete**: a revert that lands *onto the CALL-node* (the model is sealing the whole sub-task) collapses the abandoned cluster uniformly into ONE durable summary breadcrumb; a revert that lands *into the cluster (onto any RESULT node)* now KEEPS the WHOLE gathered cluster verbatim (every sibling result re-appended), instead of silently keeping one sibling and dropping the rest. The user perceives "the whole gathered span survives unless I sealed past the entire cluster" — eliminating both the silent-loss / re-fetch loop #80 surfaced AND the half-kept asymmetry, with no provider 400 in any cell.
- **Code-level binding.** Locus is `retain_pinned_calls_on_node` (`context.rs:755`) + a one-token caller change at its sole call-site (`:728`). Thread the caller's already-computed `node_is_call` (`:714`) as a NEW `tag_on_call_node: bool` parameter into `retain_pinned_calls_on_node(&mut trunk, cn_idx, node_is_call)`. Per-call branch (P0 §10.1): **(i) result ON-trunk → KEEP always** (the model continued PAST this cluster — mid-trunk pinned cluster; this is the LOAD-BEARING gate — without it `pinned_cluster_mid_trunk_kept_verbatim` regresses); **(ii) result OFF-trunk + `tag_on_call_node` → DROP the call, never re-append** (reclaim the whole cluster to the durable breadcrumb — the (a) semantic for the call-node case); **(iii) result OFF-trunk + tag-on-result-node → RE-APPEND by CALL-NODE MEMBERSHIP** (`tool_call_id == call_id` against the forensic log, NOT the direct-child `parent_id == this_node_id` gate — the linear-stamped siblings are GRANDCHILDREN, so the direct-child gate cannot reach them). Two concrete code changes vs the KC-01 body: (1) the off-trunk re-append predicate changes from `lm.parent_id == Some(this_node_id) && tool_call_id == call_id` (`:802-815` direct-child) to `tool_call_id == call_id` (call-node membership) — this is what reaches the off-trunk parallel siblings; (2) the new `tag_on_call_node` branch selects reclaim (call-node) vs keep-all (result-node). A genuine post-cluster follow-on whose `tool_call_id ∉ this node's calls` never matches → stays dropped/sealed-past. Render-pass only; `self.messages` accessed `.iter().find()` + `.clone()` ONLY (never mutated — the only `self.messages` diff is the find-predicate rename); `build_trunk_context` / node-model untouched; `enforce_call_atomic_backstop` (`streaming.rs:130`) unchanged. **~+28 LOC over the (a) deletion** (P0 §10.6: `+32 / −13` in the fn + 1 caller token; functional logic ≈ +19 net LOC).
- **Rationale.** P0 §10 VALIDATED (B) end-to-end through the verbatim production pipeline (collapse → decay → weave → inject → backstop): all **10** LOSS cells fix (not 8 — the §3 prose under-counted; the matrix table marks 10 bolded LOSS rows incl. the 2 `mid-result × arity-3` cells, P0 §10.3); **0** regressions on the currently-OK cells; **0** new orphan/dangling/400 across all 37 cells (32 matrix + 5 edges); full lib suite 236 pass / exactly 3 fail (the 3 intended KC-01 flips), all 106 integration tests green. (B) is the user-chosen broader closure: it preserves ALL gathered work on result-node reverts (the (a) draft dropped it), which the user judged the correct product semantic for "a model pinned INTO a cluster wants the gathered span". The on-trunk-keep gate (P0 §10.6 / §10.8 surprise #1) is part of the mechanism, NOT optional: naive "drop-all-on-call-node" over-reaches on mid-trunk pinned clusters the model legitimately continued past.
- **Product trajectory.** (B) gives parallel-cluster reverts a complete, target-aware disposition: seal-past-the-whole-cluster (call-node) reclaims; revert-into-the-cluster (result-node) preserves the gathered span verbatim. This is the stronger guarantee ahead of the MCP / combined-agent cluster (#67 / #64 / #68) where parallel-tool + revert co-occur heavily and gathered parallel work is expensive to re-fetch. The deferred capability — a true-sibling node model that would let result-node re-append happen WITHOUT the forensic-log membership lookup — is recoverable later (Revisit trigger, §5) but is NOT needed for #80 closure: the linear chain + the render-pass membership re-append fully closes the grid.
- **Defers (if chosen).** Defers the deeper "true-sibling node model" (stamp parallel results as real siblings of n1) — explicitly NOT required (P0 F-8 / §10.6) and surfaced as a deferrable Revisit trigger, not a KC-03 blocker (it would only let result-node re-append use the direct-child gate instead of call-node membership). Defers nothing for the S2 closure: all 10 LOSS cells flip in this chunk (4 call-node → `[DROP,…]`+crumb; 6 result-node → `[PAIR,…]` whole-cluster). The KC-01 §D1.2 "M=1 re-append the off-trunk direct-child result" + the M=1 keep-whole behaviour are SUPERSEDED for the pinned-onto-call-node / into-cluster sub-cases (see §2 + §5 + §6) — intended, not a deferral.

#### F2 = F2.confirm-uniform — abandon-class is already correct + uniform (no-op) *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** NO abandon-path code change. The plan records F2 as a **confirmation row**: abandon-class (`failure` → Lesson / `tangent` → Finding) onto-call-node + onto-first-result already fully + symmetrically collapses the cluster to the breadcrumb (`[DROP,…]`, no surviving call — P0 probe `[ABANDON call-node a3] any_call=false`); the result-node path (`:588-668`) moves the tag to the parent and clears identically. The benign abandon ASYMMETRIC cells (mid/last-result) are revert-target-position effects (everything at/before the tip is live + kept; the abandoned tail is reclaimed), NOT a keep-first/drop-rest leak. The (B) branch lives in the *pinned* arm ONLY (`:691-732` → `retain_pinned_calls_on_node`) — abandon-class never reaches it (P0 §10.4: all 16 abandon-class cells byte-identical pre/post-(B)).
- **Rationale.** P0 F-4 + F-5 settle this DEFINITIVELY: abandon-class carries NO sibling-leak on the load-bearing call-node/first-result axis, and the asymmetric mid/last-result cells are benign (the surviving siblings are LIVE on-trunk work legitimately kept atomic; the abandoned tip IS reclaimed, `crumb=true`, balance OK). There is nothing to adjust — adding any abandon-path change would be dead code. P0 §10.4 re-confirms abandon-class is untouched by (B). F2 closes as a no-op confirmation documented in ADR-0002 §D2.2.
- **Defers (if chosen).** Defers nothing — the abandon-class is already correct + uniform; no machinery is added or removed. The confirmation is captured in the ADR sub-decision + the promoted regression rows (the benign abandon-asymmetry cells ship as permanent coverage so a future change cannot silently regress them). No drift filed (non-load-bearing; confirm-only per P0 F-5).

#### F3 = F3.absorb — render-pass-only fix, no KC-03 split, no node-model change *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** Absorb the entire fix in KC-02; NO KC-03 split, NO node-model / sibling-stamping change. The fix is render-pass-only at `retain_pinned_calls_on_node` (`context.rs:755`) + the 1-token caller signature thread (`:728`), `+32 / −13` LOC (P0 §10.6). `build_trunk_context` (`context.rs:1074`, the sole `parent_id` trunk-walk consumer) is untouched; `streaming.rs` backstop untouched. P0 §10.6 CONFIRMED render-pass-only: `self.messages` read-only (`.iter().find()` + `.clone()`), the only `self.messages` diff is the find-predicate rename `direct_child` → `cluster_result`.
- **Rationale.** P0 F-6 + F-8 + §10.6 prove no separable large issue exists: (B) works purely in the render pass; the only split-worthy item (a `build_trunk_context` tree/DAG re-architecture to stamp true siblings) is UNNECESSARY for #80 closure (the linear chain + the call-node-membership re-append fully closes all 10 LOSS cells). Splitting would manufacture scope. The true-sibling node model remains a deeper-correctness deferrable, surfaced as a Revisit trigger, not a KC-03 blocker.
- **Defers (if chosen).** Defers the true-sibling node model as a documented Revisit trigger (deferrable, not needed for #80 — it would only simplify the result-node re-append lookup). Defers nothing else — the full grid is closed in KC-02 by the F1(B) render-pass change. No KC-03 chunk is opened by this cycle.

**§1 self-check (chunk-planner v32 P-plan-1-v32):** §1 carries a populated `#### F<N> = F<N>.<rec>` subsection for all 3 forks (F1, F2, F3) — F1.B with User-visible / Code-level binding / Rationale / Product trajectory / Defers; F2 + F3 (TECHNICAL) with Code-level binding / Rationale / Defers. Each block ≥ 3 sentences; F1 carries the `**User-visible:**`-equivalent + `**Product trajectory:**`-equivalent framing (per v26 P-plan-1-v26, non-TECHNICAL fork). Invoke `chunk-template-validate-locked-appendix` at end-of-draft → expect PASS (3 subsections ≥ 3-lock count, each ≥ 3 sentences).

---

## §2 — Concept alignment

| Concept doc | Line(s) | Current statement | KC-02 alignment | Action |
|---|---|---|---|---|
| `docs/concepts/concept-brake.md` | `:1` (verified-header), `:192` (co-keep constraint, amended at KC-01 to "call-atomic") | States the bonded-pair invariant is enforced per-call (KC-01); does NOT state the pinned-revert *target-aware reclaim/keep* semantic (call-node → reclaim; result-node → keep whole cluster) | KC-02 (B) makes pinned-revert disposition target-aware: call-node revert reclaims the abandoned cluster into the breadcrumb; result-node revert KEEPS the whole gathered cluster (every sibling re-appended); on-trunk results always kept | Refresh verified-header + append a note: pinned-revert disposition is target-aware — onto-call-node reclaims the abandoned cluster into the breadcrumb (symmetric with abandon-class), into-cluster (onto-result-node) keeps the WHOLE gathered span verbatim; the 32-cell matrix is the binding semantics (cite ADR-0002) |
| `src/types/context.rs` `retain_pinned_calls_on_node` doc-comment | `:738-754` + `:691-709` (the mirror at the pinned branch) — the 3-case bullet list: keep-if-on-trunk / **re-append-if-direct-child** / drop-otherwise | Documents the **re-append-direct-child** path (M=1 revert-onto-call-node re-appends the direct-child result) — the KC-01 body | KC-02 rewrites to the (B) 3-case target-aware form: keep-if-on-trunk / (off-trunk + tag-on-call-node) reclaim-drop / (off-trunk + tag-on-result-node) re-append-by-call-node-membership | Rewrite both doc-comment regions to the (B) 3-case form keyed on `tag_on_call_node` + the call-node-membership re-append (replacing the direct-child gate); cite ADR-0002 §D2.1 + the §D2.1 supersession of ADR-0001 §D1.2 |
| `docs/decisions/0001-parallel-revert-call-atomicity.md` (ADR-0001) | `§D1.2` (R1 per-call retain — "the M=1 single-call path is preserved byte-identical … a pinned assistant node emits only the calls whose result is live on the trunk (or **re-appendable as a direct child**)"), `## Verification` ("M=1 byte-identical") | ADR-0001 §D1.2 asserts the off-trunk-direct-child result is **re-appended** (M=1) + the M=1 keep-whole; M=1 byte-identical | KC-02 F1(B) **SUPERSEDES** §D1.2 for the pinned-onto-call-node / into-cluster sub-cases: onto-call-node reclaims (drop, no re-append); into-cluster re-appends by call-node membership (reaching grandchildren the direct-child gate missed). M=1 byte-identical no longer holds for the pinned-onto-call-node sub-case | Add an inline `> Superseded-in-part by ADR-0002 §D2.1 (2026-06-08): pinned-revert disposition is target-aware — onto-call-node reclaims (drop, no re-append); into-cluster re-appends siblings by call-node membership (not the direct-child gate). KC-01's M=1-byte-identical no longer holds for the pinned-onto-call-node / into-cluster sub-cases.` note at ADR-0001 §D1.2 + bump ADR-0001 verified-header |
| `src/types/content.rs` | `:143` (the native `id ↔ tool_call_id` join) | Documents the join; KC-02 consumes it for the call-node-membership re-append (`tool_call_id == call_id`) | Confirmatory; consumed unchanged (the on-trunk test + the new membership re-append both key on it) | No change |

**Concept-fidelity note (KC-01 interaction — load-bearing, surfaced per task directive):** F1(B) is the **first phi-core cross-ADR supersession**. ADR-0002 §D2.1 supersedes ADR-0001 §D1.2 (the off-trunk-direct-child re-append) AND the KC-01 M=1 keep-whole assertion. Under (B): onto-call-node reverts **reclaim** the cluster (the model sealed past it), into-cluster reverts **keep the whole gathered cluster** (re-append every sibling by call-node membership). This is **intended, not a regression**: it closes the full silent-loss class (10 LOSS cells, P0 §10.3) and makes the disposition target-complete. The consequence is that **KC-01's "M=1 byte-identical" claim no longer holds for the pinned-onto-call-node / into-cluster sub-cases** — see §5 (ADR-0002 §D2.1 supersession note + ADR-0001 amendment) + §6 (the 3 KC-01 test-flips). NO node-model change; the fix stays render-pass-only. The on-trunk-keep gate (clause (i)) is what keeps `pinned_cluster_mid_trunk_kept_verbatim` GREEN (the cluster's results are on-trunk → kept verbatim, never reclaimed).

---

## §3 — phi-core leverage map → **kernel-minimality surface-discipline** (project=phi-core)

> phi-core-leverage-check is **N/A** — the kernel does not consume itself. Per `[[feedback_phi_core_kernel_minimal]]` and the task directive, §3 carries the **kernel-minimality surface-discipline analysis** (the P0 §4 fix-locus carve-out, cited + ratified).

### §3.A — Kernel-minimality carve-out (cite P0 §4 + §10.6)

| Change (locked fork) | Side | General-primitive? | Consumer-specific leakage? | Blast radius |
|---|---|---|---|---|
| F1(B): in `retain_pinned_calls_on_node` (`:755`) — thread `tag_on_call_node`, change the off-trunk re-append predicate from direct-child (`:802-815`) to call-node membership (`tool_call_id == call_id`), branch reclaim (call-node) vs re-append-all (result-node), keep on-trunk always | **kernel** | YES — target-aware pinned-revert reclaim/keep semantics is a render-correctness property all consumers need; any consumer (i-phi / baby-phi / future MCP) firing parallel tools while pin-reverting is affected identically | NONE (zero i-phi requirement leaks in) | `retain_pinned_calls_on_node` (`context.rs:755`) + the 1-token caller signature thread at `:728` ONLY |
| F2: abandon-class — no code change (confirm-uniform) | kernel (no-op) | YES (already correct) | NONE | none |
| F3: absorb — no node-model / `build_trunk_context` change | n/a — not required (P0 F-8 / §10.6) | n/a | n/a | none |
| Doc-comment (`:738-754` + `:691-709`) + `concept-brake.md` + ADR-0001 §D1.2 supersession note + ADR-0002 | kernel docs | doc-code alignment for the new target-aware reclaim/keep semantic | NONE | docs only |

**Verdict (cite P0 F-6 / F-8 / §10.6):** fix-locus is the SINGLE function `retain_pinned_calls_on_node` + a 1-token caller signature thread. `collapse_abandon_class_cluster`'s sole call-site is `streaming.rs:237` (revert-mode only); `sub_agent.rs` / `parallel.rs` / `evaluation.rs` do NOT invoke it → **zero blast radius outside the brake render**. `self.messages` (forensic log) is `.iter().find()` + `.clone()` only — **never mutated** (P0 §10.6: the only `self.messages` diff is the find-predicate rename). No public-API / persisted-field / migration change. **Zero i-phi leakage.** Genuine general kernel fix per the `[[feedback_phi_core_kernel_minimal]]` carve-out.

### §3.B — Build / LOC caps + pause-discipline (phi-core; NO K8s axes — N/A library)

> **K8s readiness: N/A — phi-core is a library, not a daemon** (k8s-readiness-check returns N/A for all 7 axes; no `m7b` deferred-ledger). The table below is the per-file LOC cap + pause-trigger (chunk-planner v23 P-plan-1 functional-scope-derivation). Note (B) is **ADDITIVE** (a new branch + a membership re-append), NOT a pure deletion — the P0-validated production delta is `+32 / −13` (~+19 net body LOC) + 1 caller token.

| File | Surface | LOC-cap derivation (per-axis) | Cap | Pause @ (1.5×) |
|---|---|---|---|---|
| `src/types/context.rs` (`retain_pinned_calls_on_node` + caller `:728`) | thread `tag_on_call_node` param (signature + 1 caller arg ~+3 LOC); change off-trunk re-append predicate to call-node membership (~−4 / +4 LOC); add the `tag_on_call_node` reclaim-vs-re-append-all branch (~+14 LOC incl. the on-trunk-keep gate); remove the now-unused direct-child closure (~−9 LOC). P0-validated `+32 / −13` (comments included; ≈ +19 net body LOC). | per-axis ≈ `+32 / −13` production = +19 net body + ~3 caller-token; doc-comment rewrite (`:738-754` + `:691-709`) ~ +25 LOC | **≤ 55 LOC net (production +19 net body + ~3 caller + ~25 doc-comment + headroom)** | 70 |
| `src/types/context.rs` (inline tests) | promote the P0 §10 throwaway 32-cell matrix + 5 edge fixtures to in-tree regression in `collapse_abandon_class_cluster_tests` (`:2069`) — re-author the arity-3 + post-cluster + two-cluster fixtures + the `trunk_has_orphan_result` detector + the per-cell assertions (see §8); re-author the 3 flipped KC-01 tests | NEW arity-3 fixture (`multi_call_node_3` + `parallel_fixture_3`) ~50 LOC + post-cluster + two-cluster + mixed-on/off-trunk fixtures ~60 LOC + `trunk_has_orphan_result` detector ~20 LOC + per-cell/grouped assertions (see §8) ~ 240-360 LOC + the 5 edge-fixture tests ~80 LOC + framework allowance (chunk-planner v33 P-plan-1-v33: `AgentContext` construction boilerplate ~+40 LOC) | **≤ 520 LOC inline-test** | 780 |

**Pause-discipline (cascade-band):** F1(B) is an **additive single-fn change + 1 caller token** — one signature change (`tag_on_call_node: bool`), no struct/trait/cross-file cascade (P0 §10.6 CONFIRMED render-pass-only). Production band `[16, 55]` (P0-validated `+19` net body + caller + doc-comment). PAUSE if `retain_pinned_calls_on_node`-region production net exceeds **70 LOC** OR if the implementer discovers the call-node-membership re-append needs more than the forensic-log `.find()` (e.g. a node-model touch) — surface via AskUserQuestion, do NOT push through. Inline-test band is wide (matrix + 5 edges promotion is the bulk of the diff) — PAUSE if inline-test > 780 LOC. **No K8s; no migration; no public API change.**

### §3.C — Inline-test cardinality (chunk-planner v27 P-plan-1-v27 + v30 P-plan-10-v30)

The matrix is a `category(4) × target(≤5) × arity(2,3)` coverage surface, not enum-dispatch. The P0 32-cell grid + 5 edge fixtures promote to a manageable in-tree set via grouped assertions (one `#[test]` per coherent cell-group rather than 37 separate fns). Inline tolerance derivation (see §8 for the per-tier breakdown): **8-12 NEW grouped/edge tests** + **3 UPDATED KC-01 tests** (the 3 flips). Band `[8, 12]` allowing ≤ 3 incidental edge-case tests the implementer adds. Baseline: **20 existing `collapse_abandon` lib tests** (VERIFIED via `cargo test --lib collapse_abandon -- --list` → 20) MUST stay green except the 3 KC-01 tests that are UPDATED in place (not added/removed); the on-trunk-gate test `pinned_cluster_mid_trunk_kept_verbatim` stays GREEN unchanged (the (B) on-trunk-keep gate preserves it).

### §3.D — Forward-scope ↔ concept-doc precedence

No closed-set / fixed-order / frozen-schema contradiction. The fix consumes the existing native join + existing breadcrumb/weave path + the existing forensic log; it changes a render rule (target-aware reclaim/re-append), it does not extend a closed vocabulary. `N/A — no contradiction to surface`.

### §3.E — Anticipated gate-2.5 candidates (chunk-planner v13)

- **Breadcrumb-reclaim confirmation for all-calls-dropped node (call-node case).** F1(B) clause (ii) relies on the EXISTING path: when all of a pinned call-node's calls drop (off-trunk + `tag_on_call_node`), the node retains its pinned-tag text → `weave_braking_annotations` renders the breadcrumb; R4 keeps text-bearing nodes (not an empty-assistant 400). P0 §10.8 surprise #3 confirms this (pinned tags are non-decayable; `crumb=yes` in every (B) call-node cell). **Planner-rec: confirm at P1** that an all-calls-dropped pinned call-node still carries renderable tag-text. If the implementer finds the node collapses to genuinely empty → small additive weave/R4 fix; surface via AskUserQuestion at gate-2.5 (do NOT push through).
- **Call-node-membership re-append placement (result-node case).** F1(B) clause (iii) inserts off-trunk siblings right after the call node (`idx+1+offset`) — P0 §10.8 surprise #5 confirms this is benign for atomicity (`enforce_call_atomic_backstop` checks call↔result co-presence, not adjacency; no orphan/dangle in any cell). **Planner-rec: confirm at P1** the re-append placement is post-call-node + that the membership find resolves the off-trunk grandchildren (P0 §10.5 EDGE-5 mixed-on/off-trunk: a sibling PAST the revert tip re-appends).
- **Arity-3 + post-cluster + two-cluster fixture shape.** The P0 §10 matrix added arity-3 (`multi_call_node_3`) + post-cluster + two-cluster + mixed-on/off-trunk fixtures + a `trunk_has_orphan_result` detector, then reverted. Options: (a) add them to the existing `collapse_abandon_class_cluster_tests` module helpers (alongside `multi_call_node` `:3175` / `parallel_fixture` `:3213`); (b) factor a shared fixture module. **Planner-rec: (a)** (single consuming module; mirrors KC-01's choice).

---

## §4 — Drifts / issues closed

- **GitHub #80 / D-TEST-0075 (S2)** — parallel-cluster pinned-revert silent loss of gathered work. **Primary + only deliverable target.** Status flip at this cycle: the GitHub issue moves to closed at chunk-seal (orchestrator-run live close-gate + unit regression green). No phi-core drift-file ledger (the issue IS the tracking artifact). P-SEAL deliverable: post the closure comment on #80 citing the promoted 32-cell + 5-edge regression matrix + the live transcript-read close-gate result. Per the commit-subject discipline (outer CLAUDE.md), the P-SEAL commit subject prepends `D-TEST-0075:`.
- **No NEW grid-failure drift** — P0 F-1 + §10.4 proved 0 BROKEN-400 cells across all 37 cells (the #80 "hidden hard 400" hunch does NOT realize); the only defect is the 10 silent-LOSS cells, all closed by F1(B). The forward-scope §5.3 "file each NEW grid failure as its own D-TEST drift" deliverable closes as a **no-op confirmation** (no new hard failure found).
- **No other drifts touched.** Prior-cycle ratification: KC-01 (#77) is ratified-as-prereq (landed `46f7854`); KC-02 SUPERSEDES ADR-0001 §D1.2's re-append + M=1 keep-whole in-part (an ADR amendment, not a drift — see §5).

---

## §5 — ADR draft

> phi-core `docs/decisions/` exists (KC-01 minted it with ADR-0001). KC-02 adds **ADR-0002**: `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md`. Numbering continues at `0002`. Cite by `ADR-0002 §D2.<M>`.

**Proposed: phi-core ADR-0002 — Parallel-cluster pinned-revert disposition (B): reclaim-on-call-node + keep-cluster-on-result-node (F1(B)).** Status: Accepted. The implementer authors all **7 canonical ADR top-level sections** (chunk-planner v17 explicit-enumeration):

1. `## Forks` — header table: **F1** (pinned-revert disposition → **F1.B reclaim-on-call-node + keep-whole-cluster-on-result-node**, USER-LOCKED 2026-06-08, product-semantic, broader-fix) / **F2** (abandon-class consistency → confirm-uniform, no-op, TECHNICAL) / **F3** (KC-03 split → absorb, TECHNICAL). The F1 row notes USER-LOCKED + product-semantic + the (a)→(B) divergence; F2/F3 note TECHNICAL.
2. `## Context` — #80 / D-TEST-0075 S2 gap; the 32-cell matrix (P0 §3) + the validated (B) re-run (P0 §10); the **10** LOSS cells (pinned keep-first/drop-rest) vs the 0 BROKEN-400 cells (the #80 hunch does NOT realize, F-1); the node-atomic→call-atomic foundation from KC-01; the on-trunk-keep gate the matrix forced (P0 §10.6 / §10.8 surprise #1).
3. `## Sub-decisions` —
   - `### §D2.1 — F1(B) reclaim-on-call-node + keep-whole-cluster-on-result-node (resolves F1; SUPERSEDES ADR-0001 §D1.2 M=1 re-append + M=1 keep-whole)`. Pre-existing-behaviour: ADR-0001 §D1.2 (KC-01) re-appended the off-trunk-direct-child result + kept the M=1 cluster whole (`retain_pinned_calls_on_node:802-815`). KC-02 makes the disposition target-aware: **(i) result on-trunk → KEEP always** (the load-bearing mid-trunk gate); **(ii) off-trunk + tag-on-call-node → DROP, reclaim to breadcrumb**; **(iii) off-trunk + tag-on-result-node → RE-APPEND by call-node membership** (`tool_call_id == call_id`, reaching grandchildren the direct-child gate missed). **Explicitly state**: ADR-0001 §D1.2's "M=1 byte-identical" + M=1-keep-whole claims no longer hold for the pinned-onto-call-node / into-cluster sub-cases; this is intended (full silent-loss-class closure), not a regression. Cite the matrix: all 10 LOSS cells flip (4 call-node → `[DROP,…]`+crumb; 6 result-node → `[PAIR,…]` whole-cluster).
   - `### §D2.2 — F2 abandon-class confirmed-uniform (no-op)`. Net-new confirmation (no prior behaviour changed): abandon-class onto-call-node + first-result already fully + symmetrically collapse to the breadcrumb (P0 F-5); the asymmetric mid/last-result cells are benign target-position effects (P0 F-4), not leaks; all 16 abandon-class cells byte-identical pre/post-(B) (P0 §10.4). No abandon-path code change.
   - `### §D2.3 — F3 absorb, no node-model change (resolves F3)`. Net-new confirmation: the fix is render-pass-only (`retain_pinned_calls_on_node` + 1-token caller thread); no `build_trunk_context` / sibling-stamping change; no KC-03 split (P0 F-6 / F-8 / §10.6). The true-sibling node model is a deferrable (it would only simplify the result-node re-append lookup), surfaced in Revisit triggers.
   - `### §D2.4 — the no-400 invariant is preserved by the backstop`. Net-new confirmation: P0 F-1 + §10.4 proved 0 BROKEN-400 cells across all 37 cells (32 matrix + 5 edges); `enforce_call_atomic_backstop` (`streaming.rs:130`) orphan-filters any dangling call AFTER the per-call disposition. F1(B) both drops calls (call-node case) AND re-appends results (result-node case) — the backstop guarantees no orphan results + no dangling calls survive → no 400. Cite the 37-cell validation (`trunk_has_orphan_result` + `trunk_has_dangling_call` both false in every cell).
4. `## Cross-references` — (a) concept-doc: `concept-brake.md:192` + `context.rs:738-754`/`:691-709` doc-comments; (b) closed drifts: #80 / D-TEST-0075; (c) **prior ADRs: ADR-0001 §D1.2 (SUPERSEDED-in-part by §D2.1) + ADR-0001 §D1.5 (R4 empty-node, relied on for breadcrumb keep-as-text)**; (d) forward-scope row `kc-02-parallel-cluster-revert-matrix.md` + P0 investigation §3 + §10.
5. `## Consequences` — `### For consumers (i-phi / baby-phi / future)`: pinned reverts now reclaim the abandoned parallel tail uniformly on call-node reverts AND keep the whole gathered cluster on into-cluster reverts (no silent half-keep / re-fetch loop); hardens braking ahead of the MCP / combined-agent cluster (#67 / #64 / #68 — forward-routing note). `### For KC-01 / ADR-0001`: §D1.2 re-append + M=1 keep-whole superseded; **3** KC-01 tests updated. No public API / persisted-field / migration change.
6. `## Revisit triggers` — (i) a future **true-sibling node model** (stamp parallel results as real siblings of n1) is adopted → re-opens §D2.3 (the result-node re-append could use the direct-child gate instead of call-node membership); deferrable, NOT needed for #80. (ii) A product decision that a pinned `completion` onto-call-node MUST also preserve all gathered work verbatim (not reclaim) → re-opens §D2.1 clause (ii). (iii) A new revert-render path bypasses `retain_pinned_calls_on_node` → re-opens §D2.3 (the absorb-vs-split boundary). (iv) The breadcrumb-reclaim path stops firing for an all-calls-dropped call-node → re-opens §D2.1's reliance on weave/R4. (v) The on-trunk-keep gate (clause (i)) interacts badly with a future decay/window change → re-opens the mid-trunk-kept-verbatim guarantee.
7. `## Verification` — `cargo test -j 4 --manifest-path … --lib collapse_abandon` (the promoted matrix + 5 edges + the 3 updated KC-01 flips + the on-trunk-gate test) + clippy `-Dwarnings` + `fmt --check` + the live transcript-read full-grid close-gate.

**ADR-0001 amendment (P-SEAL paperwork):** add the `> Superseded-in-part by ADR-0002 §D2.1` inline note at ADR-0001 §D1.2 + bump ADR-0001's verified-header to cite the KC-02 amendment (per outer CLAUDE.md ADR-inline-amendment verified-header discipline).

---

## §6 — Prior-chunk regression / carry-forward invariants

KC-01 (`46f7854`) is the immediate + only prereq. The invariants the F1(B) render change must honor (or intentionally supersede):

| Invariant | Verifying command | Expected |
|---|---|---|
| 20 existing `collapse_abandon` lib tests stay green **EXCEPT** the 3 KC-01 tests UPDATED in place | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon` | 17 of 20 unchanged-green; the 3 flipped tests UPDATED to the new (B) dispositions; `pinned_cluster_mid_trunk_kept_verbatim` stays GREEN unchanged (on-trunk-keep gate); total after +NEW matrix/edge tests (see §8) |
| **On-trunk-keep gate (load-bearing)**: `pinned_cluster_mid_trunk_kept_verbatim` (`context.rs:3017`) stays GREEN unchanged — a pinned cluster the model continued PAST (results on-trunk) is kept verbatim | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib pinned_cluster_mid_trunk_kept_verbatim` | GREEN (the (B) clause (i) keeps on-trunk results always; without the gate this REGRESSES — P0 §10.6 / §10.8 surprise #1) |
| `build_trunk_context` linear parent-walk unchanged (P0 F-6; sole `parent_id` consumer at `:1074`) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib build_trunk_context` | green (untouched) |
| `apply_revert` marks intent only; forensic `self.messages` never mutated (P0 §2 / §10.6) | grep `self.messages` mutation in `retain_pinned_calls_on_node` = none (only `.iter().find()` + `.clone()` reads; the only diff is the find-predicate rename) | render-pass-only confirmed |
| `enforce_call_atomic_backstop` (`streaming.rs:130`) unchanged; no-400 invariant holds for all 37 cells (P0 F-1 / §10.4 / §D2.4) | read diff; `streaming.rs` untouched | order intact; 0 BROKEN-400 |
| Full phi-core suite green (no cross-module regression — F1(B) blast radius = 1 fn + 1 caller token); 106 integration tests green (P0 §10.0) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path …` | green (lib + `agent_loop_test` 56 / `agent_test` 14 / `session_test` 36) |

**KC-01 test-flip (load-bearing — chunk-planner v13 R1 closed-set verification, grep-verified against live HEAD):** P0 §10.7 + my grep of `collapse_abandon_class_cluster_tests` (`:2069`) confirm **EXACTLY 3 KC-01 tests flip under (B)** (all intended, NOT collateral; P0 §10.0: full lib suite 236 pass / exactly 3 fail):

- **`pinned_class_keeps_cluster_whole_no_dangling_call`** (`context.rs:2458`) — M=1 pinned (Outcome) revert onto the same call node n1 (call `call_abc`, result n2 off-trunk). **Today** (KC-01 §D1.2 re-append + keep-whole) asserts `trunk_has_heavy_args("PLAN-V1-HEAVY-BODY")` (`:2477-2480`, keeps the sealed body whole) + `!trunk_has_dangling_call` (`:2482-2485`, re-includes the result) + `has_result` for `call_abc` (`:2486-2498`). **Under (B)** the M=1 call-node revert (off-trunk + `tag_on_call_node`) DROPS the call, reclaims to the breadcrumb → `[DROP]`+crumb. **UPDATE this test** (the MOST consequential flip — INVERTS the "keep the sealed body whole" assertion): assert the call+result are DROPPED (`!has_result` for `call_abc`, `!trunk_has_dangling_call`) + `trunk_has_breadcrumb("reverted past: …")`. P0 §10.7 row 1.
- **`row_pinned_revert_to_call_node_now_call_atomic`** (`context.rs:3261`) — arity-2 revert onto call node n1, both results off-trunk, `pcall_a` a direct child of n1. **Today** asserts `[PAIR,DROP]` (`pcall_a` re-appended, `pcall_b` dropped). **Under (B)** (off-trunk + `tag_on_call_node`) → whole reclaim `[DROP,DROP]`+crumb. **UPDATE**: assert `!trunk_has_tool_result(pcall_a)` + `!trunk_has_tool_result(pcall_b)` + `!trunk_has_dangling_call` + `trunk_has_breadcrumb`. P0 §10.7 row 2.
- **`row_pinned_revert_to_first_result_now_call_atomic`** (`context.rs:3237`) — arity-2 revert onto result_a (n2), so `pcall_a`'s result is ON-trunk; `pcall_b`'s result (n3) is off-trunk, a grandchild (parent n2, not n1). **Today** asserts `[PAIR,DROP]` (`pcall_b` NOT re-materialised under the direct-child gate). **Under (B)** (tag-on-result-node → re-append by call-node membership) `pcall_b` re-appends → `[PAIR,PAIR]` whole cluster. **UPDATE**: assert `trunk_has_tool_result(pcall_a)` (on-trunk, kept) + `trunk_has_tool_result(pcall_b)` (re-appended by membership) + `!trunk_has_dangling_call`. P0 §10.7 row 3 — exactly the user-confirmed "→ now `[PAIR,PAIR]`" prediction.

**Confirmation (grep-verified):** `pinned_cluster_mid_trunk_kept_verbatim` (`:3017`) does **NOT** flip — its pinned cluster's result (c1 result n2) is ON-trunk (model continued to tip n4), so the (B) on-trunk-keep gate (clause (i)) keeps it verbatim. The 3 SAFE rows (`:3285`/`:3304`/`:3322`) key on on-trunk results or post-cluster nodes → unaffected. **KC-01 test-flip count = 3.**

**Back-compat decision (chunk-planner v19 P2 — F1(B) supersedes a KC-01 scaffold):** F1(B) is a **divergent-from-KC-01 semantic** (supersedes the §D1.2 re-append + M=1 keep-whole). Decision: **(a) amend the 3 carry-forward test bodies to the new (B) semantic** + document the supersession in ADR-0002 §D2.1 + the ADR-0001 inline note. Not (b) preserve-scaffold (the re-append + keep-whole are the things being superseded) nor (c) defer/ignore (the 3 flips ARE the intended fix). This is the canonical amend-carry-forward-test path.

---

## §7 — Phase plan

> **§7.0 phase-order stress-test (chunk-planner v25 P-plan-4-v25).** 4 phases; F1 USER-DIVERGENT-from-iter-1 (broader (B); re-authored here), F2/F3 at investigation-rec. No cascade-band overlap. **Compile-time:** F1(B) threads ONE new param (`tag_on_call_node: bool`) into `retain_pinned_calls_on_node` + 1 caller arg — a single-signature change, no struct/trait change → the only compile boundary is the signature+caller, landed together in P1 → no RED window from type churn. **Runtime-test:** P1 lands F1(B) (the branch + membership re-append + on-trunk-keep gate) which immediately makes the **3** KC-01 tests RED — so P1 MUST also update those 3 tests in the same phase to close the window (the updates are mechanical + co-located; the on-trunk-gate test `pinned_cluster_mid_trunk_kept_verbatim` stays GREEN by the clause-(i) gate). P2 promotes the NEW matrix + 5-edge tests (GREEN-on-arrival, authored after the fix lands). **Single RED-window mitigation:** P1 lands the fix + the 3-test update together → no boundary leaves the suite RED. Diff-coherent for a 3-auditor Large envelope (the carry-forward-regression surface = the 3 KC-01 flips + the on-trunk-gate + the no-400-across-37-cells invariant → Audit C).

### P0 — Read + ground (no code)
- **Goal:** implementer reads §9 reading list; confirms P0 §10 facts + the exact `retain_pinned_calls_on_node:755` body + the caller `node_is_call` at `:714` / call-site `:728` at live HEAD (re-verify if drift since plan-draft); confirms the breadcrumb-reclaim path fires for an all-calls-dropped call-node (§3.E candidate 1) + the call-node-membership re-append resolves off-trunk grandchildren (§3.E candidate 2).
- **Deliverables:** confirmation note; no diff.
- **Tests:** baseline `cargo test … --lib collapse_abandon` = 20 green; `pinned_cluster_mid_trunk_kept_verbatim` green.
- **Confidence:** 10/10. **Pause-discipline:** if the all-calls-dropped call-node does NOT retain renderable breadcrumb tag-text, OR the membership re-append cannot resolve grandchildren via the forensic-log `.find()` → AskUserQuestion before P1.

### P1 — F1(B) fix + update the 3 flipped KC-01 tests
- **Goal:** implement the validated (B) branch in `retain_pinned_calls_on_node`: thread `tag_on_call_node: bool` (from caller `:728`, the existing `node_is_call`), keep on-trunk results always (clause i — the load-bearing gate), reclaim-drop on off-trunk + tag-on-call-node (clause ii), re-append by call-node membership (`tool_call_id == call_id`, replacing the `:802-815` direct-child gate) on off-trunk + tag-on-result-node (clause iii); update the 3 flipped KC-01 tests (`pinned_class_keeps_cluster_whole_no_dangling_call:2458`, `row_pinned_revert_to_call_node_now_call_atomic:3261`, `row_pinned_revert_to_first_result_now_call_atomic:3237`) to the new dispositions (§6).
- **Deliverables:** `src/types/context.rs` — the F1(B) branch (`+32 / −13`, P0 §10.6) + the 1-token caller signature thread (`:728`); the 3-test update (§6). Render-pass only; `streaming.rs` untouched; forensic `self.messages` read-only.
- **Tests:** 17 of 20 existing stay green + `pinned_cluster_mid_trunk_kept_verbatim` green; the 3 updated tests green under the new (B) semantic.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if `retain_pinned_calls_on_node`-region production net > 70 LOC (§3.B), OR if the breadcrumb does not reclaim on a call-node-drop, OR if the membership re-append needs a node-model touch (would exceed the render-pass-only bound → AskUserQuestion).

### P2 — Promote the 32-cell matrix + 5 edge fixtures
- **Goal:** re-author the P0 §10 throwaway matrix + edges as in-tree regression in `collapse_abandon_class_cluster_tests` (`:2069`): the arity-3 fixture (`multi_call_node_3` + `parallel_fixture_3`), the post-cluster fixture, the mixed-on/off-trunk fixture, the two-cluster fixture, the `trunk_has_orphan_result` detector, the per-cell-group assertions (see §8), and the 5 edge fixtures (post-cluster-follow-on-dropped, mixed-on/off-trunk, M=1, KC01-flip probes, two-cluster isolation) — covering all 32 cells (the 10 LOSS cells now `[DROP,…]`+crumb (call-node) or `[PAIR,…]` whole-cluster (result-node); benign abandon-asymmetry unchanged; no-loss last-result/post-cluster unchanged; no-400 for all 32) + the 5 edges + an explicit on-trunk-gate regression probe.
- **Deliverables:** 8-12 NEW grouped/edge `#[test]` fns + 3-4 NEW fixtures/helpers (`multi_call_node_3`, `parallel_fixture_3`, post-cluster, two-cluster, `trunk_has_orphan_result`); reuse existing `multi_call_node` (`:3175`) / `parallel_fixture` (`:3213`) / `trunk_has_dangling_call` (`:2235`) / `trunk_has_breadcrumb` (`:2199`) / `trunk_has_tool_result` (`:2362`).
- **Tests:** the 4 call-node LOSS cells assert `[DROP,…]`+breadcrumb; the 6 result-node LOSS cells assert `[PAIR,…]` whole-cluster; the benign abandon + no-loss cells stay as the matrix shows; the no-400 invariant (`!trunk_has_dangling_call` + `!trunk_has_orphan_result`) holds for all 32 cells; the 5 edges PASS (P0 §10.5).
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if inline-test > 780 LOC (§3.B).

### P-SEAL — Docs + ADR + paperwork
- **Goal:** rewrite the `retain_pinned_calls_on_node` doc-comments (`:738-754` + `:691-709`) to the (B) 3-case target-aware form; update `concept-brake.md:192` + verified-header; create `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md` (7 sections); add the ADR-0001 §D1.2 supersession inline note + ADR-0001 verified-header bump; append the KC-02 `_cycle-index.md` row; post the #80 closure comment.
- **Deliverables:** doc-comment rewrite; concept-brake amendment; ADR-0002 (7 sections, §D2.1–§D2.4, ≥ 5 Revisit triggers); ADR-0001 amendment; cycle-index row (leave `Iterations = pending`, `Status = in-flight` — orchestrator owns transitions per chunk-planner v16 P-SEAL lifecycle: gate-3 → ready-for-audit; gate-4 close → audited-pending-retro; Phase 6/7 → retro-complete + Iterations to final count); #80 closure note (commit subject prepends `D-TEST-0075:`).
- **Tests:** `fmt --check` + `clippy -Dwarnings` + full suite green.
- **Confidence:** 9/10. **Pause-discipline:** none.

---

## §8 — Tests summary

> Baseline (VERIFIED via `cargo test --lib collapse_abandon -- --list` → **20**): 20 existing `collapse_abandon` lib tests pass on `dev` 0.11.4 (KC-01's 13→20 landed). KC-02 adds NO test file (inline-only in `context.rs`).

**Per-Tier breakdown (chunk-planner v22 P1):** the P0 32-cell matrix + 5 edge fixtures promote via **grouped** assertions (one `#[test]` per coherent cell-group / edge, NOT 37 separate fns) — the throwaway driver used parametrised loops; the in-tree promotion groups cells by disposition class + lands the 5 edges as named tests.

| Tier | Test (grouped) | Cells / fixtures covered | Asserts |
|---|---|---|---|
| A (4 call-node LOSS → reclaim) | `matrix_pinned_onto_call_node_reclaims_to_breadcrumb` (arity 2 + 3, completion + step-summary) | 4 onto-call-node LOSS cells | each → `[DROP,…]`+breadcrumb, symmetric, `!dangling`, `!orphan`, breadcrumb present |
| A (6 result-node LOSS → keep whole) | `matrix_pinned_into_cluster_keeps_whole_cluster` (first-result a2+a3 + mid-result a3, completion + step-summary) | 6 result-node LOSS cells | each → `[PAIR,…]` whole-cluster (every off-trunk sibling re-appended by call-node membership), `!dangling`, `!orphan` |
| A (KC-01 flips — UPDATED ×3) | `pinned_class_keeps_cluster_whole_no_dangling_call` + `row_pinned_revert_to_call_node_now_call_atomic` + `row_pinned_revert_to_first_result_now_call_atomic` (all UPDATED in place) | the 3 KC-01 flip cells | M=1 call-node → `[DROP]`+crumb; arity-2 call-node → `[DROP,DROP]`+crumb; arity-2 first-result → `[PAIR,PAIR]` |
| A (on-trunk-gate — explicit regression) | `pinned_cluster_mid_trunk_kept_verbatim` (EXISTING, stays green) + a NEW `matrix_on_trunk_pinned_cluster_kept_always` probe | the load-bearing on-trunk-keep gate (clause i) | on-trunk pinned cluster kept verbatim (never reclaimed); guards the gate without which the mid-trunk test regresses (P0 §10.6) |
| B (benign abandon asymmetry — stays) | `matrix_abandon_class_asymmetry_is_benign` (failure + tangent, mid/last-result, arity 2+3) | 6 benign ASYMMETRIC abandon cells | surviving siblings LIVE on-trunk; abandoned tip reclaimed; `!dangling`; no silent loss |
| B (abandon symmetric — stays) | `matrix_abandon_class_call_node_first_result_symmetric` (failure + tangent, arity 2+3) | 8 symmetric abandon cells | `[DROP,…]` whole-cluster, `!any_tool_call`, breadcrumb |
| B (pinned no-loss — stays) | `matrix_pinned_last_result_and_post_cluster_no_loss` (completion + step-summary) | 6 last-result + post-cluster pinned cells | `[PAIR,PAIR(,PAIR)]` all live→kept |
| C (edges — P0 §10.5) | `edge_post_cluster_followon_dropped` + `edge_mixed_on_off_trunk` + `edge_two_cluster_isolation` (M=1 + KC01-flip-probes covered by the Tier-A flip tests) | the 3 distinct edge fixtures | follow-on dropped (membership distinguishes cluster-sibling vs sealed-past); sibling PAST the tip re-appends; two clusters isolated (upstream kept verbatim, tip cluster whole) |
| C (no-400 invariant — all 32) | `matrix_no_dangling_or_orphan_across_all_32_cells` | ALL 32 | `!trunk_has_dangling_call` + `!trunk_has_orphan_result` for every cell (P0 F-1 / §10.4 / §D2.4) |
| **Total NEW MUST-SHIP** | **8-9 NEW grouped/edge + 3 UPDATED + 1 EXISTING-green** | **32 cells + 5 edges + on-trunk gate** | |

**Tolerance band:** `[8, 12]` NEW grouped/edge tests + 3 UPDATED (the 3 KC-01 flips; net test-count change from the updates = 0). Allowing ≤ 3 incidental inline tests the implementer may split out for clarity (e.g. separating the arity-2 / arity-3 LOSS groups, or a standalone breadcrumb-reclaim assertion). **Expected `collapse_abandon` lib test delta: +8 to +12 (20 → 28-32); the 3 UPDATED KC-01 tests do not change the count.** Crate delta: same (+8 to +12; no new test file; inline in `context.rs`). NEW fixtures (`multi_call_node_3`, `parallel_fixture_3`, post-cluster, two-cluster, `trunk_has_orphan_result`) are helpers, not `#[test]`s. P0 §10.0 anchors the full-suite expectation: post-(B), full lib suite green (the 3 fails in the throwaway run were the un-updated KC-01 tests; once updated they pass), 106 integration tests green.

**Re-run command:** `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture`

---

## §9 — Pre-chunk gate (reading list + carry-forward)

Implementer MUST read before P1:
1. This plan + the forward-scope (`docs/specs/plan/forward-scope/kc-02-parallel-cluster-revert-matrix.md`).
2. **P0 investigation (authoritative — the 32-cell matrix + the validated (B) mechanism are the load-bearing inputs)**: `docs/specs/plan/build/_p0-investigations/kc-02-…-p0-investigation.md` — §3 matrix + F-1..F-9, §4 fix-locus, §5 ruled-out, §6 forks, §7 repro assets, §8 close-gate, **§10 the (B) design + validation (§10.1 branch logic, §10.2 post-(B) 32-cell matrix, §10.3 all-10-fix, §10.4 no-regression, §10.5 edge fixtures, §10.6 LOC + the on-trunk-gate refinement, §10.7 the 3 KC-01 flips, §10.8 surprises)**.
3. `src/types/context.rs` — `retain_pinned_calls_on_node` (`:755`, esp. the `:802-815` direct-child lookup being REPLACED by call-node membership + the `:829-831` re-append + the `:738-754` doc-comment + the `:691-709` mirror comment); the caller `node_is_call` at `:714` + the call-site `:728` (gains the `tag_on_call_node` arg); `collapse_abandon_class_cluster` (`:462`, pinned branch `:691-732`, abandon branches `:560-668`); the test module (`:2069` + fixtures `multi_call_node:3175` / `parallel_fixture:3213` + helpers `trunk_has_dangling_call:2235` / `trunk_has_breadcrumb:2199` / `trunk_has_tool_result:2362`); the 3 KC-01 tests that flip (`pinned_class_keeps_cluster_whole_no_dangling_call:2458`, `row_pinned_revert_to_first_result:3237`, `row_pinned_revert_to_call_node:3261`) + the on-trunk-gate test that must stay green (`pinned_cluster_mid_trunk_kept_verbatim:3017`) + the 3 SAFE rows (`:3285`/`:3304`/`:3322`).
4. `src/agent_loop/streaming.rs:130` (`enforce_call_atomic_backstop` — UNCHANGED; the no-400 belt) + `:215-262` (render pipeline order).
5. `src/types/content.rs:143` (the native `id ↔ tool_call_id` join, consumed by the membership re-append) + `node_tag.rs:95-100`/`:123` (Category→TagKind→class).
6. `docs/decisions/0001-parallel-revert-call-atomicity.md` §D1.2 (the re-append + M=1 keep-whole being superseded) + §D1.5 (R4, relied on for breadcrumb keep-as-text).
7. `docs/concepts/concept-brake.md:192` (co-keep constraint).

**Carry-forward invariants:** §6 table (20 existing tests minus the 3 updated + the on-trunk-gate test stays green + linear walk + no-log-mutation + backstop unchanged + full suite + 106 integration tests green).

---

## §10 — Close criteria

**Code-aspect:**
- F1(B): `retain_pinned_calls_on_node` gains the `tag_on_call_node` param; on-trunk results kept always (clause i); off-trunk + tag-on-call-node reclaims to the breadcrumb (clause ii); off-trunk + tag-on-result-node re-appends by call-node membership (clause iii, replacing the direct-child gate); `build_trunk_context` / node-model / `streaming.rs` untouched; forensic log read-only.
- The 10 LOSS cells flip: 4 call-node → `[DROP,…]`+breadcrumb symmetric; 6 result-node → `[PAIR,…]` whole-cluster (no silent loss). Benign abandon-asymmetry + no-loss last-result/post-cluster cells unchanged; the no-400 invariant holds for all 32 cells + the 5 edges (37 total).
- The 3 KC-01 tests UPDATED to the new (B) semantics; `pinned_cluster_mid_trunk_kept_verbatim` stays GREEN unchanged (on-trunk-gate); 17 of 20 existing stay green; 8-12 NEW matrix/edge tests green.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green (lib + 106 integration) + `fmt --check` clean.
- No `build_trunk_context` / node-model change; `streaming.rs` backstop untouched; no public API / persisted-field / migration change.

**Docs-aspect:**
- `retain_pinned_calls_on_node` doc-comments (`:738-754` + `:691-709`) rewritten to the (B) 3-case target-aware reclaim/keep form.
- `concept-brake.md` verified-header + co-keep note refreshed (pinned-revert disposition is target-aware: call-node reclaims, result-node keeps the whole cluster).
- `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md` Accepted with §D2.1–§D2.4 + ≥ 5 Revisit triggers.
- ADR-0001 §D1.2 supersession inline note added + ADR-0001 verified-header bumped.
- phi-core `_cycle-index.md` KC-02 row appended.
- #80 closure comment posted (cites the 32-cell + 5-edge regression matrix + the live close-gate; commit subject `D-TEST-0075:`).

**Pre-fix live confirmation (small, orchestrator-run — per P0 §8):** drive `deepseek-chat-v3-0324` (the proven parallel-firer) to fire ≥ 2 parallel tool_calls in one node then `revert_to_state(category: completion, step=<the call-node nN>)` — the `completion × call-node` cell. Render + READ the transcript: confirm exactly ONE sibling result survives today + the model re-fetches the dropped sibling (the silent-loss seal-loop), establishing the live S2 consequence (NOT a 400 — the matrix proved no 400 exists).

**Post-fix close-gate (orchestrator-run, the real close, per `[[feedback_render_transcript_close_gate]]`):** the FULL live grid — both drivers (`deepseek-chat-v3-0324` + `minimax-m2.7`) × the 4 categories — transcript-read, asserting correct disposition end-to-end: **call-node reverts reclaim to the breadcrumb** (sealed-past cluster summarised, no half-keep) AND **result-node reverts KEEP the whole cluster** (no sibling lost, no re-fetch loop); no 400 in any cell. Orchestrator-run (live provider round-trip; not bootable at phi-core unit level).

**Implementation confidence target: 9/10** (`claims-honored / claims-in-scope` ≥ 9/10).

---

## §11 — Audit plan

**audit-envelope-size:** Phase count = 4 (P0, P1, P2, P-SEAL) → mechanically Medium. **I size this LARGE (3 auditors: A code+matrix-tests / B docs+ADR / C carry-forward-regression: the 3 KC-01 flips + the on-trunk-gate + the no-400-across-37-cells invariant + the live-grid posture).** Honest Medium-vs-Large justification (per task directive): the FIX is validated + localized to 1 fn (+1 caller token), which alone argues Medium — BUT (B) is the **broader** fix and carries a genuine cross-cutting carry-forward-regression surface the (a)-draft did not: (1) it is **ADDITIVE** (`+32/−13` + a new render behaviour — the result-node whole-cluster re-append — not a pure deletion); (2) it **flips 3 KC-01 tests** (vs 1 in the (a)-draft) — three carry-forward assertions inverted, the most consequential being the M=1 keep-whole inversion; (3) it adds a **load-bearing on-trunk-keep gate** whose absence REGRESSES `pinned_cluster_mid_trunk_kept_verbatim` (P0 §10.6 / §10.8 surprise #1) — a distinct correctness branch needing its own regression guard; (4) the test surface grew to the full 32-cell matrix + 5 edge fixtures + the on-trunk-gate probe + the 2×4 live grid. The 3-KC-01-flip + on-trunk-gate + no-400-across-37-cells carry-forward posture is precisely what an Audit C (carry-forward regression) covers — splitting it out of Audit A keeps each prompt ≤ 600 words and gives the regression posture a dedicated reviewer. This UPGRADES the iter-1 (a)-draft's Medium and aligns with the forward-scope §8 Large hint (which the (a)-draft downgraded on the now-superseded narrower scope). **Confirm Large at gate-1.**

### Audit A — code-correctness + tests (≤ 600 words)
> Read-only on source. Plan at `…/kc-02-…/plan.md`. Verify each with file:line:
> 1. F1(B): `retain_pinned_calls_on_node` (`context.rs:755`) gains the `tag_on_call_node: bool` param (threaded from caller `:728`, the existing `node_is_call` at `:714`). The off-trunk re-append predicate is now call-node membership (`tool_call_id == call_id`), NOT the direct-child `parent_id == this_node_id` gate (former `:802-815`). On-trunk results are kept always (clause i; the former `:799` `on_trunk` continue is preserved as the load-bearing mid-trunk gate).
> 2. The 4 call-node LOSS cells flip to `[DROP,…]`+breadcrumb: `matrix_pinned_onto_call_node_reclaims_to_breadcrumb` asserts `!dangling`, `!orphan`, breadcrumb present, no surviving off-trunk sibling.
> 3. The 6 result-node LOSS cells flip to `[PAIR,…]` whole-cluster: `matrix_pinned_into_cluster_keeps_whole_cluster` asserts every off-trunk sibling re-appended, `!dangling`, `!orphan`.
> 4. The no-400 invariant holds for all 32 cells + 5 edges (37 total): `matrix_no_dangling_or_orphan_across_all_32_cells` + the 3 edge tests green; `enforce_call_atomic_backstop` (`streaming.rs:130`) UNCHANGED.
> 5. 20 existing `collapse_abandon` tests: 17 unchanged-green + 3 updated (Audit C covers the flips); 8-12 NEW matrix/edge tests green; total 20 → 28-32.
> 6. Kernel-minimality (§3.A): blast radius = 1 fn + 1 caller token; `build_trunk_context` / node-model UNCHANGED; forensic `self.messages` never mutated (`.iter().find()` + `.clone()` only); no public API / persisted-field / migration change; zero consumer leakage.
> 7. `RUSTFLAGS="-Dwarnings"` clippy clean + `fmt --check` (mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked; orchestrator closes at gate-4).
> PASS/FAIL each.

### Audit B — docs + ADR + paperwork (≤ 600 words)
> Read-only. Verify:
> 1. `retain_pinned_calls_on_node` doc-comments (`:738-754` + `:691-709`) rewritten to the (B) 3-case target-aware form (keep-if-on-trunk / off-trunk+tag-on-call-node reclaim / off-trunk+tag-on-result-node re-append-by-membership); the direct-child bullet replaced.
> 2. `concept-brake.md` verified-header refreshed + `:192` note updated (pinned-revert disposition is target-aware: call-node reclaims, result-node keeps the whole cluster; cite ADR-0002).
> 3. `docs/decisions/0002-parallel-cluster-pinned-revert-disposition-b.md` exists, Status Accepted, 7 sections, §D2.1–§D2.4, ≥ 5 Revisit triggers, Verification commands present.
> 4. ADR-0002 §D2.1 states it SUPERSEDES ADR-0001 §D1.2's M=1 re-append + M=1 keep-whole + that the "M=1 byte-identical" claim no longer holds for the pinned-onto-call-node / into-cluster sub-cases (intended, not a regression); §D2.1 documents the 3-clause branch incl. the on-trunk-keep gate; Cross-references cite #80/D-TEST-0075 + ADR-0001 §D1.2 (superseded) + §D1.5 + forward-scope + P0 §3 + §10.
> 5. ADR-0001 §D1.2 carries the `> Superseded-in-part by ADR-0002 §D2.1` inline note + ADR-0001 verified-header bumped.
> 6. phi-core `_cycle-index.md` KC-02 row appended: `Iterations = pending`, `Status = in-flight` (orchestrator owns transitions).
> 7. #80 closure comment posted citing the 32-cell + 5-edge regression matrix + live close-gate; plan archive at `…/kc-02-…/plan.md` exists with cycle hex.
> 8. No K8s ledger (N/A library); no migration; no phi-core CI-guard scripts referenced.
> PASS/FAIL each.

### Audit C — carry-forward regression posture (≤ 600 words)
> Read-only. The (B) fix flips 3 KC-01 carry-forward tests + relies on the on-trunk-keep gate; this audit verifies the regression posture is intact. Verify each with file:line:
> 1. **The 3 KC-01 flips are the ONLY flips** + are the INTENDED semantic change (not collateral): `pinned_class_keeps_cluster_whole_no_dangling_call` (`:2458`) UPDATED — M=1 call-node now `[DROP]`+breadcrumb (the "keep the sealed body whole" assertion INVERTED); `row_pinned_revert_to_call_node_now_call_atomic` (`:3261`) UPDATED — `[PAIR,DROP]` → `[DROP,DROP]`+crumb; `row_pinned_revert_to_first_result_now_call_atomic` (`:3237`) UPDATED — `[PAIR,DROP]` → `[PAIR,PAIR]` (sibling re-appended by membership).
> 2. **On-trunk-keep gate (load-bearing)**: `pinned_cluster_mid_trunk_kept_verbatim` (`:3017`) stays GREEN UNCHANGED — a pinned cluster the model continued PAST (results on-trunk) is kept verbatim; clause (i) preserves it (P0 §10.6 — without the gate this REGRESSES). Confirm the test body was NOT modified.
> 3. **The 3 SAFE KC-01 rows** (`:3285`/`:3304`/`:3322`) + the 13 KC-01-era atomicity tests stay green unchanged.
> 4. No-400 across all 37 cells (32 matrix + 5 edges): `!trunk_has_dangling_call` + `!trunk_has_orphan_result` every cell (P0 F-1 / §10.4); `enforce_call_atomic_backstop` UNCHANGED.
> 5. Full phi-core suite green: lib (20 baseline minus 3 updated + NEW) + the 106 integration tests (`agent_loop_test` 56 / `agent_test` 14 / `session_test` 36, P0 §10.0). Mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked; orchestrator closes at gate-4.
> 6. Live-grid close-gate posture: the §10 post-fix full grid (both drivers × 4 categories, transcript-read) asserts call-node reclaims + result-node keeps-whole + no 400 — confirm the plan §10 close criteria name both dispositions (orchestrator-run; auditor confirms the criteria are stated, not executes them).
> PASS/FAIL each.

---

## §12 — Verification recipe (copy-paste)

```bash
# Build + lint (host cargo, single crate, -j 4, -Dwarnings)
/root/rust-env/cargo/bin/cargo build -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# Targeted regression (the promoted 32-cell matrix + 5 edges + the 3 updated KC-01 flips + on-trunk gate)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture

# On-trunk-keep gate (must stay green — the load-bearing mid-trunk guard)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib pinned_cluster_mid_trunk_kept_verbatim

# Full crate suite (lib + 106 integration)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# Confirm render-pass-only: no public API / persisted-field change; blast radius = 1 fn + 1 caller token
git -C /root/projects/phi/phi-core diff dev --stat -- src/

# Gate-5 cargo-clean (orchestrator, at cycle close)
/root/rust-env/cargo/bin/cargo clean --manifest-path /root/projects/phi/phi-core/Cargo.toml
```

**Blocking unresolved: NONE** (P0 §0 / §10.0). The pre-fix live confirmation + the post-fix full-grid close-gate (§10) are orchestrator-run (live provider round-trip; not bootable at phi-core unit level).

---

## Forks for orchestrator

> ⚠️ **F1 is USER-LOCKED at the BROADER (B) — reclaim-on-call-node + keep-whole-cluster-on-result-node (2026-06-08)** — a product-semantic fork, USER-DIVERGENT from the iter-1 (a)-only draft, re-authored fully here against the P0 §10-validated mechanism. F2 (confirm-uniform, no-op) + F3 (absorb, no split) are TECHNICAL at investigation-rec. Per the v32 USER-DIVERGENT narrow-re-author path: only the F1 subsection (§1) was re-authored; F2/F3 preserved verbatim. Cross-cycle divergence pattern: N/A at the count level (second phi-core cycle; KC-01 was planner-rec-clean) — but this IS a USER-DIVERGENT lock, so iter-2 re-author is the v32-correct route (NOT the P-orch-8 skip). Forward-scope §11 forces `approval=yes` (S2 core-tool correctness) — the gate-1 lock confirms the already-USER-LOCKED F1(B).

### F1 — pinned-revert disposition (load-bearing; USER-LOCKED (B))

| Option | User-visible | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F1.B reclaim-on-call-node + keep-whole-cluster-on-result-node** (USER-LOCKED 2026-06-08) | **User-visible:** disposition is target-aware + complete — seal-past-the-whole-cluster (call-node revert) collapses to ONE durable breadcrumb; revert-INTO-the-cluster (result-node revert) keeps the WHOLE gathered span verbatim (no sibling lost) | fully closes the silent-loss class (all 10 LOSS cells); preserves gathered work on into-cluster reverts; P0-validated end-to-end (0 regression, 0 new 400 across 37 cells); render-pass-only (+32/−13 + 1 caller token); the on-trunk-keep gate keeps mid-trunk clusters verbatim | **Product trajectory:** the stronger guarantee ahead of the MCP/combined-agent cluster (#67/#64/#68) where gathered parallel work is expensive to re-fetch; a future true-sibling node model would only simplify the result-node re-append lookup (Revisit trigger), NOT needed for #80 | **LOCKED** |
| F1.a reclaim-to-breadcrumb (the iter-1 draft) | **User-visible:** the whole abandoned-tail collapses uniformly into ONE breadcrumb on every revert target (drops gathered work even on into-cluster reverts) | lighter (a pure deletion); symmetric with abandon-class; flips only 1 KC-01 test | **Product trajectory:** simpler but DROPS gathered work on result-node reverts (the user judged this the wrong product semantic for "model reverted INTO a cluster wants the span"); superseded by the (B) lock | NOT chosen (superseded 2026-06-08) |

### F2 — abandon-class onto-call-node/result consistency
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F2.confirm-uniform** (investigation-rec) | P0 F-4 / F-5 / §10.4: abandon-class already fully + symmetrically collapses to the breadcrumb; asymmetric mid/last-result cells are benign target-position effects, not leaks; all 16 abandon cells byte-identical pre/post-(B) → no change needed | future regression risk mitigated by the promoted benign-asymmetry regression rows | **REC / lock** |
| F2.adjust | (would change a correct path) | dead code; no defect to fix (P0 F-5) | NOT chosen |

### F3 — KC-03 split decision
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F3.absorb** (investigation-rec) | render-pass-only (`retain_pinned_calls_on_node` + 1 caller token), `+32/−13`; no node-model change; closes the full grid (P0 F-6 / F-8 / §10.6) | the deeper true-sibling node model stays a deferrable (Revisit trigger) | **REC / lock** |
| F3.split (open KC-03 for a node-model change) | "fixes the root cause" (true siblings) | UNNECESSARY for #80 (P0 F-8 / §10.6 — the membership re-append closes the grid without it); reopens the trunk-walk core; over-scoped | NOT chosen |
