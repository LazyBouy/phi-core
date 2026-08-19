# P0 investigation — #109 + #110 progressive tool-catalog disclosure (locus-resolution)

Pre-scoping P0 for a not-yet-scoped chunk closing `LazyBouy/i-phi` **#109** (progressive tool-catalog
disclosure) + **#110** (tool-registration contract; corollary of #109). Both issues OPEN; both leave
locus to this P0. Cross-crate: phi-core kernel (`/root/projects/phi/phi-core`) + i-phi consumer
(`/root/projects/phi/i-phi`, branch `dev`). No production code changed; no commit; no issue mutated.

## §0 — Verdict summary

- **Fix-locus: (C) genuine split, KERNEL-PRIMARY.** The tool→provider `tools[]` serialization AND the
  `AgentTool` trait AND `tool_help` ALL live in phi-core. #110's short/detailed split is a trait-method
  change that **cannot** be made from a consumer (Rust coherence), and #109's reduced render happens at
  the kernel serializer (`streaming.rs:282-290`). The consumer residual is **thin**: flip a per-agent
  policy flag + choose validation strictness. MCP mapping is **automatic in the kernel** (i-phi wires MCP
  via phi-core's own `McpToolAdapter`). → a **KC-NN kernel chunk** (primitive) + a **thin i-phi MA-NN
  follow-on** (policy). #110 lands FIRST as the enabler for #109.
- **Findings: 8 DEFINITIVE, 1 UNRESOLVED (non-blocking).**
- **Forks surfaced for the planner: 6** (F-locus, F-policy, F-charlimit, F-contract-tier, F-chunk-split,
  F-backcompat).
- **Non-viable approaches ruled out: 2** (pure-consumer catalog reduction; mirroring skill_help's
  consumer pattern for tools).
- **Unresolved / needs-live-repro:** no captured LARGE-MCP-server (`N>10` tools) turn-1 wire exists; the
  "many MCP tools bloat the catalog" magnification is proven **by-construction** (code path) but not by a
  live artifact. Named evidence deferred to the close-gate (§8), NOT a planning blocker.

## §1 — Questions that gate planning

1. Where is the tool catalog serialized into the provider `tools[]` array — phi-core or i-phi? (The crux.)
2. Does the `AgentTool` trait match #110's claim (single `name()`/`description()`/`parameters_schema()`;
   no short/detailed split, no char-limit, no examples)? How many impls exist (backcompat blast radius)?
3. Is `tool_help` a kernel or consumer tool; what does it return TODAY, from what source; is it
   "redundant-on-top" as #109 claims?
4. How do `skill_help` + `memory_help` realize eager-catalog + lazy-body — and can that pattern be
   mirrored purely consumer-side for TOOLS?
5. Where do MCP `tools/list` results become catalog entries; can i-phi pre-strip an MCP tool's
   `parameters` before it reaches the kernel serializer (the "pure consumer-side fallback")?
6. Reproduce the gap: on a real rendered wire, is every tool's FULL `parameters` schema present in
   turn-1 `tools[]`?
7. Backcompat: if the trait gains methods, do existing impls break? Is there a default-method path?

## §2 — Current-surface map (file:line)

**Tool set → provider `tools[]` serialization owner — KERNEL.**
- Registry: `AgentContext.tools: Vec<Arc<dyn AgentTool>>` — `phi-core/src/types/context.rs:218`
  ("REGISTRY … converted to ToolDefinition for LLM").
- **The bridge (crux):** `phi-core/src/agent_loop/streaming.rs:282-290`, inside
  `stream_assistant_response(context: &AgentContext, config: &AgentLoopConfig, …)`
  (`streaming.rs:199-201`). It maps EVERY `context.tools` entry to
  `ToolDefinition { name: t.name(), description: t.description(), parameters: t.parameters_schema() }`
  — **unconditionally the FULL schema**, no reduction, no config gate.
- Wire type: `ToolDefinition { name, description, parameters: serde_json::Value }` —
  `phi-core/src/provider/traits.rs:314-321` ("the JSON schema that gets SENT TO THE LLM … only
  ToolDefinition goes to the API"). Each provider serializes it into its native array, e.g.
  `openai_compat.rs:880`, `anthropic.rs:884`.
- **Consumer (i-phi) owns NONE of this serialization.** i-phi only *populates* the tool Vec (below).

**`AgentTool` trait — KERNEL, single-description, 15 impls.**
- Defn: `phi-core/src/types/tool.rs:243-288`. Methods: `name()` (`:245`), `label()` (`:247`),
  `description()` (`:249`), `parameters_schema()` (`:251`), `execute()` (`:269`), and a **default-method**
  `timeout() -> Option<Duration> { None }` (`:285-287`). **No** `short_description`, **no**
  `detailed_description`, **no** char-limit, **no** examples field — confirms #110 verbatim.
- Impls (blast radius): **12 kernel** — `bash.rs:126`, `edit.rs:48`, `file.rs:106/256`, `list.rs:48`,
  `search.rs:59`, `revert.rs:98`, `prun.rs:137`, `tool_help.rs:67`, `agents/sub_agent.rs:156`,
  `openapi/adapter.rs:133`, `mcp/tool_adapter.rs:106`; **3 i-phi** — `custom_tool.rs:69`
  (`ShellCommandTool`), `help_tools.rs:70` (`SkillHelpTool`), `help_tools.rs:194` (`MemoryHelpTool`).

**`tool_help` — KERNEL tool, static string map, NOT a schema source.**
- `phi-core/src/tools/tool_help.rs`. `ToolHelpTool` holds `help: BTreeMap<String,String>` (`:39`) —
  a hand-written `tool_name → manual` map. `with_default_help()` (`:50-56`) seeds ONLY
  `revert_to_state`, `prun`, `prun_with_memo` from the `REVERT_HELP`/`PRUN_HELP` string consts
  (`:129`, `:173`). `execute()` (`:93-124`) returns `help.get(name)` or a "No extended manual is
  registered" miss. It **never reads any tool's `parameters_schema()`** and is decoupled from the
  catalog. `default_tools()` includes it (`tools/mod.rs:82`); `BasicAgent::with_tool_help()` installs
  `with_default_help()` (`basic_agent.rs:794-796`).

**skill_help / memory_help — the eager-catalog+lazy-body pattern, but via PROMPT TEXT.**
- Skills catalog renders as a **system-prompt block** `<available_skills>…</available_skills>`
  (`phi-core/src/context/skills.rs:282-296`; installed via `BasicAgent::with_skills_format`,
  `basic_agent.rs:514-518`). `skill_help(skill_name)` reads the SKILL.md body off disk
  (`i-phi/src/agent_factory/help_tools.rs:99-143`).
- Memory catalog renders as a short-term index block in the prompt; `memory_help(pointer)` expands one
  record (`i-phi/src/agent_factory/help_tools.rs:150-264`; index render doc at
  `memory_wire.rs:47`).
- **Architectural asymmetry (load-bearing):** skills/memory catalogs are TEXT the consumer/kernel
  prompt-strategy renders, so their `*_help` tools can live consumer-side. **Tools have no prompt-text
  catalog** — the model sees them via the native function-calling `tools[]` array owned exclusively by
  the kernel serializer (`streaming.rs:282-290`). So the tool case cannot mirror skill_help
  consumer-side (see §5, non-viable #2).

**MCP ingestion — i-phi wires via phi-core's own adapter.**
- `i-phi/src/agent_factory/builder.rs:863-904` connects with `phi_core::mcp::McpClient::connect_stdio/http`
  and wraps results with **`phi_core::mcp::McpToolAdapter::from_client`** (`:904`). So each MCP tool
  becomes a phi-core `McpToolAdapter` (kernel type). Its `description()` returns the MCP `description`
  (`mcp/tool_adapter.rs:117-122`); its `parameters_schema()` returns the full MCP `input_schema`
  (`:124-130`). Both sit in the kernel → the `description`→short / `inputSchema`→detailed mapping #110
  wants is **automatic** once the kernel renders `short_description()` (default `description()`) and
  serves `parameters_schema()` via `tool_help`.

**i-phi tool assembly (consumer wiring).** `builder.rs:550-648` builds the agent:
`.with_revert_tool().with_tool_help().with_skills_format(...)`, folds `default_tools()` + operator
custom tools (`assemble.rs`) + the help family, permission-filters the operator catalog
(`filter_tools_by_permissions`, `:609-612`), dedups by name KEEP-FIRST (`:646-647`). None of this
reduces `parameters` — it only chooses WHICH tools enter the Vec the kernel then serializes in full.

## §3 — Findings (each DEFINITIVE or UNRESOLVED)

1. **The `tools[]` serializer is phi-core kernel-owned.** DEFINITIVE — `streaming.rs:282-290` builds
   `ToolDefinition` from `t.parameters_schema()` for every tool, unconditionally full; `ToolDefinition`
   (`traits.rs:314-321`) is what each provider sends. i-phi never touches this. A consumer cannot reduce
   the array without either changing what its `AgentTool` impls return or a kernel hook.

2. **#110's trait claim is exactly correct; 15 impls; default-method backcompat path exists.** DEFINITIVE
   — trait `tool.rs:243-288` has single `description()` + `parameters_schema()`, no split/limit/examples.
   Adding `fn short_description(&self)->&str { self.description() }` + `fn detailed_description(&self)->
   Option<&str> { None }` as **default methods** keeps all 12 kernel + 3 i-phi + MCP/OpenApi/SubAgent
   impls compiling unchanged — the trait already ships a default method (`timeout()`, `:285-287`) as
   precedent.

3. **`tool_help` is a kernel tool serving a static hand-written string map — NOT the catalog schema;
   "redundant-on-top" is confirmed.** DEFINITIVE — `tool_help.rs:39/50-56/93-124`. Today `tool_help`
   returns `REVERT_HELP`/`PRUN_HELP` for 3 names and a miss for everything else (bash, MCP tools,
   skill_help, …). It does not read `parameters_schema()`. For `revert_to_state`, the FULL manual is
   ALSO in `description()` (see finding 6), so `tool_help(revert_to_state)` duplicates in-catalog
   content — the exact redundancy #109 names. There is **no** reduced-catalog mode in `tool_help` that
   i-phi is failing to use.

4. **skill_help/memory_help realize eager-catalog+lazy-body via prompt TEXT; tools cannot mirror it
   consumer-side.** DEFINITIVE — skills catalog is a prompt block (`skills.rs:282-296`), memory an index
   block; the tool catalog is the native `tools[]` array (`streaming.rs:282-290`) with no prompt-text
   equivalent. Mirroring the consumer pattern for tools would require the consumer to wrap every kernel
   tool to fake a reduced array (§5 #2).

5. **MCP tools enter as phi-core `McpToolAdapter`s; the #110 mapping is automatic in the kernel.**
   DEFINITIVE — `builder.rs:904` → `McpToolAdapter::from_client`; adapter `description()`=MCP short,
   `parameters_schema()`=full `input_schema` (`tool_adapter.rs:117-130`). i-phi needs **no** MCP-specific
   short/detailed code if the kernel renders `short_description()` and serves `parameters_schema()` via
   `tool_help`.

6. **GAP REPRODUCED: every tool's FULL `parameters` schema is on the turn-1 wire.** DEFINITIVE — read
   two captured artifacts (§5). In `…/ma01-ma04-4agent/coder.request.json` (10 tools, `tools[]` = 7759
   bytes): `revert_to_state` carries a 1870-char `description` + 1010-byte `parameters` (≈2.9 KB, ~37% of
   the whole array) — the manual is eager despite `tool_help` existing. Every tool (bash/read_file/…/MCP-
   shaped) carries a full `parameters` object. `…/ma-04-0df8734b/agent-a.turn-1.request.json` (4 tools,
   4599 bytes) shows the same. Confirms #109's core claim on real wire.

7. **No progressive / reduced-catalog / short-description prior art exists in either tree.** DEFINITIVE —
   grep for `progressive|reduced.catalog|short_desc|detailed_desc|lazy.schema|catalog_mode` hits only
   the skills/memory/compaction progressive-disclosure docs, never the `tools[]` array. The tool catalog
   is unconditionally full today.

8. **Backcompat is safe via default methods.** DEFINITIVE — see finding 2; `timeout()` precedent.

9. **The MCP many-tool magnification is proven by-construction, not by a live large-MCP artifact.**
   UNRESOLVED — needs a live turn-1 wire from a daemon booted against an MCP server exposing `N>10`
   tools. No such capture exists under `docs/tmp/livegate-logs/` or `docs/e2e-test/` (the captures have
   0 MCP tools). The code path guarantees it (`McpToolAdapter.parameters_schema()` full inputSchema →
   `streaming.rs` serializes each in full), so this does NOT block planning; it is the natural close-gate
   (§8).

## §4 — Fix-locus determination (A / B / C) with kernel-minimal reasoning

Per `[[feedback_phi_core_kernel_minimal]]`: default to fixing in the consumer UNLESS the change is a
genuinely-general kernel feature carrying zero consumer-specific logic. Per-change carve-out test:

| Change | Side | Carve-out rationale | Blast radius |
|---|---|---|---|
| `AgentTool::short_description()` + `detailed_description()` **default methods** | **KERNEL** | You cannot add methods to a phi-core trait from a downstream crate (Rust coherence) — #110's contract is structurally impossible consumer-side. Genuinely general: every phi-core consumer with many/large tools benefits; zero i-phi logic. | All 15 impls keep compiling (defaults); opt-in richer bodies per tool. |
| Reduced-catalog **render** gated by a config flag at `streaming.rs:282-290` | **KERNEL** | The bridge lives only here; a consumer cannot intercept it. General. | Touches 1 kernel fn; providers unchanged (still get a `ToolDefinition`, just leaner `parameters`). Verify `sub_agent.rs`/`parallel.rs`/`evaluation.rs` unaffected (they reuse the same bridge → inherit the flag; no bespoke serialization). |
| `tool_help` serves `detailed_description()` + full `parameters_schema()` on demand | **KERNEL** | `tool_help` is a kernel tool; needs access to the live catalog to return full schema. General. | Extends `ToolHelpTool` (kernel); i-phi's `.with_tool_help()` inherits it. |
| Char-limit **primitive** + validation (missing name/short = error; over-limit = error/truncate-warn; missing detailed = warn) | **KERNEL primitive + CONSUMER policy** | A general const + validation helper is kernel-general; the STANCE (hard-error vs truncate-warn) is a policy the consumer may set. | Forces `revert_to_state.description()` (1870 chars) to be split into a short line + detailed body (desirable; that IS the fix). Touches `revert.rs`. |
| `progressive_tool_catalog` flag / threshold on `AgentLoopConfig` | **KERNEL primitive + CONSUMER policy** | Kernel exposes the flag; i-phi decides WHEN to flip it (daemon/agent config). | Additive config field. |
| MCP `description`→short, `inputSchema`→detailed mapping | **AUTOMATIC in KERNEL** | `McpToolAdapter` is a kernel type; its `description()`/`parameters_schema()` already carry the right split. | Zero new i-phi MCP code. |
| Per-agent policy (which agents progressive; threshold value; validation strictness) | **CONSUMER (i-phi)** | i-phi owns daemon/agent config (`AgentFactoryDeps`/`assemble.rs`), mirroring the MA-cluster `<x>_override: Option<T>` per-agent pattern. | Thin — flip a flag per agent. |

**Why not (A) pure kernel:** the POLICY (threshold / per-agent opt-in / validation strictness) and the
MCP-server attachment are i-phi's; the kernel must expose them as knobs, not hard-code them.

**Why not (B) pure consumer:** DEFINITIVE-blocked on two counts — (i) #110's trait split cannot be added
to a foreign trait from a consumer (coherence); (ii) the reduced render + `tool_help` lazy-source both
live in the kernel. A consumer *could* fake reduction by wrapping every tool (§5 #1), but that is a hack,
not the author-declared contract, and it fragments the help family (issue-flagged) while re-wrapping
phi-core's own `default_tools()` and `McpToolAdapter`.

**Verdict: (C) split, KERNEL-PRIMARY.** Kernel change is mandatory + load-bearing (trait + serializer +
`tool_help` + config flag + `revert.rs` description split); the i-phi consumer change is thin (policy
flag per-agent + strictness choice). Minimal general kernel surface to add: (1) two default trait
methods; (2) one `AgentLoopConfig` flag (+ optional threshold); (3) reduced branch in the
`streaming.rs` bridge; (4) `ToolHelpTool` gains a catalog-backed schema source. Nothing i-phi-specific
leaks into the kernel.

## §5 — Gap reproduction

**What I read (no daemon boot needed — captured wire existed).** Parsed two live close-gate captures
with `python3 -c "import json"`:
- `/root/projects/phi/i-phi/docs/tmp/livegate-logs/ma01-ma04-4agent/coder.request.json` — 10 tools,
  `tools[]` = **7759 bytes**. Per-tool `(description_chars, parameters_bytes)`: `revert_to_state`
  **(1870, 1010)**, `tool_help` (247,189), `skill_help` (206,217), `memory_help` (210,221), `bash`
  (122,136), `read_file` (159,305), `write_file` (127,210), `edit_file` (196,330), `list_files`
  (126,318), `search` (127,458). Every tool carries a full `parameters` object; `revert_to_state` alone
  is ~37% of the array and its full manual is eager despite `tool_help`.
- `/root/projects/phi/i-phi/docs/tmp/livegate-logs/ma-04-0df8734b/agent-a.turn-1.request.json` — 4 tools,
  4599 bytes; the `revert_to_state` `function.description` is the entire tree/node manual verbatim, and
  every tool has a full `parameters` schema.

**Quoted (agent-a, tool #2):**
```json
{"function":{"description":"Fetch the full manual for another tool on demand … Pass the tool's exact name in `tool_name`.",
 "name":"tool_help",
 "parameters":{"properties":{"tool_name":{"type":"string", …}},"required":["tool_name"],"type":"object"}},
 "type":"function"}
```
`tool_help` ships with a full `parameters` schema AND sits next to `revert_to_state` whose full manual is
already loaded — the "redundant-on-top" state #109 describes, on real wire.

**Not reproduced (see finding 9):** a large-MCP (`N>10`) capture — no such artifact exists; deferred to
the §8 close-gate.

## §6 — Design forks surfaced (for the planner — NOT locked)

- **F-locus** — (A) kernel / (B) consumer / (C) split. Evidence points to **(C) kernel-primary** (§4):
  trait + serializer + `tool_help` are kernel; consumer role is a thin policy flag. Cost: a kernel chunk
  (KC-NN) touching trait + `streaming.rs` + `tool_help.rs` + `revert.rs` + config; enables a thin i-phi
  MA-NN. Pure-consumer (B) is structurally blocked (§5 #1/#2).

- **F-policy** — always-on / threshold (progressive above N tools or when MCP attached) / per-agent
  config knob. What the code makes cheap: a bool flag on `AgentLoopConfig` is trivial; the per-agent knob
  mirrors i-phi's proven `<x>_override: Option<T>` MA-cluster pattern (cheap). **Always-on changes EVERY
  agent's wire** — including the 4/10-tool close-gate captures — forcing every wire fixture to be
  re-blessed; threshold/opt-in bounds the blast radius. A tool-count/MCP-attached threshold is a small
  kernel helper.

- **F-charlimit** — `short_description` limit value + overflow behavior. Datapoint: current kernel
  built-in descriptions are 122–196 chars; `revert_to_state` is **1870**. A limit ~120–200 leaves the
  built-ins ~compliant (edit_file 196 is at the ceiling) but **forces `revert_to_state`'s description to
  be split** — which is exactly the desired outcome (its manual moves to `detailed`/`tool_help`).
  Overflow behavior fork: hard-error at registration (forces the split as a build-break — clean but
  makes the kernel chunk MUST-edit `revert.rs`) vs truncate-with-warn (lenient; leniency matches the
  domain-conditional trend for teaching-material text, strictness matches load-bearing config).

- **F-contract-tier** — enforce #110 at the kernel trait level or the consumer registration level.
  Evidence: the SPLIT FIELDS must be kernel (trait); the ENFORCEMENT STANCE can be a consumer policy.
  MCP tools (no author-declared short/detailed) are satisfied automatically (map `description`→short via
  the default method; synthesize detailed from the full `inputSchema` served by `tool_help`) because
  `McpToolAdapter` is a kernel type (§3 finding 5).

- **F-chunk-split** — ONE chunk vs split. Dependency: **#110 enables #109** (the reduced catalog renders
  the trait's `short` field; `tool_help` serves the trait's `detailed` field). Both halves are kernel and
  tightly coupled (trait + render + `tool_help` change together). Option 1: one KC-NN kernel chunk
  (#110+#109 mechanism) + a thin i-phi MA-NN policy follow-on. Option 2: KC-primitive chunk (#110 trait
  contract only) → KC/i-phi render chunk (#109) → i-phi policy chunk. The trade-off: coupling favors one
  kernel chunk; the coupled-fork bundle discipline (F-locus × F-policy) suggests presenting the
  kernel-primitive + consumer-policy as a coherent bundle.

- **F-backcompat** — DEFINITIVE resolved (§3 finding 2/8): default trait methods (`short_description()`
  → `description()`, `detailed_description()` → `None`), `timeout()` precedent. No existing impl breaks.
  The only forced edit is `revert.rs` IF the char-limit is hard-enforced (F-charlimit).

## §7 — Open questions / non-blocking unresolveds

- **[Close-gate, not a planning blocker]** Large-MCP magnification unproven by artifact (§3 finding 9).
  Named evidence: boot the daemon against an MCP server exposing `N>10` tools with `progressive` enabled,
  capture turn-1 `tools[]`, assert reduced `parameters`.
- **[Planner to weigh]** Whether `tool_help` should return `detailed_description` + full
  `parameters_schema` (both) or only the detailed manual (the model already knows the tool exists; the
  full JSON-Schema may still be wanted for correct arg formatting). Provider validation of args against
  schema is advisory only — the schema is model-guidance, so omitting it up-front is safe; but some
  providers may reject calls to a tool whose `parameters` is empty/minimal. Confirm at plan-time whether
  a reduced catalog needs a minimal-but-valid `parameters` stub (e.g. `{"type":"object"}`) vs a fully
  omitted one, per each provider's `tools[]` acceptance.
- **[Sibling scope]** #110 explicitly notes the same structured-frontmatter contract should apply to
  skills (#111) and memory (#112). Those are consumer-rendered prompt blocks (different mechanism, §2) —
  out of scope for this chunk but the planner should note the kernel trait change here sets the pattern.

## §8 — Recommended close-gate

Per `[[feedback_render_transcript_close_gate]]` Rule 8 (the fix manifests on the **wire** — model-facing
`tools[]` shape): boot the real daemon (`docker-iphi.sh`) with a progressive-enabled agent that has an
MCP server exposing `N>10` tools (or a many-tool set), render turn-1 `…turn-1.request.json`, and READ it:
1. Each tool's catalog entry carries **name + short_description only**; `parameters` is reduced/omitted
   (or a minimal valid stub) — assert bytes drop vs a baseline full capture.
2. `revert_to_state`'s `description` is now the SHORT one-liner (its 1870-char manual is no longer eager).
3. `tool_help(<name>)` returns the tool's **full `parameters_schema` + detailed body** — including for an
   MCP tool (its full `inputSchema`).
4. A representative model actually calls `tool_help` for a tool it needs and then calls that tool
   successfully (progressive loop closes end-to-end).
5. Registration validation fires: a tool with a missing/over-limit `short_description` errors/warns per
   the locked F-charlimit stance.
