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
| `a79c7669` | [kc-01-parallel-revert-call-atomicity](kc-01-parallel-revert-call-atomicity-a79c7669/plan.md) | 5 | 2 (A+B) | pending | in-flight | pending |
