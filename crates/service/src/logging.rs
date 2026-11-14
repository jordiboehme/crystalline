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

/// Set beside `RAYON_NUM_THREADS` when this binary chose the count, as
/// `<source>:<value>`, so a daemon the CLI started (which inherits both)
/// names the real source. See `crystalline_core::thread_env`.
pub use crystalline_core::thread_env::EMBED_THREADS_FROM_VAR;

/// Where the embedding thread count came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadSource {
    /// `RAYON_NUM_THREADS` was already set and wins.
    RayonEnv,
    /// `CRYSTALLINE_EMBED_THREADS`.
    EmbedThreadsEnv,
    /// The CPU limit: `available_parallelism` below the physical core count.
    CpuLimit,
    /// Left to candle (macOS, `CANDLE_CPU_MASK`, or no CPU limit).
    CandleDefault,
}

impl ThreadSource {
    /// The source part of `CRYSTALLINE_EMBED_THREADS_FROM` for this source.
    pub fn marker(&self) -> &'static str {
        match self {
            ThreadSource::RayonEnv => "rayon",
            ThreadSource::EmbedThreadsEnv => crystalline_core::thread_env::SOURCE_EMBED_THREADS,
            ThreadSource::CpuLimit => crystalline_core::thread_env::SOURCE_CPU_LIMIT,
            ThreadSource::CandleDefault => "default",
        }
    }
}

/// What `main` should do about the thread count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadPlan {
    /// The value for `RAYON_NUM_THREADS`, when it is to be set.
    pub set: Option<usize>,
    /// The count in effect, when it is known: `set`, or a valid
    /// `RAYON_NUM_THREADS`. `None` means the model runtime picks it.
    pub count: Option<usize>,
    pub source: ThreadSource,
    /// Problems with `CRYSTALLINE_EMBED_THREADS`, to log once logging is up.
    pub warning: Option<String>,
}

impl ThreadPlan {
    /// The value of the marker variable, when the plan sets the count.
    pub fn marker_value(&self) -> Option<String> {
        self.set.map(|n| format!("{}:{n}", self.source.marker()))
    }
}

/// The CPU counts and platform facts the plan depends on.
#[derive(Debug, Clone, Copy)]
pub struct Cpus {
    /// What this process may use (`available_parallelism`, which honours a
    /// cgroup quota and the affinity mask).
    pub available: usize,
    /// The physical core count as candle counts it (`num_cpus::get_physical`,
    /// which does not honour a quota).
    pub physical: usize,
    pub is_macos: bool,
    /// Whether `CANDLE_CPU_MASK` is set.
    pub candle_cpu_mask: bool,
}

fn positive(raw: &str) -> Option<usize> {
    raw.trim().parse::<usize>().ok().filter(|n| *n > 0)
}

/// Decides the embedding thread count. Pure: the caller passes the three
/// variables and the CPU facts.
///
/// A positive `RAYON_NUM_THREADS` always stays. The marker is trusted only
/// when its value equals it. A value that is not a positive whole number
/// counts as unset.
pub fn plan_embed_threads(
    rayon_env: Option<&str>,
    from_env: Option<&str>,
    embed_env: Option<&str>,
    cpus: Cpus,
) -> ThreadPlan {
    if let Some(n) = rayon_env.and_then(positive) {
        let source = match crystalline_core::thread_env::marker_source(from_env, rayon_env) {
            Some(m) if m == crystalline_core::thread_env::SOURCE_EMBED_THREADS => {
                ThreadSource::EmbedThreadsEnv
            }
            Some(_) => ThreadSource::CpuLimit,
            None => ThreadSource::RayonEnv,
        };
        let warning = (source == ThreadSource::RayonEnv && embed_env.is_some()).then(|| {
            format!("{EMBED_THREADS_VAR} is ignored because RAYON_NUM_THREADS={n} is set and wins.")
        });
        return ThreadPlan {
            set: None,
            count: Some(n),
            source,
            warning,
        };
    }
    let mut warning = None;
    if let Some(raw) = embed_env {
        match positive(raw) {
            Some(n) => {
                let max = cpus.available.saturating_mul(4).max(1);
                if n > max {
                    warning = Some(format!(
                        "{EMBED_THREADS_VAR}={n} is more than 4 times the {} CPUs this process may use. {max} is used.",
                        cpus.available
                    ));
                }
                let n = n.min(max);
                return ThreadPlan {
                    set: Some(n),
                    count: Some(n),
                    source: ThreadSource::EmbedThreadsEnv,
                    warning,
                };
            }
            None => {
                warning = Some(format!(
                    "{EMBED_THREADS_VAR}={raw:?} is not a positive whole number and is ignored."
                ));
            }
        }
    }
    // candle picks the performance cores on Apple Silicon and honours
    // `CANDLE_CPU_MASK` on Linux. A count from here would override both.
    // Only a CPU limit that really applies is worth overriding candle for.
    if !cpus.is_macos && !cpus.candle_cpu_mask && cpus.available < cpus.physical {
        let n = cpus.available.max(1);
        return ThreadPlan {
            set: Some(n),
            count: Some(n),
            source: ThreadSource::CpuLimit,
            warning,
        };
    }
    ThreadPlan {
        set: None,
        count: None,
        source: ThreadSource::CandleDefault,
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
    tracing::info!("{}", thread_plan_line(plan));
}

/// The start-up line for a plan. Never prints an empty count.
fn thread_plan_line(plan: &ThreadPlan) -> String {
    let count = plan
        .count
        .map_or_else(|| "automatic".to_string(), |n| n.to_string());
    let origin = match plan.source {
        ThreadSource::RayonEnv => "RAYON_NUM_THREADS",
        ThreadSource::EmbedThreadsEnv => EMBED_THREADS_VAR,
        ThreadSource::CpuLimit => "the CPU limit",
        ThreadSource::CandleDefault => "the default of the model runtime",
    };
    format!("embedding threads: {count} (from {origin})")
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

    fn cpus(available: usize, physical: usize) -> Cpus {
        Cpus {
            available,
            physical,
            is_macos: false,
            candle_cpu_mask: false,
        }
    }

    #[test]
    fn rayon_env_wins() {
        let p = plan_embed_threads(Some("3"), None, None, cpus(2, 8));
        assert_eq!(
            (p.set, p.count, p.source),
            (None, Some(3), ThreadSource::RayonEnv)
        );
        assert!(p.warning.is_none());
    }

    #[test]
    fn embed_threads_losing_to_rayon_env_warns_once() {
        let p = plan_embed_threads(Some("3"), None, Some("5"), cpus(2, 8));
        assert_eq!(p.source, ThreadSource::RayonEnv);
        let w = p.warning.expect("a warning");
        assert!(w.contains(EMBED_THREADS_VAR) && w.contains("RAYON_NUM_THREADS=3"));
    }

    #[test]
    fn embed_threads_sets_the_count() {
        let p = plan_embed_threads(None, None, Some("4"), cpus(2, 8));
        assert_eq!(
            (p.set, p.source.clone()),
            (Some(4), ThreadSource::EmbedThreadsEnv)
        );
        assert_eq!(p.marker_value().as_deref(), Some("embed-threads:4"));
        assert!(p.warning.is_none());
    }

    #[test]
    fn an_invalid_embed_threads_warns_and_falls_back() {
        for bad in ["0", "many", "-2", ""] {
            let p = plan_embed_threads(None, None, Some(bad), cpus(2, 8));
            assert_eq!((p.set, p.source.clone()), (Some(2), ThreadSource::CpuLimit));
            assert!(p.warning.is_some(), "{bad:?}");
        }
    }

    #[test]
    fn an_embed_threads_far_above_the_cpus_is_clamped_with_a_warning() {
        let p = plan_embed_threads(None, None, Some("100000"), cpus(2, 8));
        assert_eq!(
            (p.set, p.source.clone()),
            (Some(8), ThreadSource::EmbedThreadsEnv)
        );
        let w = p.warning.expect("a warning");
        assert!(w.contains(EMBED_THREADS_VAR) && w.contains("100000") && w.contains('8'));
        // Exactly 4 times is fine, and the clamp is at least 1.
        let p = plan_embed_threads(None, None, Some("8"), cpus(2, 8));
        assert_eq!((p.set, p.warning), (Some(8), None));
    }

    #[test]
    fn a_cpu_limit_below_the_physical_cores_sets_that_limit() {
        let p = plan_embed_threads(None, None, None, cpus(2, 8));
        assert_eq!((p.set, p.source.clone()), (Some(2), ThreadSource::CpuLimit));
        assert_eq!(p.marker_value().as_deref(), Some("cpu-limit:2"));
    }

    #[test]
    fn without_a_cpu_limit_candle_keeps_its_choice() {
        for (a, ph) in [(8, 8), (16, 8)] {
            let p = plan_embed_threads(None, None, None, cpus(a, ph));
            assert_eq!((p.set, p.source), (None, ThreadSource::CandleDefault));
        }
    }

    #[test]
    fn a_candle_cpu_mask_is_left_alone() {
        let c = Cpus {
            candle_cpu_mask: true,
            ..cpus(2, 8)
        };
        let p = plan_embed_threads(None, None, None, c);
        assert_eq!((p.set, p.source), (None, ThreadSource::CandleDefault));
        // An explicit count still wins over the mask.
        assert_eq!(plan_embed_threads(None, None, Some("3"), c).set, Some(3));
    }

    #[test]
    fn an_inherited_count_keeps_the_source_its_parent_chose() {
        let p = plan_embed_threads(Some("2"), Some("cpu-limit:2"), None, cpus(2, 8));
        assert_eq!((p.set, p.source), (None, ThreadSource::CpuLimit));
        let p = plan_embed_threads(Some("4"), Some("embed-threads:4"), Some("4"), cpus(2, 8));
        assert_eq!(p.source, ThreadSource::EmbedThreadsEnv);
        assert!(p.warning.is_none());
        let p = plan_embed_threads(Some("4"), Some("junk"), None, cpus(2, 8));
        assert_eq!(p.source, ThreadSource::RayonEnv);
    }

    #[test]
    fn a_marker_whose_value_differs_from_rayon_env_is_not_trusted() {
        let p = plan_embed_threads(Some("6"), Some("cpu-limit:2"), None, cpus(2, 8));
        assert_eq!(p.source, ThreadSource::RayonEnv);
        let p = plan_embed_threads(Some("6"), Some("embed-threads"), None, cpus(2, 8));
        assert_eq!(p.source, ThreadSource::RayonEnv);
    }

    #[test]
    fn a_rayon_env_that_is_not_a_positive_number_counts_as_unset() {
        for bad in ["0", "many", "", "-1"] {
            let p = plan_embed_threads(Some(bad), Some("cpu-limit:2"), None, cpus(2, 8));
            assert_eq!(
                (p.set, p.source),
                (Some(2), ThreadSource::CpuLimit),
                "{bad:?}"
            );
        }
        let p = plan_embed_threads(Some("0"), None, Some("3"), cpus(2, 8));
        assert_eq!((p.set, p.source), (Some(3), ThreadSource::EmbedThreadsEnv));
    }

    #[test]
    fn macos_keeps_candles_choice() {
        let mac = Cpus {
            is_macos: true,
            ..cpus(4, 8)
        };
        let p = plan_embed_threads(None, None, None, mac);
        assert_eq!((p.set, p.source), (None, ThreadSource::CandleDefault));
        assert_eq!(plan_embed_threads(None, None, Some("3"), mac).set, Some(3));
    }

    #[test]
    fn the_log_line_never_prints_an_empty_count() {
        for (rayon, embed, c) in [
            (None, None, cpus(8, 8)),
            (Some(""), None, cpus(8, 8)),
            (Some("x"), None, cpus(2, 8)),
            (Some("3"), None, cpus(2, 8)),
            (None, Some("2"), cpus(2, 8)),
        ] {
            let line = thread_plan_line(&plan_embed_threads(rayon, None, embed, c));
            assert!(!line.contains(": ("), "{line}");
            assert!(line.starts_with("embedding threads: "), "{line}");
        }
        let line = thread_plan_line(&plan_embed_threads(None, None, None, cpus(8, 8)));
        assert!(line.contains("automatic"));
    }
}
