# KC-04 — retro-context (deferred to future KC joint-retro batch)

**Cycle**: `91a175a4` · phi-core kernel lane · closes #78 / D-TEST-0073 (S3) · 2026-06-12
**Status**: audited-pending-retro · joint-retro pending (first KC cycle since the KC-01..03 joint-retro)

## Cycle context

- Purely-additive S3 enhancement: selectable skill-prompt layout (XML default byte-for-byte unchanged + opt-in YAML). The phi-core half of a coupled kernel→consumer pair; consumer #79 / D-TEST-0074 is the next cycle.
- `investigation=false` (mechanical/low-uncertainty opt-out per KC-#3 — not S2+ correctness, not ADR-supersede, not fundamental). Surface pre-grounded by the orchestrator before initiate. The opt-out held: zero re-plans from wrong hypotheses, 10/10 planner confidence, both auditors PASS at iter-1.
- 2 plan iterations (iter-1 planner-rec; iter-2 narrow F2-only re-author for the F2.b USER-DIVERGENT lock). A1·B1 both PASS. Zero orchestrator patches.

## Observations worth carrying

- **F2.b USER-DIVERGENT narrow re-author worked cleanly.** User locked the *lighter-output but higher-edge-case* option (conditional bare-vs-quoted) over the planner-rec safe option (always-quote). The iter-2 re-author touched ONLY F2-dependent sections (§1 F2 body, §6 close-gate, §8 Tier-C split, §5 ADR §D4.3, §3.E/§12 helper rename); F1.a + everything else stayed verbatim. This is the v32 USER-DIVERGENT narrow-re-author path applied to the kernel lane — confirms it generalizes beyond i-phi/baby-phi. (Mirrors the cross-cycle fork-divergence observation: i-phi/phi-core users prefer tighter/lighter options on TECHNICAL forks.)
- **USER-DIVERGENT on a TECHNICAL fork raised a real correctness surface.** F2.b's "load-bearing surface is the trigger set itself" — a missed reserved-word/numeric trigger is a silent YAML *type-change* defect (unquoted `null`→YAML null, `42`→integer). The orchestrator's iter-2 re-spawn prompt explicitly promoted the C-2 reserved-word-type-preservation test from MAY-COVER to MUST-SHIP, and the implementer covered it. Lesson: when a TECHNICAL fork lock trades safety for output-economy, the iter-2 re-author should harden the test that guards the traded-away safety margin.
- **No close-gate live-transcript needed for a render-only model-facing change.** Per `[[feedback_render_transcript_close_gate]]` Rule 4, the planner correctly judged that the rendered string IS the wire (YAML reaches the provider identically to XML — pure system-prompt text, no provider re-encoding, no tool interaction). Unit golden (XML byte-for-byte) + serde_yaml round-trip (fields + STRING type recover) + wiring test fully discharge the contract. Contrast KC-02/KC-03 which needed a live provider repro because the contract was a model-visible *revert disposition*. Useful precedent for "when is unit-level sufficient for a model-facing fix".
- **Plan + cycle-index row bundled into the chunk commit** (phi-core lane) vs the i-phi/baby-phi separate-archive-commit habit. Acceptable; no requirement to separate on the kernel lane. Noted in cycle-audit §6.
- **serde_yaml already a direct dep** — the planner verified this at draft time (Cargo.toml:35), resolving the only dependency-cascade question to zero-new-deps before archive (P-orch-2 grounding paid off).

## Standards-update candidates drafted (NOT applied; joint-retro decides)

1. **(LOW) Codify the "TECHNICAL-fork USER-DIVERGENT → harden the traded-away-safety test at iter-2" pattern.** When a USER-DIVERGENT lock on a TECHNICAL fork trades a safety margin for economy (here: always-quote → conditional-quote, trading invalid-YAML-immunity for fewer bytes), the iter-2 narrow re-author should explicitly promote the guarding test to MUST-SHIP. Candidate home: chunk-planner iter-2-reauthor guidance OR a one-line note in the fork-divergence observation. Single data point so far — watch for a 2nd before codifying.
2. **(LOW) "Render-string-IS-the-wire" close-gate exemption.** Document that a model-facing render change whose output reaches the provider as plain system-prompt text (no provider re-encode / no tool interaction) discharges Rule 4 at the unit level (golden + parser round-trip + wiring), no live-transcript needed — distinct from disposition-mediated fixes (KC-02/03). Candidate home: `[[feedback_render_transcript_close_gate]]` clarifying note OR the phi-core-close-gate skill. Defer — may be obvious enough to not need codifying.

## Cycle-folder artifacts

- `plan.md` (iter-2 archived) · `cycle-audit.md` · `audit-a-iter1.md` · `audit-b-iter1.md` · this `retro-context.md`
- Forward-scope: `../forward-scope/kc-04-selectable-skill-prompt-layout.md`
- ADR: `../../../decisions/0004-selectable-skill-prompt-layout.md`
- Commits: `6c9f845` (FS) · `65af84d` (impl) · `1a496c0` (gate-4 paperwork)
