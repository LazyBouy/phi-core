//! Skills — load AgentSkills-compatible skill directories and inject into system prompts.
//!
//! Follows the [AgentSkills](https://agentskills.io) open standard.
//! Skills are directories containing a `SKILL.md` file with YAML frontmatter.
//!
//! # Progressive Disclosure
//!
//! 1. **Metadata** (~100 tokens/skill) — name + description, always in the system prompt
//! 2. **Instructions** (<5k tokens) — SKILL.md body, loaded by the agent when activated
//! 3. **Resources** (unlimited) — scripts/, references/, assets/, loaded on demand
//!
//! The agent decides when to activate a skill based on the description. No trigger
//! engine needed — the LLM is smart enough.
//!
//! # Example
//!
//! ```rust,no_run
//! use phi_core::SkillSet;
//!
//! let skills = SkillSet::load(&["./skills", "~/.phi-core/skills"]).unwrap();
//! println!("{}", skills.format_for_prompt());
//! // Inject into system prompt via Agent::with_skills()
//! ```

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

/// A loaded skill with its metadata.
#[derive(Debug, Clone)]
pub struct Skill {
    /// Skill name (must match directory name, lowercase + hyphens)
    pub name: String,
    /// Description of what the skill does and when to use it
    pub description: String,
    /// Absolute path to SKILL.md
    pub file_path: PathBuf,
    /// Absolute path to the skill directory
    pub base_dir: PathBuf,
    /// Where this skill was loaded from (e.g. "workspace", "global", or a custom label)
    pub source: String,
}

/// A collection of loaded skills.
#[derive(Debug, Clone, Default)]
pub struct SkillSet {
    skills: Vec<Skill>,
}

/// Layout for rendering the skill index into a system prompt.
///
/// `Xml` is the **default** — the byte-for-byte [AgentSkills standard](https://agentskills.io/integrate-skills)
/// `<available_skills>` block that has always shipped. `Yaml` is an opt-in, lighter
/// layout (a YAML mapping + sequence) that drops the XML tag overhead for token-budget
/// sensitive callers. Both render the same `name`/`description`/`location` triple from the
/// same `Skill` fields; only the surrounding syntax differs.
///
/// Selected per render via [`SkillSet::format_for_prompt_as`] or per agent via
/// `BasicAgent::with_skills_format`. The default `SkillSet::format_for_prompt` and
/// `BasicAgent::with_skills` route through the `Xml` branch unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SkillPromptFormat {
    /// AgentSkills `<available_skills>` XML block — the default.
    #[default]
    Xml,
    /// Lighter YAML mapping + sequence (`available_skills:` → `- name:/description:/location:`).
    Yaml,
}

/*
RUST QUIRK: `Path` vs `PathBuf`

  `Path`    — borrowed path slice (like &str for strings), no allocation
  `PathBuf` — owned, heap-allocated path (like String), can grow/modify

Why does Skill store `PathBuf` (not `Path`)?
Because Skill is a struct that OWNS its data — it must hold the path independently
of wherever it was loaded from. PathBuf is the owned version.

Why does `load_skills_from_dir(dir: &Path)` take `&Path`?
Because the function only needs to READ the path — borrowing is cheaper than cloning.
`impl AsRef<Path>` accepts &str, String, PathBuf, or &Path — all convert to &Path.

Python analogy: PathBuf ≈ str (mutable), Path ≈ bytes (immutable view).
In Python, you'd just use str or pathlib.Path without these distinctions.
*/

/// Errors during skill loading.
/*
RUST QUIRK: `thiserror::Error` derive macro — automatic error types

`#[derive(thiserror::Error)]` generates the `std::error::Error` impl automatically.
The `#[error("...")]` attribute defines the Display message for each variant.

Interpolation in error strings:
  {path}   — calls Display on the `path` field (PathBuf implements Display)
  {source} — for `std::io::Error`, shows the OS error message
  {field}  — for &'static str, shows the field name directly

RUST QUIRK: `field: &'static str`

`&'static str` means "a string reference that lives for the entire program lifetime."
In practice, this means string literals: "name", "description" — they're baked into
the binary. Using `&'static str` instead of `String` avoids allocation for these
compile-time-known field names.

If the field names were dynamic (computed at runtime), you'd use `String` instead.
*/
#[derive(Debug, thiserror::Error)]
pub enum SkillError {
    #[error("IO error reading {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("SKILL.md in {path} missing required frontmatter field: {field}")]
    MissingField { path: PathBuf, field: &'static str }, // &'static = compile-time string literal
    #[error("SKILL.md in {path} has invalid frontmatter: {detail}")]
    InvalidFrontmatter { path: PathBuf, detail: String },
}

impl SkillSet {
    /// Load skills from multiple directories. Later directories take precedence
    /// (skills with the same name from later dirs override earlier ones).
    pub fn load(
        dirs: &[impl AsRef<Path>], // ORDERED DIRECTORIES — scanned left to right; later dirs win on name conflicts
    ) -> Result<Self, SkillError> {
        /*
        RUST QUIRK: `HashMap` for deduplication (last-write-wins)

        HashMap<String, Skill> maps skill name → Skill.
        `.insert(key, value)` silently OVERWRITES if the key already exists.
        Iterating dirs in order (first → last) means later dirs win on name conflict.

        Python analogy: by_name = {}; by_name[skill.name] = skill

        RUST QUIRK: `dirs: &[impl AsRef<Path>]`

        `&[impl AsRef<Path>]` = a slice of "anything that can be viewed as a Path."
        This accepts: &[&str], &[String], &[PathBuf], or any mix.
        `dir.as_ref()` converts whatever type `dir` is into &Path.
        */
        let mut by_name: HashMap<String, Skill> = HashMap::new();

        for (i, dir) in dirs.iter().enumerate() {
            let dir = dir.as_ref(); // convert to &Path regardless of input type
            if !dir.exists() {
                continue; // silently skip non-existent dirs (not an error)
            }
            let source = format!("dir:{}", i);
            /*
            RUST QUIRK: `?` operator for error propagation

            `load_skills_from_dir(dir, &source)?` means:
              - If Ok(skills): unwrap and bind to `skills`
              - If Err(e):      immediately RETURN Err(e) from the current function

            Without `?`, you'd write:
              let skills = match load_skills_from_dir(dir, &source) {
                  Ok(s) => s,
                  Err(e) => return Err(e),
              };

            `?` is syntactic sugar for this pattern. It makes error-propagating
            code as readable as Python's try/except but without hiding the errors.
            */
            let skills = load_skills_from_dir(dir, &source)?;
            for skill in skills {
                by_name.insert(skill.name.clone(), skill); // later dirs overwrite
            }
        }

        /*
        RUST QUIRK: `into_values().collect()` — consuming a HashMap into a Vec

        `by_name.into_values()` — consume the HashMap (ownership transfer), yield only the VALUES
        `.collect()` — gather the iterator into a Vec<Skill>

        `by_name.values()` would BORROW the values (&Skill), yielding references.
        `by_name.into_values()` MOVES the values out (Skill), avoiding clones.
        We use `into_values()` because we're done with the HashMap.
        */
        let mut skills: Vec<Skill> = by_name.into_values().collect();
        /*
        `.sort_by(|a, b| a.name.cmp(&b.name))` — sort in place with a comparator

        sort_by takes a closure that returns std::cmp::Ordering (Less, Equal, Greater).
        `.cmp()` on String does lexicographic comparison and returns Ordering.

        Python analogy: skills.sort(key=lambda s: s.name)

        Rust's sort_by is a stable sort (preserves relative order of equal elements).
        */
        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(Self { skills }) // wrap in Ok() to match return type Result<Self, SkillError>
    }

    /// Load skills from a single directory with a custom source label.
    pub fn load_dir(
        dir: impl AsRef<Path>, // DIRECTORY — single skill directory to scan for subdirectories with SKILL.md
        source: &str, // LABEL     — stored on each Skill for tracking origin (e.g. "workspace", "global")
    ) -> Result<Self, SkillError> {
        let skills = load_skills_from_dir(dir.as_ref(), source)?;
        Ok(Self { skills })
    }

    /// Create an empty skill set.
    pub fn empty() -> Self {
        Self { skills: Vec::new() }
    }

    /// Merge another skill set into this one. Other's skills override on name conflict.
    pub fn merge(
        &mut self,
        other: SkillSet, // INCOMING — skills from the other set; wins on name conflict (same behavior as later-dir-wins in load())
    ) {
        let mut by_name: HashMap<String, Skill> =
            self.skills.drain(..).map(|s| (s.name.clone(), s)).collect();
        for skill in other.skills {
            by_name.insert(skill.name.clone(), skill);
        }
        self.skills = by_name.into_values().collect();
        self.skills.sort_by(|a, b| a.name.cmp(&b.name));
    }

    /// Get all loaded skills.
    pub fn skills(&self) -> &[Skill] {
        &self.skills
    }

    /// Number of loaded skills.
    pub fn len(&self) -> usize {
        self.skills.len()
    }

    /// Whether no skills are loaded.
    pub fn is_empty(&self) -> bool {
        self.skills.is_empty()
    }

    /// Format skills for inclusion in a system prompt.
    ///
    /// Uses XML format per the [AgentSkills standard](https://agentskills.io/integrate-skills):
    /// ```xml
    /// <available_skills>
    ///   <skill>
    ///     <name>weather</name>
    ///     <description>Get current weather and forecasts.</description>
    ///     <location>/path/to/skills/weather/SKILL.md</location>
    ///   </skill>
    /// </available_skills>
    /// ```
    ///
    /// Returns an empty string if no skills are loaded.
    ///
    /// This is the **XML default** — it delegates to
    /// [`format_for_prompt_as`](Self::format_for_prompt_as)`(SkillPromptFormat::Xml)`, so its
    /// output is byte-for-byte identical to the historical AgentSkills render. Use
    /// `format_for_prompt_as` to opt into the lighter YAML layout.
    pub fn format_for_prompt(&self) -> String {
        self.format_for_prompt_as(SkillPromptFormat::Xml)
    }

    /// Format skills for inclusion in a system prompt using the given layout.
    ///
    /// `SkillPromptFormat::Xml` (the default, also used by [`format_for_prompt`](Self::format_for_prompt))
    /// renders the byte-for-byte [AgentSkills standard](https://agentskills.io/integrate-skills)
    /// `<available_skills>` block. `SkillPromptFormat::Yaml` renders a lighter YAML mapping +
    /// sequence carrying the same `name`/`description`/`location` triple. Returns an empty
    /// string if no skills are loaded, for either format.
    ///
    /// The YAML branch quotes scalar values only where YAML plain-scalar rules require it
    /// (so reserved words like `null`/`true` and numeric/date-looking values round-trip back
    /// as strings); see [`SkillPromptFormat`].
    pub fn format_for_prompt_as(&self, format: SkillPromptFormat) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

        match format {
            SkillPromptFormat::Xml => {
                let mut out = String::from("<available_skills>\n");
                for skill in &self.skills {
                    out.push_str("  <skill>\n");
                    out.push_str(&format!("    <name>{}</name>\n", xml_escape(&skill.name)));
                    out.push_str(&format!(
                        "    <description>{}</description>\n",
                        xml_escape(&skill.description)
                    ));
                    out.push_str(&format!(
                        "    <location>{}</location>\n",
                        xml_escape(&skill.file_path.to_string_lossy())
                    ));
                    out.push_str("  </skill>\n");
                }
                out.push_str("</available_skills>");
                out
            }
            SkillPromptFormat::Yaml => {
                let mut out = String::from("available_skills:\n");
                for skill in &self.skills {
                    out.push_str(&format!("  - name: {}\n", yaml_escape_scalar(&skill.name)));
                    out.push_str(&format!(
                        "    description: {}\n",
                        yaml_escape_scalar(&skill.description)
                    ));
                    out.push_str(&format!(
                        "    location: {}\n",
                        yaml_escape_scalar(&skill.file_path.to_string_lossy())
                    ));
                }
                // Match the XML branch's no-trailing-newline contract.
                if out.ends_with('\n') {
                    out.pop();
                }
                out
            }
        }
    }
}

/// Scan a directory for skills. Looks for:
/// - `<dir>/<name>/SKILL.md` (standard layout)
fn load_skills_from_dir(
    dir: &Path, // DIRECTORY — scanned for subdirectories, each of which may be a skill (must contain SKILL.md)
    source: &str, // LABEL     — stored verbatim on every Skill loaded from this dir (for provenance tracking)
) -> Result<Vec<Skill>, SkillError> {
    let mut skills = Vec::new();

    /*
    RUST QUIRK: `.map_err(|e| ...)` — converting error types

    `fs::read_dir()` returns `Result<ReadDir, std::io::Error>`.
    Our function returns `Result<Vec<Skill>, SkillError>`.
    The types don't match — we need to convert `std::io::Error` → `SkillError`.

    `.map_err(|e| SkillError::Io { path: ..., source: e })` transforms the Err variant:
      Ok(v)  → Ok(v) unchanged
      Err(e) → Err(SkillError::Io { path: dir.to_path_buf(), source: e })

    Then `?` propagates the converted error if it's Err.

    `dir.to_path_buf()` — converts &Path to owned PathBuf (heap allocation).
    Required because SkillError stores PathBuf (owned), not &Path (borrowed).
    */
    let entries = fs::read_dir(dir).map_err(|e| SkillError::Io {
        path: dir.to_path_buf(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| SkillError::Io {
            path: dir.to_path_buf(),
            source: e,
        })?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        let content = fs::read_to_string(&skill_md).map_err(|e| SkillError::Io {
            path: skill_md.clone(),
            source: e,
        })?;

        let (name, description) = parse_frontmatter(&content, &skill_md)?;

        // Validate name matches directory
        /*
        RUST QUIRK: `to_string_lossy()` — graceful handling of non-UTF8 paths

        File paths on some platforms (Linux) can contain arbitrary bytes, not just UTF-8.
        `OsStr::to_string_lossy()` returns a `Cow<str>`:
          - `Cow::Borrowed(&str)` if the path is valid UTF-8 (zero copy)
          - `Cow::Owned(String)` if non-UTF8, replacing invalid sequences with U+FFFD (lossy)

        `Cow` = "Clone On Write" — a smart pointer that avoids allocation when possible.
        `.to_string()` at the end converts Cow<str> to owned String in both cases.

        `file_name()` returns Option<&OsStr> — None if path ends with ".." or "/".
        `unwrap_or_default()` returns OsStr::new("") on None.
        */
        let dir_name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy() // OsStr → Cow<str> (handles non-UTF8 gracefully)
            .to_string();

        // Use directory name if frontmatter name doesn't match (be lenient)
        let name = if name == dir_name { name } else { dir_name };

        let base_dir = fs::canonicalize(&path).unwrap_or(path);
        let file_path = base_dir.join("SKILL.md");

        skills.push(Skill {
            name,
            description,
            file_path,
            base_dir,
            source: source.to_string(),
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

/// Parse YAML frontmatter from SKILL.md content.
/// Expects `---\n...\n---` block at the start.
fn parse_frontmatter(
    content: &str, // RAW TEXT — full contents of SKILL.md including the `---` frontmatter block
    path: &Path,   // ERROR CONTEXT — the file path; used only in SkillError variants (not parsed)
) -> Result<(String, String), SkillError> {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return Err(SkillError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: "missing opening ---".into(),
        });
    }

    let after_open = &trimmed[3..];
    let end = after_open
        .find("\n---")
        .ok_or(SkillError::InvalidFrontmatter {
            path: path.to_path_buf(),
            detail: "missing closing ---".into(),
        })?;

    let yaml_block = &after_open[..end];

    let mut name = None;
    let mut description = None;

    for line in yaml_block.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("name:") {
            name = Some(unquote(rest.trim()));
        } else if let Some(rest) = line.strip_prefix("description:") {
            description = Some(unquote(rest.trim()));
        }
    }

    let name = name.ok_or(SkillError::MissingField {
        path: path.to_path_buf(),
        field: "name",
    })?;
    let description = description.ok_or(SkillError::MissingField {
        path: path.to_path_buf(),
        field: "description",
    })?;

    if name.is_empty() {
        return Err(SkillError::MissingField {
            path: path.to_path_buf(),
            field: "name",
        });
    }
    if description.is_empty() {
        return Err(SkillError::MissingField {
            path: path.to_path_buf(),
            field: "description",
        });
    }

    Ok((name, description))
}

/// Remove surrounding quotes from a YAML value.
fn unquote(s: &str) -> String {
    if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Minimal XML escaping for prompt generation.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Render a YAML scalar value, choosing **bare** (unquoted) when YAML plain-scalar rules
/// permit it and a **double-quoted + escaped** form otherwise (F2.b conditional escaping).
///
/// The double-quoted form is emitted when ANY quote-trigger fires, so the value always
/// round-trips back as the original STRING (never as a YAML bool/null/number/date):
/// - the string is empty;
/// - it has leading or trailing whitespace;
/// - it contains a `:`, `#`, newline, tab, carriage-return, `"`, `'`, or `\`;
/// - it begins with a YAML indicator char (`- ? : , [ ] { } & * ! | > % @ \``, or `~`);
/// - it is a reserved/ambiguous plain scalar — `true`/`false`/`null`/`yes`/`no`/`on`/`off`/`~`
///   (any case) — or it looks numeric / float / hex / octal / date-like (`42`, `3.14`,
///   `0x1F`, `2026-06-12`).
///
/// When quoting, it escapes (backslash first) `\`→`\\`, `"`→`\"`, newline→`\n`, tab→`\t`,
/// cr→`\r`. Private + single-file, mirroring `xml_escape`.
fn yaml_escape_scalar(s: &str) -> String {
    if yaml_scalar_needs_quoting(s) {
        let escaped = s
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
            .replace('\r', "\\r");
        format!("\"{}\"", escaped)
    } else {
        s.to_string()
    }
}

/// Decide whether a YAML scalar must be double-quoted to round-trip as a string.
fn yaml_scalar_needs_quoting(s: &str) -> bool {
    // Empty → must quote (bare empty scalar parses as null).
    if s.is_empty() {
        return true;
    }

    // Leading / trailing whitespace → must quote (otherwise stripped or ambiguous).
    if s.starts_with(char::is_whitespace) || s.ends_with(char::is_whitespace) {
        return true;
    }

    // Any structural / special char anywhere → must quote.
    if s.chars()
        .any(|c| matches!(c, ':' | '#' | '\n' | '\t' | '\r' | '"' | '\'' | '\\'))
    {
        return true;
    }

    // Leading YAML indicator char → must quote.
    if let Some(first) = s.chars().next() {
        if matches!(
            first,
            '-' | '?'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '%'
                | '@'
                | '`'
                | '~'
        ) {
            return true;
        }
    }

    // Reserved / ambiguous plain scalars (bool / null spellings, any case) → must quote.
    let lower = s.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "true" | "false" | "null" | "yes" | "no" | "on" | "off" | "~"
    ) {
        return true;
    }

    // Numeric / float / hex / octal / date-looking → must quote so it stays a string.
    if looks_numeric_or_date(s) {
        return true;
    }

    false
}

/// Heuristic: does the scalar look like a number, float, hex, octal, or date so a YAML
/// parser would type-coerce it away from a string?
fn looks_numeric_or_date(s: &str) -> bool {
    // Integer / float (with optional leading sign): all chars are digits / one dot / sign.
    let body = s.strip_prefix(['+', '-']).unwrap_or(s);
    if !body.is_empty() {
        // Plain integer.
        if body.bytes().all(|b| b.is_ascii_digit()) {
            return true;
        }
        // Float: digits with exactly one dot, at least one digit present.
        if body.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && body.bytes().filter(|b| *b == b'.').count() == 1
            && body.bytes().any(|b| b.is_ascii_digit())
        {
            return true;
        }
    }

    // Hex (0x…) / octal (0o…).
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_hexdigit()) {
            return true;
        }
    }
    if let Some(rest) = s.strip_prefix("0o").or_else(|| s.strip_prefix("0O")) {
        if !rest.is_empty() && rest.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
            return true;
        }
    }

    // ISO-8601-ish date (YYYY-MM-DD).
    let date_parts: Vec<&str> = s.split('-').collect();
    if date_parts.len() == 3
        && date_parts[0].len() == 4
        && date_parts[1].len() == 2
        && date_parts[2].len() == 2
        && date_parts
            .iter()
            .all(|p| p.bytes().all(|b| b.is_ascii_digit()))
    {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn create_skill(dir: &Path, name: &str, description: &str) {
        let skill_dir = dir.join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {}\n---\n\n# {}\n\nInstructions here.\n",
                name, description, name
            ),
        )
        .unwrap();
    }

    #[test]
    fn load_skills_from_directory() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get current weather and forecasts.");
        create_skill(tmp.path(), "git", "Git operations: commit, branch, merge.");

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills.skills()[0].name, "git");
        assert_eq!(skills.skills()[1].name, "weather");
    }

    #[test]
    fn format_for_prompt_xml() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get weather.");

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        let prompt = skills.format_for_prompt();

        assert!(prompt.contains("<available_skills>"));
        assert!(prompt.contains("<name>weather</name>"));
        assert!(prompt.contains("<description>Get weather.</description>"));
        assert!(prompt.contains("SKILL.md</location>"));
        assert!(prompt.contains("</available_skills>"));
    }

    #[test]
    fn empty_when_no_skills() {
        let tmp = TempDir::new().unwrap();
        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert!(skills.is_empty());
        assert_eq!(skills.format_for_prompt(), "");
    }

    #[test]
    fn later_dirs_override_earlier() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        create_skill(dir1.path(), "weather", "Old description.");
        create_skill(dir2.path(), "weather", "New description.");

        let skills = SkillSet::load(&[dir1.path(), dir2.path()]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills.skills()[0].description, "New description.");
    }

    #[test]
    fn skips_nonexistent_dirs() {
        let skills = SkillSet::load(&[Path::new("/nonexistent/path")]).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn skips_dirs_without_skill_md() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("not-a-skill")).unwrap();
        fs::write(tmp.path().join("not-a-skill/README.md"), "hello").unwrap();

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert!(skills.is_empty());
    }

    #[test]
    fn error_on_missing_frontmatter() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("bad-skill");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "# No frontmatter\n").unwrap();

        let result = SkillSet::load(&[tmp.path()]);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_missing_name() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("no-name");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Has desc but no name.\n---\n",
        )
        .unwrap();

        let result = SkillSet::load(&[tmp.path()]);
        assert!(result.is_err());
    }

    #[test]
    fn error_on_missing_description() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("no-desc");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: no-desc\n---\n").unwrap();

        let result = SkillSet::load(&[tmp.path()]);
        assert!(result.is_err());
    }

    #[test]
    fn quoted_frontmatter_values() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("quoted");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: \"quoted\"\ndescription: 'A quoted description.'\n---\n",
        )
        .unwrap();

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert_eq!(skills.skills()[0].name, "quoted");
        assert_eq!(skills.skills()[0].description, "A quoted description.");
    }

    #[test]
    fn xml_escaping() {
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("escape-test");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: escape-test\ndescription: Uses <tags> & \"quotes\"\n---\n",
        )
        .unwrap();

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        let prompt = skills.format_for_prompt();
        assert!(prompt.contains("&lt;tags&gt;"));
        assert!(prompt.contains("&amp;"));
        assert!(prompt.contains("&quot;quotes&quot;"));
    }

    // ---- KC-04: selectable skill-prompt layout (XML default + opt-in YAML) ----

    /// Tier A — golden XML byte-for-byte pin. `format_for_prompt()` (the default) must equal
    /// the exact AgentSkills `<available_skills>` block, with no trailing newline, escaping
    /// name/description/location via `xml_escape`. Pinning the exact bytes guards the
    /// byte-for-byte back-compat contract under the new selectable-layout API.
    #[test]
    fn kc04_golden_xml_byte_for_byte() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get weather.");
        let skills = SkillSet::load(&[tmp.path()]).unwrap();

        // Build the exact expected bytes from the loaded skill's own (canonicalized) path,
        // so the golden pins the layout (tags / indent / escaping / no trailing newline)
        // independent of the tmp path.
        let s = &skills.skills()[0];
        let expected = format!(
            "<available_skills>\n  <skill>\n    <name>{}</name>\n    <description>{}</description>\n    <location>{}</location>\n  </skill>\n</available_skills>",
            xml_escape(&s.name),
            xml_escape(&s.description),
            xml_escape(&s.file_path.to_string_lossy()),
        );

        // The default path and the explicit Xml path are byte-identical.
        assert_eq!(skills.format_for_prompt(), expected);
        assert_eq!(
            skills.format_for_prompt_as(SkillPromptFormat::Xml),
            expected
        );
        // No trailing newline (matches the historical contract).
        assert!(!expected.ends_with('\n'));
    }

    /// Tier B — YAML round-trip through `serde_yaml`. A multi-skill sequence renders as valid
    /// YAML; parsing it back recovers every `name`/`description`/`location` field unchanged.
    #[test]
    fn kc04_yaml_round_trips_through_serde_yaml() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Get current weather and forecasts.");
        create_skill(tmp.path(), "git", "Git operations: commit, branch, merge.");
        let skills = SkillSet::load(&[tmp.path()]).unwrap();

        let yaml = skills.format_for_prompt_as(SkillPromptFormat::Yaml);
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("renders valid YAML");

        let seq = value
            .get("available_skills")
            .and_then(|v| v.as_sequence())
            .expect("available_skills sequence present");
        assert_eq!(seq.len(), 2);

        // Skills sorted by name: git then weather. Each field recovers as the original string.
        for (item, skill) in seq.iter().zip(skills.skills().iter()) {
            assert_eq!(item.get("name").unwrap().as_str().unwrap(), skill.name);
            assert_eq!(
                item.get("description").unwrap().as_str().unwrap(),
                skill.description
            );
            assert_eq!(
                item.get("location").unwrap().as_str().unwrap(),
                skill.file_path.to_string_lossy()
            );
        }
        // `git`'s `:`-containing description proves the quote-trigger fired (else invalid YAML).
        assert!(yaml.contains("Git operations: commit, branch, merge."));
    }

    /// Tier C-1 — escaping decision boundary: a `:`-containing description QUOTES and recovers
    /// as a STRING, while a plain safe value (`weather`) stays BARE in the rendered bytes and
    /// still recovers.
    #[test]
    fn kc04_yaml_decision_boundary_colon_quotes_plain_stays_bare() {
        let tmp = TempDir::new().unwrap();
        create_skill(tmp.path(), "weather", "Weather: now with colons.");
        let skills = SkillSet::load(&[tmp.path()]).unwrap();

        let yaml = skills.format_for_prompt_as(SkillPromptFormat::Yaml);
        // The plain `weather` name renders BARE (no surrounding quotes).
        assert!(yaml.contains("- name: weather\n"));
        assert!(!yaml.contains("- name: \"weather\""));
        // The `:`-containing description renders QUOTED.
        assert!(yaml.contains("description: \"Weather: now with colons.\""));

        // Round-trip: both recover as the original strings.
        let value: serde_yaml::Value = serde_yaml::from_str(&yaml).expect("valid YAML");
        let item = &value["available_skills"][0];
        assert_eq!(item["name"].as_str().unwrap(), "weather");
        assert_eq!(
            item["description"].as_str().unwrap(),
            "Weather: now with colons."
        );

        // Helper-level decision boundary.
        assert_eq!(yaml_escape_scalar("weather"), "weather");
        assert_eq!(
            yaml_escape_scalar("Weather: now with colons."),
            "\"Weather: now with colons.\""
        );
    }

    /// Tier C-2 (MUST-SHIP) — reserved-word / numeric type-preservation. Values like `null`,
    /// `true`, and `42` MUST quote so they round-trip back as STRINGS, not as YAML null / bool
    /// / integer. A missed trigger here is a silent type-change defect — the highest-value
    /// F2.b regression.
    #[test]
    fn kc04_yaml_reserved_words_and_numbers_preserve_string_type() {
        for raw in [
            "null",
            "Null",
            "NULL",
            "true",
            "True",
            "false",
            "yes",
            "no",
            "on",
            "off",
            "~",
            "42",
            "-7",
            "3.14",
            "0x1F",
            "0o17",
            "2026-06-12",
        ] {
            let escaped = yaml_escape_scalar(raw);
            assert!(
                escaped.starts_with('"') && escaped.ends_with('"'),
                "{raw} must be quoted but was rendered bare as {escaped}"
            );

            // Round-trip through serde_yaml as a mapping value: must recover as a STRING.
            let doc = format!("v: {escaped}");
            let value: serde_yaml::Value = serde_yaml::from_str(&doc).expect("valid YAML");
            assert_eq!(
                value["v"].as_str(),
                Some(raw),
                "{raw} did not round-trip as a string"
            );
        }

        // A genuinely-safe word stays bare and still recovers.
        assert_eq!(yaml_escape_scalar("weather"), "weather");
        let value: serde_yaml::Value = serde_yaml::from_str("v: weather").unwrap();
        assert_eq!(value["v"].as_str(), Some("weather"));
    }

    /// Tier D — empty-set renders `""` for BOTH formats (same early-return as the XML branch).
    #[test]
    fn kc04_empty_set_returns_empty_for_both_formats() {
        let tmp = TempDir::new().unwrap();
        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert!(skills.is_empty());
        assert_eq!(skills.format_for_prompt_as(SkillPromptFormat::Xml), "");
        assert_eq!(skills.format_for_prompt_as(SkillPromptFormat::Yaml), "");
        assert_eq!(skills.format_for_prompt(), "");
    }

    #[test]
    fn merge_skill_sets() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();
        create_skill(dir1.path(), "weather", "Weather v1.");
        create_skill(dir1.path(), "git", "Git operations.");
        create_skill(dir2.path(), "weather", "Weather v2.");
        create_skill(dir2.path(), "docker", "Docker management.");

        let mut set1 = SkillSet::load(&[dir1.path()]).unwrap();
        let set2 = SkillSet::load(&[dir2.path()]).unwrap();
        set1.merge(set2);

        assert_eq!(set1.len(), 3);
        let names: Vec<&str> = set1.skills().iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["docker", "git", "weather"]);
        // weather should be v2 (merged override)
        assert_eq!(
            set1.skills()
                .iter()
                .find(|s| s.name == "weather")
                .unwrap()
                .description,
            "Weather v2."
        );
    }

    #[test]
    fn load_real_agentskills_format() {
        // Test with metadata field (should be ignored, we only parse name+description)
        let tmp = TempDir::new().unwrap();
        let skill_dir = tmp.path().join("nano-banana-pro");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            r#"---
name: nano-banana-pro
description: Generate or edit images via Gemini 3 Pro Image.
metadata:
  {
    "openclaw":
      {
        "emoji": "🍌",
        "requires": { "bins": ["uv"], "env": ["GEMINI_API_KEY"] },
      },
  }
---

# Nano Banana Pro

Use the bundled script to generate images.
"#,
        )
        .unwrap();

        let skills = SkillSet::load(&[tmp.path()]).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills.skills()[0].name, "nano-banana-pro");
        assert_eq!(
            skills.skills()[0].description,
            "Generate or edit images via Gemini 3 Pro Image."
        );
    }
}
