<!-- Last verified: 2026-06-08 by Claude Code -->

# P0 investigation — KC-02 (phi-core kernel)

> Closes GitHub #80 / D-TEST-0075 (S2). Characterize-first: the exhaustive
> `category × revert-target × arity` keep/drop/balance grid, driven through the
> EXACT production render pipeline via a throwaway deterministic unit matrix
> (now reverted; tree clean). The matrix table in §3 is the load-bearing
> deliverable — the planner locks F1/F2/F3 against it.

## §0 — Verdict summary

- **Findings: 9 definitive, 0 unresolved.**
- **Fix-locus**: render-pass kernel-only (`collapse_abandon_class_cluster` / `retain_pinned_calls_on_node`, `types/context.rs`). NO node-model change required for option (a) or (b); `build_trunk_context` (`context.rs:1106`) remains the sole `parent_id` trunk-walk consumer (KC-01 F-1 re-confirmed). Zero consumer leakage; no public-API / persisted-field / migration change.
- **Forks surfaced for the planner: 3** (F1 pinned-onto-call-node semantic; F2 abandon-class consistency; F3 KC-03 split decision).
- **Non-viable approaches ruled out: 2.**
- **Unresolved / needs-live-repro**: none for the static grid. ONE recommended pre-fix live confirmation (§8) — NOT a blocker; it confirms a real-world *wasted-re-fetch* consequence, since the matrix proves there is **no hard 400** to confirm.

**Headline counts (the answer to #80's hunch):**
- **BROKEN-400 cells: 0** — the #80 hunch of hidden hard 400s does **NOT** realize. `enforce_call_atomic_backstop` (`streaming.rs:130`) catches every dangling-call / orphan-result across all 32 cells. Balance is `OK` everywhere.
- **ASYMMETRIC cells: 14** (of 32). Two distinct classes — see §3 F-4 (benign, target-position-driven) vs F-5 (the #80 defect, pinned keep-first/drop-rest).
- **LOSS cells: 8** — all are the pinned (completion/step-summary) keep-first/drop-rest cells where gathered sibling work is dropped and only a *decaying* breadcrumb remains (effectively lost for a pinned category that promised "keep the span").

## §1 — Questions that gate planning

1. Does any `category × target × arity` cell produce a **hard provider 400** today (dangling tool_call or orphaned tool_result) through the full production render path? (The #80 hunch.)
2. Across the grid, where is the disposition **ASYMMETRIC** (keep-first/drop-rest), and is each asymmetry a *stamping artifact* (#80) or a *benign target-position* effect?
3. Where is there **silent loss** — a tool_result dropped without being reclaimed into a surviving breadcrumb (or reclaimed only into a *decaying* breadcrumb for a *pinned* category)?
4. Is the **abandon-class** (failure/tangent) disposition uniform + correct (whole cluster reclaimed to a decaying breadcrumb) across every target?
5. For the **pinned-onto-call-node** cells, what would option **(a) reclaim-to-breadcrumb** vs **(b) pin-whole-cluster** each concretely produce, and which currently-LOSS cells does each fix?
6. Is the fix **render-pass-only** (no node-model change), confirming KC-01 F-1's blast-radius bound?
7. Is any found issue **separable / large enough** to route to a KC-03 split?

## §2 — Current-surface map

**Production render pipeline (revert mode), `agent_loop/streaming.rs:215-262`** — exact order the matrix mirrors:
1. `context.build_trunk_context()` (`context.rs:1074`) — linear `parent_id` walk from `active_node_id` to root. **Sole `parent_id` consumer** (re-grep-confirmed; see F-6). Parallel results chain LINEARLY `result_a.parent=call_node, result_b.parent=result_a, …` (KC-01 F-1) — so reverting onto the call-node yields trunk `[n0,n1]` with only result_a a *direct child* of n1.
2. `collapse_abandon_class_cluster(trunk, policy, turn)` (`context.rs:462`) — the persistent scan. Abandon-class branch: call-node `:563-587` (`content.clear()` when no live sibling / per-call `retain` when live sibling), result-node `:588-668` (move tag to parent, remove result, R3 pin-wins). Pinned branch `:691-731` → `retain_pinned_calls_on_node`.
3. `retain_pinned_calls_on_node(trunk, idx)` (`context.rs:755`) — per-call: keep if result on-trunk; re-append if result is a **direct child** of THIS node (`lm.parent_id == Some(this_node_id)`, `:804`); **DROP** otherwise. The direct-child gate is what makes pinned-onto-call-node keep result_a and drop result_b/c (the #80 root, F-5).
4. `decay_tags_by_policy` (`:303`) → `weave_braking_annotations` (`:892`) → `inject_continue_after_revert` (`:973`).
5. `enforce_call_atomic_backstop(trunk)` (`streaming.rs:130`) — final declarative pass: orphan-filter (drop any call whose result is absent anywhere on trunk) + R4 empty-node drop. **This is why no cell 400s** (F-1).

**Category→TagKind→class** (`node_tag.rs:95-100, :123`): Failure→Lesson (decayable=abandon), Tangent→Finding (abandon), Completion→Outcome (pinned), StepSummary→Checkpoint (pinned).

**Test scaffolding reused** (`collapse_abandon_class_cluster_tests`, `context.rs:2069`): `multi_call_node` (`:3175`, arity-2), `parallel_fixture` (`:3213`, linear `n0→n1→n2→n3`), `result_node`, `tag_message`, `trunk_has_dangling_call` (`:2235`), `trunk_has_tool_result` (`:2362`), `trunk_has_breadcrumb` (`:2199`). The matrix added arity-3 + post-cluster fixtures + an orphan-result detector + the full-pipeline driver, then reverted.

## §3 — Findings

### The matrix (load-bearing deliverable)

Every cell driven through `build_trunk_context → collapse_abandon_class_cluster → decay_tags_by_policy → weave → inject → enforce_call_atomic_backstop` (the verbatim production pipeline). `breadcrumb=true` in every cell. Disposition legend: `PAIR` = call+result both present (atomic); `DROP` = call+result both gone; `DANGLE`/`ORPHAN` = imbalance (none occurred).

| category | target | arity | balance | symmetry | loss | observed disposition |
|---|---|---|---|---|---|---|
| failure | call-node | 2 | OK | SYMMETRIC | ok | [DROP,DROP] → whole cluster→crumb |
| failure | first-result | 2 | OK | SYMMETRIC | ok | [DROP,DROP] |
| failure | last-result | 2 | OK | ASYMMETRIC | ok | [PAIR,DROP] (a live, b=tip abandoned→crumb) |
| failure | post-cluster | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] cluster fully upstream, untouched |
| tangent | call-node | 2 | OK | SYMMETRIC | ok | [DROP,DROP] |
| tangent | first-result | 2 | OK | SYMMETRIC | ok | [DROP,DROP] |
| tangent | last-result | 2 | OK | ASYMMETRIC | ok | [PAIR,DROP] |
| tangent | post-cluster | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] |
| **completion** | **call-node** | **2** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP] — #80: a kept, b silently dropped** |
| **completion** | **first-result** | **2** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP]** |
| completion | last-result | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] both live→kept |
| completion | post-cluster | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] |
| **step-summary** | **call-node** | **2** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP]** |
| **step-summary** | **first-result** | **2** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP]** |
| step-summary | last-result | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] |
| step-summary | post-cluster | 2 | OK | SYMMETRIC | ok | [PAIR,PAIR] |
| failure | call-node | 3 | OK | SYMMETRIC | ok | [DROP,DROP,DROP] |
| failure | first-result | 3 | OK | SYMMETRIC | ok | [DROP,DROP,DROP] |
| failure | mid-result | 3 | OK | ASYMMETRIC | ok | [PAIR,DROP,DROP] (a live, b=tip + c tail→crumb) |
| failure | last-result | 3 | OK | ASYMMETRIC | ok | [PAIR,PAIR,DROP] (a,b live, c=tip→crumb) |
| tangent | call-node | 3 | OK | SYMMETRIC | ok | [DROP,DROP,DROP] |
| tangent | first-result | 3 | OK | SYMMETRIC | ok | [DROP,DROP,DROP] |
| tangent | mid-result | 3 | OK | ASYMMETRIC | ok | [PAIR,DROP,DROP] |
| tangent | last-result | 3 | OK | ASYMMETRIC | ok | [PAIR,PAIR,DROP] |
| **completion** | **call-node** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP,DROP] — #80 generalizes to arity 3: a kept, b+c dropped** |
| **completion** | **first-result** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP,DROP]** |
| **completion** | **mid-result** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,PAIR,DROP] (c past the pin tip)** |
| completion | last-result | 3 | OK | SYMMETRIC | ok | [PAIR,PAIR,PAIR] all live→kept |
| **step-summary** | **call-node** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP,DROP]** |
| **step-summary** | **first-result** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,DROP,DROP]** |
| **step-summary** | **mid-result** | **3** | OK | **ASYMMETRIC** | **LOSS** | **[PAIR,PAIR,DROP]** |
| step-summary | last-result | 3 | OK | SYMMETRIC | ok | [PAIR,PAIR,PAIR] |

### Numbered findings

**F-1 (DEFINITIVE) — NO cell produces a hard 400. The #80 "hidden hard 400" hunch does NOT realize.** All 32 cells render `balance=OK` (zero dangling tool_calls, zero orphaned tool_results). Evidence: the matrix `balance` column is `OK` in every row; `enforce_call_atomic_backstop` (`streaming.rs:130-174`) orphan-filters any call whose result is absent and drops empty nodes, AFTER `retain_pinned_calls_on_node` has dropped abandoned siblings. The backstop is locus-independent and catches every imbalance the per-call logic might leave. The defect is **silent loss, not provider rejection.** DEFINITIVE.

**F-2 (DEFINITIVE) — the #80 pinned keep-first/drop-rest defect generalizes across BOTH pinned categories AND both arities.** 8 LOSS cells: `completion`/`step-summary` × `call-node`/`first-result` (arity 2 + 3) + `mid-result` (arity 3). In every one, `result_a` survives as a PAIR while `result_b`(/`c`) is dropped purely because only `result_a` is a *direct child* of the call-node under linear stamping (`retain_pinned_calls_on_node:802-815`). The kept/dropped split is a **stamping artifact, not intent** — exactly #80 §1, now proven not specific to `completion` or to deepseek. DEFINITIVE (matrix rows 9,10,13,14,25,26,27,29,30,31).

**F-3 (DEFINITIVE) — silent loss is reclaimed only into a DECAYING breadcrumb, which is insufficient for a PINNED category.** In all 8 LOSS cells `breadcrumb=true`, but the breadcrumb carries the *pinned tag's* text "reverted past: parallel cluster" — and the dropped sibling's actual *result content* (e.g. `BODY-B`) is gone, not summarized. For abandon-class that is correct (the work was abandoned). For pinned (completion/step-summary), the tool's contract is "keep the span / do NOT shrink the kept span" — so dropping a gathered sibling result is genuine lost work even with the breadcrumb present. DEFINITIVE.

**F-4 (DEFINITIVE) — the abandon-class ASYMMETRIC cells are BENIGN (target-position-driven), not the #80 artifact.** `failure`/`tangent` × `last-result`(a2) and × `mid-result`/`last-result`(a3) show `[PAIR,DROP]` / `[PAIR,PAIR,DROP]`. Probe evidence (`kc02_probe`): for `abandon last-result a3` the surviving `a,b` results are LIVE on-trunk (the revert landed on result_c, the *tip*; a,b are upstream live work legitimately kept atomic), and `c` (the abandoned tip) IS reclaimed (`crumb=true`). The asymmetry is the revert *target's position in the linear chain*, not a keep-first/drop-rest stamping artifact: everything at-or-before the tip is live and kept; the abandoned tail is reclaimed. **No silent loss, balance OK.** The abandon-class is correct + uniform on the load-bearing axis (call-node + first-result → full symmetric `[DROP,…]` whole-cluster reclamation). DEFINITIVE.

**F-5 (DEFINITIVE) — abandon-class onto-call-node / onto-result is consistent (F2 fork answer).** Abandon × call-node and × first-result both fully collapse the cluster to the breadcrumb (`[DROP,DROP(,DROP)]`, symmetric, no tool_call survives — probe `[ABANDON call-node a3] any_call=false`). The result-node path (`:588-668`) moves the tag to the parent and clears it identically. No sibling-leak in the abandon path. F2's "confirm/adjust uniformity" → **confirm: already uniform**; only the pinned path (F1) needs the fix. DEFINITIVE.

**F-6 (DEFINITIVE) — fix-locus is render-pass kernel-only; KC-01 F-1 blast-radius bound holds.** Re-grep of `parent_id` field-reads in non-test source: the only trunk-traversal consumers are `build_trunk_context` (`context.rs:1106`, `cur = lm.parent_id`) and the direct-child join in `retain_pinned_calls_on_node` (`:804`). All other hits are serde/accessor plumbing (`agent_message.rs`) or positional `parent_idx` Vec-index variables (not `parent_id` traversal). Options (a) and (b) both operate inside `retain_pinned_calls_on_node` on the cloned trunk — **no node-model / stamping change, no `build_trunk_context` change, forensic `self.messages` never mutated.** Other phi-core callers (`sub_agent.rs`, `parallel.rs`, `evaluation.rs`) do not call `collapse_abandon_class_cluster` (it is invoked only on the revert-mode render path, `streaming.rs:237`) → zero blast radius outside the brake render. DEFINITIVE.

**F-7 (DEFINITIVE) — F1 consequence analysis (pinned-onto-call-node).** For the `pinned call-node a3` cell (`[a:call, b,c via linear chain]`, trunk `[n0,n1]`, all 3 results off-trunk, only `a` a direct child of n1):
- **Option (a) reclaim-to-breadcrumb** — drop ALL of a,b,c's calls + do NOT re-append a's result; keep only the durable summary breadcrumb. Result: `[DROP,DROP,DROP]` + breadcrumb. SYMMETRIC, no LOSS-class asymmetry (the whole pinned tail is uniformly reclaimed into the durable marker; consistent with the tool's "reclaim the abandoned tail" wording). Fixes **all 6 onto-call-node + onto-first-result LOSS cells** (makes them symmetric `[DROP,…]`). For the 2 `mid-result a3` LOSS cells it makes `c` reclaim symmetrically too (the pinned span ends at the tip; the tail past it is reclaimed).
- **Option (b) pin-whole-cluster** — re-append b's and c's results too (resolve them from the forensic log by call-node membership rather than the direct-child `parent_id` gate). Result: `[PAIR,PAIR,PAIR]`. SYMMETRIC, NO loss (full sub-task pinned). Fixes the same 8 LOSS cells by KEEPING instead of dropping. Heavier render (re-materialises N results) and requires a *non-`parent_id`* lookup (resolve the cluster's results by the call-node's tool_call_ids in the forensic log) since b,c are not direct children.

**Recommendation (NOT a lock):** the matrix surfaces a real semantic tension the planner + user must resolve. Option **(a)** is the lighter, more-symmetric fix and matches the `revert_to_state` description wording ("completion/step-summary reclaim the abandoned tail"); it is the planner-lean per #80 §3 and forward-scope §7. Option **(b)** is the choice if the product intent is "a pinned completion must preserve ALL gathered parallel work verbatim." Decide by the desired semantic, not by cost — but (a) is both cheaper AND more internally-consistent with the existing abandon-class symmetry. Evidence above; the user owns the lock.

**F-8 (DEFINITIVE) — no separable large issue ⇒ no KC-03 split warranted (F3 fork answer).** Both (a) and (b) are localized to `retain_pinned_calls_on_node` (render-pass, ≤ ~30 LOC). No node-model change (true sibling stamping) is required to fix any LOSS cell — F-7 shows both options work purely in the render pass. The only thing that *would* force a KC-03 split (a `build_trunk_context` tree/DAG re-architecture to stamp siblings) is **NOT needed**: the linear chain plus the render-pass fix fully resolves the grid. Absorb in KC-02. DEFINITIVE. (Note: a future *true-sibling* node model remains the deeper-correctness option but is unnecessary for #80 closure — surface as a deferrable, not a KC-03 blocker.)

**F-9 (DEFINITIVE) — Mock `mock-tool-{i}` collision (KC-01 F-8) does not corrupt the matrix.** The fixtures use explicit distinct ids (`pcall_a/b/c`); the composite `(node_id, tool_call_id)` join (`retain_pinned_calls_on_node:802-808`, requiring `parent_id == this_node_id`) means even an id collision across turns cannot cross-join a stale result. The grid disposition is driven by the direct-child `parent_id` gate, which is collision-proof by construction. DEFINITIVE.

## §4 — Fix-locus determination

| change the chunk implies | side | carve-out test rationale | blast radius |
|---|---|---|---|
| Pinned-onto-call-node disposition: drop-all-and-breadcrumb (a) OR re-append-all-siblings (b) | **kernel** | Genuine general braking-correctness fix carrying ZERO consumer-specific logic — any consumer (i-phi/baby-phi/future MCP) firing parallel tools while braking is affected identically. No i-phi requirement leaks in. | `retain_pinned_calls_on_node` (`context.rs:755`) only. `collapse_abandon_class_cluster` callers unaffected (sole call-site `streaming.rs:237`, revert-mode only). `sub_agent.rs`/`parallel.rs`/`evaluation.rs` do not invoke it. M=1 path byte-identical (single direct-child result re-append is unchanged under both options). |
| (NONE) node-model / sibling stamping | n/a — **not required** | F-8: render-pass fix fully closes the grid; no `build_trunk_context` change. | n/a |
| Doc-comment + `concept-brake.md` + ADR-0002 matrix semantics | kernel docs | Documentation-code alignment for the new matrix semantic. | docs only. |

**No public-API / persisted-field / migration change** (target from §6 acceptance — CONFIRMED: both options mutate only the cloned trunk in an existing private fn).

## §5 — Non-viable approaches ruled out

| approach | why tempting | disproof + evidence |
|---|---|---|
| **Re-stamp parallel results as true siblings (node-model change) to "fix the root cause"** | The linear chain is #80's "original sin"; making b,c direct children of n1 would let the existing direct-child re-append keep them. | Re-confirmed KC-01's disproof: `build_trunk_context` (`context.rs:1074-1149`) is a single **linear** parent-chain walk and the SOLE `parent_id` consumer (F-6 grep). Setting `b.parent=c.parent=n1` breaks it — walking from `active=result_b` would yield `[n0, result_b]` and DROP n1 (siblings aren't on each other's ancestor path). It would require converting the linear walk into a tree assembly + a revert-snap-to-boundary in `apply_revert` — a core braking-state-machine change. F-8 proves it is UNNECESSARY: the render-pass fix closes all 8 LOSS cells. Non-viable for KC-02 (over-scoped; reopens the trunk-walk core). |
| **Spot-fix only the `completion`-onto-call-node cell #80 observed, then ship** | It's the one cell the live transcript hit; minimal diff. | The matrix proves the defect spans 8 cells across BOTH pinned categories AND arity 2+3 (F-2). Fixing one cell leaves 7 identical LOSS cells live → the same silent-loss class recurs for `step-summary` and for `first-result`/`mid-result` targets. Violates the characterize-first directive (#80 §4); non-viable. |

## §6 — Design forks surfaced (for the planner — NOT locked)

| fork | options | factual trade-off surfaced by this investigation |
|---|---|---|
| **F1 — pinned-onto-call-node semantic** (load-bearing) | (a) reclaim-to-breadcrumb · (b) pin-whole-cluster | (a): `[DROP,…]`+crumb — lighter, symmetric, matches the tool's "reclaim the tail" wording, internally consistent with the abandon-class symmetry (F-4/F-5), ≤~15 LOC. Fixes all 8 LOSS cells by dropping uniformly. (b): `[PAIR,…]` — preserves ALL gathered work verbatim, but heavier render + needs a non-`parent_id` forensic-log lookup to resolve the off-trunk siblings (they are grandchildren, not direct children) + risks re-materialising work the model intended to seal-past. Both render-pass-only, both fix the same 8 cells. Decide by product semantic; (a) is cheaper AND more consistent. |
| **F2 — abandon-class onto-call-node/result consistency** | confirm vs adjust | **Investigation answer: confirm — already uniform.** Abandon × call-node + first-result fully + symmetrically collapse the cluster to the breadcrumb (F-5); the asymmetric abandon mid/last-result cells are benign target-position effects (F-4), not leaks. No adjustment needed; F2 likely closes as a no-op confirmation row in the plan. |
| **F3 — KC-03 split decision** | absorb vs split | **Investigation answer: absorb.** No separable large issue; no node-model change required (F-8). The only split-worthy item (true-sibling node model) is unnecessary for #80 closure and can be a deferrable note, not a KC-03 blocker. |

## §7 — Repro assets

| asset | what it proves | status | re-run |
|---|---|---|---|
| `kc02_characterization_matrix_THROWAWAY` (32-cell driver: arity-2/3 fixtures + post-cluster fixture + `classify`/`trunk_has_orphan_result` + full-pipeline runner) — spliced into `collapse_abandon_class_cluster_tests`, **reverted** | The full §3 matrix (balance/symmetry/loss/disposition per cell) via the verbatim production pipeline | **throwaway-reverted** (git tree clean for `src/`). **PROMOTE-TO-REGRESSION-TEST candidate**: the implementer should land per-cell assertions (esp. the 8 LOSS cells → green under the locked F1 option, + the benign abandon-asymmetry rows + the no-400 invariant) as permanent coverage per deliverable §5.4. | re-create the harness in the test module + `RUSTFLAGS="" /root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib kc02_characterization_matrix -- --nocapture` |
| `kc02_probe_abandon_vs_pinned_THROWAWAY` (focused dispositions for abandon vs pinned at arity 3) — **reverted** | F-4 (benign abandon asymmetry: surviving siblings are live) + F-7 (pinned (a)/(b) consequence) | throwaway-reverted | same module + `--lib kc02_probe` |

Verified `git -C /root/projects/phi/phi-core status --short src/` returns empty (tree clean). The only new path is this report.

## §8 — Recommended close-gate

Per `[[feedback_render_transcript_close_gate]]` the POST-fix close-gate is the FULL live grid (orchestrator-run, §5.6 forward-scope): both drivers (`deepseek-chat-v3-0324` + `minimax-m2.7`) × 4 categories, transcript-read, asserting correct keep/drop disposition end-to-end.

**Pre-fix live confirmation (small, orchestrator-run — NOT me):** because the matrix proves **no hard 400 exists** (F-1), the worst real-world cell to confirm is the **wasteful re-fetch / lost-work** consequence, not a 400. Drive **`deepseek-chat-v3-0324`** (the proven parallel-firer, KC01-CLOSEGATE) to fire **≥ 2 parallel tool_calls in one node** then issue **`revert_to_state(category: completion, step=<the call-node nN>)`** — the `completion × call-node` cell. Render + READ the transcript: confirm exactly ONE sibling result survives and the model re-fetches the dropped sibling (the silent-loss seal-loop), establishing the live S2 consequence today. Use `minimax-m2.7` as the second driver for the `step-summary` category to confirm the defect is model-independent. This is the single worst-case pre-fix confirmation; the full 2×4 grid is the post-fix acceptance gate.

## §10 — (B)-mechanism design + validation

> Added 2026-06-08 (second P0 pass, user-directed). The user chose the **broader fix
> (B): fully close the silent-loss class**. This section DESIGNS the (B) mechanism and
> VALIDATES it against the full 32-cell matrix + 5 edge fixtures via a throwaway
> implementation driven through the VERBATIM production render pipeline
> (`streaming.rs:232-259`). Throwaway edits + harness REVERTED; `git status --short
> src/` empty. **Verdict: (B) is VALID** — all 10 LOSS cells fix, zero regression on
> OK cells, zero new orphan/dangling/400, with ONE design refinement the matrix
> surfaced (the on-trunk keep-verbatim gate, §10.6).

### §10.0 — Verdict
- **(B) mechanism VALID.** Throwaway impl + 6 throwaway tests (`kc02_b_*`) green through the exact production pipeline (collapse → decay → weave → inject → `enforce_call_atomic_backstop`).
- **True LOSS-cell count = 10, not 8** (the §0/§3 narrative said "8"; the §3 matrix table actually marks **10** bolded LOSS rows — the original prose under-counted by missing the 2 `mid-result × arity-3` cells, table rows 27 + 31). **All 10 fix under (B).**
- **0 regressions** on the currently-OK cells; **0 new orphan/dangling/400** in any of the 37 cells exercised (32 matrix + 5 edges/probes).
- **3 KC-01 tests flip** under (B) — all are the intended semantic change, NOT collateral breakage (§10.7). Full lib suite: **236 passed, exactly 3 failed** (the 3 intended flips); all 106 integration tests (`agent_loop_test` 56 / `agent_test` 14 / `session_test` 36) GREEN.
- **LOC**: production delta **+32 / −13** in `retain_pinned_calls_on_node` (+ a 1-token caller-signature change to thread `node_is_call`); **~+28 LOC larger than the (a) deletion**. Render-pass-only; no node-model / `build_trunk_context` change; forensic log read-only.

### §10.1 — The (B) branch logic (precise)
The fix threads the caller's already-computed `node_is_call` into `retain_pinned_calls_on_node(trunk, idx, tag_on_call_node)` and branches **per call**:

```
for each call_id in this call-node's calls:
    if result on-trunk:
        KEEP (never drop) — the model continued PAST this cluster (mid-trunk
        pinned cluster); dropping it would collapse a legitimately-kept cluster.
    elif tag_on_call_node:                      // revert landed onto the CALL-NODE
        DROP the call, never re-append           // → reclaim whole cluster to the
                                                 //   durable breadcrumb (= the (a) deletion)
    else:                                        // tag landed on a RESULT-NODE
        re-append the off-trunk result by CALL-NODE MEMBERSHIP
        (forensic-log find: tool_call_id == call_id), NOT the direct-child
        parent_id gate — the linear-stamped siblings are GRANDCHILDREN.
        (a genuine post-cluster follow-on whose tool_call_id ∉ this node's calls
         is never matched here → stays dropped/sealed-past.)
```

Two changes vs the locked (a)/F1.b body: (1) the off-trunk re-append predicate changes from `lm.parent_id == Some(this_node_id) && tool_call_id == call_id` (direct-child) to `tool_call_id == call_id` (call-node membership) — this is what reaches the off-trunk parallel siblings; (2) the branch on `tag_on_call_node` selects reclaim (call-node) vs keep-all (result-node).

### §10.2 — Re-run matrix (post-(B), all 32 cells; verbatim pipeline output)
Legend: `PAIR`=call+result both present; `DROP`=both gone; `crumb`=breadcrumb present. `dangle`/`orphan`=false in EVERY cell (no-400 invariant holds). `Δ` flags cells whose disposition CHANGED from the pre-(B) §3 matrix.

| category | target | arity | balance | crumb | post-(B) disposition | Δ |
|---|---|---|---|---|---|---|
| failure | call-node | 2 | OK | yes | [DROP,DROP] | — |
| failure | first-result | 2 | OK | yes | [DROP,DROP] | — |
| failure | last-result | 2 | OK | yes | [PAIR,DROP] | — |
| failure | post-cluster | 2 | OK | yes | [PAIR,PAIR] | — |
| failure | call-node | 3 | OK | yes | [DROP,DROP,DROP] | — |
| failure | first-result | 3 | OK | yes | [DROP,DROP,DROP] | — |
| failure | mid-result | 3 | OK | yes | [PAIR,DROP,DROP] | — |
| failure | last-result | 3 | OK | yes | [PAIR,PAIR,DROP] | — |
| tangent | call-node | 2 | OK | yes | [DROP,DROP] | — |
| tangent | first-result | 2 | OK | yes | [DROP,DROP] | — |
| tangent | last-result | 2 | OK | yes | [PAIR,DROP] | — |
| tangent | post-cluster | 2 | OK | yes | [PAIR,PAIR] | — |
| tangent | call-node | 3 | OK | yes | [DROP,DROP,DROP] | — |
| tangent | first-result | 3 | OK | yes | [DROP,DROP,DROP] | — |
| tangent | mid-result | 3 | OK | yes | [PAIR,DROP,DROP] | — |
| tangent | last-result | 3 | OK | yes | [PAIR,PAIR,DROP] | — |
| **completion** | **call-node** | **2** | OK | yes | **[DROP,DROP]** (was [PAIR,DROP] LOSS) | **Δ fix** |
| **completion** | **first-result** | **2** | OK | yes | **[PAIR,PAIR]** (was [PAIR,DROP] LOSS) | **Δ fix** |
| completion | last-result | 2 | OK | yes | [PAIR,PAIR] | — |
| completion | post-cluster | 2 | OK | yes | [PAIR,PAIR] | — |
| **completion** | **call-node** | **3** | OK | yes | **[DROP,DROP,DROP]** (was [PAIR,DROP,DROP] LOSS) | **Δ fix** |
| **completion** | **first-result** | **3** | OK | yes | **[PAIR,PAIR,PAIR]** (was [PAIR,DROP,DROP] LOSS) | **Δ fix** |
| **completion** | **mid-result** | **3** | OK | yes | **[PAIR,PAIR,PAIR]** (was [PAIR,PAIR,DROP] LOSS) | **Δ fix** |
| completion | last-result | 3 | OK | yes | [PAIR,PAIR,PAIR] | — |
| **step-summary** | **call-node** | **2** | OK | yes | **[DROP,DROP]** (was [PAIR,DROP] LOSS) | **Δ fix** |
| **step-summary** | **first-result** | **2** | OK | yes | **[PAIR,PAIR]** (was [PAIR,DROP] LOSS) | **Δ fix** |
| step-summary | last-result | 2 | OK | yes | [PAIR,PAIR] | — |
| step-summary | post-cluster | 2 | OK | yes | [PAIR,PAIR] | — |
| **step-summary** | **call-node** | **3** | OK | yes | **[DROP,DROP,DROP]** (was [PAIR,DROP,DROP] LOSS) | **Δ fix** |
| **step-summary** | **first-result** | **3** | OK | yes | **[PAIR,PAIR,PAIR]** (was [PAIR,DROP,DROP] LOSS) | **Δ fix** |
| **step-summary** | **mid-result** | **3** | OK | yes | **[PAIR,PAIR,PAIR]** (was [PAIR,PAIR,DROP] LOSS) | **Δ fix** |
| step-summary | last-result | 3 | OK | yes | [PAIR,PAIR,PAIR] | — |

`kc02_b_matrix_THROWAWAY` asserts `residual partial-LOSS cells == 0` AND `dangle/orphan cells == 0` — **PASS**.

### §10.3 — LOSS-cell count + all-fix confirmation (DEFINITIVE)
**True LOSS count = 10** (the §3 table's 10 bolded rows: `completion`/`step-summary` × {`call-node`,`first-result`}×{a2,a3} = 8, PLUS × `mid-result`×a3 = 2). All 10 fix under (B):
- **4 call-node cells** → `[DROP,…]` + breadcrumb (symmetric reclaim, the (a) semantic for call-node).
- **6 result-node cells** (first-result a2+a3, mid-result a3, for both pinned categories) → `[PAIR,…]` — the off-trunk cluster sibling(s) re-appended; the WHOLE cluster survives.

DEFINITIVE — captured `kc02_b_matrix_THROWAWAY` output: every formerly-LOSS row now renders `[DROP,DROP(,DROP)]` (call-node) or `[PAIR,PAIR(,PAIR)]` (result-node), `residual partial-LOSS cells under (B): 0`.

### §10.4 — No-regression + no-orphan confirmation (DEFINITIVE)
- `completion`/`step-summary` × `last-result` → stays `[PAIR,PAIR(,PAIR)]` (UNCHANGED).
- `× post-cluster` → stays `[PAIR,PAIR]` (cluster fully upstream; the post-cluster sequential revert leaves it untouched).
- ALL 16 abandon-class cells (`failure`/`tangent` × every target × a2+a3) → byte-identical to the pre-(B) §3 matrix (the (B) branch is in the *pinned* arm only; abandon-class never reaches `retain_pinned_calls_on_node`).
- **No-400 invariant**: `trunk_has_orphan_result` + `trunk_has_dangling_call` run on EVERY cell → both false in all 37 cells. DEFINITIVE.

### §10.5 — Edge fixtures (DEFINITIVE)
| fixture (throwaway test) | construction | post-(B) result | verdict |
|---|---|---|---|
| **EDGE-4 genuine post-cluster follow-on** (`kc02_b_edge_post_cluster_followon`) | `cluster[pc_0,pc_1]→r0,r1`, then SEQUENTIAL `seq_c→r_seq`; PINNED revert onto r0 (n2) | `pc_0` PAIR (on-trunk), `pc_1` PAIR (re-appended), `seq_c` **CALL+RESULT both DROPPED** | **PASS** — the mechanism distinguishes cluster-siblings (re-append) from sealed-past follow-ons (drop), keyed on call-node membership; no 400. |
| **EDGE-5 mixed on/off-trunk** (`kc02_b_edge_mixed_on_off_trunk`) | arity-3, PINNED revert onto MID result (r1, n3); r2 (n4) is OFF-trunk past the tip | `[PAIR,PAIR,PAIR]` — the off-trunk sibling pc_2 PAST the tip is re-appended → WHOLE cluster survives | **PASS** — re-append reaches siblings past the revert tip, not just those before it. |
| **M=1 single-call** (`kc02_b_m1_single_call`) | arity-1, PINNED; revert onto call-node n1 vs result-node n2 | call-node → `[DROP]`+crumb (reclaim); result-node → `[PAIR]` | **PASS** — the M=1 path behaves correctly under the new branch (no special-casing needed). |
| **KC01-flip probes** (`kc02_b_kc01_flip_probes`) | arity-2 pinned, call-node vs first-result | call-node → `[DROP,DROP]`+crumb; first-result → `[PAIR,PAIR]` | **PASS** — confirms the two locked KC-01 pinned-test expectations flip exactly as predicted. |
| **TWO-CLUSTER surprise probe** (`kc02_b_two_clusters`) | cluster1 upstream (continued-past) + cluster2 at tip; PINNED revert onto cluster2's first result | cluster1 `[PAIR,PAIR]` kept verbatim; cluster2 whole-survives (pc2_1 re-appended); no 400 | **PASS** — no cross-cluster interaction (`processed` set + per-node call-id membership isolate clusters). |

### §10.6 — LOC + locus (DEFINITIVE) + the design refinement the matrix forced
- **Production delta**: `+32 / −13` lines inside `retain_pinned_calls_on_node` (comments included; functional logic ≈ +19 net LOC) + **one** caller line (`:728`) gains the `node_is_call` argument + the fn signature gains the `tag_on_call_node: bool` param. **~+28 LOC larger than the (a) deletion** (which is a net *removal* of the ~9-line direct-child re-append path).
- **Locus CONFIRMED render-pass-only**: the change is entirely inside `retain_pinned_calls_on_node` + a 1-token signature thread from its sole caller (`collapse_abandon_class_cluster`). **No `build_trunk_context` change, no node-model / sibling-stamping change.** `self.messages` (forensic log) is accessed `.iter().find()` + `.clone()` only — **never mutated** (verified: the only `self.messages` diff line is the find-predicate rename `direct_child` → `cluster_result`). KC-01 F-1 / §6 F-6 blast-radius bound HOLDS.
- **DESIGN REFINEMENT surfaced by the matrix (load-bearing — see §10.8 surprise #1)**: the *naive* (B) "tag-on-call-node → drop ALL calls" is **too aggressive**: a pinned cluster the model legitimately **continued past** (mid-trunk; its results are ON-trunk) would be wrongly collapsed. The validated (B) gates the reclaim on **result-OFF-trunk**: an on-trunk call is ALWAYS kept (in both branches). Without this gate, `pinned_cluster_mid_trunk_kept_verbatim` FAILS (a true regression); with it, only the 3 intended KC-01 cells flip. **This gate is part of the (B) mechanism the planner must lock**, not an optional nicety.

### §10.7 — KC-01 test-flip list (DEFINITIVE)
Under validated (B), the full lib suite is **236 pass / 3 fail** — the 3 fails are the intended semantic flips; the implementer must re-author these 3 assertions when landing (B):

| KC-01 test (`collapse_abandon_class_cluster_tests::`) | pre-(B) (locked a/F1.b) | post-(B) | flip class |
|---|---|---|---|
| `pinned_class_keeps_cluster_whole_no_dangling_call` | M=1 pinned revert-onto-call-node keeps the sealed body whole + re-appends its direct-child result → `[PAIR]` | **`[DROP]` + breadcrumb** (reclaim whole cluster) | **intended** — the (B) call-node "reclaim, don't keep" semantic. The most consequential flip: the M=1 revert-onto-call-node "keep the sealed body" assertion is INVERTED. |
| `row_pinned_revert_to_call_node_now_call_atomic` | arity-2 → `[PAIR,DROP]` (call_a's direct-child re-appended, call_b dropped) | **`[DROP,DROP]`** (whole reclaim) | **intended** |
| `row_pinned_revert_to_first_result_now_call_atomic` | arity-2 → `[PAIR,DROP]` (call_b NOT re-materialised) | **`[PAIR,PAIR]`** (call_b re-appended) | **intended** — exactly the user-confirmed "row_pinned_revert_to_first_result_now_call_atomic → now [PAIR,PAIR]" prediction. |

`pinned_cluster_mid_trunk_kept_verbatim` does **NOT** flip under the *refined* (B) (it would under naive (B) — see §10.6/§10.8). All other pinned/abandon tests stay green. No integration-test flip (106/106 green).

### §10.8 — Surprises / edges the matrix revealed
1. **(SURPRISE — load-bearing) Naive "drop-all-on-call-node" over-reaches on mid-trunk pinned clusters.** The user's (B) spec phrase "pinned tag on the CALL-NODE → reclaim the whole cluster" must be qualified to **"…when the cluster's results are OFF-trunk"**. A pinned `completion` cluster the model SEALED and then continued past (active tip downstream, results on-trunk) is `node_is_call=true` too — and the naive branch would collapse it, regressing `pinned_cluster_mid_trunk_kept_verbatim`. The validated mechanism keeps any on-trunk call verbatim in BOTH branches. The planner MUST carry this gate in the F1(B) lock body.
2. **The §3 narrative's "8 LOSS cells" was an under-count; the matrix is "10".** Recounted here from the table itself — the planner's acceptance §5 must assert **10** cells fix, not 8 (the 2 `mid-result × arity-3` cells are real LOSS that (B) fixes to `[PAIR,PAIR,PAIR]`).
3. **No bad interaction with decay-drop.** Pinned tags (Outcome/Checkpoint) are non-decayable (`is_decayable()==false`), so the reachable decay-drop never targets a (B)-reclaimed pinned node; the breadcrumb on a call-node-reclaimed cluster persists (it is woven, not decayed). Confirmed by `crumb=yes` in every (B) call-node cell at `IN_WINDOW_TURN`.
4. **No cross-cluster interaction** (TWO-CLUSTER probe): the `processed` HashSet + per-node call-id membership keep a second later cluster fully isolated; an upstream continued-past pinned cluster stays verbatim while the tip cluster re-appends. No 400 across both.
5. **Re-append placement is benign for atomicity.** Off-trunk siblings are inserted right after the call node (`idx+1+offset`), which is upstream of where they sat originally for the result-node-tagged case; provider validity only requires call↔result co-presence (`enforce_call_atomic_backstop` checks presence, not adjacency), so ordering is non-load-bearing. No orphan/dangle resulted in any cell.

### §10.9 — Repro assets (all REVERTED)
| asset | what it proves | status |
|---|---|---|
| Throwaway (B) edit to `retain_pinned_calls_on_node` (+ `node_is_call` caller thread) | the validated (B) branch logic incl. the on-trunk keep-verbatim gate | **reverted** — `git status --short src/` empty; collapse module back to 20/20 green |
| `kc02_b_matrix_THROWAWAY` (32-cell driver via verbatim pipeline + `trunk_has_orphan_result` + `classify_cluster` PAIR/DROP) | §10.2 matrix + §10.3 all-10-fix + §10.4 no-regression/no-orphan | **reverted** — PROMOTE-TO-REGRESSION-TEST candidate (land per-cell assertions for the 10 fixed cells + the no-400 invariant). |
| `kc02_b_edge_post_cluster_followon` / `kc02_b_edge_mixed_on_off_trunk` / `kc02_b_m1_single_call` / `kc02_b_kc01_flip_probes` / `kc02_b_two_clusters` | §10.5 edges + §10.8 surprises | **reverted** — promote the follow-on + mixed + two-cluster as permanent (B) coverage. |

Re-run recipe (re-create the throwaway impl + harness in `collapse_abandon_class_cluster_tests`): `RUSTFLAGS="" /root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib kc02_b_ -- --nocapture`.
