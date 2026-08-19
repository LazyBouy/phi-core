<!-- Last verified: 2026-08-19 by Claude Code -->

# KC-05 retro-context (for the next joint KC-batch retro)

**Cycle** `a0830ed3` · KC-05 progressive tool-catalog disclosure (#110+#109) · phi-core kernel · investigation=true · ALL planner-rec → Direct-approval-clean · A1·B1·C1 all PASS iter-1, 0 re-spawns · 589→610 tests · gate-4 GREEN (default + all-features).

## Observations worth carrying

1. **Resume-with-P0-already-committed.** KC-05's P0 was investigated + committed in a prior session (`42c4cbb`), then paused for the MA epic. On resume, gate-0.5 was re-run (spot-checked `streaming.rs:282-290` + `tool.rs:243-288` first-hand) rather than re-dispatching the failure-prone P0 agent. Pattern: a committed P0 is a durable artifact — re-verify it, don't re-run it. No forward-scope file existed (P0 scoped directly from the issues); drafted one inline (Option A) grounded on the P0 + the 2 pre-locked forks.

2. **Generalize-a-public-kernel-API-at-gate-4 (NEW pattern).** Audit-C surfaced that `is_mcp_adapter()` names a specific adapter in a general trait, while the config keys on "carries a large schema" (MCP + OpenAPI). Because the `AgentTool` trait method ships in the imminent **0.12.0** public API (renaming later = breaking), the orchestrator surfaced it to the user via AskUserQuestion rather than absorbing or deferring. User chose generalize → `has_large_schema()` + `engage_on_large_schema` + OpenAPI opt-in. **Candidate standard**: when an auditor flags a *public-kernel-API naming/shape* generalization on a not-yet-published surface, treat it as a user-domain decision (pre-publish is the free moment; post-publish is a breaking change), not an orchestrator-absorbed refinement.

3. **Daemon-less kernel close-gate when NO consumer can enable the ON-path (NEW pattern).** KC-05 ships progressive OFF-by-default and i-phi has no wiring to flip it yet, so the `phi-core-close-gate` skill's i-phi HTC path **cannot drive the ON-path** (the daemon only renders the OFF wire). Resolved by a **phi-core-native wire render**: a real `agent_loop` turn capturing `AgentEvent::TurnRequest.payload.tools` (the exact `Vec<ToolDefinition>` each provider serializes), asserting all 8 disposition rules — incl. a representative test proving the REAL `revert_to_state` 1870-char manual is absent from the progressive wire (fails on pre-KC-05 code). The **live-model** layer is OWNED by the i-phi follow-on (Rule 6), not re-deferred. **Candidate standard**: for a kernel chunk shipping a mechanism no consumer can turn on yet, the close-gate is a kernel-native rendered-wire disposition assertion + an OWNED live-model deferral to the enabling consumer chunk.

4. **Feature-gated modules escape the default MUST-RUN (NEW gap → standards candidate).** Adding the OpenAPI `has_large_schema` override surfaced 2 PRE-EXISTING latent openapi-feature bugs (a doctest that never compiled: `phi-core::`→`phi_core::`; `derivable_impls` on 2 enums) — invisible to the phi-core default MUST-RUN (`clippy --all-targets` default features). **Candidate standard**: the phi-core MUST-RUN (chunk-initiate Per-project config) should add `clippy --all-targets --all-features` + `test --all-features` (incl. doctests) **when the chunk touches a feature-gated module** (`openapi/`, or any future `#[cfg(feature)]` surface). Otherwise feature-gated code KC-05 edits is never gated. `OpenApiConfig` correctly kept its manual `Default` (non-zero field values) — derivable_impls only for zero-valued defaults.

5. **LOC deviation-log 4th occurrence (doc+test density).** `types/tool.rs` 5.48× / `tool_help.rs` 2.66× over §3 caps, all inline-test + doc-comment density with bounded logic → P-SEAL log per v28, no mid-flight pause. Durable: CH-05 D-1 / MA-04 D-1 / MA-08 D-2 → KC-05. The §3 cap-derivation keeps under-estimating substrate modules with heavy inline tests; v35 P-plan-2 ~1.3× multiplier is still too low for trait/registry modules.

6. **P-orch-8 skip-condition held (10th+ cycle).** All planner-rec + §1 populated → archived at iter-1, no iter-2 re-spawn. `chunk-template-validate-locked-appendix` PASS verified first-hand.

## Standards-update proposals drafted (NOT applied; joint-retro decides)

- **P1 (HIGH)**: phi-core MUST-RUN gains `--all-features` clippy + test + doctest when the chunk touches a feature-gated module (obs. #4). Update chunk-initiate Per-project config (phi-core row) + outer CLAUDE.md gate-4 phi-core MUST-RUN list.
- **P2 (MEDIUM)**: codify the "public-kernel-API generalization → user AskUserQuestion at gate-4" routing (obs. #2) — extends the architectural-refinement-at-approval-gate precedent (P-orch-4) to auditor-surfaced public-API naming.
- **P3 (MEDIUM)**: codify the daemon-less-kernel close-gate pattern (obs. #3) into the `phi-core-close-gate` skill (a "no consumer can enable it yet" branch: kernel-native render + owned live-model deferral).
- **P4 (LOW)**: revisit the §3 substrate-module LOC multiplier for trait/registry modules with heavy inline tests (obs. #5).

## Cycle-folder artifacts

`plan.md` · `p0-investigation.md` · `audit-{A,B,C}-iter1.md` · `cycle-audit.md` · `close-gate-result.md` · this file.

## Status

`audited-pending-retro`. Joint KC-batch retro pending. **Follow-on owed**: an i-phi per-agent progressive-policy drift/chunk that flips `progressive_tool_catalog.enabled` per agent + OWNS the live-model close-gate (Rule 6).
