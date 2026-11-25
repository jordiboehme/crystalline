//! Marking a Reflection page not fresh (0.37.0): a person or a pipeline
//! asks for a full look at a page, and the next reflect run reconciles it
//! without the "changed_engrams 0" shortcut.
//!
//! The mark is two keys of the page record ([`r::PageRecord`]):
//! `refresh_requested` (when, RFC 3339 in UTC with milliseconds) and
//! `refresh_requested_by` (who). A page is marked while its record carries
//! a mark that reads and it is not retired ([`page_mark`]). The reconcile
//! that saw the mark takes it off again (`reconciled_reflection`), so the
//! file is the only place a mark lives.

use crystalline_core::reflection as r;

use super::page_life::is_retired;
use super::reflection::unattended_mark_refusal;
use super::*;
use crate::params::{MarkPages, MarkStale};
use crate::scope::{ReflectionCaller, ReflectionRight};

/// A page's not-fresh mark as it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    /// The time in milliseconds since the epoch, what every compare uses.
    pub(crate) at_ms: i64,
    /// The time as written.
    pub(crate) at: String,
    /// Who marked it, by login; `None` when the record does not say.
    pub(crate) by: Option<String>,
}

impl Mark {
    /// `{at, by}`, as the page view, the clock and the queue show it.
    pub(crate) fn to_json(&self) -> Value {
        json!({ "at": self.at, "by": self.by })
    }
}

/// The mark the page record `record` carries, when it reads. Whether the
/// page is retired is the caller's to check: a retired page is never
/// marked, whatever its record says.
pub(crate) fn page_mark(record: &r::PageRecord) -> Option<Mark> {
    Some(Mark {
        at_ms: record.mark_millis()?,
        at: record.refresh_requested.clone()?,
        by: record.refresh_requested_by.clone(),
    })
}

/// The mark of the page `text` at `inner` while it is marked: a page (no
/// pact file) that is not retired and carries a mark that reads.
pub(crate) fn text_mark(text: &str, inner: &str) -> Option<Mark> {
    if !matches!(r::classify(inner), r::ReflectionFile::Page(_)) || is_retired(text, inner) {
        return None;
    }
    page_mark(&r::read_provenance(text).page)
}

/// The refusal of a mark that names no page.
const NOTHING_NAMED: &str = "name the pages to mark: path, pages or all: true";

impl Engine {
    /// Mark the pages `mark` names not fresh (`write_reflection` action
    /// `mark_stale`, `POST /domains/{domain}/reflection/mark-stale`, `crystalline
    /// reflect mark-stale`). Needs edit on the Reflection; an unattended
    /// session is refused. Every named page is checked before the first
    /// write: a pact file, a missing page and a retired page are refused.
    /// Without `only_page` the pages each named page owns are marked too,
    /// recursively (a retired one left out); `All` marks every page that is
    /// not retired.
    ///
    /// All pages of one call carry the same time, which is later than every
    /// mark it replaces. Only the page record changes, under each page's
    /// lock, and the file keeps its modified time. Answers `{domain, at, by,
    /// pages, only_page}`.
    pub async fn mark_reflection_stale(
        &self,
        domain: &str,
        mark: &MarkStale,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let named: Option<Vec<String>> = match &mark.pages {
            MarkPages::One(path) => Some(vec![path.trim().to_string()]),
            MarkPages::Many(paths) => {
                let mut out: Vec<String> = Vec::new();
                for path in paths.iter().map(|p| p.trim().to_string()) {
                    if !out.contains(&path) {
                        out.push(path);
                    }
                }
                Some(out)
            }
            MarkPages::All => None,
        };
        if self.is_unattended(caller) {
            let first = named.as_ref().and_then(|named| named.first());
            return Err(EngineError::Forbidden(unattended_mark_refusal(
                domain,
                first.map(String::as_str),
            )));
        }
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Edit)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        let rows = self
            .store
            .lock()
            .await
            .list_reflections(access.domain_id)
            .await?;
        let texts: BTreeMap<&str, &str> = rows
            .iter()
            .map(|row| (row.path.as_str(), row.content.as_str()))
            .collect();
        let set: Vec<String> = match &named {
            Some(named) => {
                if named.is_empty() || named.iter().any(String::is_empty) {
                    return Err(EngineError::Invalid(NOTHING_NAMED.into()));
                }
                for inner in named {
                    self.validate_named_reflection_path(domain, inner).await?;
                    if r::is_pact_file(inner) {
                        return Err(EngineError::Invalid(format!(
                            "'{inner}' is a pact file, no page: only a page is marked not fresh"
                        )));
                    }
                    let text = texts.get(inner.as_str()).ok_or_else(|| {
                        EngineError::NotFound(format!(
                            "no reflection '{inner}' in domain '{domain}'"
                        ))
                    })?;
                    if is_retired(text, inner) {
                        return Err(EngineError::Invalid(retired_refusal(inner)));
                    }
                }
                let owned = r::owned_pages(texts.iter().map(|(p, t)| (*p, *t)));
                let mut set: Vec<String> = Vec::new();
                for inner in named {
                    if !set.contains(inner) {
                        set.push(inner.clone());
                    }
                    if mark.only_page {
                        continue;
                    }
                    for child in super::reflect::owned_tree(&owned, inner) {
                        if !set.contains(&child) {
                            set.push(child);
                        }
                    }
                }
                set
            }
            None => texts
                .iter()
                .filter(|(path, text)| {
                    matches!(r::classify(path), r::ReflectionFile::Page(_))
                        && !is_retired(text, path)
                })
                .map(|(path, _)| path.to_string())
                .collect(),
        };
        // One time for the call, later than every mark it replaces, so a
        // re-mark always moves the mark (and the item's version) forward.
        let newest = set
            .iter()
            .filter_map(|path| texts.get(path.as_str()))
            .filter_map(|text| r::read_provenance(text).page.mark_millis())
            .max();
        let now = time::now().unix_millis();
        let at_ms = newest.map_or(now, |newest| now.max(newest.saturating_add(1)));
        let at = time::Stamp::from_unix_millis(at_ms)
            .ok_or_else(|| EngineError::Internal(format!("no time for {at_ms} ms")))?
            .rfc3339_millis_z();
        let by = caller.author.name.clone();
        let mut marked: Vec<String> = Vec::new();
        for inner in &set {
            let abs = access.abs(inner);
            let lock = self.lock_file(&abs).await;
            let text = read_opt(&abs)?.ok_or_else(|| {
                EngineError::NotFound(format!("no reflection '{inner}' in domain '{domain}'"))
            })?;
            // Retired since the rows were read: a named page is refused, an
            // owned one left out.
            if is_retired(&text, inner) {
                if named.as_ref().is_some_and(|named| named.contains(inner)) {
                    return Err(EngineError::Invalid(retired_refusal(inner)));
                }
                continue;
            }
            let mut prov = r::read_provenance(&text);
            prov.page.set_mark(at.clone(), by.clone());
            let changed = r::write_provenance(&text, &prov).map_err(EngineError::Invalid)?;
            self.land_page_record(domain, &access, inner, &abs, &text, &changed, caller, true)
                .await?;
            drop(lock);
            self.absorb_into_room(domain, inner).await;
            marked.push(inner.clone());
        }
        Ok(json!({
            "domain": domain,
            "at": at,
            "by": by,
            "pages": marked,
            "only_page": mark.only_page,
        }))
    }

    /// The not-fresh mark of the page at `path` of `domain`'s Reflection,
    /// `{at, by}` while it is marked, else null (a retired page, a pact
    /// file and a missing page included). Needs the right to read the
    /// Reflection. Beside the clock, which is null for a page with no
    /// freshness.
    pub async fn reflection_page_mark(
        &self,
        domain: &str,
        path: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        self.require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let reader = self.reader();
        let Some(id) = reader.domain_id(domain).await? else {
            return Ok(Value::Null);
        };
        let Some(row) = reader.get_reflection(id, path).await? else {
            return Ok(Value::Null);
        };
        Ok(text_mark(&row.content, path).map_or(Value::Null, |mark| mark.to_json()))
    }
}

/// The refusal of a mark on a retired page.
fn retired_refusal(inner: &str) -> String {
    format!(
        "'{inner}' is retired and is never reconciled; restore it first (write_reflection action=restore) to mark it not fresh"
    )
}
