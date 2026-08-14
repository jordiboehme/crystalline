//! nginx must accept a request body at least as large as the daemon does.
//!
//! `fluid/nginx.conf.template` sits in front of the daemon in the
//! team-server-with-Fluid deployment (see `docs/deployment.md`), and every
//! browser save and every archive import passes through it before the
//! daemon ever sees the body. nginx's own default `client_max_body_size` is
//! 1 MiB; the daemon's own limit is [`crystalline_service::rest::MAX_BODY_BYTES`],
//! 10 MiB. The template sets the directive to a literal `10m` rather than
//! deriving it (an unset `CRYSTALLINE_`-prefixed variable would render an
//! empty directive and nginx would refuse to start), so nothing at build
//! time keeps the two numbers in agreement - a comment beside the directive
//! names this fact, and this test is what actually enforces it.
//!
//! Reads the template at RUNTIME through `CARGO_MANIFEST_DIR` rather than
//! `include_str!`: this crate does not own `fluid/`, and a checkout that
//! does not carry it (or a future split of the two trees) should skip this
//! one test rather than fail to build the whole crate.

use std::path::PathBuf;

fn template_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fluid/nginx.conf.template")
}

/// Parse a `client_max_body_size` value into bytes, the way nginx itself
/// does: a bare number is bytes, and `k`/`m`/`g` (either case) are binary
/// multiples of 1024, not decimal ones. That is exactly why a literal `10m`
/// in the template equals `MAX_BODY_BYTES` (`10 * 1024 * 1024`) precisely
/// rather than approximately.
fn parse_nginx_size(value: &str) -> Option<usize> {
    let value = value.trim();
    let split_at = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let (digits, suffix) = value.split_at(split_at);
    if digits.is_empty() {
        return None;
    }
    let n: usize = digits.parse().ok()?;
    let multiplier: usize = match suffix.to_ascii_lowercase().as_str() {
        "" => 1,
        "k" => 1024,
        "m" => 1024 * 1024,
        "g" => 1024 * 1024 * 1024,
        _ => return None,
    };
    n.checked_mul(multiplier)
}

#[test]
fn nginx_body_cap_is_at_least_the_daemons() {
    let path = template_path();
    let Ok(template) = std::fs::read_to_string(&path) else {
        eprintln!(
            "note: skipping the nginx body cap guard ({} not found); \
             this checkout does not carry fluid/",
            path.display()
        );
        return;
    };

    let directive = template
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("client_max_body_size"))
        .unwrap_or_else(|| {
            panic!(
                "fluid/nginx.conf.template has no client_max_body_size directive; \
                 nginx's own default is 1 MiB, an order of magnitude below \
                 crystalline_service::rest::MAX_BODY_BYTES ({} bytes), so the proxy \
                 would silently cap every save and every archive import to 1 MiB. \
                 Add `client_max_body_size 10m;` at server level, beside the gzip \
                 block.",
                crystalline_service::rest::MAX_BODY_BYTES
            )
        });

    let value = directive
        .trim_start_matches("client_max_body_size")
        .trim()
        .trim_end_matches(';')
        .trim();
    let bytes = parse_nginx_size(value)
        .unwrap_or_else(|| panic!("could not parse client_max_body_size value {value:?}"));

    assert!(
        bytes >= crystalline_service::rest::MAX_BODY_BYTES,
        "fluid/nginx.conf.template's client_max_body_size ({bytes} bytes) is below \
         crystalline_service::rest::MAX_BODY_BYTES ({} bytes); the proxy would refuse \
         a body the daemon behind it accepts",
        crystalline_service::rest::MAX_BODY_BYTES
    );
}

#[test]
fn parses_binary_nginx_size_suffixes() {
    assert_eq!(parse_nginx_size("10m"), Some(10 * 1024 * 1024));
    assert_eq!(parse_nginx_size("10M"), Some(10 * 1024 * 1024));
    assert_eq!(parse_nginx_size("512k"), Some(512 * 1024));
    assert_eq!(parse_nginx_size("1g"), Some(1024 * 1024 * 1024));
    assert_eq!(parse_nginx_size("2048"), Some(2048));
    assert_eq!(parse_nginx_size("10x"), None);
    assert_eq!(parse_nginx_size(""), None);
}
