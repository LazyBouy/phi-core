<!-- Last verified: 2026-08-19 by Claude Code (KC-05 plan iter-1 draft; phi-core kernel lane; grounded on committed P0 `42c4cbb` + forward-scope `kc-05-progressive-tool-disclosure.md`) -->

# KC-05 — Progressive tool-catalog disclosure + tool-registration contract (kernel mechanism)

> Fifth phi-core kernel chunk. Ships the **kernel-primary** mechanism for #110 (tool-registration contract: short/detailed split) + #109 (progressive tool-catalog disclosure). Fix-locus **(C) split, KERNEL-PRIMARY** per P0 §4. Back-compat by construction: the progressive path is opt-in / threshold-gated OFF by default — every existing wire byte is unchanged until a consumer flips the flag. The thin i-phi per-agent policy flag is an explicit LATER follow-on, NOT this chunk.

- **Project**: phi-core (kernel lane). Root `/root/projects/phi/phi-core`, branch `dev`. Host cargo (`/root/rust-env/cargo/bin/cargo`), single crate, NO `--workspace`, NO Docker.
- **Forward-scope**: `docs/specs/plan/forward-scope/kc-05-progressive-tool-disclosure.md`.
- **Committed P0** (gate-0.5-verified, 8 DEFINITIVE + 1 non-blocking UNRESOLVED): `p0-investigation.md` (this cycle folder; moved in from `_p0-investigations/` at archive).
- **ADR**: `docs/decisions/0005-progressive-tool-disclosure.md` (0001–0004 exist; 0005 is next).
- **Baseline**: 589 tests post-KC-04 (`_cycle-index.md` row `91a175a4`, "+6 tests → 589"). 12 kernel `AgentTool` impls (backcompat blast radius); +3 i-phi impls inherit defaults automatically.

---

## Forks for orchestrator

**Two forks are PRE-LOCKED by the user** (from the earlier scoping session, carried in forward-scope §0). Presented for CONFIRMATION, not re-litigation; planner recommends the same:

- **F-chunk-split → kernel-chunk-first** (P0 §6 Option 1): KC-05 = one coupled kernel chunk (#110 trait contract + #109 reduced render + `tool_help` schema source change together). The i-phi per-agent policy flag ships as a separate thin consumer follow-on later. **Planner: CONFIRM.**
- **F-policy → threshold-gated, OFF by default** (P0 §6 F-policy): the reduced catalog engages only above a tool-count threshold (or when explicitly enabled) — never unconditionally. **Planner: CONFIRM.**

**FIVE forks remain to lock at gate-1.** F1/F3/F4/F5 are model-facing → full 4-line AskUserQuestion template. F2 is a **TECHNICAL FORK** (no user-visible delta) → 2-line template.

> **Coupling note (per v36 bundle-framing).** **F3 and F4 are COUPLED on model arg-fidelity.** If F4 reduces the wire `parameters` (F4.b stub), then F3 MUST serve the full schema (F3.b) — otherwise the model sees a stubbed `parameters` on the wire AND `tool_help` refuses to hand over the real schema, so the reduced catalog would lose ALL arg-formatting fidelity (an incoherent lock-set). Conversely F3.a (detailed-only) is only coherent with F4.a (full `parameters` retained on the wire — which defeats the byte-drop goal). **Lock F3 + F4 as a coherent pair.** The planner-rec pair (F3.b + F4.b) is the coherent "reduce the wire, serve fidelity on demand" set. The other coherent pair (F3.a + F4.a) reduces nothing and is not recommended. F1×F2 compose independently (F1 = the kernel default stance; F2 = where the stance lives). The whole planner-rec set (F1.a + F2.b + F3.b + F4.b + F5-rec) is internally coherent.

### F1 — char-limit overflow stance (model-facing)

| Option | User-visible (what the operator/model perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F1.a (planner-rec)** — hard-error at registration | A tool whose `short_description` exceeds the limit fails fast at agent-build/boot with a clear error; no silent model-facing truncation ever ships. | - Clean: forces the `revert_to_state` 1870-char split as a must-edit (that IS the fix). - Load-bearing model-facing text is fail-loud (domain-conditional trend: descriptions the model reads are load-bearing, not teaching-material). - Kernel built-ins (122–196 chars) stay compliant at a 256-char limit. | - A consumer registering an over-long tool gets a boot failure (mitigated: F2.b makes the stance consumer-overridable). **Product trajectory:** guarantees every downstream consumer's tool catalog stays lean-by-contract; a future lint/CI can assert the limit at build time. | LOCK? |
| F1.b — truncate-with-warn | An over-long description is silently shortened on the wire and a warning is logged; the agent still boots. | - Lenient; never blocks a boot. - Matches teaching-material leniency (KC-04 F2.a precedent). | - A silently-truncated description is model-facing and may cut mid-sentence → degraded tool use with no hard signal. **Product trajectory:** tolerates catalog bloat creeping back; the limit becomes advisory, not enforced. | NOT chosen |

**Limit value (both options)**: recommend **256 chars** for `short_description` (comfortable headroom above `edit_file`'s 196; forces only the `revert_to_state` split). The const is a kernel primitive (F2).

**AskUserQuestion (4-line):**
- **User-visible:** F1.a fails fast at boot on an over-long tool description (no silent model-facing truncation); F1.b silently truncates + warns.
- **Product trajectory:** F1.a keeps every downstream tool catalog lean-by-contract; F1.b lets bloat creep back as advisory-only.
- **Cycle scope:** either is ~1 const + ~1 validation helper; F1.a additionally forces the `revert.rs` split (already a KC-05 deliverable regardless).
- **Defers if chosen:** F1.a defers nothing; F1.b defers "enforce the limit" to a future lint chunk.

### F2 — validation-stance locus  **TECHNICAL FORK** (no user-visible delta — pick on engineering merit only)

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F2.b (planner-rec)** — kernel exposes the const + validation helper; stance is a consumer-overridable default | Per `[[feedback_phi_core_kernel_minimal]]`: the primitive (char-limit const + `validate_tool_registration` helper) is genuinely-general kernel surface; the STANCE (hard-error vs truncate-warn) is policy the consumer owns. Zero i-phi flavor leaks in. | The kernel ships a default stance the consumer may not notice it can override (mitigated by rustdoc). | LOCK? |
| F2.a — kernel hard-codes the stance | Simplest; one code path. | Bakes a policy choice into the kernel; a consumer that wants the other stance must fork or wrap — the exact kernel-creep [[feedback_phi_core_kernel_minimal]] forbids. | NOT chosen |

**AskUserQuestion (2-line, TECHNICAL):**
- **Engineering merit:** F2.b keeps phi-core minimal (primitive in kernel, policy in consumer per the kernel-minimality memory); F2.a is simpler but bakes a consumer policy into the kernel.
- **Recommendation:** F2.b.

### F3 — `tool_help` return shape (model-facing; COUPLED to F4)

| Option | User-visible (what the model perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F3.b (planner-rec)** — detailed manual + full `parameters_schema` | When the model calls `tool_help(<name>)` it gets the tool's full manual AND its exact JSON-Schema, so it can format arguments correctly even though the turn-1 catalog carried only a stub. | - A reduced catalog loses ZERO arg-formatting fidelity — the schema is fetched on commit. - MCP tools get their full `inputSchema` for free (`McpToolAdapter` is a kernel type). - Closes the "redundant-on-top" static-map duplication for the progressive case. | - `tool_help` output is larger per call (only paid on demand, not every turn). **Product trajectory:** the model can safely use any reduced-catalog tool; enables aggressive catalog reduction downstream. | LOCK? |
| F3.a — detailed manual only | The model gets the prose manual but must infer arg shapes from it. | - Smaller `tool_help` output. | - **Incoherent with F4.b**: a stubbed wire `parameters` + no schema on demand = the model can never see the real arg schema. **Product trajectory:** blocks safe reduction of `parameters`. | NOT chosen (incoherent with F4.b) |

**AskUserQuestion (4-line):**
- **User-visible:** F3.b lets the model fetch a tool's full JSON-Schema on demand (correct arg formatting despite a lean catalog); F3.a serves prose only.
- **Product trajectory:** F3.b unlocks safe aggressive catalog reduction; F3.a caps how much the wire can be reduced.
- **Cycle scope:** F3.b needs a catalog-backed schema source in `tool_help.rs` (snapshot of name→(detailed, schema) built at agent-build time); F3.a reuses the static map.
- **Defers if chosen:** F3.b defers nothing; F3.a defers arg-schema-on-demand to a follow-on and constrains F4.

### F4 — reduced-`parameters` wire shape (model-facing; COUPLED to F3)

| Option | User-visible (what the model perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F4.b (planner-rec)** — minimal-valid stub `{"type":"object"}` | Each reduced catalog entry carries name + short description + an empty-but-valid `parameters` object; the model fetches the real schema via `tool_help`. | - **DECISIVE plan-time provider finding** (see §3): `ToolDefinition.parameters` is a non-optional `serde_json::Value`; `anthropic.rs:642` maps it to `"input_schema"` (Anthropic REQUIRES an object; `null` rejected); `openai_compat.rs:634` maps to `"parameters"`. Both provider test fixtures ALREADY use `{"type":"object"}` (`anthropic.rs:887`, `openai_compat.rs:883`) → proven accepted. - Zero wire-type change; low cascade. - Measurable byte drop (`revert_to_state`'s 1010-byte params → ~20 bytes). | - Not literally zero bytes for `parameters`. **Product trajectory:** universally provider-safe reduction; the close-gate confirms on the real wire. | LOCK? |
| F4.a — omit `parameters` entirely | Catalog entries carry name + short description only, no `parameters` key. | - Fewest bytes. | - Requires making `ToolDefinition.parameters` an `Option<serde_json::Value>` — a **breaking wire-type change** cascading through both providers + the golden-OFF test framing; and Anthropic rejects a null/absent `input_schema`. **Product trajectory:** higher risk of provider rejection; larger blast radius. | NOT chosen |

**AskUserQuestion (4-line):**
- **User-visible:** identical to the model in both cases (it fetches the schema via `tool_help`); the difference is wire safety. F4.b ships a minimal-valid stub every provider accepts; F4.a omits `parameters` and risks provider rejection.
- **Product trajectory:** F4.b is universally provider-safe; F4.a optimizes ~20 bytes at the cost of a breaking wire-type change + rejection risk.
- **Cycle scope:** F4.b = a value change at the bridge only (no type change); F4.a = `ToolDefinition.parameters: Option<..>` + both providers + wire-type churn.
- **Defers if chosen:** F4.b defers nothing; F4.a is not recommended given the provider evidence.

### F5 — threshold shape + OFF-default (model-facing)

| Option | User-visible (what the operator perceives) | Pros | Cons + Product trajectory | Status |
|---|---|---|---|---|
| **F5-rec (planner-rec)** — single bundled config field, default OFF; engage above `min_tools` OR when MCP attached | An operator (later, via the i-phi follow-on) opts an agent into progressive mode; below the threshold OR with the flag off, the wire is byte-identical to today. | - One `progressive_tool_catalog: ProgressiveToolCatalog` field (small struct, `Default` = disabled) → the AgentLoopConfig cascade is exactly 1 line per construction site regardless of how many knobs. - Default OFF preserves every existing wire (golden test guards it). - Threshold + MCP-trigger both computable at the bridge from `context.tools.len()`. | - The threshold value is a tunable the kernel picks a conservative default for. **Product trajectory:** consumer tunes per-agent; the kernel default stays safe. | LOCK? |
| F5-alt — bare `bool` flag, always-engage-when-true (no count threshold) | Flag on = every agent's catalog reduces regardless of tool count. | - Simplest. | - Always-on-when-true rewrites even 4-tool agents' wires (forces every small close-gate fixture re-bless); no count-gate. **Product trajectory:** blast radius is larger; loses the "small agents stay full" property. | NOT chosen |

**Threshold default (F5-rec)**: `enabled: false` (OFF); when enabled, engage iff `tools.len() > 8` **OR** an MCP tool is attached (MCP tools carry large `inputSchema` — the P0 magnification case). The 10-tool close-gate fixture engages; a 4-tool agent stays full even when enabled. Values are consumer-tunable knobs; the kernel ships conservative defaults.

**AskUserQuestion (4-line):**
- **User-visible:** F5-rec keeps small agents byte-unchanged even when a consumer enables progressive mode (count/MCP-gated); F5-alt reduces every agent when flipped on.
- **Product trajectory:** F5-rec bounds blast radius + keeps small fixtures stable; F5-alt is simpler but re-blesses every wire.
- **Cycle scope:** both add one config field; F5-rec's field is a small struct (threshold + mcp-trigger + enabled), F5-alt is a bare bool.
- **Defers if chosen:** both confirm the OFF-default pre-lock (F-policy); the per-agent policy that flips it is the i-phi follow-on.

---

## §1 — Locked fork details (per chunk-planner v32 iter-1 template; planner-rec bodies pre-filled — pre-lock draft)

> Populated at iter-1 for ALL forks per v32 P-plan-1-v32. If the user locks all planner-recs → archive at iter-1 (P-orch-8 skip-condition). If any fork locks USER-DIVERGENT → iter-2 narrow re-author of only that subsection.

> **GATE-1 LOCK OUTCOME (2026-08-19 — ALL planner-rec → Direct-approval-clean; P-orch-8 skip-condition FIRES → archive at iter-1, no iter-2 re-spawn):** F1 = **F1.a** (hard-error @ registration; 256-char limit) · F2 = **F2.b** (kernel primitive + consumer-overridable stance — orchestrator-adopted, TECHNICAL fork) · F3+F4 = **F3.b + F4.b** (reduce wire to `{"type":"object"}` stub, serve full schema on demand via `tool_help`) · F5 = **F5-rec** (bundled `ProgressiveToolCatalog` field, OFF default, count/MCP-gated). Pre-locks **CONFIRMED**: F-chunk-split (kernel-chunk-first) + F-policy (threshold-gated OFF-default). Every `*(pre-lock draft; finalizes at gate-1 lock)*` annotation below is now a **FINAL LOCK**.

#### F1 = F1.a — hard-error at registration *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): A kernel char-limit const (`SHORT_DESCRIPTION_MAX_CHARS: usize = 256`) + a validation helper (`validate_tool_registration(&dyn AgentTool) -> Result<(), ToolRegistrationError>`) are added in `src/types/tool.rs` alongside the trait (co-located, kernel-minimal). The helper returns `Err` when `short_description()` exceeds the const (over-limit) or is empty/missing a name; a missing `detailed_description()` produces a warn, not an error. The hard-error stance is the kernel DEFAULT the helper enforces; `revert_to_state`'s 1870-char `description()` is split (deliverable 6) so it passes the check.

**Rationale** (3 sentences): A silently-truncated model-facing description degrades tool use with no hard signal, so a description the model reads is load-bearing and fail-loud is correct (the domain-conditional trend from KC-04/CC-28 puts load-bearing config on the strict side). Hard-error forces the `revert.rs` split as a must-edit — which IS the fix #109 wants — rather than leaving it to a later cleanup. A 256-char limit leaves every kernel built-in (122–196 chars) compliant, so the only forced edit is the intended `revert_to_state` split.

**Defers (if chosen)** (3 sentences): Nothing is deferred for KC-05 — the const, helper, and `revert` split all ship in-chunk. A build-time lint/CI that asserts the limit across a downstream's whole catalog is a possible future consumer convenience but is out of scope. The consumer-side ability to relax the stance is delivered by F2.b in this same chunk (the stance is a consumer-overridable default).

#### F2 = F2.b — kernel primitive + consumer-overridable stance *(pre-lock draft; finalizes at gate-1 lock)* — **TECHNICAL FORK**

**Code-level binding** (1 sentence, TECHNICAL): The kernel exposes the char-limit const + `validate_tool_registration` helper as public primitives; the enforcement STANCE (hard-error vs truncate-warn) is a parameter/default the consumer may override, so no policy is hard-coded in phi-core.

**Rationale** (3 sentences): `[[feedback_phi_core_kernel_minimal]]` requires phi-core to ship general mechanism and leave policy to the consumer; the char-limit + validation shape is genuinely general, but "what to do on overflow" is a policy the i-phi follow-on owns per agent. Baking the stance into the kernel (F2.a) would force any consumer wanting the other stance to fork or wrap phi-core's own registration path — the exact kernel-creep the memory forbids. Exposing a defaultable primitive keeps the kernel minimal while making F1's hard-error the safe default.

**Defers (if chosen)** (3 sentences): The per-agent choice of stance (which agents hard-error vs truncate-warn) is deferred to the i-phi per-agent policy follow-on, mirroring the MA-cluster `<x>_override: Option<T>` pattern. KC-05 ships only the kernel default (hard-error, per F1.a) plus the override seam. No consumer-specific validation logic enters the kernel.

#### F3 = F3.b — `tool_help` returns detailed manual + full `parameters_schema` *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): `ToolHelpTool` (`src/tools/tool_help.rs`) gains a catalog-backed schema source: at agent-build time a snapshot `BTreeMap<String, (Option<String> detailed, serde_json::Value schema)>` is assembled from the fully-folded tool Vec and installed via a new constructor (e.g. `ToolHelpTool::with_catalog(...)`), so `execute()` returns the named tool's `detailed_description()` + its full `parameters_schema()` (incl. an MCP tool's full `inputSchema`). The static hand-written map path (`with_default_help`) is retained for back-compat; the catalog-backed path supersedes it when installed. The snapshot approach is cycle-free (no `Arc` back-reference from a tool into its own catalog); it is installed at `build_config`/`with_tool_help` time once the full set is known.

**Rationale** (3 sentences): This is the load-bearing coupling with F4 — a reduced wire (F4.b stub `parameters`) loses no arg-formatting fidelity ONLY because the model can fetch the exact schema on demand via `tool_help`. Serving the schema is what makes catalog reduction safe for any model to consume. It also dissolves the "redundant-on-top" duplication the P0 confirmed (finding 3): the full manual leaves the eager catalog and lives in `tool_help`.

**Defers (if chosen)** (3 sentences): Nothing is deferred for the mechanism — the catalog-backed source ships in-chunk. This body is coherent ONLY because F4 locks to F4.b (reduced wire needing on-demand schema); if F4 flips to F4.a (full wire retained), F3 could revert to F3.a detailed-only, so a USER-DIVERGENT F4 lock re-opens this subsection. No consumer-specific help content enters the kernel (consumers still supply richer bodies via `ToolHelpTool::new`).

#### F4 = F4.b — minimal-valid stub `{"type":"object"}` on the reduced wire *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): The reduced-catalog branch at `streaming.rs:282-290` sets `ToolDefinition.parameters = json!({"type":"object"})` (and `description = short_description()`) when progressive is engaged; the OFF path keeps `t.parameters_schema()` verbatim, byte-for-byte. No change to the `ToolDefinition` wire type (`parameters` stays a non-optional `serde_json::Value`) and no change to either provider's serialization (`anthropic.rs:642` still maps `parameters → input_schema`; `openai_compat.rs:634` still maps `parameters → parameters`). The stub is the same shape the provider test fixtures already ship (`anthropic.rs:887`, `openai_compat.rs:883`).

**Rationale** (3 sentences): Plan-time provider inspection is decisive — Anthropic REQUIRES `input_schema` to be an object, so an omitted/null `parameters` (F4.a) would be rejected, and F4.a would additionally force a breaking `ToolDefinition.parameters: Option<..>` wire-type change cascading through both providers. F4.b achieves the measurable byte drop (`revert_to_state`'s 1010-byte params collapse to ~20 bytes) with zero wire-type churn and universal provider acceptance. The close-gate confirms on a real wire (P0 §8), closing the one non-blocking UNRESOLVED (large-MCP magnification).

**Defers (if chosen)** (3 sentences): Nothing is deferred. This body is coupled to F3.b (the stub is only safe because `tool_help` serves the real schema on demand); a USER-DIVERGENT F3 lock re-opens this subsection. A future "truly omit `parameters`" optimization would need the wire-type change and provider re-validation and is explicitly out of scope.

#### F5 = F5-rec — single bundled config field, default OFF, count/MCP-gated *(pre-lock draft; finalizes at gate-1 lock)*

**Code-level binding** (3 sentences): A single additive field `pub progressive_tool_catalog: ProgressiveToolCatalog` is added to `AgentLoopConfig` (`config.rs`, after `provider_wire_sink` at :382), where `ProgressiveToolCatalog { enabled: bool, min_tools: usize, engage_on_mcp: bool }` derives `Default` = `{ enabled: false, min_tools: 8, engage_on_mcp: true }`. The reduced branch at `streaming.rs:282-290` engages iff `enabled && (context.tools.len() > min_tools || (engage_on_mcp && any tool is an MCP adapter))`. Because `AgentLoopConfig` has no `Default` impl, the new field is threaded to the 3 production struct-literal sites (`agent.rs:379`, `sub_agent.rs:250`, `basic_agent.rs:1270`) + 28 test-literal sites (1 line each) exactly as the recent `response_format`/`provider_wire_sink` fields were; a builder `BasicAgent::with_progressive_tool_catalog(...)` is added.

**Rationale** (3 sentences): A single struct field (vs several bare fields) keeps the AgentLoopConfig construction-site cascade at exactly one line per site while bundling the F5 threshold + MCP trigger + enabled flag together and leaving room for future knobs. Default OFF is the load-bearing back-compat guard — the reduced branch is unreachable until a consumer flips `enabled`, so every existing wire stays byte-identical (the golden-OFF test pins it). Count/MCP gating (vs a bare always-on bool) keeps small agents byte-unchanged even when enabled, bounding the blast radius exactly as F-policy intends.

**Defers (if chosen)** (3 sentences): The per-agent POLICY that flips `enabled` (which agents, what threshold override, validation strictness) is deferred to the thin i-phi per-agent follow-on (mirrors the MA-cluster `<x>_override: Option<T>` pattern). KC-05 ships the kernel mechanism + conservative defaults only. No i-phi-specific policy struct enters the kernel.

#### F-chunk-split = kernel-chunk-first (PRE-LOCKED — confirm) *(pre-lock; user-confirmed)*

**Code-level binding** (3 sentences): KC-05 is the single coupled kernel chunk touching the `AgentTool` trait + the `streaming.rs` serializer + `tool_help.rs` + `revert.rs` + `AgentLoopConfig`. The three kernel pieces (#110 short/detailed split, #109 reduced render, `tool_help` schema source) change together and cannot be decoupled without churn. The i-phi per-agent policy flag is a separate downstream chunk that flips the kernel flag once i-phi tracks the new phi-core commit.

**Rationale** (3 sentences): #110 enables #109 (the reduced catalog renders the trait's short field; `tool_help` serves the detailed field), so splitting them would ship a half-wired mechanism. All three pieces are kernel-owned and tightly coupled, so one kernel chunk minimizes cross-chunk seams. The consumer role is only a knob, which is genuinely thin and belongs downstream.

**Defers (if chosen)** (3 sentences): The i-phi per-agent policy flag + validation strictness choice is deferred to the i-phi follow-on. Sibling structured-frontmatter contracts for skills (#111) / memory (#112) are a different (prompt-text) mechanism and are out of scope; the P0 notes this trait change sets the pattern. No consumer wiring lands in KC-05.

#### F-policy = threshold-gated, OFF by default (PRE-LOCKED — confirm) *(pre-lock; user-confirmed)*

**Code-level binding** (3 sentences): The kernel flag `ProgressiveToolCatalog` defaults to `enabled: false` (below-threshold inert), so the reduced branch is unreachable with zero consumer action. Engagement is count- and/or MCP-gated, never unconditional. This is the same field described in F5's binding.

**Rationale** (3 sentences): Always-on would rewrite every agent's wire — including the 4/10-tool close-gate fixtures — forcing every wire fixture re-bless, which the golden byte-for-byte OFF test is designed to prevent. Threshold/opt-in bounds the blast radius and keeps small agents byte-unchanged. This is the one real hazard (default-drift) the forward-scope §10 names; OFF-by-default is the guard.

**Defers (if chosen)** (3 sentences): The choice of WHEN to flip the flag per agent is the i-phi follow-on's policy. KC-05 ships only the OFF-default mechanism. Nothing downstream is blocked; the follow-on is unblocked immediately.

---

## §1 — Context & principle

- **Why this chunk.** Today phi-core serializes EVERY tool's full `parameters_schema()` + full `description()` into the turn-1 `tools[]` array unconditionally (`streaming.rs:282-290`), so an agent with many/large tools pays the entire catalog on every turn. The P0 reproduced this on a real wire: `revert_to_state` alone is ~2.9 KB (~37% of a 10-tool array), its 1870-char manual eager despite `tool_help` existing (P0 §5, finding 6). KC-05 gives phi-core a progressive-catalog primitive so a config-enabled agent sends a lean turn-1 array (name + short description + stub `parameters`) and lets the model fetch a tool's full schema + detailed manual on demand via `tool_help` — the eager-catalog + lazy-body shape skills/memory already have, but for the native function-calling array the kernel owns exclusively. It closes #110 (trait split enabler) + #109 (reduced render + `tool_help` schema source).
- **Kernel-minimality restatement.** phi-core owns the MECHANISM (trait split + reduced render + on-demand schema source + config primitive + validation primitive); the consumer (i-phi, later) owns the POLICY (which agents, what threshold, validation strictness). Per `[[feedback_phi_core_kernel_minimal]]`: KC-05 adds only genuinely-general primitives; zero i-phi/consumer policy logic leaks into the kernel; additive public API; no migration/breaking change.
- **Forward-scope reference.** `docs/specs/plan/forward-scope/kc-05-progressive-tool-disclosure.md` §5 (deliverables 1–8) + §6 (acceptance) + §7 (forks) + §8 (Large audit envelope). Grounded on committed P0 `42c4cbb`.

## §2 — Concept alignment walk

| Concept doc | § anchor | Claim (paraphrase) | Status at chunk-open | Target at chunk-close |
|---|---|---|---|---|
| `docs/concepts/tools.md` | §"The AgentTool Trait" (L5–30; `description()` L14/28, `parameters_schema()` L15/29) | The trait exposes a single `description()` + `parameters_schema()`; no short/detailed split. | honored (matches code) | honored — extended: doc now describes `short_description()` + `detailed_description()` default methods (additive) |
| `docs/concepts/tools.md` | §"The `tool_help` documentation channel" (L143–164) | `tool_help(tool_name)` fetches a tool's extended manual on demand from a hand-written map. | honored (static map today) | honored — extended: doc describes the catalog-backed schema+detail source for progressive mode |
| `docs/concepts/tools.md` | L139 | `default_tools()` = 6 built-ins + `tool_help` (7 total). | honored | honored (unchanged count) |
| `docs/specs/developer/tool.md` | (tool entity deep-dive) | Tool trait method surface + wire mapping. | honored | honored — extended: new default methods + validation primitive documented |
| `docs/architecture/algorithms/core/streaming.md` | (serializer bridge algorithm) | `stream_assistant_response` maps every tool → full `ToolDefinition`. | honored (unconditional full) | honored — extended: config-gated reduced-render branch documented (OFF path unchanged) |
| `docs/reference/configuration.md` / `docs/reference/api.md` | `AgentLoopConfig` field reference | Enumerates `AgentLoopConfig` fields for callers. | honored | honored — extended: `progressive_tool_catalog` field documented (default OFF) |

Rules: no permissions subtree (phi-core has none). No baby-phi `phi-core-mapping.md` hook (this IS phi-core). Every doc whose claims the code touches is listed; each ends `honored` (additive, no contradiction).

## §2.5 — Functional outcome

**Chunk-type**: TECHNICAL-PREREQUISITE (kernel mechanism; ships the primitive, not the per-agent policy that a user flips).
**User-visible delivery**: NONE directly this chunk — default OFF means every existing agent's wire is byte-identical. The capability (a many-tool/MCP agent sends a lean turn-1 catalog + fetches full schemas on demand, cutting per-turn token cost) becomes available to consumers but is not turned on for any agent here.
**Unblocks**: the thin **i-phi per-agent policy follow-on** which flips `progressive_tool_catalog.enabled` per agent (mirrors the MA-cluster `<x>_override: Option<T>` pattern) + sets validation strictness — that follow-on is where an operator/user gets the visible token-cost reduction.
**Why this prerequisite**: the short/detailed trait split, the reduced serializer branch, and the catalog-backed `tool_help` all live in phi-core and change together (Rust coherence forbids a consumer adding trait methods); the kernel must ship the mechanism before any consumer can flip it on.

## §3 — Kernel-minimality surface-discipline map (phi-core-leverage-check N/A) + cascade discipline

> **phi-core-leverage-check is N/A** — the kernel does not consume itself. Substituted with the kernel-minimality surface check per `[[feedback_phi_core_kernel_minimal]]`: verify KC-05 adds only genuinely-general primitives with ZERO i-phi/consumer policy logic. k8s-readiness-check is N/A (phi-core is a library). No `scripts/check-*.sh` exist.

**Kernel-minimality surface check (the substitute for §3 leverage):**

| New kernel surface | General primitive? | Consumer-policy leakage? | Verdict |
|---|---|---|---|
| `AgentTool::short_description()` + `detailed_description()` default methods (`types/tool.rs`) | Yes — every consumer with many/large tools benefits; `timeout()` default-method precedent (`tool.rs:285`). | None — defaults preserve current behaviour. | general |
| Reduced-catalog render branch gated by config (`streaming.rs:282-290`) | Yes — the bridge lives only in the kernel; a consumer cannot intercept it. | None — the WHEN is a config flag the consumer sets; the render mechanism is general. | general |
| `ToolHelpTool` catalog-backed schema source (`tools/tool_help.rs`) | Yes — a kernel tool serving a kernel-known catalog. | None — consumers still supply richer bodies via `ToolHelpTool::new`. | general |
| `SHORT_DESCRIPTION_MAX_CHARS` const + `validate_tool_registration` helper (`types/tool.rs`) | Yes — a general const + helper. | None — the STANCE is a consumer-overridable default (F2.b), not baked. | general |
| `progressive_tool_catalog: ProgressiveToolCatalog` field on `AgentLoopConfig` (`config.rs`) | Yes — a bare flag/threshold. | None — no i-phi-flavored policy struct; the per-agent policy is the follow-on. | general |
| `revert_to_state` description split (`tools/revert.rs`) | Yes — a kernel built-in's own text. | None. | general |

**Forbidden-leakage grep (must return 0):** `grep -rniE "i-phi|iphi|per.agent.policy|daemon|assemble" /root/projects/phi/phi-core/src/tools/tool_help.rs /root/projects/phi/phi-core/src/types/tool.rs /root/projects/phi/phi-core/src/agent_loop/config.rs` — expect **0** consumer references in the new surface.

**MCP mapping is automatic (P0 finding 5):** `McpToolAdapter` (`mcp/tool_adapter.rs:106`) is a kernel type whose `description()` = MCP short and `parameters_schema()` = full `inputSchema`; once the kernel renders `short_description()` (default = `description()`) and `tool_help` serves `parameters_schema()`, MCP tools get the split for free. **Zero new MCP code.**

**Shared-bridge inheritance (P0 §4 verification — CONFIRMED at plan-draft):** `stream_assistant_response` is the ONLY tool-serializer and is called only from `run.rs:343`; `parallel.rs`/`evaluation.rs`/`sub_agent.rs` route through `agent_loop`/`agent_loop_continue` → `run.rs` → the same bridge. No bespoke `ToolDefinition {` construction exists outside `streaming.rs` (grep for `ToolDefinition {` in `src/` returns only the struct def + 2 `#[cfg(test)]` provider fixtures). So `sub_agent`/`parallel`/`evaluation` inherit the reduced branch automatically once the flag is on the config.

### §3 cascade-artifact discipline — AgentLoopConfig new-field cascade

`AgentLoopConfig` has **no `Default` impl / no `#[derive(Default)]`** (verified `config.rs`), so a new non-optional field cascades to every full struct-literal construction site — identical to the recent `response_format` + `provider_wire_sink` additions.

- **(a) Invocation**: `grep -rn "AgentLoopConfig {" /root/projects/phi/phi-core/src /root/projects/phi/phi-core/tests`
- **(b) Raw count**: 1 struct-def (`config.rs:200`) + **3 production literals** + **28 test literals across 10 test files** = 31 construction sites needing the new field (1 line each).
- **(c) Per-file breakdown**:
  - Production: `src/agents/agent.rs:379`, `src/agents/sub_agent.rs:250`, `src/agents/basic_agent.rs:1270` (build_config).
  - Tests (28 literals): `tests/tool_timeout_test.rs`, `credential_refresh_test.rs`, `tool_gate_deny_reason_test.rs`, `sub_agent_test.rs`, `turn_request_capture_test.rs`, `integration_anthropic.rs`, `agent_loop_test.rs`, `session_test.rs`, `async_compaction_strategy_test.rs`, `release_0_10_test.rs`.
- **Mitigation**: bundling F5 into ONE `ProgressiveToolCatalog` struct field keeps the per-site cost at exactly **1 line** regardless of the number of knobs. **Pause if actual construction-site edits > 47 (1.5× the 31 predicted)** — that would signal an unexpected new field surface.

### §3 F4 provider-acceptance check (DECISIVE plan-time finding)

`ToolDefinition.parameters` is a non-optional `serde_json::Value` (`traits.rs:315-321`). Production serialization: `anthropic.rs:642` maps it to `"input_schema"` (Anthropic REQUIRES an object; `null`/absent rejected); `openai_compat.rs:634` maps it to `"parameters"`. Both provider **test fixtures already ship `parameters: json!({"type":"object"})`** (`anthropic.rs:887`, `openai_compat.rs:883`) → the minimal stub is proven-accepted. **Conclusion: F4.b (stub) is provider-safe with zero wire-type change; F4.a (omit) would require `Option<serde_json::Value>` + both-provider churn + risks Anthropic rejection.** The close-gate confirms on a real wire.

### §3 pause-threshold table (per-fork)

| Fork | If locked | Key-file LOC cap | Cascade note |
|---|---|---|---|
| F1.a (rec) | hard-error | `tools/revert.rs` +≤ 40 (split + re-target 3 tests); `types/tool.rs` +≤ 60 (2 default methods + const + helper) | forces revert split only |
| F1.b | truncate-warn | same +≤ 20 | no revert must-edit |
| F3.b (rec) | catalog schema source | `tools/tool_help.rs` +≤ 110 (snapshot map + `with_catalog` + execute branch + inline tests) | snapshot built at build-time (cycle-free) |
| F4.b (rec) | stub | `streaming.rs` +≤ 40 (reduced branch) | zero provider/wire-type change |
| F4.a | omit | `streaming.rs` +≤ 40 **+** `traits.rs` + both providers (breaking) | NOT recommended |
| F5-rec | bundled struct | `config.rs` +≤ 40 (struct + Default + field); 31-site cascade (1 line each); `basic_agent.rs` +≤ 25 builder | see cascade discipline above |

Kernel-minimality LOC guidance (substrate-module calibration, v35 P-plan-2): `tool_help.rs` and `config.rs` carry inline tests + doc-comments → apply ~1.3× the plain-struct baseline. **Pause any single production file > 1.5× its cap** and AskUserQuestion.

## §3.B — K8s microservice readiness check

**N/A — phi-core is a library, not a daemon; K8s readiness is not applicable** (k8s-readiness-check returns N/A for phi-core). No in-process daemon state, no IPC channels owned here, no migration runner, no audit hash-chain. All 7 axes: **N/A**.

## §3.C — User-facing documentation impact map

phi-core doc tree is `docs/{concepts,architecture,specs,reference}/`. 3-tier evaluation adapted:

| Tier | File | Touches? | Action |
|---|---|---|---|
| Concept/architecture | `docs/concepts/tools.md` | Yes — trait gains 2 default methods; `tool_help` gains catalog-backed schema source | (a) update in-chunk (P-DOCS): document `short_description()`/`detailed_description()` + progressive `tool_help` behaviour + `[EXISTS]` tags |
| Concept/architecture | `docs/architecture/algorithms/core/streaming.md` | Yes — reduced-render branch at the serializer bridge | (a) update in-chunk: document the config-gated branch; note OFF path byte-identical |
| Developer spec | `docs/specs/developer/tool.md` | Yes — trait method surface + registration validation primitive | (a) update in-chunk |
| Reference | `docs/reference/configuration.md` + `docs/reference/api.md` | Yes — new `AgentLoopConfig.progressive_tool_catalog` field (default OFF) | (a) update in-chunk: field reference + default |
| Reference | `docs/reference/glossary.md` | Maybe — "progressive tool catalog" term | (a) add a one-line glossary entry in-chunk |
| ADR | `docs/decisions/0005-progressive-tool-disclosure.md` | Yes — NEW ADR | (a) author in-chunk (P-ADR) |

**Doc-LOC threshold (per-doc-class precedent, v31 P-plan-12 adapted; semantic-completeness escape v29 P-plan-7):** each doc update lands `≥ precedent-sibling LOC OR carries the full content axes` (new trait-method rustdoc + progressive `tool_help` note + config field row + OFF-default byte-stability note). Verified-headers refreshed on every touched doc.

No defer decisions — every stale doc is updated in-chunk at P-DOCS.

## §3.E — Anticipated gate-2.5 candidates

- **`tool_help` catalog wiring point.** Deliverable 4 requires the catalog snapshot to be built once the full tool Vec is assembled. If the implementer finds the cleanest install point is `build_config` (basic_agent.rs:1270) vs `with_tool_help` (basic_agent.rs:794), that is an implementation-mechanism choice within F3.b scope → route as an in-phase decision (Route A absorb), NOT a gate-1 fork. Cyclic-`Arc` alternative is explicitly rejected in favor of the snapshot (cycle-free).
- **Threshold default value.** If the close-gate or a reviewer wants a different `min_tools` default than 8, that is a 1-line const tweak → Route A absorb (the value is a consumer-tunable knob regardless).
- **No other candidates anticipated** (no placeholder `Vec::new()` returns, no deferred-but-shipping doc-comments in the touched surface).

## §4 — Issues / drifts closed + deferred functionality

| Issue/drift | Severity | Transition | Notes |
|---|---|---|---|
| `#110` (tool-registration contract) | — | closed at chunk-close | kernel trait split (short/detailed) + registration validation primitive. Enabler for #109. |
| `#109` (progressive tool-catalog disclosure) | — | closed at chunk-close | kernel reduced render + catalog-backed `tool_help`. Validated by the live close-gate. |
| P0 finding 9 (large-MCP magnification unproven by a live artifact) | non-blocking UNRESOLVED | → close-gate item | Carried to §12 open questions; the live close-gate boots a daemon against an `N>10`-tool MCP set + asserts reduced `parameters`. NOT a planning blocker (proven by-construction). |

**Deferred functionality (user-facing translation):**

| Item | User-visible feature deferred | Impact during deferral | Allocation |
|---|---|---|---|
| i-phi per-agent policy flag | An operator turning progressive mode on for a specific many-tool/MCP agent (visible per-turn token-cost drop) | Progressive mode ships OFF; no agent's wire changes until the i-phi follow-on flips the flag per agent | i-phi per-agent policy follow-on (new i-phi drift/chunk) |
| skills (#111) / memory (#112) structured contract | Same short/detailed contract applied to skills + memory catalogs | Those catalogs are prompt-text (different mechanism); unchanged | Future consumer chunk; KC-05 sets the trait pattern |

Any NEW defect KC-05 surfaces is filed as its own D-TEST drift (GitHub-mirrored per `[[feedback_dtest_github_mirror]]`).

## §5 — ADRs drafted

- **ADR number**: **ADR-0005** — `docs/decisions/0005-progressive-tool-disclosure.md` (0001–0004 exist; 0005 next, verified via `ls docs/decisions/`).
- **Title**: "Progressive tool-catalog disclosure + tool-registration contract (kernel mechanism, back-compat-by-default)".
- **Drafted-at-phase**: P-ADR (seal). Status `Proposed` at draft → `Accepted` at chunk-seal.
- **Decision summary**: phi-core ships the kernel MECHANISM (trait short/detailed split as default methods + config-gated reduced render + catalog-backed `tool_help` + registration validation primitive + `revert` split); the reduced path is opt-in / threshold-gated OFF by default; consumer owns policy.

**ADR-0005 top-level section enumeration (implementer authors ALL 7):**
1. `## Forks` — header table capturing the gate-1 lock-state for F1–F5 + the two pre-locks (Direct-approval vs Divergent form per outcome).
2. `## Context` — P0 §4 fix-locus (C kernel-primary) + forward-scope §5 citations + #109/#110.
3. `## Sub-decisions` — one `### §D5.<M>` per fork resolution + supporting decisions:
   - §D5.1 trait split (default methods; `timeout()` precedent) — **Pre-existing absence preserved** (never-shipped-yet variation, v24 P-plan-2: "no prior short/detailed behaviour to preserve — net-new default methods at KC-05").
   - §D5.2 reduced-render branch (OFF path byte-identical) — **Pre-existing-behaviour preserved**: `streaming.rs:282-290` full-render is unchanged on the OFF path (net-new branch only reachable when enabled).
   - §D5.3 catalog-backed `tool_help` — **Pre-existing-behaviour preserved**: static-map `with_default_help` retained; catalog path supersedes only when installed.
   - §D5.4 char-limit const + validation stance (F1.a hard-error default; F2.b consumer-overridable) — net-new primitive.
   - §D5.5 `progressive_tool_catalog` config field (F5-rec; OFF default) — additive; no `Default` impl so 31-site cascade.
   - §D5.6 `revert_to_state` description split — **Behaviour changed**: `description()` was the ~1870-char manual (`revert.rs:107`); KC-05 shortens it + moves the body to `detailed_description()`/`tool_help`.
   - §D5.7 F4.b stub wire shape — provider-acceptance rationale (Anthropic `input_schema` object requirement).
4. `## Cross-references` — (a) concept docs + lines (`concepts/tools.md` L5–30 / L143–164); (b) closed issues #109 + #110; (c) prior ADRs cited (ADR-0003 revert contract, ADR-0004 selectable skill-prompt layout as the eager-catalog+lazy-body sibling precedent); (d) forward-scope row path + the committed P0.
5. `## Consequences` — `### For the i-phi per-agent policy follow-on` (inherits the OFF flag to flip per agent) + `### For skills (#111) / memory (#112)` (pattern set, different mechanism).
6. `## Revisit triggers` — 3–7 bullets (e.g., a provider rejecting the `{"type":"object"}` stub → re-open §D5.7; a consumer needing truly-omitted `parameters` → §D5.7 wire-type change; `tool_help` catalog snapshot proving too stale for dynamic tool sets → §D5.3; the 256-char limit proving too tight for a real tool → §D5.4).
7. `## Verification` — the close-gate commands + the golden-OFF byte-for-byte test + `cargo test` counts.

## §6 — Prior-chunk regression re-verification

| Upstream | Invariant relied on | Re-verification |
|---|---|---|
| KC-01..KC-03 (revert render path) | `stream_assistant_response` revert-mode context assembly unchanged; OFF/linear path byte-identical | `cargo test -j 4 --manifest-path .../Cargo.toml release_0_10 revert` green; golden-OFF byte-for-byte test |
| KC-04 (skill-prompt layout) | Skills catalog is prompt-text (orthogonal mechanism); not touched | no code overlap; `cargo test` skills tests green |
| Trait backcompat (all 12 kernel + 3 i-phi impls) | Adding trait methods must not break any impl | default methods only (`timeout()` precedent); `cargo build --all-targets` compiles all 12 impls unchanged |
| `tool_help` existing behaviour | Static-map path preserved | `tool_help.rs` inline tests (`returns_manual_for_a_named_tool`, `unknown_tool_lists_available_tools`, `missing_tool_name_is_invalid_args`, `consumer_supplied_map_is_used`) stay green |

**Carry-forward INVERSION (predicate-anchored, per KC-03 #4):** the `revert.rs` description split changes what `description()` returns, so the 3 existing tests asserting the CURRENT long description content MUST be re-targeted (their tree-model / four-categories / tool_help-reference assertions move to `detailed_description()` and/or the short one-liner):
- `revert.rs:218 description_teaches_tree_node_model_and_forward_continuation`
- `revert.rs:244 description_teaches_four_categories_as_budget_levers`
- `revert.rs:286 description_names_the_tool_help_doc_reference`

These 3 are enumerated by the superseded predicate (assertions on `description()` content), NOT by name shape. Extend the doc-sync sweep to test-module narrative comments in `revert.rs`.

**Carry-forward invariants verified green at chunk-open:**
- `cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml` baseline = **589**.
- `RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --manifest-path .../Cargo.toml --all-targets` clean.
- `cargo fmt --manifest-path .../Cargo.toml -- --check` clean.
- `git -C /root/projects/phi/phi-core diff HEAD -- src/` empty (no preload edits).

## §7 — Phases within the chunk

### §7.0 — Phase-order stress-test (v25 P-plan-4)

6 phases; F3×F4 coupled locks; no migration (phi-core is a library) → the compile/runtime-RED risk is bounded. Per-boundary check: the trait split (P1) MUST land before the reduced render (P2) consumes `short_description()`; the config field (P2) MUST land with the reduced branch in the SAME phase (else `streaming.rs` references a missing field). `tool_help` catalog (P3) is independent of the wire branch. The `revert` split (P4) inverts 3 tests → land the re-target in the SAME phase. **No RED window if P1→P2→P3→P4 land in order with `cargo build --all-targets` green-gated between each.** Golden-OFF test authored in P2 pins the OFF wire from the moment the branch exists.

### P0 — Pre-chunk gate + phi-core HEAD confirm
- **Goal**: verify §9 reading list read + carry-forward invariants green + baseline 589; confirm `dev` HEAD unchanged since P0 (`42c4cbb`).
- **Deliverables**: none (verification only).
- **Tests**: baseline `cargo test` = 589.
- **Confidence**: 100%.

### P1 — Trait split + registration validation primitive
- **Goal**: add the 2 default methods + the char-limit const + validation helper; all 12 impls compile unchanged.
- **Deliverables**: (1) `types/tool.rs` — `fn short_description(&self) -> &str { self.description() }` + `fn detailed_description(&self) -> Option<&str> { None }` default methods; (2) `SHORT_DESCRIPTION_MAX_CHARS: usize = 256` const + `validate_tool_registration(&dyn AgentTool) -> Result<(), ToolRegistrationError>` (F1.a hard-error default; F2.b consumer-overridable seam); (3) `ToolRegistrationError` enum.
- **Tests**: Tier A — defaults resolve (`short_description()` == `description()`; `detailed_description()` == `None`); an impl overriding neither renders identically. Tier E — validation fires per stance (valid passes / over-limit errors / missing short errors / missing detailed warns).
- **Concept-alignment**: transitions `concepts/tools.md` §"AgentTool Trait" row (in P-DOCS). **phi-core leverage**: kernel-minimality row 1 + 4.
- **Confidence**: ≥ 98%. **Pause discipline**: pause if any of the 12 impls fails to compile (would mean a non-default method slipped in).

### P2 — Config field + reduced-catalog render branch + golden-OFF pin
- **Goal**: add `progressive_tool_catalog` to `AgentLoopConfig`; gate the reduced branch at `streaming.rs:282-290`; pin the OFF wire.
- **Deliverables**: (1) `config.rs` — `ProgressiveToolCatalog { enabled, min_tools, engage_on_mcp }` + `Default` (OFF) + the field after `provider_wire_sink:382`; (2) thread the field to 3 production + 28 test literals (1 line each) + `BasicAgent::with_progressive_tool_catalog(...)` builder; (3) `streaming.rs` reduced branch: when engaged, `description = short_description()`, `parameters = json!({"type":"object"})` (F4.b); OFF path verbatim.
- **Tests**: Tier B — golden OFF byte-for-byte unchanged; progressive-ON reduced render asserts short description + stub `parameters` + measurable byte drop. Tier C — below-threshold full / above-threshold engaged / enabled-but-below inert. Tier G — reduced `parameters` serializes cleanly through `anthropic` + `openai_compat` (build_request_body).
- **phi-core leverage**: kernel-minimality rows 2 + 5; cascade discipline (31 sites). **Pause discipline**: pause if construction-site edits > 47 (1.5× 31).
- **Confidence**: ≥ 97%.

### P3 — `tool_help` catalog-backed schema source
- **Goal**: `tool_help(<name>)` returns `detailed_description()` + full `parameters_schema()` (incl. MCP) from a build-time catalog snapshot.
- **Deliverables**: (1) `tool_help.rs` — `ToolHelpTool::with_catalog(snapshot)` + snapshot type `BTreeMap<String,(Option<String>, serde_json::Value)>`; `execute()` prefers catalog entry, falls back to static map; (2) install the snapshot at `build_config`/`with_tool_help` once the tool Vec is folded (cycle-free — no `Arc` back-reference).
- **Tests**: Tier D — `tool_help(<name>)` returns detailed + full schema; an MCP-shaped `McpToolAdapter` yields its full `inputSchema`; static-map fallback still works.
- **Pause discipline**: pause if the snapshot install requires an `Arc` cycle (should not — use the snapshot).
- **Confidence**: ≥ 97%.

### P4 — `revert_to_state` description split + carry-forward test re-target
- **Goal**: shorten `revert.rs::description()` to a one-liner; move the manual to `detailed_description()`/`tool_help`; re-target the 3 inverted tests.
- **Deliverables**: (1) `revert.rs` — short `description()` (≤ 256 chars, ends "call tool_help(\"revert_to_state\") for the full manual") + `detailed_description()` returning the tree/node body; (2) re-target `description_teaches_*` tests (P0 §6 predicate-anchored) so the tree-model/four-categories assertions run against `detailed_description()`.
- **Tests**: Tier F — short/detailed split assertions; the 3 re-targeted tests green.
- **Confidence**: ≥ 98%.

### P-DOCS — Doc sync + verified-headers
- **Goal**: update §3.C docs + refresh verified-headers.
- **Deliverables**: `concepts/tools.md`, `architecture/algorithms/core/streaming.md`, `specs/developer/tool.md`, `reference/{configuration,api,glossary}.md` per §3.C; `[EXISTS]` tags; verified-headers bumped.
- **User-facing doc updates**: all §3.C rows.
- **Confidence**: ≥ 99%.

### P-ADR / P-SEAL — ADR-0005 Accepted + cycle-index row
- **Goal**: author ADR-0005 (7 sections), flip `Accepted`; insert the cycle-index row.
- **Deliverables**: ADR-0005; `_cycle-index.md` row (leave `Iterations = pending`, `Status = in-flight` — orchestrator owns transitions per row-lifecycle paragraph).
- **Confidence**: ≥ 99%.

## §8 — Tests summary

**Baseline (baseline-snapshot Axis A)**: 589 (post-KC-04; `_cycle-index.md` row `91a175a4`). phi-core has no binary/inline split — all tests run via `cargo test` (unit inline `#[cfg(test)]` + `tests/*.rs` integration).

**Per-Tier MUST-SHIP breakdown:**
| Tier | Coverage | Tests |
|---|---|---|
| A | trait split defaults + backcompat (no-override renders identically) | 3 |
| B | golden OFF byte-for-byte + progressive-ON reduced render + byte drop | 3 |
| C | threshold engage/inert (below full / above engaged / enabled-but-below inert) | 3 |
| D | `tool_help` detail+schema + MCP-shaped adapter + static-map fallback | 3 |
| E | registration validation (valid / over-limit / missing short / missing detailed warn) | 4 |
| F | `revert` short/detailed split (+ 3 re-targeted carry-forward, count-neutral) | 2 |
| G | reduced `parameters` serializes through anthropic + openai_compat | 2 |
| **Total NEW MUST-SHIP** | | **20** |

**Accept band**: `[589 + 20, 589 + 20 + MAY-COVER + inline-overshoot]` = **[609, 615]**. Predicted final ≈ **611**. MAY-COVER: incidental inline unit tests in touched modules (~2–5). Outside band → AskUserQuestion.

**Named test files / locations**: inline `#[cfg(test)]` in `types/tool.rs`, `agent_loop/streaming.rs` (or a new `tests/progressive_tool_catalog_test.rs` for Tiers B/C/G integration), `tools/tool_help.rs`, `tools/revert.rs`; provider fixtures in `anthropic.rs`/`openai_compat.rs` tests.

**Named expected-still-green (grep-verified against `revert.rs`/`tool_help.rs`)**: `revert.rs` `description_teaches_tree_node_model_and_forward_continuation` / `description_teaches_four_categories_as_budget_levers` / `description_names_the_tool_help_doc_reference` (RE-TARGETED, not deleted); `tool_help.rs` `returns_manual_for_a_named_tool` / `unknown_tool_lists_available_tools` / `missing_tool_name_is_invalid_args` / `consumer_supplied_map_is_used` (stay green — static-map path preserved).

## §9 — Pre-chunk gate

**Reading list (mandatory):**
1. Forward-scope `kc-05-progressive-tool-disclosure.md` (§0 pre-locks, §5 deliverables, §7 forks, §8 envelope).
2. Committed P0 `p0-investigation.md` (this cycle folder; §2 surface map, §3 findings, §4 fix-locus, §6 forks, §8 close-gate).
3. `src/types/tool.rs:243-288` (trait + `timeout()` precedent); `src/agent_loop/streaming.rs:199-290` (bridge); `src/provider/traits.rs:314-321` (ToolDefinition); `src/tools/tool_help.rs` (full); `src/tools/revert.rs:97-207` (description); `src/agent_loop/config.rs:200-383` (AgentLoopConfig); `src/provider/anthropic.rs:634-642` + `openai_compat.rs:624-634` (provider param serialization).
4. `[[feedback_phi_core_kernel_minimal]]`, `[[feedback_render_transcript_close_gate]]` Rules 4/8, `[[feedback_never_hedge]]`.
5. phi-core `CLAUDE.md` (Documentation Alignment + tool architecture).

**Carry-forward invariants (green at open)**: baseline 589; clippy `-Dwarnings` clean; `fmt --check` clean; `src/` diff vs HEAD empty.

**Pending decisions carried in**: confirm F-chunk-split + F-policy pre-locks; lock F1–F5 (F3+F4 as a coherent pair).

## §10 — Close criteria

**4 aspects (pass/fail):**
- **Code**: all 6 deliverables shipped; `cargo test -j 4 --manifest-path .../Cargo.toml` green at [609, 615]; `RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --all-targets` clean; `fmt --check` clean.
- **Docs**: every §3.C row updated in-chunk; verified-headers bumped; ADR-0005 `Accepted`; `[EXISTS]` tags current.
- **Kernel-minimality** (substitute for phi-core-leverage aspect): forbidden-leakage grep returns 0; only general primitives added; additive public API; no breaking/migration change; 12 impls compile unchanged.
- **Concept alignment**: every §2 row `honored` at close; none `contradicted`.

**2 confidence %:**
- **Implementation confidence** = claims-honored / claims-in-scope. **Target ≥ 9/10** (≈ 20/20 test-backed claims). The 1 non-blocking UNRESOLVED (large-MCP magnification) is a close-gate item, not an in-scope-claim gap.
- **Documentation confidence** = doc-pages cross-checkable / doc-pages touched. Target 6/6.

**Close-gate (model-facing → `phi-core-close-gate` skill, Rules 4/8):** boot the real daemon against a progressive-enabled agent with a many-tool / `N>10`-MCP set; render turn-1 `…request.json`; READ it and ASSERT THE DISPOSITION — reduced `parameters` (stub) on every entry + measurable byte drop vs a full baseline; `revert_to_state.description` is the SHORT one-liner (1870-char manual no longer eager); `tool_help(<name>)` serves the full `parameters_schema` (incl. an MCP tool's `inputSchema`); a model actually calls `tool_help` then the tool (loop closes). No-400/well-formed is necessary but NOT sufficient — assert the reduced disposition.

## §11 — Post-chunk independent audit plan

**Envelope (audit-envelope-size)**: 6 phases → **Large (3 auditors)**. Confirms forward-scope §8 hint.

- **Audit A — code + tests + kernel-minimality**: (1) 2 default methods at `tool.rs` compile all 12 impls unchanged; (2) reduced branch at `streaming.rs:282-290` gated by `progressive_tool_catalog`; OFF path byte-identical (golden test); (3) `config.rs` field additive, `Default` OFF; 31-site cascade landed; (4) validation primitive fires per F1.a/F2.b; (5) `tool_help` catalog-backed source (incl. MCP) + static fallback; (6) `revert` split + 3 re-targeted tests; (7) `cargo test` green at expected count; clippy `-Dwarnings` + `fmt` clean (mark `NOT-EXECUTED-IN-AUDIT` — orchestrator closes at gate-4); (8) forbidden-leakage grep = 0. ≤ 600 words.
- **Audit B — docs + ADR + verified-headers**: (1) ADR-0005 `Accepted` with all 7 sections + §D5.1–§D5.7 + pre-existing-behaviour notes; (2) `concepts/tools.md` + `streaming.md` + `developer/tool.md` + `reference/*` updated + `[EXISTS]` tags; (3) verified-headers bumped on every touched doc; (4) `_cycle-index.md` row present (`grep -n <hex>`); (5) glossary entry. ≤ 600 words.
- **Audit C — cross-cutting backcompat + provider serialization + bridge inheritance + wire disposition**: (1) all 12 kernel impls (+3 i-phi inherit) compile with no override needed; (2) F4.b stub serializes cleanly through `anthropic.rs:642` (`input_schema`) + `openai_compat.rs:634` (`parameters`); (3) `sub_agent`/`parallel`/`evaluation` inherit the reduced branch (no bespoke `ToolDefinition {` — grep); (4) OFF wire byte-identical (golden); (5) close-gate disposition read (reduced params + short revert + tool_help full schema + loop closes) — record as the model-facing verification. ≤ 600 words.

## §12 — Verification recipe

> Baseline for chunk-delta greps = the pre-chunk `dev` HEAD (`git diff HEAD`), NOT `main`.

```bash
# 1. Workspace health (phi-core: host cargo, single crate, NO --workspace, NO CI-guard scripts)
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# 2. Kernel-minimality: zero consumer-policy leakage in the new surface (expect 0)
grep -rniE "i-phi|iphi|per.agent.policy|daemon|assemble" /root/projects/phi/phi-core/src/tools/tool_help.rs /root/projects/phi/phi-core/src/types/tool.rs /root/projects/phi/phi-core/src/agent_loop/config.rs

# 3. No bespoke tool serializer outside the bridge (expect: struct def + 2 test fixtures only)
grep -rn "ToolDefinition {" /root/projects/phi/phi-core/src/

# 4. Backcompat: 12 kernel AgentTool impls compile (build all targets)
/root/rust-env/cargo/bin/cargo build -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets

# 5. Config cascade landed (expect struct def + 3 production + 28 test literals)
grep -rn "AgentLoopConfig {" /root/projects/phi/phi-core/src /root/projects/phi/phi-core/tests | wc -l

# 6. ADR + cycle-index
ls /root/projects/phi/phi-core/docs/decisions/0005-progressive-tool-disclosure.md
grep -n "<cycle-hex>" /root/projects/phi/phi-core/docs/specs/plan/build/_cycle-index.md

# 7. Live close-gate (phi-core-close-gate skill): render turn-1 tools[] on a real daemon
#    wire with a progressive-enabled N>10-tool/MCP agent; READ + assert reduced params +
#    short revert_to_state + tool_help full schema + model calls tool_help then the tool.
```

**Open questions (§12 close-gate items, not planning blockers):**
- P0 finding 9 — large-MCP (`N>10`) magnification is proven by-construction but has no live artifact. Close-gate step 7 is the named evidence: boot the daemon against an MCP server exposing `N>10` tools with progressive enabled + assert the reduced-`parameters` byte drop on the real wire.
