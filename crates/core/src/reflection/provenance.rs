//! The per-section bookkeeping a reflection carries in its frontmatter under
//! [`PROVENANCE_KEY`]. None of it is shown to people; whether a section's
//! text changed since the last reflect is derived, never stored.

use std::collections::BTreeMap;

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

/// The records of a reflection, by section id.
pub type Provenance = BTreeMap<String, SectionRecord>;

fn provenance_block(provenance: &Provenance) -> Result<String, String> {
    let mut map = serde_yaml_ng::Mapping::new();
    map.insert(
        PROVENANCE_KEY.into(),
        serde_yaml_ng::to_value(provenance).map_err(|e| e.to_string())?,
    );
    serde_yaml_ng::to_string(&map).map_err(|e| e.to_string())
}

/// The records in `source`'s frontmatter; empty when there is no
/// frontmatter, no key, or it does not read as a map. One record that does
/// not read is dropped and the others are kept, so a hand edit that breaks
/// one section's record never costs the rest their sources on the next
/// write.
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

/// Whether `source` may hold section records that [`read_provenance`]
/// cannot read, so a check that must see every source cannot trust what it
/// reads: a leading `---` that never closes, frontmatter that is not a YAML
/// mapping, a [`PROVENANCE_KEY`] that is not a mapping, or a record that
/// does not read. A page with no frontmatter, with empty frontmatter or
/// without the key reads fine.
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
        Some(serde_yaml_ng::Value::Mapping(records)) => records
            .values()
            .any(|record| serde_yaml_ng::from_value::<SectionRecord>(record.clone()).is_err()),
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
}
