//! Log format and embedding thread count, both chosen from the environment
//! because they are needed before the config overlay loads.

use std::sync::OnceLock;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

/// The variable that picks the log format.
pub const LOG_FORMAT_VAR: &str = "CRYSTALLINE_LOG_FORMAT";

/// How a log line is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Text,
    Json,
}

/// Reads a `CRYSTALLINE_LOG_FORMAT` value. `json` and `text` match without
/// regard to case, unset means text. Any other value keeps text and comes
/// back with a warning to log once the subscriber is up.
pub fn parse_log_format(value: Option<&str>) -> (LogFormat, Option<String>) {
    match value.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        None | Some("text") => (LogFormat::Text, None),
        Some("json") => (LogFormat::Json, None),
        Some(other) => (
            LogFormat::Text,
            Some(format!(
                "{LOG_FORMAT_VAR}={other:?} is not a log format. Use json or text. Logging stays text."
            )),
        ),
    }
}

/// Installs the global subscriber with the format from the environment.
/// `ansi` is `Some(false)` for a log file. The first subscriber installed
/// stays, as with a plain `try_init`.
pub fn init_subscriber<W>(writer: W, ansi: Option<bool>, filter: EnvFilter)
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let (format, warning) = parse_log_format(std::env::var(LOG_FORMAT_VAR).ok().as_deref());
    let installed = match format {
        LogFormat::Json => tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_writer(writer)
            .with_env_filter(filter)
            .try_init()
            .is_ok(),
        LogFormat::Text => {
            let builder = tracing_subscriber::fmt()
                .with_writer(writer)
                .with_env_filter(filter);
            match ansi {
                Some(on) => builder.with_ansi(on).try_init().is_ok(),
                None => builder.try_init().is_ok(),
            }
        }
    };
    if installed && let Some(warning) = warning {
        tracing::warn!("{warning}");
    }
}

/// The variable that sets the embedding thread count.
pub const EMBED_THREADS_VAR: &str = "CRYSTALLINE_EMBED_THREADS";

/// Where the embedding thread count came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadSource {
    /// `RAYON_NUM_THREADS` was already set and wins.
    RayonEnv,
    /// `CRYSTALLINE_EMBED_THREADS`.
    EmbedThreadsEnv,
    /// The CPU limit: `available_parallelism` below the physical core count.
    CpuLimit,
    /// Left to candle (macOS, where it picks the performance cores).
    CandleDefault,
}

/// What `main` should do about the thread count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPlan {
    /// The value for `RAYON_NUM_THREADS`, when it is to be set.
    pub set: Option<usize>,
    pub source: ThreadSource,
    /// An ignored `CRYSTALLINE_EMBED_THREADS`, to log once logging is up.
    pub warning: Option<String>,
}

/// Decides the embedding thread count. Pure: the caller passes the two
/// variables, the CPUs this process may use (`available_parallelism`, which
/// honours a cgroup quota), the physical core count as candle counts it
/// (`num_cpus::get_physical`, which does not) and whether this is macOS.
pub fn plan_embed_threads(
    rayon_env: Option<&str>,
    embed_env: Option<&str>,
    available: usize,
    physical: usize,
    is_macos: bool,
) -> ThreadPlan {
    if rayon_env.is_some() {
        return ThreadPlan {
            set: None,
            source: ThreadSource::RayonEnv,
            warning: None,
        };
    }
    let mut warning = None;
    if let Some(raw) = embed_env {
        match raw.trim().parse::<usize>() {
            Ok(n) if n > 0 => {
                return ThreadPlan {
                    set: Some(n),
                    source: ThreadSource::EmbedThreadsEnv,
                    warning: None,
                };
            }
            _ => {
                warning = Some(format!(
                    "{EMBED_THREADS_VAR}={raw:?} is not a positive whole number and is ignored."
                ));
            }
        }
    }
    if is_macos {
        // candle picks the performance cores on Apple Silicon. A count from
        // here could land on the efficiency cores too, so it stays out.
        return ThreadPlan {
            set: None,
            source: ThreadSource::CandleDefault,
            warning,
        };
    }
    ThreadPlan {
        set: Some(available.min(physical).max(1)),
        source: ThreadSource::CpuLimit,
        warning,
    }
}

static THREAD_PLAN: OnceLock<ThreadPlan> = OnceLock::new();

/// Remembers the plan `main` applied, for the daemon's start-up log line.
pub fn record_thread_plan(plan: ThreadPlan) {
    let _ = THREAD_PLAN.set(plan);
}

/// Logs the embedding thread count once, with where it came from.
pub fn log_thread_plan() {
    let Some(plan) = THREAD_PLAN.get() else {
        return;
    };
    if let Some(warning) = &plan.warning {
        tracing::warn!("{warning}");
    }
    let (count, origin) = match (&plan.source, plan.set) {
        (ThreadSource::RayonEnv, _) => (
            std::env::var("RAYON_NUM_THREADS").unwrap_or_default(),
            "RAYON_NUM_THREADS",
        ),
        (ThreadSource::EmbedThreadsEnv, Some(n)) => (n.to_string(), EMBED_THREADS_VAR),
        (ThreadSource::CpuLimit, Some(n)) => (n.to_string(), "the CPU limit"),
        _ => ("automatic".to_string(), "the default of the model runtime"),
    };
    tracing::info!("embedding threads: {count} (from {origin})");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[test]
    fn log_format_parses_case_insensitively() {
        assert_eq!(parse_log_format(Some("json")), (LogFormat::Json, None));
        assert_eq!(parse_log_format(Some("JSON")), (LogFormat::Json, None));
        assert_eq!(parse_log_format(Some("Text")), (LogFormat::Text, None));
        assert_eq!(parse_log_format(None), (LogFormat::Text, None));
    }

    #[test]
    fn an_unknown_log_format_keeps_text_and_warns() {
        let (format, warning) = parse_log_format(Some("yaml"));
        assert_eq!(format, LogFormat::Text);
        let warning = warning.expect("a warning");
        assert!(warning.contains(LOG_FORMAT_VAR));
        assert!(warning.contains("json") && warning.contains("text"));
    }

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl<'a> MakeWriter<'a> for Buf {
        type Writer = Buf;
        fn make_writer(&'a self) -> Buf {
            self.clone()
        }
    }

    #[test]
    fn a_json_line_carries_the_top_level_fields_datadog_reads() {
        let buf = Buf::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_writer(buf.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || tracing::info!("hello world"));
        let text = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        let line: serde_json::Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        assert!(line["timestamp"].is_string());
        assert_eq!(line["level"], "INFO");
        assert_eq!(line["message"], "hello world");
        assert!(line["target"].is_string());
    }

    #[test]
    fn rayon_env_wins() {
        let p = plan_embed_threads(Some("3"), Some("5"), 2, 8, false);
        assert_eq!((p.set, p.source), (None, ThreadSource::RayonEnv));
    }

    #[test]
    fn embed_threads_sets_the_count() {
        let p = plan_embed_threads(None, Some("4"), 2, 8, false);
        assert_eq!((p.set, p.source), (Some(4), ThreadSource::EmbedThreadsEnv));
        assert!(p.warning.is_none());
    }

    #[test]
    fn an_invalid_embed_threads_warns_and_falls_back() {
        for bad in ["0", "many", "-2", ""] {
            let p = plan_embed_threads(None, Some(bad), 2, 8, false);
            assert_eq!((p.set, p.source.clone()), (Some(2), ThreadSource::CpuLimit));
            assert!(p.warning.is_some(), "{bad:?}");
        }
    }

    #[test]
    fn the_default_is_the_smaller_of_quota_and_physical_cores() {
        assert_eq!(plan_embed_threads(None, None, 2, 8, false).set, Some(2));
        assert_eq!(plan_embed_threads(None, None, 16, 8, false).set, Some(8));
    }

    #[test]
    fn macos_keeps_candles_choice() {
        let p = plan_embed_threads(None, None, 8, 8, true);
        assert_eq!((p.set, p.source), (None, ThreadSource::CandleDefault));
        let p = plan_embed_threads(None, Some("3"), 8, 8, true);
        assert_eq!(p.set, Some(3));
    }
}
