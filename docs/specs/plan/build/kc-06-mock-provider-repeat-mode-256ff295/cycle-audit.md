<!-- Last verified: 2026-08-26 by Claude Code -->

# KC-06 cycle-audit — MockProvider opt-in repeat/cycle mode (cycle `256ff295`)

**Chunk:** KC-06 (phi-core kernel half of i-phi #121 — mock provider multi-turn/repeat) · **Baseline:** `929d326` (Release phi-core 0.12.0) · **Project:** phi-core (kernel lane) · **Branch:** `dev`
**Verdict:** ✅ **PASS — sealable.** Gate-4 MUST-RUN green (host cargo); multi-turn kernel close-gate PASS (`[1,1,1]` disposition). 2 auditors PASS (A 7/7 · B 7/7). Additive, back-compat-clean, kernel-minimal.

---

## §0 — Critical findings (lead)

| Axis | Result |
|---|---|
| Gate-4 MUST-RUN (host cargo) | ✅ `fmt --check` · `clippy -Dwarnings --all-targets` (exit 0) · **test 617/0/5** (+7 vs 610; band [617,619]) |
| Kernel close-gate (disposition) | ✅ `mock_repeat_test` 4/4 — `close_gate_repeat_sustains_one_tool_call_per_send` asserts `[1,1,1]` across 1 `agent_loop` + 2 `agent_loop_continue` on ONE session (not "no panic") |
| Back-compat | ✅ ZERO RED — `repeat` defaults OFF; 133 `MockProvider::{new,text,texts}` sites byte-identical (`^-` diff on those = 0); one-shot `remove(0)` path byte-identical |
| Kernel-minimality | ✅ forbidden-leakage grep on `mock.rs` = 0 (no i-phi/TOML/consumer concept); only a general test-double primitive added |
| Semver | ✅ `Cargo.toml` 0.12.0 → **0.13.0** (additive MINOR; covers KC-05 + KC-06 API at the next publish) |

---

## §1 — Gate-4 MUST-RUN (orchestrator-authoritative; host cargo, single crate)

| Command | Result |
|---|---|
| `cargo fmt --manifest-path … -- --check` | exit 0 |
| `RUSTFLAGS="-Dwarnings" cargo clippy --manifest-path … --all-targets` | exit 0 |
| `cargo test --manifest-path …` | **617 passed / 0 failed / 5 ignored** (+7 vs 610; band [617,619]) |
| Kernel close-gate | `cargo test --test mock_repeat_test` → 4/4 (incl. the `[1,1,1]` Tier-C disposition) |
| (No `check-*.sh` guards exist for phi-core — the MUST-RUN IS the gate.) | |

---

## §2 — Kernel close-gate (disposition-asserting)

Per `[[feedback_render_transcript_close_gate]]` Rule 8, the fix MANIFESTS in per-turn provider output (a library, no live wire). `tests/mock_repeat_test.rs`:
- **Tier C (THE close-gate):** a repeating `[ToolCalls, Text]` provider driven through 3 send-inputs (1 `agent_loop` + 2 `agent_loop_continue` on one shared session, single `Arc<MockProvider>` so the queue-state carries across) → per-send tool-call counts `[1, 1, 1]` — the exact #121 `send → Ask → approve → send → Ask …` cadence.
- **Tier D (contract):** a `[ToolCalls]`-only `repeating` provider is unbounded → `max_turns`-bounded to N tool rounds in one send — proves the §D6.4 terminal-text-required CONSUMER contract (the kernel does NOT auto-terminate).
- **Tier E (cadence):** `[ToolCalls,Text]→[1,1,1,1]`, `[Text,ToolCalls]→[0,1,1,1]`, `[Text,ToolCalls,Text]→[0,1,0,1]` — the kernel cycles VERBATIM; ordering is the consumer's responsibility.
- **Tier B (back-compat):** one-shot `[ToolCalls,Text]` drains in one turn → send #2 hits `"(no more mock responses)"`, 0 tool calls.

The END-TO-END daemon validation (an i-phi session sustaining live permission-Asks over N `send-input`s) is explicitly the DOWNSTREAM i-phi consumer chunk's close-gate — owned as load-bearing per `[[feedback_render_transcript_close_gate]]` Rule 6, NOT this kernel cycle.

---

## §3 — Auditor verdicts (gate-3)

| Auditor | Scope | Verdict | Disposition |
|---|---|---|---|
| **A** | code / kernel-minimality / back-compat | **PASS 7/7** | rotate-on-pop (`mock.rs:209-216`) correct + thread-safe in the `Mutex`; `repeat==false` byte-identical to `remove(0)`; empty→fallback untouched; 133→137 additive (0 removed/edited sites); semver 0.13.0; leakage grep 0. Claim-7 PASS-with-note (doc-density overrun = §D6.4 rustdoc, permitted). |
| **B** | consumer-contract / close-gate / docs+ADR | **PASS 7/7** | the `[1,1,1]` close-gate + the §D6.4 contract (Tier D/E) + one-shot back-compat (Tier B); ADR-0006 7 sections `Accepted` mirroring 0004/0005; `mock.rs` rustdoc 3 content axes; #121 correctly OPEN. |

Gate-2 (orchestrator, first-hand): the rotate-on-pop block (`mock.rs:203-219`, returns `front`, one-shot byte-identical), the ctors (`:121`/`:133`), the version (0.13.0), and the leakage grep (0) read directly.

---

## §4 — the mechanism / kernel-minimality

**Kernel-minimal, ZERO consumer leakage.** `MockProvider` gains a private `repeat: bool` + rotate-on-pop in `stream` (re-queue the popped `MockResponse` clone to the back when `repeat`) + additive `with_repeat`/`repeating` ctors. The kernel cycles the flat `Vec<MockResponse>` VERBATIM (order-agnostic, no per-turn machinery); the `[ToolCalls, Text]` ordering + the required terminal text are the DOWNSTREAM i-phi consumer's queue-composition responsibility (documented as the §D6.4 CONSUMER contract). `phi-core-leverage-check` is N/A (the kernel does not consume itself); the kernel-minimality surface check confirms only a general test-double primitive was added. Only `src/provider/mock.rs` (+ the new test file, ADR-0006, `provider.md`, `Cargo.toml`) changed.

---

## §5 — Paperwork ledger

| Artifact | State |
|---|---|
| `p0-investigation.md` | present (10 DEFINITIVE, reproduced; gate-0.5 verified) |
| `plan.md` | archived (cycle `256ff295`); §1 Locked fork details populated (F1.a/F2.a/F3.a/F4.a) |
| `audit-{A,B}-iter1.md` | present (both PASS) |
| ADR-0006 (§D6.1–§D6.7) | Accepted (§D6.4 = consumer contract) |
| `mock.rs` rustdoc / `docs/specs/developer/provider.md` | updated; verified-headers bumped |
| `Cargo.toml` | 0.12.0 → 0.13.0 |
| `cycle-audit.md` | this file |
| cycle-index row | present (`256ff295`; Iterations/Status flip to sealed at seal-time) |
| #121 | **kernel half done; #121 stays OPEN** for the downstream i-phi consumer chunk (config + queue reorder + live daemon close-gate) |

**Post-seal steps (orchestrator/user):** `cargo publish` phi-core 0.13.0 to crates.io (crates.io token required — not in this env) → the downstream i-phi chunk bumps `phi-core = "0.13"` + adds `[provider.mock] repeat`.

---

## §6 — Deviations

| ID | Class | Description | Disposition |
|---|---|---|---|
| D-1 | LOC-cap overrun (doc-density) | `mock.rs` +63 production+rustdoc vs ~40 cap — logic ~12 (field + `repeat:false` + ctor bodies + rotate) within budget; the overrun is the §D6.4 consumer-contract rustdoc (plan §3.B semantic-completeness escape authorized it) + ctor doc-comments. | Accepted (Route A) — Audit A claim-7 PASS-with-note. **4th occurrence of the doc-density-overrun class** (CH-05 D-1 / MA-04 D-1 / MA-08 D-2 / KC-06) → candidate for the joint-retro (a doc-density cap-calibration standard). |

---

## §7 — Metrics

- Tests: 610 → **617** (+7; band [617,619]). 0 failed. Tier A(inline back-compat/equiv) 2 · F(convenience-equiv) 1 · B(one-shot) 1 · C(close-gate `[1,1,1]`) 1 · D(unbounded contract) 1 · E(cadence) 1.
- Files: `src/provider/mock.rs` (+168/−4), `Cargo.toml` (+1/−1 version). NEW: `tests/mock_repeat_test.rs` (320), ADR-0006 (109). `docs/specs/developer/provider.md` note.
- Cascade: 0 external `MockProvider::{new,text,texts}` edits (133 byte-identical; +4 additive occurrences in new test/doc). Leakage grep 0.
- Plan iterations: 1 (all forks planner-rec → Direct-approval-clean, P-orch-8 skip). Audit iterations: A1 · B1 — **0 re-spawns**.
- Kernel close-gate: `mock_repeat_test` 4/4 (`[1,1,1]` disposition).
