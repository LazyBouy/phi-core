<!-- Last verified: 2026-06-08 by Claude Code -->

# phi-core kernel-chunk cycle index

Historical/audit ledger for **phi-core kernel chunks** (the `KC-NN` namespace) run through the multi-agent chunk pipeline (`/chunk-initiate project=phi-core`). One row per cycle. This is the durable "what kernel chunks have shipped" ledger; it is distinct from the GitHub issues (the per-defect tracking) and from baby-phi's / i-phi's cycle-indexes.

**Column semantics:**

- **Hex** — the 8-hex cycle token; the cycle folder is `<slug>-<hex>/`.
- **Slug** — the chunk slug (matches the forward-scope file + cycle folder).
- **Phases** — count of implementation phases in the plan §7.
- **Auditors** — audit-envelope size (1=small, 2=medium, 3=large).
- **Iterations** — audit-fix-loop iteration count per auditor at close (orchestrator fills at gate-4; `pending` until then).
- **Status** — `in-flight` → `audited-pending-retro` → `retro-complete` (orchestrator owns transitions).
- **Retro** — link to the retrospective / retro-context (or `pending`).

| Hex | Slug | Phases | Auditors | Iterations | Status | Retro |
|---|---|---|---|---|---|---|
| `a79c7669` | [kc-01-parallel-revert-call-atomicity](kc-01-parallel-revert-call-atomicity-a79c7669/plan.md) | 5 | 2 (A+B) | A1·B1 (both PASS) | retro-complete (close-gate PASS; #77 closed) | [joint KC-01..03](kc-03-revert-tail-shrink-contract-24de5309/retrospective.md) |
| `d835a363` | [kc-02-parallel-cluster-revert-matrix](kc-02-parallel-cluster-revert-matrix-d835a363/plan.md) | 4 | 3 (A+B+C) | A1·B1·C1 (all PASS) | retro-complete · ⚠️ SUPERSEDED — (B) semantics were a misread of the revert contract; close-gate validated the wrong invariant (no-400, not disposition). Re-opened as D-TEST-0076/#81; #80 closed but subsumed; corrected by KC-03. | [joint KC-01..03](kc-03-revert-tail-shrink-contract-24de5309/retrospective.md) |
| `24de5309` | [kc-03-revert-tail-shrink-contract](kc-03-revert-tail-shrink-contract-24de5309/plan.md) | 4 | 3 (A+B+C) | A1·B1·C1 (all PASS) | retro-complete (disposition close-gate PASS; #81 closed; #80 corrected) | [joint KC-01..03](kc-03-revert-tail-shrink-contract-24de5309/retrospective.md) |
| `91a175a4` | [kc-04-selectable-skill-prompt-layout](kc-04-selectable-skill-prompt-layout-91a175a4/plan.md) — XML-default + opt-in YAML skill-prompt layout (#78/D-TEST-0073 S3 — **CLOSED; live-validated via i-phi CC-28 `401c2723`**; F2=F2.b USER-DIVERGENT) | 4 | 2 (A+B) | A1·B1 (both PASS) | `retro-complete` (gate-4 PASS; +6 tests → 589; clippy/fmt clean; **#78 live-validated + closed**) | joint KC-04..CC-30b (`i-phi/docs/v0/proposal/plan/retros/retrospective-joint-91a175a4-to-73057f1a.md`) |
