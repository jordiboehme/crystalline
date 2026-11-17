//! The update check's cache, `update-check.json` in the state folder, and
//! the release-age gate. The daemon writes the cache; the CLI hint, the
//! session-start block and Fluid's note only read it, and a cache that does
//! not parse is read as no cache at all (S23).

use crate::time::{self, Stamp};
use crate::version::Precedence;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The cache's file name in `config::state_dir()`.
pub const UPDATE_CACHE_FILE: &str = "update-check.json";

/// The cache format this build writes and reads, the `v` field.
pub const UPDATE_CACHE_FORMAT: u32 = 1;

/// The last update check: when it ran, what it found and the ETag of the
/// answer, which only saves bandwidth.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateCache {
    /// The format, [`UPDATE_CACHE_FORMAT`].
    pub v: u32,
    /// When the check ran.
    #[serde(with = "time::serde::stamp")]
    pub checked_at: Stamp,
    /// What it found.
    pub outcome: CheckOutcome,
    /// The ETag of the API answer, when there was one.
    #[serde(default)]
    pub etag: Option<String>,
}

/// What an update check found. Failed checks are cached too, so a broken
/// network does not mean a request on every start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CheckOutcome {
    /// The newest release as GitHub named it.
    Release {
        /// The release version as the tag names it, with or without a
        /// leading `v`. Never shown as written: read it through
        /// [`UpdateCache::newer_release`].
        version: String,
        /// When GitHub published it; absent when only the fallback answered.
        #[serde(default, with = "time::serde::stamp_option")]
        published_at: Option<Stamp>,
        /// When this machine first saw this version, the age gate's start
        /// when there is no publish time.
        #[serde(with = "time::serde::stamp")]
        first_seen: Stamp,
        /// The asset digests (`sha256:<hex>`) by asset file name, from the
        /// API answer; empty after the fallback.
        #[serde(default)]
        digests: BTreeMap<String, String>,
    },
    /// The check failed; `reason` says why.
    Failed {
        /// What went wrong, for logs.
        reason: String,
    },
}

impl UpdateCache {
    /// The cache from its file text, or `None` when it does not parse or is
    /// in another format.
    pub fn parse(text: &str) -> Option<UpdateCache> {
        serde_json::from_str::<UpdateCache>(text)
            .ok()
            .filter(|cache| cache.v == UPDATE_CACHE_FORMAT)
    }

    /// The cached release when it is stable and strictly newer than
    /// `current`; `None` for a failed check, a pre-release, an older or equal
    /// version or a version that does not parse.
    pub fn newer_release(&self, current: &Precedence) -> Option<Precedence> {
        let CheckOutcome::Release { version, .. } = &self.outcome else {
            return None;
        };
        let bare = version.strip_prefix('v').unwrap_or(version);
        Precedence::parse(bare).filter(|found| found.is_stable() && found > current)
    }
}

/// Whether a release is old enough to be offered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Old enough.
    Open,
    /// Too young; it opens at this instant.
    OpensAt(Stamp),
    /// Never opens: the minimum age runs past the end of the date range.
    Never,
}

/// The release-age gate: open when at least `min_age_hours` have passed
/// since `published_or_first_seen`. A release dated in the future stays
/// closed until its age is reached; an age past the end of the date range
/// is [`Gate::Never`] instead of a panic or a made-up date.
pub fn age_gate(published_or_first_seen: Stamp, now: Stamp, min_age_hours: u32) -> Gate {
    let min_age = time::hours(i64::from(min_age_hours));
    if now.since(published_or_first_seen) >= min_age {
        return Gate::Open;
    }
    match published_or_first_seen.checked_add(min_age) {
        Some(at) => Gate::OpensAt(at),
        None => Gate::Never,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(day: u32, hour: u32, minute: u32) -> Stamp {
        Stamp::from_ymd_hms(2026, 11, day, hour, minute, 0).unwrap()
    }

    fn release(version: &str) -> UpdateCache {
        UpdateCache {
            v: UPDATE_CACHE_FORMAT,
            checked_at: at(17, 9, 0),
            outcome: CheckOutcome::Release {
                version: version.to_string(),
                published_at: Some(at(16, 8, 30)),
                first_seen: at(17, 9, 0),
                digests: BTreeMap::new(),
            },
            etag: None,
        }
    }

    #[test]
    fn cache_round_trips_and_old_cache_without_published_at_loads() {
        let mut cache = release("0.34.0");
        cache.etag = Some("\"abc\"".to_string());
        if let CheckOutcome::Release { digests, .. } = &mut cache.outcome {
            digests.insert(
                "crystalline-v0.34.0-linux-amd64.tar.gz".to_string(),
                format!("sha256:{}", "a".repeat(64)),
            );
        }
        let text = serde_json::to_string(&cache).unwrap();
        assert_eq!(UpdateCache::parse(&text), Some(cache));

        let failed = UpdateCache {
            v: UPDATE_CACHE_FORMAT,
            checked_at: at(17, 9, 0),
            outcome: CheckOutcome::Failed {
                reason: "timed out".to_string(),
            },
            etag: None,
        };
        let text = serde_json::to_string(&failed).unwrap();
        assert_eq!(UpdateCache::parse(&text), Some(failed));

        // The wire format, written by hand: no published_at, no digests and
        // no etag (a check that fell back to the redirect).
        let old = r#"{"v":1,"checked_at":"2026-11-17T09:00:00Z","outcome":{"kind":"release","version":"0.34.0","first_seen":"2026-11-17T09:00:00Z"}}"#;
        let parsed = UpdateCache::parse(old).expect("an old cache loads");
        assert_eq!(parsed.etag, None);
        assert_eq!(
            parsed.outcome,
            CheckOutcome::Release {
                version: "0.34.0".to_string(),
                published_at: None,
                first_seen: at(17, 9, 0),
                digests: BTreeMap::new(),
            }
        );
        let failed = r#"{"v":1,"checked_at":"2026-11-17T09:00:00Z","outcome":{"kind":"failed","reason":"rate limited"}}"#;
        assert!(matches!(
            UpdateCache::parse(failed).map(|c| c.outcome),
            Some(CheckOutcome::Failed { .. })
        ));

        // Anything else is no cache at all.
        for broken in [
            "",
            "{",
            "null",
            r#"{"v":2,"checked_at":"2026-11-17T09:00:00Z","outcome":{"kind":"failed","reason":"x"}}"#,
            r#"{"v":1,"checked_at":"yesterday","outcome":{"kind":"failed","reason":"x"}}"#,
            r#"{"v":1,"checked_at":"2026-11-17T09:00:00Z","outcome":{"kind":"other"}}"#,
        ] {
            assert_eq!(UpdateCache::parse(broken), None, "{broken}");
        }
    }

    #[test]
    fn cache_newer_release_ignores_pre_releases_and_older() {
        let current = Precedence::parse("0.33.0").unwrap();
        let newer = |version: &str| release(version).newer_release(&current);
        assert_eq!(newer("0.34.0"), Precedence::parse("0.34.0"));
        assert_eq!(newer("v0.34.0"), Precedence::parse("0.34.0"));
        assert_eq!(newer("0.34.0-rc.1"), None);
        assert_eq!(newer("v0.34.0-dev.4817"), None);
        assert_eq!(newer("0.33.0"), None);
        assert_eq!(newer("v0.33.0"), None);
        assert_eq!(newer("0.32.9"), None);
        assert_eq!(newer("not a version"), None);
        assert_eq!(newer("vv0.34.0"), None);
        // A pre-release of the current triple is older than its release.
        let dev = Precedence::parse("0.34.0-dev.5").unwrap();
        assert_eq!(
            release("0.34.0").newer_release(&dev),
            Precedence::parse("0.34.0")
        );
        let failed = UpdateCache {
            outcome: CheckOutcome::Failed {
                reason: "x".to_string(),
            },
            ..release("0.34.0")
        };
        assert_eq!(failed.newer_release(&current), None);
    }

    #[test]
    fn age_gate_boundaries() {
        let published = at(16, 9, 0);
        assert_eq!(
            age_gate(published, at(17, 8, 59), 24),
            Gate::OpensAt(at(17, 9, 0))
        );
        assert_eq!(age_gate(published, at(17, 9, 0), 24), Gate::Open);
        assert_eq!(age_gate(published, at(20, 9, 0), 24), Gate::Open);
        assert_eq!(age_gate(published, published, 0), Gate::Open);
        // A publish time ahead of the clock keeps the gate closed.
        assert_eq!(
            age_gate(at(18, 9, 0), at(17, 9, 0), 24),
            Gate::OpensAt(at(19, 9, 0))
        );
        assert_eq!(
            age_gate(at(18, 9, 0), at(17, 9, 0), 0),
            Gate::OpensAt(at(18, 9, 0))
        );
        // An age past the end of the date range never opens, without a panic.
        assert_eq!(age_gate(published, at(17, 9, 0), u32::MAX), Gate::Never);
    }
}
