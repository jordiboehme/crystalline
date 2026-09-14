//! One way for a CLI verb to reach the index, and a guard that keeps it that
//! way.
//!
//! A verb that opens the index database on its own has no answer for the
//! ordinary case on a machine with a daemon: the daemon owns the file, so the
//! open fails and the user reads a lock error instead of their knowledge. The
//! fix is a single helper (`cmd::reach_index`) that asks a running daemon
//! first, opens the index directly when there is none, and otherwise says so
//! in words that name the daemon and a remedy.
//!
//! This guard is a source scan rather than a type-level proof, in the style of
//! the index crate's query-shape guards: the point is the class, not the
//! instances, so a new verb that grows its own opener has to be a deliberate
//! act someone reviews.

use std::path::{Path, PathBuf};

/// The index factory entry points, plus the CLI's own former wrapper around
/// one. A call to any of these is an index open.
const OPENERS: &[&str] = &["open_store", "open_standalone", "open_backend"];

/// The one function allowed to call an opener: the helper itself.
const HELPER: (&str, &str) = ("cmd.rs", "reach_index");

/// Sites allowed to open the index without going through the helper, each
/// with the reason it is not routed. An empty reason is not an exception.
const EXCEPTIONS: &[(&str, &str, &str)] = &[(
    "doctor.rs",
    "run",
    "doctor is the diagnosis of the index rather than a verb that reads it: it \
     already asks the daemon first (file_stamps, collect_orphaned_domains) and \
     falls back to this open only when none answered, and where the helper \
     refuses in one sentence doctor has to keep running and report the reason \
     it composed from the service check it just ran.",
)];

/// Every `.rs` file under the crate's `src/`, relative name and content.
fn cli_sources() -> Vec<(String, String)> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut out = Vec::new();
    collect(&root, &root, &mut out);
    out.sort();
    assert!(
        !out.is_empty(),
        "the guard found no CLI sources to walk under {}",
        root.display()
    );
    out
}

fn collect(root: &Path, dir: &Path, out: &mut Vec<(String, String)>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("reading {dir:?}: {e}"));
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(root, &path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            let name = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            out.push((name, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}

/// The name of the function the line at `index` sits inside: the nearest `fn`
/// item above it. Blunt on purpose - a guard that scans text does not need a
/// parser, and a name it misreads shows up in the failure message.
fn enclosing_fn(lines: &[&str], index: usize) -> String {
    for line in lines[..=index].iter().rev() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        let Some(at) = trimmed.find("fn ") else {
            continue;
        };
        // A real item, not `dyn Fn(` or a word ending in `fn`: whatever
        // precedes the keyword is either nothing or ends in a space.
        let before = &trimmed[..at];
        if !before.is_empty() && !before.ends_with(' ') {
            continue;
        }
        let name: String = trimmed[at + 3..]
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .collect();
        if !name.is_empty() {
            return name;
        }
    }
    "<top level>".to_string()
}

/// No CLI verb opens the index on its own.
#[test]
fn every_index_open_goes_through_the_one_helper() {
    for (_, _, reason) in EXCEPTIONS {
        assert!(
            !reason.trim().is_empty(),
            "an exception without a reason is not an exception"
        );
    }

    let mut offenders = Vec::new();
    let mut excused = Vec::new();
    let mut helper_sites = 0;
    for (file, src) in cli_sources() {
        let lines: Vec<&str> = src.lines().collect();
        for (n, line) in lines.iter().enumerate() {
            let trimmed = line.trim_start();
            if trimmed.starts_with("//") {
                continue;
            }
            // A call, not the declaration of one: `fn open_backend(` is the
            // definition of a wrapper, and the calls inside it are what this
            // guard is about.
            let Some(opener) = OPENERS
                .iter()
                .find(|o| line.contains(&format!("{o}(")) && !line.contains(&format!("fn {o}(")))
            else {
                continue;
            };
            let owner = enclosing_fn(&lines, n);
            if (file.as_str(), owner.as_str()) == HELPER {
                helper_sites += 1;
                continue;
            }
            match EXCEPTIONS
                .iter()
                .position(|(f, fun, _)| *f == file && *fun == owner)
            {
                Some(i) => excused.push(i),
                None => offenders.push(format!("  {file}:{} - {owner}() calls {opener}", n + 1)),
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these CLI paths reach the index on their own rather than through \
         cmd::reach_index, and are on no exception list:\n{}\n\nRoute them \
         through the helper, or add the site to EXCEPTIONS with the reason it \
         cannot be.",
        offenders.join("\n")
    );
    assert!(
        helper_sites > 0,
        "the helper {}::{} opens nothing, so nothing is routed through it",
        HELPER.0,
        HELPER.1
    );
    for (i, (file, owner, _)) in EXCEPTIONS.iter().enumerate() {
        assert!(
            excused.contains(&i),
            "the exception for {file}::{owner}() matches no index open any \
             more; delete the entry rather than leaving a licence nobody uses"
        );
    }
}
