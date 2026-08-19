<!-- Last verified: 2026-08-19 by Claude Code -->

# KC-05 close-gate — progressive tool-catalog disclosure

**Cycle** `a0830ed3` · **fix-locus** phi-core kernel · **surface where the fix manifests** the model-facing turn-1 `tools[]` **wire** (`[[feedback_render_transcript_close_gate]]` Rule 8).

## Why the render is phi-core-native (not the i-phi HTC harness)

The `phi-core-close-gate` skill normally borrows the i-phi HTC harness (build i-phi against patched phi-core, boot the daemon, read the wire). **That path cannot exercise KC-05's ON-path**: KC-05 ships progressive **OFF by default** and the i-phi consumer has **no wiring to flip the kernel flag yet** (that is the deferred i-phi per-agent policy follow-on). So an i-phi daemon can only ever render the OFF (full) wire. The ON-path disposition is therefore asserted at the **kernel level**, on the exact rendered wire (`AgentEvent::TurnRequest.payload.tools` — the `Vec<ToolDefinition>` each provider serializes verbatim), driven through a **real agent turn** (`agent_loop` + a real tool set). This is the actual model-facing artifact; it is a render + read, not a mock of one.

## Disposition contract — asserted on the rendered wire (not merely "renders / no-400")

Per `[[feedback_render_transcript_close_gate]]` Rule 4 — each rule of the intended contract is asserted against the model's exact input, on a real rendered wire.

| Rule | Intended disposition | Asserted by (rendered-wire test) | Verdict |
|---|---|---|---|
| **R0 — OFF unchanged** | Progressive OFF ⇒ turn-1 `tools[]` byte-identical to the historical full render | `golden_off_wire_is_byte_identical` (captures `TurnRequest.tools`, asserts `== expected_full_tools`) | PASS |
| **R1 — manual not eager** | The REAL `revert_to_state` 1870-char manual (`REVERT_DETAILED`) is absent from the ON wire | `close_gate_real_revert_manual_absent_and_wire_reduced` (needle `"The conversation is a TREE of nodes"` not in ON wire JSON — would FAIL on pre-KC-05 code where `description()` WAS the manual) | PASS |
| **R2 — short + stub for revert** | On the ON wire `revert_to_state.description` is the ≤256-char one-liner pointing at `tool_help`, `parameters` is the `{"type":"object"}` stub | `close_gate_…` (asserts len ≤ 256 + contains `tool_help("revert_to_state")` + stub params) | PASS |
| **R3 — every entry reduced** | Every catalog entry on the ON wire carries `short_description()` + the stub | `progressive_on_reduces_render` + `close_gate_…` (all entries stub) | PASS |
| **R4 — measurable byte drop** | The ON wire is materially smaller than the full render | `progressive_on_drops_bytes` + `close_gate_…` (`on_json.len() < off_json.len()`) | PASS |
| **R5 — schema served on demand** | `tool_help(<name>)` returns the tool's full `parameters_schema()` incl. an MCP tool's `inputSchema` | Tier-D `catalog_serves_mcp_full_input_schema` + siblings (`tool_help.rs`) | PASS |
| **R6 — provider acceptance** | The `{"type":"object"}` stub serializes to a valid `input_schema` (Anthropic) / `parameters` (OpenAI) — no wire-type change | Tier-G `kc05_reduced_stub_parameters_serialize_to_input_schema` / `…_to_parameters` | PASS |
| **R7 — engagement gate** | OFF-default + below-count inert; engages above `min_tools` OR when a large-schema (MCP/OpenAPI) tool is attached | Tier-C `engages_*` (incl. `engages_on_large_schema_below_count_and_disabled_never`) | PASS |

No-400 / well-formed is necessary but NOT sufficient — every rule above asserts the intended **disposition** (reduced params, short revert, schema-on-demand, OFF-unchanged), not merely that a balanced wire was produced.

## Deferred layer — OWNED (Rule 6)

The **live-model end-to-end** proof — a real provider (`deepseek/deepseek-chat-v3-0324` + a second strongest open-source driver per `[[feedback_htc_cohort_strongest_open_source]]`) driving the reduced catalog, the model actually calling `tool_help` then the tool (loop closes) on the **i-phi product daemon** wire — is **owned by the i-phi per-agent policy follow-on** (the chunk that ships the consumer wiring to flip `progressive_tool_catalog.enabled` per agent). It is load-bearing for that chunk's forward-scope and MUST run on the i-phi PRODUCT-generated wire (`sessions/<sid>/wire/*.turn-N.request.json`), not be re-deferred. KC-05 cannot run it because no consumer can enable the flag yet; "validated later" must not become "validated never."

## Evidence

- Rendered-wire tests: `phi-core/tests/progressive_tool_catalog_test.rs` (R0–R4, R7) — all assert on `AgentEvent::TurnRequest.payload.tools`.
- `phi-core/src/tools/tool_help.rs` tests (R5); `phi-core/src/provider/{anthropic,openai_compat}.rs` `#[cfg(test)]` (R6).
- Gate-4 MUST-RUN: default `cargo test` GREEN; `clippy --all-targets -Dwarnings` GREEN; `fmt --check` GREEN (see `cycle-audit.md` §3).
