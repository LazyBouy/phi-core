<!-- Last verified: 2026-08-19 by Claude Code -->

# KC-05 cycle-audit (gate-4 final re-audit) — `a0830ed3`

**Chunk** KC-05 — progressive tool-catalog disclosure + tool-registration contract (#110 + #109). **Project** phi-core (kernel lane). **investigation=true** (P0-grounded). **Fork-lock** ALL planner-rec → Direct-approval-clean (P-orch-8 skip fired; archived at iter-1, no iter-2 re-spawn).

## §1 — Audit-pipeline summary

| Auditor | Focus | Iter | Verdict |
|---|---|---|---|
| A | code + tests + kernel-minimality | 1 | **PASS** (8/8) |
| B | docs + ADR-0005 + verified-headers + cycle-index | 1 | **PASS** (4/4) |
| C | cross-cutting backcompat + provider serialization + bridge inheritance + wire disposition + design scrutiny | 1 | **PASS** (5/6; claim 6 = live close-gate, orchestrator-owned) |

All three PASS at iter-1. **Zero re-spawns.** Orchestrator spot-checked: `streaming.rs:282-290` OFF-arm byte-identity, `tool.rs` trait methods, `config.rs` `engages()`, provider diffs test-only, golden-OFF test existence — all confirmed first-hand.

## §2 — Diff review (high level)

- **Kernel mechanism**: 3 default trait methods (`short_description`/`detailed_description`/`has_large_schema`) + `SHORT_DESCRIPTION_MAX_CHARS=256` + `validate_tool_registration` + `ToolRegistrationError` (`types/tool.rs`); config-gated reduced-render branch (`streaming.rs`); `ProgressiveToolCatalog` field (`config.rs`, OFF default, 31-site cascade); catalog-backed `tool_help` shared-slot snapshot (`tool_help.rs` + `basic_agent.rs`); `revert_to_state` description split (`revert.rs`); MCP + OpenAPI `has_large_schema` overrides.
- **0 provider production change** (F4.b stub keeps the `ToolDefinition` wire type; provider additions are `#[cfg(test)]` only). **0 new MCP code** (`McpToolAdapter` mapping automatic).
- 6 docs + ADR-0005 (Accepted) + verified-headers 2026-08-19.

## §3 — MUST-RUN evidence (orchestrator gate-4, authoritative — host cargo)

phi-core has **no `check-*.sh` CI guards**; the MUST-RUN IS the gate. Sub-agents marked cargo `NOT-EXECUTED-IN-AUDIT`; closed here.

```
fmt --check ................................. GREEN (exit 0)
clippy --all-targets -Dwarnings (default) ... GREEN (exit 0)
clippy --all-targets --all-features -Dw ..... GREEN (exit 0)
cargo test (default) ........................ 610 passed / 0 failed / 5 ignored
cargo test --all-features (incl doctest) .... 652 passed / 0 failed
close-gate wire-render test .................. PASS (close_gate_real_revert_manual_absent_and_wire_reduced)
```

**Kernel-minimality** (substitute for phi-core-leverage-check, N/A on the kernel): forbidden-leakage grep clean in the new surface (the 1 hit is pre-existing KC-01/02/03 `revert_render_policy` doc, not KC-05). Only general primitives added; additive public API; no migration/breaking change; all 12 kernel + 3 i-phi impls compile via default methods. **k8s-readiness N/A** (library).

## §4 — Iteration accounting

- **Audit-fix iterations: 0** (A/B/C all PASS iter-1).
- **Trivial-1L**: plan §1/§9 stale P0-path (`_p0-investigations/…` → `p0-investigation.md`, 2 refs) — orchestrator-fixed at gate-4 (Audit-B observation); direct-verified, no re-spawn.
- **User-directed gate-4 refinement (NOT an audit-fix iteration)**: the `is_mcp_adapter→has_large_schema` + `engage_on_mcp→engage_on_large_schema` generalize + OpenAPI override — see §6 D-1.
- **Route-A absorption**: 2 pre-existing latent openapi fixes — see §6 D-2.

## §5 — Paperwork ledger

| Artifact | Status |
|---|---|
| `plan.md` (iter-1, P-orch-8 skip) | ✅ archived |
| `p0-investigation.md` (moved from `_p0-investigations/` at archive) | ✅ |
| `ADR-0005` (`docs/decisions/0005-…`) | ✅ Accepted; 7 sections; §D5.1–§D5.7 disposition notes; §D5.1 records the generalize |
| `close-gate-result.md` | ✅ 8-rule disposition table on the rendered wire |
| `cycle-audit.md` (this) | ✅ |
| cycle-index row `a0830ed3` | ✅ `audited-pending-retro`; A1·B1·C1 |
| 6 docs + verified-headers 2026-08-19 | ✅ concepts/tools · architecture/streaming · developer/tool · reference/{configuration,api,glossary} |
| audit-A/B/C-iter1.md | ✅ |

## §6 — Deviations

| # | Class | Description | Disposition |
|---|---|---|---|
| **D-1** | User-directed architectural refinement at gate-4 (Audit-C rec → AskUserQuestion "Generalize now") | `is_mcp_adapter()`→`has_large_schema()` + `engage_on_mcp`→`engage_on_large_schema` (public trait API generalized to a schema-magnitude signal); `OpenApiToolAdapter` opts in. Mildly broadens the locked F5-rec (OpenAPI agents now also engage) — rationale-aligned (magnification), user-approved. ADR §D5.1 records it. Pre-publish (0.12.0) is the free moment to rename a public trait method. | Applied + gate-4 re-verified GREEN (default + all-features). |
| **D-2** | Route-A absorption (pre-existing latent, surfaced by the OpenAPI override under `--all-features`) | 2 pre-existing openapi-feature bugs NOT caused by KC-05: (a) a doctest that never compiled (`phi-core::` → `phi_core::` in `openapi/mod.rs`); (b) `derivable_impls` clippy on `OperationFilter` + `OpenApiAuth` enums. `OpenApiConfig` correctly KEEPS its manual `Default` (non-zero values). Fixed to keep the feature green now that KC-05 touches it. | Fixed; `--all-features` clippy + doctest now GREEN. Note: openapi is outside phi-core's default CI gate → these were latent since before KC-05. |
| **D-3** | Audit-prompt-authoring miss (informational) | Gate-3 6-axis cross-check: lock-body axis DIVERGENT on all 3 prompts (prompts cite forks by F-token + semantics, not verbatim lock-body sentences). f-token/test-name/method-sig/arg-shape/literal-count all PASS. Manually confirmed each fork description semantically faithful to plan §1. | Suppressed as informational (MA-08 precedent). |
| **D-4** | LOC deviation-log (chunk-implementer v28 P-impl-1-v28) | `types/tool.rs` 5.48× / `tool_help.rs` 2.66× / `revert.rs` 2.15× / `config.rs` 1.55× over §3 caps — provably inline-test + doc-comment density with bounded logic (no mid-flight `>2×` pause; correct per v28). **4th occurrence** of the class (CH-05 D-1 / MA-04 D-1 / MA-08 D-2 → KC-05). | Accepted; logged. |
| **D-5** | Close-gate surface deviation (Rule 8 / Rule 6) | phi-core is daemon-less + no consumer can enable progressive yet, so the i-phi HTC harness cannot drive the ON-path. Close-gate is phi-core-native (rendered `TurnRequest.tools` wire, 8 disposition rules). The live-model end-to-end layer is **OWNED by the i-phi per-agent policy follow-on** (Rule 6) — must run on the i-phi PRODUCT wire once the flag is wired. | Documented in `close-gate-result.md`; follow-on drift to be filed on i-phi. |

## §7 — Metrics

- **Test delta**: 589 (post-KC-04) → **610** default (+21; plan band [609, 615]); 652 under `--all-features` (openapi + doctest).
- **phi-core import baseline**: N/A (KC-05 IS the kernel; kernel-minimality surface check applied instead).
- **Cascade**: `AgentLoopConfig {` = 34 grep hits (31 construction sites; well under the >47 pause threshold).
- **Disk reclaimed** (gate-4/verify cleans): ~10.3 GiB (final) + ~1.5 + ~10.1 GiB across intermediate runs.
- **phi-core crates.io**: local `0.11.4`; KC-05 lands toward the pending **0.12.0** release (KC-05 = last kernel chunk in the agreed sequence before the version cut).

## §8 — Issue closure

- **#110 + #109**: closed **by-construction** in-repo (ADR-0005 + concept/reference docs). GitHub-tracker closure (LazyBouy/i-phi) **pending the user-gated commit** (mirrors the MA-08 flow).
- No new D-TEST drift filed for KC-05 itself (no defects surfaced). A **follow-on i-phi drift** (per-agent progressive policy + owned live-model close-gate) is owed — surfaced at the Phase-5 pause.
