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
    pub fn format_for_prompt(&self) -> String {
        if self.skills.is_empty() {
            return String::new();
        }

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
