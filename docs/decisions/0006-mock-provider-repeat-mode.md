<!-- Last verified: 2026-08-26 by Claude Code (KC-06 / 0.13.0 — phi-core ADR-0006; MockProvider opt-in repeat/cycle mode: cycle-flat-queue, back-compat-by-default; kernel half of i-phi #121; §D6.4 = terminal-text-required CONSUMER contract) -->

# phi-core ADR-0006 — MockProvider opt-in repeat/cycle mode (kernel test-double primitive; cycle-flat-queue, back-compat-by-default)

**Status: Accepted**

> Sixth phi-core ADR. Ships the **kernel half** of i-phi **#121** (mock provider multi-turn / repeat) — an OPT-IN cycle-flat-queue repeat mode on `MockProvider` so a single scripted test/demo session re-emits its tool call(s) on every turn. Cite sub-decisions as `ADR-0006 §D6.<M>`. **Purely additive, back-compat**: `repeat` defaults OFF, so `new`/`text`/`texts` and all 133 in-crate construction sites stay byte-behaviour-identical; the one-shot destructive-consume path and its `"(no more mock responses)"` fallback are preserved verbatim. No removal / rename / migration / persisted-field / serde-wire / trait-signature / tool-arg-schema change. All 4 forks locked **planner-rec** (F1/F4 design/scope; F2/F3 TECHNICAL). The i-phi `[provider.mock] repeat` config field + `mock_responses_as_responses` `[ToolCalls, Text]` reorder + terminal-text emission is the SEPARATE downstream i-phi consumer chunk, OUT of scope here.

## Forks

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F1 | repeat MECHANISM (design; model/test-facing) | **F1.a — cycle the flat `Vec<MockResponse>` verbatim** (planner-rec) | The committed P0 REPRODUCED that cycling the flat `[ToolCalls, Text]` length-2 queue sustains `[1,1,1]` tool calls across 3 send-inputs — flat-cycle is PROVEN sufficient for #121 with the smallest possible kernel surface (~3-line rotate + one private field). A per-turn-unit abstraction (F1.b) would encode Option-B semantics into the kernel before any consumer needs them, violating `[[feedback_phi_core_kernel_minimal]]`. |
| F2 | additive API SHAPE (**TECHNICAL** — no end-user delta) | **F2.a — `with_repeat(responses, repeat)` + `repeating(responses)`** (planner-rec) | `with_repeat` threads the downstream consumer's `arm.repeat` bool through in one line (vs a call-site branch or a mis-orderable builder); `repeating` mirrors the existing `text`/`texts` convenience precedent and reads cleanest at test call-sites. Both keep `new` one-shot → 133 sites byte-identical. |
| F3 | cursor INTERNALS (**TECHNICAL** — implementation detail; conditional on F1.a) | **F3.a — rotate-on-pop under the existing `Mutex`** (planner-rec) | Smallest correct diff (~3 lines inside the existing lock scope): `remove(0)` the front then `push(front.clone())` when `repeat == true`. Keeps the one-shot `remove(0)` path byte-identical and adds no second interior-mutable state field (which F3.b's index cursor would). Thread-safe by construction under the single `std::sync::Mutex`. |
| F4 | Option B (`[[provider.mock.turns]]` per-turn script) now vs defer (scope) | **F4.a — DEFER Option B; ship only the Option-A cycle flag** (planner-rec) | #121 explicitly recommends "Option A now, Option B later"; cycle-flat-queue closes #121's full acceptance (P0). Building Option B now would grow the kernel surface beyond any consumer's need. Option B slots on top of the cycle primitive later with no rework of this surface. |

## Context

Today `MockProvider` (`src/provider/mock.rs`) consumes its response queue **destructively**: `stream` pops the front via `responses.remove(0)` per LLM call, and when the queue empties it returns a `"(no more mock responses)"` text fallback with no tool calls. The committed P0 (`docs/specs/plan/build/kc-06-mock-provider-repeat-mode-256ff295/p0-investigation.md`, 10 DEFINITIVE findings) REPRODUCED the exact #121 "dead session": one interactive `send-input` consumes **exactly 2** queue items `[ToolCalls, terminal]` because the turn-termination gate at `src/agent_loop/run.rs:531` — `if !has_tool_calls && pending.is_empty() { break; }` — does NOT break on a tool round, forcing a second `stream()` call for the terminal round. So a session whose queue holds at most one `ToolCalls` re-parks nothing on its second turn.

KC-06 is the **kernel half** of i-phi #121. `MockProvider` is phi-core's OWN test double (exercised by 133 in-crate sites and by every consumer's tests), so a general repeat/cycle capability is a genuine kernel test feature carrying ZERO consumer-specific logic — the `[[feedback_phi_core_kernel_minimal]]` carve-out. The fix-locus is phi-core, purely additive: a private `repeat` field + a rotate-on-pop cycle in `stream` + two additive constructors. The *config/choice* surface — the i-phi `[provider.mock] repeat` TOML field, the `mock_responses_as_responses` `[ToolCalls, Text]` queue ordering, and the terminal-text emission — belongs to the downstream i-phi consumer chunk and is explicitly OUT of scope here.

## Sub-decisions

### §D6.1 — cycle-flat-queue mechanism (resolves F1)

Net-new opt-in cycle mode — **no prior repeat behaviour to preserve**; the one-shot destructive-consume path is unchanged on the default. `MockProvider` gains a private `repeat: bool` field; when `repeat == true` and the response `Vec` is non-empty, `stream` pops the front (`remove(0)`) as today and then re-queues a `clone()` of it to the back, so the flat `Vec<MockResponse>` cycles VERBATIM with no reorder and no per-turn-unit machinery. The P0 REPRODUCED that cycling the length-2 `[ToolCalls, Text]` queue sustains `[1,1,1]` tool calls across 3 send-inputs (each send consumes exactly 2 items, so the cursor returns to index 0 every send). The kernel is order-agnostic: it cycles whatever `Vec<MockResponse>` the consumer supplies — the correct one-tool-call-per-turn cadence (an exact `[ToolCalls, Text]` unit) is the CONSUMER's responsibility (see §D6.4), NOT a kernel-owned turn abstraction (which F1.b would have added).

### §D6.2 — `with_repeat` + `repeating` additive constructors (resolves F2; TECHNICAL)

**Pre-existing-behaviour preserved:** `MockProvider::new`/`text`/`texts` shipped before KC-06; their signatures and observable behaviour are unchanged. `new`'s body gains one line (`repeat: false`); `text`/`texts` delegate to `new` and are literally untouched. All 133 `MockProvider::{new,text,texts}` call-sites stay byte-identical. The new `pub fn with_repeat(responses: Vec<MockResponse>, repeat: bool) -> Self` mirrors `new` and sets the private field; `pub fn repeating(responses: Vec<MockResponse>) -> Self` = `with_repeat(responses, true)` mirrors the `text`/`texts` convenience precedent. Both are purely additive (two new public methods, no removal). The downstream i-phi mapping re-points its single `MockProvider::new(...)` call-site at `with_repeat(self.mock_responses_as_responses(), arm.repeat)` — a one-line change.

### §D6.3 — rotate-on-pop cursor internal (resolves F3; TECHNICAL)

**Pre-existing-behaviour preserved:** the one-shot `remove(0)` consume path and the empty→`"(no more mock responses)"` fallback are byte-identical for `repeat == false`. Inside the existing `stream` lock scope the non-empty branch becomes `let front = responses.remove(0); if self.repeat { responses.push(front.clone()); } front` — a ~5-line change under the single `std::sync::Mutex` (thread-safe by construction; `MockResponse` derives `Clone`). No second interior-mutable state field is introduced (which F3.b's index cursor would have, and which would have changed the one-shot path from `remove(0)` to an indexed read, perturbing the proven byte-identical path for no benefit). A full cycle of a length-L queue returns it to its original order, so the private rotation is invisible and self-correcting.

### §D6.4 — terminal-text-required CONSUMER contract (net-new contract; **read before enabling repeat**)

Net-new contract — no prior behaviour to preserve. **The kernel cycles the supplied queue VERBATIM and does NOT auto-synthesize a terminal text.** In one-shot mode the `"(no more mock responses)"` fallback silently serves as the terminal round that breaks the loop (`run.rs:531`) — which is why a `[ToolCalls]`-only queue works for the FIRST turn. Under repeat the queue cycles instead of hitting the fallback, so that implicit terminal disappears. Consequences the CONSUMER MUST honor:

- The repeating unit MUST include a terminal (non-tool) `Text`; the correct one-tool-call-per-turn unit is exactly `[ToolCalls, Text]` (tool call FIRST, terminal text SECOND), because one interactive turn consumes exactly two queue items.
- A **leading** text (`[Text, ToolCalls]`) makes the first turn a dud (the preamble ends the turn before the tool is reached), then self-corrects to `[0,1,1,1]`; a non-length-2 queue (`[Text, ToolCalls, Text]`) alternates `[0,1,0,1]`. Cadence is a function of queue COMPOSITION, which the consumer owns.
- A `[ToolCalls]`-ONLY repeating queue is an **UNBOUNDED tool loop** — the inner loop never satisfies `!has_tool_calls`, so it is halted only by `max_turns`. The P0 REPRODUCED 5 tool rounds inside ONE send-input bounded by `max_turns=5`; the KC-06 Tier D test asserts this bound at `max_turns=4`.

The kernel cannot synthesize the terminal text safely: doing so would bake an i-phi cadence policy ("one tool call then end the turn") into the general kernel — a consumer-concept leak (P0 §5 A3). **The downstream i-phi consumer chunk MUST supply the `[ToolCalls, Text]` ordering + terminal text; document this so that chunk honors it.**

### §D6.5 — defer Option B / Option C (resolves F4)

Net-new scope decision. KC-06 ships ONLY the Option-A cycle flag; no `[[provider.mock.turns]]` per-turn-unit type or queue restructure is added, and #121 Option C (input-derived generators) is likewise out of scope. #121 frames Option A as the high-value minimal fix and Option B as "later." A future kernel chunk may add Option B on top of the cycle primitive with no rework of KC-06's surface.

### §D6.6 — semver 0.12.0 → 0.13.0 (additive → minor)

Net-new versioning decision. Adding a private field + two additive public constructors to `MockProvider` (no public fields today; not constructible externally by struct-literal) is a non-breaking, backward-compatible surface addition → MINOR bump **0.12.0 → 0.13.0**. This 0.13.0 also covers KC-05's additive progressive-tool-disclosure surface (KC-05 landed at `133ad33` without a separate bump). `cargo publish` to crates.io is a POST-SEAL orchestrator/user step (token not in this env) — NOT performed here; the downstream i-phi chunk bumps its `phi-core` dep to 0.13 after publish.

### §D6.7 — kernel-minimality boundary (boundary decision)

Net-new boundary decision. phi-core owns the general test-double cycle primitive — the private `repeat` field + rotate-on-pop cycle + `with_repeat`/`repeating` constructors — with ZERO consumer-specific leakage (no config struct, no `[provider.mock]` TOML field, no `mock_responses_as_responses` ordering policy, no `arm.repeat`, no i-phi naming). The forbidden-leakage grep on `src/provider/mock.rs` returns 0. The config/choice + queue-ordering + terminal-text emission is the i-phi consumer chunk per `[[feedback_phi_core_kernel_minimal]]`, NOT here.

## Cross-references

- **(a) source rustdoc + spec**: `src/provider/mock.rs:2-34` (module ARCHITECTURE / Usage-pattern block — gains the opt-in repeat-mode usage + the §D6.4 CONSUMER cadence contract) + the `with_repeat`/`repeating` ctor doc-comments; `docs/specs/developer/provider.md:192` MockProvider row + the "MockProvider repeat/cycle mode" Conceptual Note (verified-header `:1` bumped).
- **(b) closed issue**: i-phi **#121** (mock provider multi-turn/repeat) — the KERNEL half lands here; #121 stays **OPEN** for the downstream i-phi consumer chunk (config field + `[ToolCalls, Text]` reorder + terminal-text emission + the end-to-end live daemon close-gate). The kernel close-gate (§Verification) proves the mechanism; #121 is not fully closed until the i-phi chunk lands.
- **(c) prior ADRs as precedent**: `docs/decisions/0004-selectable-skill-prompt-layout.md` (kernel-minimality boundary precedent — phi-core owns the primitive, i-phi owns the config/choice) + `0003-revert-tail-shrink-contract.md` (kernel-lane additive / investigation-first precedent).
- **(d) forward-scope / committed P0**: `docs/specs/plan/build/kc-06-mock-provider-repeat-mode-256ff295/p0-investigation.md` + the cycle plan `docs/specs/plan/build/kc-06-mock-provider-repeat-mode-256ff295/plan.md`.

## Consequences

### For the downstream i-phi #121 consumer chunk

The consumer inherits a ready entry point: `MockProvider::with_repeat(responses, repeat)` (or `MockProvider::repeating(responses)` for a direct repeat). i-phi #121's consumer chunk reads a new `[provider.mock] repeat` bool, re-points `build_mock_provider` (`i-phi/src/daemon/config/mod.rs:1301`) at `MockProvider::with_repeat(self.mock_responses_as_responses(), arm.repeat)`, reorders `mock_responses_as_responses` to emit the `[ToolCalls, Text]` unit, and emits the terminal text per §D6.4. That chunk OWNS the end-to-end live validation as load-bearing (`[[feedback_render_transcript_close_gate]]` Rule 6): boot i-phi with `[provider.mock] repeat=true`, send N messages to one session, confirm N interactive Asks park, RENDER + READ the transcript. No phi-core change is needed to land it — this ADR is the kernel prerequisite, now satisfied (after 0.13.0 is published).

### For phi-core + other consumers

The opt-in repeat/cycle mode is available for free on the next tracked phi-core commit — every consumer's tests can now cycle a `MockProvider` queue without a fresh provider per turn. All existing behaviour is unaffected: `repeat` defaults OFF, so `new`/`text`/`texts` and all 133 in-crate sites are byte-behaviour-identical, and no existing test flips (ZERO RED).

## Revisit triggers

1. A consumer needs distinct scripted per-turn tool-calls (not a repeating unit) → **re-open §D6.5** (build Option B `[[provider.mock.turns]]` on top of the cycle primitive).
2. A consumer wants kernel-GUARANTEED cadence (not consumer-supplied `[ToolCalls, Text]` ordering) → re-open §D6.1 / §D6.4 (weigh the F1.b per-turn-unit abstraction against the kernel-minimality cost).
3. A future need to auto-synthesize a terminal text when repeat is on → re-open §D6.4 (weigh against the consumer-cadence-policy kernel leak the P0 ruled out in §5 A3).
4. The rotate-on-pop `Clone`-per-turn proves costly for very large mock queues → re-open §D6.3 (switch to a non-destructive index cursor — a private refactor with no public-surface commitment).
5. Option C (#121) input-derived mock generators are requested → new kernel chunk (out of scope here).

## Verification

```bash
# Full suite (host cargo; single crate; -j 4 cap; NO --workspace). Band [617, 619]; landed 617.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# The kernel close-gate (multi-turn) + repeat regression suite: Tier B/C/D/E.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --test mock_repeat_test -- --nocapture

# Back-compat one-shot + API-equivalence inline tests (Tier A/F).
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib mock

# Clippy (warnings = errors) + fmt.
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# Kernel-minimality: zero consumer-policy leakage in the new surface (expect 0).
grep -rniE "i-phi|iphi|\[provider|toml|daemon|per.agent|mock_responses_as_responses|arm\.repeat" /root/projects/phi/phi-core/src/provider/mock.rs

# Back-compat: no existing MockProvider::{new,text,texts} call-site removed/edited (expect zero `-` lines).
git -C /root/projects/phi/phi-core diff HEAD -- src tests examples | grep -E '^[-+].*MockProvider::(new|text|texts)'

# Semver bump landed.
grep -n '^version = "0.13.0"' /root/projects/phi/phi-core/Cargo.toml
```
