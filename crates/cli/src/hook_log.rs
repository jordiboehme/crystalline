//! A debug-only log of the raw payloads the harness hooks receive.
//!
//! A developer build appends every payload a hook command reads to the file
//! a variable names (read in the debug `append` only), one JSON line per
//! call, so a spike can see which events a harness sends and with which
//! fields. A release build reads no variable and writes nothing: `append` is
//! empty there.

#[cfg(debug_assertions)]
use std::io::Write as _;
#[cfg(debug_assertions)]
use std::path::Path;

/// Append one line for a hook payload: the verb that read it, the payload's
/// `hook_event_name` (or null) and the payload itself (parsed, or the raw
/// string when it is not JSON). Every error is ignored. The only place the
/// log variable is read.
#[cfg(debug_assertions)]
pub(crate) fn append(verb: &str, raw: &str) {
    if let Some(path) = std::env::var_os("CRYSTALLINE_HOOK_LOG") {
        append_to(Path::new(&path), verb, raw);
    }
}

/// A release build logs nothing.
#[cfg(not(debug_assertions))]
#[inline]
pub(crate) fn append(_verb: &str, _raw: &str) {}

#[cfg(debug_assertions)]
fn append_to(path: &Path, verb: &str, raw: &str) {
    let payload = serde_json::from_str::<serde_json::Value>(raw)
        .unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
    let event = payload
        .get("hook_event_name")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let mut line =
        serde_json::json!({ "verb": verb, "event": event, "payload": payload }).to_string();
    line.push('\n');
    // One write per line: hooks of one event run in parallel processes, and a
    // formatted write in pieces interleaves with theirs.
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

#[cfg(test)]
mod tests {
    /// The log variable is named only inside the debug-only `append`: from
    /// its `#[cfg(debug_assertions)]` line, directly followed by the
    /// signature, to the first `}` in column 0 after it. A release build
    /// compiles the other `append`, so it never reads the variable.
    #[test]
    fn release_build_never_reads_the_variable() {
        let name = concat!("CRYSTALLINE", "_HOOK_LOG");
        let lines: Vec<&str> = include_str!("hook_log.rs").lines().collect();
        let start = lines
            .windows(2)
            .position(|w| {
                w[0] == "#[cfg(debug_assertions)]"
                    && w[1] == "pub(crate) fn append(verb: &str, raw: &str) {"
            })
            .expect("the debug-only append");
        let end = start
            + lines[start..]
                .iter()
                .position(|l| *l == "}")
                .expect("the end of the debug-only append");
        let named: Vec<usize> = lines
            .iter()
            .enumerate()
            .filter(|(_, l)| l.contains(name))
            .map(|(i, _)| i)
            .collect();
        assert!(!named.is_empty(), "the variable is named nowhere");
        for i in named {
            assert!(
                (start..end).contains(&i),
                "the variable is named outside the debug-only append: {}",
                lines[i]
            );
        }
    }

    #[cfg(debug_assertions)]
    #[test]
    fn appends_one_line_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.jsonl");
        super::append_to(&path, "stop", r#"{"hook_event_name":"Stop","x":1}"#);
        super::append_to(&path, "prompt", "not json");
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["verb"], "stop");
        assert_eq!(lines[0]["event"], "Stop");
        assert_eq!(lines[0]["payload"]["x"], 1);
        assert_eq!(lines[1]["verb"], "prompt");
        assert!(lines[1]["event"].is_null());
        assert_eq!(lines[1]["payload"], "not json");
    }
}
