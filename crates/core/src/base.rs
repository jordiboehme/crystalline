//! The address a Crystalline server is reached at: an origin and an optional
//! path, the "base". One type for both halves, so the server that publishes
//! an address and the client that connects to it agree on every rule: what a
//! path may contain, how it is spelled, how a request under it is stripped and
//! where the OAuth documents of RFC 8414 and RFC 9728 live.

use std::fmt;

/// The first segments the server and Fluid answer at. A prefix may not start
/// with one of them: the daemon accepts a request with and without its prefix,
/// so behind a stripping proxy a prefix `/api` would strip the request for
/// `/api/v1/...` a second time. A new route goes under an existing first
/// segment, so an upgrade never makes a prefix in use invalid.
/// `crates/core/tests/base_routes.rs` checks Fluid's route table against it.
pub const RESERVED_FIRST_SEGMENTS: [&str; 17] = [
    "api",
    "assets",
    "health",
    ".well-known",
    "robots.txt",
    "humans.txt",
    "login",
    "π",
    "d",
    "draft",
    "search",
    "graph",
    "authorize",
    "maintenance",
    "users",
    "settings",
    "profile",
];

/// Why a path cannot be a base path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathProblem {
    /// Two slashes in a row.
    EmptySegment,
    /// A `.` or `..` segment, which a browser resolves away.
    DotSegment(String),
    /// A segment with a character outside lower-case letters, digits, `.`,
    /// `_` and `-`.
    Character(String),
    /// A first segment the server or Fluid answers at.
    Reserved(String),
    /// A query or a fragment after the path.
    QueryOrFragment,
}

impl PathProblem {
    /// The reason, as the end of a sentence that names the setting first.
    pub fn sentence(&self) -> String {
        match self {
            PathProblem::EmptySegment => {
                "its path has an empty segment (two slashes in a row)".to_string()
            }
            PathProblem::DotSegment(segment) => {
                format!("its path has a '{segment}' segment, which a browser resolves away")
            }
            PathProblem::Character(segment) => format!(
                "its path segment '{segment}' may use only lower-case letters, digits, '.', '_' and '-'"
            ),
            PathProblem::Reserved(segment) => format!(
                "its path cannot start with '{segment}': the server answers there itself, \
                 so a proxy that strips the path would send those requests to the wrong place"
            ),
            PathProblem::QueryOrFragment => "it carries a query or a fragment".to_string(),
        }
    }
}

/// The path a server is served under: empty at the root, otherwise one
/// leading slash, no trailing slash, segments of `[a-z0-9._-]`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct BasePath(String);

impl BasePath {
    /// No prefix.
    pub fn root() -> BasePath {
        BasePath(String::new())
    }

    /// A path as written (`""`, `"/"`, `"/crystalline"`, `"/crystalline/"`).
    pub fn parse(path: &str) -> Result<BasePath, PathProblem> {
        if path.is_empty() || path == "/" {
            return Ok(BasePath::root());
        }
        let trimmed = path.strip_suffix('/').unwrap_or(path);
        let Some(rest) = trimmed.strip_prefix('/') else {
            return Err(PathProblem::Character(trimmed.to_string()));
        };
        for (at, segment) in rest.split('/').enumerate() {
            if segment.is_empty() {
                return Err(PathProblem::EmptySegment);
            }
            if segment == "." || segment == ".." {
                return Err(PathProblem::DotSegment(segment.to_string()));
            }
            let allowed = segment.chars().all(|c| {
                c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')
            });
            if !allowed {
                return Err(PathProblem::Character(segment.to_string()));
            }
            if at == 0
                && RESERVED_FIRST_SEGMENTS
                    .iter()
                    .any(|reserved| reserved.eq_ignore_ascii_case(segment))
            {
                return Err(PathProblem::Reserved(segment.to_string()));
            }
        }
        Ok(BasePath(trimmed.to_string()))
    }

    /// The path of an absolute url, read from the text as written: a parser
    /// would resolve `..` and percent-encode what this has to refuse.
    pub fn of_url(url: &str) -> Result<BasePath, PathProblem> {
        let url = url.trim();
        let after_scheme = url.split_once("://").map_or(url, |(_, rest)| rest);
        let path_at = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let tail = &after_scheme[path_at..];
        let (path, extra) = match tail.find(['?', '#']) {
            Some(at) => (&tail[..at], true),
            None => (tail, false),
        };
        if extra {
            return Err(PathProblem::QueryOrFragment);
        }
        BasePath::parse(path)
    }

    /// `""` at the root, `"/crystalline"` under a prefix.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether there is no prefix.
    pub fn is_root(&self) -> bool {
        self.0.is_empty()
    }

    /// The path a request is routed with when it carries the prefix: `/` for
    /// the prefix itself, the rest otherwise. `None` for a request that does
    /// not carry the whole prefix (or for any request at the root), which is
    /// routed unchanged. One strip, never two.
    pub fn strip<'a>(&self, request_path: &'a str) -> Option<&'a str> {
        if self.is_root() {
            return None;
        }
        let rest = request_path.strip_prefix(self.0.as_str())?;
        match rest {
            "" => Some("/"),
            _ if rest.starts_with('/') => Some(rest),
            _ => None,
        }
    }

    /// Whether `path` (a path with an optional query) is the prefix or lies
    /// under it. Everything does at the root.
    pub fn contains(&self, path: &str) -> bool {
        if self.is_root() {
            return true;
        }
        match path.strip_prefix(self.0.as_str()) {
            Some(rest) => rest.is_empty() || rest.starts_with(['/', '?', '#']),
            None => false,
        }
    }

    /// The `Path` attribute of every cookie the server sets or clears.
    pub fn cookie_path(&self) -> &str {
        if self.is_root() { "/" } else { &self.0 }
    }

    /// The value of `<base href>` in the served shell.
    pub fn href(&self) -> String {
        format!("{}/", self.0)
    }
}

/// Why a value cannot be a server's public address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BaseProblem {
    /// Not an absolute url.
    NotAbsolute,
    /// Neither `http` nor `https`.
    Scheme,
    /// No host.
    NoHost,
    /// A user name or password.
    Userinfo,
    /// A path that cannot be a base path.
    Path(PathProblem),
}

impl BaseProblem {
    /// The reason, as the end of a sentence that names the address first.
    pub fn sentence(&self) -> String {
        match self {
            BaseProblem::NotAbsolute => "it is not an absolute url".to_string(),
            BaseProblem::Scheme => "it is not an http or https url".to_string(),
            BaseProblem::NoHost => "it names no host".to_string(),
            BaseProblem::Userinfo => "it carries a user name or password".to_string(),
            BaseProblem::Path(problem) => problem.sentence(),
        }
    }
}

/// A server's public address: its origin (scheme, host, optional port, in
/// the ascii spelling) and its base path.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PublicBase {
    origin: String,
    path: BasePath,
}

impl PublicBase {
    /// An absolute http or https url with an optional path.
    pub fn parse(value: &str) -> Result<PublicBase, BaseProblem> {
        let value = value.trim();
        let url = url::Url::parse(value).map_err(|_| BaseProblem::NotAbsolute)?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(BaseProblem::Scheme);
        }
        if url.host().is_none() {
            return Err(BaseProblem::NoHost);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(BaseProblem::Userinfo);
        }
        let path = BasePath::of_url(value).map_err(BaseProblem::Path)?;
        Ok(PublicBase {
            origin: url.origin().ascii_serialization(),
            path,
        })
    }

    /// An origin derived from a request, which never carries a path.
    pub fn from_origin(origin: &str) -> PublicBase {
        PublicBase {
            origin: origin.trim().trim_end_matches('/').to_string(),
            path: BasePath::root(),
        }
    }

    /// Scheme, host and port.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The base path.
    pub fn path(&self) -> &BasePath {
        &self.path
    }

    /// The absolute url of `path` (which starts with `/`) under the base.
    pub fn join(&self, path: &str) -> String {
        format!("{self}{path}")
    }

    /// The RFC 8414 and RFC 9728 address of a well-known document: the path
    /// inserted after the host, `https://example.com/.well-known/<doc>/crystalline`.
    /// At the root it is the plain root document.
    pub fn well_known(&self, document: &str) -> String {
        format!("{}{document}{}", self.origin, self.path.as_str())
    }
}

impl fmt::Display for PublicBase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.origin, self.path.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_path_and_a_lone_slash_are_the_root() {
        for path in ["", "/"] {
            let base = BasePath::parse(path).unwrap();
            assert!(base.is_root(), "{path:?}");
            assert_eq!(base.as_str(), "");
            assert_eq!(base.cookie_path(), "/");
            assert_eq!(base.href(), "/");
        }
    }

    #[test]
    fn a_path_keeps_one_leading_slash_and_loses_one_trailing_slash() {
        for (written, kept) in [
            ("/crystalline", "/crystalline"),
            ("/crystalline/", "/crystalline"),
            ("/team/kb", "/team/kb"),
            ("/team/kb/", "/team/kb"),
            ("/kb-2_x.y", "/kb-2_x.y"),
        ] {
            let base = BasePath::parse(written).unwrap();
            assert_eq!(base.as_str(), kept, "{written}");
            assert_eq!(base.cookie_path(), kept);
            assert_eq!(base.href(), format!("{kept}/"));
        }
    }

    #[test]
    fn each_unusable_path_is_refused_with_its_own_problem() {
        let cases = [
            ("/a//b", PathProblem::EmptySegment),
            ("//", PathProblem::EmptySegment),
            ("/a/./b", PathProblem::DotSegment(".".into())),
            ("/a/..", PathProblem::DotSegment("..".into())),
            ("/Crystalline", PathProblem::Character("Crystalline".into())),
            ("/a~b", PathProblem::Character("a~b".into())),
            ("/a%20b", PathProblem::Character("a%20b".into())),
            ("/π", PathProblem::Character("π".into())),
            ("/a b", PathProblem::Character("a b".into())),
            ("/a\\b", PathProblem::Character("a\\b".into())),
            ("/api", PathProblem::Reserved("api".into())),
            ("/assets/x", PathProblem::Reserved("assets".into())),
            ("/.well-known", PathProblem::Reserved(".well-known".into())),
            ("/d", PathProblem::Reserved("d".into())),
            ("/robots.txt", PathProblem::Reserved("robots.txt".into())),
        ];
        for (path, problem) in cases {
            assert_eq!(BasePath::parse(path), Err(problem), "{path}");
        }
        assert!(
            BasePath::parse("/kb/api").is_ok(),
            "only the first segment is reserved"
        );
    }

    #[test]
    fn a_segment_outside_the_allowed_characters_is_refused() {
        // `~` would share a host folder with `/` (`a~b` and `a/b`), and upper
        // case would share one on a file system that folds case.
        for path in ["/a~b", "/KB", "/Kb/x"] {
            let problem = BasePath::parse(path).unwrap_err();
            assert!(
                problem.sentence().contains("lower-case letters, digits"),
                "{path}: {}",
                problem.sentence()
            );
        }
    }

    #[test]
    fn the_dot_check_reads_the_text_as_written() {
        assert_eq!(
            BasePath::of_url("https://x.example/a/../b"),
            Err(PathProblem::DotSegment("..".into())),
            "url::Url would resolve this to /b"
        );
        assert_eq!(
            BasePath::of_url("https://x.example/%2e%2e/b"),
            Err(PathProblem::Character("%2e%2e".into()))
        );
        assert_eq!(
            BasePath::of_url("https://x.example/crystalline/?x=1"),
            Err(PathProblem::QueryOrFragment)
        );
        assert_eq!(
            BasePath::of_url("https://x.example/crystalline#top"),
            Err(PathProblem::QueryOrFragment)
        );
        assert_eq!(
            BasePath::of_url("https://x.example:8443/crystalline/")
                .unwrap()
                .as_str(),
            "/crystalline"
        );
        assert!(BasePath::of_url("https://x.example").unwrap().is_root());
        assert!(BasePath::of_url("https://x.example/").unwrap().is_root());
    }

    #[test]
    fn strip_takes_the_whole_prefix_once_and_nothing_else() {
        let base = BasePath::parse("/crystalline").unwrap();
        assert_eq!(base.strip("/crystalline"), Some("/"));
        assert_eq!(base.strip("/crystalline/"), Some("/"));
        assert_eq!(base.strip("/crystalline/health"), Some("/health"));
        assert_eq!(
            base.strip("/crystalline/crystalline/health"),
            Some("/crystalline/health"),
            "one strip, never two"
        );
        assert_eq!(base.strip("/crystallinex"), None);
        assert_eq!(base.strip("/crystallinex/health"), None);
        assert_eq!(
            base.strip("/CRYSTALLINE/health"),
            None,
            "paths are case-sensitive"
        );
        assert_eq!(base.strip("/health"), None);
        assert_eq!(
            BasePath::root().strip("/health"),
            None,
            "the root strips nothing"
        );
    }

    #[test]
    fn contains_accepts_the_prefix_and_what_is_under_it() {
        let base = BasePath::parse("/crystalline").unwrap();
        for inside in [
            "/crystalline",
            "/crystalline/",
            "/crystalline/d/x",
            "/crystalline?x",
            "/crystalline#h",
        ] {
            assert!(base.contains(inside), "{inside}");
        }
        for outside in ["/", "/d/x", "/crystallinex", "/other/crystalline"] {
            assert!(!base.contains(outside), "{outside}");
        }
        assert!(BasePath::root().contains("/d/x"));
    }

    #[test]
    fn a_public_base_joins_and_inserts_the_path_after_the_host() {
        let base = PublicBase::parse("https://Example.COM:443/crystalline/").unwrap();
        assert_eq!(base.origin(), "https://example.com");
        assert_eq!(base.path().as_str(), "/crystalline");
        assert_eq!(base.to_string(), "https://example.com/crystalline");
        assert_eq!(
            base.join("/health"),
            "https://example.com/crystalline/health"
        );
        assert_eq!(
            base.well_known("/.well-known/oauth-protected-resource"),
            "https://example.com/.well-known/oauth-protected-resource/crystalline"
        );
        assert_eq!(
            base.join("/.well-known/oauth-protected-resource"),
            "https://example.com/crystalline/.well-known/oauth-protected-resource"
        );
    }

    #[test]
    fn at_the_root_both_well_known_addresses_are_the_0_23_0_one() {
        let base = PublicBase::parse("http://127.0.0.1:7411").unwrap();
        let rfc = base.well_known("/.well-known/oauth-authorization-server");
        assert_eq!(
            rfc,
            "http://127.0.0.1:7411/.well-known/oauth-authorization-server"
        );
        assert_eq!(rfc, base.join("/.well-known/oauth-authorization-server"));
        assert_eq!(base.to_string(), "http://127.0.0.1:7411");
        assert_eq!(
            PublicBase::from_origin("http://127.0.0.1:7411/").to_string(),
            "http://127.0.0.1:7411"
        );
    }

    #[test]
    fn a_public_base_refuses_what_is_not_an_http_address() {
        assert_eq!(
            PublicBase::parse("kb.example"),
            Err(BaseProblem::NotAbsolute)
        );
        assert_eq!(
            PublicBase::parse("ftp://kb.example"),
            Err(BaseProblem::Scheme)
        );
        assert_eq!(
            PublicBase::parse("https://u:p@kb.example"),
            Err(BaseProblem::Userinfo)
        );
        assert_eq!(
            PublicBase::parse("https://kb.example/api"),
            Err(BaseProblem::Path(PathProblem::Reserved("api".into())))
        );
    }
}
