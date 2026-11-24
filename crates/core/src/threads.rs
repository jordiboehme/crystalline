//! Threads: discussions anchored on a passage of a document, stored in a
//! thread file the document's kind places. One `##` per thread. The model is
//! the familiar document comment: an anchor on a section plus a quoted
//! passage, a status, and a history that is only ever appended to. The
//! format knows no kind of document: the anchor names its section by id or
//! by heading, and the caller hands in how to find that section's text.
//! Where a kind keeps its thread file is that kind's own choice.

use serde::{Deserialize, Serialize};

/// How an anchor names its section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectionRef {
    /// A section id: what a document with id lines (a reflection) writes.
    Id(String),
    /// A heading path (`## API > ### Auth`): what a document without id
    /// lines will write.
    Heading(String),
}

impl SectionRef {
    /// The id, for an anchor addressed by id.
    pub fn id(&self) -> Option<&str> {
        match self {
            SectionRef::Id(id) => Some(id),
            SectionRef::Heading(_) => None,
        }
    }

    /// The id or the heading path, whichever the anchor names.
    pub fn key(&self) -> &str {
        match self {
            SectionRef::Id(key) | SectionRef::Heading(key) => key,
        }
    }
}

/// Where a thread is anchored.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    /// The section.
    pub section: SectionRef,
    /// The quoted passage; `None` for the whole section.
    pub quote: Option<String>,
    /// Which occurrence of the quote in the section, 1-based.
    pub occurrence: u32,
}

/// What a thread is about. The one place this module names a kind of
/// document; the engine dispatches on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadTarget {
    /// A Reflection page, by its path inside `reflections/`. Its threads
    /// live in `<page>.threads.md` beside it.
    Reflection { path: String },
}

/// Open or resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThreadStatus {
    /// Waiting for somebody.
    Open,
    /// Closed with an outcome; reopening is a new entry.
    Resolved,
}

impl ThreadStatus {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadStatus::Open => "open",
            ThreadStatus::Resolved => "resolved",
        }
    }
}

/// What one entry did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Said something.
    Comment,
    /// Closed the thread with an outcome.
    Resolve,
    /// Opened a resolved thread again.
    Reopen,
}

impl EntryKind {
    /// The wire word, which is also the word in the file.
    pub fn as_str(self) -> &'static str {
        match self {
            EntryKind::Comment => "comment",
            EntryKind::Resolve => "resolve",
            EntryKind::Reopen => "reopen",
        }
    }

    fn parse(word: &str) -> Option<EntryKind> {
        match word {
            "comment" => Some(EntryKind::Comment),
            "resolve" => Some(EntryKind::Resolve),
            "reopen" => Some(EntryKind::Reopen),
            _ => None,
        }
    }
}

/// Who wrote an entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Author {
    /// The account or agent label, sanitized.
    pub name: String,
    /// Whether an agent wrote it.
    pub agent: bool,
}

/// One entry in a thread's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// RFC 3339.
    pub at: String,
    /// What it did.
    pub kind: EntryKind,
    /// Who.
    pub author: Author,
    /// What was said; may span lines; may be empty for a reopen.
    pub text: String,
}

/// One thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    /// `t-` plus 8 hex digits.
    pub id: String,
    /// Where it is anchored.
    pub anchor: Anchor,
    /// Open or resolved.
    pub status: ThreadStatus,
    /// The history, oldest first.
    pub entries: Vec<Entry>,
}

impl Thread {
    /// When the first entry was written.
    pub fn opened_at(&self) -> &str {
        self.entries.first().map(|e| e.at.as_str()).unwrap_or("")
    }

    /// When the last entry was written.
    pub fn last_at(&self) -> &str {
        self.entries.last().map(|e| e.at.as_str()).unwrap_or("")
    }
}

/// The yaml block of one thread. `section` for an anchor by id, `heading`
/// for one by heading path; exactly one of them is written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Meta {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    section: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    heading: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    quote: Option<String>,
    #[serde(default = "one")]
    occurrence: u32,
    status: ThreadStatus,
}

fn one() -> u32 {
    1
}

/// Why an entry could not be appended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadError {
    /// No thread with this id in the file.
    NoThread(String),
    /// The thread is there and the entry does not fit its state.
    Refused(String),
}

impl std::fmt::Display for ThreadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ThreadError::NoThread(id) => write!(f, "no thread {id} here"),
            ThreadError::Refused(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for ThreadError {}

/// A fresh thread id: `t-` plus 8 lowercase hex digits, the first a letter.
pub fn random_thread_id() -> String {
    loop {
        let hex = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        if hex.as_bytes()[0].is_ascii_alphabetic() {
            return format!("t-{hex}");
        }
    }
}

/// A name that cannot break an entry line: parentheses, colons, commas and
/// line breaks become spaces, runs of spaces collapse, an empty name is
/// `someone`.
pub fn sanitize_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '(' | ')' | ':' | '\n' | '\r' | ',') {
                ' '
            } else {
                c
            }
        })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if joined.is_empty() {
        "someone".to_string()
    } else {
        joined
    }
}

fn render_entry(entry: &Entry) -> String {
    let who = if entry.author.agent {
        "agent"
    } else {
        "person"
    };
    let mut lines = entry.text.lines();
    let first = lines.next().unwrap_or("");
    let mut out = format!(
        "- {}, {} by {} ({who})",
        entry.at,
        entry.kind.as_str(),
        sanitize_name(&entry.author.name)
    );
    if !first.trim().is_empty() {
        out.push_str(": ");
        out.push_str(first);
    }
    out.push('\n');
    for line in lines {
        // An empty line is written empty, not as two spaces an editor would
        // strip; the reader keeps it as a paragraph break either way.
        if !line.is_empty() {
            out.push_str("  ");
            out.push_str(line);
        }
        out.push('\n');
    }
    out
}

fn parse_entry(line: &str) -> Option<Entry> {
    let rest = line.strip_prefix("- ")?;
    let (at, rest) = rest.split_once(", ")?;
    let (kind, rest) = rest.split_once(" by ")?;
    let open = rest.find(" (")?;
    let name = &rest[..open];
    let after = &rest[open + 2..];
    let (who, text) = match after.split_once("): ") {
        Some((who, text)) => (who, text),
        None => (after.strip_suffix(')')?, ""),
    };
    Some(Entry {
        at: at.to_string(),
        kind: EntryKind::parse(kind)?,
        author: Author {
            name: name.to_string(),
            agent: who == "agent",
        },
        text: text.to_string(),
    })
}

fn render_thread(thread: &Thread) -> String {
    let (section, heading) = match &thread.anchor.section {
        SectionRef::Id(id) => (Some(id.clone()), None),
        SectionRef::Heading(path) => (None, Some(path.clone())),
    };
    let meta = Meta {
        section,
        heading,
        quote: thread.anchor.quote.clone(),
        occurrence: thread.anchor.occurrence.max(1),
        status: thread.status,
    };
    let yaml = serde_yaml_ng::to_string(&meta).unwrap_or_default();
    let mut out = format!("## {}\n\n```yaml\n{yaml}```\n\n", thread.id);
    for entry in &thread.entries {
        out.push_str(&render_entry(entry));
    }
    out
}

/// One thread's place in a threads file: its id (the `##` line trimmed),
/// where the `##` line starts, where the text under it starts and where the
/// thread ends (the next `##` line or the end of the file).
struct Span<'a> {
    id: &'a str,
    start: usize,
    body: usize,
    end: usize,
}

/// Every `## ` line of `text` and the stretch it heads. Text before the first
/// one (the `# Threads` title) belongs to no thread, and a file without the
/// title loses nothing.
fn thread_spans(text: &str) -> Vec<Span<'_>> {
    let mut spans: Vec<Span<'_>> = Vec::new();
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        if let Some(id) = line.strip_prefix("## ") {
            if let Some(last) = spans.last_mut() {
                last.end = at;
            }
            spans.push(Span {
                id: id.trim(),
                start: at,
                body: at + line.len(),
                end: text.len(),
            });
        }
        at += line.len();
    }
    spans
}

/// The yaml block of one thread's text (everything after its `##` line):
/// the byte range of the yaml between the opening fence line (```` ```yaml ````,
/// spaces allowed after the backticks and at the end) and the first line
/// that is only the closing fence (trailing spaces allowed), and where the
/// entries start. A fence inside a quoted passage is never a line of its own
/// (the quote is a YAML scalar on the `quote:` line, or indented under it).
fn yaml_block(rest: &str) -> Option<(usize, usize, usize)> {
    let mut at = 0;
    let mut open = None;
    for line in rest.split_inclusive('\n') {
        let bare = line.trim_end();
        match open {
            None if bare.strip_prefix("```").is_some_and(|r| r.trim() == "yaml") => {
                open = Some(at + line.len());
            }
            Some(open) if bare == "```" => return Some((open, at, at + line.len())),
            _ => {}
        }
        at += line.len();
    }
    None
}

/// The thread a span holds, with its yaml block as read, or `None` when the
/// block does not read as a thread.
fn read_thread(text: &str, span: &Span<'_>) -> Option<(Thread, Meta)> {
    let rest = &text[span.body..span.end];
    let (start, end, entries_at) = yaml_block(rest)?;
    let meta = serde_yaml_ng::from_str::<Meta>(&rest[start..end]).ok()?;
    let section = match (&meta.section, &meta.heading) {
        (Some(id), _) => SectionRef::Id(id.clone()),
        (None, Some(path)) => SectionRef::Heading(path.clone()),
        (None, None) => return None,
    };
    let mut entries: Vec<Entry> = Vec::new();
    // Blank lines seen since the last entry line: inside an entry they are
    // paragraph breaks, written as empty lines and read whether or not an
    // editor left the two spaces of indentation on them.
    let mut blanks = 0;
    for line in rest[entries_at..].lines() {
        if line.trim().is_empty() {
            blanks += 1;
            continue;
        }
        if let Some(cont) = line.strip_prefix("  ")
            && let Some(last) = entries.last_mut()
        {
            for _ in 0..=blanks {
                last.text.push('\n');
            }
            last.text.push_str(cont);
        } else if let Some(entry) = parse_entry(line) {
            entries.push(entry);
        }
        blanks = 0;
    }
    let thread = Thread {
        id: span.id.to_string(),
        anchor: Anchor {
            section,
            quote: meta.quote.clone(),
            occurrence: meta.occurrence,
        },
        status: meta.status,
        entries,
    };
    Some((thread, meta))
}

/// Every thread in a threads file, in file order. A block that does not
/// read as a thread is skipped rather than failing the whole file. A CRLF
/// file reads like an LF one.
pub fn parse_threads(text: &str) -> Vec<Thread> {
    let text = crate::to_lf(text);
    thread_spans(&text)
        .iter()
        .filter_map(|span| read_thread(&text, span).map(|(thread, _)| thread))
        .collect()
}

/// `file` with `thread` appended; an empty file gets its `# Threads` title.
pub fn open_thread(file: &str, thread: &Thread) -> String {
    let mut out = if file.trim().is_empty() {
        "# Threads\n\n".to_string()
    } else {
        let mut kept = crate::to_lf(file).trim_end_matches('\n').to_string();
        kept.push_str("\n\n");
        kept
    };
    out.push_str(&render_thread(thread));
    out
}

/// `file` with `entry` appended to thread `id`. A resolve sets the status to
/// resolved, with or without an outcome; a reopen sets it to open; a comment needs
/// the thread open. The only existing byte that changes is the value on the
/// `status:` line of that thread's yaml block (a comment after it is kept).
/// The file comes back as LF.
pub fn append_entry(file: &str, id: &str, entry: &Entry) -> Result<String, ThreadError> {
    let file = crate::to_lf(file).into_owned();
    let spans = thread_spans(&file);
    let (span, (thread, meta)) = spans
        .iter()
        .filter(|span| span.id == id)
        .find_map(|span| read_thread(&file, span).map(|read| (span, read)))
        .ok_or_else(|| ThreadError::NoThread(id.to_string()))?;
    let refused = |why: String| -> Result<String, ThreadError> { Err(ThreadError::Refused(why)) };
    let new_status = match (entry.kind, thread.status) {
        (EntryKind::Comment, ThreadStatus::Open) => None,
        (EntryKind::Comment, ThreadStatus::Resolved) => {
            return refused(format!("thread {id} is resolved; reopen it to answer"));
        }
        (EntryKind::Resolve, ThreadStatus::Open) => Some(ThreadStatus::Resolved),
        (EntryKind::Resolve, ThreadStatus::Resolved) => {
            return refused(format!("thread {id} is already resolved"));
        }
        (EntryKind::Reopen, ThreadStatus::Resolved) => Some(ThreadStatus::Open),
        (EntryKind::Reopen, ThreadStatus::Open) => {
            return refused(format!("thread {id} is open already"));
        }
    };
    let (start, body_start, end) = (span.start, span.body, span.end);
    // The entry goes right after the thread's last line; the blank lines
    // before the next thread stay as they are.
    let content_end = start + file[start..end].trim_end_matches('\n').len();
    let (insert_at, line_break) = if file[content_end..].starts_with('\n') {
        (content_end + 1, "")
    } else {
        (content_end, "\n")
    };
    let mut head = file[..insert_at].to_string();
    if let Some(status) = new_status {
        let (yaml_start, yaml_end, _) = yaml_block(&file[body_start..end])
            .ok_or_else(|| ThreadError::NoThread(id.to_string()))?;
        let (yaml_start, yaml_end) = (body_start + yaml_start, body_start + yaml_end);
        let Some(yaml) = with_status(&file[yaml_start..yaml_end], &meta, status) else {
            return refused(format!(
                "thread {id} has no status line of its own to change"
            ));
        };
        head.replace_range(yaml_start..yaml_end, &yaml);
    }
    Ok(format!(
        "{head}{line_break}{}{}",
        render_entry(entry),
        &file[insert_at..]
    ))
}

/// `yaml` (a thread's block, read as `meta`) with its status set to
/// `status`. The status line is the `status:` line at the start of a line
/// whose rewrite reads back as the same block with only the status changed:
/// a quoted passage may hold the same words, even at the start of a line of
/// a quote that spans lines, and such a line is never taken. A comment after
/// the value stays. `None` when no line qualifies (a flow-style block, say).
fn with_status(yaml: &str, meta: &Meta, status: ThreadStatus) -> Option<String> {
    let lines: Vec<&str> = yaml.split_inclusive('\n').collect();
    let wanted = Meta {
        status,
        ..meta.clone()
    };
    lines.iter().enumerate().find_map(|(i, line)| {
        let value = line.trim_end_matches('\n').strip_prefix("status:")?;
        let comment = value.find(" #").map(|at| &value[at..]).unwrap_or("");
        let newline = if line.ends_with('\n') { "\n" } else { "" };
        let rewritten = format!(
            "{}status: {}{comment}{newline}{}",
            lines[..i].concat(),
            status.as_str(),
            lines[i + 1..].concat()
        );
        let read = serde_yaml_ng::from_str::<Meta>(&rewritten).ok()?;
        (read == wanted).then_some(rewritten)
    })
}

/// `file` with every thread anchored by heading re-anchored on the section
/// id `section_id` answers for that heading, so a later rename of the
/// heading keeps the thread on its section. The only bytes that change are
/// the `heading:` line of each such thread's yaml block, which becomes a
/// `section:` line. A thread whose heading answers no id, or whose block
/// has no `heading:` line of its own to rewrite (a flow-style block, a
/// heading spread over several lines), stays as it is: it keeps resolving
/// by heading. `None` when no thread changed. The file comes back as LF.
pub fn anchor_headings_on_ids(
    file: &str,
    section_id: &dyn Fn(&str) -> Option<String>,
) -> Option<String> {
    let file = crate::to_lf(file).into_owned();
    let mut rewrites: Vec<(usize, usize, String)> = Vec::new();
    for span in thread_spans(&file) {
        let Some((_, meta)) = read_thread(&file, &span) else {
            continue;
        };
        let (None, Some(heading)) = (&meta.section, &meta.heading) else {
            continue;
        };
        let Some(id) = section_id(heading) else {
            continue;
        };
        let Some((start, end, _)) = yaml_block(&file[span.body..span.end]) else {
            continue;
        };
        let (start, end) = (span.body + start, span.body + end);
        if let Some(yaml) = with_section(&file[start..end], &meta, &id) {
            rewrites.push((start, end, yaml));
        }
    }
    if rewrites.is_empty() {
        return None;
    }
    let mut out = file;
    // Back to front, so each range is still where it was found.
    for (start, end, yaml) in rewrites.into_iter().rev() {
        out.replace_range(start..end, &yaml);
    }
    Some(out)
}

/// `yaml` (a thread's block, read as `meta`, anchored by heading) anchored
/// on section `id` instead: its `heading:` line becomes `section: <id>`,
/// taken only where the rewrite reads back as the same block with the
/// anchor alone changed (a quoted passage may hold a line that starts with
/// the same word). `None` when no line qualifies.
fn with_section(yaml: &str, meta: &Meta, id: &str) -> Option<String> {
    let lines: Vec<&str> = yaml.split_inclusive('\n').collect();
    let wanted = Meta {
        section: Some(id.to_string()),
        heading: None,
        ..meta.clone()
    };
    lines.iter().enumerate().find_map(|(i, line)| {
        line.strip_prefix("heading:")?;
        let newline = if line.ends_with('\n') { "\n" } else { "" };
        let rewritten = format!(
            "{}section: {id}{newline}{}",
            lines[..i].concat(),
            lines[i + 1..].concat()
        );
        let read = serde_yaml_ng::from_str::<Meta>(&rewritten).ok()?;
        (read == wanted).then_some(rewritten)
    })
}

/// Where an anchor stands in its document now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorState {
    /// The passage is there; byte offsets within the section's text.
    Found { start: usize, end: usize },
    /// The section is there, the passage is not.
    TextChanged,
    /// The section is gone; the thread shows at the top of the document.
    SectionGone,
}

impl AnchorState {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            AnchorState::Found { .. } => "found",
            AnchorState::TextChanged => "text_changed",
            AnchorState::SectionGone => "section_gone",
        }
    }
}

/// Resolve an anchor. `section_text` is the target's lookup: the text of
/// the section an anchor names, or `None` when it is gone.
pub fn resolve_anchor(
    anchor: &Anchor,
    section_text: &dyn Fn(&SectionRef) -> Option<String>,
) -> AnchorState {
    let Some(text) = section_text(&anchor.section) else {
        return AnchorState::SectionGone;
    };
    let Some(quote) = anchor.quote.as_deref().filter(|q| !q.is_empty()) else {
        return AnchorState::Found {
            start: 0,
            end: text.len(),
        };
    };
    let mut from = 0;
    for n in 1..=anchor.occurrence.max(1) {
        match text[from..].find(quote) {
            Some(at) if n == anchor.occurrence.max(1) => {
                let start = from + at;
                return AnchorState::Found {
                    start,
                    end: start + quote.len(),
                };
            }
            Some(at) => from += at + quote.len(),
            None => break,
        }
    }
    AnchorState::TextChanged
}

/// Who a thread waits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waiting {
    /// An agent spoke last.
    People,
    /// A person spoke last.
    Agent,
}

impl Waiting {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            Waiting::People => "people",
            Waiting::Agent => "agent",
        }
    }
}

/// An open thread waits on whoever did not write its last entry.
pub fn waiting_on(thread: &Thread) -> Waiting {
    match thread.entries.last() {
        Some(entry) if entry.author.agent => Waiting::People,
        _ => Waiting::Agent,
    }
}

/// What an agent is told when it tries to close a person's thread itself.
pub const PERSON_RESOLVES: &str = "answer a person's thread and leave it open: the person who opened it resolves it. Resolve it only after the person wrote back below your answer, for example to say it can be closed";

/// Whether an agent may resolve `thread` now. A thread a person opened
/// stays open until the person resolves it, or until the person wrote again
/// after an agent's answer (for example "thanks, you can close it"). Only
/// the entries since the last reopen count: a reopen asks again. A thread an
/// agent opened is any agent's to resolve. People are never asked.
pub fn agent_may_resolve(thread: &Thread) -> bool {
    let Some(first) = thread.entries.first() else {
        return true;
    };
    if first.author.agent {
        return true;
    }
    let since = thread
        .entries
        .iter()
        .rposition(|e| e.kind == EntryKind::Reopen)
        .map_or(&thread.entries[..], |at| &thread.entries[at..]);
    let Some(answer) = since.iter().rposition(|e| e.author.agent) else {
        return false;
    };
    since[answer + 1..]
        .iter()
        .any(|e| !e.author.agent && e.kind == EntryKind::Comment)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn person(text: &str, kind: EntryKind, at: &str) -> Entry {
        Entry {
            at: at.into(),
            kind,
            author: Author {
                name: "jordi".into(),
                agent: false,
            },
            text: text.into(),
        }
    }

    fn agent(text: &str, kind: EntryKind, at: &str) -> Entry {
        Entry {
            at: at.into(),
            kind,
            author: Author {
                name: "claude-code".into(),
                agent: true,
            },
            text: text.into(),
        }
    }

    fn thread(quote: Option<&str>, occurrence: u32) -> Thread {
        Thread {
            id: "t-3f9a2c1b".into(),
            anchor: Anchor {
                section: SectionRef::Id("7f3a9c2e".into()),
                quote: quote.map(str::to_string),
                occurrence,
            },
            status: ThreadStatus::Open,
            entries: vec![person(
                "Is this still true?\nAfter the migration?",
                EntryKind::Comment,
                "2026-09-30T10:00:00Z",
            )],
        }
    }

    #[test]
    fn a_thread_file_round_trips_and_reads_in_a_file_browser() {
        let file = open_thread("", &thread(Some("Deploys run at night"), 2));
        assert!(
            file.starts_with("# Threads\n\n## t-3f9a2c1b\n\n```yaml\nsection: 7f3a9c2e\n"),
            "{file}"
        );
        assert!(file.contains("- 2026-09-30T10:00:00Z, comment by jordi (person): Is this still true?\n  After the migration?\n"));
        assert_eq!(
            parse_threads(&file),
            vec![thread(Some("Deploys run at night"), 2)]
        );
        let second = Thread {
            id: "t-00000abc".into(),
            ..thread(None, 1)
        };
        let both = open_thread(&file, &second);
        assert_eq!(parse_threads(&both).len(), 2);
        assert!(both.starts_with(&file), "opening a thread only appends");
        // A conventional comment (`<label> [decorations]: <subject>`)
        // keeps its label line whole; the entry header ends at the first "): ".
        let labelled = Thread {
            id: "t-0000c0de".into(),
            entries: vec![person(
                "issue (blocking): the batch window moved\nSee the runbook.",
                EntryKind::Comment,
                "2026-09-30T10:00:00Z",
            )],
            ..thread(None, 1)
        };
        assert_eq!(parse_threads(&open_thread("", &labelled)), vec![labelled]);
    }

    /// Target neutral: an anchor addressed by heading path (what an engram
    /// target will write) round-trips under its own key.
    #[test]
    fn a_heading_anchor_is_written_under_its_own_key() {
        let by_heading = Thread {
            anchor: Anchor {
                section: SectionRef::Heading("## API > ### Auth".into()),
                quote: None,
                occurrence: 1,
            },
            ..thread(None, 1)
        };
        let file = open_thread("", &by_heading);
        assert!(
            file.contains("heading: '## API > ### Auth'")
                || file.contains("heading: \"## API > ### Auth\""),
            "{file}"
        );
        assert!(!file.contains("section:"), "{file}");
        assert_eq!(parse_threads(&file), vec![by_heading]);
        assert_eq!(SectionRef::Id("7f3a9c2e".into()).key(), "7f3a9c2e");
        assert_eq!(SectionRef::Heading("Night".into()).id(), None);
    }

    #[test]
    fn resolving_changes_only_the_status_line_and_reopening_is_a_new_entry() {
        // Not in the form `open_thread` writes (the yaml keys in another
        // order, a blank line before the entries), so an append that
        // re-rendered the thread would change bytes.
        let file = "# Threads\n\n## t-3f9a2c1b\n\n```yaml\nstatus: open\nsection: 7f3a9c2e\nquote: 'status: open'\n```\n\n\n- 2026-09-30T10:00:00Z, comment by jordi (person): Is this still true?\n";
        let resolved = append_entry(
            file,
            "t-3f9a2c1b",
            &agent(
                "Still true; the engram says so.",
                EntryKind::Resolve,
                "2026-09-30T11:00:00Z",
            ),
        )
        .unwrap();
        let without_new_entry = resolved.replace("- 2026-09-30T11:00:00Z, resolve by claude-code (agent): Still true; the engram says so.\n", "");
        assert_eq!(
            without_new_entry.replacen("status: resolved", "status: open", 1),
            file,
            "the history is never rewritten: one appended line and the status"
        );
        assert!(
            resolved.contains("quote: 'status: open'"),
            "a quoted passage is never the status line: {resolved}"
        );
        assert_eq!(parse_threads(&resolved)[0].status, ThreadStatus::Resolved);

        let err = append_entry(
            &resolved,
            "t-3f9a2c1b",
            &person("more", EntryKind::Comment, "2026-09-30T12:00:00Z"),
        )
        .unwrap_err();
        assert!(
            matches!(err, ThreadError::Refused(_)) && err.to_string().contains("reopen"),
            "{err}"
        );
        assert!(
            append_entry(
                &resolved,
                "t-3f9a2c1b",
                &person("x", EntryKind::Resolve, "t")
            )
            .is_err()
        );

        let reopened = append_entry(
            &resolved,
            "t-3f9a2c1b",
            &person("", EntryKind::Reopen, "2026-09-30T12:00:00Z"),
        )
        .unwrap();
        assert!(reopened.contains("status: open\nsection: 7f3a9c2e"));
        assert!(reopened.contains("- 2026-09-30T11:00:00Z, resolve by claude-code (agent): Still true; the engram says so.\n- 2026-09-30T12:00:00Z, reopen by jordi (person)\n"));
        let t = &parse_threads(&reopened)[0];
        assert_eq!(t.status, ThreadStatus::Open);
        assert_eq!(t.entries.len(), 3, "the reopen is visible in the history");
        assert_eq!(t.entries[2].kind, EntryKind::Reopen);
        assert!(
            append_entry(
                &reopened,
                "t-3f9a2c1b",
                &person("x", EntryKind::Reopen, "t")
            )
            .is_err()
        );
        assert_eq!(
            append_entry(
                &reopened,
                "t-nothere",
                &person("x", EntryKind::Comment, "t")
            ),
            Err(ThreadError::NoThread("t-nothere".into())),
            "an unknown thread is its own error, so the engine answers it as not found"
        );
        let canonical = open_thread("", &thread(None, 1));
        let resolved = append_entry(
            &canonical,
            "t-3f9a2c1b",
            &agent("", EntryKind::Resolve, "t"),
        )
        .expect("an outcome is optional");
        let read = &parse_threads(&resolved)[0];
        assert_eq!(read.status, ThreadStatus::Resolved);
        let last = read.entries.last().unwrap();
        assert_eq!((last.kind, last.text.as_str()), (EntryKind::Resolve, ""));
    }

    /// A resolve with no outcome is one line with no colon after the author,
    /// and it reads back as a resolve with empty text; so does one whose
    /// outcome is only spaces.
    #[test]
    fn a_resolve_without_an_outcome_round_trips() {
        for text in ["", "   "] {
            let entry = person(text, EntryKind::Resolve, "2026-09-30T11:00:00Z");
            let line = render_entry(&entry);
            assert_eq!(
                line, "- 2026-09-30T11:00:00Z, resolve by jordi (person)\n",
                "{text:?}"
            );
            let read = parse_entry(line.trim_end_matches('\n')).unwrap();
            assert_eq!(read.kind, EntryKind::Resolve);
            assert_eq!(read.text, "");
            assert_eq!(read.author, entry.author);
        }
    }

    /// A quoted passage ahead of the status line, and blank lines between
    /// two threads: the status line is the yaml key, and every other byte of
    /// the file, the blank lines included, stays where it was.
    #[test]
    fn the_status_key_changes_wherever_it_stands_and_the_spacing_is_kept() {
        let file = "# Threads\n\n## t-3f9a2c1b\n\n```yaml\nsection: 7f3a9c2e\nquote: |-\n  status: open\n  is what it says\nstatus: open\n```\n\n- 2026-09-30T10:00:00Z, comment by jordi (person): Is this still true?\n\n\n\n## t-00000abc\n\n```yaml\nsection: 0b1c2d3e\nstatus: open\n```\n\n- 2026-09-30T10:00:00Z, comment by jordi (person): Other.\n";
        let entry = "- 2026-09-30T11:00:00Z, resolve by claude-code (agent): Done.\n";
        let resolved = append_entry(
            file,
            "t-3f9a2c1b",
            &agent("Done.", EntryKind::Resolve, "2026-09-30T11:00:00Z"),
        )
        .unwrap();
        assert_eq!(
            resolved,
            file.replacen("status: open\n```", "status: resolved\n```", 1)
                .replacen("true?\n", &format!("true?\n{entry}"), 1)
        );
        let threads = parse_threads(&resolved);
        assert_eq!(
            threads[0].anchor.quote.as_deref(),
            Some("status: open\nis what it says")
        );
        assert_eq!(
            (threads[0].status, threads[1].status),
            (ThreadStatus::Resolved, ThreadStatus::Open)
        );
        let reopened =
            append_entry(&resolved, "t-3f9a2c1b", &person("", EntryKind::Reopen, "t")).unwrap();
        assert!(
            reopened.contains("quote: |-\n  status: open\n  is what it says\nstatus: open\n```"),
            "{reopened}"
        );
        let at_the_end = append_entry(
            file.trim_end_matches('\n'),
            "t-00000abc",
            &agent("Yes.", EntryKind::Comment, "t"),
        )
        .unwrap();
        assert!(
            at_the_end.ends_with("Other.\n- t, comment by claude-code (agent): Yes.\n"),
            "{at_the_end}"
        );
    }

    /// A hand-written quote in quotes may span lines and hold a `status:`
    /// line at the start of a line; that line is part of the quote, never
    /// the status.
    #[test]
    fn a_status_line_inside_a_quote_that_spans_lines_is_never_the_status() {
        for (quote, read) in [
            ("'run\nstatus: open\nhere'", "run status: open here"),
            ("\"run\nstatus: open\nhere\"", "run status: open here"),
        ] {
            let file = format!(
                "# Threads\n\n## t-3f9a2c1b\n\n```yaml\nsection: 7f3a9c2e\nquote: {quote}\nstatus: open\n```\n\n- 2026-09-30T10:00:00Z, comment by jordi (person): Still true?\n"
            );
            assert_eq!(parse_threads(&file)[0].anchor.quote.as_deref(), Some(read));
            let resolved =
                append_entry(&file, "t-3f9a2c1b", &agent("Yes.", EntryKind::Resolve, "t")).unwrap();
            assert!(
                resolved.contains(&format!("quote: {quote}\nstatus: resolved\n```")),
                "{resolved}"
            );
            let t = &parse_threads(&resolved)[0];
            assert_eq!(
                (t.status, t.anchor.quote.as_deref()),
                (ThreadStatus::Resolved, Some(read))
            );
            assert!(
                append_entry(
                    &resolved,
                    "t-3f9a2c1b",
                    &agent("Again.", EntryKind::Resolve, "t")
                )
                .is_err()
            );
        }
        let flow = "# Threads\n\n## t-3f9a2c1b\n\n```yaml\n{section: 7f3a9c2e, status: open}\n```\n\n- t, comment by jordi (person): x\n";
        assert_eq!(parse_threads(flow).len(), 1);
        assert!(
            matches!(
                append_entry(flow, "t-3f9a2c1b", &agent("Yes.", EntryKind::Resolve, "t")),
                Err(ThreadError::Refused(_))
            ),
            "a block whose status line cannot be found is refused, not half written"
        );
        let noted = "# Threads\n\n## t-3f9a2c1b\n\n```yaml\nsection: 7f3a9c2e\nstatus: open # asked in the standup\n```\n\n- t, comment by jordi (person): x\n";
        let resolved =
            append_entry(noted, "t-3f9a2c1b", &agent("Yes.", EntryKind::Resolve, "t")).unwrap();
        assert!(
            resolved.contains("\nstatus: resolved # asked in the standup\n"),
            "a comment on the status line stays: {resolved}"
        );
    }

    /// Hand edits the reader takes are taken by the appender too: a `##`
    /// line with trailing spaces, fence lines with spaces, a file without
    /// its `# Threads` title.
    #[test]
    fn what_the_reader_lists_the_appender_finds() {
        let canonical = open_thread("", &thread(None, 1));
        for file in [
            canonical.replace("## t-3f9a2c1b\n", "## t-3f9a2c1b  \n"),
            canonical.replace("```yaml\n", "```yaml  \n"),
            canonical.replace("```yaml\n", "``` yaml\n"),
            canonical.replace("```\n\n-", "```  \n\n-"),
            canonical.replace("# Threads\n\n", ""),
        ] {
            assert_eq!(parse_threads(&file), vec![thread(None, 1)], "{file}");
            let answered =
                append_entry(&file, "t-3f9a2c1b", &agent("Yes.", EntryKind::Comment, "t")).unwrap();
            assert_eq!(
                answered,
                format!("{file}- t, comment by claude-code (agent): Yes.\n")
            );
        }
    }

    /// A paragraph break inside an entry is an empty line, which an editor
    /// that strips trailing spaces leaves alone; two spaces on it read too.
    #[test]
    fn a_paragraph_break_in_an_entry_survives_an_editor() {
        let broken = Thread {
            entries: vec![person("First.\n\nSecond.", EntryKind::Comment, "t")],
            ..thread(None, 1)
        };
        let file = open_thread("", &broken);
        assert!(file.contains("(person): First.\n\n  Second.\n"), "{file}");
        assert_eq!(parse_threads(&file), vec![broken.clone()]);
        assert_eq!(
            parse_threads(&file.replace("First.\n\n", "First.\n  \n")),
            vec![broken]
        );
    }

    #[test]
    fn a_quote_with_a_fence_and_a_crlf_file_still_read() {
        let fenced = thread(Some("run ```make deploy``` at night"), 1);
        let file = open_thread("", &fenced);
        assert_eq!(
            parse_threads(&file),
            vec![fenced],
            "the block ends at its closing fence line: {file}"
        );
        let crlf = open_thread("", &thread(None, 1)).replace('\n', "\r\n");
        assert_eq!(
            parse_threads(&crlf).len(),
            1,
            "a CRLF file reads like an LF one"
        );
        assert!(append_entry(&crlf, "t-3f9a2c1b", &agent("Yes.", EntryKind::Comment, "t")).is_ok());
    }

    #[test]
    fn an_anchor_resolves_against_the_section_its_target_hands_it() {
        let night =
            "## Night deploys\n\nDeploys run at night, after the batch.\nDeploys run at night.\n"
                .to_string();
        let lookup = |section: &SectionRef| (section.key() == "7f3a9c2e").then(|| night.clone());
        let anchor = thread(Some("Deploys run at night"), 2).anchor;
        let AnchorState::Found { start, end } = resolve_anchor(&anchor, &lookup) else {
            panic!()
        };
        assert_eq!(&night[start..end], "Deploys run at night");
        assert!(
            start > night.find("Deploys run at night").unwrap(),
            "the second occurrence"
        );
        let rewritten = |_: &SectionRef| {
            Some("## Night deploys\n\nWe deploy in the morning now.\n".to_string())
        };
        assert_eq!(
            resolve_anchor(&anchor, &rewritten),
            AnchorState::TextChanged
        );
        assert_eq!(resolve_anchor(&anchor, &|_| None), AnchorState::SectionGone);
        let whole = Anchor {
            section: SectionRef::Id("7f3a9c2e".into()),
            quote: None,
            occurrence: 1,
        };
        assert!(matches!(
            resolve_anchor(&whole, &lookup),
            AnchorState::Found { .. }
        ));
    }

    /// A thread anchored by heading moves onto the section's id once the
    /// section has one: only its `heading:` line changes. A heading that
    /// answers no id, and a block whose heading spans lines, stay as they
    /// are, so no thread is lost.
    #[test]
    fn a_heading_anchor_moves_onto_the_section_id_and_nothing_else_changes() {
        let by_heading = |id: &str, heading: &str| Thread {
            id: id.into(),
            anchor: Anchor {
                section: SectionRef::Heading(heading.into()),
                quote: Some("Deploys run at night".into()),
                occurrence: 1,
            },
            ..thread(None, 1)
        };
        let mut file = open_thread("", &by_heading("t-0000aaaa", "Night deploys"));
        file = open_thread(&file, &thread(None, 1));
        file = open_thread(&file, &by_heading("t-0000bbbb", "Gone"));
        file.push_str("\n## t-0000cccc\n\n```yaml\nheading: >-\n  Night\n  deploys\nstatus: open\n```\n\n- 2026-09-30T10:00:00Z, comment by jordi (person): Hi\n");
        let ids = |heading: &str| (heading == "Night deploys").then(|| "7f3a9c2e".to_string());
        let moved = anchor_headings_on_ids(&file, &ids).expect("one thread moved");
        let threads = parse_threads(&moved);
        assert_eq!(threads.len(), 4, "{moved}");
        assert_eq!(
            threads[0].anchor,
            Anchor {
                section: SectionRef::Id("7f3a9c2e".into()),
                quote: Some("Deploys run at night".into()),
                occurrence: 1,
            }
        );
        assert_eq!(threads[0].entries, by_heading("t-0000aaaa", "x").entries);
        assert_eq!(threads[1], thread(None, 1), "an id anchor stays");
        assert_eq!(
            threads[2],
            by_heading("t-0000bbbb", "Gone"),
            "no id, no move"
        );
        assert_eq!(
            threads[3].anchor.section,
            SectionRef::Heading("Night deploys".into()),
            "a heading over two lines is left by heading"
        );
        assert_eq!(
            moved.replacen("section: 7f3a9c2e\n", "heading: Night deploys\n", 1),
            file,
            "only the heading line changed"
        );
        assert_eq!(
            anchor_headings_on_ids(&moved, &ids),
            None,
            "a second pass changes nothing"
        );
    }

    #[test]
    fn a_thread_waits_on_whoever_did_not_speak_last() {
        let mut t = thread(None, 1);
        assert_eq!(waiting_on(&t), Waiting::Agent, "a person asked");
        t.entries.push(agent(
            "Answered.",
            EntryKind::Comment,
            "2026-09-30T11:00:00Z",
        ));
        assert_eq!(waiting_on(&t), Waiting::People);
        assert_eq!(t.opened_at(), "2026-09-30T10:00:00Z");
        assert_eq!(t.last_at(), "2026-09-30T11:00:00Z");
    }

    #[test]
    fn an_agent_leaves_a_persons_thread_to_the_person() {
        let at = "2026-09-30T11:00:00Z";
        let mut t = thread(None, 1);
        assert!(
            !agent_may_resolve(&t),
            "answering and resolving in one call is refused"
        );
        t.entries.push(agent("Yes.", EntryKind::Comment, at));
        assert!(!agent_may_resolve(&t), "the agent's answer waits");
        t.entries
            .push(person("Thanks, close it.", EntryKind::Comment, at));
        assert!(agent_may_resolve(&t), "the person wrote back");
        t.entries.push(person("Done.", EntryKind::Resolve, at));
        t.entries.push(person("It changed.", EntryKind::Reopen, at));
        assert!(!agent_may_resolve(&t), "a reopen asks again");
        t.entries.push(agent("Fixed.", EntryKind::Comment, at));
        assert!(!agent_may_resolve(&t));
        t.entries.push(person("Good.", EntryKind::Comment, at));
        assert!(agent_may_resolve(&t));

        let mut mine = thread(None, 1);
        mine.entries = vec![agent("Is this right?", EntryKind::Comment, at)];
        assert!(agent_may_resolve(&mine), "an agent's own thread");
        mine.entries.push(person("Yes.", EntryKind::Comment, at));
        assert!(agent_may_resolve(&mine));
    }

    #[test]
    fn names_cannot_break_the_entry_line() {
        assert_eq!(sanitize_name("Ada (ops): lead\nx"), "Ada ops lead x");
        assert_eq!(sanitize_name("   "), "someone");
        let id = random_thread_id();
        assert!(
            id.starts_with("t-") && id.len() == 10 && id.as_bytes()[2].is_ascii_alphabetic(),
            "{id}"
        );
    }
}
