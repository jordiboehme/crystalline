//! The `subscriptions/listen` sinks a moved tool list is announced on.
//!
//! MCP 2026-07-28 removed the unsolicited notification channel outright: a
//! `notifications/tools/list_changed` either rides a subscription stream the
//! client opened or it does not exist. So a server with a list that can move
//! needs somewhere to keep the sinks of everyone currently listening, and this
//! is it.
//!
//! **It lives on the shared [`crate::Engine`] rather than on the MCP handler,
//! and that placement is the whole reason the module exists.** On the
//! streamable-HTTP path rmcp builds a fresh service per request
//! (`get_service()`, rmcp 3.1.2 `tower.rs:1822` and `:1948`) and every modern
//! peer routes statelessly, so the handler that takes a subscription and the
//! handler that later flips a setting are different objects which share only
//! the engine. A handler-local registry would work over stdio and on the
//! legacy session path and silently do nothing over HTTP. The flip is not
//! even always an MCP call: the control socket and the REST API write the same
//! setting, which is why `Engine::configure` is what sends on these sinks.
//!
//! [`SubscriptionSink`] is `Clone`, every field is `Send + Sync + 'static`
//! (`service/server.rs:139-144`), and it holds a `Peer` plus a child
//! cancellation token - so an entry left behind after its stream ended would
//! pin a dead peer. [`Subscriber`] is the RAII guard that prevents that:
//! `listen` holds one for the life of the stream and dropping it removes the
//! entry, however the stream ended.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::service::{SubscriptionSendError, SubscriptionSink};

/// Every open `subscriptions/listen` stream, keyed by a registration id this
/// type hands out.
///
/// Registration is unconditional: `listen` records the sink whatever
/// categories the filter asked for, and rmcp applies the accepted filter on
/// send (`service/server.rs:180-215`). So a stream that subscribed to
/// resources only is held here too and simply declines a tools notification.
///
/// A plain `std` mutex on purpose: the critical section only ever clones or
/// drops sinks and never awaits. Sending is done on clones taken outside the
/// lock, so one slow or wedged peer cannot block a `configure` call or another
/// subscriber's notification.
#[derive(Debug, Default)]
pub struct ListSubscribers {
    sinks: std::sync::Mutex<Vec<(u64, SubscriptionSink)>>,
    next_id: AtomicU64,
}

impl ListSubscribers {
    /// Record `sink` and return the guard that removes it again. Held by
    /// `McpServer::listen` for exactly as long as the stream is open.
    ///
    /// An associated function rather than a method because the guard has to
    /// own a handle on the registry, and the engine is what holds the `Arc`.
    pub fn register(registry: &Arc<Self>, sink: SubscriptionSink) -> Subscriber {
        let id = registry.next_id.fetch_add(1, Ordering::Relaxed);
        registry.sinks.lock().unwrap().push((id, sink));
        Subscriber {
            registry: registry.clone(),
            id,
        }
    }

    /// How many streams are currently listening. Only used by tests and by the
    /// debug log line in `listen`.
    pub fn len(&self) -> usize {
        self.sinks.lock().unwrap().len()
    }

    /// Whether nobody is listening.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Announce that `tools/list` now answers differently, to every open
    /// stream and to nobody else.
    ///
    /// A peer that has gone away since it subscribed is dropped rather than
    /// retried: `SubscriptionSink::send` reports a cancelled stream as
    /// [`SubscriptionSendError::SubscriptionClosed`], and the guard normally
    /// removes such an entry already, so this is the belt to that braces.
    ///
    /// [`SubscriptionSendError::NotificationNotAccepted`] is not a failure at
    /// all: it is what a live stream that subscribed to other categories
    /// answers, which is the ordinary outcome for every sink here that did not
    /// ask for tools. It stays registered - its other categories are still
    /// live - and is not worth a word of alarm. Anything else is a live peer's
    /// transport trouble and is logged, also without unregistering, so the
    /// next flip tries again.
    pub async fn notify_tool_list_changed(&self) {
        let current: Vec<(u64, SubscriptionSink)> = self.sinks.lock().unwrap().clone();
        let mut closed: Vec<u64> = Vec::new();
        for (id, sink) in current {
            match sink.notify_tool_list_changed().await {
                Ok(()) => {}
                Err(SubscriptionSendError::SubscriptionClosed) => closed.push(id),
                Err(SubscriptionSendError::NotificationNotAccepted(_)) => {
                    tracing::debug!(
                        subscription = id,
                        "subscriber did not ask for the tools category"
                    );
                }
                Err(e) => {
                    tracing::debug!(error = %e, "tools/list_changed could not be delivered");
                }
            }
        }
        if !closed.is_empty() {
            self.sinks
                .lock()
                .unwrap()
                .retain(|(id, _)| !closed.contains(id));
        }
    }
}

/// One registered subscription stream. Dropping it unregisters the sink, so a
/// stream that ended - cancelled, dropped, or its connection lost - leaves no
/// dead peer behind in the registry.
#[derive(Debug)]
pub struct Subscriber {
    registry: Arc<ListSubscribers>,
    id: u64,
}

impl Drop for Subscriber {
    fn drop(&mut self) {
        self.registry
            .sinks
            .lock()
            .unwrap()
            .retain(|(id, _)| *id != self.id);
    }
}
