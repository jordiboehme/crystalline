//! The host lock of a file domain on a shared database, and the host move a
//! claim marks when it takes a domain from another instance (Task 50).
//!
//! A file domain's files on disk are its one source of truth. On a shared
//! Postgres database each instance keeps its own disk copy of a domain, so
//! an instance that takes a domain over can meet a disk that lacks files the
//! previous host wrote. The mark set here makes every complete walk keep the
//! rows it does not see until that is resolved (see [`crate::apply_scan`]).

use crate::error::Result;
use crate::store::{DomainHost, DomainId, HostClaim, HostMove, OnlyElsewhere, Store};

/// A claim of [`claim_host_marking_moves`]: the claim itself, and when it
/// replaced a live claim of this same instance id (this machine's daemon,
/// or another command of this machine), that claim's token, so a one-shot
/// command can give the lock back to it ([`give_back`]) instead of
/// deleting it from under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claimed {
    /// The claim.
    pub claim: HostClaim,
    /// The token of the live claim under the same instance id this claim
    /// replaced, if any.
    pub over: Option<String>,
    /// The label the claim recorded for this instance ([`instance_label`]).
    pub label: String,
    /// The claim of another instance this claim replaced, when it acquired
    /// the lock from one: with `--take-over`, or because that instance sent
    /// no heartbeat for the stale threshold.
    pub took_from: Option<DomainHost>,
}

/// What a holder keeps of a claim it acquired, to give the lock back with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLease {
    /// The token its claim returned.
    pub token: String,
    /// The token of the live claim under the same instance id it replaced,
    /// which [`give_back`] puts back instead of deleting the row.
    pub over: Option<String>,
}

impl Claimed {
    /// The lease to keep when the claim acquired the lock.
    pub fn lease(&self) -> Option<HostLease> {
        match &self.claim {
            HostClaim::Acquired { token } => Some(HostLease {
                token: token.clone(),
                over: self.over.clone(),
            }),
            HostClaim::HeldByOther(_) => None,
        }
    }
}

/// Give back a lock a one-shot command claimed: delete the row when it
/// still carries the lease's token, or, when the claim replaced a live claim
/// of this same instance id, put that claim's token back, so its holder
/// keeps the lock and its own release still matches. Never touches a row a
/// later claim wrote, under this id or another.
pub async fn give_back(
    store: &dyn Store,
    domain: DomainId,
    instance_id: &str,
    lease: &HostLease,
) -> Result<()> {
    match &lease.over {
        Some(previous) => {
            store
                .hand_back_domain_host(domain, instance_id, &lease.token, previous)
                .await?;
        }
        None => {
            store
                .release_domain_host(domain, instance_id, &lease.token)
                .await?;
        }
    }
    Ok(())
}

/// The label an instance records for people to read: its `name` (the
/// host of its `service.public_url`, else the machine's hostname), with the
/// first four characters of its id in parentheses when another instance is
/// already recorded under that name, for example `build-box (3f2a)`. The
/// instance that came first keeps the bare name. A label this instance is
/// already recorded under for this name is kept, so one instance never
/// shows two labels while its name stays. An empty name, or one equal to
/// the id, is the id.
pub async fn instance_label(store: &dyn Store, name: &str, instance_id: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() || name == instance_id {
        return Ok(instance_id.to_string());
    }
    // The label this instance already uses under this name stays, so one
    // instance never shows two labels: a later claim, another process of
    // this machine, a restart.
    let short: String = instance_id.chars().take(4).collect();
    let suffixed = format!("{name} ({short})");
    let own = store.own_labels(instance_id).await?;
    // Once an instance used its short id under this name, it keeps it, even
    // when the twin that made it take one is gone: a label never flips back.
    if own.contains(&suffixed) {
        return Ok(suffixed);
    }
    if own.iter().any(|label| label == name) {
        // Two instances whose first claims raced can both hold the bare
        // name. The one with the greater id gives it up, at its next claim
        // or renewal, so the two read apart from then on.
        let (_, stale_before) = claim_window(stale_secs());
        let twin = store
            .live_label_holders(name, &stale_before)
            .await?
            .iter()
            .any(|other| other.as_str() < instance_id);
        return Ok(if twin { suffixed } else { name.to_string() });
    }
    if store.label_taken(name, instance_id).await? {
        return Ok(suffixed);
    }
    Ok(name.to_string())
}

/// The default host-lock stale threshold, seconds (three missed heartbeats
/// of 30 s). A lock whose last heartbeat is older is takeable by another
/// instance.
const DEFAULT_STALE_SECS: i64 = 90;

/// The host-lock stale threshold in seconds, from `CRYSTALLINE_STALE_SECS`
/// (a short one makes multi-instance verification fast), else 90. An unset,
/// empty, unparsable or non-positive value is the default. The daemon and a
/// sync without a daemon both read it here, so they agree on when a lock is
/// stale.
pub fn stale_secs() -> i64 {
    std::env::var("CRYSTALLINE_STALE_SECS")
        .ok()
        .and_then(|v| v.trim().parse::<i64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(DEFAULT_STALE_SECS)
}

/// The `(now, stale_before)` pair a claim compares heartbeats with, both
/// RFC 3339 in the one spelling every heartbeat is written in, so the
/// comparison stays a lexical one.
pub fn claim_window(stale_secs: i64) -> (String, String) {
    let now = chrono::Utc::now().fixed_offset();
    let stale_before = now - chrono::Duration::seconds(stale_secs);
    (now.to_rfc3339(), stale_before.to_rfc3339())
}

/// The refusal of `--accept-disk` without a named domain: it drops rows, so
/// it is asked of one domain at a time.
pub const ACCEPT_DISK_NEEDS_DOMAIN: &str =
    "--accept-disk drops the rows of one domain; name it with --domain <name>";

/// Claim `domain`'s host lock for `instance_id`, and on a shared database
/// mark a host move when the claim acquires a domain whose previous holder
/// was not this instance: another instance's id, or no holder anyone
/// recorded. The previous holder is the lock row when there is one, else the
/// instance the domain row names as its last host (a release deletes the
/// lock row, the domain row keeps that name), so a restart of the same host
/// marks nothing, clean or not.
///
/// The previous holder is read, the lock claimed and the move marked in one
/// transaction, so a failed mark write or a crash between the steps never
/// leaves the lock naming this instance without the mark: the next claim
/// would read "this instance", mark nothing, and its walk would drop the
/// rows only the previous host has. `label` is this instance's name, which
/// the claim records as [`instance_label`] makes it; every other argument
/// is [`Store::claim_domain_host`]'s. Off a shared database this is that claim
/// and nothing more, since one instance owns the database.
///
/// The answer also names the live claim of this same instance id the claim
/// replaced, read in the same transaction ([`Claimed::over`]).
pub async fn claim_host_marking_moves(
    store: &dyn Store,
    domain: DomainId,
    instance_id: &str,
    label: &str,
    now: &str,
    stale_before: &str,
    take_over: bool,
) -> Result<Claimed> {
    if !store.shares_database() {
        let holder = store.domain_host(domain).await?;
        let over = live_own_token(holder.as_ref(), instance_id, stale_before);
        let label = instance_label(store, label, instance_id).await?;
        let claim = store
            .claim_domain_host(domain, instance_id, &label, now, stale_before, take_over)
            .await?;
        let took_from = foreign(holder, instance_id, &claim);
        return Ok(Claimed {
            claim,
            over,
            label,
            took_from,
        });
    }
    store.begin().await?;
    let claimed = claim_and_mark(
        store,
        domain,
        instance_id,
        label,
        now,
        stale_before,
        take_over,
    )
    .await;
    match claimed {
        Ok(claim) => {
            store.commit().await?;
            Ok(claim)
        }
        Err(e) => {
            let _ = store.rollback().await;
            Err(e)
        }
    }
}

/// The claim a re-claim would answer when it would change nothing: this
/// instance already holds `domain`'s lock under `lease`, its heartbeat is at
/// or after `fresh_since`, and the row already carries the label a claim
/// would record (and, on a shared database, the domain row already names this
/// instance as its last host). `None` when any of that is not so, and the
/// caller claims as usual.
///
/// A long-running instance re-claims every domain on every sync, and every
/// claim writes a new token, so an idle domain synced on each watcher flush
/// would write its lock row each time. The heartbeat timer keeps a held lock
/// fresh on its own, so such a re-claim is answered from the row instead,
/// with nothing written. Only reads run here. The answer's `over` is the
/// held token, which is what a claim replacing it would name, so a caller
/// that keeps the lease of its first claim keeps it unchanged. `fresh_since`
/// must not be before the stale threshold, or a lock another instance may
/// already take would be treated as held.
pub async fn held_claim(
    store: &dyn Store,
    domain: DomainId,
    instance_id: &str,
    label: &str,
    lease: &HostLease,
    fresh_since: &str,
) -> Result<Option<Claimed>> {
    let Some(holder) = store.domain_host(domain).await? else {
        return Ok(None);
    };
    if holder.instance_id != instance_id
        || holder.token != lease.token
        || holder.heartbeat_at.as_str() < fresh_since
    {
        return Ok(None);
    }
    let label = instance_label(store, label, instance_id).await?;
    if holder.label != label {
        return Ok(None);
    }
    if store.shares_database()
        && store.last_domain_host(domain).await? != Some((instance_id.to_string(), label.clone()))
    {
        return Ok(None);
    }
    Ok(Some(Claimed {
        claim: HostClaim::Acquired {
            token: holder.token.clone(),
        },
        over: Some(holder.token),
        label,
        took_from: None,
    }))
}

/// The token of a live claim this instance id holds, if `holder` is one.
fn live_own_token(
    holder: Option<&DomainHost>,
    instance_id: &str,
    stale_before: &str,
) -> Option<String> {
    holder
        .filter(|h| h.instance_id == instance_id && h.heartbeat_at.as_str() >= stale_before)
        .map(|h| h.token.clone())
}

/// The holder a claim replaced when it acquired the lock from another
/// instance.
fn foreign(holder: Option<DomainHost>, instance_id: &str, claim: &HostClaim) -> Option<DomainHost> {
    holder.filter(|h| {
        claim.is_acquired() && !h.instance_id.is_empty() && h.instance_id != instance_id
    })
}

/// The body of [`claim_host_marking_moves`] on a shared database, inside
/// its transaction.
async fn claim_and_mark(
    store: &dyn Store,
    domain: DomainId,
    instance_id: &str,
    label: &str,
    now: &str,
    stale_before: &str,
    take_over: bool,
) -> Result<Claimed> {
    let holder = store.domain_host(domain).await?;
    let over = live_own_token(holder.as_ref(), instance_id, stale_before);
    let last = store.last_domain_host(domain).await?;
    let previous = holder
        .as_ref()
        .map(|h| (h.instance_id.clone(), h.label.clone()))
        .or(last.clone());
    let label = instance_label(store, label, instance_id).await?;
    let claim = store
        .claim_domain_host(domain, instance_id, &label, now, stale_before, take_over)
        .await?;
    let took_from = foreign(holder, instance_id, &claim);
    if claim.is_acquired() {
        if previous.as_ref().map(|(id, _)| id.as_str()) != Some(instance_id) {
            let mark = HostMove {
                at: now.to_string(),
                claimant: Some(instance_id.to_string()),
                walked: false,
                only_elsewhere: OnlyElsewhere {
                    previous_host: previous.map(|(_, label)| label),
                    ..OnlyElsewhere::default()
                },
            };
            #[cfg(test)]
            seam::fail_mark_of(instance_id)?;
            store.set_host_move(domain, Some(&mark)).await?;
        }
        if last.as_ref() != Some(&(instance_id.to_string(), label.clone())) {
            store
                .set_last_domain_host(domain, instance_id, &label)
                .await?;
        }
    }
    Ok(Claimed {
        claim,
        over,
        label,
        took_from,
    })
}

/// A failure injected into the mark write, for the test that proves a
/// failed mark leaves no claim behind. Keyed by instance id, so tests that
/// run side by side never meet it.
#[cfg(test)]
mod seam {
    use std::collections::HashSet;
    use std::sync::Mutex;

    static FAILING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

    // Only the postgres-gated host tests arm it.
    #[cfg(feature = "postgres")]
    pub(super) fn fail_marks_of(instance_id: &str) {
        FAILING
            .lock()
            .unwrap()
            .get_or_insert_with(HashSet::new)
            .insert(instance_id.to_string());
    }

    pub(super) fn fail_mark_of(instance_id: &str) -> crate::error::Result<()> {
        if FAILING
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|ids| ids.contains(instance_id))
        {
            return Err(crate::error::IndexError::Db(
                "injected: the mark write failed".to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::ReadStore;

    #[test]
    fn the_line_names_the_counts_the_host_and_both_ways_out() {
        let mut counts = OnlyElsewhere {
            previous_host: Some("alpha".to_string()),
            engrams: 1,
            attachments: 0,
            pages: 2,
            files: 1,
        };
        assert_eq!(
            counts.line("eng"),
            "1 engram and 3 Reflection files exist only on alpha; copy the domain folder over or restore it, then sync, or drop them with: crystalline sync --domain eng --accept-disk"
        );
        counts.previous_host = None;
        counts.attachments = 2;
        counts.pages = 0;
        counts.files = 1;
        assert_eq!(
            counts.line("eng"),
            "1 engram, 2 attachments and 1 Reflection file exist only on another instance; copy the domain folder over or restore it, then sync, or drop them with: crystalline sync --domain eng --accept-disk"
        );
        assert_eq!(counts.total(), 4);
        assert_eq!(
            counts.line_elsewhere("beta"),
            "1 engram, 2 attachments and 1 Reflection file exist only on another instance; beta hosts this domain, so resolve it there"
        );
        assert_eq!(
            counts.line_stale_host("beta", "eng"),
            "1 engram, 2 attachments and 1 Reflection file exist only on another instance; beta hosts this domain but sends no heartbeat, so take it over with: crystalline sync --domain eng --take-over"
        );
    }

    /// The URL and a fresh schema name, or `None` with no URL.
    #[cfg(feature = "postgres")]
    fn shared_schema() -> Option<(String, String)> {
        let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL").ok()?;
        if url.is_empty() {
            return None;
        }
        let schema = format!(
            "hosting_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        Some((url, schema))
    }

    /// A Postgres store on a schema of its own, or `None` with no URL.
    #[cfg(feature = "postgres")]
    async fn shared_store() -> Option<crate::PostgresStore> {
        let (url, schema) = shared_schema()?;
        Some(
            crate::PostgresStore::open_in_schema(&url, &schema)
                .await
                .unwrap(),
        )
    }

    /// A domain folder with one engram.
    #[cfg(feature = "postgres")]
    fn domain_folder() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.md"),
            "---\ntype: engram\ntitle: A\npermalink: a\nstatus: stable\n---\n\n# A\n",
        )
        .unwrap();
        (tmp, root)
    }

    /// One complete walk of `root` applied as `walker`.
    #[cfg(feature = "postgres")]
    async fn walk_as(
        store: &dyn Store,
        domain: DomainId,
        root: &std::path::Path,
        walker: &str,
    ) -> crate::error::Result<crate::SyncReport> {
        let stamps = store.file_stamps(domain).await.unwrap();
        let mut scan =
            crate::scan_domain("eng", root, stamps, &crate::ChunkParams::default(), false)
                .await
                .unwrap();
        scan.walked_by(walker);
        crate::apply_scan(store, domain, scan).await
    }

    /// A take-over that lands while the host's apply runs waits for that
    /// apply to commit, then marks: the apply cannot end or overwrite the
    /// new host's move.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_claim_waits_for_an_apply_in_flight() {
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let open = || async {
            std::sync::Arc::new(
                crate::PostgresStore::open_in_schema(&url, &schema)
                    .await
                    .unwrap(),
            )
        };
        let (a, b) = (open().await, open().await);
        let (_tmp, root) = domain_folder();
        let domain = a
            .upsert_domain(
                "eng",
                Some(&root.to_string_lossy()),
                crate::DomainKind::File,
            )
            .await
            .unwrap();
        let (now, stale_before) = claim_window(90);
        claim_host_marking_moves(
            &*a,
            domain,
            "alpha-w",
            "alpha-w",
            &now,
            &stale_before,
            false,
        )
        .await
        .unwrap();
        walk_as(&*a, domain, &root, "alpha-w").await.unwrap();
        assert!(
            a.host_move(domain).await.unwrap().is_none(),
            "alpha's own walk ended its first mark"
        );

        let (entered, release) = crate::sync::apply_seam::arm("alpha-w");
        let applying = tokio::spawn({
            let (a, root) = (a.clone(), root.clone());
            async move { walk_as(&*a, domain, &root, "alpha-w").await }
        });
        entered.notified().await;
        let claiming = tokio::spawn({
            let (b, now, stale_before) = (b.clone(), now.clone(), stale_before.clone());
            async move {
                claim_host_marking_moves(&*b, domain, "beta-w", "beta-w", &now, &stale_before, true)
                    .await
            }
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(
            !claiming.is_finished(),
            "the claim waits for the apply in flight"
        );
        release.notify_one();
        applying.await.unwrap().unwrap();
        assert!(claiming.await.unwrap().unwrap().claim.is_acquired());
        let mark = a
            .host_move(domain)
            .await
            .unwrap()
            .expect("beta's move stands");
        assert_eq!(mark.claimant.as_deref(), Some("beta-w"));
        assert!(!mark.walked);
        a.drop_schema().await.unwrap();
    }

    /// A move written by someone else while an apply runs is not overwritten:
    /// the apply's write lands only on the value it read, else nothing of the
    /// apply is kept.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_move_changed_during_an_apply_refuses_the_apply() {
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let a = std::sync::Arc::new(
            crate::PostgresStore::open_in_schema(&url, &schema)
                .await
                .unwrap(),
        );
        let b = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let (_tmp, root) = domain_folder();
        let domain = a
            .upsert_domain(
                "eng",
                Some(&root.to_string_lossy()),
                crate::DomainKind::File,
            )
            .await
            .unwrap();
        let (now, stale_before) = claim_window(90);
        claim_host_marking_moves(
            &*a,
            domain,
            "alpha-c",
            "alpha-c",
            &now,
            &stale_before,
            false,
        )
        .await
        .unwrap();
        assert!(
            a.host_move(domain).await.unwrap().is_some(),
            "alpha's first claim marks"
        );

        let (entered, release) = crate::sync::apply_seam::arm("alpha-c");
        let applying = tokio::spawn({
            let (a, root) = (a.clone(), root.clone());
            async move { walk_as(&*a, domain, &root, "alpha-c").await }
        });
        entered.notified().await;
        let other = HostMove {
            at: now.clone(),
            claimant: Some("gamma-c".to_string()),
            walked: false,
            only_elsewhere: OnlyElsewhere::default(),
        };
        b.set_host_move(domain, Some(&other)).await.unwrap();
        release.notify_one();
        let refused = applying.await.unwrap().unwrap_err();
        assert!(
            matches!(refused, crate::IndexError::HostMoveChanged(_)),
            "{refused}"
        );
        assert_eq!(a.host_move(domain).await.unwrap(), Some(other));
        assert!(
            a.file_stamps(domain).await.unwrap().is_empty(),
            "nothing of the refused apply is kept"
        );
        a.drop_schema().await.unwrap();
    }

    /// Two instances that claim a stale lock at the same moment without
    /// `--take-over` never both get it. Each claim is one upsert whose
    /// condition (`heartbeat_at < stale_before`) Postgres checks under the
    /// row lock: the second waits for the first to commit, then reads the
    /// winner's fresh heartbeat and leaves the row alone. Run many times, so
    /// the two claims overlap in more than one way.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn two_claims_of_a_stale_lock_give_it_to_exactly_one() {
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let open = || async {
            std::sync::Arc::new(
                crate::PostgresStore::open_in_schema(&url, &schema)
                    .await
                    .unwrap(),
            )
        };
        let (a, b) = (open().await, open().await);
        let domain = a
            .upsert_domain("eng", Some("/tmp/eng"), crate::DomainKind::File)
            .await
            .unwrap();
        let mut winners = std::collections::HashSet::new();
        for _ in 0..20 {
            let gone = claim_window(91).1;
            a.claim_domain_host(domain, "crashed", "crashed", &gone, &gone, true)
                .await
                .unwrap();
            let claim = |store: std::sync::Arc<crate::PostgresStore>, id: &'static str| async move {
                let (now, stale_before) = claim_window(90);
                claim_host_marking_moves(&*store, domain, id, id, &now, &stale_before, false)
                    .await
                    .unwrap()
            };
            let (x, y) = tokio::join!(
                tokio::spawn(claim(a.clone(), "beta-s")),
                tokio::spawn(claim(b.clone(), "gamma-s"))
            );
            let (x, y) = (x.unwrap(), y.unwrap());
            let winner = match (&x.claim, &y.claim) {
                (HostClaim::Acquired { .. }, HostClaim::HeldByOther(h)) => {
                    assert_eq!(h.instance_id, "beta-s");
                    assert_eq!(x.took_from.as_ref().unwrap().instance_id, "crashed");
                    "beta-s"
                }
                (HostClaim::HeldByOther(h), HostClaim::Acquired { .. }) => {
                    assert_eq!(h.instance_id, "gamma-s");
                    assert_eq!(y.took_from.as_ref().unwrap().instance_id, "crashed");
                    "gamma-s"
                }
                other => panic!("exactly one claim wins: {other:?}"),
            };
            assert_eq!(
                a.domain_host(domain).await.unwrap().unwrap().instance_id,
                winner
            );
            let mark = a.host_move(domain).await.unwrap().expect("marked");
            assert_eq!(mark.claimant.as_deref(), Some(winner));
            winners.insert(winner);
        }
        assert!(!winners.is_empty());
        a.drop_schema().await.unwrap();
    }

    /// A claim whose mark write fails leaves no claim behind: the lock still
    /// names the previous holder, and the next claim marks the move.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_failed_mark_leaves_the_lock_where_it_was() {
        let Some(store) = shared_store().await else {
            return;
        };
        let domain = store
            .upsert_domain("eng", Some("/tmp/eng"), crate::DomainKind::File)
            .await
            .unwrap();
        let (now, stale_before) = claim_window(90);
        let claim = |id: &'static str| {
            let (store, now, stale_before) = (&store, now.clone(), stale_before.clone());
            async move {
                claim_host_marking_moves(store, domain, id, id, &now, &stale_before, true).await
            }
        };
        assert!(claim("alpha-mark").await.unwrap().claim.is_acquired());
        store.set_host_move(domain, None).await.unwrap();

        seam::fail_marks_of("beta-mark-fails");
        assert!(claim("beta-mark-fails").await.is_err());
        assert_eq!(
            store
                .domain_host(domain)
                .await
                .unwrap()
                .unwrap()
                .instance_id,
            "alpha-mark",
            "the lock was not taken"
        );
        assert!(store.host_move(domain).await.unwrap().is_none());
        assert_eq!(
            store.last_domain_host(domain).await.unwrap().unwrap().0,
            "alpha-mark"
        );

        assert!(claim("beta-mark").await.unwrap().claim.is_acquired());
        let mark = store.host_move(domain).await.unwrap().expect("marked");
        assert_eq!(mark.claimant.as_deref(), Some("beta-mark"));
        assert_eq!(
            mark.only_elsewhere.previous_host.as_deref(),
            Some("alpha-mark")
        );
        store.drop_schema().await.unwrap();
    }

    /// Only the instance that marked a move ends it: a complete walk that
    /// sees every row but names no walker, or names another, keeps the mark;
    /// the claimant's walk ends it.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn only_the_claimant_ends_a_move() {
        let Some(store) = shared_store().await else {
            return;
        };
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("eng");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("a.md"),
            "---\ntype: engram\ntitle: A\npermalink: a\nstatus: stable\n---\n\n# A\n",
        )
        .unwrap();
        let domain = store
            .upsert_domain(
                "eng",
                Some(&root.to_string_lossy()),
                crate::DomainKind::File,
            )
            .await
            .unwrap();
        let (now, stale_before) = claim_window(90);
        claim_host_marking_moves(&store, domain, "gamma", "gamma", &now, &stale_before, true)
            .await
            .unwrap();
        let marked = store.host_move(domain).await.unwrap();
        assert!(marked.is_some(), "marked");
        let walk = |walker: Option<&'static str>| {
            let (store, root) = (&store, root.clone());
            async move {
                let stamps = store.file_stamps(domain).await.unwrap();
                let mut scan =
                    crate::scan_domain("eng", &root, stamps, &crate::ChunkParams::default(), false)
                        .await
                        .unwrap();
                if let Some(walker) = walker {
                    scan.walked_by(walker);
                }
                crate::apply_scan(store, domain, scan).await
            }
        };
        walk(None).await.unwrap();
        assert_eq!(
            store.host_move(domain).await.unwrap(),
            marked,
            "a walk that names no walker leaves the move exactly as the claimant set it"
        );
        let refused = walk(Some("delta")).await.unwrap_err();
        assert!(
            matches!(refused, crate::IndexError::NotHost(_)),
            "{refused}"
        );
        let report = walk(Some("gamma")).await.unwrap();
        assert!(report.host_move_resolved);
        assert!(store.host_move(domain).await.unwrap().is_none());
        store.drop_schema().await.unwrap();
    }

    /// A move stored in an older shape (no `claimant`) is matched as stored:
    /// the claimant's walk ends it as it ends any other.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_move_in_an_older_shape_resolves() {
        use sqlx::Connection;
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let store = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let (_tmp, root) = domain_folder();
        let domain = store
            .upsert_domain(
                "eng",
                Some(&root.to_string_lossy()),
                crate::DomainKind::File,
            )
            .await
            .unwrap();
        let (now, stale_before) = claim_window(90);
        claim_host_marking_moves(
            &store,
            domain,
            "alpha-o",
            "alpha-o",
            &now,
            &stale_before,
            false,
        )
        .await
        .unwrap();
        walk_as(&store, domain, &root, "alpha-o").await.unwrap();
        assert!(store.host_move(domain).await.unwrap().is_none());

        let mut conn = sqlx::PgConnection::connect(&url).await.unwrap();
        sqlx::raw_sql(sqlx::AssertSqlSafe(format!(
            "UPDATE {schema}.domain SET host_move = '{{\"at\":\"2026-01-01T00:00:00+00:00\",\"walked\":true,\"only_elsewhere\":{{\"previous_host\":\"old\",\"engrams\":1,\"attachments\":0,\"pages\":0,\"files\":0}}}}' WHERE id={}",
            domain.0
        )))
        .execute(&mut conn)
        .await
        .unwrap();
        let legacy = store.host_move(domain).await.unwrap().expect("read back");
        assert_eq!(legacy.claimant, None);

        let report = walk_as(&store, domain, &root, "alpha-o").await.unwrap();
        assert!(report.host_move_resolved, "{report:?}");
        assert!(store.host_move(domain).await.unwrap().is_none());
        store.drop_schema().await.unwrap();
    }

    /// When the new host of `eng` stamped its own rebuild marker.
    #[cfg(feature = "postgres")]
    const BETA_REBUILD: &str = "2030-01-01T00:00:00+00:00";

    /// The hooks of a reindex that claims as `alpha-r`.
    #[cfg(feature = "postgres")]
    struct ClaimsAsAlpha {
        lost: std::sync::Mutex<Vec<String>>,
    }

    #[cfg(feature = "postgres")]
    #[async_trait::async_trait]
    impl crate::ReindexHooks for ClaimsAsAlpha {
        async fn before_domain(
            &self,
            store: &dyn Store,
            name: &str,
            root: &std::path::Path,
        ) -> crate::error::Result<bool> {
            let domain = store
                .upsert_domain(name, Some(&root.to_string_lossy()), crate::DomainKind::File)
                .await?;
            let (now, stale_before) = claim_window(90);
            claim_host_marking_moves(
                store,
                domain,
                "alpha-r",
                "alpha-r",
                &now,
                &stale_before,
                false,
            )
            .await?;
            Ok(true)
        }

        async fn not_applied(&self, name: &str, _why: &str, host_lost: bool) {
            assert!(host_lost);
            self.lost.lock().unwrap().push(name.to_string());
        }

        fn walker(&self) -> Option<String> {
            Some("alpha-r".to_string())
        }
    }

    /// A reindex whose domain is taken over while it walks skips that
    /// domain, names it and goes on with the next domain. It clears the
    /// rebuild marker it stamped there, unless the new host stamped its own
    /// since, which stays.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_deposed_reindex_skips_the_domain_and_goes_on() {
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let a = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let other = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let (_eng_tmp, eng) = domain_folder();
        let (_ops_tmp, ops) = domain_folder();
        let (_web_tmp, web) = domain_folder();
        let store: std::sync::Arc<tokio::sync::Mutex<dyn Store>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(a));
        let hooks = std::sync::Arc::new(ClaimsAsAlpha {
            lost: std::sync::Mutex::new(Vec::new()),
        });
        let gates = [
            ("eng", crate::sync::apply_seam::arm("reindex:eng")),
            ("web", crate::sync::apply_seam::arm("reindex:web")),
        ];
        let run = tokio::spawn({
            let (store, hooks) = (store.clone(), hooks.clone());
            async move {
                crate::reindex_domains(
                    &*store,
                    &[
                        ("eng".to_string(), eng),
                        ("ops".to_string(), ops),
                        ("web".to_string(), web),
                    ],
                    &crate::ChunkParams::default(),
                    Some(crate::RebuildKind::Full),
                    &*hooks,
                )
                .await
            }
        });
        // Beta takes each over once this run stamped its marker; on `eng`
        // beta then starts a rebuild of its own.
        for (name, (entered, release)) in gates {
            entered.notified().await;
            let domain = other.domain_id(name).await.unwrap().unwrap();
            let (now, stale_before) = claim_window(90);
            claim_host_marking_moves(
                &other,
                domain,
                "beta-r",
                "beta-r",
                &now,
                &stale_before,
                true,
            )
            .await
            .unwrap();
            if name == "eng" {
                other
                    .begin_rebuild(domain, BETA_REBUILD, crate::RebuildKind::Full)
                    .await
                    .unwrap();
            }
            release.notify_one();
        }
        let reports = run.await.unwrap().unwrap();
        let applied: Vec<&str> = reports.iter().map(|r| r.domain.as_str()).collect();
        assert_eq!(applied, ["ops"]);
        assert_eq!(*hooks.lost.lock().unwrap(), ["eng", "web"]);
        let stats = other.domain_stats().await.unwrap();
        let of = |name: &str| stats.iter().find(|d| d.name == name).unwrap().clone();
        assert_eq!(
            of("eng").rebuild_started.as_deref(),
            Some(BETA_REBUILD),
            "the new host's marker stays"
        );
        assert!(
            of("web").rebuild_started.is_none(),
            "this run's marker goes"
        );
        assert_eq!(of("eng").engrams, 0, "nothing of the walk was written");
        other.drop_schema().await.unwrap();
    }

    /// The hooks of a reindex that claims as `id` and records each domain
    /// it could not apply, with whether the host was lost.
    #[cfg(feature = "postgres")]
    struct ClaimsAndRecords {
        id: &'static str,
        not_applied: std::sync::Mutex<Vec<(String, bool)>>,
    }

    #[cfg(feature = "postgres")]
    #[async_trait::async_trait]
    impl crate::ReindexHooks for ClaimsAndRecords {
        async fn before_domain(
            &self,
            store: &dyn Store,
            name: &str,
            root: &std::path::Path,
        ) -> crate::error::Result<bool> {
            let domain = store
                .upsert_domain(name, Some(&root.to_string_lossy()), crate::DomainKind::File)
                .await?;
            let (now, stale_before) = claim_window(90);
            claim_host_marking_moves(store, domain, self.id, self.id, &now, &stale_before, false)
                .await?;
            Ok(true)
        }

        async fn not_applied(&self, name: &str, _why: &str, host_lost: bool) {
            self.not_applied
                .lock()
                .unwrap()
                .push((name.to_string(), host_lost));
        }

        fn walker(&self) -> Option<String> {
            Some(self.id.to_string())
        }
    }

    /// A reindex whose host move another walk changed while it ran applies
    /// nothing of that domain, and says so with the host kept: the lock
    /// still names this instance, so it goes on hosting the domain.
    #[cfg(feature = "postgres")]
    #[tokio::test]
    async fn a_reindex_whose_move_changed_meanwhile_keeps_the_host() {
        let Some((url, schema)) = shared_schema() else {
            return;
        };
        let a = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let other = crate::PostgresStore::open_in_schema(&url, &schema)
            .await
            .unwrap();
        let (_tmp, root) = domain_folder();
        let store: std::sync::Arc<tokio::sync::Mutex<dyn Store>> =
            std::sync::Arc::new(tokio::sync::Mutex::new(a));
        let hooks = std::sync::Arc::new(ClaimsAndRecords {
            id: "alpha-m",
            not_applied: std::sync::Mutex::new(Vec::new()),
        });
        let (entered, release) = crate::sync::apply_seam::arm("alpha-m");
        let run = tokio::spawn({
            let (store, hooks) = (store.clone(), hooks.clone());
            async move {
                crate::reindex_domains(
                    &*store,
                    &[("eng".to_string(), root)],
                    &crate::ChunkParams::default(),
                    None,
                    &*hooks,
                )
                .await
            }
        });
        entered.notified().await;
        let domain = other.domain_id("eng").await.unwrap().unwrap();
        let (now, _) = claim_window(90);
        let written = HostMove {
            at: now,
            claimant: Some("alpha-m".to_string()),
            walked: true,
            only_elsewhere: OnlyElsewhere::default(),
        };
        other.set_host_move(domain, Some(&written)).await.unwrap();
        release.notify_one();
        let reports = run.await.unwrap().unwrap();
        assert!(reports.is_empty(), "{reports:?}");
        assert_eq!(
            *hooks.not_applied.lock().unwrap(),
            [("eng".to_string(), false)],
            "not applied, and the host is not lost"
        );
        assert_eq!(
            other
                .domain_host(domain)
                .await
                .unwrap()
                .unwrap()
                .instance_id,
            "alpha-m"
        );
        assert_eq!(other.host_move(domain).await.unwrap(), Some(written));
        other.drop_schema().await.unwrap();
    }
}
