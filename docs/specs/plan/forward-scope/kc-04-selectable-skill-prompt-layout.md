<!-- Last verified: 2026-06-12 by Claude Code -->

# KC-04 — Selectable skill-prompt layout (XML default + opt-in YAML)

> Fourth phi-core kernel chunk. Closes GitHub **#78 / D-TEST-0073 (S3)** — the phi-core half of a coupled kernel→consumer pair (the i-phi config knob #79 / D-TEST-0074 depends on this landing first). **Purely additive, back-compat**: XML stays the byte-for-byte default; YAML is an opt-in lighter layout for token-tight deployments. No investigation phase (low-uncertainty enhancement; surface grounded by the orchestrator before initiate).

## §1 — Purpose

Add a **format selector** to the skill-prompt render so a caller can opt into a lighter **YAML** layout for the `<available_skills>` block, while keeping **XML the default** (the [AgentSkills standard](https://agentskills.io/integrate-skills) + the layout Claude models are best-aligned on). Existing callers are untouched — the default chain stays XML byte-for-byte. Per `[[feedback_phi_core_kernel_minimal]]`: phi-core owns the *layout primitive*; the consumer (i-phi #79) owns the *choice* + config surface.

## §2 — Inputs consumed

- **GitHub #78 / D-TEST-0073** — the RFC: §3 proposed additive API, §4 the XML-stays-default rationale, §5 acceptance criteria.
- **Render code** (current `dev`): `src/context/skills.rs` `format_for_prompt()` (`:235` — the XML-only emitter), `xml_escape()` (`:421`), the existing render unit tests (`format_for_prompt_xml` `:461`, empty-set `:480`). `src/agents/basic_agent.rs` `with_skills()` (`:506` — appends `format_for_prompt()` to the system prompt at construction).
- **Consumer (downstream, not this chunk)**: i-phi #79 / D-TEST-0074 — the i-phi `[skills] prompt_format = "xml" | "yaml"` config knob that selects this layout.
- **Memories**: `[[feedback_phi_core_kernel_minimal]]` (general primitive only; no i-phi leakage), `[[feedback_render_transcript_close_gate]]` (model-facing → render + read the actual block; Rule 4 — assert the YAML is valid + the default is unchanged, not just "compiles").

## §3 — Issues / drifts closed

- **#78 / D-TEST-0073 (S3)** — the phi-core selectable skill-prompt layout. Primary + only.
- Any NEW defect the work surfaces is filed as its own D-TEST drift (GitHub-mirrored).

## §4 — Prerequisites

- KC-01/02/03 landed on `dev`. ✓ (no code dependency — independent surface).
- `chunk-archive-plan` phi-core-aware. ✓
- None blocking. Unblocks i-phi #79 (the consumer half).

## §5 — Deliverables

1. **`pub enum SkillPromptFormat { Xml, Yaml }`** in `context/skills.rs`, `#[derive(Default)]` with `#[default] Xml` (+ Debug/Clone/Copy/PartialEq/Eq). Public re-export consistent with `SkillSet`'s current export path.
2. **`pub fn format_for_prompt_as(&self, format: SkillPromptFormat) -> String`** — the format-dispatching renderer. XML branch = today's logic (extracted verbatim); YAML branch = the new lighter layout with YAML-safe escaping.
3. **`format_for_prompt()` retained** as `self.format_for_prompt_as(SkillPromptFormat::Xml)` — byte-for-byte identical to today (golden-pinned). Existing callers untouched.
4. **`BasicAgent` wiring** — a path to render YAML into the system prompt (F1 decides the exact shape: `with_skills_format(set, format)` vs a `with_skill_prompt_format(format)` setter consulted by `with_skills`). Default `.with_skills(set)` unchanged.
5. **YAML escaping** — F2 decides the discipline (lean: always double-quote scalar values, escaping `"`/`\`/newlines, which round-trips robustly through any YAML parser). Reuse or sibling the existing `xml_escape` discipline.
6. **Tests** — (a) golden test pinning `format_for_prompt()` == today's XML byte-for-byte; (b) `format_for_prompt_as(Yaml)` emits valid YAML that round-trips through a parser (assert structural fields recovered); (c) escaping test (values with `:`/`#`/quotes/newlines stay valid + recover); (d) empty-set returns `""` for both formats; (e) a `with_skills_format(set, Yaml)` wiring test asserting the YAML block reaches the system prompt.
7. **Docs** — `context/skills.rs` rustdoc on the new API + the XML-default rationale; phi-core **ADR-0004** (the selectable-layout decision + why XML stays default); any concept/architecture doc that documents the skills render. Verified-headers refreshed.

## §6 — Acceptance criteria

- `format_for_prompt()` output byte-identical to today (golden test pins it).
- `format_for_prompt_as(SkillPromptFormat::Yaml)` emits valid, properly-escaped YAML that round-trips through a YAML parser.
- A `with_skills_format(set, Yaml)` path renders the YAML block into the system prompt; default `.with_skills(set)` is byte-for-byte unchanged.
- `RUSTFLAGS="-Dwarnings"` clippy clean + full phi-core suite green + `fmt --check`.
- Kernel-minimality: a general layout primitive only; zero i-phi/consumer leakage; the only public-API change is purely additive (new enum + new method(s)); no persisted-field / migration / breaking change.

## §7 — Forks for the planner

- **F1 — `BasicAgent` wiring shape**: (a) `with_skills_format(set, format)` — a sibling 2-arg constructor next to `with_skills`; explicit, the RFC's lead suggestion. vs (b) `with_skill_prompt_format(format)` setter — a separate builder setter that `with_skills` consults (stores the format on the agent, `with_skills` reads it). Lean **(a)** for explicitness + locality (the format is chosen exactly where skills are wired; no ordering hazard between setter + `with_skills`). **TECHNICAL FORK** (no user-visible delta; the i-phi consumer picks one wire-call either way).
- **F2 — YAML escaping discipline**: (a) always double-quote every scalar value (`name`/`description`/`location`), escaping `"`→`\"`, `\`→`\\`, newline→`\n` — simple + robustly round-trips any value. vs (b) conditional bare-vs-quoted per YAML plain-scalar rules (only quote when the value contains `:`/`#`/leading-special) — lighter output but more logic + edge cases. Lean **(a)** (correctness + simplicity over a few saved quote-bytes; the token win is already in dropping the XML tags). **TECHNICAL FORK**.
- **F3 — keys layout**: confirm the YAML block shape matches the RFC §2 example (`available_skills:` → list of `- name:/description:/location:`). Mechanical; no real fork.

## §8 — Audit envelope hint

**Medium (2 auditors: A code + tests + escaping correctness / B docs + ADR + rustdoc + verified-headers).** Single-module additive surface (`skills.rs` + a small `basic_agent.rs` wiring delta) + a public-API addition + model-facing format. Confirm at gate-1.

## §9 — Unblocks

- **i-phi #79 / D-TEST-0074** — the consumer config knob that makes the layout selectable per i-phi config. KC-04 is the prerequisite; #79 runs as the next cycle once i-phi tracks the new phi-core commit.
- Any future consumer (baby-phi, others) gets the same opt-in token lever for free.

## §10 — Risks

- **Default-drift** — the one real hazard is silently changing the XML default. The golden byte-for-byte test is the guard; `with_skills` default path must call the XML branch.
- **YAML validity** — a naively-escaped value could emit invalid YAML. The round-trip-through-a-parser test (not just a string-shape assertion) is the guard.
- **Kernel-creep** — resist adding an i-phi-flavored config struct here; phi-core ships only the enum + render + wiring. The config/choice surface is i-phi #79.

## §11 — Direct-approval criteria fit

Does **NOT** auto-approve (`approval=yes`): two forks (F1 wiring shape + F2 escaping) are locked at gate-1, and the chunk adds public API + a model-facing format. Both forks are TECHNICAL (no user-visible delta) so the 2-line AskUserQuestion template applies. Low-uncertainty otherwise; investigation skipped.
