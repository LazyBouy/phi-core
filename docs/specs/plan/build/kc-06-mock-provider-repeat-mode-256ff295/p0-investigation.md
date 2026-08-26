# P0 investigation — KC-06 (phi-core kernel lane)

Chunk: **KC-06** — phi-core kernel half of i-phi **#121** (MockProvider multi-turn / repeat support).
Grounded against phi-core `dev` @ HEAD `929d326` ("Release phi-core 0.12.0").
Scope of THIS chunk: the phi-core `MockProvider` repeat/cycle mechanism + its Rust API ONLY. The
i-phi `[provider.mock] repeat` config field + `mock_responses_as_responses` reordering is a SEPARATE
downstream i-phi chunk (facts about the kernel API it will call are established in §4, but the i-phi
work is out of scope here).

## §0 — Verdict summary
- Findings: **10 DEFINITIVE, 0 unresolved.** Every behavioral claim REPRODUCED with a passing phi-core
  test (`tests/_p0_kc06_repro.rs`, 4 tests, all green) or a `file:line` citation @ `929d326`.
- Fix-locus: **phi-core kernel-only, purely additive.** `MockProvider` gains an opt-in repeat/cycle
  mode + one additive constructor; the existing one-shot path stays byte-behaviour-identical. ZERO
  i-phi concepts leak in (the MockProvider is phi-core's OWN test double, used by 142 in-crate sites).
  The i-phi config→queue mapping is a strictly-downstream consumer chunk.
- Forks surfaced for the planner: **4** (cycle mechanism; API shape; terminal-text responsibility;
  Option-B per-turn-script now-vs-defer). None locked.
- Non-viable approaches ruled out: **3.**
- Unresolved / needs-live-repro: **none.** The kernel close-gate is a pure phi-core multi-turn unit
  test (§8); the end-to-end daemon render is the DOWNSTREAM i-phi chunk's close-gate, not this one.
- Semver: **additive → MINOR bump 0.12.0 → 0.13.0.** `cargo publish` is orchestrator/user-owned
  post-seal (crates.io token not in this env); NOT performed here.

## §1 — Questions that gate planning
1. **Exhaustion + consumption mechanics** — reproduce that a MockProvider whose queue empties returns
   `"(no more mock responses)"` (no tool call) on the next call; and establish EXACTLY how many queue
   items one interactive (tool-emitting) turn consumes.
2. **The CRUX — the repeat/cycle mechanism.** Does repeat cycle the WHOLE flat queue or a per-turn
   UNIT? How does the loop's per-turn consumption interact with cycling so each `send-input` re-parks
   the same Ask and completes? What exact `MockResponse` sequence does a repeating interactive session
   need, and how must the cursor/cycle advance?
3. **API shape for the i-phi mapping.** What additive Rust surface must `MockProvider` expose so the
   downstream i-phi chunk can construct a repeating provider (constructor / builder / setter), while
   keeping `MockProvider::new(responses)` one-shot back-compat?
4. **Back-compat + default-off.** Confirm every existing in-crate `MockProvider` construction stays
   byte-behaviour-identical (one-shot is the default; repeat is opt-in).
5. **Fix-locus + kernel-minimality.** Confirm this is a genuine general kernel test feature (not i-phi
   leakage); establish the phi-core test surface to add.
6. **Semver + publish impact** (note for orchestrator) — additive → minor; publish is out-of-env.

## §2 — Current-surface map (file:line @ 929d326)

### phi-core `MockProvider`
- `src/provider/mock.rs:52-58` — `pub enum MockResponse { Text(String), ToolCalls(Vec<MockToolCall>) }`.
- `src/provider/mock.rs:60-65` — `pub struct MockToolCall { pub name: String, pub arguments: serde_json::Value }`.
- `src/provider/mock.rs:68-71` — `pub struct MockProvider { responses: std::sync::Mutex<Vec<MockResponse>> }`.
  The single field is **private** → external code cannot construct by struct-literal; all construction
  flows through the three public constructors below.
- `src/provider/mock.rs:75-79` — `pub fn new(responses: Vec<MockResponse>) -> Self` (the canonical ctor).
- `src/provider/mock.rs:82-84` — `pub fn text(text: impl Into<String>) -> Self` (convenience → `new`).
- `src/provider/mock.rs:87-104` — `pub fn texts(texts: Vec<impl Into<String>>) -> Self` (convenience → `new`).
- `src/provider/mock.rs:113-273` — `impl StreamProvider for MockProvider::stream`. The consume site:
  - `:147-156` — lock `responses`; **`:149-151`** empty → `MockResponse::Text("(no more mock responses)")`
    fallback (StopReason::Stop); **`:153`** else `responses.remove(0)` (DESTRUCTIVE front-pop).
  - `:180-246` — `match response`: `Text` → `Message::Assistant { stop_reason: Stop }` (`:196-204`);
    `ToolCalls` → emits `ToolCallStart`/`ToolCallEnd` per call, `Message::Assistant { stop_reason: ToolUse }`
    (`:236-244`).

### The agent-loop consumption sites (how many `MockResponse`s a turn eats)
- `src/agent_loop/run.rs:67` outer loop (follow-ups) / **`:75` inner loop** (tool rounds + steering).
- `src/agent_loop/run.rs:342-344` — `stream_assistant_response(...)` is called **once per inner-loop
  iteration**. Each call → `provider.stream()` → consumes **exactly one** `MockResponse`.
- `src/agent_loop/streaming.rs:400-424` — the retry loop that invokes `provider.stream(...)` (`:422-424`).
  One successful `stream()` = one queue item.
- `src/agent_loop/run.rs:398-413` — extract tool calls; `has_tool_calls = !tool_calls.is_empty()`.
- `src/agent_loop/run.rs:416-467` — if `has_tool_calls`: execute tools, append tool results.
- **`src/agent_loop/run.rs:531-533`** — the turn-termination gate:
  `if !has_tool_calls && pending.is_empty() { break; }`. A tool-emitting round does NOT break → the
  inner loop iterates again → a SECOND `stream()` call → a SECOND queue item for the terminal round.
- So **one interactive (single-tool-call) `send-input` consumes 2 queue items: `[ToolCalls, <terminal>]`**
  — the `ToolCalls` round + a terminal round that returns a non-tool `Text` (which breaks the loop).

### The downstream i-phi consumer (for §4 API-shape grounding — NOT modified by KC-06)
- `i-phi/src/daemon/config/mod.rs:1278-1297` — `mock_responses_as_responses()` builds the queue as
  **`[Text(r0), Text(r1), …, ToolCalls(all_calls)]`** (all `responses` first, then ONE bundled
  `ToolCalls` LAST).
- `i-phi/src/daemon/config/mod.rs:1300-1304` — `build_mock_provider() -> Arc<MockProvider>` wraps
  `MockProvider::new(self.mock_responses_as_responses())`. **This is the exact call-site the downstream
  i-phi chunk will re-point at the new repeat constructor.**

## §3 — Findings (each DEFINITIVE; REPRODUCED or file:line)

**F1 — Exhaustion reproduced (Q1).** DEFINITIVE. Repro `kc06_exhaustion_and_consumption_count`:
queue `[ToolCalls(echo), Text("done turn 1")]`; `send-input #1` via `agent_loop` →
`4 messages [user, assistant(toolcall), toolResult, assistant(text)]`, 1 tool call. `send-input #2`
via `agent_loop_continue` → `1 message [assistant]`, **0 tool calls**, content
`Text { text: "(no more mock responses)" }`. This is precisely the #121 "dead session" — the second
message to the same session parks no Ask. Root cause = destructive `remove(0)` (`mock.rs:153`) +
empty→fallback (`mock.rs:149-151`).

**F2 — One interactive turn consumes exactly 2 queue items (Q1).** DEFINITIVE. Same repro: a 2-item
queue `[ToolCalls, Text]` fully drained by a single `agent_loop` call (turn 2 hits the fallback). Cause
is `run.rs:531` — the tool round doesn't break, forcing a second `stream()` for the terminal round. The
in-tree test `tests/agent_loop_test.rs:136-245` (`test_tool_call_and_response`) independently confirms
the same `[ToolCalls, Text] → 4 messages` shape. Corollary from `kc06_cycling_sustains_repeating_turns`:
3 send-inputs → **6 `stream()` calls (exactly 2 per send-input)**.

**F3 — Cycling the flat queue WORKS across N turns, IFF the queue is exactly the `[ToolCalls, Text]`
unit (Q2 — the CRUX).** DEFINITIVE. Repro `kc06_cycling_sustains_repeating_turns` drives a prototype
cursor-cycling provider over `[ToolCalls, Text]` through 3 send-inputs → each re-emits the tool call
`[1, 1, 1]`. Mechanics: each send-input consumes exactly 2 items; queue length 2 evenly divides the
consumption, so the cursor returns to index 0 after every send-input → `ToolCalls` is always the first
item of the next turn. **Cycle-whole-queue is sufficient for #121 — NO per-turn-unit machinery is
required in the kernel** — provided the consumer supplies the queue AS the per-turn unit.

**F4 — Cadence is a function of queue COMPOSITION; a leading text or a non-2-length queue drifts it
(Q2).** DEFINITIVE. Repro `kc06_cadence_depends_on_queue_shape` (per-turn tool-call counts over 4
send-inputs):
- `[ToolCalls, Text]` → `[1, 1, 1, 1]` (perfect — every send parks).
- `[Text, ToolCalls]` (**i-phi's CURRENT `mock_responses_as_responses` ordering**) → `[0, 1, 1, 1]`:
  send-input #1 is a **DUD** (the leading preamble text ends the turn before the tool is reached), then
  self-corrects.
- `[Text, ToolCalls, Text]` length-3 → `[0, 1, 0, 1]`: cadence **alternates** dud/park (queue length is
  not a clean multiple of the 2-items-per-send consumption).
This is the concrete, load-bearing form of the issue's "interleaving nuance." It proves the kernel
`repeat` flag cycles the queue **verbatim** (no reorder), so **correct cadence is the CONSUMER's
responsibility**: the queue must be exactly `[ToolCalls, Text]` (tool call FIRST, terminal text SECOND).

**F5 — Under repeat mode the empty→fallback safety-net is BYPASSED, so the terminal text must be
EXPLICIT; a `[ToolCalls]`-only cycle is an unbounded tool loop (Q2).** DEFINITIVE. In one-shot mode the
`"(no more mock responses)"` fallback (`mock.rs:149-151`) silently serves as the terminal round that
breaks the loop (`run.rs:531`) — which is why i-phi's current `[ToolCalls]`-only queue works for the
FIRST turn. Repeat mode cycles instead of hitting the fallback, so that implicit terminal disappears.
Repro `kc06_cycling_toolcalls_only_needs_limit`: cycling `[ToolCalls]` alone never satisfies
`!has_tool_calls` → the inner loop ran **5 tool rounds inside ONE send-input**, halted only by
`max_turns=5`. Therefore, under repeat mode the repeating unit MUST include a terminal (non-tool)
`Text`. The kernel cannot synthesize this safely (see §5 A3).

**F6 — API shape the i-phi mapping needs (Q3).** DEFINITIVE. The single downstream call-site is
`i-phi/.../daemon/config/mod.rs:1301` `MockProvider::new(self.mock_responses_as_responses())`. The
downstream chunk needs an additive constructor that carries the repeat flag, e.g.
`MockProvider::with_repeat(responses: Vec<MockResponse>, repeat: bool)` (or a `.repeat(bool)`
builder-setter / a `repeating(responses)` convenience — §6 F-B). The i-phi mapping would then pass
`arm.repeat` (its new `[provider.mock] repeat` bool). The kernel API is order-agnostic: it cycles
whatever `Vec<MockResponse>` the consumer supplies; the i-phi chunk owns emitting `[ToolCalls, Text]`
ordering + the terminal text (F4/F5).

**F7 — Back-compat: every existing construction stays byte-identical (Q4).** DEFINITIVE. Grep across
`phi-core/src` + `tests` + `examples`: **142** `MockProvider::{new,text,texts}` call-sites; **zero**
struct-literal `MockProvider { … }` constructions outside the def/impl (the field is private; the two
`MockProvider {` hits are the struct def `mock.rs:68` + `impl` `mock.rs:73`; the `CapturingMockProvider`
structs in `tests/turn_request_capture_test.rs` + `tests/release_0_10_test.rs` are SEPARATE test
doubles, unaffected). An additive private field defaulted to one-shot + `new/text/texts` unchanged →
all 142 sites unchanged. My full repro suite compiled + passed against the UNMODIFIED provider,
confirming no signature churn is forced.

**F8 — Fix-locus is a genuine general kernel feature (Q5).** DEFINITIVE. `MockProvider` is phi-core's
own test double (`src/provider/mock.rs`), exercised by phi-core's OWN tests (142 sites) and by every
consumer's tests. A repeat/cycle mode carries ZERO i-phi-specific logic — it is a general test-provider
capability. This satisfies the `[[feedback_phi_core_kernel_minimal]]` carve-out: the change belongs in
the kernel BECAUSE all consumers benefit and no consumer concept leaks in. (Contrast: the
`[provider.mock] repeat` TOML field + `mock_responses_as_responses` ordering IS i-phi-specific → stays
in the i-phi consumer chunk.)

**F9 — phi-core has its own multi-turn test surface to add (Q5).** DEFINITIVE. `tests/agent_loop_test.rs`
already carries the `[ToolCalls, Text]` single-shot pattern (`:136-245`) and `agent_loop_continue`
setup (`:284-323`). The kernel regression tests for repeat mode slot directly alongside these (a
repeating provider driven through ≥ 2 turns asserting each turn re-emits the tool call, plus a
one-shot-unchanged assertion). My throwaway repro is the ready template (§7).

**F10 — Additive → minor semver (Q6).** DEFINITIVE. Adding a private field + a new public constructor/
setter to `MockProvider` (no public fields today; not constructible externally by literal) is a
non-breaking, backward-compatible surface addition → **0.12.0 → 0.13.0 (minor)**. `cargo publish` is
orchestrator/user-owned post-seal (crates.io token absent in this env); NOT performed here.

## §4 — Fix-locus determination

| Change | Side | Carve-out-test rationale | Blast radius |
|---|---|---|---|
| `MockProvider` opt-in repeat/cycle mode (private field + cycle logic in `stream`) | **phi-core KERNEL** | General test-double capability; zero consumer-specific logic; used by phi-core's own tests. Genuine kernel feature per `[[feedback_phi_core_kernel_minimal]]`. | `src/provider/mock.rs` only. `new/text/texts` unchanged → 142 in-crate sites byte-identical (F7). Other `StreamProvider` impls (`anthropic.rs`, `openai_compat.rs`, …) untouched; the trait is unchanged. `sub_agent.rs`/`parallel.rs`/`evaluation.rs` consume the loop, not the provider internals → unaffected. |
| Additive constructor `with_repeat` (or builder `.repeat`) | **phi-core KERNEL** | The API the downstream i-phi mapping calls; additive, keeps `new` one-shot. | `src/provider/mock.rs`; re-exported via `phi_core::provider::MockProvider`. Minor semver. |
| phi-core regression tests (repeat multi-turn + one-shot-unchanged) | **phi-core KERNEL** | Kernel owns its own test surface (F9). | `tests/agent_loop_test.rs` (or a new `tests/mock_repeat_test.rs`). |
| `[provider.mock] repeat` TOML field + `mock_responses_as_responses` → `[ToolCalls, Text]` reorder + terminal-text emission | **i-phi CONSUMER (DOWNSTREAM chunk — NOT KC-06)** | i-phi-specific config schema + queue-ordering policy; must NOT leak into the kernel. | `i-phi/src/daemon/config/mod.rs:1278-1304` (`mock_responses_as_responses` + `build_mock_provider`) + `MockArm`. Consumes the published kernel API. |

**Kernel API the downstream i-phi chunk will call** (established, not built here):
`MockProvider::with_repeat(responses: Vec<MockResponse>, repeat: bool) -> MockProvider` (or the
builder/setter equivalent per §6 F-B). Order-agnostic cycle of the supplied `Vec<MockResponse>`.

## §5 — Non-viable approaches ruled out

**A1 — "Just cycle the flat queue; the config author supplies whatever responses."** Tempting because
it is the literal minimal `repeat` flag. DISPROVEN as *sufficient on its own*: F4 shows cycle-whole-queue
is correct ONLY for a `[ToolCalls, Text]` length-2 unit; i-phi's CURRENT `[Text, ToolCalls]` ordering
yields `[0,1,1,1]` (first send-input dud) and odd-length queues alternate `[0,1,0,1]`. The flag is
necessary but the CONSUMER-side ordering fix (downstream) is co-required. (This does NOT kill Option A;
it kills the assumption that the kernel flag alone closes #121 regardless of queue shape.)

**A2 — "Non-destructive read via a cursor is a behaviour change we must guard even in one-shot mode."**
Tempting worry that switching `remove(0)` internals perturbs one-shot callers. DISPROVEN as a blocker:
the one-shot observable contract is "return the i-th response for the i-th call, then fallback." A
rotate-on-repeat implementation can keep `remove(0)` verbatim for `repeat=false` and only re-push the
popped item when `repeat=true` (or gate a cursor on `repeat`), leaving one-shot byte-identical (F7 repro
passed against the unmodified provider; the additive path is orthogonal). Not a real risk; noted so the
planner doesn't over-scope a "cursor migration."

**A3 — "Kernel should auto-append a synthetic terminal text when repeat is on, so a `[ToolCalls]`-only
queue just works."** Tempting because F5 shows `[ToolCalls]`-only cycles unboundedly. DISPROVEN as a
kernel responsibility: auto-synthesizing a terminal `Text` bakes an i-phi cadence policy ("one tool call
then end the turn") into the general kernel — a consumer concept leak violating
`[[feedback_phi_core_kernel_minimal]]`. Other consumers may legitimately want a repeating multi-tool
unit. The terminal text is the CONSUMER's queue-composition responsibility (F5); the kernel cycles
verbatim. (The planner may still surface a terminal-text convenience as a FORK — §6 F-C — but it must
not be the default kernel behaviour.)

## §6 — Design forks surfaced (for the planner — NOT locked)

**F-A — Repeat mechanism: cycle-whole-flat-queue vs explicit per-turn-unit.**
- Options: (a) `repeat: bool` cycles the flat `Vec<MockResponse>` verbatim (rotate-on-pop or cursor);
  (b) a per-turn UNIT abstraction (`Vec<TurnUnit>` where a unit is `[ToolCalls?, Text]`).
- Factual trade-off (F3/F4/F5): (a) is the minimal kernel change and is PROVEN sufficient for #121 when
  the consumer supplies `[ToolCalls, Text]`; its correctness is sensitive to queue composition (the
  consumer owns ordering). (b) makes the per-turn unit explicit (robust to composition) but is a larger
  kernel surface that starts to encode Option-B semantics in the kernel. Evidence favours (a) as the
  minimal-correct kernel half; (b) trends toward §F-D.

**F-B — API shape for the additive constructor.**
- Options: (a) `MockProvider::with_repeat(responses, repeat: bool)`; (b) builder-setter
  `MockProvider::new(responses).repeat(true)` (consuming `self`); (c) a `repeating(responses)`
  convenience + keep `new` one-shot.
- Factual trade-off (F6): all three keep `new` one-shot (F7 back-compat). (a) is the most direct match
  for the single i-phi call-site (`mod.rs:1301`) — one line re-point. (b) is the most idiomatic-Rust /
  extensible if more mock flags land later. (c) reads best at test call-sites. No behavioural difference;
  purely ergonomic.

**F-C — Internal implementation: rotate-on-pop vs cursor.**
- Options: (a) keep `Mutex<Vec<MockResponse>>`; on `repeat=true` re-push the popped front to the back
  (queue rotates; ~2-line diff in `stream`); (b) add a `cursor` + `repeat` and read non-destructively.
- Factual trade-off: (a) is the smallest diff and keeps one-shot `remove(0)` untouched; (b) is
  non-destructive and arguably clearer but adds a second piece of state. Both cycle identically (F3).

**F-D — Ship Option B (`[[provider.mock.turns]]` per-turn script) now vs defer.**
- Options: (a) defer — ship only the Option-A `repeat` flag now; (b) also build a per-turn-unit kernel
  API now.
- Factual trade-off (F3/F5): Option A is PROVEN to close the #121 acceptance criteria (N send-inputs →
  N Asks) with the smallest kernel surface, matching the issue's own recommendation. Option B is strictly
  more expressive but larger, and #121 explicitly frames it as "later." The one hard requirement Option A
  imposes on the consumer (an explicit terminal `Text` in the repeating unit, F5) is a downstream-config
  concern, not a reason to pull Option B into the kernel now.

## §7 — Repro assets

| Path | What it proves | Disposition | Re-run |
|---|---|---|---|
| `phi-core/tests/_p0_kc06_repro.rs` (4 tests) | F1 exhaustion + F2 2-items/turn (`kc06_exhaustion_and_consumption_count`); F3 cycling sustains N turns (`kc06_cycling_sustains_repeating_turns`); F5 `[ToolCalls]`-only unbounded loop bounded by `max_turns` (`kc06_cycling_toolcalls_only_needs_limit`); F4 cadence-vs-queue-shape `[1,1,1,1]`/`[0,1,1,1]`/`[0,1,0,1]` (`kc06_cadence_depends_on_queue_shape`) | **THROWAWAY — reverted** (file deleted at investigation close; no production source was instrumented). It is a **promote-to-regression-test candidate**: the exhaustion + cycling + cadence tests should be re-expressed against the REAL `MockProvider::with_repeat` (dropping the local `CyclingMock` prototype) and landed in `tests/agent_loop_test.rs` / a new `tests/mock_repeat_test.rs` by the implementer. | `/root/rust-env/cargo/bin/cargo test --manifest-path /root/projects/phi/phi-core/Cargo.toml --test _p0_kc06_repro -- --nocapture --test-threads=1` (recreate from this report's assertions) |

Note: the repro used a local `CyclingMock` test double (a cursor-cycling `StreamProvider`) to prove the
mechanism WITH the real agent loop, without writing any production code into `mock.rs`.

## §8 — Recommended close-gate (kernel half)

Per `[[feedback_render_transcript_close_gate]]`, the KERNEL close-gate for a MockProvider change is a
phi-core unit/integration test (no daemon), because the fix manifests in the provider's per-turn output,
not on a live wire:

1. A repeating-provider test driving **≥ 3 turns** (one `agent_loop` + ≥ 2 `agent_loop_continue`)
   against `MockProvider::with_repeat([ToolCalls, Text], true)`, asserting **each turn re-emits exactly
   one tool call** (per-turn assistant-tool-call count `[1, 1, 1]`) — the disposition, not merely
   "no panic." (Mirrors `kc06_cycling_sustains_repeating_turns`.)
2. A one-shot-unchanged test: `MockProvider::new([ToolCalls, Text])` (repeat off / default) still
   exhausts after one turn → turn 2 hits `"(no more mock responses)"`, 0 tool calls (F1 assertion),
   proving default-off back-compat.
3. Green MUST-RUN gate (orchestrator-owned): `cargo test`, `cargo fmt -- --check`, and
   `RUSTFLAGS="-Dwarnings" cargo clippy --all-targets` (single crate, `--manifest-path`, NO `--workspace`;
   no `check-*.sh` guards exist for phi-core).

**The END-TO-END daemon validation** (boot i-phi with `[provider.mock] repeat=true`, send N messages to
one session, confirm N interactive Asks park, RENDER + READ the transcript) is the **DOWNSTREAM i-phi
chunk's** close-gate — it depends on the i-phi config field + `mock_responses_as_responses` reorder that
KC-06 does not ship. It is explicitly OUT of scope for the kernel close-gate here and must be OWNED as
load-bearing by that consumer chunk (`[[feedback_render_transcript_close_gate]]` Rule 6).
