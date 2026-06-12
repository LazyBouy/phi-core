<!-- Last verified: 2026-06-12 by Claude Code (KC-04 #78/D-TEST-0073 — selectable skill-prompt layout: XML stays the byte-for-byte default, opt-in YAML via with_skills_format / format_for_prompt_as; see ADR-0004) -->
# Skills

Skills extend an agent with domain expertise using the [AgentSkills](https://agentskills.io) open standard. A skill is a directory containing a `SKILL.md` file with instructions the agent can load on demand.

## How it works

Skills use **progressive disclosure** to manage context efficiently:

1. **Metadata** (~100 tokens/skill) — name + description, always in the system prompt
2. **Instructions** (<5k tokens) — SKILL.md body, loaded when the agent decides the skill is relevant
3. **Resources** (unlimited) — scripts, references, assets — loaded only when needed

The agent decides when to activate a skill based on the description alone. No trigger engine needed.

## Skill format

```
my-skill/
├── SKILL.md          # Required: YAML frontmatter + instructions
├── scripts/          # Optional: executable code
├── references/       # Optional: documentation loaded on demand
└── assets/           # Optional: templates, static resources
```

SKILL.md uses YAML frontmatter:

```markdown
---
name: git
description: Git operations — commit, branch, merge, rebase. Use when the user mentions version control.
---

# Git Skill

## Workflow
1. Run `git status` first
2. Stage changes, write conventional commit messages
3. For merges, check for conflicts first

## Scripts
For complex diffs: `bash {baseDir}/scripts/diff_summary.sh`
```

## Loading skills

```rust
use phi_core::SkillSet;
use std::path::PathBuf;

// Load from multiple directories (later dirs override earlier on name conflict)
let skills = SkillSet::load(&[PathBuf::from("./skills"), PathBuf::from("~/.phi-core/skills")]);

// Or load from a single directory with a label
let workspace_skills = SkillSet::load_dir("./skills", "workspace");
```

## Using with Agent

```rust
use phi_core::{BasicAgent, SkillSet};
use phi_core::provider::ModelConfig;
use std::path::PathBuf;

let api_key = std::env::var("ANTHROPIC_API_KEY").unwrap();
let skills = SkillSet::load(&[PathBuf::from("./skills")]);

let agent = BasicAgent::new(ModelConfig::anthropic(
    "claude-sonnet-4-20250514",
    "Claude Sonnet 4",
    &api_key,
))
.with_system_prompt("You are a coding assistant.")
.with_skills(skills)  // Appends skill index to system prompt
.with_tools(tools);
```

The agent's system prompt will include:

```xml
<available_skills>
  <skill>
    <name>git</name>
    <description>Git operations — commit, branch, merge, rebase.</description>
    <location>/path/to/skills/git/SKILL.md</location>
  </skill>
</available_skills>
```

When the agent encounters a task matching a skill, it reads the SKILL.md using the `read_file` tool and follows the instructions. No special infrastructure needed.

## Prompt layout (XML default + opt-in YAML)

XML is the **default** skill-index layout, per the [AgentSkills standard](https://agentskills.io/integrate-skills) — `with_skills(set)` and `SkillSet::format_for_prompt()` render the byte-for-byte `<available_skills>` block shown above, and existing consumers are unchanged.

For token-budget-sensitive callers, a lighter **YAML** layout is opt-in. It carries the same `name`/`description`/`location` metadata triple but drops the XML tag overhead:

```rust
use phi_core::{BasicAgent, SkillSet, SkillPromptFormat};

let agent = BasicAgent::new(model)
    .with_skills_format(skills, SkillPromptFormat::Yaml);  // opt into YAML
// .with_skills(skills) is unchanged — it is the XML default
```

The YAML index renders as:

```yaml
available_skills:
  - name: git
    description: "Git operations: commit, branch, merge, rebase."
    location: "/path/to/skills/git/SKILL.md"
```

Scalar values are quoted only where YAML plain-scalar rules require it — a value containing a `:` (like the description above), a reserved word (`null`/`true`/`yes`), or a numeric/date-looking value is quoted so it round-trips back as a string; a plain safe value (e.g. a bare `git` name) stays unquoted. To render directly without an agent, use `SkillSet::format_for_prompt_as(SkillPromptFormat::Yaml)`.

> **Why XML is the default:** the AgentSkills open standard specifies the `<available_skills>` XML block, so keeping it as the default preserves the standard's expectations and every consumer's behaviour byte-for-byte. YAML is purely an opt-in lighter alternative. See `docs/decisions/0004-selectable-skill-prompt-layout.md`.

## Precedence

When loading from multiple directories, later directories take precedence. A skill in `./skills/` overrides the same-named skill in `~/.phi-core/skills/`.

You can also merge skill sets explicitly:

```rust
let mut base = SkillSet::load_dir("/usr/share/phi-core/skills", "bundled")?;
let user = SkillSet::load_dir("~/.phi-core/skills", "user")?;
let workspace = SkillSet::load_dir("./skills", "workspace")?;

base.merge(user);
base.merge(workspace); // workspace wins on conflict
```

## Compatibility

By following the AgentSkills standard, skills written for phi-core work with Claude Code, Codex CLI, Gemini CLI, Cursor, OpenCode, Goose, and any other compatible agent. Write once, use everywhere.

## Design philosophy

Skills are deliberately simple:

- **No trigger engine** — the LLM decides from descriptions
- **No compile-time registration** — skills use existing tools (read_file, bash)
- **No plugin API** — skills are just files
- **No runtime loading** — loaded at startup, that's it

If a skill needs a custom tool, it can provide an [MCP](./mcp.md) server.
