<!-- Last verified: 2026-04-05 by Claude Code -->

> For pseudocode conventions, see the [README](../README.md#pseudocode-conventions).
### `ReadFileTool::execute` *(src/tools/file.rs)*

**Purpose:** Read a file's contents. Routes to binary (image) or text path based on extension.

```
FUNCTION ReadFileTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path ← params["path"] as String  // InvalidArgs if missing

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  metadata ← AWAIT fs.metadata(path)  // Err → Failed("Cannot access {path}: {e}")

  IF is_image_extension(path) THEN
    // ── Image path ────────────────────────────────────────────────────────
    IF metadata.size > 20MB THEN
      RETURN Err(Failed("Image too large"))
    END IF
    bytes ← AWAIT fs.read(path)
    data ← base64_encode(bytes)
    mime_type ← get_mime_type(path)
    RETURN Ok(ToolResult {
      content: [Image { data, mime_type }],
      details: { path, bytes: bytes.len() }
    })
  END IF

  // ── Text path ─────────────────────────────────────────────────────────
  IF metadata.size > self.max_bytes THEN
    RETURN Err(Failed("File too large. Use offset/limit for partial reads."))
  END IF

  content ← AWAIT fs.read_to_string(path)
  lines ← content.split_lines()
  total ← lines.count()

  offset ← params["offset"] as usize (1-indexed)  // optional, default: 1
  limit  ← params["limit"]  as usize               // optional, default: all

  (start, end) ← compute_range(offset, limit, total)

  // Line-numbered output: "   1 | first line"
  numbered ← ["{start+i+1:>4} | {line}" FOR (i, line) IN enumerate(lines[start..end])]

  header ←
    IF start > 0 OR end < total THEN "[Lines {start+1}-{end} of {total}]"
    ELSE "[{total} lines]"

  RETURN Ok(ToolResult {
    content: [Text("{header}\n{numbered.join('\n')}")],
    details: { path }
  })

END FUNCTION
```

---

### `EditFileTool::execute` *(src/tools/edit.rs)*

**Purpose:** Make a surgical search-and-replace edit in an existing file.
**Preconditions:** File exists; `old_text` occurs exactly once in the file.
**Postconditions:** File on disk has exactly the one occurrence of `old_text` replaced by `new_text`.

```
FUNCTION EditFileTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path     ← params["path"]     as String  // InvalidArgs if missing
  old_text ← params["old_text"] as String  // InvalidArgs if missing
  new_text ← params["new_text"] as String  // InvalidArgs if missing

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  content ← AWAIT fs.read_to_string(path)
  // Err → Failed("Cannot read {path}. Use write_file to create new files.")

  match_count ← count of occurrences of old_text in content

  IF match_count == 0 THEN
    // Provide helpful fuzzy hint
    hint ← find_similar_text(content, old_text)
    IF hint defined THEN
      message ← "old_text not found in {path}.\n\nDid you mean:\n```\n{hint}\n```\n..."
    ELSE
      message ← "old_text not found in {path}.\n\nTip: Use read_file to see contents..."
    END IF
    RETURN Err(Failed(message))
  END IF

  IF match_count > 1 THEN
    RETURN Err(Failed(
      "old_text matches {match_count} locations. Include more context to make match unique."
    ))
  END IF

  // Replace exactly the first (and only) occurrence
  new_content ← content.replace_once(old_text, new_text)
  AWAIT fs.write(path, new_content)

  old_lines ← old_text.line_count()
  new_lines ← new_text.line_count()

  RETURN Ok(ToolResult {
    content: [Text("Replaced {old_lines} line(s) with {new_lines} line(s) in {path}")],
    details: { path, old_lines, new_lines }
  })

END FUNCTION

FUNCTION find_similar_text(content: String, target: String) -> Option<String>
  // Fuzzy hint: find the first line of target in the file
  target_trimmed ← target.trim()
  first_line ← target_trimmed.first_line().trim()
  IF first_line is empty THEN RETURN None END IF

  lines ← content.split_lines()
  FOR EACH (i, line) IN enumerate(lines)
    IF line contains first_line THEN
      end ← min(i + target_trimmed.line_count() + 1, lines.count())
      RETURN Some(lines[i..end].join("\n"))
    END IF
  END FOR

  RETURN None
END FUNCTION
```

---

### `SkillSet::format_for_prompt` *(src/context/skills.rs)*

**Purpose:** Format all loaded skills as an XML index for injection into the system prompt.
**Standard:** Conforms to the AgentSkills open standard (agentskills.io/integrate-skills).

```
FUNCTION SkillSet::format_for_prompt() -> String

  IF self.skills is empty THEN RETURN "" END IF

  // Skills are sorted by name ascending
  sorted_skills ← sort(self.skills, by: skill.name)

  out ← "<available_skills>\n"

  FOR EACH skill IN sorted_skills
    out += "  <skill>\n"
    out += "    <name>"        + xml_escape(skill.name)                      + "</name>\n"
    out += "    <description>" + xml_escape(skill.description)               + "</description>\n"
    out += "    <location>"    + xml_escape(skill.file_path.to_string())     + "</location>\n"
    out += "  </skill>\n"
  END FOR

  out += "</available_skills>"
  RETURN out

  // xml_escape replaces: & → &amp;  < → &lt;  > → &gt;  " → &quot;  ' → &apos;

END FUNCTION

// Example output:
// <available_skills>
//   <skill>
//     <name>weather</name>
//     <description>Get current weather and forecasts.</description>
//     <location>/home/user/.skills/weather/SKILL.md</location>
//   </skill>
// </available_skills>
```

### `SkillSet::load` *(src/context/skills.rs)*

**Purpose:** Load skills from one or more directories. Later directories override earlier ones on name collision.

```
FUNCTION SkillSet::load(dirs: Vec<Path>) -> Result<SkillSet, SkillError>

  skill_map ← HashMap<String, Skill>  // key = skill name

  FOR EACH (index, dir) IN enumerate(dirs)
    IF dir does not exist THEN
      CONTINUE  // silently skip missing directories
    END IF

    source_label ← "dir:{index}"

    FOR EACH entry IN list_subdirectories(dir)
      skill_md_path ← entry.path / "SKILL.md"
      IF skill_md_path does not exist THEN
        CONTINUE
      END IF

      content ← read_to_string(skill_md_path)
      (name, description) ← parse_frontmatter(content)
      // Returns SkillError::InvalidFrontmatter or SkillError::MissingField on failure

      base_dir ← canonicalize(entry.path)
      file_path ← base_dir / "SKILL.md"

      skill ← Skill { name, description, file_path, base_dir, source: source_label }
      skill_map[name] ← skill  // later dirs OVERRIDE earlier on name collision
    END FOR
  END FOR

  skills ← sort(skill_map.values(), by: skill.name)
  RETURN Ok(SkillSet { skills })

END FUNCTION

FUNCTION parse_frontmatter(content: String) -> Result<(name, description), SkillError>
  // Content must start with "---"
  IF NOT content.trim_start().starts_with("---") THEN
    RETURN Err(InvalidFrontmatter)
  END IF

  // Find closing "---"
  yaml_block ← content between first "---" and next "\n---"
  IF no closing delimiter THEN
    RETURN Err(InvalidFrontmatter)
  END IF

  name ← ""
  description ← ""

  FOR EACH line IN yaml_block.lines()
    IF line.starts_with("name:") THEN
      name ← unquote(line.after("name:").trim())
    ELSE IF line.starts_with("description:") THEN
      description ← unquote(line.after("description:").trim())
    END IF
    // All other YAML fields silently ignored
  END FOR

  IF name is empty THEN RETURN Err(MissingField("name")) END IF
  IF description is empty THEN RETURN Err(MissingField("description")) END IF

  RETURN Ok((name, description))

  // unquote(): strips surrounding single or double quotes if present

END FUNCTION
```

---

---

### `ListFilesTool::execute` *(src/tools/list.rs)*

**Purpose:** List files in a directory, with optional glob filtering and depth limit.

```
FUNCTION ListFilesTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  path      ← params["path"]      as String  // optional; default: current directory
  pattern   ← params["pattern"]   as String  // optional glob filter, e.g. "*.rs"
  max_depth ← params["max_depth"] as usize   // optional; default: 3

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  // Build `find` command
  cmd ← "find {path} -maxdepth {max_depth} -type f"
  IF pattern defined THEN cmd += " -name '{pattern}'" END IF
  // Excluded paths (prepended to command):
  //   -not -path "*/target/*"
  //   -not -path "*/.git/*"
  //   -not -path "*/node_modules/*"

  SELECT {
    ctx.cancel.cancelled() → RETURN Err(Cancelled)
    sleep(self.timeout)    → RETURN Err(Failed("List timed out"))
    run(cmd)               → output
  }

  lines ← output.stdout.split_lines()

  truncated ← false
  IF lines.count() > self.max_results THEN
    lines ← lines[0..self.max_results]
    truncated ← true
  END IF

  text ← lines.join("\n")
  IF truncated THEN
    text += "\n... (truncated at {self.max_results} results)"
  END IF

  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { total: lines.count(), truncated }
  })

END FUNCTION
```

**Defaults:** `max_results = 200`, `timeout = 10s`

---

### `SearchTool::execute` *(src/tools/search.rs)*

**Purpose:** Search file contents using regex via ripgrep (preferred) or grep (fallback).

```
FUNCTION SearchTool::execute(params: JSON, ctx: ToolContext) -> Result<ToolResult, ToolError>

  pattern        ← params["pattern"]        as String  // required; regex
  path           ← params["path"]           as String  // optional; default: self.root or cwd
  include        ← params["include"]        as String  // optional file glob, e.g. "*.rs"
  case_sensitive ← params["case_sensitive"] as bool    // optional; default: false

  IF ctx.cancel.is_cancelled THEN RETURN Err(Cancelled) END IF

  // Prefer ripgrep (rg) if available, fall back to grep
  IF rg_available() THEN
    cmd ← ["rg", "--line-number", "--no-heading",
            "--max-count={self.max_results}"]
    IF NOT case_sensitive THEN cmd += ["--ignore-case"] END IF
    IF include defined THEN cmd += ["--glob={include}"] END IF
    cmd += [pattern, path]
  ELSE
    cmd ← ["grep", "-r", "-n", "-m{self.max_results}"]
    IF NOT case_sensitive THEN cmd += ["-i"] END IF
    IF include defined THEN cmd += ["--include={include}"] END IF
    cmd += [pattern, path]
  END IF

  SELECT {
    ctx.cancel.cancelled() → RETURN Err(Cancelled)
    sleep(self.timeout)    → RETURN Err(Failed("Search timed out"))
    run(cmd)               → (exit_code, stdout, stderr)
  }

  // Exit code 1 = no matches found (not an error)
  IF exit_code == 1 AND stderr is empty THEN
    stdout ← ""
  END IF
  // Exit code 2+ or non-empty stderr = actual failure
  IF exit_code >= 2 OR (exit_code != 0 AND stderr non-empty) THEN
    RETURN Err(Failed(stderr))
  END IF

  lines ← stdout.split_lines()
  match_count ← lines.count()

  text ← stdout
  IF match_count >= self.max_results THEN
    text += "\n... (truncated at {self.max_results} matches)"
  END IF

  RETURN Ok(ToolResult {
    content: [Text(text)],
    details: { matches: match_count }
  })

END FUNCTION
```

**Defaults:** `max_results = 50`, `timeout = 30s`
**Output format:** `{file}:{line_number}:{matched_line}`

---
