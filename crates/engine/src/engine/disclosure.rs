//! A person shares text from a domain fewer people may read on a Reflection
//! page, on purpose.
//!
//! The reach rule ([`Engine::readers_cover`], [`Engine::leaves_repository`])
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

use super::sources::{RepositoryReach, UNKNOWN_READERS_REFUSAL};
use super::*;
use crate::scope::{PageReaders, ReflectionCaller, ReflectionRight, Scope};

/// What a confirmed write is refused with when the page's readers or the
/// repository it is backed up to changed between the question and the
/// answer: the person agreed to the readers they were shown.
pub const DISCLOSURE_CHANGED: &str = "who may read this page, or where it is backed up, changed since the person was asked; ask them again";

/// What a confirmed write or a confirmation from Fluid is refused with while
/// a GitHub App decides who reads the page's repository and nobody knows
/// that right now (GitHub down past the stale window, the App not installed
/// there): a confirmation is recorded for those readers, so none can be
/// recorded.
pub const REPO_READERS_UNKNOWN: &str =
    "who reads this page's GitHub repository is not known right now; try again later";

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

/// A question with the readers of the page's repository it was asked
/// about: what a confirmation that answers it records, so the record holds
/// the very readers the person agreed to and not a later read.
#[derive(Debug, Clone)]
pub(crate) struct Asked {
    pub ask: DisclosureAsk,
    pub repo_readers: Option<RepositoryReaders>,
}

/// What a write would put on a page, worked out without writing it: the page
/// text, and each section and stored source domain that needs a person's
/// confirmation (a source the reach rule refuses and the writer has not
/// shared on this page yet).
#[derive(Debug, Clone)]
pub(crate) struct WritePreview {
    pub text: String,
    pub needs: Vec<(String, String)>,
}

/// What a refused write adds when nobody could be asked: how a person can
/// still share the text on purpose. It never points at Fluid, where a
/// refused write leaves nothing to confirm.
pub const NOBODY_ASKED_NOTE: &str = "a person who may read that domain can share it on purpose by making this write from an MCP client that can put a question to them; an unattended run never can";

/// One person's confirmation that a page may carry text from the engrams of
/// one other domain. It counts once per person, page and source domain: the
/// person's later edits of the page that draw on the same domain need no new
/// question, another person who cites it on the page confirms for
/// themselves, and it holds until the page's readers widen or its backup
/// target changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Disclosure {
    /// The source's domain, by the name the page's sources store.
    pub source: String,
    /// Who confirmed: the account, or the machine owner.
    pub by: String,
    /// When, RFC 3339.
    pub at: String,
    /// Who read the page then, when this instance knows its accounts. A
    /// later reader who was not among them and may not read the source
    /// makes the page too wide again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readers: Option<PageReaders>,
    /// The repository the page was backed up to then, when it was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// Who read that repository on GitHub then, when a GitHub App knew it.
    /// A later reader of the repository who was not among them and may not
    /// read the source's repository makes the page too wide again. Under an
    /// App a confirmation without it (recorded before it was kept, or while
    /// the listing was not known) holds for nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_readers: Option<RepositoryReaders>,
}

/// Who read a page's repository on GitHub when a person confirmed: whether
/// it was public or internal, and every GitHub id its listing named, sorted
/// (the page side's count, see [`super::sources::repository_covers`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RepositoryReaders {
    pub public: bool,
    /// Internal (GitHub Enterprise): read by the enterprise's members too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub internal: bool,
    pub github_ids: Vec<u64>,
}

impl RepositoryReaders {
    fn of(readers: &RepoReaders) -> RepositoryReaders {
        let mut github_ids: Vec<u64> = readers.listed_ids.iter().copied().collect();
        github_ids.sort_unstable();
        RepositoryReaders {
            public: readers.public,
            internal: readers.internal,
            github_ids,
        }
    }
}

/// Whether nobody reads the page's repository now (`now`) who did not
/// when a person confirmed (`confirmed`) and may not read the source's
/// repository (`source`, `None` when the source is not on GitHub or its
/// readers are not known). A repository that went public since is read by
/// everybody, whom nobody confirmed, unless the source's is public too; one
/// that went internal since is read by its enterprise, whom nobody
/// confirmed either.
fn repository_widened_by_nobody(
    confirmed: &RepositoryReaders,
    now: &RepoReaders,
    source: Option<&RepoReaders>,
) -> bool {
    if confirmed.public || source.is_some_and(|source| source.public) {
        return true;
    }
    !now.public
        && (!now.internal || confirmed.internal)
        && now.listed_ids.iter().all(|id| {
            confirmed.github_ids.binary_search(id).is_ok()
                || source.is_some_and(|source| source.github_ids.contains(id))
        })
}

/// The confirmations recorded on one domain's Reflection, by page.
#[derive(Debug, Default)]
pub(crate) struct Disclosures(HashMap<String, Vec<Disclosure>>);

impl Disclosures {
    /// Every confirmation recorded on page `path` for the source domain
    /// `source` (a stored name), one per person.
    pub(crate) fn of(&self, path: &str, source: &str) -> Vec<&Disclosure> {
        self.0
            .get(path)
            .map(|list| list.iter().filter(|d| d.source == source).collect())
            .unwrap_or_default()
    }

    /// The newest confirmation recorded on page `path` for any of `sources`
    /// (stored names): what a section citing them shows its curators.
    pub(crate) fn newest(&self, path: &str, sources: &[&str]) -> Option<&Disclosure> {
        self.0
            .get(path)?
            .iter()
            .filter(|d| sources.contains(&d.source.as_str()))
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
        Scope::Anonymous | Scope::Agent { .. } => None,
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

/// How much of a section's text a question shows.
const EXCERPT_CHARS: usize = 400;

/// A section's text as a question shows it: on one line, cut after
/// [`EXCERPT_CHARS`] characters.
fn excerpt(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= EXCERPT_CHARS {
        return flat;
    }
    let cut: String = flat.chars().take(EXCERPT_CHARS).collect();
    format!("{cut} ...")
}

/// The text of section `id` of `page` (heading and body, no id line).
pub(crate) fn section_text(page: &str, id: &str) -> Option<String> {
    r::split_sections(page)
        .into_iter()
        .find(|s| s.id.as_deref() == Some(id))
        .map(|s| s.text)
}

/// Why the backup half of the rule refuses a page's sources: some source is
/// not on GitHub, and some is on a repository not every reader of the
/// page's repository reads (or whose readers are not known right now).
#[derive(Debug, Clone, Copy, Default)]
struct BackupGap {
    not_on_github: bool,
    narrower_repository: bool,
}

/// The question a person is asked before `texts` (section texts) drawn from
/// `sources` (local domain names) go onto page `path` of `page`. `accounts`
/// names who reads the page when the accounts rule is what the sources
/// break, `repo` the GitHub repository when the backup is, with why. Plain
/// words: the readers, the text itself, that it becomes theirs to read, and
/// that the source's name stays hidden.
fn question(
    page: &str,
    path: &str,
    sources: &[String],
    accounts: Option<&PageReaders>,
    repo: Option<(&str, BackupGap)>,
    texts: &[String],
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
    if let Some((repo, gap)) = repo {
        readers.push(format!(
            "the people who can read the GitHub repository {repo}, where this page is backed up"
        ));
        if gap.not_on_github {
            why.push(format!("{from} is not shared through GitHub."));
        }
        if gap.narrower_repository {
            why.push(format!(
                "Not everybody who can read {repo} may read the GitHub repository of {from}."
            ));
        }
    }
    let shown: Vec<String> = texts
        .iter()
        .map(|text| format!("The text to share: \"{}\"", excerpt(text)))
        .collect();
    format!(
        "Share text from {from} on a page more people may read? The page {path} in {page} is read by {}. {} {}. If you agree, this text becomes readable by all of them. The names of the sources stay hidden from readers who may not read {from}. Agree only if you mean to share it.",
        readers.join(", and by "),
        why.join(" "),
        shown.join(". ")
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
        self.repository_of(page)
    }

    /// Who reads the repository a page of `page` is backed up to now, when
    /// a GitHub App knows it: what a confirmation is recorded for, beside
    /// the repository's name.
    async fn page_repository_readers_now(&self, page: &str) -> Option<RepositoryReaders> {
        if !self.on_github(page) {
            return None;
        }
        let RepositoryReach::Known(governor) = self.repository_reach() else {
            return None;
        };
        governor
            .repository_readers(page)
            .await
            .map(|readers| RepositoryReaders::of(&readers))
    }

    /// Whether `disclosure` still holds for the backup half of the rule: the
    /// page is still backed up to the repository it named and, under a
    /// GitHub App, nobody reads that repository now who did not then and
    /// may not read the source's repository. Without an App the repository's
    /// name is all that is compared, as the rule compares nothing more.
    async fn backup_still_confirmed(
        &self,
        page: &str,
        source: &str,
        disclosure: &Disclosure,
    ) -> bool {
        let repo = self.page_repo(page);
        if repo.is_none() || disclosure.repo != repo {
            return false;
        }
        let governor = match self.repository_reach() {
            RepositoryReach::NoApp => return true,
            RepositoryReach::Unknown => return false,
            RepositoryReach::Known(governor) => governor,
        };
        let Some(confirmed) = &disclosure.repo_readers else {
            return false;
        };
        let Some(now) = governor.repository_readers(page).await else {
            return false;
        };
        let source_readers = if self.on_github(source) {
            governor.repository_readers(source).await
        } else {
            None
        };
        repository_widened_by_nobody(confirmed, &now, source_readers.as_ref())
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

    /// Whether a page of `page` may carry text drawn from the domain `source`
    /// (both local names), with `disclosure` a person's confirmation recorded
    /// on that page for that domain, if any.
    ///
    /// The rule alone first ([`Engine::readers_cover`]); then the
    /// confirmation, held against both halves of the rule as they stand now:
    /// the page must still be backed up to the repository it named, with no
    /// reader of that repository on GitHub who was not one then and may not
    /// read the source's repository, and nobody may read the page here now
    /// who did not then and may not read the source
    /// ([`crate::scope::DomainAccess::readers_cover_for`]). A source
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
        if self.leaves_repository(page, source).await?
            && !self.backup_still_confirmed(page, source, disclosure).await
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

    /// Whether page `path` of `page` may carry text from `source` (local) as
    /// its pages store it (`stored`): the rule alone, or any person's
    /// confirmation on the page for that domain that still holds.
    pub(crate) async fn source_covered_on_page(
        &self,
        page: &str,
        path: &str,
        source: &str,
        stored: &str,
        recorded: &Disclosures,
    ) -> Result<bool> {
        if self.readers_cover(page, source).await? {
            return Ok(true);
        }
        for disclosure in recorded.of(path, stored) {
            if self.source_covered(page, source, Some(disclosure)).await? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The source domains (local names) `by` shared on page `path` of
    /// `page` and that still hold: what that person's later writes of the
    /// page may cite without a new question. Empty for an unattended request,
    /// which never confirms anything.
    pub(super) async fn shared_by(
        &self,
        page: &str,
        id: DomainId,
        path: &str,
        by: &str,
    ) -> Result<HashSet<String>> {
        if crate::read_limit::current().is_some() {
            return Ok(HashSet::new());
        }
        let recorded = self.reflection_disclosures(id).await?;
        let table = self.name_table_now().await;
        let mut out = HashSet::new();
        for d in recorded.0.get(path).into_iter().flatten() {
            if d.by != by {
                continue;
            }
            let Some(local) = self.checkable_domain(&table, &d.source) else {
                continue;
            };
            if self.source_covered(page, &local, Some(d)).await? {
                out.insert(local);
            }
        }
        Ok(out)
    }

    /// The question for `texts` (section texts, shown) drawn from `sources`
    /// (local names, sorted) on page `path` of `page`, naming each half of the
    /// rule they break now, with its basis: the page, the sources, who reads
    /// the page now and the repository it is backed up to, the very facts a
    /// confirmation records. The texts are shown and not bound: one
    /// confirmation covers the person's later edits of the page.
    pub(crate) async fn disclosure_ask_for(
        &self,
        page: &str,
        path: &str,
        sources: &[String],
        texts: &[String],
    ) -> Result<DisclosureAsk> {
        Ok(self.disclosure_asked(page, path, sources, texts).await?.ask)
    }

    /// [`Engine::disclosure_ask_for`] with the readers of the page's
    /// repository the question was worked out for.
    pub(crate) async fn disclosure_asked(
        &self,
        page: &str,
        path: &str,
        sources: &[String],
        texts: &[String],
    ) -> Result<Asked> {
        let mut accounts_wide = false;
        let mut gap = BackupGap::default();
        for source in sources {
            if self.on_github(page) && !self.on_github(source) {
                gap.not_on_github = true;
            } else if self.leaves_repository(page, source).await? {
                gap.narrower_repository = true;
            }
            if let Some(access) = self.domain_access.get() {
                accounts_wide |= !access
                    .readers_cover(page, source)
                    .await
                    .map_err(|e| EngineError::Internal(e.to_string()))?;
            }
        }
        let readers = self.page_readers_now(page).await?;
        let repo = self.page_repo(page);
        let repo_readers = self.page_repository_readers_now(page).await;
        let question = question(
            page,
            path,
            sources,
            readers.as_ref().filter(|_| accounts_wide),
            repo.as_deref()
                .filter(|_| gap.not_on_github || gap.narrower_repository)
                .map(|repo| (repo, gap)),
            texts,
        );
        let basis = sha256_hex(
            json!({
                "page": page,
                "path": path,
                "sources": sources,
                "readers": readers,
                "repo": repo,
                "repo_readers": repo_readers,
            })
            .to_string()
            .as_bytes(),
        );
        Ok(Asked {
            ask: DisclosureAsk { question, basis },
            repo_readers,
        })
    }

    /// Refuses recording a confirmation on a page of `page` for
    /// `repo_readers` (the readers of its repository the question was asked
    /// about) when a GitHub App decides who reads that repository and they
    /// are not known ([`REPO_READERS_UNKNOWN`]): such a record would hold
    /// for nothing, and the write would only be held back.
    pub(crate) fn refuse_unknown_repository_readers(
        &self,
        page: &str,
        repo_readers: Option<&RepositoryReaders>,
    ) -> Result<()> {
        let governed =
            self.on_github(page) && !matches!(self.repository_reach(), RepositoryReach::NoApp);
        if governed && repo_readers.is_none() {
            return Err(EngineError::Invalid(REPO_READERS_UNKNOWN.into()));
        }
        Ok(())
    }

    /// The question for what `preview` needs confirmed: the local names of
    /// its source domains and the texts of its sections as the write leaves
    /// them, with the readers of the page's repository the question was
    /// worked out for.
    pub(super) async fn asked_for_preview(
        &self,
        page: &str,
        path: &str,
        preview: &WritePreview,
    ) -> Result<Asked> {
        let table = self.name_table_now().await;
        let mut sources: Vec<String> = preview
            .needs
            .iter()
            .map(|(_, stored)| table.resolve(stored).unwrap_or(stored).to_string())
            .collect();
        sources.sort();
        sources.dedup();
        let mut sections: Vec<&str> = preview.needs.iter().map(|(id, _)| id.as_str()).collect();
        sections.dedup();
        let texts: Vec<String> = sections
            .into_iter()
            .filter_map(|id| section_text(&preview.text, id))
            .collect();
        self.disclosure_asked(page, path, &sources, &texts).await
    }

    /// The question a person is asked before this write lands, when it
    /// needs one: it names sources in a domain fewer people may read than
    /// the page's, or one not shared through GitHub while the page is backed
    /// up there, that this person has not shared on this page yet (or whose
    /// sharing no longer holds). The question shows the text the write
    /// leaves in those sections. `None` when nothing in
    /// the write needs a person's confirmation, and when `caller` has nobody
    /// behind it who could give one (the write is then refused as before).
    ///
    /// Every other refusal comes first, as the write would give it: it is
    /// worked out by the write itself, stopped before anything is written.
    /// So a question is only put about a write that can land.
    pub async fn reflection_disclosure_question(
        &self,
        p: &WriteReflectionParams,
        caller: &ReflectionCaller,
    ) -> Result<Option<DisclosureAsk>> {
        if p.action.as_deref().is_some_and(|action| action != "write") {
            return Ok(None);
        }
        if confirmer(caller).is_none() {
            return Ok(None);
        }
        if p.sources.is_empty() {
            return Ok(None);
        }
        let Some(preview) = self.preview_reflection_write(p, caller).await? else {
            return Ok(None);
        };
        if preview.needs.is_empty() {
            return Ok(None);
        }
        let asked = self.asked_for_preview(&p.domain, &p.path, &preview).await?;
        // A yes could not be recorded: no question is put.
        self.refuse_unknown_repository_readers(&p.domain, asked.repo_readers.as_ref())?;
        Ok(Some(asked.ask))
    }

    /// Record, after a write of page `path` landed with `text`, the
    /// confirmations it carries (`confirmed`: stored source domains, by
    /// `by`), one per person and domain, and drop every recorded one whose
    /// domain the page no longer cites anywhere. `repo_readers` are the
    /// readers of the page's repository the person's question named (the
    /// basis check read them), never a later read. Called under the page's
    /// lock.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn record_disclosures(
        &self,
        domain: &str,
        id: DomainId,
        path: &str,
        text: &str,
        confirmed: &[String],
        by: Option<&str>,
        repo_readers: Option<RepositoryReaders>,
    ) -> Result<()> {
        let mut list = self
            .reflection_disclosures(id)
            .await?
            .0
            .remove(path)
            .unwrap_or_default();
        let before = list.clone();
        let cited: HashSet<String> = r::read_provenance(text)
            .into_values()
            .flat_map(|record| record.sources.into_iter().filter_map(|src| src.domain))
            .collect();
        list.retain(|d| cited.contains(&d.source));
        if let (Some(by), false) = (by, confirmed.is_empty()) {
            let readers = self.page_readers_now(domain).await?;
            let repo = self.page_repo(domain);
            let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            for source in confirmed {
                list.retain(|d| !(d.source == *source && d.by == by));
                list.push(Disclosure {
                    source: source.clone(),
                    by: by.to_string(),
                    at: at.clone(),
                    readers: readers.clone(),
                    repo: repo.clone(),
                    repo_readers: repo_readers.clone(),
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
    /// `source_too_wide`, from Fluid's Review: every source domain of the
    /// section that the page may not carry now is shared on purpose by this
    /// person on this page, with the page's readers of this moment.
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
        let current = section_text(&text, section);
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
            if !self
                .source_covered_on_page(domain, path, &local, stored, &recorded)
                .await?
            {
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
        let texts: Vec<String> = current.into_iter().collect();
        let asked = self
            .disclosure_asked(domain, path, &wide_local, &texts)
            .await?;
        self.refuse_unknown_repository_readers(domain, asked.repo_readers.as_ref())?;
        if asked.ask.basis != basis {
            return Err(EngineError::Conflict(DISCLOSURE_CHANGED.into()));
        }
        self.record_disclosures(
            domain,
            access.domain_id,
            path,
            &text,
            &wide,
            Some(&by),
            asked.repo_readers,
        )
        .await?;
        // The page may go to the backup branch now.
        self.mark_reflection_dirty(domain, crate::reflection_backup::BackupCause::Settled);
        let recorded = self.reflection_disclosures(access.domain_id).await?;
        let cited: Vec<&str> = record
            .sources
            .iter()
            .filter_map(|src| src.domain.as_deref())
            .collect();
        let newest = recorded.newest(path, &cited);
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
            Some((
                "acme/kb",
                BackupGap {
                    not_on_github: true,
                    narrower_repository: false,
                },
            )),
            &["## Team\n\nFive people.".to_string()],
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
        assert!(
            text.contains("The text to share: \"## Team Five people.\""),
            "{text}"
        );
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'));
    }

    #[test]
    fn the_question_says_when_the_source_repository_has_fewer_readers() {
        let text = question(
            "eng",
            "overview.md",
            &["vault".to_string()],
            None,
            Some((
                "acme/kb",
                BackupGap {
                    not_on_github: false,
                    narrower_repository: true,
                },
            )),
            &["## Team\n\nFive people.".to_string()],
        );
        assert!(
            text.contains(
                "Not everybody who can read acme/kb may read the GitHub repository of vault."
            ),
            "{text}"
        );
        assert!(!text.contains("not shared through GitHub"), "{text}");
    }

    fn repo(public: bool, ids: &[u64]) -> RepoReaders {
        RepoReaders {
            public,
            internal: false,
            github_ids: ids.iter().copied().collect(),
            listed_ids: ids.iter().copied().collect(),
        }
    }

    #[test]
    fn an_internal_page_repository_covers_only_a_public_source() {
        use super::super::sources::repository_covers;
        let internal = |ids: &[u64]| RepoReaders {
            internal: true,
            ..repo(false, ids)
        };
        // Its enterprise's members read the page and are in no listing.
        assert!(!repository_covers(&internal(&[1]), &repo(false, &[1, 2])));
        assert!(!repository_covers(&internal(&[1]), &internal(&[1, 2])));
        assert!(repository_covers(&internal(&[1]), &repo(true, &[])));
        // As a source it counts its listed collaborators only.
        assert!(repository_covers(&repo(false, &[1]), &internal(&[1, 2])));
    }

    #[test]
    fn a_repository_covers_another_only_when_it_has_no_reader_the_other_lacks() {
        use super::super::sources::repository_covers;
        assert!(repository_covers(
            &repo(false, &[1, 2]),
            &repo(false, &[1, 2, 3])
        ));
        assert!(!repository_covers(
            &repo(false, &[1, 2]),
            &repo(false, &[1])
        ));
        assert!(repository_covers(&repo(true, &[]), &repo(true, &[])));
        assert!(repository_covers(&repo(false, &[9]), &repo(true, &[])));
        assert!(!repository_covers(&repo(true, &[1]), &repo(false, &[1])));
    }

    #[test]
    fn a_confirmation_holds_until_a_reader_who_lacks_the_source_joins() {
        let confirmed = RepositoryReaders::of(&repo(false, &[2, 1]));
        assert_eq!(confirmed.github_ids, vec![1, 2]);
        let source = repo(false, &[1, 4]);
        assert!(repository_widened_by_nobody(
            &confirmed,
            &repo(false, &[1, 2]),
            Some(&source)
        ));
        // Fewer readers or a new one who reads the source: still holds.
        assert!(repository_widened_by_nobody(
            &confirmed,
            &repo(false, &[1]),
            Some(&source)
        ));
        assert!(repository_widened_by_nobody(
            &confirmed,
            &repo(false, &[1, 2, 4]),
            Some(&source)
        ));
        // A new reader who does not, or a source not on GitHub.
        assert!(!repository_widened_by_nobody(
            &confirmed,
            &repo(false, &[1, 2, 3]),
            Some(&source)
        ));
        assert!(!repository_widened_by_nobody(
            &confirmed,
            &repo(false, &[1, 2, 4]),
            None
        ));
        // Made public since: everybody, whom nobody confirmed.
        assert!(!repository_widened_by_nobody(
            &confirmed,
            &repo(true, &[1, 2]),
            Some(&source)
        ));
        assert!(repository_widened_by_nobody(
            &confirmed,
            &repo(true, &[1]),
            Some(&repo(true, &[]))
        ));
        // Made internal since: its enterprise, whom nobody confirmed.
        let internal = RepoReaders {
            internal: true,
            ..repo(false, &[1, 2])
        };
        assert!(!repository_widened_by_nobody(
            &confirmed,
            &internal,
            Some(&source)
        ));
        let was_internal = RepositoryReaders::of(&internal);
        assert!(repository_widened_by_nobody(&was_internal, &internal, None));
        let public = RepositoryReaders::of(&repo(true, &[]));
        assert!(repository_widened_by_nobody(
            &public,
            &repo(true, &[7]),
            None
        ));
    }
}
