//! A person shares text from a domain fewer people may read on a Reflection
//! page, on purpose.
//!
//! The reach rule ([`Engine::readers_cover`], [`Engine::leaves_github`])
//! refuses a section that cites a domain fewer people may read. Personal
//! knowledge may still belong on a shared page, when a person who may read it
//! decides so. What the rule guards against is a confused deputy: anyone who
//! may comment on a shared page can ask a question in a thread, and an
//! unattended agent that reads every domain would answer it from a
//! confidential one into a page everyone reads. Hiding the source's name does
//! not help there, because the answer itself is the leak. So the line is who
//! decides: a person, asked and answering, never an agent or a run on its own.
//!
//! The engine trusts one bit, [`WriteReflectionParams::disclosed`], which no
//! argument can set (it is never deserialized): the MCP server sets it after
//! its person answered a question, and the JSON API's confirmation route is
//! a person in a browser. Everything else is held here: who may confirm (a
//! caller with a person behind it who may read the source and curate the
//! page), what is recorded and where (the index, per page, never the page
//! file, so a confirmation cannot arrive in a file edit, a pull or a save, and
//! a page's readers never see who else reads it), and how a recorded
//! confirmation is held against the page's readers later.

use crystalline_core::reflection as r;
use serde::{Deserialize, Serialize};

use super::sources::UNKNOWN_READERS_REFUSAL;
use super::*;
use crate::scope::{PageReaders, ReflectionCaller, ReflectionRight, Scope};

/// What a refused write adds when nobody could be asked: how a person can
/// still share the text on purpose. It never points at Fluid, where a
/// refused write leaves nothing to confirm.
/// What a confirmed write is refused with when the page's readers, or the
/// repository it is backed up to, changed between the question and the
/// answer: the person agreed to the readers they were shown.
pub const DISCLOSURE_CHANGED: &str = "who may read this page, or where it is backed up, changed since the person was asked; ask them again";

/// A question a person is asked before text from a narrower domain goes
/// onto a page, with what their yes is bound to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisclosureAsk {
    /// The question, in plain words.
    pub question: String,
    /// A digest of the question and of the page's readers and repository it
    /// was asked about. A confirmation carries it back, and it must still be
    /// what the page has when the confirmation lands.
    pub basis: String,
}

pub const NOBODY_ASKED_NOTE: &str = "a person who may read that domain can share it on purpose by making this write from an MCP client that can put a question to them; an unattended run never can";

/// One person's confirmation that a section may carry text from the engrams
/// of one other domain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Disclosure {
    /// The section's id.
    pub section: String,
    /// The source's domain, by the name the section's sources store.
    pub source: String,
    /// Who confirmed: the account, or the machine owner.
    pub by: String,
    /// When, RFC 3339.
    pub at: String,
    /// Who read the page then, when this instance knows its accounts. A
    /// later reader who was not among them and may not read the source
    /// makes the section too wide again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readers: Option<PageReaders>,
    /// The repository the page was backed up to then, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
}

/// The confirmations recorded on one domain's Reflection, by page.
#[derive(Debug, Default)]
pub(crate) struct Disclosures(HashMap<String, Vec<Disclosure>>);

impl Disclosures {
    /// The confirmation for the source domain `source` in section `section`
    /// of page `path`.
    pub(crate) fn find(&self, path: &str, section: &str, source: &str) -> Option<&Disclosure> {
        self.0
            .get(path)?
            .iter()
            .find(|d| d.section == section && d.source == source)
    }

    /// The newest confirmation recorded on section `section` of `path`.
    pub(crate) fn newest(&self, path: &str, section: &str) -> Option<&Disclosure> {
        self.0
            .get(path)?
            .iter()
            .filter(|d| d.section == section)
            .max_by(|a, b| a.at.cmp(&b.at))
    }
}

/// Who confirms for `caller`: the account behind it, or the machine owner on
/// a local session. The anonymous caller and the open tier have nobody
/// behind them who could answer, so they confirm nothing.
pub(crate) fn confirmer(caller: &ReflectionCaller) -> Option<String> {
    if caller.open_tier {
        return None;
    }
    match &caller.scope {
        Scope::User { account, .. } => Some(account.clone()),
        Scope::Unrestricted => Some(crate::scope::OWNER_IDENTITY_NAME.to_string()),
        Scope::Anonymous => None,
    }
}

/// `names` as a person reads a list: "a", "a and b", "a, b and c".
fn listed(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// The question a person is asked before text from `sources` (local domain
/// names) goes onto page `path` of `page`. `accounts` names who reads the
/// page when the accounts rule is what the sources break, `repo` the GitHub
/// repository when the backup is. Plain words: the readers, that the text
/// becomes theirs to read, and that the source's name stays hidden.
fn question(
    page: &str,
    path: &str,
    sources: &[String],
    accounts: Option<&PageReaders>,
    repo: Option<&str>,
) -> String {
    let from = listed(sources);
    let mut readers = Vec::new();
    let mut why = Vec::new();
    if let Some(accounts) = accounts {
        readers.push(match accounts {
            PageReaders::Everyone => {
                "everyone who can open this instance, visitors without an account included"
                    .to_string()
            }
            PageReaders::Accounts(names) if names.len() == 1 => {
                format!("the one account that may read {page}")
            }
            PageReaders::Accounts(names) => {
                format!("the {} accounts that may read {page}", names.len())
            }
        });
        why.push(format!("Fewer people may read {from}."));
    }
    if let Some(repo) = repo {
        readers.push(format!(
            "the people who can read the GitHub repository {repo}, where this page is backed up"
        ));
        why.push(format!("{from} is not shared through GitHub."));
    }
    format!(
        "Share text from {from} on a page more people may read? The page {path} in {page} is read by {}. {} If you agree, the text this page draws from {from} becomes readable by all of them. The names of those sources stay hidden from readers who may not read {from}. Agree only if you mean to share it.",
        readers.join(", and by "),
        why.join(" ")
    )
}

impl Engine {
    /// The confirmations recorded on the Reflection of the domain `id`.
    pub(crate) async fn reflection_disclosures(&self, id: DomainId) -> Result<Disclosures> {
        let rows = self.store.lock().await.reflection_disclosures(id).await?;
        let mut out = HashMap::new();
        for (path, records) in rows {
            // A row this binary cannot read is no confirmation: the section
            // reads as too wide until a person confirms it again.
            match serde_json::from_str::<Vec<Disclosure>>(&records) {
                Ok(list) => {
                    out.insert(path, list);
                }
                Err(e) => tracing::warn!(path = %path, "a disclosure record was not read: {e}"),
            }
        }
        Ok(Disclosures(out))
    }

    /// Store `list` as the confirmations of page `path`, or drop them.
    async fn store_disclosures(&self, id: DomainId, path: &str, list: &[Disclosure]) -> Result<()> {
        let records = if list.is_empty() {
            None
        } else {
            Some(serde_json::to_string(list).map_err(|e| EngineError::Internal(e.to_string()))?)
        };
        self.store
            .lock()
            .await
            .set_reflection_disclosures(id, path, records.as_deref())
            .await?;
        Ok(())
    }

    /// The repository a page of `page` is backed up to, when it is connected
    /// to GitHub.
    fn page_repo(&self, page: &str) -> Option<String> {
        self.domain_entry(page)
            .ok()
            .and_then(|entry| entry.origin.map(|origin| origin.repo))
    }

    /// Who reads a page of `page` now, when this instance knows its
    /// accounts: what a confirmation is recorded for.
    async fn page_readers_now(&self, page: &str) -> Result<Option<PageReaders>> {
        match self.domain_access.get() {
            Some(access) => access
                .page_readers(page)
                .await
                .map(Some)
                .map_err(|e| EngineError::Internal(e.to_string())),
            None => Ok(None),
        }
    }

    /// Whether a section of a page of `page` may carry text drawn from the
    /// domain `source` (both local names), with `disclosure` the person's
    /// confirmation recorded on that section for that domain, if any.
    ///
    /// The rule alone first ([`Engine::readers_cover`]); then the
    /// confirmation, held against both halves of the rule as they stand now:
    /// the page must still be backed up to the repository it named, and
    /// nobody may read the page now who did not then and may not read the
    /// source ([`crate::scope::DomainAccess::readers_cover_for`]). A source
    /// whose readers narrowed since changes nothing: its text reached those
    /// people already. When nobody here knows who reads which domain, no
    /// confirmation covers anything. The one rule [`Engine::too_wide_pages`],
    /// the queue and every write share.
    pub(crate) async fn source_covered(
        &self,
        page: &str,
        source: &str,
        disclosure: Option<&Disclosure>,
    ) -> Result<bool> {
        if self.readers_cover(page, source).await? {
            return Ok(true);
        }
        let Some(disclosure) = disclosure else {
            return Ok(false);
        };
        if self.leaves_github(page, source)
            && (self.page_repo(page).is_none() || disclosure.repo != self.page_repo(page))
        {
            return Ok(false);
        }
        let Some(access) = self.domain_access.get() else {
            return Ok(self.readers_unknown().is_none());
        };
        let covered = match &disclosure.readers {
            Some(readers) => access.readers_cover_for(page, source, readers).await,
            None => access.readers_cover(page, source).await,
        };
        covered.map_err(|e| EngineError::Internal(e.to_string()))
    }

    /// The question for sources in `sources` (local names, sorted) on page
    /// `path` of `page`, naming each half of the rule they break now, with
    /// its basis: the question, who reads the page now and the repository
    /// it is backed up to, the very facts a confirmation records.
    pub(crate) async fn disclosure_ask_for(
        &self,
        page: &str,
        path: &str,
        sources: &[String],
    ) -> Result<DisclosureAsk> {
        let mut accounts_wide = false;
        let mut github_wide = false;
        for source in sources {
            github_wide |= self.leaves_github(page, source);
            if let Some(access) = self.domain_access.get() {
                accounts_wide |= !access
                    .readers_cover(page, source)
                    .await
                    .map_err(|e| EngineError::Internal(e.to_string()))?;
            }
        }
        let readers = self.page_readers_now(page).await?;
        let repo = self.page_repo(page);
        let question = question(
            page,
            path,
            sources,
            readers.as_ref().filter(|_| accounts_wide),
            repo.as_deref().filter(|_| github_wide),
        );
        let basis = sha256_hex(
            json!({ "question": question, "readers": readers, "repo": repo })
                .to_string()
                .as_bytes(),
        );
        Ok(DisclosureAsk { question, basis })
    }

    /// The local names, sorted, of the domains a write may carry only on a
    /// person's confirmation ([`super::sources::NamedSource::disclose`]).
    async fn disclosed_domains(
        &self,
        named: &HashMap<String, super::sources::NamedSource>,
    ) -> Vec<String> {
        let table = self.name_table_now().await;
        let mut sources: Vec<String> = named
            .values()
            .filter(|n| n.disclose)
            .filter_map(|n| n.domain.as_deref())
            .map(|stored| table.resolve(stored).unwrap_or(stored).to_string())
            .collect();
        sources.sort();
        sources.dedup();
        sources
    }

    /// Refuse a confirmed write whose question no longer stands: the
    /// person's yes (`basis`) was given for other readers, or another
    /// repository, than the page has now. Called under the page's lock.
    pub(super) async fn hold_disclosure_to_its_question(
        &self,
        page: &str,
        path: &str,
        named: &HashMap<String, super::sources::NamedSource>,
        basis: &str,
    ) -> Result<()> {
        let sources = self.disclosed_domains(named).await;
        if sources.is_empty() {
            return Ok(());
        }
        if self.disclosure_ask_for(page, path, &sources).await?.basis != basis {
            return Err(EngineError::Conflict(DISCLOSURE_CHANGED.into()));
        }
        Ok(())
    }

    /// The question a person is asked before this write lands, when it
    /// would be refused for reach alone: it names sources in a domain fewer
    /// people may read than the page's, or one not shared through GitHub
    /// while the page is backed up there. `None` when nothing in the write
    /// needs a person's confirmation, and when `caller` has nobody behind it
    /// who could give one (the write is then refused as before).
    ///
    /// Every other refusal comes first, as the write would give it: the
    /// right to curate the page, a source in a domain the caller may not
    /// open (refused as an unknown domain, so a question never names one),
    /// and a stack that does not know who reads which domain, which is never
    /// asked about. So a question is only put about a write that can land.
    pub async fn reflection_disclosure_question(
        &self,
        p: &WriteReflectionParams,
        caller: &ReflectionCaller,
    ) -> Result<Option<DisclosureAsk>> {
        if p.sources.is_empty() || p.action.as_deref().is_some_and(|action| action != "write") {
            return Ok(None);
        }
        if confirmer(caller).is_none() {
            return Ok(None);
        }
        self.require_reflection_to_write(&p.domain, caller, ReflectionRight::Curate)
            .await?;
        r::validate_reflection_path(&p.path).map_err(EngineError::Invalid)?;
        let named = self
            .resolve_source_names(&p.domain, &caller.scope, p.sources.values().flatten(), true)
            .await?;
        let sources = self.disclosed_domains(&named).await;
        if sources.is_empty() {
            return Ok(None);
        }
        self.disclosure_ask_for(&p.domain, &p.path, &sources)
            .await
            .map(Some)
    }

    /// Record, after a write of page `path` landed with `text`, the
    /// confirmations it carries (`confirmed`: section id and stored source
    /// domain, by `by`), and drop every recorded one whose source the page
    /// no longer names in that section. Called under the page's lock.
    pub(super) async fn record_disclosures(
        &self,
        domain: &str,
        id: DomainId,
        path: &str,
        text: &str,
        confirmed: &[(String, String)],
        by: Option<&str>,
    ) -> Result<()> {
        let mut list = self
            .reflection_disclosures(id)
            .await?
            .0
            .remove(path)
            .unwrap_or_default();
        let before = list.clone();
        let prov = r::read_provenance(text);
        let named = |section: &str, source: &str| {
            prov.get(section).is_some_and(|record| {
                record
                    .sources
                    .iter()
                    .any(|src| src.domain.as_deref() == Some(source))
            })
        };
        list.retain(|d| named(&d.section, &d.source));
        if let (Some(by), false) = (by, confirmed.is_empty()) {
            let readers = self.page_readers_now(domain).await?;
            let repo = self.page_repo(domain);
            let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            for (section, source) in confirmed {
                list.retain(|d| !(d.section == *section && d.source == *source));
                list.push(Disclosure {
                    section: section.clone(),
                    source: source.clone(),
                    by: by.to_string(),
                    at: at.clone(),
                    readers: readers.clone(),
                    repo: repo.clone(),
                });
            }
        }
        if list != before {
            self.store_disclosures(id, path, &list).await?;
        }
        Ok(())
    }

    /// Carry the confirmations of page `from` to `to`, for a page move.
    pub(super) async fn move_disclosures(&self, id: DomainId, from: &str, to: &str) -> Result<()> {
        let mut all = self.reflection_disclosures(id).await?;
        let Some(list) = all.0.remove(from) else {
            return Ok(());
        };
        self.store_disclosures(id, to, &list).await?;
        self.store_disclosures(id, from, &[]).await
    }

    /// Drop the confirmations of page `path`, for a page delete.
    pub(super) async fn drop_disclosures(&self, id: DomainId, path: &str) -> Result<()> {
        self.store_disclosures(id, path, &[]).await
    }

    /// A person's confirmation of a section the queue names as
    /// `source_too_wide`, from Fluid's Review: every source of the section
    /// in another domain that the page may not carry now is shared on
    /// purpose with the page's readers of this moment.
    ///
    /// The caller needs a person behind it, the right to curate the page and
    /// the right to read every source's domain (one it may not open is
    /// refused without being named). `basis` is the item's: when the page's
    /// readers or repository changed since the item was read, the question
    /// the person answered no longer stands and the confirmation is refused
    /// ([`DISCLOSURE_CHANGED`]). Nothing is asked when nobody here knows
    /// who reads which domain. Answers `{domain, path, section, disclosed}`,
    /// `disclosed` `{by, at}`, or null when there was nothing to confirm.
    pub async fn confirm_reflection_disclosure(
        &self,
        domain: &str,
        path: &str,
        section: &str,
        basis: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let by = confirmer(caller).ok_or_else(|| {
            EngineError::Forbidden(
                "sharing text on purpose needs a person signed in who may read its source".into(),
            )
        })?;
        let access = self
            .require_reflection_to_write(domain, caller, ReflectionRight::Curate)
            .await?;
        self.refuse_read_only_domain(domain, &Scope::Unrestricted)
            .await?;
        r::validate_reflection_path(path).map_err(EngineError::Invalid)?;
        if let Some(why) = self.readers_unknown() {
            return Err(EngineError::Invalid(format!(
                "{UNKNOWN_READERS_REFUSAL} ({why})"
            )));
        }
        let abs = access.abs(path);
        let _lock = self.lock_file(&abs).await;
        let text = read_opt(&abs)?.ok_or_else(|| {
            EngineError::NotFound(format!("no reflection '{path}' in domain '{domain}'"))
        })?;
        let prov = r::read_provenance(&text);
        let record = prov.get(section).ok_or_else(|| {
            EngineError::NotFound(format!("no section '{section}' with sources in '{path}'"))
        })?;
        let table = self.name_table_now().await;
        let hidden = self.hidden_for(&caller.scope).await?;
        let recorded = self.reflection_disclosures(access.domain_id).await?;
        let mut wide: Vec<String> = Vec::new();
        let mut wide_local: Vec<String> = Vec::new();
        for src in &record.sources {
            let Some(stored) = src.domain.as_deref() else {
                continue;
            };
            let Some(local) = self.checkable_domain(&table, stored) else {
                continue;
            };
            if hidden.contains(&local) {
                return Err(EngineError::Forbidden(
                    "only a person who may read every source of this section can share it on purpose"
                        .into(),
                ));
            }
            if local == domain {
                continue;
            }
            let existing = recorded.find(path, section, stored);
            if !self.source_covered(domain, &local, existing).await? {
                if !wide.iter().any(|s| s == stored) {
                    wide.push(stored.to_string());
                }
                if !wide_local.contains(&local) {
                    wide_local.push(local);
                }
            }
        }
        if wide.is_empty() {
            return Ok(json!({
                "domain": domain,
                "path": path,
                "section": section,
                "disclosed": Value::Null,
            }));
        }
        // The person answered the question the queue showed them; it must
        // still be the question for this page now.
        wide_local.sort();
        if self
            .disclosure_ask_for(domain, path, &wide_local)
            .await?
            .basis
            != basis
        {
            return Err(EngineError::Conflict(DISCLOSURE_CHANGED.into()));
        }
        let confirmed: Vec<(String, String)> = wide
            .into_iter()
            .map(|source| (section.to_string(), source))
            .collect();
        self.record_disclosures(domain, access.domain_id, path, &text, &confirmed, Some(&by))
            .await?;
        // The page may go to the backup branch now.
        self.mark_reflection_dirty(domain, crate::reflection_backup::BackupCause::Settled);
        let recorded = self.reflection_disclosures(access.domain_id).await?;
        let newest = recorded.newest(path, section);
        Ok(json!({
            "domain": domain,
            "path": path,
            "section": section,
            "disclosed": newest.map(|d| json!({ "by": d.by, "at": d.at })),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_list_reads_as_a_person_writes_it() {
        let names = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(listed(&names(&["a"])), "a");
        assert_eq!(listed(&names(&["a", "b"])), "a and b");
        assert_eq!(listed(&names(&["a", "b", "c"])), "a, b and c");
    }

    #[test]
    fn the_question_names_both_halves_when_both_apply() {
        let text = question(
            "eng",
            "overview.md",
            &["harbor".to_string()],
            Some(&PageReaders::Everyone),
            Some("acme/kb"),
        );
        assert!(
            text.contains("everyone who can open this instance"),
            "{text}"
        );
        assert!(
            text.contains(", and by the people who can read the GitHub repository acme/kb"),
            "{text}"
        );
        assert!(
            text.contains("Fewer people may read harbor. harbor is not shared through GitHub."),
            "{text}"
        );
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }
}
