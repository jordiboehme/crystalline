//! How long the periodic jobs, the GitHub access lookups and the automation
//! status calls take, logged under one target, `crystalline_engine::timing`,
//! so an operator turns all of them on with one `RUST_LOG` directive.
//!
//! Every pass is a DEBUG line with its elapsed time; one that takes
//! [`SLOW`] or more is an INFO line instead, so it shows under the default
//! filter.

use std::time::Duration;

use tokio::time::Instant;

/// From how long a pass is logged at INFO rather than DEBUG.
pub const SLOW: Duration = Duration::from_secs(1);

/// Logs how long something took since `$started` (a [`tokio::time::Instant`])
/// under the target `crystalline_engine::timing`, with the given fields and
/// message: DEBUG, or INFO at [`SLOW`] or more. The elapsed time is the field
/// `elapsed`.
#[macro_export]
macro_rules! log_elapsed {
    ($started:expr, $($rest:tt)+) => {{
        let elapsed = $started.elapsed();
        if elapsed >= $crate::timing::SLOW {
            ::tracing::info!(target: "crystalline_engine::timing", ?elapsed, $($rest)+);
        } else {
            ::tracing::debug!(target: "crystalline_engine::timing", ?elapsed, $($rest)+);
        }
    }};
}

/// One pass of a periodic job, logged with its elapsed time when it is
/// dropped, so a pass that ends early (a `continue`, a `break`, an error)
/// is logged too.
pub struct Pass {
    job: &'static str,
    domain: Option<String>,
    started: Instant,
}

impl Pass {
    /// A pass of `job` starting now.
    pub fn start(job: &'static str) -> Self {
        Self {
            job,
            domain: None,
            started: Instant::now(),
        }
    }

    /// A pass of `job` over the domain `domain` starting now.
    pub fn of(job: &'static str, domain: &str) -> Self {
        Self {
            job,
            domain: Some(domain.to_string()),
            started: Instant::now(),
        }
    }
}

impl Drop for Pass {
    fn drop(&mut self) {
        let job = self.job;
        match &self.domain {
            Some(domain) => crate::log_elapsed!(self.started, job, ?domain, "job pass finished"),
            None => crate::log_elapsed!(self.started, job, "job pass finished"),
        }
    }
}
