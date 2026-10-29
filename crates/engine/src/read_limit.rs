//! What an unattended session may read.
//!
//! An agent that runs on its own (a reflect or evolve loop run, or any
//! session its registration marks unattended) answers people it has never
//! met: anyone who may comment on a shared page can ask it a question in a
//! thread. If it could read a private domain it could quote it into a page
//! everyone reads, and no check on what it writes can catch that, because
//! the quote names no source. So the limit sits on what it reads: while a
//! session is unattended, every read answers as if the domains it may not
//! carry onto its pages did not exist ([`crate::engine::Engine::read_limit_for`]
//! decides which).
//!
//! An unattended request also writes only into the domains its run works
//! on ([`Limit::writes`]): a run on a private domain reads that domain, and
//! writing what it read into a shared one would carry it to more readers.
//!
//! The limit travels as a task-local value the serving surface sets around
//! each request it serves ([`within`]). [`crate::engine::Engine::hidden_domains`]
//! adds it to whatever the caller's own rights hide, so every read that
//! screens domains (search, read, context, browse, activity, listings,
//! sources, attachments) screens these too, and a Reflection gate refuses
//! them as unknown domains.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

/// What one unattended request may read and write.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limit {
    /// The local domains it may not read.
    pub hidden: HashSet<String>,
    /// The domains it may write into, sorted: the domains its run works on.
    /// `None` when it may write wherever its rights allow, because what it
    /// reads every reader of every domain may read anyway.
    pub writes: Option<Vec<String>>,
}

tokio::task_local! {
    static LIMIT: Arc<Limit>;
}

/// Run `work` within `limit`, or as it is when `limit` is `None`.
pub async fn within<F: Future>(limit: Option<Limit>, work: F) -> F::Output {
    match limit {
        Some(limit) => LIMIT.scope(Arc::new(limit), work).await,
        None => work.await,
    }
}

/// The limit of the current request, when it is unattended.
pub fn current() -> Option<Arc<Limit>> {
    LIMIT.try_with(Arc::clone).ok()
}

/// The domains the current request may not read, when it is unattended.
pub fn hidden() -> Option<HashSet<String>> {
    current().map(|limit| limit.hidden.clone())
}

/// Whether the current request may not read `domain`.
pub fn hides(domain: &str) -> bool {
    current().is_some_and(|limit| limit.hidden.contains(domain))
}
