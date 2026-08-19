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
//! There are two numbers to keep now, not one. The archive preview and import
//! carry a whole domain rather than one document, so the daemon gives those
//! two routes their own [`crystalline_service::rest::ARCHIVE_BODY_BYTES`]
//! (64 MiB) and the template gives the same paths their own regex location
//! with a matching directive. A proxy that kept the server-level 10 MiB for
//! them would refuse an archive the deployment behind it had just produced,
//! and the failure would look like a Fluid bug rather than a proxy one.
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

/// The value of the `client_max_body_size` directive on `line`, in bytes.
fn directive_bytes(line: &str) -> usize {
    let value = line
        .trim()
        .trim_start_matches("client_max_body_size")
        .trim()
        .trim_end_matches(';')
        .trim();
    parse_nginx_size(value)
        .unwrap_or_else(|| panic!("could not parse client_max_body_size value {value:?}"))
}

/// The template, or `None` on a checkout that does not carry `fluid/`.
fn template() -> Option<String> {
    let path = template_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => Some(text),
        Err(_) => {
            eprintln!(
                "note: skipping the nginx body cap guard ({} not found); \
                 this checkout does not carry fluid/",
                path.display()
            );
            None
        }
    }
}

#[test]
fn nginx_body_cap_is_at_least_the_daemons() {
    let Some(template) = template() else {
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

    let bytes = directive_bytes(directive);

    assert!(
        bytes >= crystalline_service::rest::MAX_BODY_BYTES,
        "fluid/nginx.conf.template's client_max_body_size ({bytes} bytes) is below \
         crystalline_service::rest::MAX_BODY_BYTES ({} bytes); the proxy would refuse \
         a body the daemon behind it accepts",
        crystalline_service::rest::MAX_BODY_BYTES
    );
}

/// The archive routes have their own, larger directive, and it is inside the
/// location that matches them.
///
/// Two assertions rather than one, because the split is the point: the archive
/// location must cover `ARCHIVE_BODY_BYTES`, and the server-level value must
/// NOT have been raised to cover it - a proxy that let every route take 64 MiB
/// would undo the reason the daemon keeps the two numbers apart.
#[test]
fn the_archive_location_carries_the_archive_cap() {
    let Some(template) = template() else {
        return;
    };
    let archive_cap = crystalline_service::rest::ARCHIVE_BODY_BYTES;

    let mut lines = template.lines().map(str::trim);
    lines
        .find(|line| line.starts_with("location ") && line.contains("/archive"))
        .unwrap_or_else(|| {
            panic!(
                "fluid/nginx.conf.template has no location matching the archive routes; \
                 the server-level client_max_body_size would then cap an archive import \
                 at crystalline_service::rest::MAX_BODY_BYTES ({} bytes) while the daemon \
                 behind it accepts ARCHIVE_BODY_BYTES ({archive_cap} bytes). Add a regex \
                 location for ^/api/v1/domains/[^/]+/archive with its own directive.",
                crystalline_service::rest::MAX_BODY_BYTES
            )
        });
    // The block's own directive: everything up to the closing brace of the
    // location, so a directive further down the file cannot stand in for it.
    let directive = lines
        .take_while(|line| *line != "}")
        .find(|line| line.starts_with("client_max_body_size"))
        .unwrap_or_else(|| {
            panic!(
                "the archive location in fluid/nginx.conf.template sets no \
                 client_max_body_size of its own, so it inherits the server-level one \
                 and refuses a body the daemon accepts ({archive_cap} bytes)"
            )
        });
    let bytes = directive_bytes(directive);
    assert!(
        bytes >= archive_cap,
        "the archive location's client_max_body_size ({bytes} bytes) is below \
         crystalline_service::rest::ARCHIVE_BODY_BYTES ({archive_cap} bytes); the proxy \
         would refuse an archive the deployment behind it just produced"
    );

    let server_level = template
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("client_max_body_size"))
        .expect("the server-level directive is asserted by the test above");
    assert!(
        directive_bytes(server_level) < archive_cap,
        "the server-level client_max_body_size covers the archive cap, so every route \
         behind this proxy accepts an archive-sized body; the split exists so only the \
         two archive routes do"
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
