//! The per-section bookkeeping a reflection carries in its frontmatter under
//! [`PROVENANCE_KEY`]. None of it is shown to people; whether a section's
//! text changed since the last reflect is derived, never stored.

use std::borrow::Borrow;
use std::collections::{BTreeMap, btree_map};
use std::ops::Index;

use serde::{Deserialize, Serialize};

use crate::frontmatter::{key_blocks, split_frontmatter};
use crate::reflection::sections::{Section, section_hash};

/// The frontmatter key holding the per-section records.
pub const PROVENANCE_KEY: &str = "reflection";

/// One engram a section was built from, with its checksum at that time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceRef {
    /// The engram's domain, by its canonical name, when it is another
    /// domain than the page's; absent for an engram of the page's own
    /// domain, so a record written before sources could cross domains reads
    /// and writes back as it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
    /// The engram's permalink in its domain.
    pub engram: String,
    /// SHA-256 hex of the engram file when the section was reflected.
    pub checksum: String,
}

/// A source as a writer names it in `sources`: a bare permalink names an
/// engram of the page's own domain, a `crystalline://<domain>/<permalink>`
/// address an engram of any domain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceName {
    /// A permalink in the page's own domain.
    Own(String),
    /// An engram by its address: the domain as spelled, and the permalink.
    Address {
        /// The domain segment as the writer spelled it.
        domain: String,
        /// The engram's permalink in that domain.
        permalink: String,
    },
}

/// Read one entry of `sources`. An address must name one engram: a glob, an
/// address without a permalink and an attachment are refused, each with a
/// sentence a writer can act on.
pub fn parse_source_name(name: &str) -> Result<SourceName, String> {
    if !name.starts_with(crate::address::SCHEME) {
        return Ok(SourceName::Own(name.to_string()));
    }
    let what = "a source is one engram: its permalink in this domain, or its crystalline://<domain>/<permalink> address";
    let Some(url) = crate::address::CrystallineUrl::parse(name) else {
        return Err(format!("'{name}' names no domain; {what}"));
    };
    if url.glob {
        return Err(format!("'{name}' names a folder of engrams; {what}"));
    }
    if url.permalink.is_empty() {
        return Err(format!("'{name}' names a domain and no engram; {what}"));
    }
    if url.asset_ref().is_some() {
        return Err(format!("'{name}' names a file, not an engram; {what}"));
    }
    Ok(SourceName::Address {
        domain: url.domain,
        permalink: url.permalink,
    })
}

/// How many sources of `source` store a domain `named` accepts. Beside
/// [`respell_source_domains`], it tells a page whose sources could not be
/// rewritten (a frontmatter that cannot be cut key by key) from one that
/// has none to rewrite.
pub fn source_domains_named(source: &str, named: &dyn Fn(&str) -> bool) -> usize {
    read_provenance(source)
        .values()
        .flat_map(|record| record.sources.iter())
        .filter(|src| src.domain.as_deref().is_some_and(named))
        .count()
}

/// `source` with the stored `domain` of every source that `rename` answers
/// a new name for written with that name, and how many were. The text is
/// returned as it was when nothing changes, so a page without such a source
/// is never rewritten. What a domain rename runs over every page, beside
/// the respelling of the links in its body.
pub fn respell_source_domains(
    source: &str,
    rename: &dyn Fn(&str) -> Option<String>,
) -> (String, usize) {
    let mut provenance = read_provenance(source);
    let mut count = 0;
    for record in provenance.values_mut() {
        for src in &mut record.sources {
            let Some(domain) = src.domain.as_deref() else {
                continue;
            };
            if let Some(new) = rename(domain).filter(|new| new != domain) {
                src.domain = Some(new);
                count += 1;
            }
        }
    }
    if count == 0 {
        return (source.to_string(), 0);
    }
    match write_provenance(source, &provenance) {
        Ok(text) => (text, count),
        Err(_) => (source.to_string(), 0),
    }
}

/// What Crystalline keeps about one section.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionRecord {
    /// The engrams the section was built from.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceRef>,
    /// The section hash at the last reflect.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reflected: Option<String>,
    /// When a write through Crystalline last changed the text, RFC 3339.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub changed_at: Option<String>,
    /// Who changed the text since the last reflect, in order, no repeats.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed_by: Vec<String>,
    /// The open thread that answers the current feedback item, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub answered_by: Option<String>,
}

/// The reserved key in the [`PROVENANCE_KEY`] block that holds the record
/// of the page itself ([`PageRecord`]). It is no section id and never can
/// be one: a section id is 4 to 16 lowercase hex digits
/// ([`parse_id_line`](crate::reflection::parse_id_line)), or `_top` for the
/// text before the first `##`.
pub const PAGE_RECORD_KEY: &str = "_page";

/// The key of the page record that holds when the page was last marked not
/// fresh ([`PageRecord::refresh_requested`]).
pub const REFRESH_REQUESTED_KEY: &str = "refresh_requested";

/// The key of the page record that holds who marked the page not fresh
/// ([`PageRecord::refresh_requested_by`]).
pub const REFRESH_REQUESTED_BY_KEY: &str = "refresh_requested_by";

/// What Crystalline keeps about the page as a whole, under
/// [`PAGE_RECORD_KEY`] in the block.
///
/// A binary older than this record reads `_page` as one more section record
/// with nothing in it, not as unreadable: a section record ignores keys it
/// does not know. That holds only while no key of this record (or a key it
/// keeps in `other`) reuses a [`SectionRecord`] field name (`sources`,
/// `reflected`, `changed_at`, `changed_by`, `answered_by`) with another
/// shape; a new key here must keep to that. Such a binary writes the record
/// back as `_page: {}` when it rewrites the block, or drops it with the
/// section records of sections no longer on the page; `_page: {}` reads as
/// an empty record here.
///
/// A part that does not read is never dropped and never makes the block
/// unreadable (the record holds no sources): an `owner`, a `retired`, a
/// `refresh_requested` or a `refresh_requested_by` that does not read is
/// kept as written in `other`, a `_page` that is no mapping is kept whole
/// in `unread`, both are written back as they were, and each is named in
/// `unreadable` so it can be reported. Each key reads on its own, so a
/// broken `retired` costs `owner` nothing.
///
/// A not-fresh mark ([`PageRecord::refresh_requested`]) is written by a
/// person who wants the next reflect run to reconcile the page in full,
/// and taken off by the reconcile that saw it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PageRecord {
    /// The Reflection page that owns this one, by its path relative to the
    /// Reflection home, as a section's sources name pages.
    pub owner: Option<String>,
    /// When and by whom the page was retired, and the page that owned it
    /// then.
    pub retired: Option<Retirement>,
    /// When the page was last marked not fresh: RFC 3339 in UTC with
    /// milliseconds, as written. Only a value that reads as RFC 3339 is
    /// held here; anything else is kept in `other` and counts as no mark.
    pub refresh_requested: Option<String>,
    /// Who marked the page not fresh, by login.
    pub refresh_requested_by: Option<String>,
    /// Keys this binary does not know, and a typed key that does not read,
    /// kept as they were through a read and a write, so a newer binary's
    /// data survives an older one. A typed key replaces a kept one of the
    /// same name when written.
    pub other: serde_yaml_ng::Mapping,
    /// A `_page` value that is no mapping, kept to be written back as it
    /// was while the record has nothing typed or kept to write instead.
    pub unread: Option<serde_yaml_ng::Value>,
    /// The parts that did not read, for a finding: [`PAGE_RECORD_KEY`] for a
    /// `_page` that is no mapping, else the key that did not read. Filled
    /// by [`read_provenance`]; never written.
    pub unreadable: Vec<String>,
}

impl PageRecord {
    /// Whether there is nothing to record: no owner, no retirement, no
    /// mark, no key this binary does not know and no value kept unread.
    pub fn is_empty(&self) -> bool {
        !self.has_typed() && self.other.is_empty() && self.unread.is_none()
    }

    /// Whether any typed key holds a value.
    fn has_typed(&self) -> bool {
        self.owner.is_some()
            || self.retired.is_some()
            || self.refresh_requested.is_some()
            || self.refresh_requested_by.is_some()
    }

    /// The not-fresh mark in milliseconds since the epoch, when the page
    /// carries one that reads.
    pub fn mark_millis(&self) -> Option<i64> {
        let at = self.refresh_requested.as_deref()?;
        crate::time::parse_rfc3339(at).map(|at| at.stamp().unix_millis())
    }

    /// Mark the page not fresh at `at` (RFC 3339) by `by`, replacing a mark
    /// it carried and a kept value of either key that did not read.
    pub fn set_mark(&mut self, at: String, by: String) {
        self.other.remove(REFRESH_REQUESTED_KEY);
        self.other.remove(REFRESH_REQUESTED_BY_KEY);
        self.refresh_requested = Some(at);
        self.refresh_requested_by = Some(by);
    }

    /// Take the not-fresh mark off: both keys, typed or kept.
    pub fn clear_mark(&mut self) {
        self.other.remove(REFRESH_REQUESTED_KEY);
        self.other.remove(REFRESH_REQUESTED_BY_KEY);
        self.refresh_requested = None;
        self.refresh_requested_by = None;
    }

    /// The record as it is written under [`PAGE_RECORD_KEY`]: `owner`,
    /// `retired`, `refresh_requested`, `refresh_requested_by`, then the
    /// kept keys in their order; the unread value when there is nothing
    /// else.
    fn to_value(&self) -> Result<serde_yaml_ng::Value, String> {
        if !self.has_typed()
            && self.other.is_empty()
            && let Some(raw) = &self.unread
        {
            return Ok(raw.clone());
        }
        let mut map = serde_yaml_ng::Mapping::new();
        if let Some(owner) = &self.owner {
            map.insert("owner".into(), owner.as_str().into());
        }
        if let Some(retired) = &self.retired {
            map.insert(
                "retired".into(),
                serde_yaml_ng::to_value(retired).map_err(|e| e.to_string())?,
            );
        }
        if let Some(at) = &self.refresh_requested {
            map.insert(REFRESH_REQUESTED_KEY.into(), at.as_str().into());
        }
        if let Some(by) = &self.refresh_requested_by {
            map.insert(REFRESH_REQUESTED_BY_KEY.into(), by.as_str().into());
        }
        for (key, value) in &self.other {
            let typed = match key.as_str() {
                Some("owner") => self.owner.is_some(),
                Some("retired") => self.retired.is_some(),
                Some(REFRESH_REQUESTED_KEY) => self.refresh_requested.is_some(),
                Some(REFRESH_REQUESTED_BY_KEY) => self.refresh_requested_by.is_some(),
                _ => false,
            };
            if !typed {
                map.insert(key.clone(), value.clone());
            }
        }
        Ok(serde_yaml_ng::Value::Mapping(map))
    }

    /// The record a `_page` value holds. Nothing (`_page:` with no value)
    /// reads as an empty record.
    fn read(value: &serde_yaml_ng::Value) -> PageRecord {
        let mut page = PageRecord::default();
        let map = match value {
            serde_yaml_ng::Value::Null => return page,
            serde_yaml_ng::Value::Mapping(map) => map,
            other => {
                page.unread = Some(other.clone());
                page.unreadable.push(PAGE_RECORD_KEY.to_string());
                return page;
            }
        };
        for (key, value) in map {
            match key.as_str() {
                Some("owner") => match serde_yaml_ng::from_value::<String>(value.clone()) {
                    Ok(owner) => page.owner = Some(owner),
                    Err(_) => {
                        page.other.insert(key.clone(), value.clone());
                        page.unreadable.push("owner".to_string());
                    }
                },
                Some("retired") => match serde_yaml_ng::from_value::<Retirement>(value.clone()) {
                    Ok(retired) => page.retired = Some(retired),
                    Err(_) => {
                        page.other.insert(key.clone(), value.clone());
                        page.unreadable.push("retired".to_string());
                    }
                },
                Some(REFRESH_REQUESTED_KEY) => match value {
                    serde_yaml_ng::Value::String(at)
                        if crate::time::parse_rfc3339(at).is_some() =>
                    {
                        page.refresh_requested = Some(at.clone());
                    }
                    _ => {
                        page.other.insert(key.clone(), value.clone());
                        page.unreadable.push(REFRESH_REQUESTED_KEY.to_string());
                    }
                },
                Some(REFRESH_REQUESTED_BY_KEY) => match value {
                    serde_yaml_ng::Value::String(by) => {
                        page.refresh_requested_by = Some(by.clone());
                    }
                    _ => {
                        page.other.insert(key.clone(), value.clone());
                        page.unreadable.push(REFRESH_REQUESTED_BY_KEY.to_string());
                    }
                },
                _ => {
                    page.other.insert(key.clone(), value.clone());
                }
            }
        }
        page
    }
}

/// How a page was retired.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Retirement {
    /// When the page was retired, RFC 3339.
    pub at: String,
    /// Who retired it.
    pub by: String,
    /// The page that owned it when it was retired, so a restore can put it
    /// back under that page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_owned_by: Option<String>,
    /// The page's `status` when it was retired, so a restore puts it back;
    /// `None` when it had none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub was_status: Option<String>,
    /// Keys this binary does not know, kept through a read and a write.
    #[serde(flatten)]
    pub other: serde_yaml_ng::Mapping,
}

/// The engine-owned block of a reflection: the records of its sections, by
/// section id, and the record of the page itself.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provenance {
    /// The records of the sections, by section id.
    pub sections: BTreeMap<String, SectionRecord>,
    /// The record of the page, written under [`PAGE_RECORD_KEY`] after the
    /// section records when it holds anything.
    pub page: PageRecord,
}

impl Provenance {
    /// No section records and an empty page record.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is nothing to record: no section record and an empty
    /// page record. A block with nothing in it is not written.
    pub fn is_empty(&self) -> bool {
        self.sections.is_empty() && self.page.is_empty()
    }

    /// The record of section `id`.
    pub fn get<Q>(&self, id: &Q) -> Option<&SectionRecord>
    where
        String: Borrow<Q>,
        Q: Ord + ?Sized,
    {
        self.sections.get(id)
    }

    /// Put the record of section `id`, answering the one it replaces.
    pub fn insert(&mut self, id: String, record: SectionRecord) -> Option<SectionRecord> {
        self.sections.insert(id, record)
    }

    /// The section ids, in order.
    pub fn keys(&self) -> btree_map::Keys<'_, String, SectionRecord> {
        self.sections.keys()
    }

    /// The section records, in id order.
    pub fn values(&self) -> btree_map::Values<'_, String, SectionRecord> {
        self.sections.values()
    }

    /// The section records, in id order, to change in place.
    pub fn values_mut(&mut self) -> btree_map::ValuesMut<'_, String, SectionRecord> {
        self.sections.values_mut()
    }

    /// The section ids, in order.
    pub fn into_keys(self) -> btree_map::IntoKeys<String, SectionRecord> {
        self.sections.into_keys()
    }

    /// The section records, in id order.
    pub fn into_values(self) -> btree_map::IntoValues<String, SectionRecord> {
        self.sections.into_values()
    }
}

impl<Q> Index<&Q> for Provenance
where
    String: Borrow<Q>,
    Q: Ord + ?Sized,
{
    type Output = SectionRecord;

    /// The record of a section; panics when there is none, as a map does.
    fn index(&self, id: &Q) -> &SectionRecord {
        &self.sections[id]
    }
}

impl IntoIterator for Provenance {
    type Item = (String, SectionRecord);
    type IntoIter = btree_map::IntoIter<String, SectionRecord>;

    /// The section records with their ids, in id order. The page record
    /// is not among them: what is collected from this is sections only.
    fn into_iter(self) -> Self::IntoIter {
        self.sections.into_iter()
    }
}

impl<'a> IntoIterator for &'a Provenance {
    type Item = (&'a String, &'a SectionRecord);
    type IntoIter = btree_map::Iter<'a, String, SectionRecord>;

    /// The section records with their ids, in id order.
    fn into_iter(self) -> Self::IntoIter {
        self.sections.iter()
    }
}

fn provenance_block(provenance: &Provenance) -> Result<String, String> {
    if provenance.sections.contains_key(PAGE_RECORD_KEY) {
        return Err(format!(
            "'{PAGE_RECORD_KEY}' is reserved for the page's own record and is no section id"
        ));
    }
    // The section records go first, sorted by id as the map writes them, so
    // a block without a page record is written as it always was; the page
    // record goes last, which a sorted map would not do (`_` sorts between
    // the digits and the letters an id starts with).
    let mut records =
        match serde_yaml_ng::to_value(&provenance.sections).map_err(|e| e.to_string())? {
            serde_yaml_ng::Value::Mapping(records) => records,
            _ => serde_yaml_ng::Mapping::new(),
        };
    if !provenance.page.is_empty() {
        records.insert(PAGE_RECORD_KEY.into(), provenance.page.to_value()?);
    }
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(
        PROVENANCE_KEY.into(),
        serde_yaml_ng::Value::Mapping(records),
    );
    serde_yaml_ng::to_string(&map).map_err(|e| e.to_string())
}

/// The records in `source`'s frontmatter; empty when there is no
/// frontmatter, no key, or it does not read as a map. One record that does
/// not read is dropped and the others are kept, so a hand edit that breaks
/// one section's record never costs the rest their sources on the next
/// write. The page record is never dropped (see [`PageRecord`]).
pub fn read_provenance(source: &str) -> Provenance {
    let Some(parts) = split_frontmatter(source) else {
        return Provenance::new();
    };
    let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str(parts.yaml) else {
        return Provenance::new();
    };
    let Some(serde_yaml_ng::Value::Mapping(records)) = map.get(PROVENANCE_KEY) else {
        return Provenance::new();
    };
    let mut provenance = Provenance::new();
    for (id, record) in records {
        let id = match id {
            serde_yaml_ng::Value::String(id) => id.clone(),
            other => match serde_yaml_ng::to_string(other) {
                Ok(id) => id.trim().to_string(),
                Err(_) => continue,
            },
        };
        if id == PAGE_RECORD_KEY {
            provenance.page = PageRecord::read(record);
            continue;
        }
        if let Ok(record) = serde_yaml_ng::from_value::<SectionRecord>(record.clone()) {
            provenance.sections.insert(id, record);
        }
    }
    provenance
}

/// Whether `source` may hold section records that [`read_provenance`]
/// cannot read, so a check that must see every source cannot trust what it
/// reads: a leading `---` that never closes, frontmatter that is not a YAML
/// mapping, a [`PROVENANCE_KEY`] that is not a mapping, or a section record
/// that does not read. The page record holds no sources and is kept
/// whatever it holds, so it never makes a page unreadable. A page with no
/// frontmatter, with empty frontmatter or without the key reads fine.
pub fn provenance_unreadable(source: &str) -> bool {
    let Some(parts) = split_frontmatter(source) else {
        let first = source.split('\n').next().unwrap_or("");
        return first.trim_end_matches('\r') == "---";
    };
    let map = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(parts.yaml) {
        Ok(serde_yaml_ng::Value::Mapping(map)) => map,
        Ok(serde_yaml_ng::Value::Null) => return false,
        _ => return true,
    };
    match map.get(PROVENANCE_KEY) {
        None => false,
        Some(serde_yaml_ng::Value::Mapping(records)) => records.iter().any(|(id, record)| {
            id.as_str() != Some(PAGE_RECORD_KEY)
                && serde_yaml_ng::from_value::<SectionRecord>(record.clone()).is_err()
        }),
        Some(_) => true,
    }
}

/// The [`PROVENANCE_KEY`] block of `source` as YAML, in whatever shape it
/// was written, or `None` when there is none. A write holds the block it was
/// sent against the stored page's this way: a block a writer made up (`for:`,
/// `built_from:`) reads as no records at all, so comparing records would take
/// it for the stored one.
pub fn block_as_written(source: &str) -> Option<String> {
    let parts = split_frontmatter(source)?;
    let serde_yaml_ng::Value::Mapping(map) = serde_yaml_ng::from_str(parts.yaml).ok()? else {
        return None;
    };
    serde_yaml_ng::to_string(map.get(PROVENANCE_KEY)?).ok()
}

/// `source` with its records replaced by `provenance`: the key block is
/// rewritten in place, appended when absent, removed when `provenance` is
/// empty. A page with no frontmatter gets one only when there is something
/// to record, and a frontmatter that held nothing but the records goes with
/// them; one the author wrote (an empty one too) stays. `Err` when the
/// frontmatter cannot be cut into key blocks.
pub fn write_provenance(source: &str, provenance: &Provenance) -> Result<String, String> {
    let Some(parts) = split_frontmatter(source) else {
        if provenance.is_empty() {
            return Ok(source.to_string());
        }
        return Ok(format!(
            "---\n{}---\n\n{source}",
            provenance_block(provenance)?
        ));
    };
    let blocks = key_blocks(parts.yaml).ok_or_else(|| {
        "this reflection's frontmatter cannot be read key by key (a flow-style block, a line before the first key, or a key that does not stand on its own); write it as plain `key: value` lines"
            .to_string()
    })?;
    let mut yaml = blocks.preamble.clone();
    let mut placed = false;
    for block in &blocks.blocks {
        if block.key == PROVENANCE_KEY {
            if !placed && !provenance.is_empty() {
                yaml.push_str(&provenance_block(provenance)?);
            }
            placed = true;
            continue;
        }
        yaml.push_str(&block.text);
        if !block.text.ends_with('\n') {
            yaml.push('\n');
        }
    }
    if !placed && !provenance.is_empty() {
        yaml.push_str(&provenance_block(provenance)?);
    }
    let had_records = blocks.blocks.iter().any(|b| b.key == PROVENANCE_KEY);
    if had_records && yaml.trim().is_empty() {
        return Ok(parts.body.trim_start_matches(['\n', '\r']).to_string());
    }
    Ok(format!("{}{yaml}{}{}", parts.open, parts.close, parts.body))
}

/// A reflection's title: the frontmatter `title`, else the first `# `
/// heading outside fenced code, else the file stem.
pub fn title_of(source: &str, path: &str) -> String {
    if let Some(parts) = split_frontmatter(source)
        && let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str(parts.yaml)
        && let Some(serde_yaml_ng::Value::String(title)) = map.get("title")
        && !title.trim().is_empty()
    {
        return title.trim().to_string();
    }
    if let Some(heading) = crate::headings::heading_spans(source)
        .into_iter()
        .find(|h| h.level == 1)
    {
        return heading.text.trim().to_string();
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_string()
}

/// Whether a section's text differs from what was last reflected.
pub fn text_changed(record: Option<&SectionRecord>, section: &Section) -> bool {
    record
        .and_then(|r| r.reflected.as_deref())
        .is_none_or(|hash| hash != section_hash(&section.text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::reflection::sections::split_sections;

    const PAGE: &str = "---\ntitle: Deploying\ntags: [ops]\n---\n\n# Deploying\n\n## Night deploys\n<!-- r:7f3a9c2e -->\n\nDeploys run at night.\n";

    /// What [`read_provenance`] cannot read in full is said: a check that
    /// must not miss a source fails closed on it, while a page with no
    /// frontmatter, or none with records, reads fine.
    #[test]
    fn unreadable_records_are_told_apart_from_none() {
        let ok = [
            "# A\n\nNo frontmatter.\n",
            PAGE,
            "---\nreflection:\n  aaaa1111:\n    sources:\n    - engram: team/rota\n      checksum: abc\n---\n\n# A\n",
            "---\nreflection: {}\n---\n\n# A\n",
            "---\n---\n\n# A\n",
        ];
        for page in ok {
            assert!(!provenance_unreadable(page), "{page}");
        }
        let unreadable = [
            // A leading fence that never closes.
            "---\nreflection:\n  aaaa1111:\n    sources: []\n\n# A\n",
            // Frontmatter that is no mapping, or no YAML at all.
            "---\n- a\n- b\n---\n\n# A\n",
            "---\nreflection: [\n---\n\n# A\n",
            // The key there, but no mapping.
            "---\nreflection: 5\n---\n\n# A\n",
            // One record that does not read.
            "---\nreflection:\n  aaaa1111:\n    sources: 5\n---\n\n# A\n",
        ];
        for page in unreadable {
            assert!(provenance_unreadable(page), "{page}");
        }
    }

    /// The block as it stands, in any shape: what a writer's block is held
    /// against, so a page sent back as it was read is told nothing.
    #[test]
    fn the_block_is_read_as_written() {
        let theirs = "---\nreflection:\n  for: new operators\n---\n\n# A\n";
        assert_eq!(
            block_as_written(theirs).as_deref(),
            Some("for: new operators\n")
        );
        assert!(read_provenance(theirs).is_empty(), "not a record");
        assert_eq!(block_as_written(PAGE), None);
        assert_eq!(block_as_written("# No frontmatter\n"), None);
    }

    fn record() -> SectionRecord {
        SectionRecord {
            sources: vec![SourceRef {
                domain: None,
                engram: "runbooks/deploy".into(),
                checksum: "abc".into(),
            }],
            reflected: Some("hash".into()),
            changed_at: None,
            changed_by: vec![],
            answered_by: None,
        }
    }

    #[test]
    fn provenance_round_trips_and_keeps_the_rest_of_the_frontmatter() {
        let mut prov = Provenance::new();
        prov.insert("7f3a9c2e".into(), record());
        let written = write_provenance(PAGE, &prov).unwrap();
        assert!(written.starts_with("---\ntitle: Deploying\ntags: [ops]\nreflection:\n"));
        assert!(
            written.ends_with("## Night deploys\n<!-- r:7f3a9c2e -->\n\nDeploys run at night.\n")
        );
        assert_eq!(read_provenance(&written), prov);
        let rewritten = write_provenance(&written, &prov).unwrap();
        assert_eq!(rewritten, written, "idempotent");
        let cleared = write_provenance(&written, &Provenance::new()).unwrap();
        assert_eq!(cleared, PAGE, "an empty map removes the key");
    }

    #[test]
    fn a_page_without_frontmatter_gets_one_only_when_there_is_something_to_say() {
        let bare = "# Bare\n\n## A\n<!-- r:aaaa1111 -->\n\nText.\n";
        assert_eq!(write_provenance(bare, &Provenance::new()).unwrap(), bare);
        let mut prov = Provenance::new();
        prov.insert("aaaa1111".into(), record());
        let written = write_provenance(bare, &prov).unwrap();
        assert!(written.starts_with("---\nreflection:\n"));
        assert!(written.ends_with(bare));
        assert_eq!(
            write_provenance(&written, &Provenance::new()).unwrap(),
            bare,
            "a frontmatter that only held the records goes with them"
        );
        assert_eq!(read_provenance(bare), Provenance::new());
        assert_eq!(
            read_provenance("---\nreflection: [broken\n---\n"),
            Provenance::new()
        );
    }

    /// A record written before sources could name another domain has no
    /// `domain`: it reads, and writes back byte for byte.
    #[test]
    fn a_record_without_a_domain_writes_back_byte_for_byte() {
        let old = "---\ntitle: T\nreflection:\n  aaaa1111:\n    sources:\n    - engram: runbooks/deploy\n      checksum: abc\n    reflected: hash\n---\n\n# T\n";
        let prov = read_provenance(old);
        assert_eq!(prov["aaaa1111"].sources[0].domain, None);
        assert_eq!(write_provenance(old, &prov).unwrap(), old);
    }

    /// A source of another domain keeps its domain through a write and a
    /// read.
    #[test]
    fn a_source_of_another_domain_round_trips() {
        let mut rec = record();
        rec.sources.push(SourceRef {
            domain: Some("harbor".into()),
            engram: "team/harbor-team".into(),
            checksum: "def".into(),
        });
        let mut prov = Provenance::new();
        prov.insert("7f3a9c2e".into(), rec);
        let written = write_provenance(PAGE, &prov).unwrap();
        assert!(
            written.contains("  - domain: harbor\n      engram: team/harbor-team\n"),
            "{written}"
        );
        assert!(
            written.contains("  - engram: runbooks/deploy\n"),
            "an own source names no domain: {written}"
        );
        assert_eq!(read_provenance(&written), prov);
    }

    #[test]
    fn a_source_is_a_permalink_or_an_address_of_one_engram() {
        assert_eq!(
            parse_source_name("runbooks/deploy"),
            Ok(SourceName::Own("runbooks/deploy".into()))
        );
        assert_eq!(
            parse_source_name("crystalline://harbor/team/harbor-team"),
            Ok(SourceName::Address {
                domain: "harbor".into(),
                permalink: "team/harbor-team".into(),
            })
        );
        for refused in [
            "crystalline://harbor/team/*",
            "crystalline://harbor/*",
            "crystalline://harbor",
            "crystalline://harbor/",
            "crystalline:///team/x",
            "crystalline://harbor/assets/diagram.png",
            "crystalline://harbor/reflections/assets/diagram.png",
        ] {
            let err = parse_source_name(refused).unwrap_err();
            assert!(err.contains(refused), "{err}");
            assert!(err.contains("a source is one engram"), "{err}");
        }
    }

    /// A rename rewrites the stored domain of the sources it names and
    /// leaves every other byte; a page with none is returned as it was.
    #[test]
    fn a_rename_respells_the_stored_domain_only() {
        let page = "---\ntitle: T\nreflection:\n  aaaa1111:\n    sources:\n    - engram: own/one\n      checksum: a\n    - domain: harbor\n      engram: team/x\n      checksum: b\n    - domain: other\n      engram: o/y\n      checksum: c\n---\n\n# T\n\nharbor stays in the body.\n";
        let rename = |d: &str| (d == "harbor").then(|| "crew".to_string());
        let (out, n) = respell_source_domains(page, &rename);
        assert_eq!(n, 1);
        assert_eq!(out, page.replace("domain: harbor", "domain: crew"));
        let (again, n) = respell_source_domains(&out, &rename);
        assert_eq!((again.as_str(), n), (out.as_str(), 0), "idempotent");
        let plain = "# Plain\n\nharbor\n";
        assert_eq!(
            respell_source_domains(plain, &rename),
            (plain.to_string(), 0)
        );
        let named = |d: &str| d == "harbor";
        assert_eq!(source_domains_named(page, &named), 1);
        // A flow-style frontmatter reads, but cannot be rewritten: the
        // count tells it from a page with nothing to rewrite.
        let flow = "---\n{title: T, reflection: {aaaa1111: {sources: [{domain: harbor, engram: team/x, checksum: b}]}}}\n---\n\n# T\n";
        assert_eq!(respell_source_domains(flow, &rename), (flow.to_string(), 0));
        assert_eq!(source_domains_named(flow, &named), 1);
    }

    #[test]
    fn an_empty_frontmatter_the_author_wrote_is_kept() {
        let page = "---\n---\n\n# P\n";
        assert_eq!(write_provenance(page, &Provenance::new()).unwrap(), page);
    }

    #[test]
    fn one_record_that_does_not_read_costs_only_itself() {
        let page = "---\nreflection:\n  aaaa1111:\n    reflected: x\n  bbbb2222: 5\n---\n\n# P\n";
        let prov = read_provenance(page);
        assert_eq!(
            prov.keys().collect::<Vec<_>>(),
            ["aaaa1111"],
            "the others survive: {prov:?}"
        );
        assert_eq!(prov["aaaa1111"].reflected.as_deref(), Some("x"));
    }

    #[test]
    fn titles_come_from_the_frontmatter_the_heading_or_the_name() {
        assert_eq!(title_of(PAGE, "guides/deploying.md"), "Deploying");
        assert_eq!(title_of("# From heading\n", "a.md"), "From heading");
        assert_eq!(
            title_of("no heading\n", "guides/night-deploys.md"),
            "night-deploys"
        );
        assert_eq!(
            title_of("```\n# not a title\n```\n\n# Real title\n", "a.md"),
            "Real title",
            "a heading inside a fence is no title"
        );
    }

    #[test]
    fn text_changed_is_derived_from_the_reflected_hash() {
        let sections = split_sections(PAGE);
        let mut rec = record();
        assert!(
            text_changed(Some(&rec), &sections[1]),
            "the stored hash is not this text's"
        );
        rec.reflected = Some(crate::reflection::sections::section_hash(&sections[1].text));
        assert!(!text_changed(Some(&rec), &sections[1]));
        assert!(text_changed(None, &sections[1]), "never reflected");
    }

    /// A page that owns nothing and was never retired is written exactly as
    /// before the page record existed: the block an earlier binary wrote
    /// for the same records, byte for byte, section ids starting with a
    /// digit and with a letter alike.
    #[test]
    fn a_page_without_a_page_record_is_written_as_before() {
        let pages = [
            PAGE.to_string(),
            "# Bare\n\n## A\n<!-- r:aaaa1111 -->\n\nText.\n".to_string(),
            "---\ntitle: T\nreflection:\n  1234abcd:\n    reflected: x\n  _top:\n    changed_by:\n    - ada\n  aaaa1111:\n    sources:\n    - domain: harbor\n      engram: team/x\n      checksum: b\n    reflected: y\n---\n\n# T\n".to_string(),
        ];
        for page in &pages {
            let mut prov = read_provenance(page);
            assert_eq!(
                serde_yaml_ng::to_value(older_binary::read_provenance(page)).unwrap(),
                serde_yaml_ng::to_value(&prov.sections).unwrap(),
                "both binaries read the same records: {page}"
            );
            prov.insert("0abc1234".into(), record());
            prov.insert("ffff0000".into(), record());
            assert!(prov.page.is_empty());
            let now = write_provenance(page, &prov).unwrap();
            let then = older_binary::write_provenance(page, &prov.sections).unwrap();
            assert_eq!(now, then, "{page}");
            assert_eq!(read_provenance(&now), prov);
        }
    }

    fn owned_and_retired() -> PageRecord {
        PageRecord {
            owner: Some("arch/overview.md".into()),
            retired: Some(Retirement {
                at: "2026-11-20T10:00:00Z".into(),
                by: "ada".into(),
                was_owned_by: Some("arch/overview.md".into()),
                was_status: Some("draft".into()),
                other: serde_yaml_ng::Mapping::new(),
            }),
            ..PageRecord::default()
        }
    }

    /// The page record is written last in the block, after section ids that
    /// start with a digit and with a letter (a sorted map would put `_page`
    /// between them), and reads back as written.
    #[test]
    fn the_page_record_is_written_last_and_round_trips() {
        let mut prov = Provenance::new();
        prov.insert("0abc1234".into(), record());
        prov.insert("aaaa1111".into(), record());
        prov.page = owned_and_retired();
        let written = write_provenance(PAGE, &prov).unwrap();
        let block = block_as_written(&written).unwrap();
        let keys: Vec<&str> = block
            .lines()
            .filter(|l| !l.starts_with(' '))
            .map(|l| l.trim_end_matches(':'))
            .collect();
        assert_eq!(keys, ["0abc1234", "aaaa1111", "_page"], "{block}");
        assert!(
            block.ends_with(
                "_page:\n  owner: arch/overview.md\n  retired:\n    at: 2026-11-20T10:00:00Z\n    by: ada\n    was_owned_by: arch/overview.md\n    was_status: draft\n"
            ),
            "{block}"
        );
        assert_eq!(read_provenance(&written), prov);
        assert_eq!(write_provenance(&written, &prov).unwrap(), written);
        assert!(!provenance_unreadable(&written));
    }

    /// A page with nothing but a page record (an owned page no section of
    /// which was reflected yet) keeps its block; an empty page record is
    /// nothing to write.
    #[test]
    fn a_page_record_alone_is_something_to_record() {
        let bare = "# Child\n\nText.\n";
        let prov = Provenance {
            sections: BTreeMap::new(),
            page: PageRecord {
                owner: Some("arch/overview.md".into()),
                ..PageRecord::default()
            },
        };
        assert!(!prov.is_empty());
        let written = write_provenance(bare, &prov).unwrap();
        assert_eq!(
            written,
            "---\nreflection:\n  _page:\n    owner: arch/overview.md\n---\n\n# Child\n\nText.\n"
        );
        assert_eq!(read_provenance(&written), prov);
        assert_eq!(
            write_provenance(&written, &Provenance::new()).unwrap(),
            bare,
            "an empty provenance removes the block"
        );
    }

    /// Keys a newer binary writes in the page record, and in its
    /// retirement, survive a read and a write here, in their order.
    #[test]
    fn unknown_keys_in_the_page_record_are_kept() {
        let page = "---\ntitle: T\nreflection:\n  aaaa1111:\n    reflected: x\n  _page:\n    owner: arch/overview.md\n    retired:\n      at: 2026-11-20T10:00:00Z\n      by: ada\n      note: kept\n    zeta: 1\n    alpha:\n      nested: true\n---\n\n# T\n";
        let prov = read_provenance(page);
        assert_eq!(prov.page.owner.as_deref(), Some("arch/overview.md"));
        let retired = prov.page.retired.as_ref().unwrap();
        assert_eq!(
            (retired.at.as_str(), retired.by.as_str()),
            ("2026-11-20T10:00:00Z", "ada")
        );
        assert_eq!(retired.was_owned_by, None);
        assert_eq!(retired.other.len(), 1);
        assert_eq!(prov.page.other.len(), 2);
        assert_eq!(write_provenance(page, &prov).unwrap(), page);
        assert!(!provenance_unreadable(page));
    }

    /// `_page: {}` (what an older binary writes back) and a `_page` with no
    /// value read as an empty page record and are written as nothing.
    #[test]
    fn an_empty_page_record_reads_and_goes() {
        for page in [
            "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page: {}\n---\n\n# T\n",
            "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page:\n---\n\n# T\n",
        ] {
            assert!(!provenance_unreadable(page), "{page}");
            let prov = read_provenance(page);
            assert!(prov.page.is_empty(), "{page}");
            assert_eq!(prov.keys().collect::<Vec<_>>(), ["aaaa1111"], "{page}");
            assert_eq!(
                write_provenance(page, &prov).unwrap(),
                "---\nreflection:\n  aaaa1111:\n    reflected: x\n---\n\n# T\n"
            );
        }
    }

    /// A page record, or a part of it, that does not read leaves the
    /// section records readable and the page readable, is named for a
    /// finding, and is written back as it was on the next write: it is never
    /// dropped. `owner` and `retired` read on their own, so a broken
    /// `retired` costs `owner` nothing.
    #[test]
    fn a_page_record_that_does_not_read_is_kept_as_written() {
        let cases: [(&str, &[&str], Option<&str>); 4] = [
            (
                "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page: 5\n---\n\n# T\n",
                &["_page"],
                None,
            ),
            (
                "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page:\n    owner: arch/overview.md\n    retired:\n      by: ada\n---\n\n# T\n",
                &["retired"],
                Some("arch/overview.md"),
            ),
            (
                "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page:\n    owner:\n    - a\n    - b\n    zeta: 1\n---\n\n# T\n",
                &["owner"],
                None,
            ),
            (
                "---\nreflection:\n  aaaa1111:\n    reflected: x\n  _page:\n    owner: 7\n    retired: soon\n---\n\n# T\n",
                &["owner", "retired"],
                None,
            ),
        ];
        for (page, broken, owner) in cases {
            assert!(!provenance_unreadable(page), "{page}");
            let prov = read_provenance(page);
            assert_eq!(prov.page.unreadable, broken, "{page}");
            assert_eq!(prov.page.owner.as_deref(), owner, "{page}");
            assert!(!prov.page.is_empty(), "{page}");
            assert_eq!(prov["aaaa1111"].reflected.as_deref(), Some("x"), "{page}");
            assert_eq!(
                write_provenance(page, &prov).unwrap(),
                page,
                "kept as written"
            );
            let mut changed = prov.clone();
            changed.insert("bbbb2222".into(), record());
            let written = write_provenance(page, &changed).unwrap();
            assert_eq!(
                block_as_written(&written)
                    .unwrap()
                    .split_once("_page:")
                    .unwrap()
                    .1,
                block_as_written(page)
                    .unwrap()
                    .split_once("_page:")
                    .unwrap()
                    .1,
                "a write of other records keeps the page record: {written}"
            );
        }
    }

    /// A typed `owner` or `retired` set over a kept one that did not read
    /// replaces it, so the key is written once; a typed record over an
    /// unread `_page` writes the record.
    #[test]
    fn a_typed_part_replaces_the_kept_one() {
        let page = "---\nreflection:\n  _page:\n    owner: 7\n    zeta: 1\n---\n\n# T\n";
        let mut prov = read_provenance(page);
        prov.page.owner = Some("arch/overview.md".into());
        assert_eq!(
            write_provenance(page, &prov).unwrap(),
            "---\nreflection:\n  _page:\n    owner: arch/overview.md\n    zeta: 1\n---\n\n# T\n"
        );
        let page = "---\nreflection:\n  _page: 5\n---\n\n# T\n";
        let mut prov = read_provenance(page);
        prov.page.owner = Some("arch/overview.md".into());
        assert_eq!(
            write_provenance(page, &prov).unwrap(),
            "---\nreflection:\n  _page:\n    owner: arch/overview.md\n---\n\n# T\n"
        );
    }

    /// A not-fresh mark is written after `owner` and `retired` and before
    /// the kept keys, reads back as written, and goes with `clear_mark`.
    #[test]
    fn a_mark_round_trips_after_owner_and_retired_and_goes_again() {
        let mut prov = Provenance::new();
        prov.insert("aaaa1111".into(), record());
        prov.page = owned_and_retired();
        prov.page
            .other
            .insert("zeta".into(), serde_yaml_ng::Value::Bool(true));
        prov.page
            .set_mark("2026-11-25T14:03:07.412Z".into(), "ada".into());
        let written = write_provenance(PAGE, &prov).unwrap();
        let block = block_as_written(&written).unwrap();
        let page_keys: Vec<&str> = block
            .split_once(
                "_page:
",
            )
            .unwrap()
            .1
            .lines()
            .filter(|l| l.starts_with("  ") && !l.starts_with("   "))
            .map(|l| l.trim().split(':').next().unwrap())
            .collect();
        assert_eq!(
            page_keys,
            [
                "owner",
                "retired",
                "refresh_requested",
                "refresh_requested_by",
                "zeta"
            ],
            "{block}"
        );
        assert!(
            block.contains("  refresh_requested: 2026-11-25T14:03:07.412Z\n"),
            "{block}"
        );
        let back = read_provenance(&written);
        assert_eq!(back, prov);
        assert_eq!(back.page.mark_millis(), Some(1_795_615_387_412));
        assert_eq!(write_provenance(&written, &back).unwrap(), written);

        let mut cleared = back.clone();
        cleared.page.clear_mark();
        assert_eq!(cleared.page.mark_millis(), None);
        let without = write_provenance(&written, &cleared).unwrap();
        assert!(!without.contains("refresh_requested"), "{without}");
        assert!(without.contains("zeta: true"), "{without}");

        // A mark alone is something to record, and clearing it takes the
        // whole page record away.
        let mut only = Provenance::new();
        only.page
            .set_mark("2026-11-25T14:03:07.412Z".into(), "ada".into());
        assert!(!only.is_empty());
        let bare = "# T\n";
        let marked = write_provenance(bare, &only).unwrap();
        assert_eq!(
            marked,
            "---\nreflection:\n  _page:\n    refresh_requested: 2026-11-25T14:03:07.412Z\n    refresh_requested_by: ada\n---\n\n# T\n"
        );
        let mut read = read_provenance(&marked);
        read.page.clear_mark();
        assert!(read.page.is_empty());
        assert_eq!(write_provenance(&marked, &read).unwrap(), bare);
    }

    /// A mark that is no RFC 3339 time, or a `refresh_requested_by` that is
    /// no string, is kept as written, named in `unreadable` and counts as no
    /// mark; the page settings name an unreadable mark as a finding. A new
    /// mark replaces the kept value.
    #[test]
    fn a_mark_that_does_not_read_is_kept_and_named() {
        let page = "---\nreflection:\n  _page:\n    owner: arch/overview.md\n    refresh_requested: last tuesday\n    refresh_requested_by: 7\n---\n\n# T\n";
        let prov = read_provenance(page);
        assert_eq!(
            prov.page.unreadable,
            ["refresh_requested", "refresh_requested_by"]
        );
        assert_eq!(prov.page.refresh_requested, None);
        assert_eq!(prov.page.mark_millis(), None);
        assert_eq!(prov.page.owner.as_deref(), Some("arch/overview.md"));
        assert_eq!(write_provenance(page, &prov).unwrap(), page);
        let (_, findings) =
            crate::reflection::read_page_settings(page, "arch/overview/db.md", &prov.page);
        let finding = findings
            .iter()
            .find(|f| f.key == REFRESH_REQUESTED_KEY)
            .expect("a finding for the mark");
        assert_eq!(finding.value, "last tuesday");
        assert_eq!(finding.reason, crate::reflection::MARK_UNREADABLE);

        let mut marked = prov.clone();
        marked
            .page
            .set_mark("2026-11-25T14:03:07.412Z".into(), "ada".into());
        let written = write_provenance(page, &marked).unwrap();
        assert_eq!(
            written,
            "---\nreflection:\n  _page:\n    owner: arch/overview.md\n    refresh_requested: 2026-11-25T14:03:07.412Z\n    refresh_requested_by: ada\n---\n\n# T\n"
        );
    }

    /// A binary from before the mark (0.36) reads both keys as keys it does
    /// not know, keeps them in `other` and writes them back as they were:
    /// the same record with the typed keys moved to `other` writes the same
    /// block. A section record reader (0.34.1) ignores them.
    #[test]
    fn an_older_reader_keeps_the_mark_through_a_write() {
        let mut prov = Provenance::new();
        prov.insert("aaaa1111".into(), record());
        prov.page.owner = Some("arch/overview.md".into());
        prov.page
            .set_mark("2026-11-25T14:03:07.412Z".into(), "ada".into());
        let written = write_provenance(PAGE, &prov).unwrap();
        let mut as_036 = read_provenance(&written);
        let at = as_036.page.refresh_requested.take().unwrap();
        let by = as_036.page.refresh_requested_by.take().unwrap();
        as_036
            .page
            .other
            .insert(REFRESH_REQUESTED_KEY.into(), at.as_str().into());
        as_036
            .page
            .other
            .insert(REFRESH_REQUESTED_BY_KEY.into(), by.as_str().into());
        assert_eq!(write_provenance(&written, &as_036).unwrap(), written);
        assert!(!older_binary::provenance_unreadable(&written));
        let old = older_binary::read_provenance(&written);
        assert_eq!(old["_page"], older_binary::SectionRecord::default());
    }

    /// `_page` is never a section id: an id line never names it, and a section record keyed by it is refused on
    /// write instead of being taken for the page record.
    #[test]
    fn the_page_record_key_is_no_section_id() {
        assert_eq!(
            crate::reflection::parse_id_line(&crate::reflection::id_line(PAGE_RECORD_KEY)),
            None
        );
        let mut prov = Provenance::new();
        prov.insert(PAGE_RECORD_KEY.into(), record());
        let err = write_provenance(PAGE, &prov).unwrap_err();
        assert!(err.contains("reserved"), "{err}");
    }

    /// An older binary (0.34.1, the reading rules copied below as they were)
    /// reads the new block, page record included, as one more section record
    /// with nothing in it: not unreadable, and every section record intact.
    #[test]
    fn an_older_binary_reads_the_new_block() {
        let mut prov = Provenance::new();
        prov.insert("0abc1234".into(), record());
        prov.insert("aaaa1111".into(), record());
        prov.page = owned_and_retired();
        prov.page
            .other
            .insert("future_key".into(), serde_yaml_ng::Value::Bool(true));
        let written = write_provenance(PAGE, &prov).unwrap();
        assert!(!older_binary::provenance_unreadable(&written), "{written}");
        let old = older_binary::read_provenance(&written);
        assert_eq!(
            old.keys().collect::<Vec<_>>(),
            ["0abc1234", "_page", "aaaa1111"]
        );
        assert_eq!(old["_page"], older_binary::SectionRecord::default());
        assert_eq!(old["aaaa1111"].sources[0].engram, "runbooks/deploy");
        assert_eq!(old["0abc1234"].reflected.as_deref(), Some("hash"));
    }

    /// The reading and writing rules of 0.34.1 (crystalline-core
    /// `reflection/provenance.rs` at e2c6c2877), copied as they were (only
    /// the writer's refusal text is shortened, and the writer is generic, see
    /// there), so
    /// the tests above can hold the new block against them. The record
    /// struct is copied too: a later change to [`SectionRecord`] must not
    /// quietly change what this older binary reads.
    mod older_binary {
        use std::collections::BTreeMap;

        use serde::{Deserialize, Serialize};

        use crate::frontmatter::{key_blocks, split_frontmatter};

        const PROVENANCE_KEY: &str = "reflection";

        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub struct SourceRef {
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub domain: Option<String>,
            pub engram: String,
            pub checksum: String,
        }

        #[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
        pub struct SectionRecord {
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub sources: Vec<SourceRef>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub reflected: Option<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub changed_at: Option<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            pub changed_by: Vec<String>,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            pub answered_by: Option<String>,
        }

        pub type Provenance = BTreeMap<String, SectionRecord>;

        fn provenance_block<T: Serialize>(
            provenance: &BTreeMap<String, T>,
        ) -> Result<String, String> {
            let mut map = serde_yaml_ng::Mapping::new();
            map.insert(
                PROVENANCE_KEY.into(),
                serde_yaml_ng::to_value(provenance).map_err(|e| e.to_string())?,
            );
            serde_yaml_ng::to_string(&map).map_err(|e| e.to_string())
        }

        pub fn read_provenance(source: &str) -> Provenance {
            let Some(parts) = split_frontmatter(source) else {
                return Provenance::new();
            };
            let Ok(serde_yaml_ng::Value::Mapping(map)) = serde_yaml_ng::from_str(parts.yaml) else {
                return Provenance::new();
            };
            let Some(serde_yaml_ng::Value::Mapping(records)) = map.get(PROVENANCE_KEY) else {
                return Provenance::new();
            };
            records
                .iter()
                .filter_map(|(id, record)| {
                    let id = match id {
                        serde_yaml_ng::Value::String(id) => id.clone(),
                        other => serde_yaml_ng::to_string(other).ok()?.trim().to_string(),
                    };
                    serde_yaml_ng::from_value::<SectionRecord>(record.clone())
                        .ok()
                        .map(|record| (id, record))
                })
                .collect()
        }

        pub fn provenance_unreadable(source: &str) -> bool {
            let Some(parts) = split_frontmatter(source) else {
                let first = source.split('\n').next().unwrap_or("");
                return first.trim_end_matches('\r') == "---";
            };
            let map = match serde_yaml_ng::from_str::<serde_yaml_ng::Value>(parts.yaml) {
                Ok(serde_yaml_ng::Value::Mapping(map)) => map,
                Ok(serde_yaml_ng::Value::Null) => return false,
                _ => return true,
            };
            match map.get(PROVENANCE_KEY) {
                None => false,
                Some(serde_yaml_ng::Value::Mapping(records)) => records.values().any(|record| {
                    serde_yaml_ng::from_value::<SectionRecord>(record.clone()).is_err()
                }),
                Some(_) => true,
            }
        }

        /// The writer, generic over the record type only so the new
        /// [`super::SectionRecord`] can be handed to it: it serializes the
        /// same fields the same way.
        pub fn write_provenance<T: Serialize>(
            source: &str,
            provenance: &BTreeMap<String, T>,
        ) -> Result<String, String> {
            let Some(parts) = split_frontmatter(source) else {
                if provenance.is_empty() {
                    return Ok(source.to_string());
                }
                return Ok(format!(
                    "---\n{}---\n\n{source}",
                    provenance_block(provenance)?
                ));
            };
            let blocks = key_blocks(parts.yaml).ok_or_else(|| "cannot cut".to_string())?;
            let mut yaml = blocks.preamble.clone();
            let mut placed = false;
            for block in &blocks.blocks {
                if block.key == PROVENANCE_KEY {
                    if !placed && !provenance.is_empty() {
                        yaml.push_str(&provenance_block(provenance)?);
                    }
                    placed = true;
                    continue;
                }
                yaml.push_str(&block.text);
                if !block.text.ends_with('\n') {
                    yaml.push('\n');
                }
            }
            if !placed && !provenance.is_empty() {
                yaml.push_str(&provenance_block(provenance)?);
            }
            let had_records = blocks.blocks.iter().any(|b| b.key == PROVENANCE_KEY);
            if had_records && yaml.trim().is_empty() {
                return Ok(parts.body.trim_start_matches(['\n', '\r']).to_string());
            }
            Ok(format!("{}{yaml}{}{}", parts.open, parts.close, parts.body))
        }
    }
}
