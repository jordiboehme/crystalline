//! Every first segment the server or Fluid answers at is on the reserved
//! list, so a prefix that would be stripped twice behind a stripping proxy is
//! refused when it is set. Reads `fluid/` at run time; a checkout without it
//! skips the test rather than failing the build.

use std::path::PathBuf;

use crystalline_core::base::RESERVED_FIRST_SEGMENTS;

fn fluid() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fluid")
}

fn reserved(segment: &str) -> bool {
    RESERVED_FIRST_SEGMENTS
        .iter()
        .any(|r| r.eq_ignore_ascii_case(segment))
}

/// Every `path=` of a route element, in all three spellings JSX allows for a
/// string: `path="/x"`, `path='/x'` and `path={"/x"}` or `` path={`/x`} ``.
/// A `path={...}` that is not one string literal is an error naming the
/// line, so a route written as an expression can never slip past the guard.
fn route_paths(source: &str) -> Result<Vec<String>, String> {
    let mut paths = Vec::new();
    for (at, _) in source.match_indices("path=") {
        let line = source[..at].lines().count();
        let rest = &source[at + "path=".len()..];
        let literal = |quote: char, body: &str| -> Option<String> {
            let body = body.strip_prefix(quote)?;
            body.find(quote).map(|end| body[..end].to_string())
        };
        let found = match rest.chars().next() {
            Some(q @ ('"' | '\'')) => literal(q, rest),
            Some('{') => {
                let inner = rest[1..].trim_start();
                let value = match inner.chars().next() {
                    Some(q @ ('"' | '\'' | '`')) => literal(q, inner),
                    _ => None,
                };
                match value {
                    Some(v) if !v.contains("${") => {
                        let after = &inner[v.len() + 2..];
                        after.trim_start().starts_with('}').then_some(v)
                    }
                    _ => None,
                }
                .ok_or_else(|| {
                    format!(
                        "routes.tsx line {line}: a route path written as an expression is not \
                         read by this guard; write it as path=\"/...\""
                    )
                })
                .map(Some)?
            }
            _ => None,
        };
        if let Some(path) = found {
            paths.push(path);
        }
    }
    Ok(paths)
}

#[test]
fn every_top_level_route_of_fluid_is_reserved() {
    let Ok(routes) = std::fs::read_to_string(fluid().join("src/routes.tsx")) else {
        eprintln!("note: skipping, this checkout does not carry fluid/");
        return;
    };
    let paths = route_paths(&routes).unwrap_or_else(|e| panic!("{e}"));
    let mut seen = 0;
    for path in paths {
        let Some(rest) = path.strip_prefix('/') else {
            continue;
        };
        let end = rest.find(['/', '*', ':']).unwrap_or(rest.len());
        let segment = &rest[..end];
        if segment.is_empty() {
            continue;
        }
        seen += 1;
        assert!(
            reserved(segment),
            "the Fluid route /{segment} is not in RESERVED_FIRST_SEGMENTS: add it there, \
             or put the route under an existing first segment"
        );
    }
    assert!(seen >= 10, "the route table was not read: {seen} routes");
}

#[test]
fn every_root_level_file_of_the_bundle_is_reserved() {
    let Ok(entries) = std::fs::read_dir(fluid().join("public")) else {
        eprintln!("note: skipping, this checkout does not carry fluid/public");
        return;
    };
    for entry in entries {
        let name = entry.unwrap().file_name().to_string_lossy().to_string();
        assert!(
            reserved(&name),
            "fluid/public/{name} is not in RESERVED_FIRST_SEGMENTS"
        );
    }
}

#[test]
fn the_server_first_segments_are_reserved() {
    for segment in ["api", "assets", "health", ".well-known"] {
        assert!(reserved(segment), "{segment}");
    }
}

#[test]
fn every_spelling_of_a_route_path_is_read_and_an_expression_is_refused() {
    let source = "<Route path=\"/a\" />\n<Route path='/b' />\n<Route path={\"/c\"} />\n\
                  <Route path={`/d/x`} />\n<Route path=\"*\" />";
    assert_eq!(
        route_paths(source).unwrap(),
        ["/a", "/b", "/c", "/d/x", "*"]
    );
    let err = route_paths("<Route\n path={base + \"/e\"} />").unwrap_err();
    assert!(err.contains("line 2"), "{err}");
    assert!(route_paths("<Route path={`/f/${x}`} />").is_err());
}
