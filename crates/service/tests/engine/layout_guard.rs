//! Source guards over the walks of a domain's folder.
//!
//! A source scan rather than a behavioural test, for the reason the overlay
//! census guards are one (`crates/service/tests/overlay/overlay_domains.rs`,
//! `call_sites`): the failure is a walk added later that filters by hand,
//! which no request can be written to provoke in advance. A walk that does
//! not ask `DomainLayout` is how a new kind of document leaks into the
//! engram side.

use std::path::{Path, PathBuf};

/// The source roots of every crate whose code walks a domain: the `src`
/// folder of each folder under `crates/`, so a new crate is scanned without
/// anyone remembering to list it here.
///
/// Only `WalkDir` walks are looked for. A recursive `read_dir` walk written
/// by hand is out of scope: no source in the tree does one, and a scan for
/// it would flag every ordinary directory listing.
fn roots() -> Vec<PathBuf> {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut roots: Vec<PathBuf> = std::fs::read_dir(crates)
        .unwrap()
        .flatten()
        .map(|entry| entry.path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    roots.sort();
    roots
}

/// The source text of `text` with comments taken out: whole-line and
/// trailing `//` comments, and `/* */` blocks. A comment that names a type
/// is not a use of it, so neither guard may count one.
fn code_lines(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_block = false;
    for line in text.lines() {
        let mut code = String::new();
        let mut rest = line;
        loop {
            if in_block {
                match rest.find("*/") {
                    Some(end) => {
                        rest = &rest[end + 2..];
                        in_block = false;
                    }
                    None => break,
                }
            } else {
                let line_comment = comment_start(rest, "//");
                let block = comment_start(rest, "/*");
                match (line_comment, block) {
                    (Some(l), Some(b)) if b < l => {
                        code.push_str(&rest[..b]);
                        rest = &rest[b + 2..];
                        in_block = true;
                    }
                    (Some(l), _) => {
                        code.push_str(&rest[..l]);
                        break;
                    }
                    (None, Some(b)) => {
                        code.push_str(&rest[..b]);
                        rest = &rest[b + 2..];
                        in_block = true;
                    }
                    (None, None) => {
                        code.push_str(rest);
                        break;
                    }
                }
            }
        }
        if !code.trim().is_empty() {
            out.push(code);
        }
    }
    out
}

/// Where `marker` starts a comment in `line`: the first occurrence outside a
/// string literal.
fn comment_start(line: &str, marker: &str) -> Option<usize> {
    let mut quotes = 0;
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if quotes % 2 == 0 && line[i..].starts_with(marker) {
            return Some(i);
        }
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' {
            quotes += 1;
        }
    }
    None
}

/// The name a line declares a function under, if it declares one. Strips
/// every qualifier a function can carry, in any order.
fn declared_fn(line: &str) -> Option<&str> {
    let mut rest = line.trim_start();
    loop {
        if let Some(r) = rest.strip_prefix("pub(") {
            rest = r[r.find(')')? + 1..].trim_start();
            continue;
        }
        if let Some(r) = rest.strip_prefix("extern \"") {
            rest = r[r.find('"')? + 1..].trim_start();
            continue;
        }
        let stripped = ["pub ", "const ", "async ", "unsafe ", "extern "]
            .iter()
            .find_map(|q| rest.strip_prefix(q));
        match stripped {
            Some(r) => rest = r.trim_start(),
            None => break,
        }
    }
    let rest = rest.strip_prefix("fn ")?;
    let end = rest.find(|c: char| !c.is_alphanumeric() && c != '_')?;
    Some(&rest[..end])
}

/// The name of the pseudo-function that holds everything before a file's
/// first `fn`: its imports, constants and statics.
const HEAD: &str = "<head>";

/// A file's code split into `(fn name, body text)`, the body running from its
/// declaration to the next declaration. Lines before the first `fn` (or the
/// whole file when it has none) form a `"<head>"` body. Coarse on purpose: a
/// nested `fn` ends the enclosing body early, which only ever makes the guard
/// stricter.
fn split_fns(text: &str) -> Vec<(String, String)> {
    let lines = code_lines(text);
    let starts: Vec<(usize, &str)> = lines
        .iter()
        .enumerate()
        .filter_map(|(i, l)| declared_fn(l).map(|n| (i, n)))
        .collect();
    let head_end = starts.first().map(|(i, _)| *i).unwrap_or(lines.len());
    let mut out = vec![(HEAD.to_string(), lines[..head_end].join("\n"))];
    for (k, (start, name)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|(i, _)| *i).unwrap_or(lines.len());
        out.push((name.to_string(), lines[*start..end].join("\n")));
    }
    out
}

/// Every `.rs` file under the roots as `(crate-relative file, source text)`.
fn sources() -> Vec<(String, String)> {
    let mut out = Vec::new();
    for root in roots() {
        let crates = root.parent().unwrap().parent().unwrap().to_path_buf();
        for entry in walkdir::WalkDir::new(&root).into_iter().flatten() {
            if entry.path().extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let file = entry
                .path()
                .strip_prefix(&crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            out.push((file, std::fs::read_to_string(entry.path()).unwrap()));
        }
    }
    out
}

/// Every function of every source file as `(crate-relative file, fn name,
/// body text)`, plus one `"<head>"` entry per file, comments taken out.
pub(crate) fn fn_bodies() -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (file, text) in sources() {
        for (name, body) in split_fns(&text) {
            out.push((file.clone(), name, body));
        }
    }
    out
}

/// Every function that walks a folder with `WalkDir` either asks
/// `DomainLayout` what each path is, or is listed here with the reason it
/// walks something that is not a domain's own folder.
#[test]
fn every_domain_walk_asks_the_layout() {
    let allowed: &[(&str, &str)] = &[
        // Walks a folder that is about to be imported, before it is a domain.
        ("core/src/import.rs", "collect_entries"),
        // Lists every file under `reflections/`, hidden ones included, for
        // verify's M012: whatever the layout would call each one, any file
        // there is the finding.
        ("core/src/verify/team.rs", "reflection_files"),
        // A `#[cfg(test)]` unit test of the walk error a missing folder gives.
        (
            "remote/src/changes.rs",
            "the_walk_keeps_the_os_code_the_4395_check_reads",
        ),
    ];
    let mut strays: Vec<(String, String)> = fn_bodies()
        .into_iter()
        .filter(|(_, _, body)| body.contains("WalkDir::new("))
        .filter(|(_, _, body)| !body.contains("DomainLayout"))
        .filter(|(file, name, _)| !allowed.contains(&(file.as_str(), name.as_str())))
        .map(|(file, name, _)| (file, name))
        .collect();
    strays.sort();
    assert!(
        strays.is_empty(),
        "these walk a folder and filter by hand; ask crystalline_core::DomainLayout what each path is, \
         or list the walk with the reason it is not a domain's folder: {strays:?}"
    );
    // The two walks that classify without a directory walk.
    for (file, name) in [
        ("index/src/sync.rs", "scan_paths"),
        ("service/src/daemon.rs", "classify_in_root"),
    ] {
        let body = fn_bodies()
            .into_iter()
            .find(|(f, n, _)| f == file && n == name)
            .unwrap_or_else(|| {
                panic!("{file}::{name} is gone; point this guard at what replaced it")
            })
            .2;
        assert!(
            body.contains("DomainLayout"),
            "{file}::{name} classifies paths without DomainLayout"
        );
    }
    for (file, name) in allowed {
        assert!(
            fn_bodies()
                .iter()
                .any(|(f, n, b)| f == file && n == name && b.contains("WalkDir::new(")),
            "{file}::{name} is allow-listed but no longer walks; drop it"
        );
    }
}

/// The Reflection's rows and variants stay in the modules that serve the
/// Reflection: an engram reader that
/// named one would be the one missed filter that leaks a reflection into the
/// engram side. Each needle may appear only in the files listed for it; a
/// task that adds a caller adds its file here in the same commit.
#[test]
fn reflection_variants_stay_in_their_modules() {
    let allowed = reflection_allow_list();
    let mut strays: Vec<(String, String, &str)> = Vec::new();
    for (file, name, body) in fn_bodies() {
        for (needle, files) in allowed {
            if body.contains(needle) && !files.contains(&file.as_str()) {
                strays.push((file.clone(), name.clone(), needle));
            }
        }
    }
    strays.sort();
    strays.dedup();
    assert!(
        strays.is_empty(),
        "these name a Reflection row or variant outside the modules that serve the Reflection: {strays:?}"
    );
}

/// Each needle of the Reflection guard with the files that may name it.
fn reflection_allow_list() -> &'static [(&'static str, &'static [&'static str])] {
    &[
        (
            "list_reflections(",
            &[
                "index/src/store.rs",
                "index/src/turso/mod.rs",
                "index/src/postgres/mod.rs",
                // The read connections' wrapper of every store read, which
                // hands each call through unchanged.
                "index/src/reader.rs",
                "engine/src/engine/reflect.rs",
                "engine/src/engine/reflection.rs",
                "engine/src/engine/rename_domain.rs",
                "engine/src/engine/threads.rs",
            ],
        ),
        (
            "AssetTable::Reflection",
            &[
                "index/src/turso/mod.rs",
                "index/src/postgres/mod.rs",
                "index/src/sync.rs",
                "engine/src/engine/asset_home.rs",
                // The host-lock fence of a Reflection file's row.
                "engine/src/engine/fence.rs",
            ],
        ),
        (
            "RoomDoc::Reflection",
            &[
                "engine/src/collab/session.rs",
                "engine/src/engine/reflection.rs",
                "engine/src/engine/move_.rs",
                // The automation status asks whether a person has a page
                // open, so its update is not due while they type.
                "engine/src/engine/reflect.rs",
            ],
        ),
        (
            "AssetHome::Reflection",
            &[
                "engine/src/engine/asset_home.rs",
                "engine/src/engine/reflection_assets.rs",
                "engine/src/engine/reflect.rs",
            ],
        ),
        (
            "ChunkTable::Reflection",
            &[
                "index/src/store.rs",
                "index/src/turso/mod.rs",
                "index/src/postgres/mod.rs",
                // These three count reflection chunks for status, doctor and
                // the tick; none of them reads a vector.
                "engine/src/engine/sync.rs",
                "cli/src/cmd.rs",
                "cli/src/doctor.rs",
            ],
        ),
        // R53's funnel: a reflection row written anywhere else would have no
        // chunks.
        (".upsert_reflection(", &["index/src/reflection.rs"]),
    ]
}

/// The allow-list cannot go stale: every file it grants a needle still names
/// that needle in code. An entry whose file stopped naming it would leave
/// the guard blind there, and a later breach in that file would pass.
#[test]
fn reflection_allow_list_names_only_files_that_still_name_the_needle() {
    let code: std::collections::HashMap<String, String> = sources()
        .into_iter()
        .map(|(file, text)| (file, code_lines(&text).join("\n")))
        .collect();
    let mut stale: Vec<(&str, &str)> = Vec::new();
    for (needle, files) in reflection_allow_list() {
        for file in *files {
            match code.get(*file) {
                Some(text) if text.contains(needle) => {}
                _ => stale.push((needle, file)),
            }
        }
    }
    assert!(
        stale.is_empty(),
        "these allow-list entries no longer name their needle; drop them: {stale:?}"
    );
}

#[test]
fn declared_fn_sees_every_qualifier() {
    for line in [
        "fn a() {}",
        "pub fn a() {}",
        "pub(crate) fn a() {}",
        "pub(super) fn a() {}",
        "pub(in crate::engine) fn a() {}",
        "pub(self) async fn a() {}",
        "const fn a() {}",
        "pub const fn a() {}",
        "unsafe fn a() {}",
        "pub unsafe fn a() {}",
        "extern \"C\" fn a() {}",
        "pub unsafe extern \"C\" fn a() {}",
        "pub(crate) const unsafe fn a() {}",
        "    pub async fn a() {}",
    ] {
        assert_eq!(declared_fn(line), Some("a"), "{line}");
    }
    assert_eq!(declared_fn("let fn_x = 1;"), None);
    assert_eq!(declared_fn("pub struct A;"), None);
}

#[test]
fn a_comment_naming_the_layout_does_not_count() {
    let text = "fn walk() {\n    // DomainLayout would tell us\n    /// DomainLayout\n    /* DomainLayout\n       DomainLayout */\n    let w = WalkDir::new(p); // DomainLayout\n}\n";
    let fns = split_fns(text);
    let body = &fns.iter().find(|(n, _)| n == "walk").unwrap().1;
    assert!(body.contains("WalkDir::new("), "{body}");
    assert!(!body.contains("DomainLayout"), "{body}");
    let real = split_fns("fn walk() {\n    let l = DomainLayout::of(p);\n}\n");
    assert!(real[1].1.contains("DomainLayout"));
    // A `//` inside a string is not a comment.
    let url = code_lines("let u = \"http://x\"; let l = DomainLayout::new();");
    assert!(url[0].contains("DomainLayout"), "{url:?}");
}

#[test]
fn the_file_head_is_scanned() {
    let fns =
        split_fns("use x::ChunkTable::Reflection;\nstatic W: u8 = WalkDir::new(p);\n\nfn a() {}\n");
    let head = &fns.iter().find(|(n, _)| n == HEAD).unwrap().1;
    assert!(head.contains("ChunkTable::Reflection"));
    assert!(head.contains("WalkDir::new("));
    assert!(
        !fns.iter()
            .find(|(n, _)| n == "a")
            .unwrap()
            .1
            .contains("WalkDir")
    );
    // A file with no fn at all is all head.
    let none = split_fns("const A: u8 = 1;\n");
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].0, HEAD);
}

#[test]
fn every_crate_source_folder_is_scanned() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let scanned = roots();
    let mut found = 0;
    for entry in std::fs::read_dir(crates).unwrap().flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            found += 1;
            assert!(scanned.contains(&src), "{} is not scanned", src.display());
        }
    }
    assert!(found >= 8, "only {found} crate source folders found");
    let files: Vec<String> = fn_bodies().into_iter().map(|(f, _, _)| f).collect();
    for krate in [
        "core", "index", "identity", "engine", "rest", "remote", "service", "cli",
    ] {
        assert!(
            files
                .iter()
                .any(|f| f.starts_with(&format!("{krate}/src/"))),
            "no file of {krate} was scanned"
        );
    }
}
