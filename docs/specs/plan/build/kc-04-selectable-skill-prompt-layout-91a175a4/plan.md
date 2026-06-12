<!-- cycle hex: 91a175a4. project=phi-core (kernel lane). Archived 2026-06-12 (iter-2; F2=F2.b USER-DIVERGENT). -->

# KC-04 — Selectable skill-prompt layout (XML default + opt-in YAML) — cycle plan

> Fourth phi-core kernel chunk. Closes GitHub **#78 / D-TEST-0073 (S3)** — the phi-core half of a coupled kernel→consumer pair (i-phi config knob **#79 / D-TEST-0074** depends on this landing first, OUT of scope here). **Purely additive, back-compat**: XML stays byte-for-byte the default; YAML is an opt-in lighter layout. Forward-scope: `/root/projects/phi/phi-core/docs/specs/plan/forward-scope/kc-04-selectable-skill-prompt-layout.md`.
>
> **Verified-header / iteration banner**: **iter-2 re-author** (gate-1 locks in: F1 = F1.a planner-rec; **F2 = F2.b USER-DIVERGENT** — user chose conditional bare-vs-quoted YAML scalar escaping over the planner-rec F2.a always-quote). Only F2-dependent content re-authored (§1 F2, §5 §D4.3, §6 close-gate, §8 Tier C, §3.E/§12 helper rename `yaml_escape_quoted` → `yaml_escape_scalar`, Forks table F2 row). F1.a body + §2 + §3.B/C/D/F + §7 phase plan + §9 + §10 + §11 envelope all VERBATIM from iter-1. Surface facts grounded against `dev` HEAD on 2026-06-12. No investigation phase (low-uncertainty additive enhancement; surface pre-grounded). Both forks (F1 + F2) are **TECHNICAL** (no user-visible delta).

---

## Forks for orchestrator

> Both forks below are **TECHNICAL FORK** (no user-visible delta — the i-phi consumer picks one wire-call either way; the YAML bytes the model reads are valid YAML regardless of which escaping discipline produces them). Per chunk-planner v26 P-plan-1-v26, TECHNICAL FORKs are released from the User-visible / Product-trajectory framing and use the 2-line AskUserQuestion template. F1 and F2 are **independent** (not coupled — no bundle map needed). Both planner-rec bodies are pre-filled in §1 per v32 P-plan-1-v32; **F2 locked USER-DIVERGENT at F2.b** → §1 F2 subsection re-authored at iter-2 (v32 narrow re-author path).

### F1 — `BasicAgent` wiring shape — **TECHNICAL FORK** (no user-visible delta)

| Option | Pros | Cons | Status |
|---|---|---|---|
| **F1.a (planner-rec)** — `with_skills_format(set, format)` sibling 2-arg constructor next to `with_skills` | Explicit + local: the format is chosen exactly where skills are wired; no ordering hazard between a setter and `with_skills`; mirrors the RFC §3 lead; smallest reasoning surface for the i-phi consumer. | One more public method on `BasicAgent` (additive, no removal). | **F1.a LOCKED** (planner-rec) |
| F1.b — `with_skill_prompt_format(format)` setter that `with_skills` consults | Single arg per call; format can be set independently of skills. | Ordering hazard: if `with_skills` is called *before* the setter, the format is missed silently; statefulness on the agent for a one-shot render choice; harder for the consumer to reason about. | not chosen |

**Planner-rec: F1.a.** AskUserQuestion (2-line): *"F1 — BasicAgent YAML-wiring shape. TECHNICAL (no user-visible delta). (a) `with_skills_format(set, format)` sibling constructor [planner-rec — explicit, no ordering hazard] vs (b) `with_skill_prompt_format(format)` setter consulted by `with_skills` [setter; ordering hazard]."*

### F2 — YAML escaping discipline — **TECHNICAL FORK** (no user-visible delta)

| Option | Pros | Cons | Status |
|---|---|---|---|
| F2.a (planner-rec) — always double-quote every scalar value, escaping `"`→`\"`, `\`→`\\`, newline→`\n` | Robustly round-trips ANY value through any YAML parser; minimal branching; no plain-scalar edge-case table to maintain. | A few extra quote-bytes on values that would have been bare-safe. | **not chosen** |
| **F2.b — conditional bare-vs-quoted per YAML plain-scalar rules** | Lighter output (fewer quotes) — bare-safe values stay unquoted. | Larger logic surface + many edge cases (`:` / `#` / leading `-`/`?`/`!` / leading-trailing space / numeric-looking / `null`/`true` reserved words); each missed rule is a latent invalid-YAML defect → the round-trip + reserved-word-type tests become MORE load-bearing (§8 Tier C). | **F2.b LOCKED (USER-DIVERGENT)** — user prefers lighter output; accepts the edge-case table in exchange |

**LOCKED: F2.b (USER-DIVERGENT from planner-rec F2.a).** User-lock rationale: lighter YAML output (bare scalars where safe) accepted in exchange for the explicit quote-trigger table. The `yaml_escape_scalar` helper now decides bare-vs-quoted and the §8 Tier C tests assert the decision boundary (incl. reserved-word type-preservation) — see §1 F2 body + §6 close-gate.

### F3 — keys layout — no real fork

Mechanical. The YAML block matches RFC §2: `available_skills:` mapping → a sequence of `- name: / description: / location:` items. Confirmed in §1 F-layout body; no AskUserQuestion needed.

---

## §1 — Locked fork details (per chunk-planner v32 iter-1 template; planner-rec bodies pre-filled; v28 §1-position codification)

> Open sub-fork option tables above are retained in `## Forks for orchestrator` for traceability. F1 body below is the planner-rec lock semantics (unchanged from iter-1). **F2 body below is re-authored at iter-2 to F2.b USER-DIVERGENT** (v32 narrow re-author: only the F2 subsection changed).

#### F1 = F1.a — `with_skills_format(set, format)` sibling constructor *(LOCKED at gate-1: F1.a planner-rec)*

**Code-level binding.** Add `pub fn with_skills_format(mut self, skills: crate::context::skills::SkillSet, format: crate::context::skills::SkillPromptFormat) -> Self` to `BasicAgent` (`src/agents/basic_agent.rs`, immediately after `with_skills` at `:516`). Its body is `with_skills`'s body verbatim except the render line becomes `skills.format_for_prompt_as(format)` instead of `skills.format_for_prompt()` (same empty-skip + `\n\n`-join logic). `with_skills` (`:506`) is refactored to delegate: `self.with_skills_format(skills, SkillPromptFormat::Xml)` — so the default path provably routes through the XML branch and stays byte-for-byte unchanged.

**Rationale.** F1.a is explicit and local: the layout choice is made at the exact call site where skills are wired, so there is no ordering hazard (the F1.b setter-vs-`with_skills` race silently drops the format if called out of order). It mirrors the RFC §3 lead suggestion and gives the i-phi consumer (#79) the smallest reasoning surface — one 2-arg call. Delegation keeps a single shared body, eliminating drift between the default and explicit paths.

**Defers (if chosen).** Nothing deferred for KC-04: the full wiring ships. The *config/choice* surface (reading `[skills] prompt_format = "xml" | "yaml"` and calling `with_skills_format` accordingly) belongs to i-phi #79 / D-TEST-0074, the consumer half — explicitly OUT of scope here per `[[feedback_phi_core_kernel_minimal]]`. No baby-phi wiring is added (baby-phi inherits the opt-in lever for free when it tracks the new phi-core commit).

#### F2 = F2.b — conditional bare-vs-quoted YAML scalar escaping *(LOCKED at gate-1: F2.b USER-DIVERGENT)*

**Code-level binding.** Add `fn yaml_escape_scalar(s: &str) -> String` in `src/context/skills.rs` (sibling to `xml_escape` at `:421`, private `fn`) that decides **bare** vs **double-quoted** per YAML plain-scalar rules, emitting bare when safe and a double-quoted+escaped form otherwise. It emits the **double-quoted** form when ANY of these quote-triggers fires: the string is **empty**; has **leading or trailing whitespace**; contains a `:` (especially `: ` or a trailing `:`), a `#` (especially ` #`), a **newline / tab / carriage-return**, a `"`, a `'`, or a `\`; **begins** with a YAML indicator char (`- ? : , [ ] { } & * ! | > % @ \``); or is a **reserved / ambiguous plain scalar** that must be quoted to stay a string — `true`/`false`/`null`/`yes`/`no`/`on`/`off`/`~` and their case variants, plus any value that **looks numeric / float / hex / octal / date-looking** (so `42`, `3.14`, `0x1F`, `2026-06-12` survive as strings, not as parsed numbers/dates). When any trigger fires it wraps in `"…"` after replacing (order matters, backslash first) `\`→`\\`, `"`→`\"`, newline→`\n`, tab→`\t`, cr→`\r`; otherwise it emits the value **bare** (unquoted). The YAML branch of `format_for_prompt_as` renders each item as `  - name: {yaml_escape_scalar(name)}\n    description: {…}\n    location: {…}` under an `available_skills:\n` header (no trailing newline, matching the XML branch's no-trailing-newline contract).

**Rationale.** This is the **USER-DIVERGENT lock** (planner-rec was F2.a always-quote): the user prefers **lighter output** (bare scalars where the plain-scalar rules permit) and accepts the explicit quote-trigger table as the cost. The load-bearing surface is now the trigger set itself — a *missed* trigger class is a latent invalid-YAML or, worse, a silent type-change defect (e.g. an unquoted `null` description parsing back as YAML `null`, or `42` parsing back as an integer), so the trigger enumeration above is exhaustive-by-construction and the §6 close-gate + §8 Tier C tests assert BOTH branches of the decision. The byte win this chunk delivers (dropping XML tag overhead) is compounded by skipping quotes on bare-safe values.

**Defers (if chosen).** Block-scalar and folded-scalar styles (`|` / `>`) and multi-document support are NOT added — a single mapping plus a single sequence is the entire RFC §2 shape, so block-literal styling for a future multi-line skill description is a separate enhancement, not a KC-04 gap. The escape helper stays phi-core-internal (`fn`, not `pub`) — no public-surface commitment to the bare-vs-quoted discipline, so a future flip back to always-quote (or to a third style) is a private refactor. No anchor/alias/tag support is in scope.

#### F-layout = F3 — keys match RFC §2 *(mechanical; no lock vote)*

**Code-level binding.** The YAML block is exactly:
```yaml
available_skills:
  - name: "weather"
    description: "Get current weather and forecasts."
    location: "/path/to/skills/weather/SKILL.md"
```
Top-level key `available_skills:`, then a sequence (`- `) of mappings with keys `name` / `description` / `location` in that order, two-space indent for sequence items, four-space indent for item keys. (Under F2.b the value renders bare when plain-scalar-safe and quoted otherwise — the example above shows the quoted form for values carrying spaces/`:`/`/`; a bare `weather`-style value would render unquoted.) Empty set → `""` (same early-return as the XML branch). **Rationale.** Matches the RFC §2 example and the field-order of the XML render (`name`/`description`/`location`). **Defers.** None.

---

## §2 — Concept alignment

| Concept doc (file:line) | Claim the chunk honors | How |
|---|---|---|
| `docs/concepts/skills.md:78-88` (the rendered `<available_skills>` XML example) | The XML render shape is the canonical default and is byte-for-byte preserved. | `format_for_prompt()` delegates to the XML branch; golden test pins it. P-DOCS adds a sibling YAML example + XML-default rationale; verified-header bumped (`:1`, currently `2026-04-05`). |
| `docs/concepts/skills.md:6-13` (Progressive Disclosure: metadata always in system prompt) | The format selector changes only the *layout* of the always-present metadata block, never which metadata is present. | Both branches render the same `name`/`description`/`location` triple from the same `Skill` fields; only the surrounding syntax differs. |
| `docs/specs/architecture.md:108` + `:593` (`format_for_prompt()` renders `<available_skills>` XML, appended to system prompt) | The architecture narrative stays accurate after the additive API lands. | P-DOCS amends `:108` to note the new `format_for_prompt_as(format)` selector with XML default; verified-header bumped (currently `2026-06-03`). |
| `CLAUDE.md` "Key Design Conventions" (`context/skills.rs` loads `<name>/SKILL.md` per the AgentSkills standard) | The AgentSkills XML standard remains the default; YAML is opt-in only. | XML default unchanged; no convention edit required (additive). N/A for body edit; cited for alignment. |

---

## §3 — phi-core leverage map + dependency cascade

**Project = phi-core (kernel lane).** `phi-core-leverage-check` is **N/A** — the kernel does not consume itself (no `use phi_core::` imports inside phi-core's own source). Per the prompt's substitution, the leverage check is replaced by the **kernel-minimality surface-discipline check** below.

### Kernel-minimality surface-discipline check (per `[[feedback_phi_core_kernel_minimal]]`)

| Discipline question | Answer | Evidence |
|---|---|---|
| Does the chunk add ONLY a general layout primitive (enum + render + wiring)? | ✅ Yes | Deliverables = `SkillPromptFormat` enum + `format_for_prompt_as` renderer + `yaml_escape_scalar` helper + `with_skills_format` wiring. Nothing else. |
| Zero i-phi / consumer-specific leakage (no config struct, no i-phi-flavored field)? | ✅ Yes | No config struct, no `prompt_format`-string parsing, no TOML field, no i-phi naming. The *choice* surface (`[skills] prompt_format = …`) is i-phi #79, NOT here. |
| Public-API change purely additive (no removal / rename / breaking)? | ✅ Yes | New `pub enum SkillPromptFormat`, new `pub fn format_for_prompt_as`, new `pub fn with_skills_format`. `format_for_prompt()` signature + output unchanged (delegates). `with_skills()` signature + behaviour unchanged (delegates). |
| No persisted-field / migration / serde-wire / tool-arg-schema change? | ✅ Yes | `SkillPromptFormat` is a render-time selector only; never serialized, never persisted. `Skill`/`SkillSet` structs untouched. |

**Verdict: kernel-minimality PASS** — genuinely-general additive layout primitive, zero consumer leakage.

### Forbidden-duplication greps (the inverse contract)

```bash
# No second YAML-layout emitter / no parallel skill-render path may be introduced:
grep -rn "available_skills:" /root/projects/phi/phi-core/src/    # expect ONLY the new YAML branch in skills.rs
grep -rnE "fn .*format.*skill|fn .*skill.*prompt" /root/projects/phi/phi-core/src/ | grep -v "format_for_prompt"  # expect 0 — no sibling renderer outside skills.rs
```

### Dependency cascade (vector B) — **resolved: ZERO new deps** (unchanged under F2.b)

The deliverable §6(b) round-trip test needs a real YAML parser. **`serde_yaml = "0.9"` is ALREADY a regular `[dependencies]` entry** (`Cargo.toml:35`) — confirmed via `grep -nE "serde_yaml" Cargo.toml`. The round-trip test uses `serde_yaml::from_str::<serde_yaml::Value>(&yaml)` (or `serde_yaml::Value` field-walk) with **no new dev-dependency**. `tempfile = "3"` (`:43`) is the existing dev-dep used to materialize on-disk skills in tests; reused unchanged. F2.b does not change the dep cascade — the conditional escaping is hand-rolled string logic, no new crate.

| Dep / feature | Predicted change | Verification (run at gate-1.5 P-orch-2) | PAUSE if discovered |
|---|---|---|---|
| `serde_yaml` | **none** (already a direct dep) | `cargo tree --manifest-path /root/projects/phi/phi-core/Cargo.toml -p serde_yaml \| head -1` confirms present | A NEW yaml crate add (e.g. `yaml-rust`) would PAUSE — not expected; serde_yaml suffices. |
| `tempfile` | none (dev-dep, reused) | already present `:43` | n/a |

**Cascade-band (production-tier, per v34 P-plan-3-v34)**: `format_for_prompt` non-test call sites = **2** (`skills.rs:21` doc-comment example + `basic_agent.rs:507` `with_skills`), confirmed via `grep -rn "format_for_prompt\b" src/ | grep -v format_for_prompt_as`. Production cascade ≤ 2 (both stay calling `format_for_prompt()` which delegates; zero edits to either *call* — `with_skills` body changes to delegate but its `format_for_prompt()`-equivalent semantics are preserved). Test-tier: 3 existing inline tests call `format_for_prompt()` (`:466`, `:480`, `:576`) — all stay green unchanged (they assert XML, which is byte-preserved). NOT band-constrained.

### §3.B — K8s readiness

**N/A — phi-core is a library, not a daemon; K8s posture not applicable** (per `k8s-readiness-check` v2 phi-core branch: returns N/A for all 7 axes, no ledger entry). No in-process state, no IPC, no pod-local resources, no migration runner, no Repository trait, no cross-pod sharing, no audit hash-chain — the chunk adds a pure render function.

### §3.C — User-facing docs (3-tier)

| Tier | Touched? | Decision |
|---|---|---|
| Architecture (`docs/specs/architecture.md`) | Yes | Amend `:108` + `:593` to note the `format_for_prompt_as(format)` selector (XML default); verified-header bump. In-scope at P-DOCS. |
| Operations | No | **N/A — phi-core is a library; no operations/deployment doc tier.** |
| User-guide (`docs/concepts/skills.md` + rustdoc) | Yes | `concepts/skills.md` gains a sibling YAML render example + XML-default rationale; rustdoc on the new API. In-scope at P-DOCS. |

### §3.D — Forward-scope ↔ concept-doc precedence

No closed-set / fixed-order / frozen-schema contradiction. The chunk adds an enum + render branch; it does not touch any concept-doc canonical closed set (no Action vocabulary, no fundamental kinds — those are baby-phi/i-phi permission concepts, absent in phi-core). **N/A — no contradiction.**

### §3.E — Anticipated gate-2.5 candidates

- **Hand-rolled helper `yaml_escape_scalar` used in ≥ 2 files?** No — used only inside `skills.rs` (the YAML branch). Stays `fn` (private). No centralization decision needed. Sibling to `xml_escape` (`:421`) which is likewise private + single-file. (Under F2.b this helper carries the bare-vs-quoted decision logic; still single-file.)
- **No `chrono`/`time` substitution candidate** (no timestamp emission in this render).
- No latent-defect cushion class (no NEW Repository method, no SurrealDB, no compound-tx). N/A.
- **F2.b quote-trigger completeness** (gate-2.5 candidate): if P1 surfaces a plain-scalar edge class not enumerated in the §1 F2 body (e.g. a non-ASCII indicator or a locale-specific bool spelling), the implementer adds the trigger + a Tier C MAY-COVER case rather than silently emitting bare. Revisit-trigger logged in §5 §D4.3.

### §3.F — SurrealDB SCHEMAFULL checklist

**N/A — no SurrealDB, no migration, no SCHEMAFULL table** in this chunk (phi-core has no persistence layer touched here).

---

## §4 — Drifts closed

- **GitHub #78 / D-TEST-0073 (S3)** — selectable skill-prompt layout. Primary + only. Status flip at this cycle: `open → closed` at P-SEAL (GitHub-mirrored per `[[feedback_dtest_github_mirror.md]]`). The phi-core half lands; the issue notes #79 (consumer) as the follow-on, NOT closed by KC-04.
- **Prior-cycle closure ratified at this cycle (per v34 P-plan-2-v34)**: none — KC-04 has no upstream drift dependency (KC-01/02/03 are independent revert-contract surfaces; no shared scaffold).
- Any NEW defect surfaced → filed as its own D-TEST drift (GitHub-mirrored).

---

## §5 — ADR draft

**ADR file**: `/root/projects/phi/phi-core/docs/decisions/0004-selectable-skill-prompt-layout.md` — confirmed naming convention via `ls docs/decisions/` (`0001-…`, `0002-…`, `0003-…`; KC-04 = `0004-`). The forward-scope's "ADR-0004" matches. Cite sub-decisions as `ADR-0004 §D4.<M>`.

**ADR top-level sections** (mirrors ADR-0003 verbatim shape — confirmed 7 `## ` headers: Forks / Context / Sub-decisions / Cross-references / Consequences / Revisit triggers / Verification):

1. `## Forks` — header table: F1 (TECHNICAL, F1.a locked) + F2 (TECHNICAL, **F2.b locked USER-DIVERGENT**) + F3 (mechanical), each with Locked option + Why.
2. `## Context` — #78/D-TEST-0073 RFC + the coupled kernel→consumer pair (#79 downstream); the AgentSkills XML-standard rationale for keeping XML default.
3. `## Sub-decisions`:
   - `### §D4.1` — XML stays the byte-for-byte default (golden-pinned). *Net-new selectable-layout surface; no prior behaviour to preserve beyond the existing XML render, which is preserved verbatim — narrative form per v24 P-plan-2-v24, with explicit "existing `format_for_prompt()` XML output preserved byte-for-byte" note.*
   - `### §D4.2` — F1.a `with_skills_format(set, format)` sibling constructor + `with_skills` delegates (resolves F1). *Pre-existing-behaviour: `with_skills` shipped before KC-04; its signature + system-prompt-append behaviour preserved (now via delegation to XML branch).*
   - `### §D4.3` — **F2.b conditional bare-vs-quoted YAML escaping (resolves F2). USER-DIVERGENT from the planner-rec F2.a (always-quote).** One-line rationale: *user-locked lighter output (bare scalars where plain-scalar-safe), accepting the explicit quote-trigger table as the cost.* Body enumerates the quote-trigger set (empty / leading-trailing space / `:`/`#`/newline/tab/cr/`"`/`'`/`\` / leading indicator chars / reserved-word + numeric/date-looking) + the escape sequence applied when quoting. *Net-new surface — no prior YAML render existed; no prior behaviour to preserve.*
   - `### §D4.4` — F3 keys layout matches RFC §2. *Net-new surface.*
   - `### §D4.5` — kernel-minimality: phi-core owns the layout primitive; i-phi #79 owns the config/choice surface. *Net-new boundary decision.*
4. `## Cross-references` — (a) concept doc `concepts/skills.md` + `specs/architecture.md` (line ranges); (b) closed drift #78/D-TEST-0073; (c) prior ADRs as precedent: `0003-revert-tail-shrink-contract.md` (kernel-lane ADR shape + investigation-first/additive precedent); (d) forward-scope row `kc-04-selectable-skill-prompt-layout.md`.
5. `## Consequences` — `### For i-phi #79 / D-TEST-0074` (the consumer config knob inherits the `with_skills_format(set, SkillPromptFormat::Yaml)` entry point; forward-routing note); `### For baby-phi + future consumers` (opt-in token lever available for free on the next tracked phi-core commit).
6. `## Revisit triggers` — (i) **if a quote-trigger class is found missing in the field** (a description that should have quoted renders bare and parses back as a non-string / invalid YAML) → **re-open §D4.3** (extend the trigger set, or fall back to F2.a always-quote); (ii) if a consumer needs even-lighter byte-budget output not achievable under the plain-scalar rules → re-open §D4.3; (iii) if a third layout (e.g. JSON / Markdown) is requested → re-open §D4.1 enum-extension; (iv) if the AgentSkills standard changes the XML default shape → re-open §D4.1; (v) if `with_skills` ordering becomes a documented foot-gun → re-open §D4.2 (F1.b).
7. `## Verification` — the §12 recipe commands (golden XML pin + YAML round-trip [both branches] + wiring test + clippy/fmt).

---

## §6 — Prior-chunk regression invariants + close-gate

| Invariant | Verifying command | Expected |
|---|---|---|
| KC-01/02/03 revert-contract tests intact (independent surface) | `cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml` | full suite green; **583 baseline** unchanged except the new tests added (§8). |
| Existing XML render byte-for-byte preserved | golden test (§8 test a) + existing `format_for_prompt_xml` (`:461`), `empty_when_no_skills` (`:480`), `xml_escaping` (`:565`) stay green | XML output identical. |
| `with_skills` system-prompt-append behaviour preserved | existing callers + the 13 inline `skills.rs` tests | unchanged. |

**No divergent-fork scaffold removal** (both forks are additive TECHNICAL) → v19 P2 back-compat decision-template **N/A** (nothing removed).

### Close-gate (model-facing format → `[[feedback_render_transcript_close_gate]]` Rule 4)

**Decision: unit-level golden + parser round-trip + wiring test FULLY discharges the contract; a live-transcript provider read is NOT warranted.** (Decision unchanged from iter-1; the round-trip bullets below are STRENGTHENED under F2.b because a missed quote-trigger is now a latent invalid-YAML / silent-type-change defect.) Rationale: the YAML block is pure system-prompt *text* that reaches the provider through the identical path as XML (it is appended to `system_prompt` by `with_skills_format`, exactly as XML is by `with_skills`). There is no provider-specific re-encoding, no streaming transform, no tool-call interaction — the rendered string IS the wire. The Rule-4 contract ("assert each rule of the intended contract against the wire, not merely 'compiles'") is discharged at the unit level by:
- **(a) XML default byte-for-byte unchanged** — golden test asserts `format_for_prompt() == <exact today's bytes>` (the disposition, not just no-panic).
- **(b) YAML is valid AND the bare-vs-quoted decision is correct on the MUST-QUOTE branch** — round-trip through `serde_yaml` asserts that for EACH quote-trigger class (a `:`-containing value; a reserved-word / numeric-looking value like `null` / `true` / `42`; a value with a leading indicator / leading-trailing space; an embedded `"`/`\`/newline), the field **recovers as the original STRING** — not as a YAML bool, null, integer, float, or date. A missed trigger here silently changes the value's *type*, so this branch is the highest-value F2.b regression check (assert the *content disposition AND its YAML type*, not merely "parses").
- **(c) bare values still round-trip AND the wiring reaches the system prompt** — a plain safe value (e.g. `weather`) renders **bare** and still round-trips as the original string through `serde_yaml`; and `with_skills_format(set, Yaml)` test asserts the YAML block substring is present in the constructed agent's `system_prompt`.

This is lighter than KC-03's close-gate (which needed a live render-pipeline repro because the contract was a model-visible *revert disposition* mediated by the provider). Here the rendered text is the contract surface directly — but under F2.b the *escaping decision* is itself part of the contract, so (b) asserts both branches.

---

## §7 — Phase plan

**§7.0 phase-order stress-test** (per v25 P-plan-4): only 2 substantive phases + paperwork; both are purely additive (new enum + new methods); no migration, no struct-field removal, no SCHEMAFULL. Workspace stays GREEN by construction at every boundary (each phase compiles + tests independently). No RED window. Cross-lock interaction (F1 × F2): independent — F1 wires the call, F2 escapes the value; they meet only at `format_for_prompt_as`'s YAML branch and compose trivially. **No phase-order hazard.**

### P1 — Render core (enum + dispatching renderer + YAML branch + golden/round-trip/escaping/empty tests)

- **Goal**: ship `SkillPromptFormat`, `format_for_prompt_as`, `yaml_escape_scalar`, and refactor `format_for_prompt` to delegate — all in `src/context/skills.rs` — with the XML default golden-pinned.
- **Deliverables**: (1) `pub enum SkillPromptFormat { Xml, Yaml }` with `#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]` + `#[default] Xml`; re-export at `lib.rs:71` + `context/mod.rs:37` alongside `SkillSet`. (2) `pub fn format_for_prompt_as(&self, format: SkillPromptFormat) -> String` — `match format { Xml => <today's body verbatim>, Yaml => <new body> }`. (3) `format_for_prompt()` → `self.format_for_prompt_as(SkillPromptFormat::Xml)`. (4) `fn yaml_escape_scalar(s: &str) -> String` (conditional bare-vs-quoted per F2.b — see §1 body for the trigger set).
- **Tests**: (a) golden byte-for-byte XML pin (`format_for_prompt()` == literal expected string); (b) YAML round-trips through `serde_yaml::from_str::<serde_yaml::Value>` (fields recovered); (c) escaping decision boundary — MUST-QUOTE classes (`:`-value + reserved-word/numeric value) recover as STRING + a plain safe value stays bare (split into 2 MUST-SHIP, see §8); (d) empty-set `""` for BOTH `Xml` and `Yaml`.
- **Confidence**: 10/10 — single-file additive; all surface grounded.
- **Pause-discipline**: `skills.rs` is currently ~600 LOC + 13 inline tests. Per-file cap **≤ 720 LOC** (baseline ~600 + ~40 production [enum + YAML branch + conditional escape helper, ~30 LOC framework allowance per v33 P-plan-1-v33 for derives] + ~80 inline-test LOC [4–5 new tests incl. serde_yaml round-trip boilerplate ~30 LOC]). PAUSE if file > 1.5× → > 1080 LOC (not expected). PAUSE if any cross-file edit beyond `lib.rs`/`context/mod.rs` re-exports surfaces.

### P2 — `BasicAgent` wiring (F1.a) + wiring test

- **Goal**: `with_skills_format(set, format)` sibling constructor; `with_skills` delegates to the XML branch.
- **Deliverables**: (5) `pub fn with_skills_format` at `basic_agent.rs` (after `:516`); refactor `with_skills` (`:506`) to `self.with_skills_format(skills, SkillPromptFormat::Xml)`. Import `SkillPromptFormat`.
- **Tests**: (e) `with_skills_format(set, Yaml)` reaches `system_prompt` (assert YAML substring present); a paired assert that `with_skills(set)` still produces the XML substring (default unchanged).
- **Confidence**: 10/10 — mirrors existing `with_skills` body.
- **Pause-discipline**: `basic_agent.rs` delta ≈ +12 LOC production. PAUSE if delegation breaks any existing `basic_agent.rs` test.

### P-DOCS — rustdoc + ADR-0004 + concept/architecture docs + verified-headers

- **Goal**: document the additive API + XML-default rationale; author ADR-0004; refresh verified-headers.
- **Deliverables**: (6) rustdoc on `SkillPromptFormat` + `format_for_prompt_as` + `with_skills_format` (XML-default rationale; cite AgentSkills standard). (7) ADR-0004 (§5 shape). (8) `concepts/skills.md` sibling YAML example + rationale (verified-header `:1` bump). (9) `specs/architecture.md:108` + `:593` amend (verified-header bump). Doc-LOC: ADR-0004 ≥ precedent-baseline (ADR-0003 = ~18 KB / 7 sections; KC-04 is simpler — target all 7 sections present + ≥ 5 sub-decisions, semantic-completeness escape per v29 P-plan-7).
- **Confidence**: 9/10.
- **Pause-discipline**: doc-only; no cargo pause class.

### P-SEAL — paperwork

- **Goal**: cycle-index row, GitHub #78 close, verified-headers final pass, chunk-order N/A (phi-core has no chunk-order.md).
- **Deliverables**: cycle-index row (leave `Iterations = pending`, `Status = in-flight` — orchestrator owns transitions per `_cycle-index.md` row-lifecycle: gate-3 → audited-pending-retro; Phase 7 → retro-complete + final iter count); commit subject prepends `D-TEST-0073:` per outer CLAUDE.md commit-subject discipline; GitHub #78 close note (kernel half; #79 follow-on).
- **Confidence**: 10/10.

---

## §8 — Tests summary

**Baseline (grounded `cargo test` 2026-06-12)**: **583 passed** total = lib unittests **254** (includes the 13 inline `skills.rs` tests) + 329 across integration binaries. No integration binary touches `with_skills`/`format_for_prompt` (confirmed `grep -rln` over `tests/` → 0 hits) — all new tests are **inline in `skills.rs`** (a–d) + **1 inline in `basic_agent.rs`** (e).

Per-Tier breakdown (per v22 P1):

| Tier | Tests | Location |
|---|---|---|
| A — golden XML byte-for-byte pin | 1 | `skills.rs` inline |
| B — YAML round-trip through `serde_yaml` (multi-skill sequence recovers) | 1 | `skills.rs` inline |
| C-1 — escaping decision boundary: a `:`-containing description QUOTES + recovers as a STRING; a plain safe value (`weather`) stays BARE + recovers | 1 | `skills.rs` inline |
| C-2 — reserved-word type-preservation: a reserved/numeric value (`null` / `true` / `42`) QUOTES + recovers as a STRING (not as YAML bool/null/integer) — highest-value F2.b regression | 1 | `skills.rs` inline |
| D — empty-set `""` for both Xml + Yaml | 1 | `skills.rs` inline |
| E — `with_skills_format(set, Yaml)` reaches system prompt (+ XML-default-unchanged paired assert) | 1 | `basic_agent.rs` inline |
| **Total NEW MUST-SHIP** | **6** | |

**Tier C split (per F2.b USER-DIVERGENT lock)**: under F2.b the escaping branch carries the load-bearing decision logic, so Tier C splits into **C-1 (bare-vs-quoted decision boundary)** + **C-2 (reserved-word/numeric type-preservation)**. C-2 is **MUST-SHIP, not MAY-COVER** — a missed reserved-word trigger silently changes a value's YAML *type* (an unquoted `null` description parses back as YAML null; an unquoted `42` as an integer), which is the single most dangerous F2.b regression.

**Expected delta: +6** (583 → **589**). Tolerance band **[589, 592]** (+0 to +3 MAY-COVER: a YAML parser may motivate an extra "leading-indicator value quotes + recovers" or "embedded-`#` comment-char value quotes" or "leading/trailing-space value quotes" inline case). Existing 583 all stay green (XML byte-preserved; revert-contract suites independent).

---

## §9 — Pre-chunk gate (reading list)

Implementer MUST read before P1:
1. `src/context/skills.rs:220-256` (current `format_for_prompt` XML body) + `:411-427` (`xml_escape` sibling) + `:429-580` (inline test module shape).
2. `src/context/skills.rs:29-48` (`Skill` + `SkillSet` struct defs — render fields `name`/`description`/`file_path`).
3. `src/agents/basic_agent.rs:501-516` (`with_skills` to mirror).
4. `src/lib.rs:71` + `src/context/mod.rs:37` (`SkillSet` re-export path — `SkillPromptFormat` follows it).
5. `Cargo.toml:35` (`serde_yaml = "0.9"` already present — round-trip test dep).
6. `docs/decisions/0003-revert-tail-shrink-contract.md:1-98` (ADR shape template for ADR-0004).
7. Forward-scope `kc-04-selectable-skill-prompt-layout.md` (full).

**Carry-forward invariants**: 583 baseline green; XML byte-for-byte; `with_skills` behaviour preserved (§6).

---

## §10 — Close criteria

**Code aspect**: `format_for_prompt()` byte-identical to today (golden pins it); `format_for_prompt_as(Yaml)` emits valid YAML round-tripping through `serde_yaml`, with the F2.b bare-vs-quoted decision asserted on BOTH branches (MUST-QUOTE classes recover as strings; bare-safe values stay bare); `with_skills_format(set, Yaml)` renders YAML into system prompt; default `with_skills(set)` byte-for-byte unchanged; `RUSTFLAGS="-Dwarnings"` clippy clean + full suite green + `fmt --check`. Kernel-minimality: additive-only, zero consumer leakage (§3 check PASS).

**Docs aspect**: ADR-0004 Accepted with §D4.1–§D4.5 + all 7 sections (§D4.3 records F2.b USER-DIVERGENT + the quote-trigger set + the Revisit trigger linking back); `concepts/skills.md` + `architecture.md` amended + verified-headers bumped; rustdoc on all 3 new public items.

**Confidence target ≥ 9/10**, written as `claims-honored / claims-in-scope`. Target: **10/10** (every claim grounded; additive; no migration/persistence/provider interaction).

---

## §11 — Audit plan

**Envelope: Medium (2 auditors A + B)** — phase count 2 substantive (P1 + P2) + P-DOCS + P-SEAL; the sizing rule (3–5 phases → Medium) + the forward-scope §8 hint both confirm Medium. Single-module additive surface + public-API addition + model-facing format. (Envelope unchanged at iter-2 under F2.b; the escaping-correctness audit claim in Audit A absorbs the bare-vs-quoted decision.)

### Audit A (code + tests + escaping correctness + kernel-minimality) — ≤ 600 words

```
You are auditing KC-04 in phi-core at /root/projects/phi/phi-core. Read-only on source. Plan at <cycle-folder>/plan.md.

Verify each claim with file:line citation:
1. `pub enum SkillPromptFormat { Xml, Yaml }` exists with #[derive(Debug,Clone,Copy,PartialEq,Eq,Default)] + #[default] Xml; re-exported at lib.rs + context/mod.rs alongside SkillSet.
2. `format_for_prompt_as(&self, SkillPromptFormat) -> String` exists; XML branch == today's logic; `format_for_prompt()` delegates to `format_for_prompt_as(Xml)`.
3. Golden test pins `format_for_prompt()` byte-for-byte to today's XML (assert the exact bytes, not substring).
4. F2.b conditional escaping: `yaml_escape_scalar` decides bare-vs-quoted; round-trip test parses output via serde_yaml and (i) MUST-QUOTE classes (`:`-value, reserved-word/numeric value like null/true/42) recover as STRING (not bool/null/integer); (ii) a plain safe value (weather) stays BARE + recovers; empty-set returns "" for BOTH formats.
5. `with_skills_format(set, format)` exists on BasicAgent; `with_skills` delegates with Xml; a wiring test asserts YAML reaches system_prompt AND default `with_skills` still emits XML.
6. Kernel-minimality: additive-only; ZERO i-phi/consumer leakage (no config struct, no prompt_format string-parse); no migration/persisted-field/serde-wire change. `yaml_escape_scalar` is private (fn, not pub).
7. cargo test green at expected count (583 baseline + 6 = 589, band [589,592]); RUSTFLAGS="-Dwarnings" clippy --all-targets clean; fmt --check clean.
   (MUST-RUN: clippy + test + fmt are orchestrator-closed; mark NOT-EXECUTED-IN-AUDIT if sandbox-blocked.)
8. Forbidden-duplication: only ONE YAML emitter (the skills.rs branch); no parallel skill-render fn outside skills.rs.

PASS/FAIL each. ≤ 600 words.
```

### Audit B (docs + ADR + rustdoc + verified-headers) — ≤ 600 words

```
You are auditing KC-04's docs/concept fidelity. Read-only. Plan at <cycle-folder>/plan.md.

Verify each claim:
1. ADR-0004 Accepted at docs/decisions/0004-selectable-skill-prompt-layout.md with §D4.1..§D4.5 and all 7 top-level sections (Forks/Context/Sub-decisions/Cross-references/Consequences/Revisit triggers/Verification).
2. ADR §D4.3 records F2.b conditional bare-vs-quoted YAML escaping as USER-DIVERGENT from planner-rec F2.a, with the quote-trigger set + a one-line user-lock rationale.
3. ADR Consequences carries a `### For i-phi #79 / D-TEST-0074` subsection (consumer config-knob forward-routing) + `### For baby-phi + future consumers`. ADR Revisit triggers lists ≥ 3 conditions (incl. the F2.b missing-quote-trigger → re-open §D4.3), each citing a §D4.<M>.
4. docs/concepts/skills.md gains a sibling YAML render example + XML-default rationale; verified-header (:1) bumped to 2026-06-12 with a KC-04 note.
5. docs/specs/architecture.md :108 + :593 amended to note format_for_prompt_as(format) selector (XML default); verified-header bumped.
6. rustdoc present on SkillPromptFormat + format_for_prompt_as + with_skills_format, each stating XML is the default.
7. GitHub #78 / D-TEST-0073 closed; commit subject prepends the drift id; cycle-index row appended (Iterations=pending, Status=in-flight at audit time).
8. Cross-references cite concepts/skills.md + architecture.md line ranges + prior ADR-0003 as precedent + forward-scope row. XML-standard rationale (AgentSkills) stated.

PASS/FAIL each. ≤ 600 words.
```

---

## §12 — Verification recipe (copy-paste)

```bash
# 1. Full suite (host cargo; single crate; -j 4 cap; NO --workspace).
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml 2>&1 | grep -E "test result:|^error" | tail -40

# 2. Targeted new tests.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml format_for_prompt 2>&1 | tail -15
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml with_skills_format 2>&1 | tail -15

# 3. Clippy (warnings = errors) + fmt.
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets 2>&1 | tail -15
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# 4. Kernel-minimality + forbidden-duplication greps.
grep -rn "available_skills:" /root/projects/phi/phi-core/src/                                  # expect: only the YAML branch in skills.rs
grep -rnE "prompt_format|PromptFormat" /root/projects/phi/phi-core/src/ | grep -vi "SkillPromptFormat"   # expect: 0 — no consumer config leakage
grep -n "SkillPromptFormat" /root/projects/phi/phi-core/src/lib.rs /root/projects/phi/phi-core/src/context/mod.rs   # expect: re-export present
grep -n "yaml_escape_scalar" /root/projects/phi/phi-core/src/context/skills.rs                 # expect: the F2.b conditional helper (private fn, single-file)

# 5. Dep cascade confirm (zero new deps).
grep -nE "serde_yaml" /root/projects/phi/phi-core/Cargo.toml                                   # expect: serde_yaml = "0.9" already present

# 6. Doc-links: phi-core has no scripts/check-*.sh — N/A (skip with paperwork note).
```

**MUST-RUN gate (orchestrator gate-4, no CI scripts exist on phi-core)**: items 1 + 3 above. phi-core ships only a `scripts/pre-commit` fmt+clippy hook; the MUST-RUN list IS the gate. Note guard-absence in cycle-audit.
