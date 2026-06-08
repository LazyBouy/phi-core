<!-- Last verified: 2026-06-08 by Claude Code -->
<!-- KC-01 cycle plan (ARCHIVED at gate-1.5; cycle hex a79c7669; iter-1 — all 3 forks locked planner-rec, skip-condition fired). project=phi-core. -->

# KC-01 — parallel-revert call-atomicity (phi-core kernel)

> **First phi-core kernel chunk.** Closes GitHub #77 / D-TEST-0072 (S2 runtime-correctness: parallel-revert dangling `tool_call`). Design is **LOCKED** (R1–R4) per the #77 design-comment 2026-06-08; the Phase-0.5 P0 investigation (`_p0-investigations/kc-01-…-p0-investigation.md`, 9 DEFINITIVE findings, 0 unresolved) GROUNDED it against live `dev` 0.11.4. This plan formalises the implementation — phases, file-level deliverables, test allocation, audit envelope, §1 locked-fork bodies — and does NOT re-derive the design.
>
> **Project = phi-core** (NOT baby-phi). Root `/root/projects/phi/phi-core`, branch `dev`, crate 0.11.4. HOST cargo (`/root/rust-env/cargo/bin/cargo`), single crate (no `--workspace`), cap `-j 4`. No CI-guard scripts (`scripts/pre-commit` fmt+clippy hook only). phi-core-leverage-check + k8s-readiness-check are **N/A** (kernel does not consume itself / library not daemon). Verified-headers APPLY.

---

## §1 — Locked fork details (per chunk-planner v32 P-plan-1-v32; planner-rec bodies pre-filled at iter-1)

> All three forks are **TECHNICAL** (P0 §6): no user-visible behaviour delta between options — every option closes the same S2 defect and renders the same call-atomic working context. The user perceives only "parallel-tool + pinned-revert no longer 400s" regardless of which option ships. Decided on engineering merit. Per chunk-planner v26 P-plan-1-v26, TECHNICAL forks are released from the User-visible / Product-trajectory framing.

#### F1 = F1.b — ingest-time id-uniqueness shape: `(node_id, tool_call_id)` composite *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** The call↔result join key becomes the composite `(node_id, tool_call_id)` rather than raw `tool_call_id`, riding on the already-session-unique `node_id` minted by `alloc_node_id` (`context.rs:943`). Inside `collapse_abandon_class_cluster` (`context.rs:440`) and the F3 backstop, call↔result matching uses `(parent_node_id, tool_call_id)` equality instead of bare `tool_call_id == node_call_id` (current `context.rs:559`/`:632`/`:640`); an id-less / synthetic call (MockProvider edge, F-8) is handled by a synthetic fallback id minted within the same composite so every call carries a join key. NO new persisted field and NO change to provider id semantics — ids stay exactly as the provider echoes them (F-7); the namespacing is render-internal only.
- **Rationale.** F-8 proved raw `tool_call_id` is NOT session-unique (MockProvider mints `mock-tool-{i}` resetting `i=0` per assistant message — `mock.rs:219` — so `mock-tool-0` recurs every turn and would cross-join a STALE prior-turn result under raw equality), making option (a) "raw + uniqueness assertion" a non-starter that fails in the Mock test path. Option (c) "monotonic internal re-mint replacing provider ids" is the heaviest — it must round-trip a mint↔provider-id map through the wire serializer and risks diverging from what the provider echoes back on the next turn. Option (b) adds zero new state (the `node_id` counter already exists and is monotonic) and is collision-proof by construction.
- **Defers (if chosen).** None deferred for the S2 closure — the composite key fully covers the join uniqueness requirement at this chunk. No persisted-field migration is incurred (the composite is computed at render from existing `node_id` + `tool_call_id`). The synthetic-fallback minting is scoped to render-time id-less calls only; provider id semantics on the wire are untouched and require no downstream consumer change.

#### F2 = F2.document-as-invariant — R3 × prun / decay-drop interaction *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** Add NO separate mechanism for prun/decay-drop vs an R3-kept (pin-wins) node; instead document the non-interaction as an invariant in the `collapse_abandon_class_cluster` doc-comment (`context.rs:426-439` region) + the R3 ADR sub-decision. The R3 mixed-class rule lands at the tag-migration site (`context.rs:577`): if a node carries BOTH an abandon tag migrated from a collapsed sibling result AND a pinned/live sibling call, do NOT collapse — keep the node whole. The documented invariant states pin-wins is self-consistent with both prun and decay-drop today by construction.
- **Rationale.** P0 F2 settled this DEFINITIVELY: (i) `prun` (`apply_prun`, `run.rs:601-655`) operates ONLY on `inrun_context` (the non-revert linear path) and never touches the trunk parent-walk render, so it cannot target an R3-kept trunk node — prun × R3 is a non-interaction on current `dev`; (ii) the reachable decay-drop (`context.rs:608-616`) fires ONLY inside `if abandon_class`, and an R3-kept node by definition carries a pinned/live sibling so it is NOT abandon-class-collapsed → the decay-drop branch is never reached for it. Adding a separate guard mechanism would be dead code guarding an unreachable interaction; pin-wins ("extra context can't 400") is safe-by-construction either way, so the residual future-decay-of-migrated-tag sub-question is non-load-bearing for the S2 closure.
- **Defers (if chosen).** Defers no S2-closure work — the interaction is provably non-reachable today, so no mechanism is needed. The future sub-question (should a FUTURE decay-drop of an abandon tag migrated onto an R3-kept node still keep the node?) is captured as a documented invariant (R3 answer: yes, keep) rather than implemented machinery; if a future chunk introduces a path where decay-drop CAN reach an R3-kept node, that chunk re-opens this as a real fork. No drift filed (non-load-bearing; documented-as-invariant per P0 F2 recommendation).

#### F3 = F3.both — enforcement placement: per-call retain in collapse pass + declarative backstop *(pre-lock draft; finalizes at gate-1)*
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

- **Code-level binding.** Land R1/R2/R3 (per-call retain + per-call collapse + pin-wins) INSIDE `collapse_abandon_class_cluster` (`context.rs:440`) where per-cluster tag/window state + content ownership live, AND add a cheap declarative orphan-filter / empty-node backstop (R4 + F3) as a final linear pass in the `streaming.rs:165-191` render pipeline (slotting after `weave_braking_annotations`/before/around the trunk handoff). The backstop drops any on-trunk `tool_use` whose matching `tool_result` is absent and any empty assistant node (no calls + no text) — a locus-independent invariant guard that also covers any other path assembling a trunk (e.g. `build_trunk_context_with_policy`, exercised at `run.rs:1467`).
- **Rationale.** The collapse pass is the only place that knows the per-cluster decay-window + owns content mutation, so R1/R2/R3 MUST land there — there is no alternative locus for them. A standalone declarative backstop is a cheap (one extra linear pass over the trunk; negligible cost) belt-and-suspenders that guarantees the call-atomic invariant at render regardless of which path built the trunk, closing the risk that a FUTURE trunk-assembly path re-introduces an orphan unguarded (P0 §6 F3 + R10 risk). Single-locus would leave that future-path gap open; the design rec was explicitly "both".
- **Defers (if chosen).** Defers nothing — both loci ship in this chunk. The backstop is additive (a pure filter pass; no state) and does not alter the M=1 single-call path because a well-formed single-call cluster already has its result on-trunk (no orphan to drop, no empty node to remove). The F-6 SAFE rows are the explicit regression guard that the backstop + per-call retain leave the M=1 path byte-equivalent.

**§1 self-check (chunk-planner v32 P-plan-1-v32):** §1 carries a populated `#### F<N> = F<N>.<rec>` subsection for all 3 forks (F1, F2, F3), each with 3-sentence Code-level binding / Rationale / Defers blocks + TECHNICAL FORK label + `*(pre-lock draft; finalizes at gate-1)*` tag. Invoke `chunk-template-validate-locked-appendix` at end-of-draft → expect PASS.

---

## §2 — Concept alignment

| Concept doc | Line(s) | Current statement | KC-01 alignment | Action |
|---|---|---|---|---|
| `docs/concepts/concept-brake.md` | `:1` (verified-header), `:192` (co-keep constraint: "`ToolCall` ↔ `ToolResult` are a bonded pair; the refresh filter must keep them atomically (or drop both)…dropped tool-call blocks orphan their results and the next LLM call fails at the provider layer") | States the bonded-pair invariant at the **filter/refresh** layer in single-call terms; does not address the N-parallel-call cluster | KC-01 makes the render call-atomic so the bonded-pair invariant holds for N>1 parallel calls, not just M=1 | Refresh verified-header + append a note (or §) that the bonded-pair invariant is now enforced per-call at the revert render locus (call-atomic, was node-atomic) |
| `src/types/context.rs` doc-comment | `:426-428` ("The **atomic-cluster invariant** therefore holds for ALL categories: the rendered trunk never carries a tool-call without its matching result (abandon-class drops both; pinned keeps both)") | The invariant is asserted **node-atomically** — "drops both / keeps both" assumes one call per node | KC-01 upgrades this to **call-atomic**: drop/keep PER CALL keyed on `(node_id, tool_call_id)`; a node may now retain some calls and drop others | Rewrite `:426-428` doc-comment to state call-atomicity + the per-call retain/collapse/pin-wins/empty-node rules (R1–R4) |
| `src/types/content.rs` | `:143` (doc), `:85-89` (`Content::ToolCall { id, … }`), `:147-156` (`Message::ToolResult { tool_call_id, … }`) | Documents the native `id` ↔ `tool_call_id` join | KC-01 consumes this join as the call↔result link (no new field) — alignment is confirmatory | No change (the join is already documented + populated; F-7) |

**Concept-fidelity note:** the locked design adds NO new persisted field (F-7) and changes NO provider id semantics (F1.b namespacing is render-internal). The only conceptual shift is node-atomic → call-atomic, localized to the revert render path. `concept-brake.md` is flagged "concept exploration; not yet a binding spec" — the KC-01 amendment is a small accuracy update, not a spec ratification.

---

## §3 — phi-core leverage map → **kernel-minimality surface-discipline** (project=phi-core)

> phi-core-leverage-check is **N/A** — the kernel does not consume itself. Per `[[feedback_phi_core_kernel_minimal]]` and the task directive, §3 instead carries the **kernel-minimality surface-discipline analysis** (the carve-out test the P0 §4 fix-locus table already applied). I cite + ratify that table here.

### §3.A — Kernel-minimality carve-out (cite P0 §4)

| Change (locked R#) | Side | General-primitive? | Consumer-specific leakage? | Blast radius |
|---|---|---|---|---|
| F1.b composite `(node_id, tool_call_id)` join (ingest/render-internal) | kernel | YES — id↔result join uniqueness is a render-correctness property all consumers need; rides on existing `alloc_node_id` (`context.rs:943`) | NONE | render join match-sites in `collapse_abandon_class_cluster` |
| R1 per-call retain (emit only calls whose result is live) | kernel | YES — every consumer's trunk must be call-atomic | NONE | hot working-context build `streaming.rs:165-191`; M=1 path MUST stay byte-identical (F-6 regression guard) |
| R2 per-call collapse (replace whole-node `content.clear()` `:546`/`:575` with id-keyed per-call removal) | kernel | YES — general braking correctness | NONE | `collapse_abandon_class_cluster` only |
| R3 mixed-class → pin wins (`:577` tag-migration site) | kernel | YES — general safe-by-construction rule | NONE | `collapse_abandon_class_cluster` tag-migration site |
| R4 empty-node drop / keep-as-text | kernel | YES — general empty-assistant-400 avoidance | NONE | final render pass in `streaming.rs` pipeline |
| F3 declarative orphan/empty backstop | kernel | YES — general invariant enforcement | NONE | additive final pipeline pass |

**Verdict (cite P0 F-9):** fix-locus is kernel render-pass; NO viable consumer-side fix exists (the working context is built inside `agent_loop`/`streaming.rs` and sent directly; the only consumer surface `transform_context` at `streaming.rs:194` runs AFTER the broken trunk is assembled and would force every consumer to re-implement call-atomic repair — a textbook kernel-correctness property). **Zero i-phi leakage.** Genuine general kernel fix per the `[[feedback_phi_core_kernel_minimal]]` carve-out.

### §3.B — Build / LOC caps + pause-discipline (phi-core; NO K8s axes — N/A library)

> **K8s readiness: N/A — phi-core is a library, not a daemon** (k8s-readiness-check returns N/A for all 7 axes; no `m7b` deferred-ledger). The §3.B table below is the **per-file LOC cap + pause-trigger** table (chunk-planner v23 P-plan-1 functional-scope-derivation + v24 cascade-plumbing weights).

| File | Surface | LOC-cap derivation (per-axis) | Cap | Pause @ (1.5×, re-derived) |
|---|---|---|---|---|
| `src/types/context.rs` | R1/R2/R3 per-call retain+collapse+pin-wins inside `collapse_abandon_class_cluster` (rewrite of `:544-651` whole-node logic to id-keyed per-call) + F1.b composite-key match-sites + doc-comment `:426-428` rewrite | per-call retain rewrite ~60 LOC (iterate `Content::ToolCall` not first-only + sibling-preserving removal) + per-call collapse ~40 LOC + R3 mixed-class branch ~25 LOC + composite-key threading ~20 LOC + doc-comment ~25 LOC + 30% slack | **≤ 230 LOC net production** | 345 |
| `src/types/context.rs` (inline tests) | promoted regression tests in `collapse_abandon_class_cluster_tests` module: 5 matrix rows + R4 empty-node + R3 pin-wins + a parallel `two_call`/`multi_call` fixture | NEW fixture ~40 LOC (parallel-call node builder) + 7 tests × ~25 LOC + framework allowance (chunk-planner v33 P-plan-1-v33: `AgentContext` construction boilerplate) | **≤ 240 LOC inline-test** | 360 |
| `src/agent_loop/streaming.rs` | R4 empty-node + F3 declarative orphan-filter backstop pass in the `:165-191` pipeline | backstop filter ~35 LOC (orphan-drop + empty-node-drop linear pass) + wiring ~10 LOC + 30% slack | **≤ 60 LOC** | 90 |

**Pause-discipline (cascade-band):** the R1 per-call retain rewrite touches the HOT working-context build (R10 risk). Predicted production LOC band `[150, 230]`; per chunk-planner v25 P-plan-3 latent-defect cushion (NEW-correctness rewrite of a load-bearing render pass), apply ~2× cushion → **effective pause-threshold = 345 LOC (≤230 cap × 1.5)**. If `context.rs` net production exceeds 345 LOC OR `streaming.rs` exceeds 90 LOC, implementer PAUSES + AskUserQuestion (chunk-implementer v11 pause-discipline). **No cross-file struct-cascade predicted** — the change is confined to two render functions; no public API signature change (F-7); no new persisted field; no migration.

### §3.C — Inline-test cardinality (chunk-planner v27 P-plan-1-v27)

`collapse_abandon_class_cluster` is not enum-dispatch but a matrix-coverage surface. Inline tolerance derivation: 5 matrix rows (2 BROKEN-now-flip-to-SAFE + 3 SAFE-stay-SAFE regression) + 1 R4 empty-node + 1 R3 mixed-class pin-wins = **7 NEW inline tests** (band `[7, 9]` allowing ≤ 2 incidental edge-case tests the implementer adds for the composite-key / synthetic-fallback paths). Baseline: 13 existing `collapse_abandon` lib tests (P0 §7) MUST stay green.

### §3.D — Forward-scope ↔ concept-doc precedence

No closed-set / fixed-order / frozen-schema contradiction. The native `id ↔ tool_call_id` join is an existing documented primitive (`content.rs:143`); KC-01 consumes it, does not extend a closed vocabulary. `N/A — no contradiction to surface`.

### §3.E — Anticipated gate-2.5 candidates (chunk-planner v13)

- **Parallel-call test fixture centralization.** A NEW parallel `multi_call_node` (N tool_calls in one assistant node) + matching N result nodes fixture must be authored (no `two_call`/parallel fixture exists today; only single-call `tool_call_node`/`result_node` at `context.rs:1876`/`:1908`). Options: (a) add the parallel fixture to the existing `collapse_abandon_class_cluster_tests` module helpers; (b) factor a shared fixture module. **Planner-rec: (a)** at this chunk (single consuming module; ≤ 1 fixture). If the live close-gate or a future chunk needs the same parallel fixture, flip to (b).
- **Synthetic-id fallback shape.** F1.b needs a synthetic id for id-less calls. If the implementer finds id-less calls never reach the render locus in practice (all ingest sites copy a real id per `tools.rs`, F-7), the fallback may be a defensive `unreachable!`-guard or a deterministic `format!("synth-{node_id}-{idx}")`. **Planner-rec: deterministic synthetic id** (collision-proof under the composite; no panic path). Surface at P1 if the ingest walk shows id-less calls are genuinely unreachable.

---

## §4 — Drifts / issues closed

- **GitHub #77 / D-TEST-0072 (S2)** — parallel-revert dangling `tool_call`. **Primary + only deliverable target.** Status flip at this cycle: the GitHub issue moves to closed at chunk-seal (orchestrator-run live close-gate + unit regression green). No phi-core drift-file ledger exists (first kernel cycle); the issue IS the tracking artifact. P-SEAL deliverable: post the closure comment on #77 citing the promoted regression tests + the live transcript-read close-gate result.
- **No other drifts** touched. No prior-cycle closures to ratify (first phi-core cycle).

---

## §5 — ADR draft

> **No `decisions/` directory exists in phi-core docs** (verified: `find docs -type d -name decisions` → empty; first kernel cycle). KC-01 **creates the phi-core ADR home**: `docs/decisions/0001-parallel-revert-call-atomicity.md`. Numbering starts at `0001` (phi-core ADR-0001). Cross-milestone path-prefix discipline (chunk-planner v6) is N/A — single flat `docs/decisions/` for phi-core; cite by `ADR-0001 §D1.<M>`.

**Proposed: phi-core ADR-0001 — Parallel-revert call-atomicity (call-atomic render).** Status: Accepted. The implementer authors all 7 canonical ADR top-level sections (chunk-planner v17 explicit-enumeration):

1. `## Forks` — header table: F1 (id-uniqueness shape → F1.b composite) / F2 (R3 × prun/decay-drop → document-as-invariant) / F3 (enforcement placement → both loci). All three TECHNICAL, planner-rec, gate-1-locked.
2. `## Context` — #77 / D-TEST-0072 S2 gap; P0 §2 surface map; the node-atomic → call-atomic shift.
3. `## Sub-decisions` —
   - `### §D1.1 — Join key = `(node_id, tool_call_id)` composite (resolves F1)`. *Net-new surface* (chunk-planner v24 P-plan-2-v24 never-shipped narrative): no prior behaviour to preserve — the composite-key join is i-phi-net-new... no, phi-core-net-new at KC-01; the raw `tool_call_id` join existed but was not session-namespaced.
   - `### §D1.2 — R1 per-call retain`. Pre-existing-behaviour: `collapse_abandon_class_cluster` shipped whole-node `content.clear()` (`:546`/`:575`) at CC-18/#73; KC-01 replaces with id-keyed per-call removal. The M=1 single-call path is preserved byte-identical (F-6 SAFE rows guard it).
   - `### §D1.3 — R2 per-call collapse`. Pre-existing-behaviour: whole-node clear `:546`/`:575`; KC-01 collapses only the matched call, breadcrumbs it, leaves sibling calls intact.
   - `### §D1.4 — R3 mixed-class → pin wins (`:577`)`. Pre-existing-behaviour: tag-migration unconditionally migrated tags onto parent; KC-01 keeps a node whole when it carries both an abandon tag and a pinned/live sibling.
   - `### §D1.5 — R4 empty-node handling`. Net-new: empty-assistant-node drop / keep-as-text is a new render rule.
   - `### §D1.6 — F2 documented non-interaction invariant (resolves F2)`. R3 × prun/decay-drop is provably non-reachable today (P0 F2); documented-as-invariant, no mechanism added.
   - `### §D1.7 — F3 both-loci enforcement (resolves F3)`. Per-call retain in collapse pass + declarative backstop in `streaming.rs` pipeline.
4. `## Cross-references` — (a) `concept-brake.md:192` + `context.rs:426-428`; (b) closed drifts: #77 / D-TEST-0072; (c) prior ADRs: none (first phi-core ADR); (d) forward-scope row `kc-01-parallel-revert-call-atomicity.md`.
5. `## Consequences` — `### For consumers (i-phi / baby-phi / future)`: removes S2 hard-fail for any consumer firing parallel tool calls while braking; hardens braking ahead of MCP / combined-agent clusters (#67/#64/#68 — forward-routing note).
6. `## Revisit triggers` — (i) a FUTURE path makes decay-drop reach an R3-kept node (re-opens §D1.6); (ii) a provider id scheme makes the composite key insufficient (re-opens §D1.1); (iii) a new trunk-assembly path bypasses both F3 loci (re-opens §D1.7); (iv) per-call retain shows an M=1 regression (re-opens §D1.2).
7. `## Verification` — `cargo test -j 4 --manifest-path … --lib collapse_abandon` (regression rows) + clippy `-Dwarnings` + `fmt --check` + the live transcript-read close-gate.

---

## §6 — Prior-chunk regression / carry-forward invariants

First phi-core cycle → no prior KC chunk. Carry-forward invariants are the **existing braking-machinery invariants** the render rewrite must NOT regress:

| Invariant | Verifying command | Expected |
|---|---|---|
| 13 existing `collapse_abandon` lib tests stay green (M=1 single-call path byte-identical) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon` | all pass (baseline 13 → 20 after +7 NEW) |
| `build_trunk_context` linear parent-walk unchanged (F-1; sole `parent_id` consumer) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path … --lib build_trunk_context_tests` | green |
| `apply_revert` marks intent only; forensic `messages` log never mutated (P0 §2, `run.rs:709` doc `:685-688`) | grep `self.messages` mutation in render path = none; `cargo test … --lib node_id_alloc_tests` | green |
| Full phi-core suite green (no cross-module regression from the render change) | `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path …` | green |
| `decay_tags_by_policy` 2nd-pass + `weave_braking_annotations` + `inject_continue_after_revert` pipeline order preserved (`streaming.rs:165-191`) | read diff; pipeline order unchanged, backstop slots after weave | order intact |

**Back-compat preservation (chunk-planner v19 P2):** no divergent fork removes a load-bearing scaffold — F1/F2/F3 all locked at planner-rec; the M=1 path is preserved (not removed) per R1/R2 design. `N/A — no scaffold removal`.

---

## §7 — Phase plan

> **§7.0 phase-order stress-test (chunk-planner v25 P-plan-4-v25).** 5 phases; 0 USER-DIVERGENT forks predicted (all 3 TECHNICAL planner-rec); no cascade-band overlap between adjacent phases. Compile-time: each phase ends GREEN (`cargo build` + `clippy`); no struct/trait signature change mid-cycle (F-7, no public API change) → no RED window from type churn. Runtime-test: P1 (join-key + R1/R2) may transiently break a BROKEN-row assertion until P2 lands R3/R4 — but the BROKEN-row tests are NEW (authored at P3), so no existing test goes RED in P1/P2. P3 authors the regression tests AFTER R1–R4 land → tests are GREEN-on-arrival. **No RED window across any phase boundary.** Single-locus per phase; diff-coherent for a 2-auditor Medium envelope.

### P0 — Read + ground (no code)
- **Goal:** implementer reads §9 reading list; confirms P0 facts at live line numbers (re-verify if drift since plan-draft).
- **Deliverables:** confirmation note; no diff.
- **Tests:** baseline `cargo test … --lib collapse_abandon` = 13 green.
- **Confidence:** 10/10. **Pause-discipline:** none.

### P1 — Join-key (F1.b) + R1 per-call retain + R2 per-call collapse
- **Goal:** thread the `(node_id, tool_call_id)` composite key; rewrite the whole-node `content.clear()` (`:546`/`:575`) + pinned re-append (`:628-646`) to id-keyed per-call retain/collapse; add the synthetic-id fallback.
- **Deliverables:** `src/types/context.rs` — composite-key match-sites; R1 per-call retain (emit only calls whose result is live; drop only the collapsing result, never siblings); R2 per-call collapse (breadcrumb the matched call, leave siblings intact).
- **Tests:** existing 13 stay green (M=1 path byte-identical); no NEW tests yet.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if `context.rs` net production > 345 LOC (§3.B cushion-adjusted).

### P2 — R3 mixed-class pin-wins + R4 empty-node + F3 backstop
- **Goal:** add the R3 mixed-class branch at the `:577` tag-migration site (keep whole if abandon-tag + pinned/live sibling); add R4 empty-node handling + the F3 declarative orphan/empty backstop in `streaming.rs:165-191`.
- **Deliverables:** `src/types/context.rs` R3 branch; `src/agent_loop/streaming.rs` backstop pass (drop on-trunk `tool_use` with absent `tool_result`; drop empty assistant node, keep-as-text if text present).
- **Tests:** existing 13 stay green.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if `streaming.rs` > 90 LOC.

### P3 — Promote regression tests
- **Goal:** promote the P0 §7 throwaway 5-row matrix to in-tree regression tests in `collapse_abandon_class_cluster_tests` (`context.rs:1855`); add the parallel `multi_call` fixture + the R4 empty-node + R3 pin-wins tests.
- **Deliverables:** 7 NEW `#[test]` fns (see §8) + 1 NEW parallel-call fixture; using existing `trunk_has_dangling_call` helper (`context.rs:2021-2043`, VERIFIED present) + `two_call`-style construction.
- **Tests:** both BROKEN rows now `dangling=[]`; 3 SAFE rows stay green; R4 + R3 pass. Total `collapse_abandon` lib tests 13 → 20.
- **Confidence:** 9/10. **Pause-discipline:** PAUSE if inline-test > 360 LOC.

### P-SEAL — Docs + ADR + paperwork
- **Goal:** rewrite the `context.rs:426-428` atomic-cluster doc-comment to call-atomic; update `concept-brake.md:192` + verified-header; create `docs/decisions/0001-parallel-revert-call-atomicity.md`; mint the phi-core `_cycle-index.md` row (first row); post the #77 closure comment.
- **Deliverables:** doc-comment rewrite; concept-brake amendment; ADR-0001 (7 sections); cycle-index row (leave `Iterations = pending`, `Status = in-flight` — orchestrator owns transitions per chunk-planner v16 P-SEAL lifecycle); #77 closure note.
- **Tests:** `fmt --check` + `clippy -Dwarnings` + full suite green.
- **Confidence:** 9/10. **Pause-discipline:** none.

---

## §8 — Tests summary

> Baseline (P0 §7, VERIFIED): 13 existing `collapse_abandon` lib tests pass on `dev` 0.11.4. Full-crate baseline test count is the phi-core suite (`cargo test --no-run` total); KC-01 adds NO test file (inline-only in `context.rs`).

**Per-Tier NEW MUST-SHIP (chunk-planner v22 P1):**

| Tier | Test | Asserts |
|---|---|---|
| A (BROKEN→SAFE flip) | `row_pinned_revert_to_first_result_now_call_atomic` | revert→result_a, tag=Outcome → `dangling=[]`, `orphans=[]` (was `dangling=["call_b"]`, F-4) |
| A (BROKEN→SAFE flip) | `row_pinned_revert_to_call_node_now_call_atomic` | revert→call node, tag=Outcome → `dangling=[]` (was `dangling=["call_b"]`, F-5) |
| B (SAFE regression guard) | `row_abandon_revert_to_first_result_stays_safe` | Failure/Lesson → `dangling=[] orphans=[]` (F-6) |
| B (SAFE regression guard) | `row_abandon_revert_to_call_node_stays_safe` | Failure/Lesson → `dangling=[]` (F-6) |
| B (SAFE regression guard) | `row_revert_to_last_result_stays_safe` | Outcome, both results on-trunk → `dangling=[] orphans=[]` (F-6) |
| C (R4) | `r4_empty_node_after_per_call_retain_is_dropped` | per-call retain leaving 0 calls + no text → node dropped (no empty-assistant 400); has text → kept as plain text |
| C (R3) | `r3_mixed_class_pin_wins_keeps_node_whole` | node with abandon tag (migrated) + pinned/live sibling call → NOT collapsed, kept whole, no dangle |
| **Total NEW MUST-SHIP** | **7** | |

**Tolerance band:** `[7, 9]` — 7 MUST-SHIP + ≤ 2 incidental inline tests the implementer may add for the composite-key uniqueness / synthetic-fallback edge paths. **Expected `collapse_abandon` lib test delta: +7 (13 → 20).** Workspace/crate delta: **+7** (no new test file; inline in `context.rs`). NEW parallel `multi_call` fixture is a helper (not a `#[test]`), authored at P3.

**Re-run command (P0 §7):** `/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture`

---

## §9 — Pre-chunk gate (reading list + carry-forward)

Implementer MUST read before P1:
1. This plan + the forward-scope (`docs/specs/plan/forward-scope/kc-01-parallel-revert-call-atomicity.md`).
2. P0 investigation (`docs/specs/plan/build/_p0-investigations/kc-01-…-p0-investigation.md`) — §2 surface map (file:line), §3 findings F-1..F-9, §4 fix-locus, §5 ruled-out, §6 forks, §7 repro assets, §8 close-gate.
3. `src/types/context.rs` — `collapse_abandon_class_cluster` (`:426-656` doc-comment + body), `alloc_node_id` (`:943`), `trunk_has_dangling_call` (`:2021-2043`), test module (`:1855` + fixtures `:1876`/`:1908`).
4. `src/agent_loop/streaming.rs:160-198` (render pipeline).
5. `src/agent_loop/run.rs` — `stamp_node_identity:571-597`, `apply_revert:709` (+ User-refusal guard `:752-762`).
6. `src/types/content.rs:85-89`/`:143`/`:147-156` (join key) + `src/types/node_tag.rs:112-117`/`:97-100` (TagKind / RevertCategory mapping) + `src/provider/mock.rs:219` (Mock id, F-8).
7. `docs/concepts/concept-brake.md:192` (co-keep constraint).

**Carry-forward invariants:** §6 table (13 existing tests + linear walk + no-log-mutation + full suite green).

---

## §10 — Close criteria

**Code-aspect:**
- R1–R4 + F1.b composite key + F3 backstop land at the loci in §1.
- 7 NEW regression tests green; both BROKEN rows flip to `dangling=[]`; 3 SAFE rows stay green; R4 + R3 pass.
- 13 existing `collapse_abandon` tests stay green (M=1 byte-identical).
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check` clean.
- No public API signature change; no new persisted field; no migration (F-7).

**Docs-aspect:**
- `context.rs:426-428` doc-comment rewritten node-atomic → call-atomic.
- `concept-brake.md` verified-header + co-keep note refreshed.
- `docs/decisions/0001-parallel-revert-call-atomicity.md` Accepted with §D1.1–§D1.7.
- phi-core `_cycle-index.md` first row minted.
- #77 closure comment posted (cites tests + live close-gate).

**Live close-gate (orchestrator-run, per `[[feedback_render_transcript_close_gate]]`):** an i-phi HTC with a ≥2-parallel-tool turn (e.g. `memory_help` + `skill_help`, CC-23 HTC-0009 shape) + a pinned `completion`/`step-summary` `revert_to_state` landing into the cluster — render + READ the transcript: provider request carries exactly N `tool_use` ↔ N `tool_result` blocks (or the cluster collapses atomically) and SENDS without an Anthropic/OpenAI 400. **Not bootable at phi-core unit level** (needs a live provider round-trip); orchestrator runs it at close.

**Implementation confidence target: 9/10** (`claims-honored / claims-in-scope` ≥ 9/10).

---

## §11 — Audit plan

**audit-envelope-size:** Phase count = 5 (P0, P1, P2, P3, P-SEAL) → **Medium (2 auditors: A + B).** Confirms forward-scope §8 hint. **Rationale for NOT bumping to Large:** single-subsystem kernel fix; no cross-crate surface (single crate, no `--workspace`); no migration; P0 §4 confirmed NO unexpected call-site cascade in the render path (change confined to 2 functions in 2 files); no public API change (F-7). The §10 "bump to Large only if the investigation surfaces an unexpectedly wide call-site cascade" trigger did NOT fire.

### Audit A — code-correctness + tests (≤ 600 words)
> Read-only on source. Plan at `…/kc-01-…/plan.md`. Verify each with file:line:
> 1. R1 per-call retain lands in `collapse_abandon_class_cluster` (`context.rs:~440`): emits only calls whose result is live; drops only the collapsing result, never live siblings.
> 2. R2 per-call collapse replaces whole-node `content.clear()` (former `:546`/`:575`) with id-keyed per-call removal.
> 3. R3 mixed-class pin-wins branch at the tag-migration site (former `:577`): node with abandon tag + pinned/live sibling kept whole.
> 4. R4 empty-node + F3 backstop in `streaming.rs:165-191`: orphan `tool_use` dropped; empty assistant node dropped / kept-as-text.
> 5. F1.b composite `(node_id, tool_call_id)` join at match-sites; synthetic-id fallback present; NO new persisted field; NO provider id-semantics change (F-7).
> 6. 7 NEW tests green; both BROKEN rows `dangling=[]`; 3 SAFE rows green; R4 + R3 pass; 13 existing tests green; full suite green at +7.
> 7. `RUSTFLAGS="-Dwarnings"` clippy clean + `fmt --check` (mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked; orchestrator closes at gate-4).
> 8. Kernel-minimality (§3.A): zero consumer-specific leakage; M=1 path byte-identical; no public API signature change.
> PASS/FAIL each.

### Audit B — docs + ADR + paperwork (≤ 600 words)
> Read-only. Verify:
> 1. `context.rs:426-428` doc-comment rewritten node-atomic → call-atomic (per-call retain/collapse/pin-wins/empty-node stated).
> 2. `concept-brake.md` verified-header refreshed + co-keep `:192` note updated to call-atomic.
> 3. `docs/decisions/0001-parallel-revert-call-atomicity.md` exists, Status Accepted, 7 sections, §D1.1–§D1.7, Revisit triggers present (≥ 4), Verification commands present.
> 4. ADR Cross-references cite #77/D-TEST-0072 + concept-brake + forward-scope row; Consequences carries `### For consumers` forward-routing (#67/#64/#68).
> 5. phi-core `_cycle-index.md` first row minted: `Iterations = pending`, `Status = in-flight` (orchestrator owns transitions).
> 6. #77 closure comment posted citing regression tests + live close-gate.
> 7. Plan archive at `…/kc-01-…/plan.md` exists with cycle hex.
> 8. No K8s ledger (N/A library); no migration; no phi-core CI-guard scripts referenced.
> PASS/FAIL each.

---

## §12 — Verification recipe (copy-paste)

```bash
# Build + lint (host cargo, single crate, -j 4, -Dwarnings)
/root/rust-env/cargo/bin/cargo build -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# Targeted regression (the promoted matrix + R4 + R3)
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib collapse_abandon -- --nocapture

# Full crate suite
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# Confirm no public API / persisted-field change + no new use-sites cascade
git -C /root/projects/phi/phi-core diff dev --stat -- src/

# Gate-5 cargo-clean (orchestrator, at cycle close)
/root/rust-env/cargo/bin/cargo clean --manifest-path /root/projects/phi/phi-core/Cargo.toml
```

**Blocking unresolved: NONE** (P0 §9). The live transcript-read close-gate (§10) is the only non-unit gate; it is orchestrator-run (needs a live provider round-trip; not bootable at phi-core unit level).

---

## Forks for orchestrator

> ⚠️ All 3 forks are **TECHNICAL** (no user-visible delta) and recommended at planner-rec — design is LOCKED (R1–R4) per #77 + GROUNDED by P0. The user lock at gate-1 is confirmatory (forward-scope §11 forces `approval=yes`: first phi-core kernel chunk, load-bearing braking machinery). Cross-cycle divergence pattern: N/A (first phi-core cycle; no prior phi-core fork-lock history).

### F1 — ingest-time id-uniqueness shape
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| F1.a raw `tool_call_id` + uniqueness assertion | minimal | **non-starter** — F-8: Mock `mock-tool-{i}` collides every turn → assertion FAILS in Mock test path | NOT chosen |
| **F1.b `(node_id, tool_call_id)` composite** (planner-rec) | rides existing session-unique `node_id` (`alloc_node_id:943`); zero new state; collision-proof by construction; synthetic-fallback handles id-less calls | composite match at render (negligible) | **REC / lock** |
| F1.c monotonic internal mint replacing provider ids | fully unique | heaviest — round-trips mint↔provider-id map through wire serializer; risks diverging from provider echo | NOT chosen |

**Recommendation: F1.b** — P0 §6 leans (b); rides on existing infra, zero new persisted field.

### F2 — R3 × prun / decay-drop interaction
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F2.document-as-invariant** (planner-rec) | P0 F2: prun touches only `inrun_context`; decay-drop fires only inside `if abandon_class` → never reaches an R3-kept node; non-interaction by construction; no dead-code guard | future-path sub-question stays a documented invariant | **REC / lock** |
| F2.add-separate-mechanism | defensive against a future decay-reaches-R3 path | dead code guarding an unreachable interaction today; over-engineering for S2 closure | NOT chosen |

**Recommendation: F2.document-as-invariant** — provably non-reachable today (P0 F2); pin-wins safe-by-construction.

### F3 — enforcement placement
**TECHNICAL FORK** (no user-visible delta — pick on engineering merit only).

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F3.both** (planner-rec) | R1/R2/R3 in collapse pass (only locus with per-cluster tag/window state) + cheap declarative backstop in `streaming.rs` pipeline; guards future trunk-assembly paths (e.g. `run.rs:1467` shape) | one extra linear pass (negligible) | **REC / lock** |
| F3.single-locus | slightly less code | future trunk-assembly path could re-introduce an orphan unguarded | NOT chosen |

**Recommendation: F3.both** — design rec was "both"; belt-and-suspenders for a load-bearing invariant.
