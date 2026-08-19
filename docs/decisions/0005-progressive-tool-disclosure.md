<!-- Last verified: 2026-08-19 by Claude Code (KC-05 #109/#110 — phi-core ADR-0005; progressive tool-catalog disclosure: short/detailed trait split + config-gated reduced render + catalog-backed tool_help + registration validation; back-compat-by-default) -->

# phi-core ADR-0005 — Progressive tool-catalog disclosure + tool-registration contract (kernel mechanism, back-compat-by-default)

**Status: Accepted**

> Fifth phi-core ADR. Closes GitHub #110 (tool-registration contract: short/detailed description split — the enabler) + #109 (progressive tool-catalog disclosure: reduced turn-1 render + `tool_help` schema source). Cite sub-decisions as `ADR-0005 §D5.<M>`. Fix-locus **(C) split, KERNEL-PRIMARY** per the committed P0 (`docs/specs/plan/build/kc-05-progressive-tool-disclosure-a0830ed3/p0-investigation.md`): the `tools[]` serializer, the `AgentTool` trait, and `tool_help` all live in phi-core; the i-phi per-agent policy flag that flips the mechanism ON is an explicit thin consumer follow-on, OUT of scope here. **Back-compat by construction**: the progressive path is opt-in / threshold-gated **OFF by default**, so every existing wire is byte-identical until a consumer flips the flag. No removal / rename / migration / persisted-field / breaking wire-type change; additive public API only.

## Forks

Two forks were **pre-locked** by the user at scoping time (confirmed at gate-1, not re-litigated); five forks locked at gate-1. All landed at the planner recommendation (Direct-approval-clean).

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F-chunk-split | one coupled kernel chunk vs split (**pre-locked**) | **kernel-chunk-first** | The three kernel pieces (#110 trait split, #109 reduced render, `tool_help` schema source) are tightly coupled and change together; the consumer role is only a knob → one kernel chunk + a thin i-phi policy follow-on. |
| F-policy | always-on vs threshold-gated (**pre-locked**) | **threshold-gated, OFF by default** | Always-on rewrites every agent's wire (incl. small close-gate fixtures) forcing re-bless; OFF-default + count/MCP gating bounds the blast radius and keeps small agents byte-unchanged. |
| F1 | char-limit overflow stance (model-facing) | **F1.a — hard-error at registration** (`SHORT_DESCRIPTION_MAX_CHARS = 256`) | A description the model reads is load-bearing; fail-loud (no silent model-facing truncation). Forces the `revert_to_state` split as a must-edit — which IS the fix. 256 leaves every built-in (122–196 chars) compliant. |
| F2 | validation-stance locus (**TECHNICAL**) | **F2.b — kernel primitive + consumer-overridable stance** | Per `[[feedback_phi_core_kernel_minimal]]`: the char-limit const + validation helper is genuinely-general kernel surface; the *stance* (hard-error vs truncate-warn) is policy the consumer owns. No policy baked in. |
| F3 | `tool_help` return shape (model-facing; **coupled to F4**) | **F3.b — detailed manual + full `parameters_schema`** | A reduced catalog loses ZERO arg-formatting fidelity — the model fetches the exact schema on commit. MCP tools get their full `inputSchema` for free. |
| F4 | reduced-`parameters` wire shape (model-facing; **coupled to F3**) | **F4.b — minimal-valid stub `{"type":"object"}`** | Provider-safe with zero wire-type change (Anthropic REQUIRES `input_schema` to be an object; both provider test fixtures already ship `{"type":"object"}`). F4.a (omit) would force a breaking `Option<..>` wire-type change + risk rejection. |
| F5 | threshold shape + OFF-default (model-facing) | **F5-rec — bundled `ProgressiveToolCatalog` field, default OFF, count/MCP-gated** | One struct field keeps the `AgentLoopConfig` construction cascade at 1 line/site; OFF-default is the back-compat guard (the golden test pins it); count/MCP gating keeps small agents byte-unchanged even when enabled. |

**Coupling note:** F3 + F4 are locked as a coherent pair (`F3.b + F4.b`) — a stubbed wire `parameters` is only safe because `tool_help` serves the real schema on demand.

## Context

phi-core serializes EVERY tool's full `description()` + `parameters_schema()` into the turn-1 `tools[]` array unconditionally (`src/agent_loop/streaming.rs`), so an agent with many/large tools pays the entire catalog on every turn. The P0 reproduced this on a real wire: `revert_to_state` alone is ~2.9 KB (~37% of a 10-tool array), its 1870-char manual eager despite `tool_help` existing. MCP servers magnify this — each MCP tool carries its full `inputSchema`.

Per the P0 fix-locus table, the mechanism is **kernel-primary**: (i) #110's trait split cannot be added to a phi-core trait from a downstream crate (Rust coherence); (ii) the reduced render happens at the kernel serializer bridge; (iii) `tool_help` is a kernel tool. The i-phi consumer residual is thin (a per-agent policy flag + validation strictness), deferred to a follow-on. MCP mapping is **automatic** — `McpToolAdapter` is a kernel type whose `description()` = MCP short and `parameters_schema()` = full `inputSchema`, so once the kernel renders `short_description()` (default = `description()`) and `tool_help` serves `parameters_schema()`, MCP tools get the split for free (zero new MCP code).

Per `[[feedback_phi_core_kernel_minimal]]`: KC-05 adds only genuinely-general primitives (trait split + reduced render + on-demand schema source + config primitive + validation primitive); zero i-phi/consumer policy logic leaks into the kernel.

## Sub-decisions

### §D5.1 — Short/detailed description split as default methods (resolves part of F1/F3)

Net-new surface — **no prior short/detailed behaviour to preserve**; these are net-new default methods at KC-05. The `AgentTool` trait (`src/types/tool.rs`) gains `fn short_description(&self) -> &str { self.description() }` + `fn detailed_description(&self) -> Option<&str> { None }` as **default methods**, plus `fn has_large_schema(&self) -> bool { false }` (the `engage_on_large_schema` trigger — a **general schema-magnitude signal**, not MCP-specific). Defaults keep all 12 kernel + 3 i-phi + MCP/OpenApi/SubAgent impls compiling unchanged — the `timeout()` default-method precedent. `McpToolAdapter` **and** `OpenApiToolAdapter` override `has_large_schema()` to `true` (both carry large parameter schemas — MCP `inputSchema`s / OpenAPI-generated operation schemas). A tool that wants a real split overrides `short_description()` + `detailed_description()`. **(Generalized from a first-cut `is_mcp_adapter()` per gate-4 Audit-C: naming the signal after the property it keys on — a large deferred schema — future-proofs the public trait API and lets OpenAPI opt into the same magnification trigger without a second method.)**

### §D5.2 — Config-gated reduced-render branch (resolves part of F4)

**Pre-existing-behaviour:** the historical full render at the serializer bridge (`stream_assistant_response`) is preserved byte-for-byte on the OFF path. A new branch, reachable only when `config.progressive_tool_catalog.engages(tools)` returns `true`, emits `ToolDefinition { description: short_description(), parameters: json!({"type":"object"}) }`; the `else` arm reconstructs the exact historical `ToolDefinition { description: description(), parameters: parameters_schema() }` verbatim. Default OFF ⇒ `engages()` returns `false`, the branch is unreachable, and the wire is byte-identical (the golden-OFF test pins it). `sub_agent` / `parallel` / `evaluation` inherit the branch automatically (they route through the same single bridge — no bespoke `ToolDefinition` construction exists elsewhere).

### §D5.3 — Catalog-backed `tool_help` schema source (resolves F3)

**Pre-existing-behaviour:** the static hand-written `with_default_help` map path is retained; the catalog path supersedes it only when a snapshot is installed. `ToolHelpTool` (`src/tools/tool_help.rs`) gains a shared catalog slot (`Arc<Mutex<Option<ToolCatalogSnapshot>>>`) filled at `build_config` time via `build_tool_catalog_snapshot(&tools)` (a `BTreeMap<String, (detailed, schema)>` sourced from the folded tool Vec). On `tool_help(<name>)`, if the name is in the snapshot, `execute()` returns the tool's `detailed_description()` (or the static-map body when a tool has no detailed body) **plus its full `parameters_schema()`** — including an MCP tool's complete `inputSchema`; otherwise it falls back to the static map + miss message. The snapshot is **cycle-free** (it holds owned data, not tool `Arc`s) and installed via interior mutability so `BasicAgent::build_config(&self)` can fill it without downcasting the type-erased `ToolHelpTool` back out of the `Vec<Arc<dyn AgentTool>>`.

### §D5.4 — Char-limit const + registration validation primitive (resolves F1 + F2)

Net-new primitive — **no prior registration-validation behaviour to preserve**. `src/types/tool.rs` adds `pub const SHORT_DESCRIPTION_MAX_CHARS: usize = 256`, `pub fn validate_tool_registration(&dyn AgentTool) -> Result<(), ToolRegistrationError>`, `pub fn tool_registration_warning(&dyn AgentTool) -> Option<String>`, and `ToolRegistrationError { MissingName, MissingShortDescription, ShortDescriptionTooLong }`. The kernel DEFAULT stance is hard-error (F1.a): an empty name/short or an over-limit short returns `Err`; a missing `detailed_description()` is a non-fatal advisory. Per F2.b, phi-core exposes the primitive but does **not** force-invoke it on the hot path — the enforcement *stance* is a consumer-overridable default, so no policy is baked into the kernel. A kernel test asserts every `default_tools()` built-in passes validation (the CI guard that keeps catalogs lean-by-contract).

### §D5.5 — `progressive_tool_catalog` config field (resolves F5)

Additive — a single new non-optional field `pub progressive_tool_catalog: ProgressiveToolCatalog` on `AgentLoopConfig` (`src/agent_loop/config.rs`), where `ProgressiveToolCatalog { enabled: bool, min_tools: usize, engage_on_large_schema: bool }` derives no `Default` on `AgentLoopConfig` (which has none), so the field cascades to every full struct-literal construction site — 3 production + 17 test literals (20 edits; 2 spread sites inherit). `ProgressiveToolCatalog::default()` = `{ enabled: false, min_tools: 8, engage_on_large_schema: true }`; the `engages(tools)` predicate returns `enabled && (tools.len() > min_tools || (engage_on_large_schema && any has_large_schema()))`. Builder: `BasicAgent::with_progressive_tool_catalog(...)`. Bundling the F5 knobs into ONE struct keeps the per-site cascade at exactly one line. **Pre-existing-behaviour preservation note (net-new field):** there is no prior progressive behaviour; the field defaults OFF so no existing wire changes.

### §D5.6 — `revert_to_state` description split (resolves part of F1)

**Behaviour changed (model-facing, intentional — this IS the fix):** `RevertTool::description()` was the ~1870-char tree/node manual (over the 256-char limit); KC-05 shortens it to a ≤256-char one-liner ("Rewind the conversation to an earlier node … Call `tool_help("revert_to_state")` for the full tree model and worked examples.") and moves the full body verbatim into `detailed_description()` (a module `const REVERT_DETAILED`), served on demand via `tool_help` + the catalog snapshot. No model-facing guidance is lost — the full manual is one `tool_help` call away, and the short wire form still names the reference. The three carry-forward tests asserting the old long-description content (`description_teaches_tree_node_model_and_forward_continuation`, `description_teaches_four_categories_as_budget_levers`) are **re-targeted** (predicate-anchored) to `detailed_description()`; the tool_help-reference test stays on the (short) `description()`.

### §D5.7 — F4.b minimal-valid stub wire shape (resolves F4)

Net-new reduced-wire shape — **no prior reduced render to preserve**. The reduced branch sets `parameters = json!({"type":"object"})` — a value change **at the bridge only**, with NO change to the `ToolDefinition` wire type (`parameters` stays a non-optional `serde_json::Value`) and NO change to either provider's serialization (`anthropic.rs` maps `parameters → input_schema`; `openai_compat.rs` maps `parameters → parameters`). Plan-time provider inspection is decisive: Anthropic REQUIRES `input_schema` to be an object, so F4.a (omit) would be rejected AND would force a breaking `Option<..>` wire-type change cascading through both providers. Both provider test fixtures already ship `{"type":"object"}`, so the stub is proven-accepted; Tier G tests assert it serializes cleanly through both providers. The live close-gate confirms on a real wire.

## Cross-references

- **Concept / architecture docs**: `docs/concepts/tools.md` (§"The AgentTool Trait" default-methods + registration-validation subsections; §"The `tool_help` documentation channel" catalog-backed source; §"Progressive tool-catalog disclosure"), `docs/architecture/algorithms/core/streaming.md` (`stream_assistant_response` reduced-render branch), `docs/specs/developer/tool.md` (default methods + registration validation tables), `docs/reference/{configuration,api,glossary}.md` (`ProgressiveToolCatalog` field + `with_progressive_tool_catalog` builder + capability row).
- **Closed issues**: GitHub #110 (tool-registration contract — enabler), #109 (progressive tool-catalog disclosure).
- **Prior ADRs cited**: ADR-0003 (revert tail-shrink contract — the `description()` body split here preserves its guidance in `detailed_description()`); ADR-0004 (selectable skill-prompt layout — the sibling eager-catalog + lazy-body precedent, applied here to the native function-calling array).
- **Forward-scope + P0**: `docs/specs/plan/forward-scope/kc-05-progressive-tool-disclosure.md`; committed P0 `docs/specs/plan/build/kc-05-progressive-tool-disclosure-a0830ed3/p0-investigation.md`.

## Consequences

### For the i-phi per-agent policy follow-on

phi-core ships the mechanism + conservative OFF defaults. The thin i-phi follow-on flips `progressive_tool_catalog.enabled` per agent (mirroring the MA-cluster `<x>_override: Option<T>` pattern) + chooses validation strictness — that follow-on is where an operator gets the visible per-turn token-cost drop. Nothing downstream is blocked; the follow-on is unblocked immediately once i-phi tracks the new phi-core commit.

### For skills (#111) / memory (#112)

The #110 short/detailed trait contract sets the pattern the same structured-frontmatter idea could extend to skills + memory catalogs. Those are consumer-rendered prompt-text blocks (a different mechanism) and are out of scope here; this trait change is the reference pattern.

## Revisit triggers

- A provider rejects the `{"type":"object"}` stub on a real wire → re-open §D5.7 (the close-gate is the proving ground).
- A consumer needs a *truly-omitted* `parameters` (fewer bytes than the stub) → §D5.7 wire-type change (`ToolDefinition.parameters: Option<..>` + both-provider re-validation).
- The `tool_help` catalog snapshot proves too stale for a dynamic tool set (tools mutated between `build_config` and use) → §D5.3 (rebuild cadence).
- The 256-char `SHORT_DESCRIPTION_MAX_CHARS` proves too tight for a legitimate tool → §D5.4 (limit value is a kernel-chosen conservative default).
- A consumer needs the truncate-with-warn stance rather than hard-error → §D5.4 (the stance is already a consumer-overridable default; formalize a config knob if demand appears).
- `min_tools = 8` / `engage_on_large_schema = true` defaults prove wrong for a real workload → §D5.5 (consumer-tunable knobs).

## Verification

```bash
# Workspace health (phi-core: host cargo, single crate, NO --workspace)
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml   # 609 passed (589 baseline + 20 NEW)

# Kernel-minimality: zero NEW consumer-policy leakage in the new surface
grep -rniE "i-phi|iphi|per.agent.policy|daemon|assemble" \
  src/tools/tool_help.rs src/types/tool.rs src/agent_loop/config.rs
#   → 1 PRE-EXISTING hit only (config.rs revert_render_policy doc, KC-01/02/03 artifact); KC-05's new surface is clean

# 12 kernel AgentTool impls compile unchanged (default methods only)
/root/rust-env/cargo/bin/cargo build -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
```

- **Golden-OFF byte-for-byte guard**: `progressive_tool_catalog_test::golden_off_wire_is_byte_identical` pins the default (OFF) turn-1 `tools[]` to the historical full render exactly, even with 10 tools.
- **Progressive-ON**: `progressive_on_reduces_render` (short description + `{"type":"object"}` stub) + `progressive_on_drops_bytes` (measurable byte drop).
- **Threshold predicate**: `engages_below_threshold_is_false` / `engages_above_threshold_is_true` / `engages_on_large_schema_below_count_and_disabled_never`.
- **`tool_help` catalog**: `catalog_serves_detailed_and_full_schema` / `catalog_serves_mcp_full_input_schema` / `catalog_miss_falls_back_to_static_map`.
- **Registration validation**: `validation_passes_for_valid_tool` / `validation_errors_on_over_limit_short` / `validation_errors_on_missing_short_or_name` / `validation_warns_on_missing_detailed` / `kc05_default_tools_pass_registration_validation`.
- **Provider serialization (F4.b)**: `anthropic::tests::kc05_reduced_stub_parameters_serialize_to_input_schema` + `openai_compat::tests::kc05_reduced_stub_parameters_serialize_to_parameters`.
- **Live close-gate (P0 §8, `[[feedback_render_transcript_close_gate]]` Rules 4/8)** — orchestrator-owned at gate-4: boot the real daemon against a progressive-enabled agent with a many-tool / `N>10`-MCP set; render turn-1 `…request.json` and ASSERT THE DISPOSITION — reduced `parameters` (stub) on every entry + measurable byte drop; `revert_to_state.description` is the SHORT one-liner; `tool_help(<name>)` serves the full schema (incl. an MCP `inputSchema`); a model calls `tool_help` then the tool (loop closes). No-400/well-formed is necessary but NOT sufficient.
