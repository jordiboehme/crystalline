//! What a Reflection page says about itself beside its text, as a page read
//! answers it (design 2.1 to 2.4 and 4): its settings as written and what
//! did not read, the pages and pact files it follows, its own pact file, its
//! owner and its retirement, each page named with its title.
//!
//! A page read is the hot path of Fluid's Reflection mode, so the chain is
//! walked over the page stamps (paths and titles, no text) and only the
//! pages on the way are read.

use std::collections::{BTreeMap, BTreeSet};

use crystalline_core::reflection as r;
use crystalline_index::DomainId;

use super::*;

/// A page as another page names it: its path, its title and whether it
/// exists (a missing page is titled by its path).
fn page_ref(path: &str, titles: &BTreeMap<String, String>) -> Value {
    match titles.get(path) {
        Some(title) => json!({ "path": path, "title": title, "exists": true }),
        None => json!({ "path": path, "title": path, "exists": false }),
    }
}

/// The links a page's text gives its pact chain: its `follows` and owner.
fn links_of(path: &str, text: &str) -> r::PageLinks {
    let record = r::read_provenance(text).page;
    r::read_page_settings(text, path, &record).0.links(&record)
}

fn finding(f: &r::PageFinding) -> Value {
    json!({ "page": f.page, "key": f.key, "value": f.value, "reason": f.reason })
}

impl Engine {
    /// The `page` member of a read of the page at `inner` of the Reflection
    /// `domain_id`, whose stored text is `text`: `{freshness, follows,
    /// status, owner, retired, chain, pacts, pact, findings}`.
    ///
    /// - `freshness`: as written when it reads (an hour or more, not on an
    ///   owned page), else null and named in `findings`.
    /// - `follows` and `status`: as written, or null.
    /// - `owner`: `{path, title, exists}` of the page that owns this one.
    /// - `retired`: `{at, by, was_owned_by}` of a retired page, the former
    ///   owner named as `owner` is; all three null for a page retired only
    ///   by a retired word in its status.
    /// - `chain`: the pages it follows, `{path, title}` from the top down,
    ///   the last the one it follows directly.
    /// - `pacts`: the pact files that apply, in order: `PACT.md`, then each
    ///   page pact on the way, then its own.
    /// - `pact`: `{path, exists}` of its own page pact.
    /// - `findings`: `{page, key, value, reason}` for what did not read, on
    ///   this page or on a page up its chain.
    pub(super) async fn page_view(
        &self,
        domain_id: DomainId,
        inner: &str,
        text: &str,
    ) -> Result<Value> {
        let record = r::read_provenance(text).page;
        let (settings, mut findings) = r::read_page_settings(text, inner, &record);
        let reader = self.reader();
        let stamps = reader.list_reflection_stamps(domain_id).await?;
        let mut titles: BTreeMap<String, String> = BTreeMap::new();
        let mut pact_files: BTreeSet<String> = BTreeSet::new();
        for stamp in &stamps {
            match r::classify(&stamp.path) {
                r::ReflectionFile::Page(_) => {
                    titles.insert(stamp.path.clone(), stamp.title.clone());
                }
                r::ReflectionFile::Pact | r::ReflectionFile::PagePact { .. } => {
                    pact_files.insert(stamp.path.clone());
                }
                _ => {}
            }
        }
        titles.insert(inner.to_string(), r::title_of(text, inner));
        // Every page exists with no links until it is read; a page the walk
        // reaches is read and the walk asked again, until every page on the
        // way is one that was read.
        let mut links: BTreeMap<String, r::PageLinks> = titles
            .keys()
            .map(|path| (path.clone(), r::PageLinks::default()))
            .collect();
        links.insert(inner.to_string(), settings.links(&record));
        let mut read: BTreeSet<String> = BTreeSet::from([inner.to_string()]);
        let chain = loop {
            let chain = r::pact_chain(inner, &links, &pact_files);
            let Some(next) = chain.followed.iter().find(|p| !read.contains(*p)).cloned() else {
                break chain;
            };
            if let Some(row) = reader.get_reflection(domain_id, &next).await? {
                links.insert(next.clone(), links_of(&next, &row.content));
            }
            read.insert(next);
        };
        findings.extend(chain.findings.iter().cloned());
        let own_pact = r::page_pact_path(inner);
        // Retired by the record a retire writes, or only by a retired word
        // in the status (a page from before retire, or one written by
        // hand): then there is no time, author or owner to name.
        let retired = match &record.retired {
            Some(retired) => Some(json!({
                "at": retired.at,
                "by": retired.by,
                "was_owned_by": retired.was_owned_by.as_deref().map(|was| page_ref(was, &titles)),
            })),
            None if settings.retired() => {
                Some(json!({ "at": null, "by": null, "was_owned_by": null }))
            }
            None => None,
        };
        Ok(json!({
            "freshness": settings.freshness.as_ref().map(|f| f.written.clone()),
            "follows": settings.follows,
            "status": settings.status,
            "owner": record.owner.as_deref().map(|owner| page_ref(owner, &titles)),
            "retired": retired,
            "chain": chain
                .followed
                .iter()
                .map(|path| json!({ "path": path, "title": titles.get(path).cloned().unwrap_or_else(|| path.clone()) }))
                .collect::<Vec<_>>(),
            "pacts": chain.pacts,
            "pact": { "path": own_pact, "exists": pact_files.contains(&own_pact) },
            "findings": findings.iter().map(finding).collect::<Vec<_>>(),
        }))
    }
}
