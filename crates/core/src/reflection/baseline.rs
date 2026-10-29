//! The baseline: the last reflected text of every section, kept beside the
//! tree under `.reflected/`, and the feedback changes taken against it. The
//! changes are computed on demand and never stored.

use std::collections::{BTreeMap, BTreeSet};

use crate::reflection::provenance::{read_provenance, write_provenance};
use crate::reflection::sections::{
    TOP_SECTION, id_line, parse_id_line, section_hash, split_sections,
};

/// The marker that opens the top block of a baseline file.
fn top_marker() -> String {
    id_line(TOP_SECTION)
}

/// The section an entry line opens: a `##` heading directly followed by an
/// id line, which is how [`render_baseline`] writes every section.
fn entry_start(line: &str, next: Option<&&str>) -> Option<String> {
    let (level, _) = crate::parse::parse_heading(line)?;
    let id = parse_id_line(next?)?;
    (level == 2).then(|| id.to_string())
}

/// The sections of a baseline file, in file order.
///
/// Entries are cut where an id line stands under a `##` heading, not by the
/// heading walker: a baseline joins sections reflected at different times,
/// so one entry may end inside a fence it never closed, and that must not
/// hide the entries after it. A `##` heading with no id line, fenced or not,
/// stays part of its entry.
pub fn parse_baseline(text: &str) -> Vec<(String, String)> {
    if text.trim().is_empty() {
        return Vec::new();
    }
    let lines: Vec<&str> = text.split('\n').map(|l| l.trim_end_matches('\r')).collect();
    let mut out = Vec::new();
    let mut current: Option<(String, Vec<&str>)> = None;
    let mut k = 0;
    if lines[0] == top_marker() {
        current = Some((TOP_SECTION.to_string(), Vec::new()));
        k = 1;
    }
    while k < lines.len() {
        let line = lines[k];
        if let Some(id) = entry_start(line, lines.get(k + 1)) {
            out.extend(current.take().map(entry_text));
            current = Some((id, vec![line]));
            k += 2;
            continue;
        }
        if let Some((_, body)) = current.as_mut() {
            body.push(line);
        }
        k += 1;
    }
    out.extend(current.map(entry_text));
    out
}

/// One parsed entry: its lines without the blank lines that separate it from
/// the next one.
fn entry_text((id, mut lines): (String, Vec<&str>)) -> (String, String) {
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    let mut text = lines.join("\n");
    if !lines.is_empty() {
        text.push('\n');
    }
    (id, text)
}

/// A baseline file for these sections: the top block first when present,
/// then each section as heading, id line, body, one blank line between. The
/// top block goes first wherever `entries` hold it, because only a first
/// line opens it when the file is read back.
pub fn render_baseline(entries: &[(String, String)]) -> String {
    let mut out = String::new();
    let top = entries.iter().filter(|(id, _)| id == TOP_SECTION);
    let rest = entries.iter().filter(|(id, _)| id != TOP_SECTION);
    for (id, text) in top.chain(rest) {
        let text = text.trim_end_matches('\n');
        if id == TOP_SECTION {
            out.push_str(&top_marker());
            out.push('\n');
            out.push_str(text);
            out.push_str("\n\n");
            continue;
        }
        let (heading, body) = text.split_once('\n').unwrap_or((text, ""));
        out.push_str(heading);
        out.push('\n');
        out.push_str(&id_line(id));
        out.push('\n');
        out.push_str(body);
        out.push_str("\n\n");
    }
    out
}

/// Every id a baseline holds.
pub fn baseline_ids(text: &str) -> BTreeSet<String> {
    parse_baseline(text).into_iter().map(|(id, _)| id).collect()
}

/// What happened to one section since it was last reflected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    /// Not in the baseline: a new section, or one with no id yet.
    New,
    /// In both, with different text.
    Changed,
    /// In the baseline, gone from the page.
    Removed,
}

impl ChangeKind {
    /// The wire word.
    pub fn as_str(self) -> &'static str {
        match self {
            ChangeKind::New => "new",
            ChangeKind::Changed => "changed",
            ChangeKind::Removed => "removed",
        }
    }
}

/// One feedback change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionChange {
    /// The section id; `None` for a heading with no id yet.
    pub id: Option<String>,
    /// The heading (the baseline's for a removed section).
    pub heading: String,
    /// What happened.
    pub kind: ChangeKind,
    /// The reflected text; empty for a new section.
    pub before: String,
    /// The current text; empty for a removed section.
    pub after: String,
}

/// The heading text of a section, read the way every heading is read.
fn heading_of(text: &str) -> String {
    text.lines()
        .next()
        .and_then(crate::parse::parse_heading)
        .filter(|(level, _)| *level == 2)
        .map(|(_, heading)| heading)
        .unwrap_or_default()
}

/// Whether two section texts differ only in task boxes: `[ ]`, `[x]`, `[X]`
/// and `[~]` at the head of a list item count as one mark. Ticking a box on
/// the reading page is not something the agent has to weigh. An item counts
/// under a bullet (`-`, `*`, `+`) or an ordered marker (`1.`, `2)`), also
/// inside a quote or callout (`> - [ ]`, `> > 1. [ ]`); a box inside fenced
/// code is text like any other, so changing it is a change.
pub fn same_but_task_boxes(before: &str, after: &str) -> bool {
    /// The line with its task box read as `[ ]`, or the line as it is.
    fn unticked(line: &str) -> Option<String> {
        let quoted = line.len() - line.trim_start_matches([' ', '\t', '>']).len();
        let (quote, rest) = line.split_at(quoted);
        let marker_len = if rest.starts_with(['-', '*', '+']) {
            1
        } else {
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let closes = rest[digits..].starts_with(['.', ')']);
            if (1..=9).contains(&digits) && closes {
                digits + 1
            } else {
                return None;
            }
        };
        let (marker, item) = rest.split_at(marker_len);
        let item = item.strip_prefix(' ')?;
        let tail = ["[ ]", "[x]", "[X]", "[~]"]
            .iter()
            .find_map(|b| item.strip_prefix(b))?;
        Some(format!("{quote}{marker} [ ]{tail}"))
    }
    fn norm(text: &str) -> String {
        let normalized = crate::reflection::normalize_section_text(text);
        let mut fence = crate::parse::FenceTracker::new();
        normalized
            .lines()
            .map(|line| {
                let content = line.trim_start_matches([' ', '\t', '>']);
                if fence.in_fence(content) {
                    return line.to_string();
                }
                unticked(line).unwrap_or_else(|| line.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    norm(before) == norm(after)
}

/// The feedback changes of `current` against `baseline`, in page order, the
/// removed sections last in baseline order. A top that is empty and was
/// never reflected is no change.
pub fn feedback_changes(current: &str, baseline: &str) -> Vec<SectionChange> {
    let reflected = parse_baseline(baseline);
    let lookup = |id: &str| {
        reflected
            .iter()
            .find(|(k, _)| k == id)
            .map(|(_, t)| t.as_str())
    };
    let sections = split_sections(current);
    let mut changes = Vec::new();
    for section in &sections {
        let before = section.id.as_deref().and_then(lookup);
        let kind = match (section.id.as_deref(), before) {
            (Some(TOP_SECTION), None) if section.text.trim().is_empty() => continue,
            (_, None) => ChangeKind::New,
            (_, Some(before))
                if section_hash(before) != section_hash(&section.text)
                    && !same_but_task_boxes(before, &section.text) =>
            {
                ChangeKind::Changed
            }
            _ => continue,
        };
        changes.push(SectionChange {
            id: section.id.clone(),
            heading: section.heading.clone(),
            kind,
            before: before.unwrap_or("").to_string(),
            after: section.text.clone(),
        });
    }
    for (id, text) in &reflected {
        if sections
            .iter()
            .any(|s| s.id.as_deref() == Some(id.as_str()))
        {
            continue;
        }
        changes.push(SectionChange {
            id: Some(id.clone()),
            heading: heading_of(text),
            kind: ChangeKind::Removed,
            before: text.clone(),
            after: String::new(),
        });
    }
    changes
}

/// `baseline` with the named sections advanced to their current text; a
/// named id no longer in the page leaves the baseline. Sections come out in
/// page order, then any other baseline sections in their own order.
pub fn advance_baseline(baseline: &str, current: &str, ids: &[String]) -> String {
    let mut reflected = parse_baseline(baseline);
    let sections = split_sections(current);
    for id in ids {
        let now = sections
            .iter()
            .find(|s| s.id.as_deref() == Some(id.as_str()));
        reflected.retain(|(k, _)| k != id);
        if let Some(section) = now {
            reflected.push((id.clone(), section.text.clone()));
        }
    }
    let position = |id: &str| sections.iter().position(|s| s.id.as_deref() == Some(id));
    // A stable sort, so sections the page no longer holds keep their order
    // at the end.
    reflected.sort_by_key(|(id, _)| position(id).unwrap_or(usize::MAX));
    render_baseline(&reflected)
}

/// The baseline a page stands for when its baseline file is missing (a
/// Reflection restored from its backup branch, which does not carry
/// `.reflected/`): every section whose text is still what was last reflected
/// by the `reflected` hash its record keeps. A section never reflected, or
/// changed since, is left out, so it stays a person's change. Empty when no
/// section qualifies.
pub fn seed_baseline(page: &str) -> String {
    let prov = read_provenance(page);
    let entries: Vec<(String, String)> = split_sections(page)
        .into_iter()
        .filter_map(|section| {
            let id = section.id?;
            let reflected = prov.get(&id)?.reflected.as_deref()?;
            (reflected == section_hash(&section.text)).then_some((id, section.text))
        })
        .collect();
    if entries.is_empty() {
        return String::new();
    }
    render_baseline(&entries)
}

/// A reflection and its baseline after a link rewrite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relinked {
    /// The page, provenance included.
    pub page: String,
    /// The baseline, rewritten the same way.
    pub baseline: String,
    /// How many links of the page changed.
    pub links: usize,
}

/// Apply a link rewrite that is not a person's change: a reflection or a
/// file moved and the links follow it. The baseline gets the same rewrite,
/// so the feedback diff stays empty, and every section whose text matched
/// its `reflected` hash before carries the hash of its new text. A section a
/// person had already changed keeps its old hash and stays a change. The
/// baseline is rewritten even when the page holds no such link (a removed
/// section may), and the page is left byte for byte when none of its links
/// changed.
pub fn relink(
    page: &str,
    baseline: &str,
    rewrite: &dyn Fn(&str) -> (String, usize),
) -> Result<Relinked, String> {
    let (new_baseline, _) = rewrite(baseline);
    let (rewritten, links) = rewrite(page);
    if links == 0 {
        return Ok(Relinked {
            page: page.to_string(),
            baseline: new_baseline,
            links: 0,
        });
    }
    let before: BTreeMap<String, String> = split_sections(page)
        .into_iter()
        .filter_map(|s| Some((s.id?, section_hash(&s.text))))
        .collect();
    let mut provenance = read_provenance(&rewritten);
    let mut carried = false;
    for section in split_sections(&rewritten) {
        let Some(id) = section.id else { continue };
        let Some(record) = provenance.get_mut(&id) else {
            continue;
        };
        let now = section_hash(&section.text);
        if record.reflected.is_some()
            && record.reflected.as_deref() == before.get(&id).map(String::as_str)
            && record.reflected.as_deref() != Some(now.as_str())
        {
            record.reflected = Some(now);
            carried = true;
        }
    }
    let page = if carried {
        write_provenance(&rewritten, &provenance)?
    } else {
        rewritten
    };
    Ok(Relinked {
        page,
        baseline: new_baseline,
        links,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const CURRENT: &str = "---\ntitle: T\n---\n\n# T\n\nIntro.\n\n## Kept\n<!-- r:aaaa1111 -->\n\nSame text.\n\n## Edited\n<!-- r:bbbb2222 -->\n\nNew wording.\n\n## Added\n<!-- r:cccc3333 -->\n\nA person added this.\n\n## Typed by hand\n\nNo id yet.\n";

    fn baseline() -> String {
        render_baseline(&[
            ("_top".into(), "\n# T\n\nIntro.\n".into()),
            ("aaaa1111".into(), "## Kept\n\nSame text.\n".into()),
            ("bbbb2222".into(), "## Edited\n\nOld wording.\n".into()),
            ("dddd4444".into(), "## Gone\n\nRemoved later.\n".into()),
        ])
    }

    /// A top entry handed over after the sections still opens the file, so
    /// it reads back as the top and not as part of the last section.
    #[test]
    fn the_top_block_is_written_first_wherever_it_is_handed_over() {
        let text = render_baseline(&[
            ("aaaa1111".into(), "## Kept\n\nSame text.\n".into()),
            ("_top".into(), "# T\n\nIntro.\n".into()),
        ]);
        assert!(text.starts_with("<!-- r:_top -->\n# T\n"), "{text}");
        let ids: Vec<String> = parse_baseline(&text)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["_top", "aaaa1111"]);
    }

    #[test]
    fn the_baseline_round_trips_with_its_top_block() {
        let text = baseline();
        assert!(text.starts_with("<!-- r:_top -->\n"));
        assert!(text.contains("## Kept\n<!-- r:aaaa1111 -->\n"));
        let parsed = parse_baseline(&text);
        let ids: Vec<&str> = parsed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["_top", "aaaa1111", "bbbb2222", "dddd4444"]);
        assert_eq!(baseline_ids(&text).len(), 4);
        assert!(parse_baseline("").is_empty());
    }

    #[test]
    fn a_fenced_heading_in_the_top_block_does_not_end_it() {
        let text = render_baseline(&[
            (
                "_top".into(),
                "\n# T\n\n```\n## not a section\n```\n".into(),
            ),
            ("aaaa1111".into(), "## Kept\n\nSame text.\n".into()),
        ]);
        let parsed = parse_baseline(&text);
        let ids: Vec<&str> = parsed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["_top", "aaaa1111"], "{parsed:?}");
        assert!(parsed[0].1.contains("## not a section"), "{parsed:?}");
    }

    #[test]
    fn feedback_changes_are_new_changed_and_removed() {
        let changes = feedback_changes(CURRENT, &baseline());
        let summary: Vec<(Option<&str>, &str, &str)> = changes
            .iter()
            .map(|c| (c.id.as_deref(), c.heading.as_str(), c.kind.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (Some("bbbb2222"), "Edited", "changed"),
                (Some("cccc3333"), "Added", "new"),
                (None, "Typed by hand", "new"),
                (Some("dddd4444"), "Gone", "removed"),
            ]
        );
        let edited = &changes[0];
        assert!(edited.before.contains("Old wording.") && edited.after.contains("New wording."));
        assert!(changes[3].after.is_empty() && changes[3].before.contains("Removed later."));
    }

    #[test]
    fn ticking_task_boxes_is_not_a_change() {
        let baseline_text = "## Steps\n<!-- r:aaaa0001 -->\n\n- [ ] stop the job\n- [ ] restore\n";
        let ticked = "## Steps\n<!-- r:aaaa0001 -->\n\n- [x] stop the job\n- [~] restore\n";
        let edited = "## Steps\n<!-- r:aaaa0001 -->\n\n- [x] stop the job now\n- [ ] restore\n";
        let base = advance_baseline("", baseline_text, &["aaaa0001".to_string()]);
        assert!(feedback_changes(ticked, &base).is_empty());
        assert_eq!(feedback_changes(edited, &base).len(), 1);
    }

    #[test]
    fn ticking_a_box_inside_a_quote_or_callout_is_not_a_change() {
        let before = "## Steps\n<!-- r:aaaa0001 -->\n\n> [!IMPORTANT]\n> - [ ] export the tenant\n> > - [ ] nested\n";
        let after = "## Steps\n<!-- r:aaaa0001 -->\n\n> [!IMPORTANT]\n> - [x] export the tenant\n> > - [~] nested\n";
        assert!(same_but_task_boxes(before, after));
        let tight = "## Steps\n<!-- r:aaaa0001 -->\n\n> [!IMPORTANT]\n>- [X] export the tenant\n>> - [~] nested\n";
        assert!(
            !same_but_task_boxes(before, tight),
            "the quote markers stay part of the text"
        );
        let reworded = after.replace("export the tenant", "export every tenant");
        assert!(!same_but_task_boxes(before, &reworded));
    }

    #[test]
    fn ticking_a_box_in_an_ordered_list_is_not_a_change() {
        let before = "## Steps\n<!-- r:aaaa0001 -->\n\n1. [ ] stop the service\n2) [ ] restore\n10. [ ] start\n";
        let after = "## Steps\n<!-- r:aaaa0001 -->\n\n1. [x] stop the service\n2) [X] restore\n10. [~] start\n";
        assert!(same_but_task_boxes(before, after));
        let renumbered = after.replace("2) [X]", "3) [X]");
        assert!(!same_but_task_boxes(before, &renumbered));
    }

    #[test]
    fn a_box_changed_inside_fenced_code_is_a_change() {
        let before = "## Example\n<!-- r:aaaa0001 -->\n\n```markdown\n- [ ] a task in a sample\n```\n- [ ] a real task\n";
        let fenced = "## Example\n<!-- r:aaaa0001 -->\n\n```markdown\n- [x] a task in a sample\n```\n- [ ] a real task\n";
        let ticked = "## Example\n<!-- r:aaaa0001 -->\n\n```markdown\n- [ ] a task in a sample\n```\n- [x] a real task\n";
        assert!(!same_but_task_boxes(before, fenced));
        assert!(same_but_task_boxes(before, ticked));
    }

    #[test]
    fn a_page_never_reflected_is_all_new_and_an_empty_top_is_nothing() {
        let changes = feedback_changes("## Only\n<!-- r:aaaa1111 -->\n\nText.\n", "");
        assert_eq!(
            changes.len(),
            1,
            "the empty top is not a change: {changes:?}"
        );
        assert_eq!(changes[0].kind, ChangeKind::New);
    }

    #[test]
    fn advancing_copies_the_named_sections_and_drops_removed_ones() {
        let advanced = advance_baseline(
            &baseline(),
            CURRENT,
            &["bbbb2222".into(), "cccc3333".into(), "dddd4444".into()],
        );
        let left = feedback_changes(CURRENT, &advanced);
        let kinds: Vec<(Option<&str>, &str)> = left
            .iter()
            .map(|c| (c.id.as_deref(), c.kind.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [(None, "new")],
            "only the heading without an id is left"
        );
        let ids: Vec<String> = parse_baseline(&advanced)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(
            ids,
            ["_top", "aaaa1111", "bbbb2222", "cccc3333"],
            "page order"
        );
    }

    #[test]
    fn a_link_rewrite_owes_no_feedback_and_carries_the_reflected_hash() {
        use crate::reflection::{Provenance, SectionRecord, read_provenance, write_provenance};
        let text = "# Deploying\n\n## Night\n<!-- r:7f3a9c2e -->\n\nSee [rollback](rollback.md).\n\n## Other\n<!-- r:0b1c2d3e -->\n\nSee [rollback](rollback.md), changed by a person.\n";
        let sections = split_sections(text);
        let mut prov = Provenance::new();
        prov.insert(
            "7f3a9c2e".into(),
            SectionRecord {
                reflected: Some(section_hash(&sections[1].text)),
                ..Default::default()
            },
        );
        prov.insert(
            "0b1c2d3e".into(),
            SectionRecord {
                reflected: Some("an older hash".into()),
                ..Default::default()
            },
        );
        let page = write_provenance(text, &prov).unwrap();
        let baseline = advance_baseline("", &page, &["7f3a9c2e".to_string()]);
        let rewrite = |s: &str| {
            (
                s.replace("](rollback.md)", "](../guides/rollback.md)"),
                s.matches("](rollback.md)").count(),
            )
        };
        let out = relink(&page, &baseline, &rewrite).unwrap();
        assert_eq!(out.links, 2);
        assert!(
            out.baseline.contains("](../guides/rollback.md)"),
            "the baseline follows"
        );
        let after = read_provenance(&out.page);
        let night = split_sections(&out.page)
            .into_iter()
            .find(|s| s.id.as_deref() == Some("7f3a9c2e"))
            .unwrap();
        assert_eq!(
            after["7f3a9c2e"].reflected.as_deref(),
            Some(section_hash(&night.text).as_str())
        );
        assert_eq!(
            after["0b1c2d3e"].reflected.as_deref(),
            Some("an older hash"),
            "a person's change stays a change"
        );
        assert!(
            feedback_changes(&out.page, &out.baseline)
                .iter()
                .all(|c| c.id.as_deref() != Some("7f3a9c2e"))
        );
        let untouched = relink(&page, &baseline, &|s: &str| (s.to_string(), 0)).unwrap();
        assert_eq!(
            (untouched.page.as_str(), untouched.links),
            (page.as_str(), 0),
            "no link, no byte changed"
        );
    }
    #[test]
    fn an_unclosed_fence_in_one_entry_does_not_hide_the_entries_after_it() {
        let text = render_baseline(&[
            ("_top".into(), "\n# T\n\n```\nstill open\n".into()),
            ("aaaa1111".into(), "## Code\n\n```\nfoo\n".into()),
            ("bbbb2222".into(), "## New\n\nText.\n".into()),
            ("cccc3333".into(), "## Last\n\nMore.\n".into()),
        ]);
        let ids: Vec<String> = parse_baseline(&text)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, ["_top", "aaaa1111", "bbbb2222", "cccc3333"], "{text}");
        let page = "## Code\n<!-- r:aaaa1111 -->\n\n```\nfoo\n```\n\n## New\n<!-- r:bbbb2222 -->\n\nText.\n";
        let left: Vec<(Option<String>, &str)> = feedback_changes(page, &text)
            .into_iter()
            .map(|c| (c.id, c.kind.as_str()))
            .collect();
        assert_eq!(
            left,
            [
                (Some("_top".to_string()), "changed"),
                (Some("aaaa1111".to_string()), "changed"),
                (Some("cccc3333".to_string()), "removed"),
            ],
            "the settled section after the open fence is no change"
        );
    }

    #[test]
    fn relink_rewrites_the_baseline_when_only_the_baseline_holds_the_link() {
        let page = "## Kept\n<!-- r:aaaa1111 -->\n\nNo link here.\n";
        let baseline = render_baseline(&[
            ("aaaa1111".into(), "## Kept\n\nNo link here.\n".into()),
            (
                "dddd4444".into(),
                "## Gone\n\nSee [rollback](rollback.md).\n".into(),
            ),
        ]);
        let rewrite = |s: &str| {
            let n = s.matches("](rollback.md)").count();
            (s.replace("](rollback.md)", "](../guides/rollback.md)"), n)
        };
        let out = relink(page, &baseline, &rewrite).unwrap();
        assert_eq!(
            (out.page.as_str(), out.links),
            (page, 0),
            "the page is untouched"
        );
        assert!(
            out.baseline.contains("](../guides/rollback.md)"),
            "{}",
            out.baseline
        );
    }

    #[test]
    fn relink_leaves_the_frontmatter_alone_when_no_hash_moves() {
        let body = "## Kept\n<!-- r:aaaa1111 -->\n\nNo link here.\n\n## Other\n<!-- r:0b1c2d3e -->\n\nSee [rollback](rollback.md), changed by a person.\n";
        let kept = split_sections(body)[1].text.clone();
        // Written by hand with a four-space indent, which a re-emit would change.
        let page = format!(
            "---\nreflection:\n    aaaa1111:\n        reflected: {}\n    0b1c2d3e:\n        reflected: an older hash\n---\n\n{body}",
            section_hash(&kept)
        );
        let rewrite = |s: &str| {
            let n = s.matches("](rollback.md)").count();
            (s.replace("](rollback.md)", "](../guides/rollback.md)"), n)
        };
        let out = relink(&page, "", &rewrite).unwrap();
        assert_eq!(out.links, 1);
        assert_eq!(
            out.page,
            page.replace("](rollback.md)", "](../guides/rollback.md)")
        );
    }

    #[test]
    fn crlf_renames_reorders_and_a_changed_top_read_as_people_expect() {
        let lf = feedback_changes(CURRENT, &baseline());
        let crlf = feedback_changes(&CURRENT.replace('\n', "\r\n"), &baseline());
        let kinds = |c: &[SectionChange]| -> Vec<(Option<String>, String, &'static str)> {
            c.iter()
                .map(|c| (c.id.clone(), c.heading.clone(), c.kind.as_str()))
                .collect()
        };
        assert_eq!(
            kinds(&crlf),
            kinds(&lf),
            "a CRLF page reads like its LF twin"
        );

        let renamed = CURRENT.replace("## Kept\n", "## Kept, renamed\n");
        let changes = feedback_changes(&renamed, &baseline());
        let kept = changes
            .iter()
            .find(|c| c.id.as_deref() == Some("aaaa1111"))
            .unwrap();
        assert_eq!(
            (kept.heading.as_str(), kept.kind),
            ("Kept, renamed", ChangeKind::Changed)
        );
        assert!(
            kept.before.starts_with("## Kept\n") && kept.after.starts_with("## Kept, renamed\n")
        );

        let page =
            "## One\n<!-- r:aaaa1111 -->\n\nFirst.\n\n## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n";
        let reflected = advance_baseline("", page, &["aaaa1111".into(), "bbbb2222".into()]);
        let swapped =
            "## Two\n<!-- r:bbbb2222 -->\n\nSecond.\n\n## One\n<!-- r:aaaa1111 -->\n\nFirst.\n";
        assert!(
            feedback_changes(swapped, &reflected).is_empty(),
            "a reorder alone is no change"
        );

        let top = CURRENT.replace("Intro.", "A new intro.");
        let changes = feedback_changes(&top, &baseline());
        let first = &changes[0];
        assert_eq!(
            (first.id.as_deref(), first.heading.as_str(), first.kind),
            (Some("_top"), "", ChangeKind::Changed)
        );
        assert!(first.before.contains("Intro.") && first.after.contains("A new intro."));
    }

    #[test]
    fn a_missing_baseline_is_seeded_from_the_reflected_hashes() {
        let body = "# P\n\n## Night\n<!-- r:aaaa1111 -->\n\nAt night.\n\n## Day\n<!-- r:bbbb2222 -->\n\nBy day.\n";
        let night = split_sections(body)
            .into_iter()
            .find(|s| s.id.as_deref() == Some("aaaa1111"))
            .unwrap();
        let page = format!(
            "---\nreflection:\n  aaaa1111:\n    reflected: {}\n  bbbb2222:\n    reflected: stale\n---\n\n{body}",
            section_hash(&night.text)
        );
        let seeded = seed_baseline(&page);
        assert_eq!(baseline_ids(&seeded), ["aaaa1111".to_string()].into());
        assert!(
            feedback_changes(&page, &seeded)
                .iter()
                .all(|c| c.id.as_deref() != Some("aaaa1111"))
        );
        assert_eq!(seed_baseline("# P\n\n## Idea\n\nNew.\n"), "");
    }
}
