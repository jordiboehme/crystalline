//! Portability lints over the auth store's sources: the SQL it sends has to
//! run unchanged on Turso and on Postgres. They read the source files from
//! disk (every file of the store except its two test modules), so a new file
//! of the store is covered without being listed here.

use std::path::{Path, PathBuf};

/// The files the lints scan: `auth_store.rs` and every file of the
/// `auth_store/` folder except the test modules, this one and `tests.rs`.
fn store_sources() -> Vec<(PathBuf, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut paths = vec![src.join("auth_store.rs")];
    for entry in std::fs::read_dir(src.join("auth_store")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if name.ends_with(".rs") && name != "lint.rs" && name != "tests.rs" {
            paths.push(path);
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let text = std::fs::read_to_string(&p).unwrap();
            (p, text)
        })
        .collect()
}

/// Every string literal of `source` outside `//` comment lines, escapes left
/// as written except a backslash-newline, which joins the lines. A raw
/// string (`r"..."`, `r#"..."#`) is one literal too, taken as written, with
/// the quotes inside it: the scan must not cut it at its first `"`.
fn string_literals(source: &str) -> Vec<String> {
    let code: String = source
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .map(|l| format!("{l}\n"))
        .collect();
    let mut literals = Vec::new();
    let mut chars = code.chars().peekable();
    let mut current: Option<String> = None;
    let mut previous = ' ';
    while let Some(c) = chars.next() {
        if current.is_none() && c == 'r' && !is_word_char(previous) {
            let mut lookahead = chars.clone();
            let mut hashes = 0;
            while lookahead.peek() == Some(&'#') {
                lookahead.next();
                hashes += 1;
            }
            if lookahead.next() == Some('"') {
                chars = lookahead;
                literals.push(raw_body(&mut chars, hashes));
                previous = '"';
                continue;
            }
        }
        previous = c;
        match (&mut current, c) {
            (None, '\'') => {
                // A char literal such as '"' must not open a string.
                if chars.peek() == Some(&'"') {
                    chars.next();
                    chars.next();
                }
            }
            (None, '"') => current = Some(String::new()),
            (Some(lit), '\\') => match chars.next() {
                Some('\n') => {
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                }
                Some(next) => {
                    lit.push('\\');
                    lit.push(next);
                }
                None => {}
            },
            (Some(_), '"') => literals.push(current.take().unwrap()),
            (Some(lit), c) => lit.push(c),
            (None, _) => {}
        }
    }
    literals
}

/// The body of a raw string whose opening `r`, `hashes` times `#` and `"`
/// were read: everything up to a `"` followed by as many `#`.
fn raw_body(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, hashes: usize) -> String {
    let mut body = String::new();
    while let Some(c) = chars.next() {
        if c == '"' {
            let mut lookahead = chars.clone();
            if (0..hashes).all(|_| lookahead.next() == Some('#')) {
                *chars = lookahead;
                return body;
            }
        }
        body.push(c);
    }
    body
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Whether `text` holds `word` as a whole word.
fn contains_word(text: &str, word: &str) -> bool {
    text.match_indices(word).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word_char) && !after.is_some_and(is_word_char)
    })
}

/// Whether `text` holds a SQL keyword followed by whitespace, as a whole word.
fn has_sql_keyword(text: &str) -> bool {
    const KEYWORDS: [&str; 9] = [
        "SELECT", "INSERT", "UPDATE", "DELETE", "CREATE", "WHERE", "FROM", "JOIN", "ON",
    ];
    KEYWORDS.iter().any(|kw| {
        text.match_indices(kw).any(|(at, _)| {
            let before = text[..at].chars().next_back();
            let after = text[at + kw.len()..].chars().next();
            !before.is_some_and(is_word_char) && after.is_some_and(char::is_whitespace)
        })
    })
}

/// The SQL literals of every store source, with the quoted `"user"`
/// identifier and the `'user'` text value taken out.
fn sql_literals_without_quoted_user() -> Vec<String> {
    store_sources()
        .iter()
        .flat_map(|(_, text)| string_literals(text))
        .filter(|l| has_sql_keyword(l))
        .map(|l| {
            l.replace("\\\"user\\\"", "")
                .replace("\"user\"", "")
                .replace("'user'", "")
        })
        .collect()
}

/// `user` is a reserved word on Postgres: every use as a column must be
/// the quoted identifier, which SQLite and Turso read the same way.
#[test]
fn no_bare_user_keyword_in_sql() {
    let literals = sql_literals_without_quoted_user();
    assert!(
        literals.len() > 100,
        "the scan found only {}",
        literals.len()
    );
    for lit in literals {
        assert!(
            !contains_word(&lit, "user"),
            "a bare user keyword in SQL: {lit}"
        );
    }
}

/// `last_insert_rowid` is a SQLite function; an insert reads its id with
/// `RETURNING id` instead. Nothing else in the store may name it either.
#[test]
fn no_last_insert_rowid_in_auth_store() {
    let sources = store_sources();
    assert!(sources.len() >= 2, "the scan found only {}", sources.len());
    for (path, text) in sources {
        assert!(
            !text.contains(&["last_insert", "_rowid"].concat()),
            "last_insert_rowid in {}",
            path.display()
        );
        assert!(
            !text.contains("INSERT OR "),
            "INSERT OR in {}",
            path.display()
        );
    }
}

/// The scan reaches the executor seam too, not only `auth_store.rs`.
#[test]
fn the_lints_scan_the_executor_seam() {
    assert!(
        store_sources()
            .iter()
            .any(|(p, _)| p.ends_with("auth_store/db.rs"))
    );
}

/// A raw string is one literal with its quotes, an ordinary one keeps its
/// escapes, and an `r` ending a word opens nothing.
#[test]
fn the_scan_reads_raw_strings_whole() {
    let source = concat!(
        "let a = r#\"SELECT \"user\" FROM t\"#;\n",
        "let b = r\"WHERE x = 1\";\n",
        "let c = \"plain \\\"q\\\"\";\n",
        "let for_r = bar\"tail\";\n",
    );
    assert_eq!(
        string_literals(source),
        [
            "SELECT \"user\" FROM t",
            "WHERE x = 1",
            "plain \\\"q\\\"",
            "tail",
        ]
    );
}

/// The migrations' raw SQL is scanned too, as one literal per statement
/// block, not cut at its first quoted identifier.
#[test]
fn the_lints_scan_the_raw_migration_sql() {
    let migrations = store_sources()
        .into_iter()
        .find(|(p, _)| p.ends_with("auth_store/pg_migrations.rs"))
        .expect("the migrations file is scanned");
    assert!(
        string_literals(&migrations.1).iter().any(|l| l
            .contains("CREATE TABLE IF NOT EXISTS users (")
            && l.contains("\"user\" TEXT")),
    );
}

/// The keyword filter sees a keyword followed by any whitespace, whole words
/// only.
#[test]
fn the_keyword_filter_matches_whole_words_before_whitespace() {
    assert!(has_sql_keyword("SELECT\n1"));
    assert!(has_sql_keyword("x WHERE\ty = 1"));
    assert!(has_sql_keyword("a JOIN b ON\nc"));
    assert!(!has_sql_keyword("CONDITION x"));
    assert!(!has_sql_keyword("NOT_ON x"));
}

/// Every statement goes through the executor: the store never reaches a
/// connection field of `self` outside the executor itself, `db.rs` and its
/// Postgres arm `pg.rs`.
#[test]
fn auth_store_has_no_direct_conn_access() {
    let needle = ["self", ".conn"].concat();
    for (path, text) in store_sources() {
        if !path.ends_with("auth_store/db.rs") && !path.ends_with("auth_store/pg.rs") {
            assert!(!text.contains(&needle), "{needle} in {}", path.display());
        }
    }
}
