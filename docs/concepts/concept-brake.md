<!-- Last verified: 2026-05-22 by Claude Code (concept exploration; not yet a binding spec) -->

# Context Brake — concept exploration

**Status**: `[CONCEPTUAL]` — exploration of the design space; no commitments yet.
**Origin**: Triggered 2026-05-21 by user analogy *"driving a car with only the gas pedal and no brakes"* during a plan-mode discussion about phi-core 0.8.0 (`compress_and_reload` tool + async-trait `BlockCompactionStrategy`).
**Scope**: i-phi's context-management posture; phi-core implications considered separately.
**Related docs**: [`compaction.md`](compaction.md), [`memory.md`](memory.md), [`sessions.md`](sessions.md).
**Related drifts**: [`D-CH16-PHICORE-COMPRESS-RELOAD.md`](../design/drifts/D-CH16-PHICORE-COMPRESS-RELOAD.md), [`D-CH16b-FOLLOWUP-01-phi-core-async-strategy.md`](../design/drifts/D-CH16b-FOLLOWUP-01-phi-core-async-strategy.md).

---

## 1. Problem statement

phi-core's current context-management posture has **a gas pedal but no brake pedal**: in-run context grows monotonically as turns accumulate; the only mechanism to slow growth is a token-budget-overflow trigger that fires automatic compaction at the END of growth (when budget is already at limit). There is **no mechanism that recognises *wasteful* growth** — a sequence of steps that produced no progress, an exploration that hit a dead end, a tool result that turned out to be irrelevant — and trims those proactively.

The user's framing (verbatim, 2026-05-21):
> *"Currently the context just grows even if a step is not producing any value and there is no mechanism to trim or prun or compress the context to avoid the growing context. It's like driving a car with only the gas pedal and no brakes."*

The gap is **decoupling compaction trigger from token budget**. Token-threshold triggers are reactive to *size*; what's missing is triggers reactive to *quality*.

## 2. The hidden assumption discovered en route

The conversation initially explored phi-core's drafted `compress_and_reload` model-callable tool + the existing `prun` tool — both 2-phase deferred-apply tools that take a `tokens: integer` parameter the LLM specifies. **Critical analysis surfaced a load-bearing assumption: that LLMs can accurately locate token-level positions in their own context.**

Empirical reality:
- LLMs cannot count their own tokens (~10-40% estimation error; no introspection-time tokenizer).
- LLMs cannot specify token-precise offsets ("remove tokens 12,500–13,200" is incoherent to them).
- LLMs degrade reading their own middle context (Liu et al., 2024 NeurIPS "lost in the middle").
- LLMs cannot predict their own future needs (don't know the user's next message at compression time).

The 2-phase tools paper over this by treating `tokens: integer` as a **fuzzy budget hint** that the system rounds to message-boundary granularity. The LLM isn't actually positioning; the SYSTEM is positioning. The integer-typed parameter is an architectural lie.

This shifts the design question from *"how does the LLM compress its own context?"* to *"what's a brake architecture that doesn't depend on LLM self-introspection?"*

## 3. The reflective-agent design (initial proposal)

User's proposal (verbatim, 2026-05-21):
> *"Although llms cannot see their context but it can perform surgical operation on files. So the context (not the system prompts, identity etc., but what happens during the a turn or turns) gets stored in a file exactly as it gets loaded into the context. During reflective monitoring the main agent passes the task and the context file to another agent to judge what to prun or whether / how there should be compression. The other agent is an incognito agent that checks and creates a trimmed context file from the original, keeping a backup of the original (if that helps). The calling agent just loads the context in the next turn and proceeds."*

**Why this is structurally sound** (full analysis preserved in conversation log):
- Externalises context to a FILE — sidesteps positional blindness (LLMs are good at file editing; bad at memory introspection).
- Bias-decouples — fresh incognito sub-agent doesn't have the main agent's self-preservation bias.
- Tool-mediated decisions are auditable (each Edit/Read is a discrete operation in the tool log).
- Composes existing phi-core primitives (`SubAgentTool`, `agent_loop_parallel`) — no new architecture.
- The context file format already exists: phi-core's `FileSystemSessionStore` writes `LoopRecord.messages` JSONL per session.

**Drawbacks** (preserved verbatim from analysis):
1. **Latency at compaction boundary** — 5-30s for sub-agent multi-turn loop.
2. **Cost amplification** — sub-agent processes the FULL context as INPUT to decide what to drop; worst case may cost more tokens than it saves on the next turn.
3. **Sub-agent prompt is now a load-bearing surface** — bad prompt = bad compression = silent semantic loss.
4. **Doesn't fundamentally solve the judgment problem** — relocates the decision; doesn't guarantee correctness.
5. **Future-prediction is still impossible** — neither agent knows what the user will say next.
6. **Semantic drift across compactions** — compress → compress → compress is lossy each pass.
7. **Atomicity / mid-edit failure modes** — corrupt context file on crash; needs atomic-rename + rollback discipline.
8. **Recursive compression risk** — sub-agent must NOT have access to its own compaction tools.
9. **No "feel" for main agent's reasoning state** — implicit hypotheses being formed may die when their birth-discussion is dropped.
10. **What's reflectable vs invariant boundary needs codification** — system prompt + identity + memory tier stay; in-turn working memory is reflectable.

**User's response to latency concern** (verbatim, 2026-05-21):
> *"My reasoning is that latency should be fine because compress or prun would not happen often only for long running larger cycles."*

This pre-validates the latency cost for long-running session use cases (which is the intended i-phi posture).

## 4. The full design space — 3 orthogonal axes

Any brake architecture picks one (or combines several) from each axis:

### Axis 1: Triggers (the WHEN)

| ID | Trigger | Pros | Cons |
|---|---|---|---|
| **T1** | Token-threshold (current phi-core; fires at X% of context budget) | Simple; deterministic | Fires regardless of work quality; can't tell stall from progress |
| **T2** | Turn-count (every N turns) | Even simpler than T1 | Same blind-to-quality problem |
| **T3** | Per-step value-score (rate each turn 1-5; brake when avg low) | **Directly addresses the user's "no brakes" framing** | Requires a value-scoring oracle (rules or LLM); novel; not in production state-of-the-art |
| **T4** | Stall detection (last K steps produced no state change / no new info) | Targeted at the exact failure mode | "Progress" is fuzzy to define; risks false positives in legitimate exploration |
| **T5** | Anticipatory (predict overflow before next turn) | Smoother growth profile | Requires accurate token forecasting |
| **T6** | Manual / user-initiated | User has full control | Burden on user; only works for interactive UIs |
| **T7** | Background continuous (always running; smooth eviction over time) | No spikes; smooth | Constant token overhead |
| **T8** | Hybrid T1 + T3/T4 ("fire on token threshold OR on stall, whichever first") | Captures both failure modes | More complex decision logic |

**T3 / T4 / T8 are the candidates that directly address the "no brakes" gap.** T1/T2 are the current world.

### Axis 2: Decision mechanism (the WHO / HOW chooses what to remove)

| ID | Mechanism | Pros | Cons |
|---|---|---|---|
| **D1** | Hard-rule heuristic (current phi-core `DefaultBlockCompaction`; e.g., drop tool outputs > X lines) | Zero LLM cost; deterministic | Blunt; can't judge semantic relevance |
| **D2** | System picks range, LLM summarises (CH-16b's planned `IphiBlockCompactionStrategy`) | One LLM call per compaction; better than heuristic | LLM has no input on WHAT to drop; only HOW to summarise |
| **D3** | Main agent self-directs (`prun`; drafted `compress_and_reload`) | Agent has task context | LLM positional blindness; can't introspect own context — see §2 |
| **D4** | Reflective sub-agent with file-tools (the just-proposed design) | Bias-decoupled; tool-mediated; surgical | Multi-turn latency; sub-agent prompt is a load-bearing surface |
| **D5** | Value-scoring oracle decides | Targeted at "drop wasteful steps" | Requires defining "value"; LLM-judged (cost) or rule-judged (brittle) |
| **D6** | Retrieval-based (drop old turns; RAG-retrieve at each turn) | Industrial pattern (Cursor / Cody) | Different failure modes (retrieval miss); vector-store infra |
| **D7** | Pinning (agent/user explicitly marks important; rest is droppable) | Conservative; recoverable | Requires pinning UX; agent must learn to pin |

### Axis 3: Preservation strategy (WHAT happens to removed content)

| ID | Strategy | Pros | Cons |
|---|---|---|---|
| **P1** | Hard drop | Maximum savings | Lossy; not recoverable |
| **P2** | Replace with one-line summary (current phi-core `keep_compacted`) | Some recovery via summary | Limited fidelity |
| **P3** | Move to memory tier (extract long/short-term records → drop raw turns) — CH-16b mechanism | Memory persists across sessions | Requires memory pipeline; recall is keyword-grep at v0 |
| **P4** | Move to backup file (replayable; the reflective approach uses this) | Full recovery possible | Storage cost; replay complexity |
| **P5** | Tool-output-only truncation (keep the call, drop the bytes) | Conservative; preserves conversation flow | Saves less per compaction |
| **P6** | Hierarchical roll-up (oldest → episode → super-episode; CH-16b's F4.b) | Bounded growth without complete loss | Cumulative semantic drift across compactions |
| **P7** | Externalise to vector store | Industrial; sub-second retrieval | Vector-store dependency; RAG infra |

## 5. Pre-built compositions (the catalogue)

Coherent combinations from the 3-axis matrix. Each addresses different priorities.

### Composition A — Current phi-core baseline (status quo)
**T1 token threshold + D1 heuristic + P2 summary.**
- Already shipped in phi-core 0.7.x.
- Blind to step quality.
- Baseline; the gap is here.

### Composition B — CH-16a + planned CH-16b (in the existing roadmap)
**T1 threshold + D2 system-picks-LLM-summarises + P3 memory-extract + P6 hierarchical roll-up.**
- Memory extraction salvages important facts.
- Super-episodes bound long-session growth.
- Still blind to step quality at trigger time.
- Status: structurally landed at CH-16a + CH-16b; LLM bodies deferred via D-CH16b-FOLLOWUP-01.

### Composition C — Reflective sub-agent (just-discussed)
**T1 threshold + D4 reflective sub-agent + P4 backup file.**
- Sidesteps LLM positional blindness via file-tools.
- Quality potentially highest of size-triggered designs.
- Latency cost (acceptable per user 2026-05-21).
- **Still doesn't directly address "a step produced no value"** — fires on size, not quality.

### Composition D — Value-scoring brakes (novel; directly addresses the framing)
**T3/T4 stall-detection trigger + D5 value-scoring + P1/P3 drop or memory-extract.**

Mechanics sketch:
- After each turn, compute a step-value score. Options for the score:
  - **Rule-based**: did any file get edited? did a tool succeed? did the assistant produce new task-relevant content (heuristically)? did the user message indicate dissatisfaction?
  - **LLM-judged** (cheap model): one-shot prompt: *"Rate this step 1-5: did it move the task forward?"*
  - **State-diff-based**: did the workspace state (files, memory, session metadata) change in non-trivial ways?
- Maintain rolling window of recent step-values.
- When rolling average drops below threshold for K consecutive turns → fire compaction.
- Decision: drop the low-value turns first; preserve high-value ones.

**This is the brake pedal the user is describing** — the system actively recognises "the last 5 steps weren't going anywhere" and trims them, not waiting for token budget to overflow.

### Composition E — Hybrid (token threshold + value brakes + reflective decision)
**T8 hybrid trigger + D4 reflective sub-agent + P4 backup.**

The richest design:
- Fast brake (token threshold; reactive to size)
- Smart brake (value-score stall detection; reactive to wasted work)
- Sub-agent decides what to remove + uses backup for recoverability
- Cost: highest of any design; quality: highest

### Composition F — Pinning + continuous eviction (alternative paradigm)
**T7 background continuous + D7 agent-self-pinning + P1 drop-unpinned.**
- Agent learns to pin progress milestones; everything else is dispensable.
- Continuous quiet eviction in background.
- Used by some experimental agent systems; low production precedent.
- Novel UX surface (the agent has to learn what to pin).

### Composition G — RAG-based (industrial pattern)
**T1 threshold + D6 retrieval + P7 vector store.**
- Drop everything > N turns; at each turn, embed user query, retrieve top-K relevant past turns, inject into prompt.
- Used by Cursor / Cody.
- Different failure modes (retrieval miss can silently lose context).
- Heavy infra; doesn't directly address "step had no value".

### Composition H — Importance-scored overlay (lossless filter; LLM-rated)

**Status**: surfaced 2026-05-21; **superseded 2026-05-22 by Composition I**. The tree-structured design subsumes H's use cases more leanly — inline node-tags eliminate H's per-block scoring overhead, and the parent-chain gives for free the dependency coherence H must enforce via co-keep constraints. H is retained below for the design-space record; it is no longer a live candidate. See Composition I for the subsumption comparison.

**Concept**: LLM emits a score 0.0–1.0 per content block (thinking, tool calls, tool results, text). Scores write to a sidecar overlay file. At refresh, the system filters the model-facing `inrun_context` to blocks whose latest score meets a selection criterion. Session file is canonical and lossless; refresh is reconstructive, not destructive.

**Axis mapping**: T6 (LLM-initiated refresh) + T8 (write-time index) × D1 (LLM-emitted score) gated by D7 (threshold rule applied mechanically) × P3 (filter sub-window from canonical session).

**Locked design choices (user 2026-05-21)**:
- **L1 — Granularity = per content block.** A turn typically emits multiple blocks (Thinking + ToolCall + Text + … + ToolResult on next turn); each gets its own score.
- **L2 — Re-evaluation allowed.** The LLM may revise a block's score later. Latest-wins semantics over append-only JSONL sidecar.
- **L3 — Overhead optimization is in scope.** Per-turn indexing cost + mandatory-skill prompt cost are engineering surface, not blockers.

#### Key points (the structural strengths)

1. **Sidesteps positional blindness.** LLM never names offsets or token counts; it names observable `block_id`s and scalar scores. The §2 hidden-assumption critique doesn't apply.
2. **Lossless by construction.** Session is canonical, scores are an overlay, refresh is reconstructive. Apply a different threshold → get a different working set. Reversibility built in.
3. **Maps onto existing phi-core machinery.** This is a NEW `BlockCompactionStrategy` VARIANT, not a NEW mechanism. The CompactionBlock overlay system already separates session-truth from compacted view. Score-overlay is a new SIGNAL SOURCE, not new architecture.
4. **Non-stationarity handled.** Re-evaluation (L2) closes the "what was important at turn 5 may not be at turn 30" gap that the original write-once draft suffered from.
5. **Cost/savings ratio is favourable.** Even with naive continuous-per-turn indexing the brake overhead is <10% of refreshed savings on a 50-turn session; with the optimizations below the ratio drops further (~100× return).

#### Critical questions (load-bearing risks)

1. **Co-keep constraint specification.** `ToolCall` ↔ `ToolResult` are a bonded pair; the refresh filter must keep them atomically (or drop both). Without an explicit rule, dropped tool-call blocks orphan their results and the next LLM call fails at the provider layer. Other implicit pairs need cataloguing (assistant message + immediate tool-result follow-up).
2. **Self-rating bias persists.** Re-evaluation closes drift over time, but the in-the-moment scoring still skews high (recency / sunk-cost). Re-evaluation requires the model to remember to do it; needs a triggering mechanism (see "Bundled revise+refresh" in contributions).
3. **Score self-confirmation loop.** If score records appear in model-facing context, seeing *"I rated this 0.9"* reinforces the judgment in subsequent reasoning. Mitigation: score records are operational metadata for the strategy, NOT part of the conversational stream visible to the LLM.
4. **Block-ID stability.** Position-derived IDs (`turn12.thinking[0]`) can shift under splices/edits; content-hash IDs require original bytes for revision. Prefer monotonic IDs assigned at write-time by phi-core with a session-side mapping.
5. **Score semantics calibration drift.** What does 0.7 MEAN? Without an explicit band-table, models calibrate differently across sessions / tasks. See Fork H2.
6. **User-message override policy.** Lock at 1.0 (chat-heavy workflows can't compact at all) vs allow override (model might silently drop a user instruction containing the actual goal). Needs an explicit carve-out.
7. **Threshold semantics under non-stationarity.** Threshold-based selection drifts as score distributions shift between sessions; top-K is more robust. See Fork H3.

#### Fork H1 — Indexing mode

**H1.a — Continuous per-turn indexing.** After each turn, the model emits a single `mark_importance` tool call with a batched array of `{block_id, score}` records for the blocks it just produced. Skill prompt explains the discipline; the model pays a ~30–100 token tax per turn.

**H1.b — JIT (just-in-time) batched indexing.** No continuous indexing. When the model calls `refresh_context(...)`, the refresh tool returns a summarised list of past blocks (`[turn7.thinking: 200 tokens — "considered Arc<Mutex<>>"]`) and the model emits all scores in that same invocation. Continuous overhead = 0; refresh cost is higher (single batch read).

**Recommendation: H1.b (JIT).** Three reasons:
1. Zero per-turn tax means short sessions pay nothing for a brake that never fires.
2. Naturally batches "score what just happened" with "decide what to keep" — these cognitive moments are already aligned; forcing the model to separate them introduces friction without value.
3. Avoids the within-turn timing problem: the model can meaningfully score Block N only AFTER seeing the tool result that came after Block N. Continuous mode (H1.a) either scores too eagerly (before result) or has awkward end-of-turn semantics.

**Caveat**: H1.b assumes the model can re-read a list of block summaries reliably at refresh time. If summaries become long, the refresh call itself crowds the working budget. Mitigation: two-level batching — scores for the last K turns emitted at compaction; older blocks inherit defaults.

#### Fork H2 — Score semantics

**H2.a — Free-form scalar 0.0–1.0.** Skill prompt explains "higher = more important; preserve longer". Calibration left to the model.

**H2.b — Banded with concrete band-table.** Skill prompt locks specific bands with named semantics:

| Score | Meaning |
|---|---|
| 0.0 | Noise / scratch / discardable artefact |
| 0.3 | Procedural context (incidental tool calls; thinking that led to discarded ideas) |
| 0.5 | Relevant detail (default for unmarked blocks) |
| 0.7 | Key decision or finding (load-bearing for current task) |
| 1.0 | Invariant / cross-turn anchor (user-stated goal, design commitment, irreversible state change) |

The model picks among 5 discrete bands; refresh selection is interpretable to humans reading the overlay.

**Recommendation: H2.b (banded).** Free-form scalars sound flexible but produce session-to-session drift: the same model rates similar blocks 0.6 in one session and 0.8 in another, depending on calibration. Banded scoring forces a mapping into named categories; the categories themselves are interpretable to humans reading the overlay file, which is independently valuable for debugging the brake.

**Caveat**: 5 bands may be too coarse for ambiguous cases. Could allow midpoints (0.4, 0.6) as "between bands" without diluting the table — the model would still default to the named bands and only use midpoints when explicitly between two adjacent meanings.

#### Fork H3 — Selection semantics

**H3.a — Threshold-only.** Refresh tool takes a scalar threshold `t`; all blocks with latest-score ≥ `t` are kept.

**H3.b — Top-K with absolute override.** Refresh takes two parameters: `keep_top: usize` (always keep the K highest-scored blocks) AND `plus_above: f32` (also keep any block ≥ this absolute score). Example: `refresh_context(keep_top=30, plus_above=0.9)` keeps the 30 most important blocks AND any block at 0.9+ regardless of rank.

**Recommendation: H3.b (top-K with override).** Three reasons:
1. **Robust to score distribution drift.** A session with generous rating and one with harsh rating both yield sensible working-set sizes.
2. **More intuitive parameter.** `keep_top=30` ("keep the 30 most important things") is interpretable in a way `t=0.6` is not.
3. **`plus_above` is the escape hatch** for "this is absolutely critical regardless of slots left." Provides a hard floor for invariants without forcing them into the top-K race.

**Caveat**: Co-keep constraints (Critical Q #1) can push the effective working set above K — keeping the top 30 blocks may pull in 5 additional bonded tool-result blocks. The refresh logic must compute the final set under co-keep and report the actual size to the model.

#### Contributions to optimization (engineering surface from L3)

Orthogonal to the three forks; compose with any combination of fork choices.

| Optimization | Where it helps | Compose-ability |
|---|---|---|
| **Defaults + override** — most blocks inherit defaults; LLM only emits `mark_importance` to deviate | Cuts H1.a per-turn cost from ~100 → ~5–10 tokens average | Composes with H1.a; less relevant for H1.b (already zero) |
| **Minimal skill prompt (~50 tokens)** with band-table inline | Reduces system-prompt overhead from prose to compact reference card | Composes with all forks |
| **Bundled revise + refresh** — `refresh_context(...)` takes optional `revise: Vec<{block_id, score}>` payload so revision and refresh happen in one call | Eliminates standalone "should I revise now?" meta-decision; aligns cost with moment of intent | Composes with all forks; particularly natural with H1.b |
| **Hide score records from LLM context** — overlay is operational metadata for the strategy, not conversational stream | Mitigates self-confirmation feedback loop (Critical Q #3) | Composes with all forks |
| **Reflective-agent post-pass revise** — an out-of-band agent reads the session at refresh time and proposes score revisions before the main agent applies them | Removes self-rating bias at the moment of compaction; cost paid in latency, not main-agent tokens | Composes naturally with H1.b — the JIT batched read is what the reflective sub-agent already does (Composition C × Composition H crossover) |
| **Phi-core auto-defaulting at write-time** — phi-core itself populates baseline scores via heuristics (user msg = 1.0, length-decayed for tool results); LLM only overrides exceptional cases | Provides a sensible baseline even if the model never explicitly indexes | Composes with H1.a; less relevant for H1.b |

#### phi-core 0.8.0 surface area if Composition H is committed

Significantly tighter than the original `compress_and_reload` draft:

| New surface | Sync/async | Notes |
|---|---|---|
| `mark_importance` model-callable tool | sync | Validates input, writes to sidecar JSONL, returns synthetic ack; same 2-phase deferred pattern as `PrunTool` |
| `refresh_context` model-callable tool | sync | Reads sidecar, computes filtered block-set respecting co-keep constraints, returns synthetic ack; deferred apply mutates `inrun_context` between turns |
| `ScoreOverlayCompactionStrategy: BlockCompactionStrategy` impl | sync | The strategy that reads the overlay and applies the filter |
| Sidecar file format spec (`<session>.importance.jsonl`) | n/a | Append-only `{block_id, score, at_turn}` records; latest-wins per `block_id` |

**No async-trait migration needed for Composition H itself.** `D-CH16b-FOLLOWUP-01` stays deferred independently — it remains the right answer for the LLM-emitting body work CH-16b stubbed (extractor / strategy / super-episode summarisation), but it is NOT blocking for the brake.

**`compress_and_reload` model-callable tool is DROPPED** — superseded by `mark_importance` + `refresh_context` (the score-overlay design replaces the position-naming design that the §2 hidden-assumption critique invalidated).

### Composition I — Tree-structured state with typed reverts (the "braking" layer)

**Status**: surfaced 2026-05-22 from a contributed design note (*Tree-Based State Architecture*) + refined across the same-day discussion. **Current front-runner.** Subsumes Composition H.

**Origin**: a contributed design doc proposed treating conversation history as a tree — each block carries `node_id` + `parent_id`; the agent calls `revert_to_state` to abandon failed branches; the prompt is rebuilt by walking `parent_id` links. The discussion that followed refined it substantially (inline node-tags, typed revert categories, model-generated summary tags, a kind-aware render policy) and clarified its role relative to compaction.

**What it is — and is NOT**: Composition I is a **braking layer**, NOT a compaction mechanism. It does not replace `BlockCompactionStrategy` or episodic memory. It sits ABOVE them, keeping the active context lean by never carrying *wasteful* content forward, and thereby **delays** how often the heavier machinery must run. Three roles, kept strictly distinct:

| Layer | Mechanism | Trigger | Role |
|---|---|---|---|
| **Braking** | Composition I (tree + typed reverts) | Agent-elected; quality-driven | Keeps the trunk lean; severs wasted work; checkpoints long spans |
| **Compaction** | `BlockCompactionStrategy` (phi-core 0.7.x) | System; token-threshold; size-driven | Compresses even a lean trunk once it grows genuinely large |
| **Episodic memory** | CH-16a / CH-16b | Created when compaction processes old content | Durable cross-session records + recursive super-episodes |

A braked trunk accumulates only real work → it crosses the compaction threshold later and less often → fewer compaction events → fewer episodic-memory extractions. That delay is the value.

**Axis mapping**: T4 (dead-end / stall) + T6 (agent-initiated) × D3-cured (agent self-directs, but via discrete `node_id`s — no positional blindness) × P1 (drop abandoned branch from active path) + P4 (full forensic JSONL retained).

#### The three-layer architecture

1. **Forensic layer** — append-only JSONL. Every block ever produced — including abandoned branches and every generated summary — persists here forever. Lossless audit trail.
2. **Working trunk** — the active context, built by walking `parent_id` links from the latest active node to root, reversed to chronological order. Abandoned branches are simply not on this path.
3. **Annotation layer** — model-generated summaries (lesson / checkpoint) attached as **tags on trunk nodes**, NOT as trunk nodes themselves, rendered into context by a kind-aware policy. This is what keeps the trunk from growing even as summaries accumulate.

#### Core mechanics

**Node model**: every content block (user message, assistant thinking, tool call, tool result) carries `node_id` (monotonic, globally unique, write-time assigned by phi-core) + `parent_id` (the block it followed on the active path). Storage stays standard append-only JSONL.

**Inline node-tags — the leanness win**: the `node_id` is rendered inline in the context the LLM already reads —

```
[ID: n10] User: Write a fast sorting algorithm.
[ID: n11] Assistant: I will write a Bubble Sort.
[ID: n12] Tool Result: Error: Execution timed out.
```

— so the index and the working memory are the **same artifact**. The tag costs ~5–8 tokens per block and nothing else: no separate index file, no separate scoring call. The agent reads IDs for free and acts only when it chooses to revert. This is strictly leaner than Composition H, which needs either a per-turn scoring call (H1.a) or a JIT re-read of block summaries (H1.b).

**The tool**: `revert_to_state(category, step)`. The agent names a discrete `node_id` (visible inline) — never an offset or token count. Sidesteps the §2 positional-blindness critique entirely.

**Four revert categories** (extensible):

| Category | When | Abandoned / summarised span becomes |
|---|---|---|
| `failure` | A branch hit a dead-end (error, timeout, wrong approach) | Severed; a one-line **lesson** is generated |
| `tangent` | An exploration finished; the finding should fold back | Severed; the **finding** is summarised |
| `completion` | A sub-task is done | Span squashed; a sealed **outcome** summary |
| `step-summary` | The current task is ongoing but the trunk got long | Span squashed into a **checkpoint**; live work continues from it |

`step-summary` is deliberately named to avoid collision with *compaction* — it is a braking operation (agent-elected, "I hit a milestone, consolidate"), not a compaction event (system, threshold-fired). `completion` and `step-summary` share the squash machinery; they differ in intent (sealed outcome vs. live checkpoint the agent keeps building on).

**The revert flow**:
1. Agent observes `[ID: n12] Tool Result: Error: ...` inline — the failure happened after n10.
2. Agent calls `revert_to_state(category=failure, step=n10)`.
3. The system generates a summary of the abandoned span via a cheap model call — `evidence: tool X → timed out` + a one-line lesson. (The agent may pass an inline lesson itself to skip the model call for trivial cases.)
4. The summary is written as a **tag on n10** and logged as a special `revert_to_state` record (category, target, abandoned-span IDs, generated summary).
5. The next prompt is built: walk the parent-chain from the new active node + render tags by policy. The abandoned branch is bypassed; the lesson rides as a tag, not a trunk node:

```
[ID: n10] User: Write a fast sorting algorithm.
   ↳ [lesson] evidence: bubble sort (O(n²)) timed out on this input
```

#### Kind-aware render policy — and the load-bearing asymmetry

Summary tags do not all behave the same; the render policy MUST key off the tag kind (the `revert_to_state:<category>::<kind>` ID encodes it):

- **`failure` lessons / `tangent` findings — decay-able.** Rendered through a **sliding window** (last K, or last M turns); older ones fall to **log-only**. Safe: repeat-risk is highest right after the mistake and decays as the work moves on. Lessons must be rendered in **forward context** — not merely surfaced at the next revert decision — because forward rendering is what makes a lesson *preventive* rather than only *diagnostic*. (Log-retrievability as a revert-decision input is the complementary, reactive half.)
- **`completion` / `step-summary` checkpoints — load-bearing, pinned.** A `step-summary` checkpoint is the **only surviving representation of a long span of real work the agent is actively building on.** It cannot decay while live work depends on it. If lossy or wrong it does not cost a repeat — it **corrupts live work.** Fidelity stakes for the checkpoint generator are therefore far higher than for the lesson generator: higher-quality model, possibly verification, possibly the option to keep a few raw blocks verbatim when faithful compression is not possible.

This asymmetry is the single most important thing to get right when this becomes a spec.

#### Why Composition I subsumes Composition H

| Dimension | Composition H (importance overlay) | Composition I (tree + typed reverts) |
|---|---|---|
| Indexing overhead | Per-turn scoring call (H1.a) or JIT re-read (H1.b) | Inline node-tags — free; revert tool called only when acting |
| Positional burden on LLM | Score blocks + pick top-K / threshold | Name one discrete `node_id` |
| Dependency coherence | Enforced via co-keep constraints (a real risk) | Perfect by construction — the parent-chain IS the dependency path |
| Wrong-turn pruning | Score the branch low + refresh | `failure` / `tangent` revert |
| Long correct-span compaction | Down-weight early bloat + refresh | `step-summary` checkpoint |
| Preserves the lesson of a failure | Partial — keep a high-scored summary block | Yes — model-generated `failure` lesson tag |
| Re-evaluation | Yes (L2 latest-wins) | Reverts are ~permanent; full JSONL allows manual re-graft |
| Losslessness | Yes (overlay) | Yes (forensic JSONL) |

The one capability H has that I lacks: keeping an **arbitrary non-contiguous subset** of raw blocks verbatim. In practice that is rarely wanted — on completion you want a summary, not a hand-picked scatter of raw turns. Acceptable loss. **Net: Composition I covers ~95% of H's ground, more leanly, and with the dependency-coherence property H must work to enforce.**

#### Detection — solved for hard failures, open for soft

Inline-tag detection works well for **hard failures**: an explicit `Error:` / timeout / non-zero exit / failed assertion is a concrete token the LLM sees and can act on — no fuzzy "am I making progress?" introspection needed. It does NOT cover **soft failures / silent stalls**: code that runs clean but is subtly wrong, a correct-but-irrelevant tangent, going in circles with no error. No token triggers on those, and the agent stuck in a rut is precisely the one that will not self-declare it. This is the principal residual open question. Even for hard failures a **revert-vs-fix** judgment remains (wrong *approach* → revert; wrong *execution* → just fix).

#### Critical questions (open before a binding spec)

1. **Soft-failure detection** — the principal residual (above). `step-summary` / `tangent` help only when the agent recognises the situation.
2. **Revert-vs-fix judgment** — the skill prompt must shape it: revert when the approach is wrong, fix when the execution is wrong.
3. **User messages inside an abandoned span** — a revert must not silently discard a user instruction. Either revert targets cannot abandon a span containing an unaddressed user message, or such messages auto-rebase onto the new branch.
4. **Co-keep at revert targets** — a revert target must snap to a safe boundary (not between a ToolCall and its ToolResult). Restrict revert targets to turn boundaries.
5. **Tag-vs-node visual distinction** — a summary tag must render visually distinct from a trunk node so the LLM cannot try to `revert_to_state` *to* a tag.
6. **Lesson-gen latency / frequency** — reverts are more frequent than the "rare compaction" originally assumed; each = one cheap model call (~1–2 s, small span). Acceptable, with the inline-lesson escape hatch for trivial cases.
7. **Checkpoint roll-up** — `step-summary` checkpoints accumulate slowly; when they pile up they should roll up hierarchically (`checkpoint(50) + checkpoint(100) → merged`), which is exactly CH-16b's recursive super-episode mechanism. Decide whether `step-summary` checkpoints feed CH-16b episodes directly.
8. **Lessons as a memory tier** — lessons in the log, retrievable, surfaced when relevant = P3 (move-to-memory). Decide: their own structured store, or a category inside i-phi's `MemoryStore` (CH-05)? Folding in gets recall machinery for free but couples brake effectiveness to recall quality (keyword-grep at v0).
9. **DAG vs tree** — the mechanics are strictly a **tree** (one `parent_id` per node). A true DAG needs multi-parent merge semantics (pulling a good idea from an abandoned branch into the active one) — unspecified. Decide whether merge is in scope.

#### phi-core 0.8.0 surface area if Composition I is committed

| Surface | Notes |
|---|---|
| `node_id` + `parent_id` on the message / content-block model | Monotonic write-time IDs; the structural foundation |
| `revert_to_state(category, step)` model-callable tool | Discrete `node_id` argument; 2-phase deferred apply (mutates the active pointer between turns) |
| Model-backed summary generator | `failure` lessons + `completion` / `step-summary` checkpoints; higher-fidelity tier for checkpoints |
| Parent-chain + kind-aware tag-policy resolution at prompt-build | The new context-assembly path |
| `revert_to_state` log record type | Category, target, abandoned-span IDs, generated summary |

**`BlockCompactionStrategy` STAYS** — Composition I is a layer above it, not a replacement; the async-trait migration question (`D-CH16b-FOLLOWUP-01`) remains independent of Composition I. **`compress_and_reload` is dropped** (the positional-assumption tool, §2). **Composition H's scoring surface is dropped** (`mark_importance` / `refresh_context` / score overlay — subsumed). No async-trait migration is needed *for Composition I itself*.

## 6. Three concrete sketches for the value-scoring axis (T3/T4/D5)

If we commit to value-scoring brakes as the load-bearing innovation, three concrete forms emerge:

### Sketch 1 — Simple stall brake (lowest cost)
- Track per-turn metrics: tool success rate, file changes count, assistant message novelty (heuristic).
- If 3 consecutive turns score "stalled" → fire compaction; drop those 3 turns; emit `AgentEvent::StallDetected`.
- No LLM needed for the brake decision.
- **Smallest delta from current architecture.**

### Sketch 2 — LLM-judged step value (richer signal)
- After each turn, a cheap LLM (Haiku / Flash) rates 1-5 with a fixed prompt.
- Track rolling avg of last K turns.
- If avg < 2.5 for 3 consecutive turns → fire compaction; sub-agent decides what to drop.
- Adds ~$0.01/turn ongoing cost.
- **Recognises nuanced stalling that heuristics would miss.**

### Sketch 3 — Continuous reflective monitoring
- Background incognito agent runs after every N turns OR every M tokens (whichever first).
- Reads current context file.
- Reports back: `OK` | `stalled — recommend prune` | `make progress — context is healthy`.
- Main agent acts on report (or system does).
- **Closest to the reflective-monitoring framing from §3.**

## 7. Implications for phi-core 0.8.0 release

The original 0.8.0 plan (drafted in the parent plan-mode session) bundled:
1. `compress_and_reload` tool primitive + `CompressStrategy` trait + 2-phase deferred apply.
2. async-trait migration for `BlockCompactionStrategy::keep_compacted`.

Depending on which composition the project commits to, the 0.8.0 surface area changes substantively:

| Composition chosen | phi-core 0.8.0 needs |
|---|---|
| **C (reflective sub-agent on token threshold)** | Just the async-trait migration. Drop `compress_and_reload` primitive entirely. Reflective strategy lives in i-phi as a `BlockCompactionStrategy` impl. |
| **D (value-scoring brakes)** | async-trait migration + possibly NEW lifecycle hook `OnTurnValueScoreFn` so consumers can wire value-scoring into the loop. No `compress_and_reload` tool. |
| **E (hybrid)** | async-trait migration + value-score hook + reflective strategy in i-phi. No `compress_and_reload` tool. |
| **G (RAG)** | Possibly nothing in phi-core; RAG layer lives entirely in i-phi consumer code. |
| **H (importance-scored overlay)** | NEW `mark_importance` + `refresh_context` model-callable tools (sync, 2-phase deferred) + `ScoreOverlayCompactionStrategy` impl + sidecar JSONL format spec. async-trait migration NOT needed for H itself (D-CH16b-FOLLOWUP-01 stays independent). `compress_and_reload` DROPPED (superseded by the score-overlay tools). *(Superseded by Composition I.)* |
| **I (tree-structured state + typed reverts) — front-runner** | `node_id`/`parent_id` on the message model + `revert_to_state(category, step)` tool + model-backed summary generator + parent-chain/kind-aware-tag resolution at prompt-build + `revert_to_state` log record type. `compress_and_reload` DROPPED; Composition H scoring surface DROPPED. No async-trait migration needed for Composition I itself. |

**Common to C/D/E/G/H/I**: `compress_and_reload` model-callable tool can be **DROPPED** from phi-core 0.8.0 — it embodies the model-self-positional assumption from §2 that this exploration questioned.

**Braking ≠ compaction (clarified 2026-05-22)**: Composition I is a *braking* layer that **delays** the need for compaction — it does NOT replace `BlockCompactionStrategy`, which stays in phi-core and continues to fire as-and-when-needed on the token threshold. Episodic memory (CH-16a/16b) likewise stays. Composition I reduces how *often* both run; it does not remove them. The async-trait migration of `BlockCompactionStrategy::keep_compacted` is therefore still needed regardless of Composition I, tracked independently via `D-CH16b-FOLLOWUP-01`.

**Common to all**: async-trait `BlockCompactionStrategy::keep_compacted` migration is needed regardless (it's load-bearing for any LLM-driven compaction body).

## 8. Open questions (for resolution before binding spec)

1. **Trigger commitment**: do we want value-scoring brakes (T3/T4) as an addition to token-threshold (T1), as a replacement, or layered hybrid (T8)?
2. **Mechanism commitment**: given the trigger fires, who decides what to drop? (D1 heuristic / D2 system-picks-LLM-summarises / D4 reflective sub-agent / D5 value-scoring-decides)
3. **Detection vs execution priority**: which is the harder problem — knowing when to brake (T3/T4) or how to brake cleanly (D4)? They're separable.
4. **Self-expressed vs externally-detected stall**: does the agent itself express "I'm stuck" (model-callable signal), or does the system detect from external signals (tool success, file changes, state diffs)?
5. **Cost tolerance**: value-scoring on every turn adds continuous LLM cost; threshold-based fires only at compaction time. What's the budget?
6. **Brake surface visibility**: should the "brake" be model-callable (agent can request), system-only (agent never knows), or both?
7. **Rollback vs trim paradigm**: for low-value steps, would we prefer to UNDO them (revert state) rather than just drop from context? (Different conceptual model: brake = revert vs brake = trim.) Snapshotting infrastructure required for rollback.
8. **Reflectable scope boundary**: what counts as "reflectable in-turn context" vs "invariant identity / memory tier"? Needs codification before the reflective design can ship.
9. **Empirical validation plan**: do we ship one composition + collect telemetry, or A/B test multiple? `IphiBlockCompactionStrategy` planned at CH-17 could carry both a reflective impl AND a single-call impl behind a config switch.
10. **Recovery story**: if compression turns out to be wrong (dropped content was needed), what's the recovery path? P4 backup file enables this; P1 hard-drop doesn't.

## 9. Reference primitives that exist today

Useful to remember what's already in phi-core / i-phi when designing the brake:

| Primitive | Lives at | Reusable for which composition |
|---|---|---|
| `phi_core::context::BlockCompactionStrategy` | `phi-core/src/context/strategy.rs:93-152` | Foundation for B, C, D, E |
| `phi_core::tools::PrunTool` (2-phase deferred pattern) | `phi-core/src/tools/prun.rs` | Template for any model-callable brake (D3 family) |
| `phi_core::agents::SubAgentTool` | `phi-core/src/agents/` | Enables D4 reflective sub-agent |
| `phi_core::session::FileSystemSessionStore` | `phi-core/src/session/storage.rs` | Materialises context to disk (P4 backup; D4 file input) |
| `phi_core::types::AgentMessage` JSON serde | `phi-core/src/types/message.rs` | The file format for the reflective sub-agent to read/edit |
| `i_phi::compaction::*` (CH-16a + CH-16b) | `i-phi/src/compaction/` | Episode model + recursive layer + EpisodeStore (Composition B; partial for E) |
| `i_phi::memory::MemoryStore` (CH-05) | `i-phi/src/memory/` | Memory tier extraction sink (P3) |
| `i_phi::sessions::SessionMode::Incognito` (CH-06) | `i-phi/src/sessions/` | Sub-agent runs incognito (D4 sub-agent isolation; harvest precondition) |

## 10. Status of this document

This is **exploration**, not commitment. Status tag: `[CONCEPTUAL]`.

The document captures:
- The problem framing surfaced 2026-05-21 (the "no brakes" analogy)
- The §2 hidden-assumption critique that nullified the original `compress_and_reload` design intuition
- The reflective-agent design proposal (§3)
- The full 3-axis design space (§4)
- The catalogue of coherent compositions (§5)
- Three concrete sketches for value-scoring brakes (§6)
- Implications for the in-flight phi-core 0.8.0 release (§7)
- Open questions to resolve before committing to a binding spec (§8)
- Existing primitives that can be reused (§9)

**Next step** (when discussion resumes): **Composition I (tree-structured state with typed reverts) is the current front-runner** — it subsumes Composition H and is the leanest design surfaced. Remaining work before a binding spec: resolve Composition I's §5 critical questions (soft-failure detection is the principal open one). Then either:
- Author a binding spec (this doc transitions from `[CONCEPTUAL]` to `[PLANNED]`, possibly absorbing into [`compaction.md`](compaction.md)).
- Revise the phi-core 0.8.0 plan (per §7) to match Composition I — note `BlockCompactionStrategy` stays; Composition I is a braking layer above it.
- File new drift entries for any deferrals identified.