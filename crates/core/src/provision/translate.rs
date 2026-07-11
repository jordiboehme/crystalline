//! Cross-dialect translation of command and agent artifacts.
//!
//! An artifact is authored once, in a source dialect a scan auto-detects from
//! its extension and structure, and provisioned into every harness whose format
//! can carry it. Where the target speaks the same dialect the artifact was
//! authored in, the source bytes pass through verbatim ([`Translated::Emit`]
//! with a [`DesiredPayload::File`]) so hashes, hand-edit detection and `.bak`
//! semantics stay exact for that native harness. Only a genuinely different
//! target format is generated ([`DesiredPayload::Rendered`]).
//!
//! Two dialect gaps exist to bridge:
//!
//! - Commands and Codex prompts are one markdown dialect that differs only in
//!   nesting: Claude keeps `<ns>/<name>.md` namespaces, Codex prompts live in a
//!   flat directory. So a command's bytes never change across the two; only its
//!   key flattens (`charts/plot-route.md` -> `charts-plot-route.md`) for Codex.
//! - Agents split by extension: a `.md` agent is the markdown dialect Claude and
//!   GitHub Copilot both read natively; a `.toml` agent is the Codex dialect.
//!   Crossing between them is a real format conversion (frontmatter and body
//!   <-> `name`/`description`/`developer_instructions`), and every field that
//!   has no counterpart in the target - a vendor model name, a permission mode,
//!   a tool list - is dropped with a notice, never silently.

use crate::harness::HarnessKind;
use crate::manifest::ArtifactType;
use crate::parse::locate;
use crate::provision::model::{ArtifactFile, DesiredPayload};
use crate::provision::receipt::sha256_hex;
use crate::yaml::YamlValue;
use indexmap::IndexMap;

/// What projecting one source artifact onto one harness produced.
pub(crate) enum Translated {
    /// A desired file at kind-relative `rel`, with the payload and hash to
    /// install and any per-field notices raised while rendering it.
    Emit {
        /// The kind-relative key the file lands at (post-flatten, post-rename).
        rel: String,
        /// The bytes to write: a passthrough source path or rendered bytes.
        payload: DesiredPayload,
        /// Lowercase hex sha256 of the bytes actually written.
        sha256: String,
        /// Notices for fields dropped while rendering a cross-dialect target.
        notices: Vec<String>,
    },
    /// This artifact is skipped for this harness (a parse failure on a
    /// cross-dialect source, an agent with no description to render), with one
    /// user-facing notice explaining why. The rest of the domain still projects.
    Skip {
        /// The single notice explaining the skip.
        notice: String,
    },
    /// This harness provisions nothing of this kind at all (GitHub Copilot has
    /// no command surface). The caller raises one notice per `(domain, kind)`,
    /// however many artifacts of the kind the domain ships.
    Unsupported,
}

/// Project one scanned source artifact onto one harness. Skills are one
/// cross-harness format and always pass through; commands and agents route
/// through the dialect logic above.
pub(crate) fn translate_file(harness: HarnessKind, file: &ArtifactFile) -> Translated {
    match file.kind {
        ArtifactType::Skills => passthrough(file),
        ArtifactType::Commands => translate_command(harness, file),
        ArtifactType::Agents => translate_agent(harness, file),
        // MCP configs are not file artifacts (a scan keeps them separate), so a
        // file translation never sees one; degrade defensively rather than panic.
        ArtifactType::Mcps => Translated::Unsupported,
    }
}

/// Emit `file` unchanged: a byte-identical passthrough keyed at its own `rel`,
/// hashed at the source hash the scan already computed.
fn passthrough(file: &ArtifactFile) -> Translated {
    Translated::Emit {
        rel: file.rel.clone(),
        payload: DesiredPayload::File(file.source.clone()),
        sha256: file.sha256.clone(),
        notices: Vec::new(),
    }
}

// --- commands ---------------------------------------------------------------

/// Project a command. Claude keeps the nested namespace path; Codex flattens it
/// into its flat prompts directory; GitHub Copilot has no command surface.
///
/// Codex custom prompts are deprecated upstream ("use skills for reusable
/// prompts") but still functional, and their frontmatter (`description`,
/// `argument-hint`) matches the Claude command fields exactly, so a command's
/// bytes are identical across the two dialects and only its key flattens. Drop
/// this Codex arm when upstream removes the prompt surface.
fn translate_command(harness: HarnessKind, file: &ArtifactFile) -> Translated {
    match harness {
        HarnessKind::ClaudeCode => passthrough(file),
        HarnessKind::Codex => Translated::Emit {
            rel: Command::flatten_rel(&file.rel),
            payload: DesiredPayload::File(file.source.clone()),
            sha256: file.sha256.clone(),
            notices: Vec::new(),
        },
        HarnessKind::Copilot => Translated::Unsupported,
    }
}

// --- agents -----------------------------------------------------------------

/// Project an agent, choosing passthrough or a format conversion by the source
/// dialect (extension) and the target harness.
fn translate_agent(harness: HarnessKind, file: &ArtifactFile) -> Translated {
    let toml_source = has_extension(&file.rel, "toml");
    let stem = agent_stem(&file.rel);
    match (toml_source, harness) {
        // Markdown agent (Claude and Copilot both read it natively).
        (false, HarnessKind::ClaudeCode) | (false, HarnessKind::Copilot) => passthrough(file),
        // Markdown agent -> Codex TOML.
        (false, HarnessKind::Codex) => render_agent(file, &stem, harness, AgentTarget::Toml),
        // Codex TOML agent (its own native format).
        (true, HarnessKind::Codex) => passthrough(file),
        // Codex TOML agent -> markdown, for Claude or Copilot.
        (true, HarnessKind::ClaudeCode) | (true, HarnessKind::Copilot) => {
            render_agent(file, &stem, harness, AgentTarget::Markdown)
        }
    }
}

/// The format a cross-dialect agent render produces.
#[derive(Clone, Copy)]
enum AgentTarget {
    /// Codex `name`/`description`/`developer_instructions` TOML.
    Toml,
    /// Markdown with `name`/`description` frontmatter and a body.
    Markdown,
}

/// Read `file`, parse it into the canonical [`Agent`] and emit it in `target`'s
/// format, degrading a read or parse failure - or an agent too thin to render -
/// to a skip with one notice. `harness` names the target in every notice.
fn render_agent(
    file: &ArtifactFile,
    stem: &str,
    harness: HarnessKind,
    target: AgentTarget,
) -> Translated {
    let Ok(text) = std::fs::read_to_string(&file.source) else {
        return Translated::Skip {
            notice: format!(
                "could not read the `{stem}` agent to translate it for {} - skipping it.",
                harness.display_name()
            ),
        };
    };
    let parsed = match target {
        AgentTarget::Toml => Agent::parse_markdown(stem, &text),
        AgentTarget::Markdown => Agent::parse_toml(stem, &text),
    };
    let agent = match parsed {
        Ok(agent) => agent,
        Err(reason) => {
            return Translated::Skip {
                notice: format!(
                    "the `{stem}` agent could not be translated for {} ({reason}) - skipping it.",
                    harness.display_name()
                ),
            };
        }
    };
    let (rel, rendered) = match target {
        AgentTarget::Toml => (format!("{stem}.toml"), agent.to_toml()),
        AgentTarget::Markdown => (format!("{stem}.md"), agent.to_markdown()),
    };
    let Some((bytes, dropped)) = rendered else {
        return Translated::Skip {
            notice: format!(
                "the `{stem}` agent declares no description, so it cannot be translated for {} - skipping it.",
                harness.display_name()
            ),
        };
    };
    let sha256 = sha256_hex(&bytes);
    let notices = dropped
        .into_iter()
        .map(|field| {
            format!(
                "the `{field}` field of the `{stem}` agent has no equivalent in {} - dropping it.",
                harness.display_name()
            )
        })
        .collect();
    Translated::Emit {
        rel,
        payload: DesiredPayload::Rendered(bytes),
        sha256,
        notices,
    }
}

/// The canonical agent model, the meeting point of the two dialects. Only
/// `name`, `description` and `instructions` carry across a conversion; every
/// other field an author wrote is kept in `dropped` so a render can name what it
/// left behind.
pub struct Agent {
    /// The agent's identity: its frontmatter/TOML `name`, or the file stem.
    pub name: String,
    /// The one-line description. Required to render into either target format.
    pub description: Option<String>,
    /// The agent's instructions: a markdown body or a TOML
    /// `developer_instructions` block, whitespace-trimmed from either dialect.
    pub instructions: String,
    /// The names of source fields that do not cross to the other dialect - in
    /// document order for markdown frontmatter and in sorted key order for
    /// TOML - so a render can raise one drop notice each.
    pub dropped: Vec<String>,
}

impl Agent {
    /// Parse a markdown agent: YAML frontmatter plus a body. `name` falls back
    /// to `stem` when the frontmatter omits it. A frontmatter block that is not
    /// a valid YAML mapping is an error; an absent block is fine (the agent just
    /// has no description and will not render across dialects).
    pub fn parse_markdown(stem: &str, source: &str) -> Result<Agent, String> {
        let (front, body) = split_frontmatter(source)?;
        let mut name = stem.to_string();
        let mut description = None;
        let mut dropped = Vec::new();
        for (key, value) in front {
            match key.as_str() {
                "name" => {
                    if let Some(s) = value.as_str() {
                        name = s.to_string();
                    }
                }
                "description" => description = value.as_str().map(str::to_string),
                other => dropped.push(other.to_string()),
            }
        }
        Ok(Agent {
            name,
            description,
            instructions: body.trim().to_string(),
            dropped,
        })
    }

    /// Parse a Codex TOML agent through the `toml` crate. `name`, `description`
    /// and `developer_instructions` are the string fields that carry; every
    /// other top-level key (a table counts once, by its top-level name) is
    /// recorded as dropped. Malformed TOML, or a missing required field, is an
    /// error.
    pub fn parse_toml(stem: &str, source: &str) -> Result<Agent, String> {
        let table: toml::Table = source
            .parse()
            .map_err(|e: toml::de::Error| format!("its TOML does not parse: {}", e.message()))?;
        let mut name = stem.to_string();
        let mut description = None;
        let mut instructions = None;
        let mut dropped = Vec::new();
        for (key, value) in table {
            match key.as_str() {
                "name" => {
                    if let Some(s) = value.as_str() {
                        name = s.to_string();
                    }
                }
                "description" => description = value.as_str().map(str::to_string),
                // Trimmed like parse_markdown trims its body, so the canonical
                // model's instructions are whitespace-trimmed from either
                // dialect and a trimmed body round-trips exactly (TOML's own
                // multiline closer otherwise carries a trailing newline).
                "developer_instructions" => {
                    instructions = value.as_str().map(|s| s.trim().to_string());
                }
                _ => dropped.push(key),
            }
        }
        let instructions =
            instructions.ok_or_else(|| "it declares no developer_instructions".to_string())?;
        Ok(Agent {
            name,
            description,
            instructions,
            dropped,
        })
    }

    /// Emit this agent as Codex TOML, or `None` when it has no description to
    /// carry. Returns the bytes and the field names dropped along the way.
    pub fn to_toml(&self) -> Option<(Vec<u8>, Vec<String>)> {
        let description = self.description.as_ref()?;
        let mut out = String::new();
        out.push_str(&format!("name = {}\n", toml_basic(&self.name)));
        out.push_str(&format!("description = {}\n", toml_basic(description)));
        out.push_str("developer_instructions = ");
        out.push_str(&toml_multiline(&self.instructions));
        out.push('\n');
        Some((out.into_bytes(), self.dropped.clone()))
    }

    /// Emit this agent as a markdown agent with `name`/`description`
    /// frontmatter, or `None` when it has no description to carry. Returns the
    /// bytes and the field names dropped along the way.
    pub fn to_markdown(&self) -> Option<(Vec<u8>, Vec<String>)> {
        let description = self.description.as_ref()?;
        let mut out = String::new();
        out.push_str("---\n");
        out.push_str(&format!("name: {}\n", yaml_scalar(&self.name)));
        out.push_str(&format!("description: {}\n", yaml_scalar(description)));
        out.push_str("---\n\n");
        out.push_str(self.instructions.trim());
        out.push('\n');
        Some((out.into_bytes(), self.dropped.clone()))
    }
}

// --- markdown frontmatter ----------------------------------------------------

/// Split a markdown source into its parsed frontmatter mapping (in document
/// order) and its body. An absent frontmatter block yields an empty mapping; a
/// block that is not a valid YAML mapping is an error.
fn split_frontmatter(source: &str) -> Result<(IndexMap<String, YamlValue>, String), String> {
    let (has_fm, fm_span, body_start) = locate(source);
    let body = source[body_start..].to_string();
    if !has_fm {
        return Ok((IndexMap::new(), body));
    }
    let raw = &source[fm_span];
    let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(raw)
        .map_err(|e| format!("its frontmatter is not valid YAML: {e}"))?;
    match YamlValue::from_backend(value) {
        YamlValue::Mapping(map) => Ok((map, body)),
        YamlValue::Null => Ok((IndexMap::new(), body)),
        _ => Err("its frontmatter is not a mapping".to_string()),
    }
}

/// Render a scalar as a YAML frontmatter value, quoting only when the plain
/// form would be ambiguous. Multiline values DO reach this function - a TOML
/// agent can carry a multiline `description` through a `"""` string - and a
/// raw newline would truncate line-based frontmatter at the first line (or
/// worse, a line reading `---` would close the block early), so newlines,
/// carriage returns and every other control character are escaped inside the
/// double-quoted form (`\n`, `\r`, `\t`, `\uXXXX`), which YAML unescapes back
/// to the original value. The emitted frontmatter line is always exactly one
/// line.
fn yaml_scalar(s: &str) -> String {
    let needs_quote = s.is_empty()
        || s.contains(':')
        || s.contains('#')
        || s.contains('"')
        || s.chars().any(|c| c.is_control())
        || s.starts_with([
            ' ', '\'', '"', '-', '[', '{', '&', '*', '!', '|', '>', '%', '@', '`',
        ])
        || s.ends_with(' ')
        || matches!(s, "true" | "false" | "null" | "yes" | "no");
    if !needs_quote {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// --- TOML emission (parsing goes through the `toml` crate) -------------------

/// Render a single-line TOML basic string: backslash, quote, the named
/// whitespace escapes and every other control character (C0 and delete, which
/// TOML forbids raw in a basic string) are escaped, so the emitted string is
/// always valid TOML and round-trips through a TOML parser.
fn toml_basic(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0C}' => out.push_str("\\f"),
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Render an instructions block as a TOML multiline literal (`'''`) string,
/// which needs no escaping and preserves the body verbatim. A body the literal
/// form cannot carry - one containing `'''`, or a control character TOML
/// forbids raw (anything but newline and tab) - falls back to a multiline
/// basic (`"""`) string with those characters escaped and any triple-quote run
/// defused, so the closing delimiter can never be forged and the emitted TOML
/// is always valid.
fn toml_multiline(body: &str) -> String {
    let needs_basic = body.contains("'''")
        || body
            .chars()
            .any(|c| c != '\n' && c != '\t' && ((c as u32) < 0x20 || c as u32 == 0x7F));
    if !needs_basic {
        return format!("'''\n{body}\n'''");
    }
    let mut escaped = String::with_capacity(body.len());
    for c in body.chars() {
        match c {
            '\\' => escaped.push_str("\\\\"),
            '\n' | '\t' => escaped.push(c),
            '\r' => escaped.push_str("\\r"),
            c if (c as u32) < 0x20 || c as u32 == 0x7F => {
                escaped.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => escaped.push(c),
        }
    }
    let escaped = escaped.replace("\"\"\"", "\"\"\\\"");
    format!("\"\"\"\n{escaped}\n\"\"\"")
}

/// Whether `rel`'s file name ends with `.<ext>` (lowercase compared).
fn has_extension(rel: &str, ext: &str) -> bool {
    rel.rsplit('/')
        .next()
        .and_then(|name| name.rsplit_once('.'))
        .is_some_and(|(_, e)| e.eq_ignore_ascii_case(ext))
}

/// The file stem of an agent key: its final path component with a `.md` or
/// `.toml` extension stripped.
fn agent_stem(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    name.strip_suffix(".md")
        .or_else(|| name.strip_suffix(".toml"))
        .unwrap_or(name)
        .to_string()
}

/// The canonical command model, parsed from a Claude command or a Codex prompt -
/// one markdown dialect with the same frontmatter fields. The model owns the
/// flatten mapping ([`Command::flatten_rel`]) the Codex projection routes
/// through; the projection itself passes command bytes through unchanged.
pub struct Command {
    /// The namespace path segments, empty for a flat command.
    pub namespace: Vec<String>,
    /// The command name (its file stem).
    pub name: String,
    /// The `description` frontmatter field, if present.
    pub description: Option<String>,
    /// The `argument-hint` frontmatter field, if present.
    pub argument_hint: Option<String>,
    /// The markdown body after the frontmatter.
    pub body: String,
    /// Any other frontmatter fields, kept in document order.
    pub extra: IndexMap<String, YamlValue>,
}

impl Command {
    /// Parse a command from its kind-relative key and source. The namespace and
    /// name come from the key's nesting; `description`, `argument-hint` and any
    /// extras come from optional frontmatter. Malformed YAML frontmatter is an
    /// error; an absent block is fine, since a command may be a bare body.
    pub fn parse(rel: &str, source: &str) -> Result<Command, String> {
        let mut segments: Vec<String> = rel.split('/').map(str::to_string).collect();
        let file = segments.pop().unwrap_or_default();
        let name = file.strip_suffix(".md").unwrap_or(&file).to_string();
        let (front, body) = split_frontmatter(source)?;
        let mut description = None;
        let mut argument_hint = None;
        let mut extra = IndexMap::new();
        for (key, value) in front {
            match key.as_str() {
                "description" => description = value.as_str().map(str::to_string),
                "argument-hint" => argument_hint = value.as_str().map(str::to_string),
                _ => {
                    extra.insert(key, value);
                }
            }
        }
        Ok(Command {
            namespace: segments,
            name,
            description,
            argument_hint,
            body,
            extra,
        })
    }

    /// The flat `<ns>-<name>.md` key this command installs at for Codex,
    /// derived through [`Command::flatten_rel`].
    pub fn flat_key(&self) -> String {
        let mut parts = self.namespace.clone();
        parts.push(self.name.clone());
        Self::flatten_rel(&format!("{}.md", parts.join("/")))
    }

    /// Flatten a nested, `/`-separated command key into the flat
    /// `<ns>-<name>.md` shape Codex prompts require:
    /// `charts/plot-route.md` -> `charts-plot-route.md`, a flat `deploy.md`
    /// staying `deploy.md`. The single flatten implementation, used by the
    /// Codex projection in this module and by [`Command::flat_key`].
    pub fn flatten_rel(rel: &str) -> String {
        rel.replace('/', "-")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- command parse -------------------------------------------------------

    #[test]
    fn command_parse_reads_namespace_name_and_frontmatter() {
        let source = "---\ndescription: Plot a route\nargument-hint: <from> <to>\n---\n\nPlot a route between two buoys.\n";
        let command = Command::parse("charts/plot-route.md", source).unwrap();
        assert_eq!(command.namespace, vec!["charts"]);
        assert_eq!(command.name, "plot-route");
        assert_eq!(command.description.as_deref(), Some("Plot a route"));
        assert_eq!(command.argument_hint.as_deref(), Some("<from> <to>"));
        assert!(command.body.contains("Plot a route between two buoys."));
        assert_eq!(command.flat_key(), "charts-plot-route.md");
    }

    #[test]
    fn flat_command_parses_with_no_namespace_and_no_frontmatter() {
        let command = Command::parse("deploy.md", "Just do it.\n").unwrap();
        assert!(command.namespace.is_empty());
        assert_eq!(command.name, "deploy");
        assert_eq!(command.description, None);
        assert_eq!(command.flat_key(), "deploy.md");
    }

    #[test]
    fn command_flatten_replaces_every_separator() {
        assert_eq!(
            Command::flatten_rel("charts/plot-route.md"),
            "charts-plot-route.md"
        );
        assert_eq!(Command::flatten_rel("a/b/c.md"), "a-b-c.md");
        assert_eq!(Command::flatten_rel("deploy.md"), "deploy.md");
    }

    // --- agent parse ---------------------------------------------------------

    #[test]
    fn markdown_agent_parse_reads_fields_and_records_extras() {
        let source = "---\nname: quartermaster\ndescription: Keeps the stores\nmodel: opus\ntools:\n  - Read\n---\n\nKeep the ship's stores in order.\n";
        let agent = Agent::parse_markdown("fallback", source).unwrap();
        assert_eq!(agent.name, "quartermaster");
        assert_eq!(agent.description.as_deref(), Some("Keeps the stores"));
        assert_eq!(agent.instructions, "Keep the ship's stores in order.");
        // model and tools do not cross to TOML: both are recorded as dropped.
        assert_eq!(agent.dropped, vec!["model", "tools"]);
    }

    #[test]
    fn markdown_agent_without_frontmatter_has_no_description() {
        let agent = Agent::parse_markdown("scout", "Scout ahead.\n").unwrap();
        assert_eq!(agent.name, "scout");
        assert_eq!(agent.description, None);
        assert!(
            agent.to_toml().is_none(),
            "no description: cannot render TOML"
        );
    }

    #[test]
    fn markdown_agent_with_broken_frontmatter_is_an_error() {
        let source = "---\ndescription: [unterminated\n---\n\nbody\n";
        assert!(Agent::parse_markdown("x", source).is_err());
    }

    #[test]
    fn toml_agent_parse_reads_fields_and_records_extras() {
        let source = "name = \"reviewer\"\ndescription = \"Reviews code\"\nmodel = \"gpt-5-codex\"\ndeveloper_instructions = '''\nBe thorough.\nName every risk.\n'''\n";
        let agent = Agent::parse_toml("fallback", source).unwrap();
        assert_eq!(agent.name, "reviewer");
        assert_eq!(agent.description.as_deref(), Some("Reviews code"));
        assert_eq!(agent.instructions, "Be thorough.\nName every risk.");
        assert_eq!(agent.dropped, vec!["model"]);
    }

    #[test]
    fn toml_agent_parse_reads_a_basic_multiline_block() {
        let source = "name = \"reviewer\"\ndescription = \"Reviews code\"\ndeveloper_instructions = \"\"\"\nLine one.\nLine two.\n\"\"\"\n";
        let agent = Agent::parse_toml("fallback", source).unwrap();
        assert_eq!(agent.instructions, "Line one.\nLine two.");
    }

    #[test]
    fn toml_agent_records_a_table_as_a_single_dropped_field() {
        // The whole `[skills.config]` table drops as its one top-level key.
        let source = "name = \"reviewer\"\ndescription = \"Reviews code\"\ndeveloper_instructions = '''\nGo.\n'''\n\n[skills.config]\nfoo = \"bar\"\nbaz = \"qux\"\n";
        let agent = Agent::parse_toml("fallback", source).unwrap();
        assert_eq!(agent.dropped, vec!["skills"]);
    }

    #[test]
    fn toml_agent_missing_required_field_is_an_error() {
        let source = "name = \"reviewer\"\ndescription = \"Reviews code\"\n";
        assert!(Agent::parse_toml("x", source).is_err());
    }

    // --- agent emit goldens --------------------------------------------------

    #[test]
    fn markdown_agent_emits_expected_codex_toml_and_drop_list() {
        let source = "---\nname: quartermaster\ndescription: Keeps the stores\nmodel: opus\ntools:\n  - Read\n  - Grep\npermissionMode: acceptEdits\n---\n\n# Quartermaster\n\nKeep the ship's stores in order.\n";
        let agent = Agent::parse_markdown("quartermaster", source).unwrap();
        let (bytes, dropped) = agent.to_toml().unwrap();
        let toml = String::from_utf8(bytes).unwrap();
        let expected = "name = \"quartermaster\"\n\
description = \"Keeps the stores\"\n\
developer_instructions = '''\n\
# Quartermaster\n\n\
Keep the ship's stores in order.\n\
'''\n";
        assert_eq!(toml, expected);
        assert_eq!(dropped, vec!["model", "tools", "permissionMode"]);
    }

    #[test]
    fn toml_agent_emits_expected_markdown_and_drop_list() {
        let source = "name = \"reviewer\"\ndescription = \"Reviews code\"\nmodel = \"gpt-5-codex\"\nsandbox_mode = \"workspace-write\"\ndeveloper_instructions = '''\nBe thorough.\nName every risk.\n'''\n";
        let agent = Agent::parse_toml("reviewer", source).unwrap();
        let (bytes, dropped) = agent.to_markdown().unwrap();
        let md = String::from_utf8(bytes).unwrap();
        let expected = "---\n\
name: reviewer\n\
description: Reviews code\n\
---\n\n\
Be thorough.\n\
Name every risk.\n";
        assert_eq!(md, expected);
        assert_eq!(dropped, vec!["model", "sandbox_mode"]);
    }

    // --- round-trip of the instructions body ---------------------------------

    #[test]
    fn instructions_round_trip_through_toml_multiline() {
        let body = "First paragraph.\n\nSecond, with a \"quote\" and a tab\tinside.";
        let toml = format!("developer_instructions = {}\n", toml_multiline(body));
        let full = format!("name = \"x\"\ndescription = \"y\"\n{toml}");
        let agent = Agent::parse_toml("x", &full).unwrap();
        assert_eq!(agent.instructions, body);
    }

    #[test]
    fn instructions_with_triple_quote_defuse_the_delimiter() {
        // A literal ''' in the body forces the basic-string fallback; the value
        // still round-trips and the closing delimiter is never forged.
        let body = "Do not write ''' here.";
        let rendered = toml_multiline(body);
        assert!(
            rendered.starts_with("\"\"\""),
            "fell back to a basic string"
        );
        let full =
            format!("name = \"x\"\ndescription = \"y\"\ndeveloper_instructions = {rendered}\n");
        let agent = Agent::parse_toml("x", &full).unwrap();
        assert_eq!(agent.instructions, body);
    }

    #[test]
    fn hostile_quote_runs_next_to_the_delimiter_round_trip() {
        // Regression: a body mixing a ''' run with a long run of double quotes
        // (`x'''""""""`) mis-parsed under the earlier hand-rolled reader. The
        // emitter must defuse every triple-quote run and the `toml` crate must
        // read the value back exactly.
        let body = "x'''\"\"\"\"\"\"";
        let rendered = toml_multiline(body);
        let full =
            format!("name = \"x\"\ndescription = \"y\"\ndeveloper_instructions = {rendered}\n");
        let agent = Agent::parse_toml("x", &full).unwrap();
        assert_eq!(agent.instructions, body);
    }

    #[test]
    fn control_characters_in_the_body_emit_valid_toml() {
        // A body with a carriage return and another C0 control character can
        // ride neither the literal form nor raw basic content; the emitter must
        // escape them and the value must still round-trip.
        let body = "line one\r\nbell \u{07} done";
        let rendered = toml_multiline(body);
        assert!(rendered.starts_with("\"\"\""), "control chars force basic");
        let full =
            format!("name = \"x\"\ndescription = \"y\"\ndeveloper_instructions = {rendered}\n");
        let agent = Agent::parse_toml("x", &full).unwrap();
        assert_eq!(agent.instructions, body);
    }

    #[test]
    fn control_characters_in_name_and_description_emit_valid_toml() {
        // Minor-1: toml_basic must escape \r and every other control character
        // so a single-line field can never emit invalid TOML.
        let agent = Agent {
            name: "odd\rname".to_string(),
            description: Some("desc with \u{01} control".to_string()),
            instructions: "Go.".to_string(),
            dropped: Vec::new(),
        };
        let (bytes, _) = agent.to_toml().unwrap();
        let toml = String::from_utf8(bytes).unwrap();
        let back = Agent::parse_toml("x", &toml).unwrap();
        assert_eq!(back.name, "odd\rname");
        assert_eq!(
            back.description.as_deref(),
            Some("desc with \u{01} control")
        );
    }

    #[test]
    fn multiline_toml_description_survives_markdown_frontmatter() {
        // Important-1 regression: a TOML agent can carry a multiline
        // description (a `"""` string), and a raw newline - or a line reading
        // `---` - inside the emitted frontmatter would truncate it. The
        // emitted frontmatter must stay one line per field and the value must
        // round-trip exactly through a markdown re-parse.
        let source = "name = \"reviewer\"\ndescription = \"\"\"\nFirst line.\n--- not a delimiter\n\"\"\"\ndeveloper_instructions = '''\nGo.\n'''\n";
        let agent = Agent::parse_toml("reviewer", source).unwrap();
        let description = agent.description.clone().expect("description parsed");
        assert!(description.contains('\n'), "the fixture is multiline");

        let (bytes, _) = agent.to_markdown().unwrap();
        let md = String::from_utf8(bytes).unwrap();
        let back = Agent::parse_markdown("reviewer", &md).unwrap();
        assert_eq!(back.description.as_deref(), Some(description.as_str()));
        assert_eq!(back.name, "reviewer");
        assert_eq!(back.instructions, "Go.");
    }
}
