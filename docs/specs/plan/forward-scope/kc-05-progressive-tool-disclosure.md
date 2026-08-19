<!-- Last verified: 2026-08-19 by Claude Code -->

# KC-05 — Progressive tool-catalog disclosure + tool-registration contract (kernel mechanism)

> Fifth phi-core kernel chunk. Closes the **kernel-primary** half of `LazyBouy/i-phi` **#110** (tool-registration contract: short/detailed split) + **#109** (progressive tool-catalog disclosure). Grounded by the committed P0 (`docs/specs/plan/build/_p0-investigations/109-110-progressive-tool-disclosure-p0.md`, `42c4cbb`): fix-locus **(C) split, KERNEL-PRIMARY** — the `tools[]` serializer, the `AgentTool` trait, and `tool_help` all live in phi-core; the i-phi consumer residual is a thin per-agent policy flag deferred to a later i-phi follow-on. **#110 lands first as the enabler for #109.** Back-compat by construction: the progressive path is **opt-in / threshold-gated OFF by default**, so every existing wire is byte-unchanged until a consumer flips the flag.

## §0 — Pre-locked forks (from the earlier scoping session — confirm at gate-1)

Two forks the user locked when this chunk was first scoped (pre-compaction); carried here for confirmation, not re-litigation:

- **F-chunk-split → kernel-chunk-first (P0 §6 Option 1).** KC-05 = the ONE coupled kernel chunk (#110 trait contract + #109 reduced render + `tool_help` schema source, which change together). The i-phi per-agent **policy** flag ships as a separate thin consumer follow-on later. Rationale: the three kernel pieces are tightly coupled; the consumer role is only a knob.
- **F-policy → threshold-gated, NOT always-on (P0 §6 F-policy).** The reduced catalog engages only above a tool-count threshold (or when explicitly enabled) — never unconditionally. Rationale: always-on rewrites every agent's wire (including the small 4/10-tool close-gate fixtures) and forces every fixture re-bless; threshold/opt-in bounds the blast radius and keeps small agents byte-unchanged.

## §1 — Purpose

Give phi-core a **progressive tool-catalog** primitive so an agent with many/large tools sends the model a **lean turn-1 `tools[]`** (name + short description; reduced/omitted `parameters`) and lets the model fetch a tool's **full schema + detailed manual on demand** via `tool_help` — the same eager-catalog + lazy-body shape skills/memory already have, but for the native function-calling array the kernel owns exclusively. Per `[[feedback_phi_core_kernel_minimal]]`: phi-core owns the *mechanism* (trait split + reduced render + on-demand schema source + config primitive); the consumer (i-phi, later) owns the *policy* (which agents, what threshold, validation strictness).

## §2 — Inputs consumed

- **GitHub #110** — tool-registration contract (short/detailed description split on the tool trait; the enabler). **GitHub #109** — progressive tool-catalog disclosure (reduce the eager catalog; `tool_help` serves the detail). Both OPEN; both leave locus to the P0.
- **The committed P0** (`_p0-investigations/109-110-progressive-tool-disclosure-p0.md`) — §2 surface map, §3 findings (8 DEFINITIVE + 1 non-blocking UNRESOLVED), §4 fix-locus table, §6 the 6 surfaced forks, §8 the close-gate.
- **Kernel surface (current `dev`)**:
  - `AgentTool` trait — `src/types/tool.rs:243-288` (single `description()`/`parameters_schema()`; `timeout()` default-method precedent at `:285-287`).
  - The serializer bridge (crux) — `src/agent_loop/streaming.rs:282-290` (maps every tool → `ToolDefinition{name, description, parameters: t.parameters_schema()}`, unconditionally full).
  - `ToolDefinition` wire type — `src/provider/traits.rs:314-321`.
  - `ToolHelpTool` — `src/tools/tool_help.rs` (static hand-written `BTreeMap<String,String>`; does NOT read any `parameters_schema()` today).
  - `revert_to_state.description()` — `src/tools/revert.rs` (the 1870-char eager manual the char-limit forces to split).
  - MCP — `McpToolAdapter` (`src/mcp/tool_adapter.rs:106-130`) is a kernel type: `description()`=MCP short, `parameters_schema()`=full `inputSchema` → the #110 mapping is **automatic**, zero new MCP code.
- **Memories**: `[[feedback_phi_core_kernel_minimal]]` (mechanism-only, no i-phi leakage), `[[feedback_render_transcript_close_gate]]` Rule 4 + Rule 8 (model-facing → render + READ the actual turn-1 wire; assert the DISPOSITION, not just no-400), `[[feedback_never_hedge]]`.

## §3 — Issues / drifts closed

- **#110** (tool-registration contract) — the kernel trait split. Primary enabler.
- **#109** (progressive tool-catalog disclosure) — the kernel reduced render + `tool_help` schema source.
- **Not closed here** (consumer follow-on): the i-phi per-agent policy flag / threshold config surface — filed as a new i-phi drift/chunk. Sibling contracts #111 (skills) / #112 (memory) are a different (prompt-text) mechanism — out of scope; the P0 §7 notes this trait change sets the pattern.
- Any NEW defect the work surfaces is filed as its own D-TEST drift (GitHub-mirrored).

## §4 — Prerequisites

- KC-01/02/03/04 landed on `dev`. ✓ (no code dependency — independent surface.)
- P0 committed + gate-0.5 verified. ✓
- None blocking. Unblocks the thin i-phi policy follow-on.

## §5 — Deliverables

1. **Trait split (#110)** — `AgentTool` gains `short_description(&self) -> &str { self.description() }` + `detailed_description(&self) -> Option<&str> { None }` as **default methods** (backcompat: all 12 kernel + 3 i-phi + MCP/OpenApi/SubAgent impls compile unchanged; `timeout()` precedent). Tools MAY override to supply a real split.
2. **Reduced-catalog render (#109)** — a config-gated branch at `streaming.rs:282-290`: when progressive is engaged, `ToolDefinition.description` = `short_description()` and `parameters` = the reduced form (F4 decides omit vs minimal-valid stub). Default path = today's full render, byte-for-byte.
3. **`AgentLoopConfig` progressive primitive** — an additive flag (+ threshold value per F5) that engages the reduced branch. **Defaults OFF / below-threshold inert** (back-compat). Inherited automatically by `sub_agent`/`parallel`/`evaluation` (they reuse the same bridge).
4. **`ToolHelpTool` catalog-backed schema source (#109)** — on `tool_help(<name>)`, return the tool's `detailed_description()` + (per F3) its full `parameters_schema()`, sourced from the live catalog — including MCP tools' full `inputSchema`. Removes the "redundant-on-top" static-map duplication for the progressive case.
5. **Registration validation primitive (#110)** — a char-limit const + a validation helper (missing name/short → error; over-limit → per F1 stance; missing detailed → warn). Kernel exposes the primitive; the STANCE is a knob the consumer may set (F2).
6. **`revert_to_state` description split** — split the 1870-char manual into a short one-liner (`description()`/`short_description()`) + the detailed body (`detailed_description()` / `tool_help`). This IS the fix; forced by any hard char-limit.
7. **Tests** — (a) golden: default (progressive OFF) turn-1 `tools[]` byte-for-byte unchanged; (b) progressive-ON reduced render asserts short description + reduced `parameters` + measurable byte drop; (c) `tool_help(<name>)` returns detailed + full schema (incl. an MCP-shaped adapter); (d) backcompat: an impl that overrides neither default method renders identically; (e) threshold: below-threshold agent stays full, above-threshold engages; (f) registration validation fires per the locked stance; (g) `revert_to_state` short/detailed split assertions.
8. **Docs** — rustdoc on the new trait methods + config flag + `tool_help` behavior; phi-core **ADR-0005** (progressive tool-disclosure mechanism + kernel-primary locus + back-compat-by-default rationale + the F-lock outcomes); update any concept/architecture doc that documents the tool catalog / `tool_help`. Verified-headers refreshed.

## §6 — Acceptance criteria

- Progressive **OFF** (default): turn-1 `tools[]` is byte-identical to today (golden test pins it); every existing test green.
- Progressive **ON** (above threshold): each catalog entry carries name + short description; `parameters` reduced/omitted per F4; measurable byte drop vs the full baseline; `revert_to_state`'s 1870-char manual no longer eager.
- `tool_help(<name>)` returns the tool's detailed body + (F3) full `parameters_schema`, including for an MCP tool.
- Backcompat: all 15 existing `AgentTool` impls compile + behave unchanged with no override.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check`.
- Kernel-minimality: only general primitives added (2 default trait methods, 1 config flag+threshold, a reduced render branch, a catalog-backed `tool_help`, a validation helper); zero i-phi/consumer policy logic leaks in; additive public API; no persisted-field / migration / breaking change.
- **Live close-gate (P0 §8, Rule 8)**: boot the real daemon against a progressive-enabled agent with a many-tool / `N>10`-MCP set; render turn-1 `…request.json`; READ it and assert reduced `parameters`, short `revert_to_state`, `tool_help` serving full schema, and a model actually calling `tool_help` then the tool (loop closes) — via the `phi-core-close-gate` skill.

## §7 — Forks for the planner

Beyond the two §0 pre-locks (F-chunk-split, F-policy), these remain to lock at gate-1:

- **F1 — char-limit overflow stance**: (a) **hard-error at registration** (over-limit `short_description` fails the build/boot — clean, forces the `revert.rs` split as a must-edit) vs (b) **truncate-with-warn** (lenient; matches teaching-material leniency, but a silently-truncated description is model-facing). Datapoint: built-ins are 122–196 chars; `revert_to_state` is 1870. A limit ~160–256 leaves built-ins compliant and forces only the `revert` split.
- **F2 — validation-stance locus**: kernel hard-codes the stance vs kernel exposes the const/helper and the **stance is a consumer knob** (lean: primitive in kernel, stance defaultable + consumer-overridable, per kernel-minimality).
- **F3 — `tool_help` return shape**: (a) detailed manual **only** vs (b) detailed **+ full `parameters_schema`**. The model already knows the tool exists; the full JSON-Schema aids correct arg formatting. Lean (b) so a reduced catalog loses no arg-formatting fidelity (the model fetches the schema when it commits to the tool).
- **F4 — reduced-`parameters` shape on the wire**: (a) **omit** `parameters` entirely vs (b) **minimal-valid stub** (`{"type":"object"}`). P0 §7 flags provider `tools[]` acceptance — some providers may reject a tool with empty/absent `parameters`. Lean (b) unless plan-time provider check proves omission is universally accepted; the close-gate confirms on the real wire.
- **F5 — threshold shape + default**: the tool-count threshold value (e.g. engage above N tools) and/or an MCP-attached trigger; confirm the kernel flag **defaults progressive-OFF** (below-threshold inert) so back-compat holds with zero consumer action.

## §8 — Audit envelope hint

**Large (3 auditors)** — lean Large: A = trait split + reduced render + `tool_help` + config correctness + tests; B = docs + ADR-0005 + rustdoc + verified-headers + concept/architecture doc sync; C = cross-cutting (15-impl backcompat blast radius, provider `tools[]` serialization + acceptance, `sub_agent`/`parallel`/`evaluation` bridge inheritance, model-facing wire disposition). Confirm via `audit-envelope-size` at gate-1; medium acceptable if the reduced render lands tightly bounded.

## §9 — Unblocks

- **i-phi per-agent policy follow-on** — the thin consumer chunk that flips the kernel flag per agent (mirrors the MA-cluster `<x>_override: Option<T>` pattern) + sets validation strictness. Runs once i-phi tracks the new phi-core commit.
- Any future consumer (baby-phi, others) gets progressive tool disclosure for free.
- Sets the structured-contract pattern #110 wants extended to skills (#111) / memory (#112) later.

## §10 — Risks

- **Default-drift** — the one real hazard is silently changing the OFF-default wire. The golden byte-for-byte test (progressive OFF) is the guard; the reduced branch must be unreachable unless the flag+threshold engage.
- **Provider rejection** — an omitted/empty `parameters` could be rejected by some providers (F4). Guard: plan-time provider-acceptance check + the live close-gate on a real wire.
- **Trait backcompat** — a non-default method would break the 15 impls. Guard: default methods only (mirrors `timeout()`).
- **Kernel-creep** — resist adding an i-phi-flavored policy struct here; phi-core ships the mechanism + a bare flag/threshold only. Policy is the i-phi follow-on.

## §11 — Direct-approval criteria fit

Does **NOT** auto-approve: several forks lock at gate-1 (F1 overflow stance + F3 `tool_help` shape + F4 wire-`parameters` + F5 threshold are non-trivial and model-facing), the chunk adds public API, changes a model-facing wire, and touches a load-bearing kernel bridge (Large audit envelope). Investigation done (P0 committed + gate-0.5-verified). Approval gate is `approval=yes` with the pre-locks confirmed + remaining forks presented via the 4-line AskUserQuestion template (F1/F3/F4/F5 have user-visible/model-facing deltas → full template; F2 is a TECHNICAL fork → 2-line).
