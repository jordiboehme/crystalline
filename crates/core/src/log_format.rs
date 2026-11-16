//! The log format variable, read before the config overlay loads and by
//! crates below the service.

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

/// Whether the environment asks for JSON logs. The service installs the
/// JSON subscriber exactly when this holds, so lower crates can use it to
/// pick how a console line is written.
pub fn env_wants_json() -> bool {
    parse_log_format(std::env::var(LOG_FORMAT_VAR).ok().as_deref()).0 == LogFormat::Json
}
