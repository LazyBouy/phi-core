<!-- Last verified: 2026-06-12 by Claude Code (KC-04 #78/D-TEST-0073 — phi-core ADR-0004; selectable skill-prompt layout: XML stays the byte-for-byte default, YAML is opt-in; §D4.3 = F2.b USER-DIVERGENT conditional bare-vs-quoted escaping) -->

# phi-core ADR-0004 — Selectable skill-prompt layout (XML default + opt-in YAML)

**Status: Accepted**

> Fourth phi-core ADR. Closes GitHub #78 / D-TEST-0073 (S3: selectable skill-prompt layout) — the phi-core half of a coupled kernel→consumer pair (i-phi config knob #79 / D-TEST-0074 depends on this landing first, OUT of scope here). Cite sub-decisions as `ADR-0004 §D4.<M>`. **Purely additive, back-compat**: the existing `format_for_prompt()` XML output is preserved byte-for-byte; YAML is an opt-in lighter layout. No removal / rename / migration / persisted-field / serde-wire / tool-arg-schema change. Both forks are TECHNICAL (no user-visible delta); F2 locked **USER-DIVERGENT** at gate-1.

## Forks

| Fork | Question | Locked option | Why |
|---|---|---|---|
| F1 | `BasicAgent` YAML-wiring shape (**TECHNICAL** — no user-visible delta) | **F1.a — `with_skills_format(set, format)` sibling constructor**; `with_skills` delegates with `Xml` | Explicit + local: the layout choice is made at the exact call site where skills are wired, so there is no ordering hazard (the F1.b setter-vs-`with_skills` race would silently drop the format if called out of order). Mirrors the RFC §3 lead and gives the i-phi consumer (#79) the smallest reasoning surface — one 2-arg call. Delegation keeps a single shared body, eliminating drift between the default and explicit paths. |
| F2 | YAML escaping discipline (**TECHNICAL** — no user-visible delta) | **F2.b — conditional bare-vs-quoted per YAML plain-scalar rules** (**USER-DIVERGENT** from planner-rec F2.a always-quote) | User prefers lighter output — bare scalars where the plain-scalar rules permit — and accepts the explicit quote-trigger table as the cost. The `yaml_escape_scalar` helper decides bare-vs-quoted; the load-bearing surface becomes the trigger set itself (a missed trigger is a latent invalid-YAML / silent type-change defect), so the round-trip + reserved-word-type tests assert BOTH branches of the decision. |
| F3 | keys layout (**mechanical** — no lock vote) | **F3 — keys match RFC §2** (`available_skills:` mapping → a sequence of `- name: / description: / location:` items) | Matches the RFC §2 example and the field-order of the XML render. |

## Context

GitHub #78 / D-TEST-0073 is an S3 enhancement: phi-core's skill index is rendered into the system prompt only as an `<available_skills>` XML block (`SkillSet::format_for_prompt`, `src/context/skills.rs`). The AgentSkills standard specifies XML, but for token-budget-sensitive callers a lighter YAML layout drops the XML tag overhead while carrying the same `name`/`description`/`location` metadata triple. The request is to make the layout **selectable**, with XML remaining the byte-for-byte default so no existing consumer changes behaviour.

This is the kernel half of a coupled kernel→consumer pair: the *config/choice* surface (reading `[skills] prompt_format = "xml" | "yaml"` from operator config and calling the new entry point accordingly) belongs to i-phi #79 / D-TEST-0074, the downstream consumer — explicitly OUT of scope here per the phi-core kernel-minimality principle (`[[feedback_phi_core_kernel_minimal]]`). phi-core ships ONLY the general layout primitive; the consumer chooses.

The AgentSkills XML standard remains the **default** so that the historical render — and every consumer that relies on it — is unchanged unless it opts in.

## Sub-decisions

### §D4.1 — XML stays the byte-for-byte default (golden-pinned) (resolves part of F1)

Net-new selectable-layout surface; there is no prior behaviour to preserve beyond the existing XML render, which IS preserved verbatim. The existing `format_for_prompt()` XML output is preserved byte-for-byte: `format_for_prompt()` now delegates to `format_for_prompt_as(SkillPromptFormat::Xml)`, whose `Xml` match arm carries the historical body verbatim (same `<available_skills>` block, same two-space/four-space indentation, same `xml_escape` on name/description/location, same no-trailing-newline contract, same empty-set `""` early-return). A golden test pins `format_for_prompt()` to the exact bytes (assert the disposition, not merely a `contains` substring), so any future drift of the default render is caught.

### §D4.2 — F1.a `with_skills_format(set, format)` sibling constructor (resolves F1)

**Pre-existing-behaviour:** `BasicAgent::with_skills` shipped before KC-04; its signature and its system-prompt-append behaviour (append the rendered fragment with a blank-line separator when non-empty; otherwise leave the prompt unchanged) are preserved — now via delegation. `with_skills(set)` is refactored to `self.with_skills_format(skills, SkillPromptFormat::Xml)`, so the default path provably routes through the XML branch and stays byte-for-byte unchanged. The new `with_skills_format(set, format)` is a 2-arg sibling constructor placed immediately after `with_skills`; its body is `with_skills`'s body except the render line calls `skills.format_for_prompt_as(format)`. This is purely additive — one new public method, no removal.

### §D4.3 — F2.b conditional bare-vs-quoted YAML escaping (resolves F2; USER-DIVERGENT)

**USER-DIVERGENT from the planner-rec F2.a (always-quote).** User-lock rationale: *user-locked lighter output — bare scalars where plain-scalar-safe — accepting the explicit quote-trigger table as the cost.* Net-new surface — no prior YAML render existed; no prior behaviour to preserve.

The private `yaml_escape_scalar(s: &str) -> String` helper (sibling to `xml_escape`, single-file, `fn` not `pub`) decides **bare** vs **double-quoted** per YAML plain-scalar rules. It emits the **double-quoted + escaped** form when ANY of these quote-triggers fires — otherwise it emits the value **bare**:

- the string is **empty** (a bare empty scalar parses as YAML null);
- it has **leading or trailing whitespace**;
- it contains a `:`, `#`, **newline**, **tab**, **carriage-return**, `"`, `'`, or `\` anywhere;
- it **begins** with a YAML indicator char (`- ? : , [ ] { } & * ! | > % @ \`` or `~`);
- it is a **reserved / ambiguous plain scalar** — `true`/`false`/`null`/`yes`/`no`/`on`/`off`/`~` (any case);
- it **looks numeric / float / hex / octal / date-looking** (`42`, `-7`, `3.14`, `0x1F`, `0o17`, `2026-06-12`) so it survives as a STRING, not as a parsed number/date.

When quoting, it escapes (order matters, backslash first) `\`→`\\`, `"`→`\"`, newline→`\n`, tab→`\t`, cr→`\r`, then wraps in `"…"`. The load-bearing risk this lock accepts is that a *missed* trigger class is a latent invalid-YAML or — worse — a silent **type-change** defect (an unquoted `null` description parsing back as YAML null, or `42` as an integer). The §8 Tier C tests therefore assert BOTH branches: a `:`-containing value and a reserved-word/numeric value QUOTE and round-trip back as STRINGS through `serde_yaml`; a plain safe value (`weather`) stays BARE and still recovers.

### §D4.4 — F3 keys layout matches RFC §2 (resolves F3)

Net-new surface. The YAML block is a top-level key `available_skills:`, then a sequence (`- `) of mappings with keys `name` / `description` / `location` in that order (two-space indent for sequence items, four-space indent for item keys), mirroring the field-order of the XML render. Under F2.b each value renders bare when plain-scalar-safe and quoted otherwise. Empty set → `""` (same early-return as the XML branch); no trailing newline (matches the XML branch's contract).

### §D4.5 — kernel-minimality: phi-core owns the layout primitive; i-phi #79 owns the config/choice (boundary decision)

Net-new boundary decision. phi-core ships ONLY the general layout primitive — `SkillPromptFormat` enum + `format_for_prompt_as` renderer + `yaml_escape_scalar` helper + `with_skills_format` wiring — with ZERO consumer-specific leakage (no config struct, no `prompt_format` string-parse, no TOML field, no i-phi naming). The config/choice surface (reading `[skills] prompt_format = …` and selecting the format) is i-phi #79 / D-TEST-0074, NOT here, per `[[feedback_phi_core_kernel_minimal]]`. The escape helper stays phi-core-internal (`fn`), so a future flip back to always-quote (or a third style) is a private refactor with no public-surface commitment.

## Cross-references

- **(a) concept-docs**: `docs/concepts/skills.md:78-88` (the rendered `<available_skills>` XML example — gains a sibling YAML example + XML-default rationale; verified-header `:1` bumped) + `docs/specs/architecture.md:108` + `:593` (the `format_for_prompt()` narrative — amended to note the `format_for_prompt_as(format)` selector with XML default; verified-header bumped).
- **(b) closed drift/issue**: GitHub #78 / D-TEST-0073 (primary, S3). The phi-core half lands; #79 / D-TEST-0074 (the i-phi consumer config knob) is the follow-on, NOT closed by KC-04.
- **(c) prior ADRs as precedent**: `docs/decisions/0003-revert-tail-shrink-contract.md` (kernel-lane ADR shape; additive / investigation-first precedent) + `0002-…` + `0001-…`.
- **(d) forward-scope**: `docs/specs/plan/forward-scope/kc-04-selectable-skill-prompt-layout.md` + the cycle plan `docs/specs/plan/build/kc-04-selectable-skill-prompt-layout-91a175a4/plan.md`.

The AgentSkills XML standard (`https://agentskills.io/integrate-skills`) is retained as the **default** layout precisely so existing consumers and the standard's expectations are unchanged; YAML is opt-in only.

## Consequences

### For i-phi #79 / D-TEST-0074

The consumer config knob inherits a ready entry point: `BasicAgent::with_skills_format(set, SkillPromptFormat::Yaml)` (or `SkillSet::format_for_prompt_as(SkillPromptFormat::Yaml)` for a direct render). i-phi #79 reads `[skills] prompt_format = "xml" | "yaml"` from operator config, maps it to `SkillPromptFormat`, and calls `with_skills_format` accordingly. No phi-core change is needed to land #79 — this ADR is the kernel prerequisite, now satisfied. Forward-routing: #79 is the next chunk in this coupled pair.

### For baby-phi + future consumers

The opt-in layout lever is available for free on the next tracked phi-core commit — no wiring is added in baby-phi here (it inherits the new public surface when it tracks the commit). A future consumer wanting the lighter YAML layout calls `with_skills_format(set, Yaml)`; everyone else keeps XML by default with zero code change.

## Revisit triggers

1. **A quote-trigger class is found missing in the field** (a description that should have quoted renders bare and parses back as a non-string / invalid YAML) → **re-open §D4.3**: extend the trigger set, or fall back to the planner-rec F2.a always-quote discipline. (This is the highest-priority Revisit trigger under the F2.b USER-DIVERGENT lock — the missing-quote-trigger class is the accepted risk.)
2. A consumer needs even-lighter byte-budget output not achievable under the plain-scalar rules (e.g. block-scalar / folded styles) → re-open §D4.3 (block-literal styling was explicitly deferred).
3. A third layout (e.g. JSON / Markdown) is requested → re-open §D4.1 (extend the `SkillPromptFormat` enum).
4. The AgentSkills standard changes the XML default shape → re-open §D4.1 (the golden-pinned default would need re-pinning).
5. The `with_skills` ordering becomes a documented foot-gun for some consumer pattern → re-open §D4.2 (reconsider the F1.b setter shape).

## Verification

```bash
# Full suite (host cargo; single crate; -j 4 cap; NO --workspace).
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml

# Golden XML byte-for-byte pin + YAML round-trip (both branches) + empty-set both formats.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib kc04

# Wiring: with_skills_format(set, Yaml) reaches system_prompt; with_skills still emits XML.
/root/rust-env/cargo/bin/cargo test -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --lib kc04_with_skills_format

# Clippy (warnings = errors) + fmt.
RUSTFLAGS="-Dwarnings" /root/rust-env/cargo/bin/cargo clippy -j 4 --manifest-path /root/projects/phi/phi-core/Cargo.toml --all-targets
/root/rust-env/cargo/bin/cargo fmt --manifest-path /root/projects/phi/phi-core/Cargo.toml -- --check

# Kernel-minimality + forbidden-duplication greps.
grep -rn "available_skills:" /root/projects/phi/phi-core/src/                                            # only the YAML branch in skills.rs
grep -rnE "prompt_format|PromptFormat" /root/projects/phi/phi-core/src/ | grep -vi "SkillPromptFormat"   # 0 — no consumer config leakage
grep -n "yaml_escape_scalar" /root/projects/phi/phi-core/src/context/skills.rs                           # the private F2.b helper, single-file
```
