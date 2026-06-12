# KC-04 — cycle-audit (gate-4 final re-audit)

**Cycle hex**: `91a175a4` · **Slug**: kc-04-selectable-skill-prompt-layout · **Project**: phi-core (kernel lane)
**Closes**: GitHub #78 / D-TEST-0073 (S3) · **Impl commit**: `65af84d` · **Locks**: F1=F1.a, **F2=F2.b (USER-DIVERGENT)**

## §0 — Critical findings

| Axis | Result |
|---|---|
| Build (clippy `-Dwarnings`) | ✅ clean (24.6s) |
| Tests | ✅ 589 passed / 0 failed (band [589,592]) |
| fmt --check | ✅ exit 0 |
| Kernel-minimality | ✅ PASS (additive-only; zero consumer leakage; helpers private) |
| Audit A (code+tests) | ✅ PASS 8/8 |
| Audit B (docs+ADR) | ✅ PASS 8/8 |
| Live close-gate (model sees YAML) | ✅ **PASS** (2026-06-12) — live-validated via i-phi CC-28 (`401c2723`) `[skills] prompt_format="yaml"`: a real model (deepseek-chat-v3-0324) received the YAML `available_skills:` block on the wire + parsed it. Evidence: i-phi `docs/e2e-test/cycles/cc28-close-gate-401c2723/` |
| **Overall** | ✅ **PASS — #78 live-validated + CLOSED** (via i-phi CC-28) |

## §1 — Audit-pipeline summary

| Auditor | Focus | Iter | Verdict |
|---|---|---|---|
| A | code + tests + F2.b escaping + kernel-minimality | 1 | PASS (8/8; Claim 7 MUST-RUN orchestrator-closed) |
| B | docs + ADR + rustdoc + verified-headers | 1 | PASS (8/8) |

Zero re-spawns. Zero findings to triage.

## §2 — Diff review

`65af84d` — 9 files, +913/−26.
- **`src/context/skills.rs`** (+387): `SkillPromptFormat { Xml, Yaml }` enum (`#[default] Xml`); `format_for_prompt_as(format)` dispatcher (XML arm = historical body verbatim, only re-indented; YAML arm = `available_skills:` mapping+sequence, no trailing newline, empty→`""`); `format_for_prompt()` delegates to XML. F2.b helpers (all private `fn`): `yaml_escape_scalar` + `yaml_scalar_needs_quoting` + `looks_numeric_or_date` — exhaustive quote-trigger set. 6 new inline tests.
- **`src/agents/basic_agent.rs`** (+58/−): `with_skills_format(set, format)` sibling constructor; `with_skills` delegates with `Xml`. Tier-E wiring test.
- **`src/lib.rs` / `src/context/mod.rs`**: `SkillPromptFormat` re-exported alongside `SkillSet`.
- **Docs**: ADR-0004 (105 lines, 7 sections, §D4.1–§D4.5); `concepts/skills.md` YAML example + XML-default rationale; `architecture.md:108/:593` selector note; both verified-headers bumped 2026-06-12.
- **Paperwork**: plan.md archive + cycle-index row (bundled in the chunk commit — acceptable for phi-core lane).

XML default byte-for-byte preserved (golden `assert_eq!`). F2.b USER-DIVERGENT honored in code (conditional escaping), tests (C-2 reserved-word type-preservation MUST-SHIP), and docs (§D4.3 + rustdoc).

## §3 — MUST-RUN evidence (orchestrator gate-4, authoritative)

```
cargo test -j 4 --manifest-path …/phi-core/Cargo.toml
  → lib 260 + integration binaries; SUM = 589 passed; 0 failed; 5 ignored (live-API)
RUSTFLAGS="-Dwarnings" cargo clippy -j 4 --manifest-path … --all-targets
  → Finished `dev` in 24.60s (no warnings, no errors)
cargo fmt --manifest-path … -- --check  → exit 0
```
Kernel-minimality greps: single `available_skills:` emitter (skills.rs:300); `prompt_format|PromptFormat` minus `SkillPromptFormat` = 0 hits (zero consumer leakage); re-exports present at lib.rs:71 + context/mod.rs:37.

**Guard-absence note**: phi-core ships no `scripts/check-*.sh` — the MUST-RUN list (clippy + test + fmt) IS the gate, per the kernel-lane standard.

## §4 — Iteration accounting

- Plan iterations: **2** (iter-1 planner-rec draft; iter-2 narrow re-author for F2=F2.b USER-DIVERGENT, F1+everything-else verbatim — P-orch-8 skip did NOT apply due to the divergent lock).
- Audit iterations: **A1 · B1** (both PASS at iter-1). Zero Tactical/Architectural/Trivial findings. Zero orchestrator patches.

## §5 — Paperwork ledger

| Artifact | Status |
|---|---|
| Forward-scope file | ✅ committed prerequisite `6c9f845` |
| Plan archive (`plan.md`) | ✅ `91a175a4/plan.md` |
| Cycle-index row | ✅ appended (Status `in-flight` → flips at Phase 7) |
| ADR-0004 | ✅ Accepted, 7 sections, §D4.1–§D4.5 |
| Drift #78 / D-TEST-0073 | ✅ **CLOSED** (2026-06-12) — live-validated via i-phi CC-28 (`401c2723`): the model received + parsed the YAML `available_skills:` block on the wire (`source: wire — exact`). The deferred live close-gate ran + PASSed via the i-phi `[skills] prompt_format="yaml"` config path. |
| Verified-headers (skills.md, architecture.md) | ✅ bumped 2026-06-12 |
| rustdoc (3 new public items) | ✅ present |
| audit-a/b-iter1.md | ✅ written |

## §6 — Deviations

- **Plan iter-2 F2 USER-DIVERGENT** — user locked F2.b (conditional bare-vs-quoted) over planner-rec F2.a (always-quote); not a deviation, a routed lock. Re-author was F2-only; F1.a + all else verbatim.
- **Plan + cycle-index bundled into chunk commit `65af84d`** (vs a separate archive commit) — acceptable for the phi-core lane; no separate-commit requirement.
- **F2.b helper split into 3 private fns** (`yaml_escape_scalar` / `yaml_scalar_needs_quoting` / `looks_numeric_or_date`) — readability refactor within the locked `fn yaml_escape_scalar` contract; not a semantic deviation.
- No audit-prompt-authoring miss (§11 Audit-A test count correctly updated to +6/589 at iter-2).

## §7 — Metrics

- Test count: 583 → **589** (+6, exact plan §8 match).
- phi-core public surface: +1 enum (`SkillPromptFormat`) + 2 methods (`format_for_prompt_as`, `with_skills_format`); all additive.
- New deps: **0** (`serde_yaml` already present).
- Disk reclaimed (placement-1 during cycle): ~11 GiB across cargo-clean invocations; gate-5 final clean logged at Phase 5.
