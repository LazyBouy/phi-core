<!-- Last verified: 2026-08-26 by Claude Code (KC-06 plan iter-1 draft; phi-core kernel lane; grounded on committed P0 `kc-06-mock-provider-repeat-mode-p0-investigation.md` + phi-core `dev` HEAD `929d326`; baseline 610 tests grounded via full `cargo test`) -->

# KC-06 — MockProvider opt-in repeat/cycle mode (kernel half of i-phi #121)

> Sixth phi-core kernel chunk. Ships the **kernel-only** half of i-phi **#121** (mock provider multi-turn / repeat): an OPT-IN cycle-flat-queue repeat mode on `MockProvider` so a single test/demo session can re-emit its scripted tool-call(s) on every turn. Fix-locus **phi-core KERNEL, purely additive** per P0 §0/§4 — one private field + two additive constructors + a ~3-line rotate-on-pop cycle in `stream`. Back-compat by construction: `repeat` defaults OFF; all 133 existing `MockProvider::{new,text,texts}` call-sites stay byte-behaviour-identical; **ZERO existing phi-core test flips (predict ZERO RED)**. The i-phi `[provider.mock] repeat` config field + `mock_responses_as_responses` `[ToolCalls, Text]` reordering is an explicit **SEPARATE downstream i-phi chunk**, NOT this one.

- **Project**: phi-core (kernel lane). Root `/root/projects/phi/phi-core`, branch `dev`. Host cargo (`/root/rust-env/cargo/bin/cargo`), single crate, NO `--workspace`, NO Docker, NO `check-*.sh` guards.
- **HEAD**: `929d326` ("Release phi-core 0.12.0"). KC-05 (`133ad33`, progressive tool disclosure) is committed BEFORE HEAD → the baseline already carries KC-05's surface.
- **Committed P0** (gate-0.5-verified, 10 DEFINITIVE / 0 unresolved, every claim REPRODUCED): `_p0-investigations/kc-06-mock-provider-repeat-mode-p0-investigation.md` (moves into this cycle folder at archive). §2 surface map, §3 findings, §4 fix-locus, §5 non-viable, §6 forks, §8 close-gate are authoritative.
- **Issue**: i-phi **#121** (`LazyBouy/i-phi`). Acceptance: N `send-input`s to ONE session park N interactive Asks with the new mode enabled; one-shot stays the default.
- **ADR**: `docs/decisions/0006-mock-provider-repeat-mode.md` (0001–0005 exist; 0006 is next — verified via `ls docs/decisions/`).
- **Baseline**: **610** tests passing (+5 ignored live-API) at HEAD `929d326`, grounded via full `cargo test -j 4` (matches `_cycle-index.md` KC-05 row "+21 → 610").
- **Semver**: additive public API → MINOR bump **0.12.0 → 0.13.0** (P-SEAL deliverable). `cargo publish` is a POST-SEAL orchestrator/user step (crates.io token not in this env) — NOT scripted here.

---

## Forks for orchestrator

> All 4 forks are **planner-rec** and evidence-settled by the committed P0 (nothing diverges from forward-scope + precedent). Surfaced for CONFIRMATION, not re-litigation. If the user confirms all planner-recs → **Direct-approval-clean / P-orch-8 skip-condition fires → archive at iter-1** (no iter-2 re-spawn). F1 + F4 are design/scope forks (full 4-line framing); F2 + F3 are **TECHNICAL FORKS** (test-double API only; zero end-user delta).

> **Cross-cycle divergence note**: i-phi/phi-core kernel-lane forks run ~87% planner-rec (unanimous is dominant on the kernel lane; the divergence axes — architectural-bridge / process-velocity / production-readiness — do not apply to a test-only additive provider change). No divergence anticipated here.

> **Coupling note (light)**: **F3 (cursor internals) is conditional on F1.a.** Rotate-on-pop is an *implementation* of cycle-flat-queue; if F1 flipped to per-turn-unit machinery (F1.b) the F3 cursor question is moot. The planner-rec set (F1.a + F2.a + F3.a + F4.a) is internally coherent. No other coupling.

### F1 — repeat MECHANISM (CRUX design fork; model/test-facing)

| Option | User-visible (test-author / operator perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F1.a (planner-rec)** — cycle the flat `Vec<MockResponse>` verbatim | A test/demo author scripts one mock session whose configured tool-call re-parks its interactive Ask on EVERY `send-input` (via the downstream i-phi flag), so `send → Ask → approve → send → Ask …` sustains in one session — no fresh session per interaction. | - **P0-PROVEN sufficient** for #121: F3 repro drove `[ToolCalls, Text]` through 3 send-inputs → `[1,1,1]` tool calls. - Smallest kernel surface (~3-line rotate + 1 private field). - Zero i-phi concept in the kernel — the queue is order-agnostic; the consumer supplies the unit. | - Cadence is a function of queue COMPOSITION — the consumer MUST supply `[ToolCalls, Text]` (F4/F5 in P0); a leading text or non-2-length queue drifts cadence. **Product trajectory:** the kernel stays a thin, general cycle primitive; per-turn expressiveness (Option B) is a clean later add without kernel churn. | LOCK? |
| F1.b — per-turn UNIT abstraction (`Vec<TurnUnit>`, unit = `[ToolCalls?, Text]`) in the kernel | Same test-author capability, but the kernel owns turn-unit cadence so the consumer can't mis-order. | - Robust to queue composition (kernel guarantees cadence). | - Larger kernel surface that starts encoding Option-B semantics in the kernel BEFORE they're needed (P0 §5 A1/§6 F-D); **UNNECESSARY** — P0 proved flat-cycle closes #121. **Product trajectory:** pulls Option-B scope into the kernel prematurely, growing the surface `[[feedback_phi_core_kernel_minimal]]` wants minimal. | NOT chosen |

**AskUserQuestion (4-line):**
- **User-visible:** F1.a lets one mock session re-park its Ask on every send (via the i-phi flag later); F1.b delivers the same capability but the kernel owns per-turn cadence.
- **Product trajectory:** F1.a keeps the kernel a thin general cycle primitive (Option B a clean later add); F1.b grows the kernel surface prematurely.
- **Cycle scope:** F1.a = 1 private field + ~3-line rotate + 2 ctors; F1.b = a new `TurnUnit` type + queue restructure + larger cascade.
- **Defers if chosen:** F1.a defers Option B per-turn-script to a later chunk (P0 recommendation + #121's own "Option B later"); F1.b partially pulls it in now.

### F2 — additive API SHAPE  **TECHNICAL FORK** (test-double API; no end-user delta — pick on engineering merit only)

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F2.a (planner-rec)** — `MockProvider::with_repeat(responses, repeat: bool)` **+** `repeating(responses)` convenience | `with_repeat` mirrors `new` and is a one-line re-point for the single downstream i-phi call-site (`config/mod.rs:1301`); it passes `arm.repeat` directly. `repeating(responses)` = `with_repeat(responses, true)` mirrors the existing `text`/`texts` conveniences and reads cleanest at the many new test call-sites (close-gate + promoted repro). Both keep `new`/`text`/`texts` one-shot → all 133 sites byte-identical. | Two new public methods (vs one) — but both are thin and parallel the existing convenience pattern; still minimal. | LOCK? |
| F2.b — builder-setter `MockProvider::new(responses).repeat(true)` (consumes `self`) | Most idiomatic-Rust / extensible if more mock flags land later. | An out-of-order/forgotten `.repeat()` silently one-shots; the i-phi call-site becomes a 2-call chain vs one ctor. | NOT chosen |
| F2.c — `repeating(responses)` convenience ONLY (no `bool` ctor) | Smallest surface. | Forces the i-phi mapping to branch on `arm.repeat` at the call-site (`if repeat { repeating(v) } else { new(v) }`) instead of passing the bool through — clumsier for the exact consumer we're serving. | NOT chosen |

**AskUserQuestion (2-line, TECHNICAL):**
- **Engineering merit:** F2.a gives the i-phi call-site a one-line `bool`-carrying ctor AND a clean test convenience, parallel to the existing `text`/`texts`; F2.b is idiomatic but risks a silent one-shot on mis-ordering; F2.c is smaller but pushes a branch onto the consumer.
- **Recommendation:** F2.a (`with_repeat` + `repeating`).

### F3 — cursor INTERNALS  **TECHNICAL FORK** (implementation detail; no user-visible delta — pick on engineering merit only)  *(conditional on F1.a)*

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F3.a (planner-rec)** — rotate-on-pop under the existing `Mutex` | Smallest diff (~3 lines inside the existing lock scope): on `repeat=true` & non-empty, `remove(0)` the front then `push(front.clone())` to the back (`MockResponse` derives `Clone`, `mock.rs:52`). The `repeat=false` path keeps `remove(0)` **byte-identical** to today; the empty→fallback branch is unchanged. Thread-safe by construction (all mutation stays under the single existing `std::sync::Mutex`). | The queue's stored order rotates over the session — invisible (the field is private; no external observation) and self-correcting (a full cycle returns to start). | LOCK? |
| F3.b — separate index `cursor` + non-destructive read | Arguably clearer; leaves the `Vec` immutable. | Adds a SECOND piece of interior-mutable state (a `cursor`) needing its own guard/ordering vs the `responses` lock; more surface for a subtle bug; the one-shot path would change from `remove(0)` to indexed-read (perturbs the proven byte-identical path). | NOT chosen |

**AskUserQuestion (2-line, TECHNICAL):**
- **Engineering merit:** F3.a is a ~3-line rotate that keeps the one-shot `remove(0)` path verbatim and stays under the existing lock; F3.b is cleaner-looking but adds a second state field and perturbs the proven one-shot path.
- **Recommendation:** F3.a (rotate-on-pop).

### F4 — Option B (`[[provider.mock.turns]]` per-turn script) now vs defer (scope fork; product-trajectory)

| Option | User-visible (test-author perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F4.a (planner-rec)** — DEFER Option B; ship only the Option-A cycle flag now | A test author gets the repeat/cycle capability now (closes #121); expressive per-turn scripting (`[[provider.mock.turns]]`) is a later enhancement. | - #121 explicitly recommends "Option A now, Option B later." - Cycle-flat-queue satisfies #121's full acceptance (P0 F3). - Smallest kernel surface; ships the high-value fix fastest. | - A consumer wanting distinct scripted per-turn tool-calls (not a repeating unit) waits for the later chunk. **Product trajectory:** clean incremental path — Option B slots on top of the cycle primitive later with no rework. | LOCK? |
| F4.b — also build the per-turn-script kernel API now | A test author can script a realistic multi-turn conversation (each turn its own tool-calls + terminal text). | - Strictly more expressive. | - Larger kernel surface than #121 needs today; the one hard constraint Option A imposes (an explicit terminal `Text` in the repeating unit, P0 F5) is a downstream-CONFIG concern, not a reason to pull Option B into the kernel. **Product trajectory:** front-loads scope #121 framed as "later"; risks over-building before a consumer needs it. | NOT chosen |

**AskUserQuestion (4-line):**
- **User-visible:** F4.a ships the repeat/cycle capability now (closes #121); F4.b additionally ships expressive per-turn scripting.
- **Product trajectory:** F4.a is a clean incremental path (Option B slots on later); F4.b front-loads scope the issue framed as "later."
- **Cycle scope:** F4.a = the cycle flag only; F4.b = a new per-turn-unit type + queue restructure + docs + tests.
- **Defers if chosen:** F4.a defers Option B to a future kernel chunk (unblocked by this cycle's primitive); F4.b defers nothing but over-scopes now.

---

## §1 — Locked fork details (per chunk-planner v32 iter-1 template; planner-rec bodies pre-filled — pre-lock draft)

> Populated at iter-1 for ALL 4 forks per v32 P-plan-1-v32. If the user locks all planner-recs → archive at iter-1 (P-orch-8 skip-condition). If any fork locks USER-DIVERGENT → iter-2 narrow re-author of only that subsection.

#### F1 = F1.a — cycle the flat queue verbatim *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): `MockProvider` gains a private `repeat: bool` field; when `repeat == true` and the response `Vec` is non-empty, `stream` pops the front (`remove(0)`) as today and then re-queues a `clone()` of it to the back, so the flat `Vec<MockResponse>` cycles verbatim with no reorder and no per-turn-unit machinery (the P0-proven mechanism at `mock.rs:147-156`). The kernel is order-agnostic: it cycles whatever `Vec<MockResponse>` the consumer supplies, so a correct one-tool-call-per-turn cadence requires the queue to be exactly the `[ToolCalls, Text]` length-2 unit — which is the CONSUMER's (i-phi's) responsibility, not the kernel's. Under `repeat` the empty→`"(no more mock responses)"` fallback (`mock.rs:149-151`) is bypassed while the queue is non-empty, so a terminal (non-tool) `Text` MUST be present in the cycled unit (see §D6.4 CONSUMER contract).

**Rationale** (3 sentences): The P0 REPRODUCED that cycling the flat length-2 `[ToolCalls, Text]` queue sustains `[1,1,1]` tool calls across 3 send-inputs (each send consumes exactly 2 items, so the cursor returns to index 0 every send), proving flat-cycle is sufficient for #121 with the smallest possible kernel surface. A per-turn-unit abstraction (F1.b) is UNNECESSARY and would start encoding Option-B semantics into the kernel before any consumer needs them, violating `[[feedback_phi_core_kernel_minimal]]`. The queue-order-agnostic design keeps ZERO i-phi cadence policy in the kernel — the terminal-text + ordering responsibility lives entirely in the downstream consumer.

**Defers (if chosen)** (3 sentences): Option B per-turn scripting (`[[provider.mock.turns]]`) is deferred to a later kernel chunk (F4.a), unblocked by this cycle's cycle primitive. The i-phi `[provider.mock] repeat` config field + `mock_responses_as_responses` `[ToolCalls, Text]` reorder + terminal-text emission is the SEPARATE downstream i-phi chunk. No auto-synthesis of a terminal text enters the kernel (P0 §5 A3 — that would be a consumer-cadence leak).

#### F2 = F2.a — `with_repeat` constructor + `repeating` convenience *(pre-lock draft; finalizes at gate-1 lock)* — **TECHNICAL FORK**

**Code-level binding** (3 sentences): Add `pub fn with_repeat(responses: Vec<MockResponse>, repeat: bool) -> Self` (mirrors `new`; sets the private `repeat` field) and `pub fn repeating(responses: Vec<MockResponse>) -> Self` (= `with_repeat(responses, true)`, mirroring the `text`/`texts` convenience precedent at `mock.rs:82-104`). `new`/`text`/`texts` are refactored ONLY internally to set `repeat: false` (their signatures + observable behaviour are unchanged), so all 133 `MockProvider::{new,text,texts}` call-sites stay byte-identical. The downstream i-phi mapping re-points its single `MockProvider::new(...)` call-site (`config/mod.rs:1301`) at `with_repeat(self.mock_responses_as_responses(), arm.repeat)`.

**Rationale** (3 sentences): `with_repeat(responses, bool)` is the most direct match for the one downstream i-phi call-site — it threads `arm.repeat` through in a single line rather than forcing a call-site branch (F2.c) or a mis-orderable builder chain (F2.b). `repeating(responses)` is a thin, general convenience exactly parallel to the existing `text`/`texts` conveniences and reads cleanest at the many new test call-sites (the promoted P0 repro + the multi-turn close-gate). Both keep `new` one-shot, preserving back-compat by construction.

**Defers (if chosen)** (3 sentences): No further mock flags ship in KC-06; if future flags are wanted, a builder pattern can be added additively later without touching `with_repeat`. The i-phi-side `arm.repeat` config field + its wiring is the downstream chunk. No consumer-specific constructor logic enters the kernel.

#### F3 = F3.a — rotate-on-pop cursor internal *(pre-lock draft; finalizes at gate-1 lock)* — **TECHNICAL FORK (conditional on F1.a)**

**Code-level binding** (3 sentences): Inside the existing `stream` lock scope (`mock.rs:147-156`), the `else` branch (queue non-empty) becomes: `let front = responses.remove(0); if self.repeat { responses.push(front.clone()); } front` — a ~3-line change under the single `std::sync::Mutex` (thread-safe by construction; `MockResponse: Clone` at `mock.rs:52`). The `repeat == false` path is byte-identical to today's `responses.remove(0)`, and the empty→fallback branch (`mock.rs:149-151`) is untouched. No second interior-mutable state field is introduced.

**Rationale** (3 sentences): Rotate-on-pop is the smallest correct diff and, crucially, keeps the proven one-shot `remove(0)` path verbatim so no existing behaviour perturbs (P0 §5 A2 — the one-shot observable contract is unchanged). An index cursor (F3.b) would add a second piece of state needing its own ordering discipline and would change the one-shot path from `remove(0)` to indexed-read, perturbing the byte-identical path for no benefit. This body is coherent ONLY because F1 locks F1.a (flat-cycle); a USER-DIVERGENT F1.b lock makes the cursor question moot and re-opens this subsection.

**Defers (if chosen)** (3 sentences): Nothing is deferred — the rotate ships in-chunk. A non-destructive-read refactor remains available later as a private internal change with zero public-surface commitment. No behaviour is exposed that a consumer could depend on beyond "cycle the supplied queue."

#### F4 = F4.a — defer Option B (per-turn-script) *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): KC-06 ships ONLY the Option-A cycle flag (F1.a mechanism + F2.a API); no `[[provider.mock.turns]]` per-turn-unit type or queue restructure is added. The `[ToolCalls, Text]` per-turn ordering + the required terminal text remain the consumer's queue-composition responsibility (documented as the §D6.4 CONSUMER contract), not a kernel-owned turn abstraction. A future kernel chunk may add Option B on top of the cycle primitive with no rework of KC-06's surface.

**Rationale** (3 sentences): #121 explicitly recommends "Option A as the high-value minimal fix … Option B later," and the P0 REPRODUCED that cycle-flat-queue closes #121's full acceptance (N send-inputs → N Asks). Building Option B now would grow the kernel surface beyond what any consumer needs today, and the one hard constraint Option A imposes (an explicit terminal `Text`, P0 F5) is a downstream-config concern, not a reason to pull Option B into the kernel. Deferring keeps the kernel minimal and the incremental path clean.

**Defers (if chosen)** (3 sentences): Per-turn-script expressiveness (`[[provider.mock.turns]]`) is deferred to a future kernel chunk, unblocked by KC-06's cycle primitive. Input-derived generators (#121 Option C) are likewise out of scope. No per-turn-unit config surface enters the kernel.

---

## §1 — Context & principle

- **Why this chunk.** Today `MockProvider` (`src/provider/mock.rs`) consumes its response queue **destructively** — `stream` pops the front via `responses.remove(0)` (`mock.rs:153`) per LLM call, and when the queue empties it returns a `"(no more mock responses)"` text fallback with no tool calls (`mock.rs:149-151`). The P0 REPRODUCED the exact #121 "dead session": one interactive `send-input` consumes **exactly 2** queue items `[ToolCalls, terminal]` (because the turn-termination gate at `run.rs:531` — `if !has_tool_calls && pending.is_empty() { break; }` — does NOT break on a tool round, forcing a second `stream()` for the terminal round), so a session whose queue holds at most one `ToolCalls` re-parks nothing on its second turn. KC-06 gives `MockProvider` an OPT-IN cycle-flat-queue repeat mode: when `repeat` is on and the queue would exhaust, it re-emits from the start (cycle) instead of the fallback, so N `send-input`s to one session sustain N interactive Asks — closing #121's kernel half.
- **Kernel-minimality restatement.** phi-core owns the MECHANISM (a general test-double cycle primitive with ZERO consumer-specific logic — `MockProvider` is phi-core's OWN test double, used by 133 in-crate sites and by every consumer's tests). The consumer (i-phi, later) owns the POLICY: the `[provider.mock] repeat` TOML field, the `mock_responses_as_responses` `[ToolCalls, Text]` queue ordering, and the terminal-text emission. Per `[[feedback_phi_core_kernel_minimal]]`: KC-06 adds only a genuinely-general primitive; no i-phi/consumer concept leaks into the kernel; additive public API; no migration/breaking change.
- **Authoritative inputs.** Committed P0 (`_p0-investigations/kc-06-mock-provider-repeat-mode-p0-investigation.md`, 10 DEFINITIVE) + i-phi #121. Grounded on `dev` HEAD `929d326`.

## §2 — Concept alignment walk

| Concept / spec doc | § anchor | Claim (paraphrase) | Status at chunk-open | Target at chunk-close |
|---|---|---|---|---|
| `src/provider/mock.rs` module rustdoc | `mock.rs:2-34` "ARCHITECTURE / Usage pattern" | Documents the queue + destructive consume + "third call → `(no more mock responses)` fallback." | honored (matches code) | honored — extended: rustdoc documents the opt-in repeat/cycle mode + the CONSUMER contract (`[ToolCalls, Text]` ordering + terminal-text-required) |
| `docs/specs/developer/provider.md` | `:192` (MockProvider row) + `:200` (`provider_override` note) | Lists `MockProvider (testing)` as the test double behind `provider_override`. | honored | honored — extended: one-line repeat-mode note added; verified-header bumped |
| `docs/reference/api.md` | `:86` (`with_provider_override`) | `provider_override` is used "for testing with `MockProvider`." | honored | honored (unchanged — incidental `provider_override` reference, no MockProvider ctor docs) |
| `docs/reference/configuration.md` | `:12` | `provider_override` "Use for MockProvider in tests." | honored | honored (unchanged — incidental reference) |
| `docs/concepts/agent-loop.md` | `:80` / `:116` | `provider_override` "used for MockProvider in tests." | honored | honored (unchanged — incidental reference) |

Rules: phi-core has no permissions subtree, no baby-phi `phi-core-mapping.md` hook (this IS phi-core). Every doc whose claims the code touches is listed; each ends `honored` (additive, no contradiction). The load-bearing doc surface is the `mock.rs` module rustdoc (documents the exhaustion behaviour KC-06 makes opt-in-cycle-able).

## §2.5 — Functional outcome

**Chunk-type**: TECHNICAL-PREREQUISITE (kernel test-double primitive; ships the cycle mechanism, not the i-phi config policy that a test author flips).
**User-visible delivery**: NONE directly this chunk — `repeat` defaults OFF, so every existing mock session + all 133 construction sites are byte-behaviour-identical. The capability (one mock session sustaining `send → Ask → approve → send → Ask …` across N turns) becomes available to consumers but is not turned on anywhere here.
**Unblocks**: the thin **downstream i-phi chunk** that adds `[provider.mock] repeat`, re-points `build_mock_provider` at `MockProvider::with_repeat(queue, arm.repeat)`, reorders `mock_responses_as_responses` to emit the `[ToolCalls, Text]` unit, and emits the terminal text — that chunk is where a test author/operator gets the visible "live multi-turn demo session."
**Why this prerequisite**: `MockProvider` is phi-core's kernel test double; the destructive-consume + cycle mechanism lives in the kernel (Rust coherence + kernel-minimality), and the i-phi config→queue mapping can only be built once the kernel exposes the `with_repeat` API.

## §3 — Kernel-minimality surface-discipline map (phi-core-leverage-check N/A) + cascade discipline

> **phi-core-leverage-check is N/A** — the kernel does not consume itself. Substituted with the kernel-minimality surface check per `[[feedback_phi_core_kernel_minimal]]`: verify KC-06 adds only a genuinely-general test-double primitive with ZERO i-phi/consumer policy logic. **k8s-readiness-check is N/A** (phi-core is a library, not a daemon). No `scripts/check-*.sh` exist.

**Kernel-minimality surface check (the substitute for §3 leverage):**

| New kernel surface | General primitive? | Consumer-policy leakage? | Verdict |
|---|---|---|---|
| `MockProvider.repeat` private `bool` field (`provider/mock.rs`) | Yes — a general test-double toggle; every consumer's tests benefit. | None — defaults OFF; no i-phi concept. | general |
| Rotate-on-pop cycle in `stream` (`mock.rs:147-156`) | Yes — cycles whatever `Vec<MockResponse>` is supplied, order-agnostic. | None — the WHEN/WHAT-ordering is the consumer's queue (F1.a). | general |
| `with_repeat(responses, repeat)` constructor | Yes — additive ctor mirroring `new`. | None. | general |
| `repeating(responses)` convenience | Yes — mirrors `text`/`texts` convenience precedent (`mock.rs:82-104`). | None. | general |

**Forbidden-leakage grep (must return 0):** `grep -rniE "i-phi|iphi|\[provider|toml|daemon|per.agent|mock_responses_as_responses|arm\.repeat" /root/projects/phi/phi-core/src/provider/mock.rs` — expect **0** consumer/TOML references in the new surface. (The field/param name `repeat` and the word "cycle" are kernel-general and NOT flagged.)

### §3 cascade discipline — the ONLY production file changed is `src/provider/mock.rs`

Unlike KC-05 (which cascaded a new `AgentLoopConfig` field to 31 sites), KC-06 adds a private field to a struct that is **NOT constructible externally by struct-literal** (the `responses` field is private; all construction flows through `new`/`text`/`texts`/`with_repeat`/`repeating`). So the new field cascades to **ZERO external call-sites** — only the in-file constructor bodies set `repeat: false`.

- **(a) Invocation**: `grep -rnE "MockProvider::(new|text|texts)\b" /root/projects/phi/phi-core/src /root/projects/phi/phi-core/tests /root/projects/phi/phi-core/examples`
- **(b) Raw count**: **133** call-sites (grep-verified at plan-draft; P0 §3 F7 cited 142 via a different pattern — the exact figure is NOT load-bearing; the invariant is "all stay byte-identical").
- **(c) Struct-literal `MockProvider {` outside def/impl**: **0** (grep-verified — the only `MockProvider {` hits are the struct def `mock.rs:68` + `impl` blocks `mock.rs:73`/`:108`; `CapturingMockProvider` in `turn_request_capture_test.rs`/`release_0_10_test.rs` are SEPARATE test doubles, unaffected).
- **Predicted call-site edits**: **0**. **PAUSE if any existing `MockProvider::{new,text,texts}` call-site requires an edit** (>0 would signal a signature change accidentally slipped in — a back-compat break). Predict **ZERO RED** across the whole existing suite.

### §3 pause-threshold table (per-fork)

| Fork | If locked | Key-file LOC cap | Cascade note |
|---|---|---|---|
| F1.a (rec) | cycle-flat-queue | `provider/mock.rs` +≤ 15 (field + rotate + ~4 lines rustdoc) | rotate-on-pop; empty→fallback untouched |
| F1.b | per-turn-unit | `provider/mock.rs` +≤ 60 **+** new `TurnUnit` type + queue restructure | NOT recommended (over-scope) |
| F2.a (rec) | `with_repeat` + `repeating` | `provider/mock.rs` +≤ 20 (2 ctors + doc-comments) | mirrors `text`/`texts` |
| F3.a (rec) | rotate-on-pop | included in F1.a cap | one-shot path byte-identical |
| F4.a (rec) | defer Option B | 0 | no per-turn-unit surface |

**Substrate-module LOC calibration (v35 P-plan-2)**: `mock.rs` carries a large module rustdoc block + will gain inline `#[cfg(test)]` back-compat tests → apply ~1.3× the plain-struct baseline to the *inline-test* portion. Total production+rustdoc delta on `mock.rs` predicted **≤ 40 LOC**; new integration test file `tests/mock_repeat_test.rs` predicted **~120–160 LOC** (multi-turn loop harness + 4–5 loop-driven tests, incl. `tokio::test` runtime boilerplate per v33 P-plan-1-v33 framework-boilerplate allowance ~30 LOC/async test — this is test-envelope LOC, not functional-scope). **Pause any single production file > 1.5× its cap** and AskUserQuestion.

## §3.B — K8s microservice readiness check

**N/A — phi-core is a library, not a daemon; K8s readiness is not applicable** (k8s-readiness-check returns N/A for phi-core). No in-process daemon state owned here, no IPC channels, no migration runner, no audit hash-chain. All 7 axes: **N/A**. No `CHK8S-D-NN` ledger entry.

## §3.C — User-facing documentation impact map

phi-core doc tree is `docs/{concepts,architecture,specs,reference}/`. The load-bearing MockProvider documentation is the **source rustdoc**, not a standalone `.md`:

| Tier | File | Touches? | Action |
|---|---|---|---|
| Source rustdoc (primary) | `src/provider/mock.rs` module doc-comment (`:2-34`) | Yes — the "Usage pattern" block documents the exhaustion fallback | (a) update in-chunk (P-DOCS): document the opt-in `repeat`/cycle mode via `with_repeat`/`repeating`, the CONSUMER contract (`[ToolCalls, Text]` ordering + terminal-text-required so a `[ToolCalls]`-only cycle isn't an unbounded loop), and that one-shot stays the default |
| Developer spec | `docs/specs/developer/provider.md` (`:192` MockProvider row) | Yes (light) | (a) add a one-line repeat-mode note beneath the MockProvider row; verified-header bumped |
| Reference | `docs/reference/api.md` (`:86`) / `configuration.md` (`:12`) | No | no change — incidental `provider_override` references only; no MockProvider ctor documentation to drift |
| Concept | `docs/concepts/agent-loop.md` (`:80`/`:116`) | No | no change — incidental `provider_override` reference only |
| ADR | `docs/decisions/0006-mock-provider-repeat-mode.md` | Yes — NEW ADR | (a) author in-chunk (P-ADR) |

**Doc-LOC threshold (semantic-completeness escape, v29 P-plan-7)**: the `mock.rs` rustdoc update lands `≥ ~15 LOC OR carries all 3 content axes` (repeat-mode ctor usage example + `[ToolCalls, Text]` ordering + terminal-text-required contract). `provider.md` note is a 1-line addition. Verified-headers refreshed on `provider.md`. No defer decisions — every stale doc surface is updated in-chunk at P-DOCS.

## §3.E — Anticipated gate-2.5 candidates

- **Test-file placement.** The multi-turn / cadence tests need the full agent loop (`agent_loop` + `agent_loop_continue`) → they land in a NEW `tests/mock_repeat_test.rs`; the pure-provider back-compat + API-equivalence tests can be inline `#[cfg(test)]` in `mock.rs`. If the implementer finds it cleaner to co-locate all in one place, that is a Route-A absorb (mechanism choice within scope), NOT a gate-1 fork.
- **`repeating` convenience inclusion.** If the orchestrator/user prefers the tighter single-ctor surface (F2.c), dropping `repeating` is a ≤ 5-LOC trim → Route-A absorb. Planner-rec ships both (parallel to `text`/`texts`).
- **No other candidates anticipated** — no placeholder returns, no deferred-but-shipping doc-comments, no `Default`-impl cascade (the field is private).

## §4 — Issues / drifts closed + deferred functionality

| Issue/drift | Severity | Transition | Notes |
|---|---|---|---|
| i-phi `#121` (mock provider multi-turn/repeat) | enhancement | **kernel half closed** at chunk-close | KC-06 ships the phi-core `MockProvider` cycle-flat-queue primitive + `with_repeat`/`repeating` API. #121 stays **OPEN** pending the downstream i-phi consumer chunk (config field + queue reorder + terminal-text emission + the end-to-end live close-gate). The KERNEL close-gate (§8/§10) proves the mechanism; #121 is not fully closed until the i-phi chunk lands. |

**No phi-core drift files are opened or closed** (phi-core tracks defects via GitHub issues, not a `drifts/` tree). Any NEW defect KC-06 surfaces is filed as its own GitHub issue (mirrored per `[[feedback_dtest_github_mirror]]` if a D-TEST class).

**Deferred functionality (user-facing translation):**

| Item | User-visible feature deferred | Impact during deferral | Allocation |
|---|---|---|---|
| i-phi `[provider.mock] repeat` config + queue reorder + terminal-text | A test author/operator driving one live mock session through N interactive Asks (`send → Ask → approve → send …`) | Repeat mode ships in the kernel OFF; no i-phi session changes until the downstream chunk wires the flag + `[ToolCalls, Text]` ordering | **Downstream i-phi chunk** (new i-phi issue/chunk; consumes the published `with_repeat` API + owns the live end-to-end close-gate per `[[feedback_render_transcript_close_gate]]` Rule 6) |
| Option B `[[provider.mock.turns]]` per-turn script (#121 Option B) | Scripted realistic multi-turn conversations (distinct per-turn tool-calls) | Only repeating-unit cycling available; distinct per-turn scripts unavailable | Future kernel chunk (F4.a defer; slots on top of KC-06's cycle primitive) |
| Option C input-derived generator (#121 Option C) | Mock tool-calls derived from the input (echo / per-turn counter) | Not available | Future kernel chunk (out of scope) |

## §5 — ADRs drafted

- **ADR number**: **ADR-0006** — `docs/decisions/0006-mock-provider-repeat-mode.md` (0001–0005 exist; 0006 next, verified via `ls docs/decisions/`).
- **Title**: "MockProvider opt-in repeat/cycle mode (kernel test-double primitive; cycle-flat-queue, back-compat-by-default)".
- **Drafted-at-phase**: P-ADR (seal). Status `Proposed` at draft → `Accepted` at chunk-seal.
- **Decision summary**: phi-core ships an opt-in cycle-flat-queue repeat mode on `MockProvider` (private `repeat` field + rotate-on-pop cycle + `with_repeat`/`repeating` ctors); one-shot stays the byte-identical default; the queue is cycled VERBATIM so `[ToolCalls, Text]` ordering + the terminal text are the CONSUMER's responsibility; Option B deferred; additive → minor semver 0.13.0.
- **Prior-ADR precedent path convention**: phi-core ADRs live flat under `docs/decisions/` (no milestone-prefix needed — the baby-phi milestone-prefix rule does not apply to phi-core).

**ADR-0006 top-level section enumeration (implementer authors ALL 7, mirroring ADR-0004's shape):**
1. `## Forks` — header table capturing the gate-1 lock-state for F1–F4 (Direct-approval vs Divergent form per outcome; note F2/F3 are TECHNICAL).
2. `## Context` — P0 §4 fix-locus (phi-core kernel-only additive) + #121 + the `run.rs:531` 2-items-per-turn mechanic + the destructive-consume root cause.
3. `## Sub-decisions` — one `### §D6.<M>` per fork resolution + supporting decisions:
   - §D6.1 cycle-flat-queue mechanism (F1.a) — **Pre-existing absence preserved** (never-shipped-yet, v24 P-plan-2: "no prior repeat behaviour to preserve — net-new opt-in cycle mode; one-shot destructive-consume is unchanged on the default path").
   - §D6.2 `with_repeat` + `repeating` additive ctors (F2.a) — **Pre-existing-behaviour preserved**: `new`/`text`/`texts` signatures + observable behaviour unchanged (bodies set `repeat: false`); 133 call-sites byte-identical.
   - §D6.3 rotate-on-pop internal (F3.a) — **Pre-existing-behaviour preserved**: the one-shot `remove(0)` path (`mock.rs:153`) + the empty→fallback (`mock.rs:149-151`) are byte-identical for `repeat == false`.
   - §D6.4 **terminal-text-required CONSUMER contract** (P0 F5/§5 A3) — the kernel cycles VERBATIM and does NOT auto-synthesize a terminal text; under repeat the empty→fallback is bypassed, so a `[ToolCalls]`-only cycle is an UNBOUNDED tool loop halted only by `max_turns`; the repeating unit MUST include a terminal (non-tool) `Text`, and `[ToolCalls, Text]` ordering is the consumer's responsibility. **Document this prominently so the downstream i-phi chunk honors it.** Net-new contract.
   - §D6.5 defer Option B / Option C (F4.a) — net-new scope decision; #121 "Option A now, B later."
   - §D6.6 semver 0.12.0 → 0.13.0 (additive → minor) — note 0.13.0 also covers KC-05's additive surface (KC-05 landed at `133ad33` without a separate bump); `cargo publish` is post-seal orchestrator/user-owned.
   - §D6.7 kernel-minimality boundary — phi-core owns the general cycle primitive; the i-phi `[provider.mock] repeat` config + `mock_responses_as_responses` `[ToolCalls, Text]` ordering + terminal-text emission is the downstream consumer, NOT here.
4. `## Cross-references` — (a) source rustdoc `provider/mock.rs:2-34` + `docs/specs/developer/provider.md:192`; (b) closed issue: i-phi #121 (kernel half; stays open for the i-phi consumer chunk); (c) prior ADRs cited: `docs/decisions/0004-selectable-skill-prompt-layout.md` (kernel-minimality boundary precedent — phi-core owns the primitive, i-phi owns the config) + `0003-revert-tail-shrink-contract.md` (kernel-lane additive / investigation-first precedent); (d) forward-scope/committed P0 path.
5. `## Consequences` — `### For the downstream i-phi #121 consumer chunk` (inherits the `with_repeat` API to call from `build_mock_provider`; MUST supply `[ToolCalls, Text]` ordering + terminal text per §D6.4; owns the live end-to-end close-gate) + `### For phi-core + other consumers` (repeat mode available for free on the next tracked commit; all existing tests unaffected).
6. `## Revisit triggers` — 3–7 bullets, e.g.: a consumer needing distinct per-turn scripts → re-open §D6.5 (build Option B); a consumer wanting kernel-guaranteed cadence (not consumer-supplied ordering) → re-open §D6.1/§D6.4; the rotate-on-pop `Clone`-per-turn proving costly for very large mock queues → re-open §D6.3 (index cursor); a future need to auto-synthesize a terminal text → re-open §D6.4 (weigh against the kernel-minimality leak).
7. `## Verification` — the §12 commands: full `cargo test` at [617, 619]; the multi-turn close-gate test; the back-compat one-shot test; clippy `-Dwarnings` + fmt; the forbidden-leakage grep = 0; the `MockProvider::{new,text,texts}` call-site count unchanged.

## §6 — Prior-chunk regression re-verification

| Upstream | Invariant relied on | Re-verification |
|---|---|---|
| Existing `MockProvider` behaviour (all consumers' tests) | One-shot destructive-consume unchanged; empty→`"(no more mock responses)"` fallback unchanged for `repeat == false` | `cargo test -j 4 --manifest-path .../Cargo.toml` green at [617, 619]; `test_tool_call_and_response` (`agent_loop_test.rs:137`) + `test_continue_from_tool_result` (`:284`) stay green |
| 133 `MockProvider::{new,text,texts}` call-sites | Signatures + observable behaviour byte-identical (private-field addition only) | `git diff HEAD -- src tests examples \| grep -E '^[-+].*MockProvider::(new\|text\|texts)'` shows ZERO removed/edited call-sites (only new `with_repeat`/`repeating` additions) |
| KC-05 (progressive tool disclosure, `133ad33`) | Orthogonal surface (`AgentLoopConfig` / `streaming.rs` / `tool_help.rs`); MockProvider untouched by KC-05 | no code overlap; KC-05's +21 tests stay green |
| `StreamProvider` trait contract | `MockProvider::stream` signature unchanged; other provider impls untouched | `cargo build --all-targets` compiles; `anthropic.rs`/`openai_compat.rs`/etc. unchanged |

**Carry-forward INVERSION check (predicate-anchored, per KC-03 #4):** **NONE.** KC-06 is purely additive — no existing test asserts a disposition that KC-06 inverts. The one-shot default is preserved verbatim, so no `MockProvider`-consuming test flips. **Predict ZERO RED.** (Contrast KC-05, which inverted 3 `revert.rs` description tests.)

**Carry-forward invariants verified green at chunk-open:**
- `cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml` baseline = **610** (grounded via full run at HEAD `929d326`; +5 ignored live-API).
- `RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --manifest-path .../Cargo.toml --all-targets` clean.
- `cargo fmt --manifest-path .../Cargo.toml -- --check` clean.
- `git -C /root/projects/phi/phi-core diff HEAD -- src/` empty (no preload edits).

## §7 — Phases within the chunk

### §7.0 — Phase-order stress-test (v25 P-plan-4)

5 phases; no coupled divergent locks; no migration (phi-core is a library); a single production file (`mock.rs`) touched. **Per-boundary check**: P1 lands the private field + `with_repeat`/`repeating` ctors + rotate-on-pop cycle together (the field, ctors, and `stream` branch MUST land in the SAME phase or `mock.rs` won't compile) → `cargo build --all-targets` green at P1 close. P2 adds tests against the P1 API (no production change) → additive-only, no RED window. P-DOCS + P-SEAL are paperwork. **No compile-time or runtime RED window is possible**: KC-06 is additive with `repeat` OFF by default, so the existing suite stays green from P1 onward (the new field/ctors don't touch any existing call-site). Workspace-RED risk = **zero**.

### P0 — Pre-chunk gate + HEAD confirm
- **Goal**: verify §9 reading list read + carry-forward invariants green + baseline 610; confirm `dev` HEAD unchanged since P0 (`929d326`).
- **Deliverables**: none (verification only).
- **Tests**: baseline `cargo test` = 610.
- **Confidence**: 100%.

### P1 — MockProvider repeat mode: field + ctors + rotate-on-pop cycle
- **Goal**: add the opt-in cycle mechanism; `repeat` defaults OFF; all 133 sites + the whole existing suite stay green.
- **Deliverables**: (1) `provider/mock.rs` — private `repeat: bool` field on `MockProvider`; `new`/`text`/`texts` bodies set `repeat: false` (signatures unchanged); (2) `pub fn with_repeat(responses, repeat: bool)` + `pub fn repeating(responses)` additive ctors (F2.a); (3) rotate-on-pop in `stream` (`mock.rs:147-156`): on `repeat == true` & non-empty, `remove(0)` then `push(front.clone())`; `repeat == false` path byte-identical; empty→fallback untouched (F1.a + F3.a).
- **Tests**: Tier A back-compat inline `#[cfg(test)]` in `mock.rs` — `new`/`with_repeat(_, false)` still one-shot; Tier F API-equivalence — `repeating(r)` ≡ `with_repeat(r, true)`, `with_repeat(r, false)` ≡ `new(r)`.
- **Concept-alignment**: `mock.rs` rustdoc row transitions (in P-DOCS). **Kernel-minimality**: surface rows 1–4; forbidden-leakage grep = 0.
- **Confidence**: ≥ 98%. **Pause discipline**: PAUSE if any existing `MockProvider::{new,text,texts}` call-site requires an edit (back-compat break) OR if any existing test flips (predict ZERO RED).

### P2 — Repeat-mode regression + kernel close-gate (multi-turn) tests
- **Goal**: promote the 4-test P0 repro against the REAL `with_repeat`/`repeating` API + land the multi-turn close-gate.
- **Deliverables**: NEW `tests/mock_repeat_test.rs` (loop-driven): (1) **CLOSE-GATE** — `with_repeat([ToolCalls, Text], true)` driven through ≥ 3 turns (one `agent_loop` + ≥ 2 `agent_loop_continue`, using the `test_tool_call_and_response` template at `agent_loop_test.rs:137` + the `agent_loop_continue` setup at `:284`) asserting per-turn tool-call counts `[1, 1, 1]` (the DISPOSITION, not merely "no panic"); (2) exhaustion + 2-items-per-turn (one-shot `[ToolCalls, Text]` exhausts turn 1 → turn 2 hits `"(no more mock responses)"`, 0 tool calls); (3) **terminal-text-required contract** — `with_repeat([ToolCalls], true)` runs an unbounded tool loop halted only by `max_turns` (assert the loop is bounded, proving the §D6.4 contract); (4) **cadence-vs-queue-shape** — `[ToolCalls,Text]→[1,1,1,1]`, `[Text,ToolCalls]→[0,1,1,1]`, `[Text,ToolCalls,Text]→[0,1,0,1]` (documents the CONSUMER ordering contract in a test).
- **Tests**: Tiers B/C/D/E (see §8).
- **Confidence**: ≥ 97%. **Pause discipline**: PAUSE if the multi-turn test does NOT observe `[1,1,1]` (would falsify the P0 mechanism — investigate before proceeding, do not weaken the assertion).

### P-DOCS — Doc sync + verified-headers
- **Goal**: update the §3.C doc surfaces + refresh verified-headers.
- **Deliverables**: `provider/mock.rs` module rustdoc (repeat-mode usage + `[ToolCalls, Text]` ordering + terminal-text-required contract + one-shot default); one-line repeat-mode note at `docs/specs/developer/provider.md:192` + verified-header bump.
- **User-facing doc updates**: §3.C rows (source rustdoc + `provider.md`).
- **Confidence**: ≥ 99%.

### P-ADR / P-SEAL — ADR-0006 Accepted + version bump + cycle-index row
- **Goal**: author ADR-0006 (7 sections), flip `Accepted`; bump the crate version; record the cycle-index row.
- **Deliverables**: (1) ADR-0006 (`docs/decisions/0006-mock-provider-repeat-mode.md`); (2) `Cargo.toml` version `0.12.0 → 0.13.0` (`Cargo.toml:3`; additive-API MINOR bump — makes the crate publish-ready; **`cargo publish` is a POST-SEAL orchestrator/user step, crates.io token not in this env — NOT scripted here**; note the downstream follow-on i-phi bump-to-0.13 chunk); (3) `_cycle-index.md` row — leave `Iterations = pending`, `Status = in-flight` (orchestrator owns the transitions per the row-lifecycle paragraph: gate-3 → ready-for-audit; gate-4 close → audited-pending-retro; Phase 6/7 close → retro-complete + Iterations to final count).
- **Confidence**: ≥ 99%.

## §8 — Tests summary

**Baseline (baseline-snapshot Axis A)**: **610** passing (+5 ignored live-API) at HEAD `929d326`, grounded via full `cargo test -j 4`. phi-core has no binary/inline split — all tests run via `cargo test` (inline `#[cfg(test)]` + `tests/*.rs` integration).

**Per-Tier MUST-SHIP breakdown:**
| Tier | Coverage | Location | Tests |
|---|---|---|---|
| A | back-compat: `new` / `with_repeat(_, false)` still one-shot (exhaust → fallback) | inline `mock.rs` | 2 |
| B | exhaustion + exactly-2-items-per-turn (F1/F2 root-cause repro) | `tests/mock_repeat_test.rs` | 1 |
| C | **CLOSE-GATE** cycle sustains ≥ 3 turns → `[1,1,1]` tool calls | `tests/mock_repeat_test.rs` | 1 |
| D | terminal-text-required: `[ToolCalls]`-only cycle = unbounded loop bounded by `max_turns` | `tests/mock_repeat_test.rs` | 1 |
| E | cadence-vs-queue-shape (CONSUMER ordering contract): `[1,1,1,1]` / `[0,1,1,1]` / `[0,1,0,1]` | `tests/mock_repeat_test.rs` | 1 |
| F | API-equivalence: `repeating(r)` ≡ `with_repeat(r, true)`; `with_repeat(r, false)` ≡ `new(r)` | inline `mock.rs` | 1 |
| **Total NEW MUST-SHIP** | | | **7** |

**Accept band**: `[610 + 7, 610 + 7 + inline-overshoot-allowance]` = **[617, 619]**. Predicted final ≈ **617**. Inline-overshoot allowance (~2) = incidental helper-driven inline unit tests in `mock.rs`. Outside band → AskUserQuestion. No MAY-COVER tier beyond the inline allowance (this is a tightly-scoped provider change).

**Named test files / locations**: NEW `tests/mock_repeat_test.rs` (Tiers B/C/D/E — loop-driven); inline `#[cfg(test)] mod tests` in `src/provider/mock.rs` (Tiers A/F — pure provider/API). **Named expected-still-green (grep-verified)**: `agent_loop_test.rs` `test_tool_call_and_response` (`:137`) + `test_continue_from_tool_result` (`:284`) + `test_tool_error_is_reported` (`:326`) stay green (one-shot default unchanged).

**Kernel close-gate (`[[feedback_render_transcript_close_gate]]` Rule 8 — artifact = where the fix manifests):** the fix manifests in `MockProvider`'s per-turn output, NOT on a live daemon wire, so the KERNEL close-gate is the **Tier C multi-turn integration test** (drive a repeating `MockProvider` through ≥ 3 `agent_loop`/`agent_loop_continue` turns, ASSERT each turn re-emits exactly one tool call → `[1,1,1]` — the disposition, not "no panic"). **The END-TO-END daemon validation** (boot i-phi with `[provider.mock] repeat=true`, send N messages to one session, confirm N interactive Asks park, RENDER + READ the transcript) is the **DOWNSTREAM i-phi consumer chunk's** close-gate — it depends on the i-phi config field + `[ToolCalls, Text]` reorder KC-06 does NOT ship — and is explicitly OUT of scope here (that chunk OWNS it as load-bearing per `[[feedback_render_transcript_close_gate]]` Rule 6).

## §9 — Pre-chunk gate

**Reading list (mandatory):**
1. Committed P0 `_p0-investigations/kc-06-mock-provider-repeat-mode-p0-investigation.md` (§2 surface map, §3 findings F1–F10, §4 fix-locus, §5 non-viable A1–A3, §6 forks, §8 close-gate).
2. i-phi #121 body (Options A/B/C + the "one interleaving nuance" + acceptance criteria) — `bash /root/projects/phi/.claude/scripts/gh-rest.sh issue-pull 121`.
3. `src/provider/mock.rs` (full — struct `:68`, ctors `:75-104`, `stream` consume site `:147-156`, `match response` `:180-246`; `MockResponse` Clone derive `:52`); `src/agent_loop/run.rs:525-535` (the `if !has_tool_calls && pending.is_empty() { break; }` termination gate); `tests/agent_loop_test.rs:136-245` (`test_tool_call_and_response` template) + `:283-323` (`agent_loop_continue` setup).
4. `[[feedback_phi_core_kernel_minimal]]`, `[[feedback_render_transcript_close_gate]]` Rules 6/8, `[[feedback_never_hedge]]`.
5. phi-core `CLAUDE.md` (Documentation Alignment + Testing / MockProvider conventions).

**Carry-forward invariants (green at open)**: baseline 610; clippy `-Dwarnings` clean; `fmt --check` clean; `src/` diff vs HEAD empty.

**Pending decisions carried in**: confirm F1.a (cycle-flat-queue) + F2.a (`with_repeat` + `repeating`) + F3.a (rotate-on-pop) + F4.a (defer Option B).

## §10 — Close criteria

**4 aspects (pass/fail):**
- **Code**: all P1/P2 deliverables shipped; `cargo test -j 4 --manifest-path .../Cargo.toml` green at [617, 619]; `RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --all-targets` clean; `fmt --check` clean; `Cargo.toml` version `0.13.0`.
- **Docs**: `mock.rs` rustdoc + `provider.md` updated in-chunk; verified-header bumped; ADR-0006 `Accepted`.
- **Kernel-minimality** (substitute for phi-core-leverage aspect): forbidden-leakage grep returns 0; only a general test-double primitive added; additive public API; no breaking/migration change; 133 `MockProvider::{new,text,texts}` call-sites byte-identical (0 edited); `MockProvider::stream` signature unchanged.
- **Concept alignment**: every §2 row `honored` at close; none `contradicted`.

**2 confidence %:**
- **Implementation confidence** = claims-honored / claims-in-scope. **Target ≥ 9/10** (≈ 7/7 test-backed MUST-SHIP claims + back-compat invariant). The mechanism is P0-REPRODUCED, so no unresolved in-scope gap.
- **Documentation confidence** = doc-surfaces cross-checkable / touched. Target 2/2 (rustdoc + `provider.md`).

**Kernel close-gate (`phi-core`-lane, `[[feedback_render_transcript_close_gate]]` Rule 8):** the Tier C multi-turn integration test (repeating `MockProvider` over ≥ 3 turns → `[1,1,1]` tool calls, asserting the DISPOSITION) IS the close-gate. **The end-to-end daemon live validation is the DOWNSTREAM i-phi chunk's close-gate, NOT this one** — stated explicitly so the i-phi chunk owns it as load-bearing (Rule 6).

## §11 — Post-chunk independent audit plan

**Envelope (audit-envelope-size)**: 5 phases → **Medium (2 auditors A + B)**. Justification: small, single-production-file additive kernel change (favors Small/1), BUT the "terminal-text-required / unbounded-loop" CONSUMER contract (§D6.4) is a subtle correctness edge + there is a NEW ADR + a version bump + a multi-turn close-gate to verify → split code/back-compat (A) from concept/contract/docs/ADR (B). Medium is the right size.

- **Audit A — code + tests + kernel-minimality + back-compat**: read-only on source. (1) `provider/mock.rs` private `repeat` field + `with_repeat`/`repeating` ctors + rotate-on-pop in `stream` (`:147-156`); `repeat == false` path byte-identical to `remove(0)`; empty→fallback untouched. (2) 133 `MockProvider::{new,text,texts}` call-sites unchanged (`git diff HEAD` shows 0 edited); `stream` signature unchanged. (3) Tier A back-compat + Tier F API-equivalence inline tests present + assert one-shot default. (4) forbidden-leakage grep = 0 (`i-phi|iphi|\[provider|toml|daemon|mock_responses_as_responses`). (5) `Cargo.toml` version `0.13.0`. (6) `cargo test` green at [617,619]; clippy `-Dwarnings` + fmt clean (mark `NOT-EXECUTED-IN-AUDIT` — orchestrator closes at gate-4). PASS/FAIL each. ≤ 600 words.
- **Audit B — concept/contract + close-gate + docs + ADR**: read-only. (1) **CLOSE-GATE** Tier C multi-turn test drives ≥ 3 turns → asserts `[1,1,1]` tool calls (the disposition). (2) Tier D terminal-text-required test proves `[ToolCalls]`-only cycles unbounded (bounded by `max_turns`); Tier E cadence-vs-queue-shape documents `[1,1,1,1]`/`[0,1,1,1]`/`[0,1,0,1]`. (3) ADR-0006 `Accepted` with all 7 sections + §D6.1–§D6.7 + the §D6.4 CONSUMER contract documented prominently + pre-existing-behaviour notes. (4) `mock.rs` rustdoc + `provider.md:192` updated; verified-header bumped; `[EXISTS]`-equivalent tags current. (5) `_cycle-index.md` row present (`grep -n 256ff295`). (6) #121 correctly recorded as **kernel-half-closed / stays open for the i-phi consumer chunk**; the end-to-end daemon close-gate correctly attributed to the downstream i-phi chunk. PASS/FAIL each. ≤ 600 words.

## §12 — Verification recipe

> Baseline for chunk-delta greps = the pre-chunk `dev` HEAD (`git diff HEAD`), NOT `main`. phi-core: host cargo, single crate, NO `--workspace`, NO CI-guard scripts.

```bash
# 1. Workspace health (the MUST-RUN gate — no check-*.sh exist for phi-core)
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# 2. The kernel close-gate (multi-turn) + repeat regression suite
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --test mock_repeat_test -- --nocapture

# 3. Kernel-minimality: zero consumer-policy leakage in the new surface (expect 0)
grep -rniE "i-phi|iphi|\[provider|toml|daemon|per.agent|mock_responses_as_responses|arm\.repeat" /root/projects/phi/phi-core/src/provider/mock.rs

# 4. Back-compat: no existing MockProvider::{new,text,texts} call-site edited (expect empty)
git -C /root/projects/phi/phi-core diff HEAD -- src tests examples | grep -E '^[-+].*MockProvider::(new|text|texts)'

# 5. Back-compat: call-site count unchanged vs plan-draft 133
grep -rnE "MockProvider::(new|text|texts)\b" /root/projects/phi/phi-core/src /root/projects/phi/phi-core/tests /root/projects/phi/phi-core/examples | wc -l

# 6. Semver bump landed
grep -n '^version = "0.13.0"' /root/projects/phi/phi-core/Cargo.toml

# 7. ADR + cycle-index
ls /root/projects/phi/phi-core/docs/decisions/0006-mock-provider-repeat-mode.md
grep -n "256ff295" /root/projects/phi/phi-core/docs/specs/plan/build/_cycle-index.md
```

**Downstream (NOT KC-06 — the i-phi consumer chunk's own close-gate):** boot i-phi with `[provider.mock] repeat = true` + an agent with `ask = ["Bash(*)"]` + `interactive=true`; send N `send-input`s to ONE session; confirm N interactive Asks park; RENDER + READ the daemon log / transcript to ASSERT each `routed Input command` is followed by a `permissions::engine: permission decision`. That chunk consumes `MockProvider::with_repeat` + emits the `[ToolCalls, Text]` ordering + terminal text per §D6.4; it OWNS the end-to-end validation as load-bearing (`[[feedback_render_transcript_close_gate]]` Rule 6). Post-seal, phi-core 0.13.0 is published to crates.io (orchestrator/user; token not in this env) and the i-phi chunk bumps its `phi-core` dep to 0.13.
