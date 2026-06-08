<!-- Last verified: 2026-06-08 by Claude Code (KC-01 live close-gate) -->

# KC-01 — live close-gate result (#77 / D-TEST-0072)

## §0 — Verdict

**PASS** — the #77 BROKEN scenario (pinned revert onto a parallel multi-tool-call cluster) was reproduced **live** against a real provider and rendered call-atomic with **no provider 400**. Validated by reading the rendered transcript per `[[feedback_render_transcript_close_gate]]`.

## §1 — What ran

- **HTC**: `KC01-CLOSEGATE` (i-phi e2e harness, phi-e2e worktree `dev-v0-e2e`), built against KC-01 phi-core (`dev` `46f7854`, path-patched dependency).
- **Driver**: `deepseek/deepseek-chat-v3-0324` (strongest-available open-source per `[[feedback_htc_cohort_strongest_open_source]]`; proven parallel in HTC-0009).
- **Fixture**: seeded short-term memory record + `release-checklist` skill; `revert_to_state` + `write_file` allowed; `memory_help`/`skill_help` permission-exempt; sedimentative; transcript=true.
- **Flow** (single `/send-input`, agent loops internally): STEP 1 fire `memory_help` + `skill_help` in parallel → STEP 2 pinned `revert_to_state(completion)` into the cluster → STEP 3 `write_file` the answer.

## §2 — Transcript evidence (read)

- **Parallel cluster**: TURN 1 fired **two** parallel tool_calls (`memory_help` + `skill_help`); TURN 2 input rendered them as `[n0]` assistant (both calls) + `[n1]` memory result (`call_06241…`) + `[n2]` skill result (`call_8772…`) — the exact #77 shape.
- **Pinned revert into the cluster**: the model called `revert_to_state {category:"completion", step:"n0", summary:…}` — landing onto the multi-call assistant node = the §3 matrix **F-5 BROKEN row** (the worst case).
- **Post-revert request accepted (the load-bearing proof)**: TURN 3 — the request the daemon sent AFTER the pinned revert to n0 — rendered `[n0]` (with the `[outcome: …]` breadcrumb) + `[n1]` memory result, **balanced and accepted, no 400**. KC-01's per-call retain kept the memory call + its direct-child result and dropped the abandoned skill call (whose result was not a direct child of n0) — no dangling `tool_use`. The model then re-fetched the abandoned skill and issued several MORE pinned reverts into the cluster; **every post-revert request was accepted**.
- **Clean termination**: `AgentEnd` with `rejection: null`; `write_file` executed (`ToolExecutionEnd` ×8); daemon log carries **no** 400 / orphan / dangling / provider-error.

## §3 — Why this is the real close

Pre-KC-01, the TURN-3 post-revert-to-n0 request carries a dangling `skill_help` tool_call (only the first call's result was recoverable via the first-call-only repair) → Anthropic/OpenAI-compat **400**. The render-pass call-atomic fix (R1–R4 + the `streaming.rs` backstop) makes that request balanced; the live provider accepted it. Unit tests proved the trunk shape; this proves the end-to-end wire is accepted by a real provider.

## §4 — Model-behaviour note (NOT a KC-01 defect)

deepseek looped mildly — it re-applied the "seal" instruction across several turns (repeated reverts to n0, re-fetching the abandoned skill) before delivering `write_file`. This is driver instruction-following behaviour, not a render defect; it actually **strengthens** the gate (multiple distinct pinned-revert-into-cluster requests, all rendered call-atomic + accepted). Duration 36,965 ms; clean convergence to `AgentEnd`.

## §5 — Artifacts

In the i-phi phi-e2e worktree under `docs/e2e-test/cycles/kc01-close-gate-a79c7669/`:
- `transcripts/KC01-CLOSEGATE.transcript.txt` (482-line product wire render)
- `scripts/{setup-kc01-closegate.sh, prompt-kc01-closegate.txt, run-harness-htc.sh, assert-htc.py}`
- `sessions/KC01-CLOSEGATE.session.json` · `events/` · `assertions/` · `stdout-stderr/`
- test-case: `docs/e2e-test/test-cases/KC01-CLOSEGATE.md`
