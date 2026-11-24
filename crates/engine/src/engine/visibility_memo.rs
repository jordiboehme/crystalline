//! One fold of a caller's visibility per call.
//!
//! [`Engine::visibility_in`](super::Engine::visibility_in) folds every
//! private and every GitHub-governed domain for the caller, and asks the
//! governor about each governed one. A call that checks rights on many
//! domains (an automation status names every domain a caller may run a loop
//! on, and each check asks for the whole fold again) runs its body
//! [`within`] a memo: the first fold for a scope is kept for the rest of
//! that call and nowhere else. The memo is a task local, so it ends with the
//! call and never reaches another request, and a right that changes is seen
//! by the next call.

use std::future::Future;
use std::sync::Mutex;

use crate::scope::{DomainVisibility, Scope};

/// The folds one call made, by the scope they were made for.
#[derive(Default)]
struct Memo(Mutex<Vec<(Scope, DomainVisibility)>>);

tokio::task_local! {
    static MEMO: Memo;
}

/// Run `work` with one fold per scope: a fresh memo, or the one of the call
/// it already runs in.
pub(crate) async fn within<F: Future>(work: F) -> F::Output {
    if MEMO.try_with(|_| ()).is_ok() {
        return work.await;
    }
    MEMO.scope(Memo::default(), work).await
}

/// The fold this call already made for `scope`, when it runs within a memo
/// and made one.
pub(super) fn recall(scope: &Scope) -> Option<DomainVisibility> {
    MEMO.try_with(|memo| {
        memo.0
            .lock()
            .unwrap()
            .iter()
            .find(|(kept, _)| kept == scope)
            .map(|(_, visibility)| visibility.clone())
    })
    .ok()
    .flatten()
}

/// Keep `visibility` as the fold for `scope`, when the call runs within a
/// memo.
pub(super) fn keep(scope: &Scope, visibility: &DomainVisibility) {
    let _ = MEMO.try_with(|memo| {
        memo.0
            .lock()
            .unwrap()
            .push((scope.clone(), visibility.clone()));
    });
}
