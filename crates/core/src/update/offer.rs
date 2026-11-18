//! Whether an update is offered: the one rule every surface that names an
//! available release asks (S29), so Fluid's note says what an agent would be
//! told. Pure: the caller hands in the daemon's facts, its own version, the
//! time and the live `update.min_age_hours`.

use crate::time::{self, Delta, Stamp};
use crate::version::Precedence;

use super::cache::Gate;
use super::facts::{CheckState, UpdateFacts};
use super::install::Detection;

/// How long a check's release stands for an offer: two days. A daemon checks
/// once a day, so an older cache means the checks stopped, and a stopped
/// check must not keep claiming a release.
pub fn offer_stale_after() -> Delta {
    time::hours(48)
}

/// The release to offer, or `None`. Offered only when the check is on and
/// nothing suppresses it, the build is official, `update now` acts for the
/// detected method, the last check found a release (never a failure) less
/// than [`offer_stale_after`] before `now`, its tag is an exact stable
/// release tag newer than `current`, and the age gate for `min_age_hours` is
/// open. The version is rendered only from the parsed tag, so a stable one
/// is digits and dots only (S23, R1).
pub fn offer(
    facts: &UpdateFacts,
    current: &Precedence,
    now: Stamp,
    min_age_hours: u32,
) -> Option<Precedence> {
    if facts.check != CheckState::On || !facts.official {
        return None;
    }
    let detection = Detection {
        method: facts.method,
        refusal: facts.refusal,
    };
    if !detection.acts() {
        return None;
    }
    let cache = facts.cache.as_ref()?;
    if !cache.fresh(now, offer_stale_after()) {
        return None;
    }
    let version = cache.newer_release(current).filter(Precedence::is_stable)?;
    (cache.gate(now, min_age_hours)? == Gate::Open).then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::{
        CheckOutcome, InstallMethod, OffReason, Refusal, UPDATE_CACHE_FORMAT, UpdateCache,
    };

    fn at(day: u32, hour: u32) -> Stamp {
        Stamp::from_ymd_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    fn current() -> Precedence {
        Precedence::parse("0.34.0").unwrap()
    }

    /// Checked on the 18th at 09:00, v0.35.0 published on the 16th at 09:00.
    fn release(tag: &str) -> UpdateCache {
        UpdateCache {
            v: UPDATE_CACHE_FORMAT,
            checked_at: at(18, 9),
            outcome: CheckOutcome::Release {
                version: tag.to_string(),
                published_at: Some(at(16, 9)),
                first_seen: at(17, 9),
            },
            etag: None,
            last_release: None,
        }
    }

    /// Everything open: an official tarball install with the check on.
    fn open() -> UpdateFacts {
        UpdateFacts {
            method: InstallMethod::Tarball,
            official: true,
            refusal: None,
            check: CheckState::On,
            cache: Some(release("v0.35.0")),
        }
    }

    /// The 18th at 10:00, one hour after the check.
    fn now() -> Stamp {
        at(18, 10)
    }

    fn offered(facts: &UpdateFacts) -> Option<String> {
        offer(facts, &current(), now(), 24).map(|v| v.render())
    }

    #[test]
    fn an_open_case_offers_the_rendered_version() {
        assert_eq!(offered(&open()), Some("0.35.0".to_string()));
        // A pre-release of the current triple still sees the release.
        let dev = Precedence::parse("0.35.0-dev.3").unwrap();
        assert_eq!(
            offer(&open(), &dev, now(), 24).map(|v| v.render()),
            Some("0.35.0".to_string())
        );
    }

    #[test]
    fn the_check_off_or_suppressed_offers_nothing() {
        for reason in [
            OffReason::NotTurnedOn,
            OffReason::NoUpdateNotifier,
            OffReason::Ci,
            OffReason::Docker,
            OffReason::DesktopExtension,
            OffReason::NoStateFolder,
        ] {
            let facts = UpdateFacts {
                check: CheckState::Off(reason),
                ..open()
            };
            assert_eq!(offered(&facts), None, "{}", reason.text());
        }
    }

    #[test]
    fn a_failed_or_missing_check_offers_nothing() {
        let mut failed = release("v0.35.0");
        failed.last_release = failed.known_release();
        failed.outcome = CheckOutcome::Failed {
            reason: "the check failed".to_string(),
        };
        let facts = UpdateFacts {
            cache: Some(failed),
            ..open()
        };
        assert_eq!(
            offered(&facts),
            None,
            "a failure, even with a known release"
        );
        let facts = UpdateFacts {
            cache: None,
            ..open()
        };
        assert_eq!(offered(&facts), None, "no check yet");
    }

    #[test]
    fn a_check_older_than_48_hours_or_from_the_future_offers_nothing() {
        let facts = open();
        let checked = at(18, 9);
        let just_inside = checked.checked_add(time::hours(48)).unwrap();
        let just_inside = just_inside.checked_sub(time::seconds(1)).unwrap();
        assert_eq!(
            offer(&facts, &current(), just_inside, 24).map(|v| v.render()),
            Some("0.35.0".to_string())
        );
        let stale = checked.checked_add(time::hours(48)).unwrap();
        assert_eq!(offer(&facts, &current(), stale, 24), None, "48 h old");
        let before = checked.checked_sub(time::minutes(1)).unwrap();
        assert_eq!(
            offer(&facts, &current(), before, 0),
            None,
            "from the future"
        );
    }

    #[test]
    fn a_closed_gate_offers_nothing() {
        // Published on the 16th at 09:00: 49 hours old at now(), so a
        // minimum of 50 hours keeps the gate closed.
        assert_eq!(offer(&open(), &current(), now(), 50), None);
        assert_eq!(
            offer(&open(), &current(), now(), 49).map(|v| v.render()),
            Some("0.35.0".to_string())
        );
        // Without a publish time the gate counts from the first sighting.
        let mut cache = release("v0.35.0");
        if let CheckOutcome::Release { published_at, .. } = &mut cache.outcome {
            *published_at = None;
        }
        let facts = UpdateFacts {
            cache: Some(cache),
            ..open()
        };
        assert_eq!(offer(&facts, &current(), now(), 26), None);
        assert!(offer(&facts, &current(), now(), 25).is_some());
    }

    #[test]
    fn a_version_not_newer_offers_nothing() {
        for tag in ["v0.34.0", "v0.33.9"] {
            let facts = UpdateFacts {
                cache: Some(release(tag)),
                ..open()
            };
            assert_eq!(offered(&facts), None, "{tag}");
        }
    }

    #[test]
    fn a_pre_release_or_an_inexact_tag_offers_nothing() {
        for tag in [
            "v0.35.0-rc.1",
            "v0.35.0-dev.4",
            "0.35.0",
            "v0.35",
            "v0.35.0.rc1",
            "v9.9.9 <b>now</b>",
        ] {
            let facts = UpdateFacts {
                cache: Some(release(tag)),
                ..open()
            };
            assert_eq!(offered(&facts), None, "{tag}");
        }
    }

    #[test]
    fn an_unofficial_build_offers_nothing() {
        let facts = UpdateFacts {
            official: false,
            ..open()
        };
        assert_eq!(offered(&facts), None);
        let facts = UpdateFacts {
            refusal: Some(Refusal::NotOfficial),
            ..open()
        };
        assert_eq!(offered(&facts), None);
    }

    #[test]
    fn a_method_update_now_does_not_act_on_offers_nothing() {
        for method in [
            InstallMethod::Docker,
            InstallMethod::Mcpb,
            InstallMethod::Unknown,
        ] {
            let facts = UpdateFacts { method, ..open() };
            assert_eq!(offered(&facts), None, "{method:?}");
        }
        for refusal in [
            Refusal::AppPackage,
            Refusal::MarkerPathMismatch {
                marker: InstallMethod::Deb,
                path: InstallMethod::Tarball,
            },
        ] {
            let facts = UpdateFacts {
                refusal: Some(refusal),
                ..open()
            };
            assert_eq!(offered(&facts), None, "{refusal:?}");
        }
        for method in [
            InstallMethod::Homebrew,
            InstallMethod::Deb,
            InstallMethod::Msi,
            InstallMethod::Zip,
        ] {
            let facts = UpdateFacts { method, ..open() };
            assert!(offered(&facts).is_some(), "{method:?}");
        }
    }
}
