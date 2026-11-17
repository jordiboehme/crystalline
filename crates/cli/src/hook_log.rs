//! A debug-only log of the raw payloads the harness hooks receive.
//!
//! A developer build appends every payload a hook command reads to the file
//! a variable names (see `imp`), one JSON line per call, so a spike can see
//! which events a harness sends and with which fields. A release build reads
//! no variable and writes nothing: `append` is empty there.

/// Append one line for a hook payload: the verb that read it, the payload's
/// `hook_event_name` (or null) and the payload itself (parsed, or the raw
/// string when it is not JSON). Every error is ignored.
#[cfg(debug_assertions)]
pub(crate) fn append(verb: &str, raw: &str) {
    imp::append(verb, raw);
}

/// A release build logs nothing.
#[cfg(not(debug_assertions))]
#[inline]
pub(crate) fn append(_verb: &str, _raw: &str) {}

/// The debug-only half: `CRYSTALLINE_HOOK_LOG` names the file the lines go to.
#[cfg(debug_assertions)]
mod imp {
    use std::io::Write as _;
    use std::path::Path;

    const VAR: &str = "CRYSTALLINE_HOOK_LOG";

    pub(super) fn append(verb: &str, raw: &str) {
        if let Some(path) = std::env::var_os(VAR) {
            append_to(Path::new(&path), verb, raw);
        }
    }

    pub(super) fn append_to(path: &Path, verb: &str, raw: &str) {
        let payload = serde_json::from_str::<serde_json::Value>(raw)
            .unwrap_or_else(|_| serde_json::Value::String(raw.to_owned()));
        let event = payload
            .get("hook_event_name")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let mut line =
            serde_json::json!({ "verb": verb, "event": event, "payload": payload }).to_string();
        line.push('\n');
        // One write per line: hooks of one event run in parallel processes,
        // and a formatted write in pieces interleaves with theirs.
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    /// Every line that names the log variable sits inside an item under
    /// `#[cfg(debug_assertions)]`, so a release build never reads it.
    #[test]
    fn release_build_never_reads_the_variable() {
        let name = concat!("CRYSTALLINE", "_HOOK_LOG");
        let source = include_str!("hook_log.rs");
        let lines: Vec<&str> = source.lines().collect();
        let mut guarded = vec![false; lines.len()];
        let mut i = 0;
        while i < lines.len() {
            if lines[i].trim() == "#[cfg(debug_assertions)]" {
                // The item runs from the attribute (with its doc lines just
                // above) to the brace that closes its first opening brace.
                let mut start = i;
                while start > 0 && lines[start - 1].trim_start().starts_with("///") {
                    start -= 1;
                }
                let mut depth = 0i32;
                let mut opened = false;
                let mut end = i;
                for (j, line) in lines.iter().enumerate().skip(i) {
                    for c in line.chars() {
                        match c {
                            '{' => {
                                depth += 1;
                                opened = true;
                            }
                            '}' => depth -= 1,
                            _ => {}
                        }
                    }
                    end = j;
                    if opened && depth == 0 {
                        break;
                    }
                }
                for flag in &mut guarded[start..=end] {
                    *flag = true;
                }
                i = end + 1;
            } else {
                i += 1;
            }
        }
        let mut found = 0;
        for (line, guarded) in lines.iter().zip(&guarded) {
            if line.contains(name) {
                found += 1;
                assert!(
                    guarded,
                    "the variable is named outside a debug item: {line}"
                );
            }
        }
        assert!(found > 0, "the variable is named nowhere");
    }

    #[cfg(debug_assertions)]
    #[test]
    fn appends_one_line_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("hooks.jsonl");
        super::imp::append_to(&path, "stop", r#"{"hook_event_name":"Stop","x":1}"#);
        super::imp::append_to(&path, "prompt", "not json");
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
