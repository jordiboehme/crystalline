//! The browser address of an engram's page, worked out once per caller.
//!
//! A `crystalline://` address names an engram to an agent; it names nothing to
//! a browser. The page is at `<base>/d/<domain>/e/<permalink>`, and the whole
//! question is what `<base>` is for the caller being answered - which is not a
//! property of the engram and not a property of the instance either, but of
//! how that one caller reaches it.
//!
//! Resolution has three outcomes, and each one is a different thing to say:
//!
//! - **known** - this caller's address for the page is worked out, so the
//!   payload carries `web_url`;
//! - **unresolved** - a page exists but this caller's address for it could not
//!   be worked out, so the payload carries [`UNRESOLVED_NOTE`] instead, which
//!   names the setting that fixes it;
//! - **no page** - nothing here serves the web UI, so there is nothing to
//!   point at and neither key appears. Silence, rather than a note telling
//!   somebody to configure a page they never asked for.
//!
//! Attachment happens at the edge - the MCP router, the REST route, the CLI
//! dispatcher - and never inside the engine. The base is per caller, the
//! payload builders are shared, and a builder that baked one caller's address
//! into a value another caller then reads would be wrong for whoever asked
//! second.

use axum::http::HeaderMap;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use serde_json::{Value, json};

use crate::serving::{HttpBinding, loopback_connect_addr};

/// The one sentence an unresolved outcome carries, naming the setting that
/// answers it. Said once per response, never per row.
pub const UNRESOLVED_NOTE: &str = "no web address could be worked out for this caller: set service.public_url to the address people open Fluid at";

/// Exactly what JavaScript's `encodeURIComponent` leaves alone: the
/// alphanumerics and `- _ . ! ~ * ' ( )`. Fluid encodes with that function,
/// and the two builders have to agree byte for byte.
const COMPONENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// What resolution answered for one caller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebBase {
    /// The origin this caller opens the page at.
    Known(String),
    /// A page exists but this caller's address for it could not be worked out.
    Unresolved,
    /// Nothing serves the web UI; there is no page to point at.
    NoPage,
}

/// The rule that answers what this instance is called, as the engine needs
/// it: the configured public address, and the base for one HTTP caller.
///
/// The JSON API's `OriginRule` implements it, and the daemon installs that
/// rule on the engine once it has built the HTTP surface. A trait rather than
/// the type, because the rule lives with the request handling it shares with
/// OAuth, above the engine, while the engine is what every surface asks.
pub trait WebOrigin: Send + Sync {
    /// `service.public_url` as the rule was built with it.
    fn public_url(&self) -> Option<&str>;
    /// The base for the HTTP caller these headers came from.
    fn request_base(&self, headers: &HeaderMap, ui_enabled: bool) -> WebBase;
}

/// The base for a caller on this machine: a stdio bridge, the daemon's own
/// socket, the CLI through the control socket.
///
/// The configured address wins whatever the bind says. Otherwise the bind is
/// the answer, rewritten through [`loopback_connect_addr`] so a wildcard is
/// never handed out as if it were an address.
pub fn local_base(
    public_url: Option<&str>,
    http: Option<&HttpBinding>,
    ui_enabled: bool,
) -> WebBase {
    if let Some(url) = public_url {
        return WebBase::Known(url.to_string());
    }
    match http {
        Some(HttpBinding::Bound(addr)) if ui_enabled => {
            WebBase::Known(format!("http://{}", loopback_connect_addr(addr)))
        }
        _ => WebBase::NoPage,
    }
}

/// One path segment, encoded the way Fluid encodes one.
pub fn encode_segment(value: &str) -> String {
    utf8_percent_encode(value, COMPONENT).to_string()
}

/// A permalink, encoded per segment: its slashes are path separators in the
/// route and stay slashes, everything inside a segment is encoded.
pub fn encode_permalink(permalink: &str) -> String {
    permalink
        .split('/')
        .map(encode_segment)
        .collect::<Vec<_>>()
        .join("/")
}

/// The page address of one engram.
pub fn engram_url(base: &str, domain: &str, permalink: &str) -> String {
    format!(
        "{base}/d/{}/e/{}",
        encode_segment(domain),
        encode_permalink(permalink)
    )
}

/// The one shape a list result carries instead of a URL per row, for an agent
/// to fill in from a hit's own `domain` and `permalink`.
pub fn url_template(base: &str) -> String {
    format!("{base}/d/{{domain}}/e/{{permalink}}")
}

/// Put `web_url` (or the note) on the object `value` is, read off its own
/// `domain` and `permalink`. A value that is not that shape is left alone.
pub fn attach_engram_url(value: &mut Value, base: &WebBase) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    let (Some(domain), Some(permalink)) = (
        obj.get("domain")
            .and_then(Value::as_str)
            .map(str::to_string),
        obj.get("permalink")
            .and_then(Value::as_str)
            .map(str::to_string),
    ) else {
        return;
    };
    match base {
        WebBase::Known(b) => {
            obj.insert("web_url".into(), json!(engram_url(b, &domain, &permalink)));
        }
        WebBase::Unresolved => {
            obj.insert("web_url_note".into(), json!(UNRESOLVED_NOTE));
        }
        WebBase::NoPage => {}
    }
}

/// Put `web_url_template` (or the note) on the object `value` is.
pub fn attach_template(value: &mut Value, base: &WebBase) {
    let Some(obj) = value.as_object_mut() else {
        return;
    };
    match base {
        WebBase::Known(b) => {
            obj.insert("web_url_template".into(), json!(url_template(b)));
        }
        WebBase::Unresolved => {
            obj.insert("web_url_note".into(), json!(UNRESOLVED_NOTE));
        }
        WebBase::NoPage => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound(addr: &str) -> HttpBinding {
        HttpBinding::Bound(addr.to_string())
    }

    #[test]
    fn a_configured_base_wins_over_the_bind() {
        let configured = Some("https://kb.example.com");
        assert_eq!(
            local_base(configured, Some(&bound("0.0.0.0:7411")), true),
            WebBase::Known("https://kb.example.com".to_string())
        );
        assert_eq!(
            local_base(configured, None, true),
            WebBase::Known("https://kb.example.com".to_string()),
            "a daemon with no HTTP surface of its own still knows where the pages are"
        );
        assert_eq!(
            local_base(configured, Some(&bound("0.0.0.0:7411")), false),
            WebBase::Known("https://kb.example.com".to_string()),
            "and the key is exactly how an API-only daemon names the front in front of it"
        );
    }

    #[test]
    fn a_wildcard_bind_is_handed_out_as_loopback() {
        for wildcard in ["0.0.0.0:7411", "[::]:7411"] {
            assert_eq!(
                local_base(None, Some(&bound(wildcard)), true),
                WebBase::Known("http://127.0.0.1:7411".to_string()),
                "{wildcard} is a bind, not an address anything can open"
            );
        }
    }

    #[test]
    fn an_explicit_bind_passes_through() {
        assert_eq!(
            local_base(None, Some(&bound("192.168.1.5:7411")), true),
            WebBase::Known("http://192.168.1.5:7411".to_string())
        );
        assert_eq!(
            local_base(None, Some(&bound("[::1]:7411")), true),
            WebBase::Known("http://[::1]:7411".to_string())
        );
    }

    #[test]
    fn no_http_surface_means_no_page() {
        assert_eq!(local_base(None, None, true), WebBase::NoPage);
        assert_eq!(
            local_base(None, Some(&HttpBinding::Off), true),
            WebBase::NoPage
        );
        assert_eq!(
            local_base(None, Some(&HttpBinding::Unrecorded), true),
            WebBase::NoPage
        );
    }

    #[test]
    fn a_ui_that_is_off_is_no_page_unless_a_public_url_names_the_front() {
        assert_eq!(
            local_base(None, Some(&bound("127.0.0.1:7411")), false),
            WebBase::NoPage,
            "this daemon's own address serves no pages"
        );
        assert_eq!(
            local_base(Some("https://kb.example.com"), None, false),
            WebBase::Known("https://kb.example.com".to_string())
        );
    }

    #[test]
    fn the_engram_url_encodes_the_domain_whole_and_the_permalink_per_segment() {
        assert_eq!(
            engram_url("http://127.0.0.1:7411", "team notes", "notes/deep/gamma"),
            "http://127.0.0.1:7411/d/team%20notes/e/notes/deep/gamma",
            "a permalink's slashes are path separators and stay slashes"
        );
        assert_eq!(
            engram_url("https://kb.example.com", "büro", "a/b"),
            "https://kb.example.com/d/b%C3%BCro/e/a/b"
        );
        assert_eq!(
            engram_url("x", "a-b_c.d!e~f*g'h(i)", "p"),
            "x/d/a-b_c.d!e~f*g'h(i)/e/p",
            "the nine characters encodeURIComponent keeps survive here too"
        );
    }

    /// The one set of bytes both builders are pinned to. Fluid's own test
    /// reads the same file, so a change on either side that the other did not
    /// make turns one of the two red.
    #[test]
    fn the_fixture_cases_round_trip_through_the_builder() {
        let cases: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/web-url/cases.json"))
                .unwrap();
        let urls = cases["urls"].as_array().unwrap();
        assert!(!urls.is_empty(), "the fixture has to carry cases");
        for case in urls {
            let built = engram_url(
                case["base"].as_str().unwrap(),
                case["domain"].as_str().unwrap(),
                case["permalink"].as_str().unwrap(),
            );
            assert_eq!(built, case["web_url"].as_str().unwrap(), "{case}");
        }
    }

    #[test]
    fn attach_puts_the_url_the_note_or_nothing() {
        let subject = || json!({"domain": "eng", "permalink": "alpha"});

        let mut known = subject();
        attach_engram_url(
            &mut known,
            &WebBase::Known("http://127.0.0.1:7411".to_string()),
        );
        assert_eq!(known["web_url"], "http://127.0.0.1:7411/d/eng/e/alpha");
        assert!(known.get("web_url_note").is_none());

        let mut unresolved = subject();
        attach_engram_url(&mut unresolved, &WebBase::Unresolved);
        assert_eq!(unresolved["web_url_note"], UNRESOLVED_NOTE);
        assert!(unresolved.get("web_url").is_none());

        let mut no_page = subject();
        attach_engram_url(&mut no_page, &WebBase::NoPage);
        assert_eq!(no_page, subject(), "nothing is said where there is no page");

        let mut not_an_engram = json!({"domain": "eng"});
        attach_engram_url(
            &mut not_an_engram,
            &WebBase::Known("http://127.0.0.1:7411".to_string()),
        );
        assert_eq!(
            not_an_engram,
            json!({"domain": "eng"}),
            "a value that is not an engram is left alone"
        );
    }

    #[test]
    fn attach_template_puts_the_quoted_shape() {
        let mut known = json!({"engrams": []});
        attach_template(
            &mut known,
            &WebBase::Known("http://127.0.0.1:7411".to_string()),
        );
        assert_eq!(
            known["web_url_template"],
            "http://127.0.0.1:7411/d/{domain}/e/{permalink}"
        );

        let mut unresolved = json!({"engrams": []});
        attach_template(&mut unresolved, &WebBase::Unresolved);
        assert_eq!(unresolved["web_url_note"], UNRESOLVED_NOTE);

        let mut no_page = json!({"engrams": []});
        attach_template(&mut no_page, &WebBase::NoPage);
        assert_eq!(no_page, json!({"engrams": []}));
    }
}
