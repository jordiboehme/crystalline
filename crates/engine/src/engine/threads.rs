//! `discuss`: threads on a passage of a document, stored where the
//! document's kind keeps them. The verbs are target neutral: the right, the
//! thread file, the lock, the section lookup and the anchor check come from
//! the `ThreadTarget`, whose one variant is a Reflection page.

use crystalline_core::reflection as r;
use crystalline_core::threads::{
    self as th, Entry, EntryKind, SectionRef, Thread, ThreadError, ThreadStatus, ThreadTarget,
};

use super::fence::{DiskUndo, KeepText, OnRefusal};
use super::reflection::{ReflectionAccess, thread_view};
use super::*;
use crate::reflection_backup::BackupCause;
use crate::scope::{ReflectionCaller, ReflectionRight};

/// The access a target's right check answers.
enum TargetAccess {
    Reflection(ReflectionAccess),
}

/// Whether an agent's thread text links to engrams, which the people who
/// read threads may not be able to see.
fn links_to_engrams(text: &str) -> bool {
    text.contains("[[") || text.contains(crystalline_core::address::SCHEME)
}

const PLAIN_WORDS: &str = "thread text is read by people who may not see engrams: say it in plain words, without engram links ([[...]] or crystalline:// addresses)";

/// Reading and commenting are one level, with one exception that comes from
/// being anonymous, not from a level: the anonymous visitor of a public
/// domain reads threads and never writes one.
///
/// On a domain GitHub governs the open tier comments only as the account
/// `github.agent_identity` names ([`Engine::open_tier_may_comment`](super::Engine::open_tier_may_comment)).
fn refuse_anonymous_comment(
    engine: &Engine,
    domain: &str,
    caller: &ReflectionCaller,
) -> Result<()> {
    if engine.open_tier_may_comment(caller, domain) {
        return Ok(());
    }
    Err(EngineError::Forbidden(
        super::reflection::ANONYMOUS_COMMENT.to_string(),
    ))
}

/// One entry by `caller`, now.
fn entry(kind: EntryKind, caller: &ReflectionCaller, text: &str) -> Entry {
    Entry {
        at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        kind,
        author: caller.author.clone(),
        text: text.to_string(),
    }
}

/// A thread-file failure as the engine answers it: an unknown thread is not
/// found (404), a refused entry is a malformed request.
fn thread_error(e: ThreadError) -> EngineError {
    match e {
        ThreadError::NoThread(_) => EngineError::NotFound(e.to_string()),
        ThreadError::Refused(why) => EngineError::Invalid(why),
    }
}

impl Engine {
    /// The target's right check, at `need`.
    async fn target_access(
        &self,
        domain: &str,
        target: &ThreadTarget,
        caller: &ReflectionCaller,
        need: ReflectionRight,
    ) -> Result<TargetAccess> {
        match target {
            ThreadTarget::Reflection { path } => {
                let access = self
                    .require_reflection_to_write(domain, caller, need)
                    .await?;
                r::validate_reflection_path(path).map_err(EngineError::Invalid)?;
                Ok(TargetAccess::Reflection(access))
            }
        }
    }

    /// The document's own file and its thread file. The document's file is
    /// the one whose lock every write to either takes.
    fn target_files(target: &ThreadTarget, access: &TargetAccess) -> (PathBuf, PathBuf) {
        match (target, access) {
            (ThreadTarget::Reflection { path }, TargetAccess::Reflection(access)) => {
                (access.abs(path), access.abs(&r::threads_path(path)))
            }
        }
    }

    /// `discuss`: dispatch on the action.
    pub async fn discuss(&self, p: &DiscussParams, caller: &ReflectionCaller) -> Result<Value> {
        let path = || {
            p.path.as_deref().ok_or_else(|| {
                EngineError::Invalid("this action needs `path`, the reflection".into())
            })
        };
        let thread = || {
            p.thread.as_deref().ok_or_else(|| {
                EngineError::Invalid("this action needs `thread`, the thread id".into())
            })
        };
        let text = p.text.as_deref().unwrap_or("");
        match p.action.as_deref().unwrap_or("open") {
            "open" => self.open_reflection_thread(p, caller).await,
            "answer" => {
                self.reflection_thread_entry(
                    &p.domain,
                    path()?,
                    thread()?,
                    EntryKind::Comment,
                    text,
                    caller,
                )
                .await
            }
            "resolve" => {
                self.reflection_thread_entry(
                    &p.domain,
                    path()?,
                    thread()?,
                    EntryKind::Resolve,
                    text,
                    caller,
                )
                .await
            }
            "reopen" => {
                self.reflection_thread_entry(
                    &p.domain,
                    path()?,
                    thread()?,
                    EntryKind::Reopen,
                    text,
                    caller,
                )
                .await
            }
            "list" => {
                self.list_reflection_threads(
                    &p.domain,
                    p.path.as_deref(),
                    p.status.as_deref().unwrap_or("open"),
                    caller,
                )
                .await
            }
            other => Err(EngineError::Invalid(format!(
                "`{other}` is not a discuss action; use open, answer, resolve, reopen or list"
            ))),
        }
    }

    /// Open a thread on a Reflection page.
    pub async fn open_reflection_thread(
        &self,
        p: &DiscussParams,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let path = p
            .path
            .clone()
            .ok_or_else(|| EngineError::Invalid("open needs `path`, the reflection".into()))?;
        self.open_thread(&p.domain, &ThreadTarget::Reflection { path }, p, caller)
            .await
    }

    /// Add an entry to a thread on a Reflection page.
    pub async fn reflection_thread_entry(
        &self,
        domain: &str,
        path: &str,
        id: &str,
        kind: EntryKind,
        text: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        self.thread_entry(
            domain,
            &ThreadTarget::Reflection {
                path: path.to_string(),
            },
            id,
            kind,
            text,
            caller,
        )
        .await
    }

    /// Open a thread on `target`.
    pub async fn open_thread(
        &self,
        domain: &str,
        target: &ThreadTarget,
        p: &DiscussParams,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let need = if p.answers_feedback {
            ReflectionRight::Curate
        } else {
            ReflectionRight::Read
        };
        let access = self.target_access(domain, target, caller, need).await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        refuse_anonymous_comment(self, domain, caller)?;
        let key = p.section.as_deref().ok_or_else(|| {
            EngineError::Invalid("open needs `section`, an id or a heading".into())
        })?;
        let text = p
            .text
            .as_deref()
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .ok_or_else(|| {
                EngineError::Invalid("open needs `text`, what you want to say".into())
            })?;
        if caller.author.agent && links_to_engrams(text) {
            return Err(EngineError::Invalid(PLAIN_WORDS.to_string()));
        }
        let (doc_abs, threads_abs) = Self::target_files(target, &access);
        // One lock per document: the page's, for the page and its threads
        // file alike.
        let _lock = self.lock_file(&doc_abs).await;
        let (thread, document, page_update) = match (target, &access) {
            (ThreadTarget::Reflection { path }, TargetAccess::Reflection(_)) => {
                let page = read_opt(&doc_abs)?.ok_or_else(|| {
                    EngineError::NotFound(format!("no reflection '{path}' in domain '{domain}'"))
                })?;
                let sections = r::split_sections(&page);
                let section = match r::resolve_section_key(&sections, key) {
                    Ok(s) if s.id.as_deref() != Some(r::TOP_SECTION) || key == r::TOP_SECTION => s,
                    Err(r::SectionKeyMiss::Ambiguous(headings)) => {
                        // Headings typed in a plain editor have no id until
                        // the page's next write through Crystalline.
                        let shared: Vec<&r::Section> = sections
                            .iter()
                            .filter(|s| headings.contains(&s.heading))
                            .collect();
                        let idless: Vec<String> = shared
                            .iter()
                            .filter(|s| s.id.is_none())
                            .map(|s| s.line.to_string())
                            .collect();
                        const BY_ID: &str = "open it by the section's id (read_reflection \
                                             shows each id under its heading)";
                        const GIVE_IDS: &str = "save or write the page once so Crystalline \
                             gives each heading its id, then open the thread by the id (the \
                             write's ids_assigned, or read_reflection under the heading)";
                        let way = match idless.len() {
                            0 => BY_ID.to_string(),
                            n if n == shared.len() => {
                                format!("none of them has an id yet: {GIVE_IDS}")
                            }
                            1 => format!(
                                "the one at line {} has no id yet: {GIVE_IDS}; for the others, \
                                 {BY_ID}",
                                idless[0]
                            ),
                            _ => format!(
                                "the ones at lines {} have no id yet: {GIVE_IDS}; for the \
                                 others, {BY_ID}",
                                idless.join(", ")
                            ),
                        };
                        return Err(EngineError::Invalid(format!(
                            "the heading '{key}' names {} sections in '{path}', so the thread \
                             cannot tell which one it is on: {way}",
                            headings.len()
                        )));
                    }
                    _ => {
                        return Err(EngineError::Invalid(format!(
                            "no section '{key}' in '{path}'"
                        )));
                    }
                };
                // A heading a plain editor added has no id yet. A comment
                // never rewrites the page (a reader may open a thread, and a
                // rewrite would make a person's open editor stale), so the
                // thread anchors on the heading instead; the write that
                // later gives the section its id moves the thread onto it.
                let section_ref = match &section.id {
                    Some(id) => SectionRef::Id(id.clone()),
                    None => SectionRef::Heading(section.heading.clone()),
                };
                let anchor = th::Anchor {
                    section: section_ref,
                    quote: p.quote.clone().filter(|q| !q.trim().is_empty()),
                    occurrence: p.occurrence.unwrap_or(1).max(1),
                };
                if let Some(quote) = anchor.quote.as_deref()
                    && !matches!(
                        r::anchor_state(&page, &anchor),
                        th::AnchorState::Found { .. }
                    )
                {
                    return Err(EngineError::Invalid(format!(
                        "'{quote}' is not in section '{key}' of '{path}' (occurrence {})",
                        anchor.occurrence
                    )));
                }
                let thread = Thread {
                    id: th::random_thread_id(),
                    anchor,
                    status: ThreadStatus::Open,
                    entries: vec![entry(EntryKind::Comment, caller, text)],
                };
                // The Reflection target's hook: a thread that answers a
                // person's change records itself on the section. A feedback
                // item belongs to a section with an id, so a heading without
                // one has none to answer.
                let page_update = if p.answers_feedback {
                    let id = thread.anchor.section.id().ok_or_else(|| {
                        EngineError::Invalid(format!(
                            "section '{key}' of '{path}' has no id yet, so it has no change to answer; open a plain thread instead"
                        ))
                    })?;
                    let mut prov = r::read_provenance(&page);
                    prov.entry(id.to_string()).or_default().answered_by = Some(thread.id.clone());
                    Some(r::write_provenance(&page, &prov).map_err(EngineError::Invalid)?)
                } else {
                    None
                };
                (thread, page, page_update)
            }
        };
        // The thread first, then the page that names it: a failed page write
        // leaves a plain thread, never a section that waits on a thread that
        // is not there.
        let mut undo = DiskUndo::default();
        undo.record(&threads_abs)?;
        undo.record(&doc_abs)?;
        let existing = read_opt(&threads_abs)?.unwrap_or_default();
        let threads_file = th::open_thread(&existing, &thread);
        write_file(&threads_abs, &threads_file)?;
        let mut document = document;
        // The thread and the page commit behind the host-lock fence: a host
        // that lost the lock since its gate keeps the comment in the kept
        // folder and puts both files back.
        match (target, &access) {
            (ThreadTarget::Reflection { path }, TargetAccess::Reflection(access)) => {
                let on_refusal = OnRefusal {
                    domain,
                    undo,
                    keep: Some(KeepText {
                        kind: crystalline_core::kept::KeptKind::Comment,
                        path,
                        text,
                        caller,
                        holds: &threads_abs,
                    }),
                    read: false,
                };
                match &page_update {
                    Some(page) => {
                        write_file(&doc_abs, page)?;
                        self.index_reflection(access.domain_id, path, page, &doc_abs, on_refusal)
                            .await?;
                    }
                    None => {
                        self.commit_reflection_rows(access.domain_id, &[], on_refusal)
                            .await?;
                    }
                }
            }
        }
        if let Some(page) = page_update {
            document = page;
        }
        match target {
            ThreadTarget::Reflection { path } => {
                self.mark_reflection_dirty(domain, BackupCause::Settled);
                self.announce_threads(domain, path, &threads_file, change_label(&caller.scope));
            }
        }
        Ok(match target {
            ThreadTarget::Reflection { path } => thread_view(path, &thread, &document),
        })
    }

    /// Add an entry to thread `id` on `target`.
    pub async fn thread_entry(
        &self,
        domain: &str,
        target: &ThreadTarget,
        id: &str,
        kind: EntryKind,
        text: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        if self.read_only {
            return Err(EngineError::ReadOnly);
        }
        let access = self
            .target_access(domain, target, caller, ReflectionRight::Read)
            .await?;
        self.refuse_read_only_domain(domain, &crate::scope::Scope::Unrestricted)
            .await?;
        refuse_anonymous_comment(self, domain, caller)?;
        let text = text.trim();
        if kind == EntryKind::Comment && text.is_empty() {
            return Err(EngineError::Invalid("answer needs `text`".into()));
        }
        if caller.author.agent && links_to_engrams(text) {
            return Err(EngineError::Invalid(PLAIN_WORDS.to_string()));
        }
        let (doc_abs, threads_abs) = Self::target_files(target, &access);
        // The page's lock, as in `open_thread`: a thread entry and a page
        // write, move or delete never interleave.
        let _lock = self.lock_file(&doc_abs).await;
        let document = match target {
            ThreadTarget::Reflection { path } => read_opt(&doc_abs)?.ok_or_else(|| {
                EngineError::NotFound(format!("no reflection '{path}' in domain '{domain}'"))
            })?,
        };
        let file = read_opt(&threads_abs)?
            .ok_or_else(|| EngineError::NotFound(format!("no thread {id} here")))?;
        // A person's thread is the person's to close: an agent's answer in a
        // resolved thread is hidden from them until they ask to see it.
        if kind == EntryKind::Resolve
            && caller.author.agent
            && th::parse_threads(&file)
                .iter()
                .find(|t| t.id == id)
                .is_some_and(|t| !th::agent_may_resolve(t))
        {
            return Err(EngineError::Invalid(th::PERSON_RESOLVES.to_string()));
        }
        let updated =
            th::append_entry(&file, id, &entry(kind, caller, text)).map_err(thread_error)?;
        let mut undo = DiskUndo::default();
        undo.record(&threads_abs)?;
        write_file(&threads_abs, &updated)?;
        // Behind the host-lock fence, as `open_thread`; the entry has no
        // row, so only the lock is read.
        match (target, &access) {
            (ThreadTarget::Reflection { path }, TargetAccess::Reflection(access)) => {
                self.commit_reflection_rows(
                    access.domain_id,
                    &[],
                    OnRefusal {
                        domain,
                        undo,
                        keep: Some(KeepText {
                            kind: crystalline_core::kept::KeptKind::Comment,
                            path,
                            text,
                            caller,
                            holds: &threads_abs,
                        }),
                        read: false,
                    },
                )
                .await?;
            }
        }
        let thread = th::parse_threads(&updated)
            .into_iter()
            .find(|t| t.id == id)
            .expect("just appended");
        match target {
            ThreadTarget::Reflection { path } => {
                self.mark_reflection_dirty(domain, BackupCause::Settled);
                self.announce_threads(domain, path, &updated, change_label(&caller.scope));
            }
        }
        Ok(match target {
            ThreadTarget::Reflection { path } => thread_view(path, &thread, &document),
        })
    }

    /// Every thread of a domain's Reflection, filtered by page and status,
    /// oldest first.
    pub async fn list_reflection_threads(
        &self,
        domain: &str,
        path: Option<&str>,
        status: &str,
        caller: &ReflectionCaller,
    ) -> Result<Value> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        let wanted = |t: &Thread| match status {
            "all" => true,
            "resolved" => t.status == ThreadStatus::Resolved,
            _ => t.status == ThreadStatus::Open,
        };
        let mut threads: Vec<(String, Thread, String)> = self
            .all_threads(&access)
            .await?
            .into_iter()
            .filter(|(p, t, _)| path.is_none_or(|want| want == p) && wanted(t))
            .collect();
        threads.sort_by(|a, b| a.1.opened_at().cmp(b.1.opened_at()));
        Ok(json!({
            "threads": threads
                .iter()
                .map(|(p, t, text)| thread_view(p, t, text))
                .collect::<Vec<_>>()
        }))
    }

    /// How many open threads a domain's Reflection holds, for a caller who
    /// reads it: the domain listing's count. Refused like every other
    /// reflection read.
    pub async fn reflection_open_threads(
        &self,
        domain: &str,
        caller: &ReflectionCaller,
    ) -> Result<usize> {
        let access = self
            .require_reflection(domain, caller, ReflectionRight::Read)
            .await?;
        self.open_thread_count(&access).await
    }

    /// How many open threads the Reflection behind `access` holds: the pages
    /// from the stamps listing, then each page's threads file, never a
    /// page's text. The files are read on the blocking pool, with no store
    /// lock held.
    pub(crate) async fn open_thread_count(&self, access: &ReflectionAccess) -> Result<usize> {
        let pages = self
            .store
            .lock()
            .await
            .list_reflection_stamps(access.domain_id)
            .await?;
        let files: Vec<PathBuf> = pages
            .iter()
            .map(|page| access.abs(&r::threads_path(&page.path)))
            .collect();
        tokio::task::spawn_blocking(move || {
            let mut open = 0;
            for file in files {
                if let Some(text) = read_opt(&file)? {
                    open += th::parse_threads(&text)
                        .iter()
                        .filter(|t| t.status == ThreadStatus::Open)
                        .count();
                }
            }
            Ok(open)
        })
        .await
        .map_err(|e| EngineError::Internal(format!("the threads files could not be read: {e}")))?
    }

    /// Every thread of a domain's Reflection with its reflection's path and
    /// current text, page by page in the listing's order. The threads files
    /// are read on the blocking pool, with no store lock held.
    pub(crate) async fn all_threads(
        &self,
        access: &ReflectionAccess,
    ) -> Result<Vec<(String, Thread, String)>> {
        #[cfg(any(test, feature = "testing"))]
        self.reflection_text_loads
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let rows = self
            .store
            .lock()
            .await
            .list_reflections(access.domain_id)
            .await?;
        let pages: Vec<(PathBuf, String, String)> = rows
            .into_iter()
            .map(|row| {
                (
                    access.abs(&r::threads_path(&row.path)),
                    row.path,
                    row.content,
                )
            })
            .collect();
        tokio::task::spawn_blocking(move || {
            let mut out = Vec::new();
            for (file, path, content) in pages {
                let Some(text) = read_opt(&file)? else {
                    continue;
                };
                for thread in th::parse_threads(&text) {
                    out.push((path.clone(), thread, content.clone()));
                }
            }
            Ok(out)
        })
        .await
        .map_err(|e| EngineError::Internal(format!("the threads files could not be read: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_core::config::DomainEntry;
    use crystalline_index::TursoStore;

    const MANIFEST: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\nreflection: on\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

    const PAGE: &str = "# D\n\n## Night deploys\n<!-- r:aaaa1111 -->\n\nDeploys run at night.\n";

    /// A file domain `eng` with its Reflection on and one page, `d.md`.
    async fn reflecting(tmp: &Path) -> (Arc<Engine>, PathBuf) {
        let root = tmp.join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("MANIFEST.md"), MANIFEST).unwrap();
        let store = TursoStore::open_in_memory().await.unwrap();
        let mut config = GlobalConfig::default();
        config
            .domains
            .insert("eng".to_string(), DomainEntry::file(root.clone()));
        let engine = Arc::new(
            Engine::new(Arc::new(Mutex::new(store)), config, None, None)
                .with_state_dir(tmp.join("state"))
                .with_origins_dir(tmp.join("origins")),
        );
        engine.sync(None).await.unwrap();
        let caller = ReflectionCaller::person(crate::scope::Scope::Unrestricted, "jordi");
        engine
            .write_reflection(
                &crate::params::WriteReflectionParams {
                    domain: "eng".into(),
                    path: "d.md".into(),
                    markdown: Some(PAGE.into()),
                    ..Default::default()
                },
                &caller,
            )
            .await
            .unwrap();
        (engine, root.join("reflections/d.md"))
    }

    fn open_params(text: &str) -> DiscussParams {
        DiscussParams {
            domain: "eng".into(),
            path: Some("d.md".into()),
            section: Some("aaaa1111".into()),
            text: Some(text.into()),
            ..Default::default()
        }
    }

    /// Every write to a threads file runs under the page's own lock, the one
    /// a page write, move or delete takes: with that lock held from outside,
    /// an open and an answer both wait, and each reads the files only once
    /// it is theirs. A page moved away underneath them is then not found,
    /// and nothing is written beside the old path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_thread_write_waits_on_the_page_lock() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, page_abs) = reflecting(tmp.path()).await;
        let caller = ReflectionCaller::person(crate::scope::Scope::Unrestricted, "jordi");
        let opened = engine
            .discuss(&open_params("Still true?"), &caller)
            .await
            .unwrap();
        let id = opened["id"].as_str().unwrap().to_string();
        let threads_abs = page_abs.with_file_name("d.threads.md");

        let lock = engine.write_lock(&page_abs);
        let held = lock.lock().await;
        let (opener, answerer) = (engine.clone(), engine.clone());
        let (open_caller, answer_caller) = (caller.clone(), caller.clone());
        let open = tokio::spawn(async move {
            opener
                .discuss(&open_params("Another one"), &open_caller)
                .await
        });
        let answer = tokio::spawn(async move {
            answerer
                .reflection_thread_entry(
                    "eng",
                    "d.md",
                    &id,
                    EntryKind::Comment,
                    "Yes.",
                    &answer_caller,
                )
                .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(!open.is_finished(), "the open waits on the page's lock");
        assert!(!answer.is_finished(), "the answer waits on the page's lock");

        // What a move does under that lock: the page and its threads leave.
        let moved = page_abs.with_file_name("moved.md");
        std::fs::rename(&page_abs, &moved).unwrap();
        std::fs::rename(&threads_abs, moved.with_file_name("moved.threads.md")).unwrap();
        drop(held);

        let open = open.await.unwrap().unwrap_err();
        assert!(matches!(open, EngineError::NotFound(_)), "{open:?}");
        let answer = answer.await.unwrap().unwrap_err();
        assert!(matches!(answer, EngineError::NotFound(_)), "{answer:?}");
        assert!(
            !threads_abs.exists(),
            "no threads file is left at the old path"
        );
        assert!(!page_abs.exists());
    }

    /// Both walks over the threads files read them on the blocking pool:
    /// every file read in `all_threads` and `open_thread_count` comes after
    /// their `spawn_blocking`, never on a runtime worker (F2).
    #[test]
    fn the_threads_files_are_read_off_the_runtime() {
        let src = include_str!("threads.rs");
        for signature in [
            "pub(crate) async fn all_threads(",
            "pub(crate) async fn open_thread_count(",
        ] {
            let start = src
                .find(signature)
                .unwrap_or_else(|| panic!("source pin: {signature} is not in threads.rs"));
            let end = start + src[start..].find("\n    }\n").expect("the function ends");
            let body = &src[start..end];
            // A source pin: it reads this file's text, not the running code.
            let pin = "this is a source pin on threads.rs: a threads file must be read \
                       inside spawn_blocking, never on a runtime worker. Move the read \
                       back into the closure; if the function was renamed or reshaped \
                       on purpose, update the signature or the needles here";
            let blocking = body
                .find("spawn_blocking(")
                .unwrap_or_else(|| panic!("{signature} has no spawn_blocking; {pin}"));
            let first_read = body
                .find("read_opt(")
                .unwrap_or_else(|| panic!("{signature} has no read_opt; {pin}"));
            assert!(
                first_read > blocking,
                "{signature} reads a threads file before its spawn_blocking; {pin}"
            );
        }
    }

    /// The threads of every page, page by page in the listing's order and
    /// each with its own page's text, as the reflect queue reads them.
    #[tokio::test]
    async fn all_threads_keeps_the_page_order_and_text() {
        let tmp = tempfile::tempdir().unwrap();
        let (engine, _) = reflecting(tmp.path()).await;
        let caller = ReflectionCaller::person(crate::scope::Scope::Unrestricted, "jordi");
        let second = "# E\n\n## Day deploys\n<!-- r:bbbb2222 -->\n\nDeploys run by day.\n";
        engine
            .write_reflection(
                &crate::params::WriteReflectionParams {
                    domain: "eng".into(),
                    path: "e.md".into(),
                    markdown: Some(second.into()),
                    ..Default::default()
                },
                &caller,
            )
            .await
            .unwrap();
        for (path, section) in [
            ("e.md", "bbbb2222"),
            ("d.md", "aaaa1111"),
            ("e.md", "bbbb2222"),
        ] {
            engine
                .discuss(
                    &DiscussParams {
                        domain: "eng".into(),
                        path: Some(path.into()),
                        section: Some(section.into()),
                        text: Some("Still true?".into()),
                        ..Default::default()
                    },
                    &caller,
                )
                .await
                .unwrap();
        }
        let access = engine
            .require_reflection("eng", &caller, ReflectionRight::Read)
            .await
            .unwrap();
        let got: Vec<(String, bool)> = engine
            .all_threads(&access)
            .await
            .unwrap()
            .into_iter()
            .map(|(path, _, text)| {
                let own = if path == "d.md" { "night" } else { "by day" };
                (path, text.contains(own))
            })
            .collect();
        assert_eq!(
            got,
            [
                ("d.md".to_string(), true),
                ("e.md".to_string(), true),
                ("e.md".to_string(), true),
            ]
        );
    }
}
