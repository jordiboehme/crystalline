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
//! The limit travels as a task-local value the serving surface sets around
//! each request it serves ([`within`]). [`crate::engine::Engine::hidden_domains`]
//! adds it to whatever the caller's own rights hide, so every read that
//! screens domains (search, read, context, browse, activity, listings,
//! sources, attachments) screens these too, and a Reflection gate refuses
//! them as unknown domains.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Arc;

tokio::task_local! {
    static LIMIT: Arc<HashSet<String>>;
}

/// Run `work` with `hidden` (local domain names) out of every read it makes,
/// or as it is when `hidden` is `None`.
pub async fn within<F: Future>(hidden: Option<HashSet<String>>, work: F) -> F::Output {
    match hidden {
        Some(hidden) => LIMIT.scope(Arc::new(hidden), work).await,
        None => work.await,
    }
}

/// The domains the current request may not read because it is unattended,
/// or `None` when no limit is set.
pub fn current() -> Option<Arc<HashSet<String>>> {
    LIMIT.try_with(Arc::clone).ok()
}
