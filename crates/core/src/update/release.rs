//! The newest release as the update lookup found it, and the pure reading of
//! GitHub's two answers: the API's `releases/latest` body and, on the
//! rate-limit fallback, the `location` of the web page's redirect. The
//! service does the requests; nothing here touches the network.
//!
//! Only a stable release is ever taken (R1): the tag must read through
//! [`release_version`], and a draft or a pre-release is refused, so a version
//! from here renders as digits and dots only.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::time::{self, Stamp};
use crate::version::{Precedence, release_tag, release_version};

/// The newest stable release.
#[derive(Debug, Clone, PartialEq)]
pub struct LatestRelease {
    /// The release tag, `v0.34.0`, written by [`release_tag`] from
    /// [`version`](Self::version) and never copied from the answer: the one
    /// tag value an asset is downloaded from and its signature must name.
    pub tag: String,
    /// The release's version, always stable.
    pub version: Precedence,
    /// When the release was published; `None` on the fallback or when the
    /// API's time does not parse.
    pub published_at: Option<Stamp>,
    /// Each asset's digest as the API names it (`sha256:<hex>`), by asset
    /// name. Empty on the fallback, and an asset without a digest has no
    /// entry.
    pub digests: BTreeMap<String, String>,
    /// Whether the API answered (true) or the redirect fallback did (false).
    /// The digest is checked only when the API answered (S8).
    pub from_api: bool,
}

/// What a lookup found.
#[derive(Debug, Clone, PartialEq)]
pub enum Lookup {
    /// A fresh answer, with the ETag to send next time when there was one.
    Fresh(LatestRelease, Option<String>),
    /// The API answered 304 to the ETag sent: nothing changed.
    NotModified,
}

/// The fields of GitHub's release object the lookup reads.
#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
    #[serde(default)]
    digest: Option<String>,
}

impl LatestRelease {
    /// Read the body of `GET /repos/<owner>/<repo>/releases/latest`. `Err`
    /// when the body is not a release object; `Ok(None)` when the release is
    /// a draft, a pre-release or its tag is not exactly `v<x.y.z>`.
    pub fn from_api(body: &[u8]) -> Result<Option<LatestRelease>, serde_json::Error> {
        let release: ApiRelease = serde_json::from_slice(body)?;
        if release.draft || release.prerelease {
            return Ok(None);
        }
        let Some((tag, version)) = stable(&release.tag_name) else {
            return Ok(None);
        };
        let published_at = release
            .published_at
            .as_deref()
            .and_then(time::parse_rfc3339)
            .map(|at| at.stamp());
        let digests = release
            .assets
            .into_iter()
            .filter_map(|asset| asset.digest.map(|digest| (asset.name, digest)))
            .collect();
        Ok(Some(LatestRelease {
            tag,
            version,
            published_at,
            digests,
            from_api: true,
        }))
    }

    /// Read the `location` of the redirect `HEAD
    /// <web>/jordiboehme/crystalline/releases/latest` answers with:
    /// `.../jordiboehme/crystalline/releases/tag/<tag>`. `None` when the
    /// location has another shape or the tag is not exactly `v<x.y.z>`.
    pub fn from_redirect(location: &str) -> Option<LatestRelease> {
        let (_, tag) = location.rsplit_once(REDIRECT_PATH)?;
        let (tag, version) = stable(tag)?;
        Some(LatestRelease {
            tag,
            version,
            published_at: None,
            digests: BTreeMap::new(),
            from_api: false,
        })
    }
}

/// The path a redirect to a release page carries before the tag.
const REDIRECT_PATH: &str = "/jordiboehme/crystalline/releases/tag/";

/// The tag and version of a stable release tag, the tag written anew.
fn stable(tag: &str) -> Option<(String, Precedence)> {
    let version = release_version(tag)?;
    Some((release_tag(&version)?, version))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(tag: &str, draft: bool, prerelease: bool) -> String {
        serde_json::json!({
            "tag_name": tag,
            "draft": draft,
            "prerelease": prerelease,
            "published_at": "2026-11-17T10:00:00Z",
            "assets": [
                {"name": "crystalline-v0.34.0-linux-amd64.tar.gz", "digest": "sha256:ab"},
                {"name": "SHA256SUMS", "digest": null},
                {"name": "old.zip"}
            ]
        })
        .to_string()
    }

    #[test]
    fn a_stable_release_reads_with_its_digests_and_time() {
        let release = LatestRelease::from_api(body("v0.34.0", false, false).as_bytes())
            .unwrap()
            .expect("a stable release");
        assert_eq!(release.tag, "v0.34.0");
        assert_eq!(release.version.render(), "0.34.0");
        assert!(release.from_api);
        assert_eq!(
            release
                .published_at
                .map(|at| at.rfc3339_secs_z())
                .as_deref(),
            Some("2026-11-17T10:00:00Z")
        );
        assert_eq!(
            release.digests,
            BTreeMap::from([(
                "crystalline-v0.34.0-linux-amd64.tar.gz".to_string(),
                "sha256:ab".to_string()
            )])
        );
    }

    #[test]
    fn drafts_pre_releases_and_lax_tags_are_not_offered() {
        for (tag, draft, prerelease) in [
            ("v0.34.0", true, false),
            ("v0.34.0", false, true),
            ("v0.35.0-rc.1", false, false),
            ("v0.35.0.rc1", false, false),
            ("v0.35", false, false),
            ("0.35.0", false, false),
            ("v0.35.0 ", false, false),
        ] {
            assert_eq!(
                LatestRelease::from_api(body(tag, draft, prerelease).as_bytes()).unwrap(),
                None,
                "{tag} draft={draft} prerelease={prerelease}"
            );
        }
    }

    #[test]
    fn an_unparsable_time_is_no_time() {
        let text = r#"{"tag_name":"v0.34.0","published_at":"yesterday"}"#;
        let release = LatestRelease::from_api(text.as_bytes()).unwrap().unwrap();
        assert_eq!(release.published_at, None);
        assert!(release.digests.is_empty());
    }

    #[test]
    fn a_body_that_is_no_release_is_an_error() {
        assert!(LatestRelease::from_api(b"<html>").is_err());
        assert!(LatestRelease::from_api(b"{}").is_err());
    }

    #[test]
    fn the_redirect_names_the_tag() {
        let release = LatestRelease::from_redirect(
            "https://github.com/jordiboehme/crystalline/releases/tag/v0.34.0",
        )
        .expect("a release");
        assert_eq!(release.tag, "v0.34.0");
        assert!(!release.from_api);
        assert_eq!(release.published_at, None);
        assert!(release.digests.is_empty());
        for location in [
            "https://github.com/jordiboehme/crystalline/releases/tag/v0.35.0-rc.1",
            "https://github.com/jordiboehme/crystalline/releases",
            "https://github.com/jordiboehme/crystalline/releases/tag/",
            "https://github.com/other/crystalline/releases/tag/v0.34.0",
            "https://github.com/jordiboehme/crystalline/releases/tag/v0.34.0/extra",
        ] {
            assert_eq!(LatestRelease::from_redirect(location), None, "{location}");
        }
    }
}
