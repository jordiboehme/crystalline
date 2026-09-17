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
//! act someone reviews. Three ways around a scan of call sites are covered
//! too, because each is a rename away: importing an opener under another name,
//! constructing a concrete store and so skipping the factory entirely, and
//! importing either from the index crate at the top of a file.
//!
//! It scans `crates/cli/src` and stops at that crate's boundary, which is not
//! the whole of the tool: the data verbs (`search`, `read`, `context`,
//! `recent`, `evolve`, `vocabulary`) reach the index through
//! `crystalline_service::client`, whose standalone fallback opens it there and
//! is invisible to this scan. That path says the same sentence by calling the
//! same composer (`instance::index_unreachable_words`) rather than by being
//! guarded here; a verb moved into the service crate therefore leaves this
//! guard's sight, and keeping the wording one function is what holds it
//! together.

use std::path::{Path, PathBuf};

/// The index factory entry points, plus the CLI's own former wrapper around
/// one. A call to any of these is an index open.
const OPENERS: &[&str] = &["open_store", "open_standalone", "open_backend"];

/// The concrete store types. Both are `pub use`d from `crystalline_index`, so
/// a direct constructor call opens an index without touching an opener's name
/// at all; any mention of one inside the CLI is an opening in disguise.
const STORE_TYPES: &[&str] = &["TursoStore", "PostgresStore"];

/// The one function allowed to call an opener: the helper itself.
const HELPER: (&str, &str) = ("cmd.rs", "reach_index");

/// Sites allowed to open the index without going through the helper: the
/// file, the function, how many openings that function is licensed for, and
/// the reason. The count is part of the licence - a second opener grown inside
/// the same function is a new decision and must not inherit a reason written
/// for the first. An empty reason is not an exception.
const EXCEPTIONS: &[(&str, &str, usize, &str)] = &[(
    "doctor.rs",
    "run",
    1,
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

/// One place a file reaches the index: the line, the function it sits in and
/// what was found there.
struct Opening {
    line: usize,
    owner: String,
    what: String,
}

/// Every index opening one source file performs, however it is spelled.
///
/// The scan stops at the file's first `#[cfg(test)]`. What this guards is the
/// command paths a person runs, and a test that builds a store in a temp
/// directory is not one of those; every test module in this crate is the
/// trailing block of its file, so cutting there costs no coverage of the code
/// that ships. A test module written in the middle of a file would hide what
/// follows it, which is the price of a scan that counts no braces.
fn openings_in(src: &str) -> Vec<Opening> {
    let all: Vec<&str> = src.lines().collect();
    let end = all
        .iter()
        .position(|l| l.trim_start().starts_with("#[cfg(test)]"))
        .unwrap_or(all.len());
    let lines = &all[..end];
    let mut found = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        // An import first: `use crystalline_index::open_store as open_idx;`
        // renames the thing a call-site scan looks for, and importing a
        // concrete store type is the same move by another route. Either is
        // reported wherever it appears, since a `use` sits at the top of a
        // file and belongs to no function.
        if trimmed.starts_with("use ")
            && let Some(name) = OPENERS
                .iter()
                .chain(STORE_TYPES)
                .find(|name| line.contains(**name))
        {
            found.push(Opening {
                line: n + 1,
                owner: "<an import>".to_string(),
                what: format!("imports {name}"),
            });
            continue;
        }
        // Then a call. Not the declaration of one: `fn open_backend(` is the
        // definition of a wrapper, and the calls inside it are the point.
        let what = OPENERS
            .iter()
            .find(|o| line.contains(&format!("{o}(")) && !line.contains(&format!("fn {o}(")))
            .map(|o| format!("calls {o}"))
            .or_else(|| {
                STORE_TYPES
                    .iter()
                    .find(|t| line.contains(&format!("{t}::")))
                    .map(|t| format!("constructs {t} directly"))
            });
        let Some(what) = what else {
            continue;
        };
        found.push(Opening {
            line: n + 1,
            owner: enclosing_fn(lines, n),
            what,
        });
    }
    found
}

/// No CLI verb opens the index on its own.
#[test]
fn every_index_open_goes_through_the_one_helper() {
    for (_, _, licensed, reason) in EXCEPTIONS {
        assert!(
            !reason.trim().is_empty(),
            "an exception without a reason is not an exception"
        );
        assert!(*licensed > 0, "an exception licensing nothing is noise");
    }

    let mut offenders = Vec::new();
    let mut excused = vec![0usize; EXCEPTIONS.len()];
    let mut helper_sites = 0;
    for (file, src) in cli_sources() {
        for opening in openings_in(&src) {
            let Opening { line, owner, what } = opening;
            if (file.as_str(), owner.as_str()) == HELPER {
                helper_sites += 1;
                continue;
            }
            match EXCEPTIONS
                .iter()
                .position(|(f, fun, _, _)| *f == file && *fun == owner)
            {
                Some(i) => excused[i] += 1,
                None => offenders.push(format!("  {file}:{line} - {owner}() {what}")),
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
    for (i, (file, owner, licensed, _)) in EXCEPTIONS.iter().enumerate() {
        assert_eq!(
            excused[i], *licensed,
            "the exception for {file}::{owner}() is written for {licensed} index \
             opening(s) and that function now performs {}; a new one there needs \
             its own reason, and a vanished one needs the entry deleted",
            excused[i]
        );
    }
}

/// The three ways around a call-site scan are seen.
///
/// Without this the hardening would be untestable from the tree itself: the
/// CLI contains none of these spellings, so the scan would report zero either
/// way and nobody could tell a working detector from a dead one.
#[test]
fn an_aliased_import_and_a_direct_store_constructor_are_flagged() {
    let source = "\
use crystalline_index::open_store as open_idx;
use crystalline_index::TursoStore;

async fn sneaky() {
    let a = open_idx(&cfg.database(), None, false).await;
    let b = TursoStore::open(&path).await;
}
";
    let found = openings_in(source);
    let described: Vec<String> = found
        .iter()
        .map(|o| format!("{}:{} {}", o.owner, o.line, o.what))
        .collect();

    assert!(
        described.iter().any(|d| d.contains("imports open_store")),
        "an aliased import of an opener must be flagged: {described:?}"
    );
    assert!(
        described.iter().any(|d| d.contains("imports TursoStore")),
        "importing a concrete store must be flagged: {described:?}"
    );
    assert!(
        described
            .iter()
            .any(|d| d.contains("sneaky") && d.contains("constructs TursoStore directly")),
        "a direct store constructor must be flagged, with its function: {described:?}"
    );
    // The aliased call itself is invisible to a name scan, which is exactly
    // why the import is flagged instead; say so rather than pretending.
    assert!(
        !described.iter().any(|d| d.contains("open_idx")),
        "the alias is caught at the import, not the call: {described:?}"
    );
}

/// A store built inside a test module is not a command path.
#[test]
fn a_test_module_is_not_scanned() {
    let source = "\
fn real() {
    let a = crystalline_index::open_store(&cfg, None, false);
}

#[cfg(test)]
mod tests {
    use crystalline_index::TursoStore;
    fn fixture() {
        let s = TursoStore::open(\":memory:\");
    }
}
";
    let found = openings_in(source);
    assert_eq!(found.len(), 1, "only the shipping call counts");
    assert_eq!(found[0].owner, "real");
}

/// An ordinary source file trips nothing.
#[test]
fn a_file_that_never_touches_the_index_is_not_flagged() {
    let source = "\
use std::path::Path;

/// A comment naming open_store and TursoStore, which is prose, not an opening.
fn render(path: &Path) -> String {
    path.display().to_string()
}
";
    assert!(
        openings_in(source).is_empty(),
        "a scan that fires on prose would be ignored within a week"
    );
}
