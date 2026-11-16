use super::*;
use crate::auth_store::db::test_store;

/// Whether [`open_database`] is expected to reach multiprocess WAL on the
/// platform these tests are running on, rather than falling back to a
/// legacy open. Observable as the `web-auth.db-tshm` coordination file
/// beside the database, which only the multiprocess open creates.
///
/// Stated once here, with its two upstream conditions, so no assertion has
/// to carry a `cfg` of its own:
///
/// * turso_core's `host_shared_wal` cfg, which its `build.rs` sets to
///   `all(any(unix, target_os = "windows"), target_pointer_width = "64")`.
///   Where it is off the flag is a documented no-op and legacy behavior
///   stays, so a 32-bit target never gets the coordination file.
/// * an IO backend whose `supports_shared_wal_coordination` is true
///   (turso_core 0.7.2 and 0.8.1 `io/mod.rs`, where the trait default is `false`).
///   The unix and io_uring backends override it to `true`; the default
///   Windows backend, `WindowsIO`, does not, and only `WindowsIOCP`,
///   compiled only under the off-by-default `experimental_win_iocp` cargo
///   feature, does. So a Windows open takes the fallback.
///
/// The day either changes upstream, whatever reads this goes red rather
/// than quietly stale, which is the point of asserting the mode at all.
const MULTIPROCESS_WAL_EXPECTED: bool = cfg!(unix) && cfg!(target_pointer_width = "64");

/// A GitHub delivery id is new once: the second time within a day it is
/// a replay.
#[tokio::test]
async fn a_delivery_is_remembered_once() {
    let (store, _guard) = test_store().await;
    assert!(store.remember_delivery("d-1").await.unwrap());
    assert!(!store.remember_delivery("d-1").await.unwrap());
    assert!(store.remember_delivery("d-2").await.unwrap());
}

/// A delivery older than a day is pruned, so its id counts as new again;
/// one younger than a day stays.
#[tokio::test]
async fn a_delivery_older_than_a_day_is_forgotten() {
    let (store, _guard) = test_store().await;
    let now = chrono::Utc::now().timestamp();
    assert!(
        store
            .remember_delivery_at("old", now - 86_401)
            .await
            .unwrap()
    );
    assert!(
        store
            .remember_delivery_at("young", now - 86_399)
            .await
            .unwrap()
    );
    assert!(store.remember_delivery_at("old", now).await.unwrap());
    assert!(!store.remember_delivery_at("young", now).await.unwrap());
}

/// A delivery whose handling failed is forgotten, so GitHub's redelivery
/// is handled.
#[tokio::test]
async fn a_forgotten_delivery_is_new_again() {
    let (store, _guard) = test_store().await;
    assert!(store.remember_delivery("d-1").await.unwrap());
    store.forget_delivery("d-1").await.unwrap();
    assert!(store.remember_delivery("d-1").await.unwrap());
}

#[tokio::test]
async fn password_roundtrip() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    assert!(
        store
            .verify_password("ada", "correct horse")
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        store
            .verify_password("ada", "wrong")
            .await
            .unwrap()
            .is_none()
    );
}

/// The brief mandates argon2id specifically, and `Argon2::default()` is
/// what supplies it, so pin the algorithm the PHC string records. The cost
/// parameters are deliberately not pinned: those are meant to track the
/// crate's recommendation, not a number frozen here.
#[tokio::test]
async fn passwords_are_stored_as_argon2id_phc_strings() {
    let hash = hash_password("hunter2").await.unwrap();
    assert!(
        hash.starts_with("$argon2id$"),
        "expected an argon2id PHC string, got {hash}"
    );
    assert!(
        verify_hash(hash.clone(), "hunter2".to_string())
            .await
            .unwrap()
    );
    assert!(!verify_hash(hash, "other".to_string()).await.unwrap());
}

/// A hash written by argon2 0.5.3 still verifies after the 0.6 bump. Every
/// account provisioned before the upgrade carries one of these, so this is
/// the test that says an upgrade is not a mass password reset. Captured
/// verbatim from argon2 0.5.3 with `Argon2::default()` on 2026-09-14; the
/// password it was written for is beside it, because a hash without its
/// password pins nothing.
#[tokio::test]
async fn a_hash_written_under_argon2_0_5_still_verifies() {
    const PASSWORD: &str = "correct horse battery staple";
    const HASH_0_5: &str = "$argon2id$v=19$m=19456,t=2,p=1$lU1KHTS8csu8CtBAN/g/Zw$gq+EDJ0qxuAHNNk1oxZGDP88YjooUWCCp14VWhB+b30";

    assert!(
        verify_hash(HASH_0_5.to_string(), PASSWORD.to_string())
            .await
            .unwrap(),
        "the stored parameters travel in the PHC string, so the hash verifies whatever this \
         crate's current defaults are"
    );
    assert!(
        !verify_hash(HASH_0_5.to_string(), "wrong".to_string())
            .await
            .unwrap(),
        "and it still refuses the wrong password"
    );
}

#[tokio::test]
async fn the_same_password_hashes_differently_each_time() {
    let a = hash_password("pw").await.unwrap();
    let b = hash_password("pw").await.unwrap();
    assert_ne!(a, b, "every hash carries its own random salt");
}

#[tokio::test]
async fn a_corrupt_hash_fails_closed() {
    assert!(
        !verify_hash("not a phc string".to_string(), "pw".to_string())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unknown_user_never_verifies() {
    let (store, _guard) = test_store().await;
    assert!(
        store
            .verify_password("nobody", "pw")
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn disabled_user_never_verifies() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    store.set_disabled("ada", true).await.unwrap();
    assert!(store.verify_password("ada", "pw").await.unwrap().is_none());
    store.set_disabled("ada", false).await.unwrap();
    assert!(store.verify_password("ada", "pw").await.unwrap().is_some());
}

#[tokio::test]
async fn session_create_lookup_and_expiry() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", Some("ada@example.com"), Role::Editor, "pw")
        .await
        .unwrap();

    let live = store.create_session("ada", 3600).await.unwrap();
    assert_eq!(live.token.len(), 64);
    assert_eq!(live.csrf.len(), 64);
    assert_ne!(live.token, live.csrf);
    let (user, csrf) = store.session_user(&live.token).await.unwrap().unwrap();
    assert_eq!(user.name, "ada");
    assert_eq!(user.email.as_deref(), Some("ada@example.com"));
    assert_eq!(user.role, Role::Editor);
    assert_eq!(csrf, live.csrf);

    let expired = store.create_session("ada", -1).await.unwrap();
    assert!(store.session_user(&expired.token).await.unwrap().is_none());

    assert!(store.session_user("not-a-token").await.unwrap().is_none());
    // The live session survived the expired one's prune.
    assert!(store.session_user(&live.token).await.unwrap().is_some());
}

/// The quiet lookup answers what `session_user` answers and writes
/// nothing. Catches a lookup that stamped `last_seen_at` (every open tab
/// would read as a fresh sighting) or pruned (a write per check), and
/// one that let an expired session or a disabled account through. The
/// account is disabled under its session row here: `set_disabled`
/// deletes the sessions with it, so only a row an older build left
/// behind reaches the filter, and the filter is what keeps it out.
#[tokio::test]
async fn the_quiet_session_lookup_reads_without_writing() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let live = store.create_session("ada", 3600).await.unwrap();
    let expired = store.create_session("ada", -1).await.unwrap();
    let seen = store.user("ada").await.unwrap().unwrap().last_seen;
    let user = store
        .session_user_quiet(&live.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((user.name.as_str(), user.role), ("ada", Role::Editor));
    assert!(
        store
            .session_user_quiet(&expired.token)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .session_user_quiet("not-a-token")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.user("ada").await.unwrap().unwrap().last_seen,
        seen,
        "not stamped"
    );
    assert_eq!(
        store.session_count().await.unwrap(),
        2,
        "the expired row was not pruned"
    );
    store.set_role("ada", Role::Viewer).await.unwrap();
    let user = store
        .session_user_quiet(&live.token)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(user.role, Role::Viewer, "the stored role, read now");
    disable_keeping_sessions(&store, "ada").await;
    assert!(
        store
            .session_user_quiet(&live.token)
            .await
            .unwrap()
            .is_none(),
        "a disabled account's surviving session row answers none"
    );
}

/// Flag `name` disabled and leave its session rows alone, the state a
/// file written before disabling revoked sessions can hold.
async fn disable_keeping_sessions(store: &AuthStore, name: &str) {
    let _guard = store.guard.lock().await;
    store
        .db
        .execute(
            "UPDATE users SET disabled = 1 WHERE name = ?1",
            vec![Value::Text(name.to_string())],
        )
        .await
        .unwrap();
}

/// `ensure_session` issues at most one session per account: the first call
/// creates, every later one reuses, and only an account with nothing live
/// gets a second row. This is what keeps `/auth/me` from adding a session
/// per probe for a trusted-header client that keeps no cookie.
#[tokio::test]
async fn ensure_session_reuses_a_live_session_rather_than_adding_one() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    assert_eq!(store.session_count().await.unwrap(), 0);
    assert!(store.newest_session_csrf("ada").await.unwrap().is_none());

    let SessionMint::Created(first) = store.ensure_session("ada", 3600).await.unwrap() else {
        panic!("the first call has nothing to reuse, so it creates");
    };
    assert_eq!(store.session_count().await.unwrap(), 1);

    // Ten more probes, all reusing: the row count does not move and the
    // token does not change, which is what keeps a second tab working.
    for _ in 0..10 {
        let mint = store.ensure_session("AdA", 3600).await.unwrap();
        assert!(
            matches!(mint, SessionMint::Reused { .. }),
            "a live session must be reused, not duplicated"
        );
        assert_eq!(mint.csrf(), first.csrf);
    }
    assert_eq!(store.session_count().await.unwrap(), 1);
    assert_eq!(
        store.newest_session_csrf("ada").await.unwrap().as_deref(),
        Some(first.csrf.as_str())
    );
    // The created session is still the one the cookie resolves to.
    let (_, csrf) = store.session_user(&first.token).await.unwrap().unwrap();
    assert_eq!(csrf, first.csrf);

    // An expired session is not live, so the next probe issues a fresh one
    // and takes the dead row with it. `session_user` is the only other
    // pruner and a cookieless probe never reaches it, so without this an
    // account whose session lapsed would leave a row behind every time.
    store.delete_session(&first.token).await.unwrap();
    store.create_session("ada", -1).await.unwrap();
    assert_eq!(store.session_count().await.unwrap(), 1, "the expired row");
    assert!(store.newest_session_csrf("ada").await.unwrap().is_none());
    let SessionMint::Created(second) = store.ensure_session("ada", 3600).await.unwrap() else {
        panic!("nothing live is left to reuse, so this creates");
    };
    assert_ne!(second.csrf, first.csrf);
    assert_eq!(
        store.session_count().await.unwrap(),
        1,
        "the expired row was pruned rather than left beside the new one"
    );

    // Another account's live session is never handed over.
    store
        .add_user("bob", "Bob", None, Role::Editor, "pw")
        .await
        .unwrap();
    assert!(store.newest_session_csrf("bob").await.unwrap().is_none());
    let SessionMint::Created(bobs) = store.ensure_session("bob", 3600).await.unwrap() else {
        panic!("bob holds nothing, so this creates");
    };
    assert_ne!(bobs.csrf, second.csrf);

    // An account nobody created has no session to ensure.
    assert!(store.ensure_session("ghost", 3600).await.is_err());
}

/// Concurrent probes, which is the two-tabs-at-once case: the check and the
/// insert are one transaction, so exactly one session is created however
/// many arrive together, and every caller is handed the same token. Without
/// the transaction each tab would get its own token and only the one whose
/// `Set-Cookie` landed last would be able to write.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_probes_settle_on_one_session() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let store = std::sync::Arc::new(store);

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.ensure_session("ada", 3600).await.unwrap()
        }));
    }
    let mints: Vec<SessionMint> = futures::future::join_all(tasks)
        .await
        .into_iter()
        .map(|t| t.unwrap())
        .collect();

    assert_eq!(
        store.session_count().await.unwrap(),
        1,
        "one session however many probes raced for it"
    );
    let created = mints
        .iter()
        .filter(|m| matches!(m, SessionMint::Created(_)))
        .count();
    assert_eq!(created, 1, "exactly one caller created it");
    let token = mints[0].csrf();
    for mint in &mints {
        assert_eq!(mint.csrf(), token, "every tab was handed the same token");
    }
}

#[tokio::test]
async fn a_disabled_user_loses_a_live_session() {
    let (store, _guard) = test_store().await;
    // Editor rather than admin: the role is incidental here, and the last
    // enabled admin cannot be disabled (see [`NOT_LAST_ADMIN`]).
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let s = store.create_session("ada", 3600).await.unwrap();
    store.set_disabled("ada", true).await.unwrap();
    assert!(store.session_user(&s.token).await.unwrap().is_none());
}

/// Disabling is a revocation, not a flag over the session rows: re-enabling
/// the account must not hand back the cookies it held before. Otherwise
/// disabling a compromised account and enabling it again once the password
/// was changed would restore the intruder's session.
#[tokio::test]
async fn re_enabling_does_not_resurrect_the_sessions_disabling_took() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let s = store.create_session("ada", 3600).await.unwrap();
    store.set_disabled("ada", true).await.unwrap();
    store.set_disabled("ada", false).await.unwrap();
    assert!(
        store.session_user(&s.token).await.unwrap().is_none(),
        "the session was deleted, not merely hidden while the flag was set"
    );
    // The account itself is back, and a fresh session works.
    let fresh = store.create_session("ada", 3600).await.unwrap();
    assert!(store.session_user(&fresh.token).await.unwrap().is_some());
}

/// A password reset evicts whoever was signed in under the old one: a
/// session never presents a password again, so without this the holder of a
/// cookie minted before the reset keeps the account.
#[tokio::test]
async fn changing_a_password_revokes_the_sessions_it_issued() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let before = store.create_session("ada", 3600).await.unwrap();
    store.set_password("ada", "new pw").await.unwrap();
    assert!(
        store.session_user(&before.token).await.unwrap().is_none(),
        "the cookie from before the reset is dead"
    );
    assert!(
        store
            .verify_password("ada", "new pw")
            .await
            .unwrap()
            .is_some(),
        "and the new password logs in"
    );
    let after = store.create_session("ada", 3600).await.unwrap();
    assert!(store.session_user(&after.token).await.unwrap().is_some());
}

/// A refused edit revokes nothing: the transaction rolls back with the
/// sessions in it, the same property `a_refused_removal_keeps_the_admins_sessions`
/// pins for a removal.
#[tokio::test]
async fn a_refused_edit_leaves_the_sessions_alone() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let live = store.create_session("ada", 3600).await.unwrap();
    assert!(
        store.set_disabled("ada", true).await.is_err(),
        "the last enabled admin cannot be disabled"
    );
    assert!(
        store.session_user(&live.token).await.unwrap().is_some(),
        "the rolled-back disabling must leave the live session in place"
    );
    assert!(store.set_password("ghost", "new pw").await.is_err());
    assert!(store.session_user(&live.token).await.unwrap().is_some());
}

#[tokio::test]
async fn the_plaintext_token_is_not_in_the_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    let store = AuthStore::open(&path).await.unwrap();
    store
        .add_user("ada", "Ada", None, Role::Admin, "hunter2")
        .await
        .unwrap();
    let s = store.create_session("ada", 3600).await.unwrap();
    let stored = store
        .db
        .query_first(
            "SELECT token_hash FROM sessions WHERE token_hash = ?1",
            vec![Value::Text(token_hash(&s.token))],
        )
        .await
        .unwrap();
    assert!(stored.is_some(), "the session is stored by its hash");

    // Read the main database *and* its sidecars. A freshly written row
    // lives in `-wal` until a checkpoint moves it, so scanning only
    // `web-auth.db` would pass without proving anything.
    let mut haystack = String::new();
    let mut files_read = 0;
    for suffix in ["", "-wal", "-shm"] {
        let mut p = path.as_os_str().to_os_string();
        p.push(suffix);
        if let Ok(bytes) = std::fs::read(std::path::PathBuf::from(p)) {
            files_read += 1;
            haystack.push_str(&String::from_utf8_lossy(&bytes));
        }
    }
    assert!(files_read > 0, "at least the main database file must exist");
    // The hash is what proves the sweep can actually see session bytes; if
    // this fails the search covered no live data and the assertions below
    // would be vacuous.
    assert!(
        haystack.contains(&token_hash(&s.token)),
        "the sweep must reach the bytes the session was written into"
    );
    assert!(
        !haystack.contains(&s.token),
        "the raw token is never stored"
    );
    assert!(
        !haystack.contains("hunter2"),
        "the password is never stored"
    );
}

/// The resurrection bug: a session row that outlived its account is
/// inherited by the next account to claim the freed name.
#[tokio::test]
async fn a_readded_name_does_not_inherit_the_old_holders_session() {
    let (store, _guard) = test_store().await;
    // Editor rather than admin: the role is incidental, and the last
    // enabled admin cannot be removed (see [`NOT_LAST_ADMIN`]).
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let old = store.create_session("ada", 3600).await.unwrap();
    store.remove_user("ada").await.unwrap();
    store
        .add_user("ada", "Ada The Second", None, Role::Viewer, "pw2")
        .await
        .unwrap();
    assert!(
        store.session_user(&old.token).await.unwrap().is_none(),
        "the previous holder's token must not resolve to the new account"
    );
}

/// A session row left behind by an older build (or any other route) must
/// not become live again when the name is re-registered.
#[tokio::test]
async fn an_orphaned_session_row_is_swept_rather_than_inherited() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let old = store.create_session("ada", 3600).await.unwrap();
    // Delete only the user row, exactly what the pre-fix ordering could
    // leave behind if it crashed between its two statements.
    store
        .db
        .execute(
            "DELETE FROM users WHERE name = ?1",
            vec![Value::Text("ada".to_string())],
        )
        .await
        .unwrap();
    assert!(store.session_user(&old.token).await.unwrap().is_none());
    store
        .add_user("ada", "Ada The Second", None, Role::Viewer, "pw2")
        .await
        .unwrap();
    assert!(
        store.session_user(&old.token).await.unwrap().is_none(),
        "the orphan must have been swept, not left dormant"
    );
}

#[tokio::test]
async fn remove_user_reports_a_missing_account_without_touching_anything() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let live = store.create_session("ada", 3600).await.unwrap();
    assert!(store.remove_user("ghost").await.is_err());
    // The rollback left the unrelated account and its session intact.
    assert!(store.session_user(&live.token).await.unwrap().is_some());
    assert_eq!(store.list_users().await.unwrap().len(), 1);
}

/// The display name is editable, and clearing it falls back to the login
/// name: "optional" means a client may always unset it, never that a row
/// goes nameless.
#[tokio::test]
async fn display_names_are_editable_and_clearing_resets_to_the_login_name() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();

    store
        .set_display("ada", Some("Ada Lovelace"))
        .await
        .unwrap();
    assert_eq!(store.list_users().await.unwrap()[0].display, "Ada Lovelace");

    store.set_display("ADA", None).await.unwrap();
    assert_eq!(store.list_users().await.unwrap()[0].display, "ada");

    store.set_display("ada", Some("   ")).await.unwrap();
    assert_eq!(
        store.list_users().await.unwrap()[0].display,
        "ada",
        "blank is a clear, not a display name of spaces"
    );

    assert!(store.set_display("ghost", Some("Ghost")).await.is_err());
}

/// Two `AuthStore` handles on one file see each other's writes as they
/// happen, in the order the `crystalline users` CLI and the daemon do it.
///
/// **Same process only, and deliberately so.** These two handles are not
/// two processes and cannot stand in for them: turso keeps a process-wide
/// registry of open databases keyed by file identity
/// (`Database::lookup_in_registry`), so the second `open` here is handed
/// the first one's `Database` back and never opens the file a second time.
/// That makes this a test of statement ordering and visibility through one
/// engine, which is worth having, and no evidence at all about the file
/// locking between processes that [`open_database`] exists to solve. The
/// test that does spawn a second process is
/// `users_add_works_while_another_process_holds_the_auth_db` in the CLI's
/// `tests/users.rs`, and it is the one that fails if the multiprocess WAL
/// flag is dropped.
#[tokio::test]
async fn two_stores_on_one_file_interleave_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    let daemon = AuthStore::open(&path).await.unwrap();
    let cli = AuthStore::open(&path).await.unwrap();

    // The CLI adds an account; the daemon sees it without reopening.
    cli.add_user("ada", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();
    let session = daemon.create_session("ada", 3600).await.unwrap();
    let (user, _) = daemon.session_user(&session.token).await.unwrap().unwrap();
    assert_eq!(user.role, Role::Viewer);

    // The CLI promotes; the daemon's next lookup reflects it. Editor, not
    // admin: this account is disabled and removed below, which the
    // last-admin guard would refuse (see [`NOT_LAST_ADMIN`]).
    cli.set_role("ada", Role::Editor).await.unwrap();
    let (user, _) = daemon.session_user(&session.token).await.unwrap().unwrap();
    assert_eq!(user.role, Role::Editor, "the daemon sees the CLI's write");

    // The daemon writes; the CLI reads it back.
    let second = daemon.create_session("ada", 3600).await.unwrap();
    assert!(cli.session_user(&second.token).await.unwrap().is_some());

    // The CLI disables, which revokes; the daemon no longer resolves the
    // session the CLI's write deleted.
    cli.set_disabled("ada", true).await.unwrap();
    assert!(daemon.session_user(&session.token).await.unwrap().is_none());

    // The CLI removes the account entirely, sessions and all, while the
    // daemon still holds its own handle open.
    cli.set_disabled("ada", false).await.unwrap();
    cli.remove_user("ada").await.unwrap();
    assert!(daemon.session_user(&second.token).await.unwrap().is_none());
    assert!(daemon.list_users().await.unwrap().is_empty());
}

/// The upgrade path: a `web-auth.db` written by a build that opened
/// without the multiprocess WAL flag must open, and keep its accounts,
/// under a build that opens with it.
///
/// The two modes are exclusive only while a database is *live* in the
/// other mode (turso probes both directions on open). Nothing about the
/// file itself is mode-specific: the coordination file is a separate
/// `-tshm` sibling created on demand, so an existing state directory needs
/// no migration. This writes the file exactly as the previous build did,
/// closes it and reopens it the way [`open_database`] now does.
///
/// The title's promise - that it still opens, with its accounts, and stays
/// writable - is asserted everywhere. Which mode did the opening is
/// asserted against [`MULTIPROCESS_WAL_EXPECTED`], because on Windows
/// multiprocess WAL is refused and the fallback is the correct outcome
/// there, not a defect.
#[tokio::test]
async fn a_database_written_without_the_multiprocess_flag_still_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    {
        let legacy = Builder::new_local(&path.to_string_lossy())
            .build()
            .await
            .unwrap();
        let conn = legacy.connect().unwrap();
        conn.execute_batch(SCHEMA).await.unwrap();
        conn.execute(
            "INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
             VALUES ('ada', 'Ada', NULL, 'admin', 'x', 0, '2026-01-01T00:00:00Z')",
            (),
        )
        .await
        .unwrap();
    }

    let store = AuthStore::open(&path)
        .await
        .expect("an existing legacy-mode database must open");
    let users = store.list_users().await.unwrap();
    assert_eq!(users.len(), 1, "the accounts survived the mode change");
    assert_eq!(users[0].name, "ada");
    // The account is still editable, so the reopen is a real read-write
    // open and not a degraded one.
    store.set_role("ada", Role::Admin).await.unwrap();
    assert_eq!(
        path.with_file_name("web-auth.db-tshm").exists(),
        MULTIPROCESS_WAL_EXPECTED,
        "the mode that opened the file must be the one this platform can \
         have (see MULTIPROCESS_WAL_EXPECTED): multiprocess leaves the \
         coordination file beside the database, the fallback leaves none"
    );
}

/// The fallback in [`open_database`] hinges on recognizing turso's own
/// "not supported" wording, which reaches us as a string rather than a
/// typed error. A memory-like path is the one way to provoke that message
/// without a network filesystem to hand, so it is what pins it: if a turso
/// upgrade rewords it, this fails here, rather than the fallback silently
/// going missing on the NFS or SMB state directory it exists for.
#[tokio::test]
async fn turso_still_words_an_unsupported_multiprocess_open_the_way_we_match() {
    let err = Builder::new_local(":memory:")
        .experimental_multiprocess_wal(true)
        .build()
        .await
        .expect_err("multiprocess WAL cannot be had on an in-memory path");
    assert!(
        is_multiprocess_unsupported(&err),
        "the fallback no longer recognizes turso's message: {err}"
    );
}

/// A locked fallback open is the one failure a user can do something
/// about, so it must say what to do rather than hand back turso's byte
/// range wording. The error is synthesized here because the platform that
/// produces it (Windows, whose default IO backend has no shared WAL
/// coordination) is not the platform this test usually runs on: the
/// mapping is a pure function of the message, so it is testable
/// everywhere.
#[test]
fn a_locked_fallback_open_says_what_holds_the_database() {
    let locked = turso::Error::Error(
        "Locking error: Failed locking file, The process cannot access the file because \
         another process has locked a portion of the file. (os error 33)"
            .to_string(),
    );
    let err = legacy_open_error(locked, Path::new("/state/web-auth.db"));
    let text = format!("{err:#}");
    assert!(
        text.contains("held by a running daemon"),
        "the message must name what holds the file: {text}"
    );
    assert!(
        text.contains("crystalline ctl shutdown"),
        "the message must name the way out: {text}"
    );
    assert!(
        text.contains("web-auth.db"),
        "the message must name the file: {text}"
    );
}

/// Any other reason a fallback open fails is not the locked case and must
/// keep the plain context, so a corrupt file or a missing directory is not
/// reported as a running daemon.
#[test]
fn a_fallback_open_that_fails_for_another_reason_keeps_the_plain_context() {
    let other = turso::Error::Error("file is not a database".to_string());
    let err = legacy_open_error(other, Path::new("/state/web-auth.db"));
    let text = format!("{err:#}");
    assert!(
        text.contains("opening auth database"),
        "an unrelated failure keeps the plain wording: {text}"
    );
    assert!(
        !text.contains("running daemon"),
        "an unrelated failure must not blame a daemon: {text}"
    );
}

/// Two simultaneous logins in the daemon are two concurrent
/// `create_session` calls on the one shared `AuthStore`. Both open a
/// transaction on the same connection, so without serialization the second
/// fails outright with "cannot start a transaction within a transaction"
/// rather than queueing. Every call must succeed and every token must work.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_sessions_on_one_store_never_fail_spuriously() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();

    let mut tasks = Vec::new();
    for _ in 0..16 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            store.create_session("ada", 3600).await
        }));
    }
    let mut tokens = Vec::new();
    for task in tasks {
        let session = task
            .await
            .expect("the task must not panic")
            .expect("create_session must not fail under concurrency");
        tokens.push(session.token);
    }
    for token in &tokens {
        assert!(store.session_user(token).await.unwrap().is_some());
    }
}

/// The same shape with the two transactional methods mixed, plus the
/// autocommit ones, so nothing can slip into another call's transaction.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mixed_writes_on_one_store_never_fail_spuriously() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(
        AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap(),
    );
    // One admin nobody touches, so every task's promote-then-remove is
    // allowed: the last-admin guard's `EXISTS` subquery is thereby also
    // exercised under concurrency rather than short-circuited away.
    store
        .add_user("keeper", "Keeper", None, Role::Admin, "pw")
        .await
        .unwrap();
    for i in 0..8 {
        store
            .add_user(&format!("u{i}"), "U", None, Role::Editor, "pw")
            .await
            .unwrap();
    }

    let mut tasks = Vec::new();
    for i in 0..8 {
        let store = store.clone();
        tasks.push(tokio::spawn(async move {
            let name = format!("u{i}");
            store.create_session(&name, 3600).await?;
            store.set_role(&name, Role::Admin).await?;
            store.create_session(&name, 3600).await?;
            store.remove_user(&name).await?;
            store.list_users().await?;
            anyhow::Ok(())
        }));
    }
    for task in tasks {
        task.await
            .expect("the task must not panic")
            .expect("no call may fail under concurrency");
    }
    let left: Vec<String> = store
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect();
    assert_eq!(left, vec!["keeper".to_string()]);
}

#[tokio::test]
async fn delete_session_revokes_it() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();
    let s = store.create_session("ada", 3600).await.unwrap();
    assert!(store.session_user(&s.token).await.unwrap().is_some());
    store.delete_session(&s.token).await.unwrap();
    assert!(store.session_user(&s.token).await.unwrap().is_none());
    // Logging out twice is not an error.
    store.delete_session(&s.token).await.unwrap();
}

#[tokio::test]
async fn ensure_user_is_idempotent() {
    let (store, _guard) = test_store().await;
    let first = store
        .ensure_user("ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    let second = store
        .ensure_user("ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert_eq!(first.name, second.name);
    assert_eq!(first.role, second.role);
    assert_eq!(store.list_users().await.unwrap().len(), 1);
}

/// The provisioning cap: ensure_user refuses to mint an account past the cap,
/// while an existing account keeps resolving whatever the count is. This is
/// the trusted-header mitigation - a proxy misconfiguration (a header carrying
/// a session id, say) must not mint one account per request forever.
#[tokio::test]
async fn ensure_user_refuses_to_mint_past_the_cap() {
    let (store, _guard) = test_store().await;
    store.ensure_user("ada", Role::Viewer, 2).await.unwrap();
    store.ensure_user("bob", Role::Viewer, 2).await.unwrap();

    let err = store.ensure_user("cyd", Role::Viewer, 2).await.unwrap_err();
    assert!(
        err.to_string().contains("auth.max_users"),
        "the refusal names the setting: {err}"
    );
    assert_eq!(store.list_users().await.unwrap().len(), 2);

    // Existing accounts resolve regardless of the count-vs-cap state.
    assert_eq!(
        store
            .ensure_user("ada", Role::Viewer, 2)
            .await
            .unwrap()
            .name,
        "ada"
    );
    assert_eq!(
        store
            .ensure_user("ADA", Role::Viewer, 1)
            .await
            .unwrap()
            .name,
        "ada"
    );
}

#[tokio::test]
async fn ensure_user_keeps_an_admin_assigned_role() {
    let (store, _guard) = test_store().await;
    store
        .ensure_user("ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    store.set_role("ada", Role::Admin).await.unwrap();
    let again = store
        .ensure_user("ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert_eq!(again.role, Role::Admin);
}

#[tokio::test]
async fn a_provisioned_user_has_no_password_to_log_in_with() {
    let (store, _guard) = test_store().await;
    store
        .ensure_user("ada", Role::Editor, usize::MAX)
        .await
        .unwrap();
    assert!(store.verify_password("ada", "").await.unwrap().is_none());
    store.set_password("ada", "pw").await.unwrap();
    assert!(store.verify_password("ada", "pw").await.unwrap().is_some());
}

/// The privilege-restoration path: a trusted header carrying a case
/// variant must resolve to the existing account, not provision a fresh one
/// at the default role.
#[tokio::test]
async fn ensure_user_folds_case_instead_of_minting_a_second_account() {
    let (store, _guard) = test_store().await;
    // Editor rather than admin: what matters is that a privileged, then
    // disabled, account is not restored by a case variant, and the last
    // enabled admin could not be disabled here (see [`NOT_LAST_ADMIN`]).
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    store.set_disabled("ada", true).await.unwrap();

    let same = store
        .ensure_user("Ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert_eq!(same.name, "ada");
    assert!(same.disabled, "the disable must not have been undone");
    assert_eq!(same.role, Role::Editor, "the role must not have been reset");
    assert_eq!(
        store.list_users().await.unwrap().len(),
        1,
        "no second account may exist for a case variant"
    );
}

#[tokio::test]
async fn names_are_folded_and_trimmed_on_every_path() {
    let (store, _guard) = test_store().await;
    store
        .add_user("  AdA  ", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();
    let users = store.list_users().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].name, "ada", "stored folded and trimmed");

    // Every read and write path accepts any casing of the same name. The
    // role moves to editor rather than admin so the removal below is not
    // refused as a lockout (see [`NOT_LAST_ADMIN`]).
    assert!(store.verify_password("ADA", "pw").await.unwrap().is_some());
    store.set_role(" Ada ", Role::Editor).await.unwrap();
    let session = store.create_session("aDa", 3600).await.unwrap();
    let (user, _) = store.session_user(&session.token).await.unwrap().unwrap();
    assert_eq!(user.role, Role::Editor);
    store.set_password("ADA", "pw2").await.unwrap();
    assert!(store.verify_password("ada", "pw2").await.unwrap().is_some());
    store.remove_user("  ADA  ").await.unwrap();
    assert!(store.list_users().await.unwrap().is_empty());
}

#[tokio::test]
async fn a_case_variant_cannot_be_added_twice() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();
    assert!(
        store
            .add_user("ADA", "Impostor", None, Role::Admin, "pw2")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn an_empty_or_blank_name_is_rejected() {
    let (store, _guard) = test_store().await;
    for blank in ["", "   ", "\t\n"] {
        assert!(
            store
                .add_user(blank, "Nobody", None, Role::Viewer, "pw")
                .await
                .is_err(),
            "add_user must reject {blank:?}"
        );
        assert!(
            store
                .ensure_user(blank, Role::Viewer, usize::MAX)
                .await
                .is_err()
        );
        // A login attempt is a `None`, not an error, like every other bad
        // credential.
        assert!(store.verify_password(blank, "pw").await.unwrap().is_none());
    }
    assert!(store.list_users().await.unwrap().is_empty());
}

/// Login names are space-free: the readable form belongs in the display name.
/// Enforced in normalize_account_name so every path - add, ensure, verify, edit -
/// refuses the same way.
#[tokio::test]
async fn a_name_with_internal_whitespace_is_rejected_on_every_path() {
    let (store, _guard) = test_store().await;
    for name in ["ada lovelace", "ada\tlovelace", "a b c"] {
        assert!(
            store
                .add_user(name, "Ada", None, Role::Viewer, "pw")
                .await
                .is_err(),
            "add_user must reject {name:?}"
        );
        assert!(
            store
                .ensure_user(name, Role::Viewer, usize::MAX)
                .await
                .is_err()
        );
        assert!(store.set_role(name, Role::Admin).await.is_err());
        // A login attempt is a NoHash, not an error, like other bad names.
        assert!(store.verify_password(name, "pw").await.unwrap().is_none());
    }
    assert!(store.list_users().await.unwrap().is_empty());
    // Surrounding whitespace is still merely trimmed.
    store
        .add_user("  ada  ", "Ada Lovelace", None, Role::Viewer, "pw")
        .await
        .unwrap();
    assert_eq!(store.list_users().await.unwrap()[0].name, "ada");
}

/// A governed domain made private at connect names who chose it. The
/// machine owner holds no account, so its reserved name is taken without
/// the live-account check; any other name still needs a live account,
/// and an already-private domain is left as it stands.
#[tokio::test]
async fn set_governed_private_takes_the_owner_name_and_leaves_a_private_domain_alone() {
    let (store, _guard) = test_store().await;
    assert_eq!(
        store.set_governed_private("kb", "Owner").await.unwrap(),
        VisibilityWrite::Written
    );
    assert_eq!(
        store.domain_visibility("kb").await.unwrap().unwrap().owner,
        "owner"
    );
    assert!(store.set_governed_private("ops", "nobody").await.is_err());
    assert!(store.domain_visibility("ops").await.unwrap().is_none());
    assert_eq!(
        store.set_governed_private("kb", "owner").await.unwrap(),
        VisibilityWrite::AlreadyPrivate {
            owner: "owner".to_string()
        }
    );
}

/// Taking back a connect's private record deletes the acl row and no
/// membership row.
#[tokio::test]
async fn take_back_governed_private_deletes_only_the_acl_row() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    store
        .add_user("bea", "Bea", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    store
        .set_domain_visibility("kb", true, "ada")
        .await
        .unwrap();
    store
        .upsert_domain_member("kb", "bea", MemberLevel::Viewer, None, "ada")
        .await
        .unwrap();
    assert!(store.take_back_governed_private("kb").await.unwrap());
    assert!(store.domain_visibility("kb").await.unwrap().is_none());
    assert_eq!(store.domain_members("kb").await.unwrap().len(), 1);
    assert!(!store.take_back_governed_private("kb").await.unwrap());
}

/// The machine owner's own name is not a login.
///
/// `owner` is the actor key every private draft the CLI, the control socket
/// and a local stdio agent write is filed under. An account holding it
/// would read, edit, share and fold the machine owner's unshared work in
/// every reviewing domain at once - in both directions, since the owner
/// would be acting in that account's drafts too. Every path that names an
/// account folds through `normalize_account_name`, so every one of them
/// refuses it, whichever way it is spelled.
#[tokio::test]
async fn the_machine_owners_name_is_reserved_on_every_path() {
    let (store, _guard) = test_store().await;
    for name in ["owner", "Owner", "OWNER", "  owner  "] {
        let refusal = store
            .add_user(name, "Owner", None, Role::Admin, "pw12345678")
            .await
            .expect_err("add_user must refuse {name:?}");
        let message = refusal.to_string();
        assert!(
            message.contains("pick another name"),
            "the refusal teaches what to do instead: {message}"
        );
        assert!(
            store
                .ensure_user(name, Role::Viewer, usize::MAX)
                .await
                .is_err(),
            "a trusted header cannot provision it either: {name:?}"
        );
        assert!(
            store
                .add_first_admin(name, "Owner", "pw12345678")
                .await
                .is_err(),
            "and the first-run admin is not it either: {name:?}"
        );
        // The lookup paths are untouched: the machine owner's name is a
        // legitimate principal everywhere it already appears (it holds
        // drafts, it stands in a fold plan), so naming it answers "nobody
        // by that name" rather than refusing the name itself.
        assert!(store.verify_password(name, "pw").await.unwrap().is_none());
    }
    assert!(store.list_users().await.unwrap().is_empty());
    // A name that merely contains it is a name like any other.
    store
        .add_user("owner-ada", "Ada", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
}

#[test]
fn normalize_account_name_trims_folds_and_rejects_empty() {
    assert_eq!(normalize_account_name("  AdA  ").unwrap(), "ada");
    assert_eq!(normalize_account_name("Ada").unwrap(), "ada");
    assert!(normalize_account_name("").is_err());
    assert!(normalize_account_name("   ").is_err());
    assert!(normalize_account_name("ada lovelace").is_err());
    // The reserved name folds like any other here; only the account
    // creation paths refuse it.
    assert_eq!(normalize_account_name("Owner").unwrap(), "owner");
    assert!(normalize_new_account_name("Owner").is_err());
}

#[tokio::test]
async fn duplicate_add_errors() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    assert!(
        store
            .add_user("ada", "Ada Again", None, Role::Viewer, "pw2")
            .await
            .is_err()
    );
}

/// The count the first-run probe reads: zero on a fresh file, and one more
/// for every account however it was created.
#[tokio::test]
async fn user_count_is_zero_until_an_account_exists() {
    let (store, _guard) = test_store().await;
    assert_eq!(store.user_count().await.unwrap(), 0);
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    assert_eq!(store.user_count().await.unwrap(), 1);
    // The trusted-header path mints accounts too, and they count the same.
    store
        .ensure_user("bob", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert_eq!(store.user_count().await.unwrap(), 2);
    store.remove_user("bob").await.unwrap();
    assert_eq!(store.user_count().await.unwrap(), 1);
}

/// The first admin is created only into an empty table, and the account it
/// creates is a real one: admin, enabled, folded name, display as typed,
/// and a password that verifies.
#[tokio::test]
async fn add_first_admin_creates_one_admin_and_only_on_an_empty_table() {
    let (store, _guard) = test_store().await;
    assert!(
        store.add_first_admin("Ada", "Ada", "s3cret").await.unwrap(),
        "an empty table is what the first-run path is for"
    );
    let users = store.list_users().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].name, "ada", "the name is folded like every other");
    assert_eq!(users[0].display, "Ada");
    assert_eq!(users[0].role, Role::Admin);
    assert!(!users[0].disabled);
    assert!(matches!(
        store.check_password("ada", "s3cret").await.unwrap(),
        PasswordCheck::Verified(_)
    ));

    assert!(
        !store.add_first_admin("bob", "Bob", "pw").await.unwrap(),
        "the slot is gone once any account exists"
    );
    assert_eq!(
        store.user_count().await.unwrap(),
        1,
        "and nothing was written"
    );
}

/// The account that closes the slot need not be an admin, and need not have
/// been created here: `crystalline users add` in another process is the
/// case that matters, and it lands an ordinary row.
#[tokio::test]
async fn add_first_admin_refuses_once_any_account_exists() {
    let (added, _guard) = test_store().await;
    added
        .add_user("vera", "Vera", None, Role::Viewer, "pw")
        .await
        .unwrap();
    assert!(
        !added
            .add_first_admin("root", "Root", "rootpw")
            .await
            .unwrap()
    );
    assert_eq!(added.list_users().await.unwrap().len(), 1);

    let (provisioned, _guard2) = test_store().await;
    provisioned
        .ensure_user("proxied", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert!(
        !provisioned
            .add_first_admin("root", "Root", "rootpw")
            .await
            .unwrap()
    );
}

/// An unusable name is refused before anything is written, so a typo does
/// not consume the one slot there is.
#[tokio::test]
async fn add_first_admin_refuses_a_name_the_store_cannot_key_on() {
    let (store, _guard) = test_store().await;
    assert!(store.add_first_admin("  ", "Blank", "pw").await.is_err());
    assert!(
        store
            .add_first_admin("ada lovelace", "Ada", "pw")
            .await
            .is_err()
    );
    assert_eq!(store.user_count().await.unwrap(), 0);
    assert!(store.add_first_admin("ada", "Ada", "pw").await.unwrap());
}

/// Invariant 1's real pin: the claim holds across PROCESSES, which is what
/// the REST layer's own guard mutex can never show.
///
/// Two [`AuthStore`]s are opened on one `web-auth.db` - the exact shape
/// `crystalline users add` takes while the daemon serves - and the first
/// admin is raced against a second first-admin call and against a plain
/// `add_user`. Two assertions carry the invariant, and neither is "exactly
/// one row lands": in the `add_user` leg two rows legitimately can, since
/// an ordinary add of a different name is not competing for the slot at
/// all. What must hold is that `add_first_admin` reports success at most
/// once, and never once any row already exists.
///
/// **The two racing calls are `tokio::spawn`ed behind a barrier, and the
/// race is repeated ten times.** All three details are load bearing and
/// none is style. A `tokio::join!` polls both futures on ONE task, so they
/// can only interleave where one of them returns `Pending` - and the
/// check-then-insert window this test exists to catch is store work that
/// never yields, so a `join!` version of this test passes against exactly
/// the naive implementation the plan rejects (measured against a
/// deliberately naive store: 12 runs, 12 misses). Spawned onto different
/// worker threads and released together, the same race caught that store in
/// roughly four rounds out of five, and ten rounds is what turns "roughly
/// four out of five" into a pin. Anyone tempted to tidy this back into a
/// `join!` is removing the only test that can see the bug.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_store_open_cannot_also_win_first_admin() {
    use std::sync::Arc;

    // Leg one: first admin against first admin, on two opens of one file.
    for round in 0..10 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        let daemon = Arc::new(AuthStore::open(&path).await.unwrap());
        let cli = Arc::new(AuthStore::open(&path).await.unwrap());
        // Released together, so what separates the two calls is the work
        // itself rather than however long each task waited to be scheduled.
        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let one = tokio::spawn({
            let store = daemon.clone();
            let gate = gate.clone();
            async move {
                gate.wait().await;
                store.add_first_admin("root", "Root", "rootpw").await
            }
        });
        let two = tokio::spawn({
            let store = cli.clone();
            let gate = gate.clone();
            async move {
                gate.wait().await;
                store.add_first_admin("boss", "Boss", "bosspw").await
            }
        });
        let won = [one.await.unwrap().unwrap(), two.await.unwrap().unwrap()];
        assert_eq!(
            won.iter().filter(|w| **w).count(),
            1,
            "round {round}: exactly one of two racing first-admin calls may win"
        );
        assert_eq!(
            daemon.user_count().await.unwrap(),
            1,
            "round {round}: and exactly one row is what they left behind"
        );
    }

    // Leg two: first admin against an ordinary add from the other open.
    // Both may land - the names differ and `users add` is not competing for
    // the slot - but a first admin created after a row exists would be a
    // check-then-insert that read stale, and a first admin that reported
    // failure while writing a row would be worse still.
    for round in 0..10 {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-auth.db");
        let daemon = Arc::new(AuthStore::open(&path).await.unwrap());
        let cli = Arc::new(AuthStore::open(&path).await.unwrap());
        let gate = Arc::new(tokio::sync::Barrier::new(2));
        let first = tokio::spawn({
            let store = daemon.clone();
            let gate = gate.clone();
            async move {
                gate.wait().await;
                store.add_first_admin("root", "Root", "rootpw").await
            }
        });
        let added = tokio::spawn({
            let store = cli.clone();
            let gate = gate.clone();
            async move {
                gate.wait().await;
                store.add_user("ada", "Ada", None, Role::Viewer, "pw").await
            }
        });
        added.await.unwrap().unwrap();
        // Both racers have finished before the table is read, or the read
        // could miss a row that was still on its way in.
        let first = first.await.unwrap().unwrap();
        let names: Vec<String> = daemon
            .list_users()
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.name)
            .collect();
        if first {
            assert!(names.contains(&"root".to_string()), "round {round}");
        } else {
            assert_eq!(
                names,
                vec!["ada".to_string()],
                "round {round}: a first admin that reported failure must not \
                 have written a row"
            );
        }
        assert!(
            !daemon.add_first_admin("late", "Late", "pw").await.unwrap(),
            "round {round}: and the slot stays shut for every later caller"
        );
    }
}

#[tokio::test]
async fn edits_to_an_unknown_user_error() {
    let (store, _guard) = test_store().await;
    assert!(store.set_password("ghost", "pw").await.is_err());
    assert!(store.set_role("ghost", Role::Admin).await.is_err());
    assert!(store.set_disabled("ghost", true).await.is_err());
    assert!(store.remove_user("ghost").await.is_err());
    assert!(store.create_session("ghost", 60).await.is_err());
}

#[tokio::test]
async fn remove_user_takes_its_sessions_with_it() {
    let (store, _guard) = test_store().await;
    // Editor rather than admin: the role is incidental, and the last
    // enabled admin cannot be removed (see [`NOT_LAST_ADMIN`]).
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    let s = store.create_session("ada", 3600).await.unwrap();
    store.remove_user("ada").await.unwrap();
    assert!(store.session_user(&s.token).await.unwrap().is_none());
    assert!(store.list_users().await.unwrap().is_empty());
}

#[tokio::test]
async fn list_users_is_sorted_and_reopens_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("web-auth.db");
    {
        let store = AuthStore::open(&path).await.unwrap();
        store
            .add_user("zoe", "Zoe", None, Role::Viewer, "pw")
            .await
            .unwrap();
        store
            .add_user("ada", "Ada", None, Role::Admin, "pw")
            .await
            .unwrap();
    }
    let reopened = AuthStore::open(&path).await.unwrap();
    let names: Vec<String> = reopened
        .list_users()
        .await
        .unwrap()
        .into_iter()
        .map(|u| u.name)
        .collect();
    assert_eq!(names, vec!["ada".to_string(), "zoe".to_string()]);
}

/// The lockout guard: the last enabled admin cannot be removed, disabled
/// or demoted, so an installation cannot lose its last way in. Enforced in
/// the store rather than in the `crystalline users` CLI because a
/// check-then-act in the CLI would race a second invocation.
#[tokio::test]
async fn the_last_admin_cannot_be_removed_disabled_or_demoted() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    // A non-admin account is no help: it cannot administer anything.
    store
        .add_user("bob", "Bob", None, Role::Editor, "pw")
        .await
        .unwrap();

    for err in [
        store.remove_user("ada").await.unwrap_err(),
        store.set_disabled("ada", true).await.unwrap_err(),
        store.set_role("ada", Role::Viewer).await.unwrap_err(),
    ] {
        assert!(
            err.to_string().contains("last admin"),
            "expected a last-admin refusal, got: {err}"
        );
    }

    // Nothing was applied: the account is still an enabled admin.
    let users = store.list_users().await.unwrap();
    let ada = users.iter().find(|u| u.name == "ada").unwrap();
    assert_eq!(ada.role, Role::Admin);
    assert!(!ada.disabled);
}

/// With a second enabled admin in place, all three operations go through.
#[tokio::test]
async fn an_admin_can_be_removed_disabled_and_demoted_beside_another_admin() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    store
        .add_user("bob", "Bob", None, Role::Admin, "pw")
        .await
        .unwrap();
    store
        .add_user("cyd", "Cyd", None, Role::Admin, "pw")
        .await
        .unwrap();
    store
        .add_user("dee", "Dee", None, Role::Admin, "pw")
        .await
        .unwrap();

    // Each edit leaves at least one enabled admin behind, so each is
    // allowed: four admins, then a disable, a demotion and a removal.
    store.set_disabled("ada", true).await.unwrap();
    store.set_role("bob", Role::Viewer).await.unwrap();
    store.remove_user("cyd").await.unwrap();

    // Re-enabling the disabled admin is never refused.
    store.set_disabled("ada", false).await.unwrap();
    let users = store.list_users().await.unwrap();
    assert_eq!(users.len(), 3);
    assert!(users.iter().any(|u| u.name == "ada" && !u.disabled));
}

/// A disabled admin cannot log in, so it is not a way back in and must not
/// satisfy the guard for the one admin that still works.
#[tokio::test]
async fn a_disabled_admin_does_not_count_as_a_remaining_admin() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    store
        .add_user("bob", "Bob", None, Role::Admin, "pw")
        .await
        .unwrap();
    store.set_disabled("bob", true).await.unwrap();

    assert!(store.remove_user("ada").await.is_err());
    assert!(store.set_disabled("ada", true).await.is_err());
    assert!(store.set_role("ada", Role::Editor).await.is_err());

    // The already-disabled admin is not the last *enabled* one, so it is
    // not itself protected.
    store.remove_user("bob").await.unwrap();
}

/// The guard only applies to enabled admins. Every other account, and
/// every promotion, is untouched by it.
#[tokio::test]
async fn the_guard_leaves_non_admins_and_promotions_alone() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    store
        .add_user("bob", "Bob", None, Role::Viewer, "pw")
        .await
        .unwrap();
    // The one and only viewer is not an admin: no protection.
    store.set_disabled("bob", true).await.unwrap();
    store.set_disabled("bob", false).await.unwrap();
    // Promoting is always allowed, even for the last admin itself.
    store.set_role("ada", Role::Admin).await.unwrap();
    store.set_role("bob", Role::Admin).await.unwrap();
    // Now that there are two, the first may go.
    store.remove_user("ada").await.unwrap();
}

/// A refused removal must not take the account's sessions with it: the
/// whole operation rolls back, not just the delete that was refused.
#[tokio::test]
async fn a_refused_removal_keeps_the_admins_sessions() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let live = store.create_session("ada", 3600).await.unwrap();
    assert!(store.remove_user("ada").await.is_err());
    assert!(
        store.session_user(&live.token).await.unwrap().is_some(),
        "the rolled-back removal must leave the live session in place"
    );
}

/// The guard must not swallow a plain typo: an unknown name still reports
/// itself as unknown rather than as a lockout refusal.
#[tokio::test]
async fn an_unknown_name_still_reports_itself_as_unknown() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    for err in [
        store.remove_user("ghost").await.unwrap_err(),
        store.set_disabled("ghost", true).await.unwrap_err(),
        store.set_role("ghost", Role::Viewer).await.unwrap_err(),
    ] {
        assert!(
            err.to_string().contains("no such user"),
            "expected an unknown-user error, got: {err}"
        );
    }
}

#[test]
fn roles_round_trip_through_text_and_json() {
    for role in [Role::Viewer, Role::Editor, Role::Admin] {
        assert_eq!(role.as_str().parse::<Role>().unwrap(), role);
        assert_eq!(
            serde_json::to_value(role).unwrap(),
            serde_json::Value::String(role.as_str().to_string())
        );
    }
    assert!("root".parse::<Role>().is_err());
    // A hand-edited or corrupt row resolves to the least privileged role.
    assert_eq!(role_from_db("root"), Role::Viewer);
}

/// The column migration: a database created by the slice-1 schema (no
/// last_seen_at) opens cleanly and gains the column, and opening twice is
/// harmless - the idempotent-open contract, extended to columns.
#[tokio::test]
async fn an_old_database_gains_the_last_seen_column_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    // Hand-create the pre-migration shape.
    {
        let db = Builder::new_local(&path.to_string_lossy())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                name TEXT PRIMARY KEY,
                display TEXT NOT NULL,
                email TEXT,
                role TEXT NOT NULL,
                pass_hash TEXT,
                disabled INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );
            INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
            VALUES ('ada', 'Ada', NULL, 'admin', NULL, 0, '2026-01-01T00:00:00Z');",
        )
        .await
        .unwrap();
    }
    let store = AuthStore::open(&path).await.unwrap();
    let users = store.list_users().await.unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].name, "ada");
    assert!(users[0].last_seen.is_none(), "never seen yet");
    drop(store);
    // Re-opening (the migrated shape) must not fail on the duplicate column.
    let again = AuthStore::open(&path).await.unwrap();
    assert_eq!(again.list_users().await.unwrap().len(), 1);
}

/// The two Reflection columns: a file written before them opens, reads
/// every account and membership as `default`, and keeps what is set.
#[tokio::test]
async fn the_reflection_levels_are_stored_and_an_older_file_opens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    {
        // The shape before 0.26.0: no `reflection` column anywhere.
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        // The per-domain grants table came later still (0.30.0).
        let grants = SCHEMA
            .find("CREATE TABLE IF NOT EXISTS domain_reflection_grant")
            .unwrap();
        let old = format!(
            "{}{}",
            &SCHEMA[..grants],
            SCHEMA[grants..].split_once(";\n").unwrap().1
        );
        let old = old.replace("    reflection TEXT,\n", "");
        assert!(!old.contains("reflection"), "both columns are left out");
        conn.execute_batch(&old).await.unwrap();
    }
    let auth = AuthStore::open(&path).await.unwrap();
    auth.add_user("ann", "Ann", None, Role::None, "pw12345678")
        .await
        .unwrap();
    auth.add_user("keeper", "Keeper", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    assert_eq!(
        auth.user("ann").await.unwrap().unwrap().reflection,
        ReflectionLevel::Default
    );
    assert_eq!(auth.user("ann").await.unwrap().unwrap().role, Role::None);
    // The grants table is made on open.
    auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "keeper")
        .await
        .unwrap();
    auth.set_reflection_level("ann", ReflectionLevel::Contributor)
        .await
        .unwrap();
    assert_eq!(
        auth.user("ann").await.unwrap().unwrap().reflection,
        ReflectionLevel::Contributor
    );
    auth.set_reflection_level("ann", ReflectionLevel::Default)
        .await
        .unwrap();
    // Created with a level: one write, read back as given.
    auth.add_user_with_reflection(
        "bea",
        "Bea",
        None,
        Role::None,
        ReflectionLevel::Reader,
        "pw12345678",
    )
    .await
    .unwrap();
    assert_eq!(
        auth.user("bea").await.unwrap().unwrap().reflection,
        ReflectionLevel::Reader
    );
    auth.add_token_only_user_with_reflection(
        "bot",
        "bot",
        None,
        Role::None,
        ReflectionLevel::Contributor,
        "bot",
    )
    .await
    .unwrap();
    assert_eq!(
        auth.user("bot").await.unwrap().unwrap().reflection,
        ReflectionLevel::Contributor
    );
    assert_eq!(
        auth.user("ann").await.unwrap().unwrap().reflection,
        ReflectionLevel::Default
    );

    auth.set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    let err = auth
        .upsert_domain_member("lab", "ann", MemberLevel::None, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::EmptyMembership),
        "{err:#}"
    );
    auth.upsert_domain_member(
        "lab",
        "ann",
        MemberLevel::None,
        Some(ReflectionLevel::Reader),
        "keeper",
    )
    .await
    .unwrap();
    let members = auth.domain_members("lab").await.unwrap();
    assert_eq!(
        (members[0].level, members[0].reflection),
        (MemberLevel::None, ReflectionLevel::Reader)
    );
    // Changing one axis never resets the other: no Reflection level given
    // keeps the stored one.
    auth.upsert_domain_member("lab", "ann", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    let members = auth.domain_members("lab").await.unwrap();
    assert_eq!(
        (members[0].level, members[0].reflection),
        (MemberLevel::Viewer, ReflectionLevel::Reader)
    );
    let err = auth
        .upsert_domain_member(
            "lab",
            "ann",
            MemberLevel::None,
            Some(ReflectionLevel::Default),
            "keeper",
        )
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::EmptyMembership),
        "{err:#}"
    );
    assert_eq!(
        auth.member_reflection("lab", "ann").await.unwrap(),
        ReflectionLevel::Reader
    );
    assert_eq!(
        auth.member_reflection("lab", "keeper").await.unwrap(),
        ReflectionLevel::Default
    );
    drop(auth);
    // Re-opening the migrated file does not fail on the duplicate columns.
    AuthStore::open(&path).await.unwrap();
}

fn entries(items: &[&str]) -> Vec<String> {
    items.iter().map(|e| e.to_string()).collect()
}

/// The table is created on open over a database an older build wrote,
/// and opening twice is harmless.
#[tokio::test]
async fn an_old_database_gains_the_write_list_table_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    {
        let db = Builder::new_local(&path.to_string_lossy())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        conn.execute_batch(
            "CREATE TABLE users (
                name TEXT PRIMARY KEY,
                display TEXT NOT NULL,
                email TEXT,
                role TEXT NOT NULL,
                pass_hash TEXT,
                disabled INTEGER NOT NULL DEFAULT 0,
                last_seen_at TEXT,
                created_at TEXT NOT NULL
            );
            INSERT INTO users (name, display, email, role, pass_hash, disabled, created_at)
            VALUES ('bot', 'bot', NULL, 'editor', NULL, 0, '2026-01-01T00:00:00Z');",
        )
        .await
        .unwrap();
    }
    let store = AuthStore::open(&path).await.unwrap();
    assert_eq!(store.write_list("bot").await.unwrap(), None);
    store
        .set_write_list("bot", &entries(&["agents-bot"]))
        .await
        .unwrap();
    drop(store);
    let again = AuthStore::open(&path).await.unwrap();
    assert_eq!(
        again.write_list("bot").await.unwrap(),
        Some(entries(&["agents-bot"]))
    );
}

#[tokio::test]
async fn a_write_list_is_set_replaced_read_and_cleared() {
    let (store, _guard) = test_store().await;
    store
        .add_user("bot", "Bot", None, Role::Editor, "pw")
        .await
        .unwrap();
    assert_eq!(store.write_list("bot").await.unwrap(), None);
    store
        .set_write_list(
            "BOT",
            &entries(&[" shared-* ", "agents-bot", "agents-bot", ""]),
        )
        .await
        .unwrap();
    assert_eq!(
        store.write_list("bot").await.unwrap(),
        Some(entries(&["agents-bot", "shared-*"])),
        "trimmed, deduplicated, empties dropped, sorted, keyed on the folded name"
    );
    store
        .set_write_list("bot", &entries(&["other"]))
        .await
        .unwrap();
    assert_eq!(
        store.write_list("bot").await.unwrap(),
        Some(entries(&["other"])),
        "a set replaces the whole list"
    );
    store.clear_write_list("bot").await.unwrap();
    assert_eq!(store.write_list("bot").await.unwrap(), None);
}

#[tokio::test]
async fn a_write_list_needs_an_account_and_an_entry() {
    let (store, _guard) = test_store().await;
    store
        .add_user("bot", "Bot", None, Role::Editor, "pw")
        .await
        .unwrap();
    let ghost = store
        .set_write_list("ghost", &entries(&["open"]))
        .await
        .unwrap_err();
    assert!(
        format!("{ghost:#}").contains("no such user: 'ghost'"),
        "{ghost:#}"
    );
    let ghost = store.clear_write_list("ghost").await.unwrap_err();
    assert!(
        format!("{ghost:#}").contains("no such user: 'ghost'"),
        "{ghost:#}"
    );
    let empty = store
        .set_write_list("bot", &entries(&["", "  "]))
        .await
        .unwrap_err();
    assert!(
        format!("{empty:#}").contains("at least one entry"),
        "{empty:#}"
    );
    assert_eq!(store.write_list("bot").await.unwrap(), None);
}

/// A rename rewrites exact entries and leaves `*` entries alone, on a
/// shared domain too (which has no acl, member or grant row), keeps an
/// entry that already named the new name, and a second run changes
/// nothing.
#[tokio::test]
async fn a_rename_follows_exact_write_entries_only() {
    let (store, _guard) = test_store().await;
    for name in ["bot", "both"] {
        store
            .add_user(name, name, None, Role::Editor, "pw")
            .await
            .unwrap();
    }
    store
        .set_write_list("bot", &entries(&["eng", "eng-*", "open"]))
        .await
        .unwrap();
    store
        .set_write_list("both", &entries(&["eng", "platform"]))
        .await
        .unwrap();
    store.rename_domain("eng", "platform").await.unwrap();
    assert_eq!(
        store.write_list("bot").await.unwrap(),
        Some(entries(&["eng-*", "open", "platform"]))
    );
    assert_eq!(
        store.write_list("both").await.unwrap(),
        Some(entries(&["platform"])),
        "an account that named both ends keeps one entry"
    );
    store.rename_domain("eng", "platform").await.unwrap();
    assert_eq!(
        store.write_list("bot").await.unwrap(),
        Some(entries(&["eng-*", "open", "platform"])),
        "idempotent"
    );
}

/// The list goes with the account, on both removal paths, so a name
/// added again starts with no list.
#[tokio::test]
async fn removing_an_account_removes_its_write_list() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    for name in ["bot", "bot2"] {
        store
            .add_user(name, name, None, Role::Editor, "pw")
            .await
            .unwrap();
        store
            .set_write_list(name, &entries(&["agents-bot"]))
            .await
            .unwrap();
    }
    store.remove_user("bot").await.unwrap();
    store.remove_user_force("bot2").await.unwrap();
    for name in ["bot", "bot2"] {
        store
            .add_user(name, name, None, Role::Editor, "pw")
            .await
            .unwrap();
        assert_eq!(store.write_list(name).await.unwrap(), None, "{name}");
    }
}

/// Resolving a session stamps the account as seen; a trusted-header contact
/// (ensure_user) stamps it too.
#[tokio::test]
async fn resolving_a_session_updates_last_seen() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw")
        .await
        .unwrap();
    assert!(store.list_users().await.unwrap()[0].last_seen.is_none());

    let s = store.create_session("ada", 3600).await.unwrap();
    store.session_user(&s.token).await.unwrap().unwrap();
    let seen = store.list_users().await.unwrap()[0].last_seen.clone();
    assert!(seen.is_some(), "a resolved session is a sighting");

    let provisioned = store
        .ensure_user("bob", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    assert!(
        provisioned.last_seen.is_some(),
        "provisioning is a sighting too"
    );
}

#[tokio::test]
async fn marking_a_sighting_moves_last_seen() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Viewer, "pw")
        .await
        .unwrap();
    let before = store.user("ada").await.unwrap().unwrap().last_seen;
    assert!(before.is_none(), "an account nobody has used yet");
    store.mark_seen("ada").await.unwrap();
    let after = store.user("ada").await.unwrap().unwrap().last_seen;
    assert!(after.is_some(), "and one that just arrived");
    // A name nobody holds is not an error: the caller stamps a row it read,
    // and a row removed in between is somebody else's problem to report.
    store.mark_seen("nobody").await.unwrap();
}

/// The operator escape hatch: --force bypasses the last-admin guard. It still
/// reports a missing account, and a forced removal still takes the sessions
/// with it in the same transaction.
#[tokio::test]
async fn forced_edits_bypass_the_last_admin_guard() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let live = store.create_session("ada", 3600).await.unwrap();

    // The guarded paths refuse; the forced ones do not.
    assert!(store.set_role("ada", Role::Viewer).await.is_err());
    store.set_role_force("ada", Role::Viewer).await.unwrap();
    assert_eq!(store.list_users().await.unwrap()[0].role, Role::Viewer);

    store.set_role_force("ada", Role::Admin).await.unwrap();
    assert!(store.remove_user("ada").await.is_err());
    store.remove_user_force("ada").await.unwrap();
    assert!(store.list_users().await.unwrap().is_empty());
    assert!(
        store.session_user(&live.token).await.unwrap().is_none(),
        "a forced removal still revokes the sessions"
    );

    assert!(store.set_role_force("ghost", Role::Viewer).await.is_err());
    assert!(store.remove_user_force("ghost").await.is_err());
}

#[tokio::test]
async fn mcp_token_round_trip_and_revocation() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "laptop-agent").await.unwrap();
    assert!(issued.token.starts_with("cmt_"));
    assert_eq!(issued.token.len(), 4 + 64);
    let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
    assert_eq!(user.name, "ada");
    let listed = store.list_mcp_tokens("ada").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(listed[0].last_used.is_some());

    // The store never keeps the plaintext - proved, not just commented.
    // Read the raw column via the store's own connection (tests are a
    // descendant module, so the private field and helper are reachable)
    // and check it against an independently computed sha256, the same
    // property `token_hash` is supposed to have.
    let row = store
        .db
        .query_first(
            "SELECT token_hash FROM mcp_tokens WHERE id = ?1",
            vec![Value::Integer(issued.id)],
        )
        .await
        .unwrap()
        .unwrap();
    let stored_hash = cell_text(&row, 0).unwrap();
    assert_eq!(stored_hash.len(), 64, "a sha256 hex digest is 64 chars");
    assert!(
        stored_hash.chars().all(|c| c.is_ascii_hexdigit()),
        "the stored value must be hex, not the token itself"
    );
    assert_ne!(stored_hash, issued.token);
    assert!(!stored_hash.contains(issued.token.as_str()));
    assert!(!issued.token.contains(stored_hash.as_str()));
    let mut hasher = Sha256::new();
    hasher.update(issued.token.as_bytes());
    let expected = crystalline_index::hex_lower(&hasher.finalize());
    assert_eq!(
        stored_hash, expected,
        "the column holds sha256 of the whole token, prefix included"
    );

    assert!(store.revoke_mcp_token("ada", issued.id).await.unwrap());
    assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());
}

#[tokio::test]
async fn a_token_only_account_has_no_password_and_one_token() {
    let (store, _guard) = test_store().await;
    let issued = store
        .add_token_only_user("Agent-Build", "Agent-Build", None, Role::Editor, "build")
        .await
        .unwrap();
    assert!(issued.token.starts_with("cmt_"));
    assert_eq!(issued.token.len(), 4 + 64);
    assert_eq!(issued.label, "build");

    for attempt in ["", "pw12345678", "agent-build"] {
        assert!(
            store
                .verify_password("agent-build", attempt)
                .await
                .unwrap()
                .is_none(),
            "password sign-in never succeeds for a token-only account"
        );
    }
    assert!(matches!(
        store.check_password("agent-build", "x").await.unwrap(),
        PasswordCheck::NoHash
    ));

    let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
    assert_eq!(user.name, "agent-build");
    assert_eq!(user.role, Role::Editor);
    assert_eq!(store.list_mcp_tokens("agent-build").await.unwrap().len(), 1);
    assert!(
        store
            .passwordless_users()
            .await
            .unwrap()
            .contains("agent-build")
    );
    let debug = format!("{issued:?}");
    assert!(!debug.contains(&issued.token), "{debug}");
}

#[tokio::test]
async fn a_token_only_account_with_a_taken_name_is_refused_and_issues_nothing() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    let err = store
        .add_token_only_user("ADA", "Ada", None, Role::Editor, "build")
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}")
            .to_lowercase()
            .contains("unique constraint"),
        "{err:#}"
    );
    assert!(store.list_mcp_tokens("ada").await.unwrap().is_empty());
    let ada = store.user("ada").await.unwrap().unwrap();
    assert_eq!(ada.role, Role::Viewer, "the existing account is untouched");
    assert!(
        store
            .verify_password("ada", "pw12345678")
            .await
            .unwrap()
            .is_some()
    );
}

#[tokio::test]
async fn a_failed_token_issue_leaves_no_account() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let taken = store.issue_mcp_token("ada", "laptop").await.unwrap();
    // The token hash is UNIQUE, so reusing a live token makes the second
    // insert fail after the account insert succeeded.
    let err = store
        .add_token_only_user_with(
            "agent-build",
            "agent-build",
            None,
            Role::Editor,
            ReflectionLevel::Default,
            "build",
            taken.token.clone(),
        )
        .await
        .unwrap_err();
    assert!(
        format!("{err:#}")
            .to_lowercase()
            .contains("unique constraint"),
        "{err:#}"
    );
    assert!(
        store.user("agent-build").await.unwrap().is_none(),
        "the account was rolled back with the token"
    );
    assert_eq!(store.list_mcp_tokens("ada").await.unwrap().len(), 1);
    // The name is free again.
    store
        .add_token_only_user("agent-build", "agent-build", None, Role::Editor, "build")
        .await
        .unwrap();
}

#[tokio::test]
async fn mcp_token_of_a_disabled_account_stops_resolving() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "t").await.unwrap();
    store.set_disabled("ada", true).await.unwrap();
    assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());

    // Disabled without deletion: the row must still be there, and
    // re-enabling must hand the very same token back rather than force a
    // re-issue.
    let listed = store.list_mcp_tokens("ada").await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "disabling an account must not delete its mcp tokens"
    );
    store.set_disabled("ada", false).await.unwrap();
    let user = store.mcp_token_user(&issued.token).await.unwrap().unwrap();
    assert_eq!(user.name, "ada");
}

#[tokio::test]
async fn an_orphaned_mcp_token_row_cannot_resolve_and_gets_pruned() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "agent").await.unwrap();

    // Simulate the stranded-row scenario `issue_mcp_token`'s own
    // transaction now prevents live: an account vanishing by a path other
    // than `remove_user`'s sweep, leaving an `mcp_tokens` row with no
    // matching account (the table carries no foreign key). A row written
    // before this defense existed, or reached some other way, must still
    // fail closed rather than resolve for whoever next claims the name.
    store
        .db
        .execute("DELETE FROM users WHERE name = 'ada'", vec![])
        .await
        .unwrap();

    assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());
    // Turso prunes on the lookup itself. Postgres prunes at open and then
    // hourly, and sweeps a name's orphans when an account takes the name.
    if store.db.is_postgres() {
        store
            .add_user("ada", "Ada The Second", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }

    let remaining = store
        .db
        .query_first("SELECT COUNT(*) FROM mcp_tokens", vec![])
        .await
        .unwrap()
        .unwrap();
    let count = match remaining.get_value(0) {
        Ok(Value::Integer(n)) => n,
        other => panic!("unexpected COUNT(*) result: {other:?}"),
    };
    assert_eq!(
        count, 0,
        "mcp_token_user must prune the orphaned row, not just refuse it"
    );
}

#[tokio::test]
async fn mcp_token_rotation_replaces_in_one_step() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let first = store.issue_mcp_token("ada", "agent").await.unwrap();
    let second = store.rotate_mcp_token("ada", first.id).await.unwrap();
    assert_eq!(second.label, "agent");
    assert!(store.mcp_token_user(&first.token).await.unwrap().is_none());
    assert!(store.mcp_token_user(&second.token).await.unwrap().is_some());
}

#[tokio::test]
async fn removing_a_user_revokes_its_mcp_tokens() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "agent").await.unwrap();
    store.remove_user("ada").await.unwrap();
    assert!(store.mcp_token_user(&issued.token).await.unwrap().is_none());
    assert!(store.list_mcp_tokens("ada").await.unwrap().is_empty());
}

/// A name that is not an account reads as `None`, and a folded spelling of
/// one that is finds it: the difference between "no such user" and "an
/// account with no tokens" rests on this.
#[tokio::test]
async fn one_account_is_read_back_by_any_spelling_of_its_name() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    for spelling in ["ada", "ADA", "  Ada  "] {
        let found = store.user(spelling).await.unwrap();
        assert_eq!(found.expect("'{spelling}' is ada").name, "ada");
    }
    assert!(store.user("ghost").await.unwrap().is_none());
}

/// The one unhashed copy of a live credential must never be one
/// `tracing::debug!` or one failed assertion away from a log file, while
/// the id and the label - the parts that make such a line useful - still
/// print. Asserted on the random half rather than on the whole token: the
/// redaction keeps the [`MCP_TOKEN_PREFIX`], so `!contains(&issued.token)`
/// would pass even if the secret leaked in pieces.
#[tokio::test]
async fn an_issued_token_never_prints_its_secret() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let issued = store.issue_mcp_token("ada", "laptop").await.unwrap();
    let secret = issued
        .token
        .strip_prefix(MCP_TOKEN_PREFIX)
        .expect("a token carries the prefix");
    let text = format!("{issued:?}");
    assert!(!text.contains(secret), "the secret is redacted: {text}");
    assert!(text.contains("redacted"), "and says so: {text}");
    assert!(
        text.contains("laptop") && text.contains(&issued.id.to_string()),
        "while the id and label still print: {text}"
    );
}

/// The membership cast every test below shares: two accounts that own or
/// join things and one that never does.
async fn members_cast(store: &AuthStore) {
    for (name, role) in [
        ("keeper", Role::Editor),
        ("mem", Role::Viewer),
        ("out", Role::Editor),
    ] {
        store
            .add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_domain_is_shared_until_somebody_makes_it_private() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    assert!(store.domain_visibility("lab").await.unwrap().is_none());
    assert!(store.private_domains().await.unwrap().is_empty());
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    let acl = store.domain_visibility("lab").await.unwrap().unwrap();
    assert_eq!(acl.domain, "lab");
    assert_eq!(acl.owner, "keeper");
    assert_eq!(
        store.private_domains().await.unwrap(),
        vec![DomainAcl {
            domain: "lab".into(),
            owner: "keeper".into()
        }]
    );
    // The name is trimmed but never folded: the engine keys its domain map
    // on the literal name.
    assert!(store.domain_visibility("  lab  ").await.unwrap().is_some());
    assert!(store.domain_visibility("Lab").await.unwrap().is_none());
}

#[tokio::test]
async fn a_domain_acl_needs_a_live_owner() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    let err = store
        .set_domain_visibility("lab", true, "ghost")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no such user"), "{err}");
    store.set_disabled("mem", true).await.unwrap();
    let err = store
        .set_domain_visibility("lab", true, "mem")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("disabled"), "{err}");
    assert!(
        store.domain_visibility("lab").await.unwrap().is_none(),
        "a refused call writes nothing"
    );
}

#[tokio::test]
async fn making_a_domain_shared_again_takes_its_members_with_it() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    assert_eq!(store.domain_members("lab").await.unwrap().len(), 1);
    store
        .set_domain_visibility("lab", false, "keeper")
        .await
        .unwrap();
    assert!(store.domain_visibility("lab").await.unwrap().is_none());
    assert!(
        store.domain_members("lab").await.unwrap().is_empty(),
        "the members go with the acl, so making it private again does not \
         restore somebody else's invitations"
    );
    assert!(store.memberships_of("mem").await.unwrap().is_empty());
    // Asking for a state that already holds is not an error.
    store
        .set_domain_visibility("lab", false, "keeper")
        .await
        .unwrap();
}

/// A domain rename carries every record the accounts database keeps under
/// the old name: the acl row with its owner, the membership rows and the
/// draft share-links. A second call finds nothing under the old name and is
/// fine, which is what a rename completed again after a crash does.
#[tokio::test]
async fn renaming_a_domain_moves_its_acl_members_and_grants() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("eng", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("eng", "out", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    let grant = store
        .mint_overlay_grant("eng", "plan.md", "keeper", None)
        .await
        .unwrap();

    store.rename_domain("eng", "platform").await.unwrap();

    assert!(store.domain_visibility("eng").await.unwrap().is_none());
    assert!(store.domain_members("eng").await.unwrap().is_empty());
    assert_eq!(
        store
            .domain_visibility("platform")
            .await
            .unwrap()
            .map(|acl| acl.owner),
        Some("keeper".to_string())
    );
    let mut members: Vec<(String, MemberLevel)> = store
        .domain_members("platform")
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.principal, m.level))
        .collect();
    members.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        members,
        vec![
            ("mem".to_string(), MemberLevel::Viewer),
            ("out".to_string(), MemberLevel::Editor),
        ]
    );
    assert_eq!(
        store.overlay_grant_domain(&grant.token).await.unwrap(),
        Some("platform".to_string())
    );
    assert!(
        store
            .overlay_grants_of("keeper", "eng", "plan.md")
            .await
            .unwrap()
            .is_empty()
    );

    store
        .rename_domain("eng", "platform")
        .await
        .expect("a second call finds nothing under the old name");
    assert_eq!(store.domain_members("platform").await.unwrap().len(), 2);
    store
        .rename_domain("never", "seen")
        .await
        .expect("a domain with no records has nothing to move");
}

/// The renamed domain's records win over stale ones already under the new
/// name: the stale acl owner, every stale member (colliding or not) and
/// every stale share-link go, so none of them gains access to the renamed
/// domain. A second call leaves the moved rows alone.
#[tokio::test]
async fn renaming_onto_stale_records_keeps_the_renamed_domains_rows() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("eng", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    let live = store
        .mint_overlay_grant("eng", "plan.md", "keeper", None)
        .await
        .unwrap();
    // Stale records under the new name, from a removal that never forgot
    // them: another owner, a colliding and a non-colliding member, and a
    // share-link.
    store
        .set_domain_visibility("platform", true, "out")
        .await
        .unwrap();
    store
        .upsert_domain_member("platform", "mem", MemberLevel::Viewer, None, "out")
        .await
        .unwrap();
    store
        .upsert_domain_member("platform", "keeper", MemberLevel::Manager, None, "out")
        .await
        .unwrap();
    let stale = store
        .mint_overlay_grant("platform", "old.md", "out", None)
        .await
        .unwrap();

    store.rename_domain("eng", "platform").await.unwrap();
    assert!(store.domain_visibility("eng").await.unwrap().is_none());
    assert!(store.domain_members("eng").await.unwrap().is_empty());
    renamed_records_won(&store, &live.token, &stale.token).await;

    store
        .rename_domain("eng", "platform")
        .await
        .expect("a second call finds nothing under the old name");
    renamed_records_won(&store, &live.token, &stale.token).await;
}

/// What `renaming_onto_stale_records_keeps_the_renamed_domains_rows`
/// expects under `platform` after each call.
async fn renamed_records_won(store: &AuthStore, live: &str, stale: &str) {
    assert_eq!(
        store
            .domain_visibility("platform")
            .await
            .unwrap()
            .map(|acl| acl.owner),
        Some("keeper".to_string()),
        "the renamed domain's owner, not the stale one"
    );
    let members: Vec<(String, MemberLevel)> = store
        .domain_members("platform")
        .await
        .unwrap()
        .into_iter()
        .map(|m| (m.principal, m.level))
        .collect();
    assert_eq!(
        members,
        vec![("mem".to_string(), MemberLevel::Editor)],
        "only the renamed domain's members, at their levels"
    );
    assert_eq!(
        store.overlay_grant_domain(live).await.unwrap(),
        Some("platform".to_string())
    );
    assert_eq!(
        store.overlay_grant_domain(stale).await.unwrap(),
        None,
        "the stale share-link is gone"
    );
}

/// A rename that changes only the case is a real rename: names are stored
/// as registered and compare byte for byte.
#[tokio::test]
async fn a_case_only_domain_rename_moves_the_records() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("Eng", true, "keeper")
        .await
        .unwrap();
    store.rename_domain("Eng", "eng").await.unwrap();
    assert!(store.domain_visibility("Eng").await.unwrap().is_none());
    assert!(store.domain_visibility("eng").await.unwrap().is_some());
}

/// Privatizing a domain that is already private writes nothing: not the
/// owner, not the membership rows. It used to hand the domain to whoever
/// asked last, which made a retried request an ownership transfer nobody
/// asked for and dropped the previous owner - who holds no membership row
/// by construction - to no access at all.
#[tokio::test]
async fn privatizing_an_already_private_domain_writes_nothing() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();

    let again = store
        .set_domain_visibility("lab", true, "out")
        .await
        .unwrap();
    assert_eq!(
        again,
        VisibilityWrite::AlreadyPrivate {
            owner: "keeper".to_string()
        },
        "the answer says it was already private, and names the owner it kept"
    );
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        "keeper",
        "a visibility statement is not an ownership transfer"
    );
    assert_eq!(
        store.domain_members("lab").await.unwrap().len(),
        1,
        "and nobody was evicted"
    );

    // The verb that DOES change an owner still does, and still keeps the
    // members: the two paths that change an owner must agree, and one of
    // them is no longer this one.
    store.transfer_domain("lab", "out").await.unwrap();
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        "out"
    );
    assert_eq!(
        store.domain_members("lab").await.unwrap().len(),
        1,
        "an owner change is not an eviction"
    );
}

// `promoting_a_member_to_owner_drops_the_row_that_now_says_less` used to sit
// here, promoting a member by privatizing an already-private domain with a
// new owner. That is not a thing this method does any more - privatizing
// one that is already private writes nothing at all - and the step it was
// really about is `transfer_domain`'s, which the test below pins on the one
// path that still takes it.

#[tokio::test]
async fn transfer_domain_swaps_the_owner_and_drops_its_member_row() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "out", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    store.transfer_domain("lab", "mem").await.unwrap();
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        "mem"
    );
    let members = store.domain_members("lab").await.unwrap();
    assert_eq!(
        members
            .iter()
            .map(|m| m.principal.as_str())
            .collect::<Vec<_>>(),
        vec!["out"],
        "the new owner's membership row is gone: it could only say less"
    );
    assert!(store.memberships_of("mem").await.unwrap().is_empty());
}

#[tokio::test]
async fn transfer_domain_refuses_a_shared_domain_and_a_dead_owner() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    let err = store
        .transfer_domain("lab", "mem")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not private"), "{err}");
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    let err = store
        .transfer_domain("lab", "ghost")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no such user"), "{err}");
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        "keeper",
        "a refused transfer leaves the owner alone"
    );
}

#[tokio::test]
async fn membership_is_upserted_and_removed_by_name() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "MEM", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
        .await
        .unwrap();
    let members = store.domain_members("lab").await.unwrap();
    assert_eq!(
        members.len(),
        1,
        "an upsert replaces rather than duplicates"
    );
    assert_eq!(members[0].principal, "mem", "the name is folded");
    assert_eq!(members[0].level, MemberLevel::Manager);
    assert_eq!(members[0].added_by, "keeper");
    assert!(!members[0].added_at.is_empty());
    assert_eq!(
        store.memberships_of("Mem").await.unwrap(),
        vec![("lab".to_string(), MemberLevel::Manager)]
    );
    assert!(store.remove_domain_member("lab", "mem").await.unwrap());
    assert!(
        !store.remove_domain_member("lab", "mem").await.unwrap(),
        "removing what is not there reports itself rather than erroring"
    );
    assert!(store.domain_members("lab").await.unwrap().is_empty());
}

#[tokio::test]
async fn membership_refuses_a_shared_domain_a_stranger_and_the_owner() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    let err = store
        .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("not private"), "{err}");
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    let err = store
        .upsert_domain_member("lab", "ghost", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("no such user"), "{err}");
    let err = store
        .upsert_domain_member("lab", "keeper", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("already holds every level"), "{err}");
    store.set_disabled("mem", true).await.unwrap();
    let err = store
        .upsert_domain_member("lab", "mem", MemberLevel::Viewer, None, "keeper")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("disabled"), "{err}");
    let err = store
        .upsert_domain_member("lab", "out", MemberLevel::Viewer, None, "  ")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("an actor to record it as"), "{err}");
    assert!(store.domain_members("lab").await.unwrap().is_empty());
}

#[tokio::test]
async fn removing_a_user_takes_its_memberships_with_it() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .add_user("boss", "boss", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "out", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    store.remove_user("mem").await.unwrap();
    store.remove_user_force("out").await.unwrap();
    assert!(
        store.domain_members("lab").await.unwrap().is_empty(),
        "a re-added name must not inherit the old holder's access"
    );
}

#[tokio::test]
async fn removing_a_user_leaves_the_domains_it_owned_owned_by_nobody() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    store.remove_user("keeper").await.unwrap();
    let acl = store
        .domain_visibility("lab")
        .await
        .unwrap()
        .expect("the domain stays private when its owner goes");
    assert_eq!(
        acl.owner, "",
        "owned by nobody: a name no live account can ever hold"
    );
    assert_eq!(
        store.domain_members("lab").await.unwrap().len(),
        1,
        "and the people invited into it keep their levels"
    );
    // Re-adding the freed name mints a different person. The resolver half
    // of this is `a_re_added_owner_name_does_not_inherit_the_domain` in
    // `crate::scope`; here the record itself must not name them.
    store
        .add_user("keeper", "keeper", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        ""
    );
    assert!(store.memberships_of("keeper").await.unwrap().is_empty());
}

/// The forced removal takes the same step. `remove_user_force` runs the
/// same four cleanups the guarded remove does, and this is the one of them
/// that widens rather than narrows if it is ever dropped: a private domain
/// left naming a departed account would hand it to whoever next signs up
/// under that login name. The guarded path is pinned above; a `--force`
/// removal is exactly the path an operator reaches for when the account
/// being removed is the last admin, so it must not be the one that leaks.
#[tokio::test]
async fn force_removing_a_user_also_leaves_its_domains_owned_by_nobody() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;
    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap();
    store.remove_user_force("keeper").await.unwrap();
    let acl = store
        .domain_visibility("lab")
        .await
        .unwrap()
        .expect("the domain stays private when its owner is forced out");
    assert_eq!(
        acl.owner, "",
        "owned by nobody, exactly as the guarded removal leaves it"
    );
    assert_eq!(
        store.domain_members("lab").await.unwrap().len(),
        1,
        "and the people invited into it keep their levels"
    );
}

/// Every membership refusal carries its kind as a TYPE, and its own
/// sentence as the message.
///
/// The pin that lets `rest::members` classify by value: matching prose
/// meant a rewording here silently turned a 409 into a 500. Both halves
/// are asserted - the kind, which the surface decides on, and the
/// message, which the operator-facing surfaces still print - so neither
/// can be dropped in favour of the other.
#[tokio::test]
async fn every_membership_refusal_carries_its_kind_and_its_words() {
    let (store, _guard) = test_store().await;
    members_cast(&store).await;

    let shared = store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&shared),
        Some(RefusalKind::NotPrivate)
    );
    assert!(
        format!("{shared:#}").contains("is not private"),
        "{shared:#}"
    );

    let no_owner = store.transfer_domain("lab", "mem").await.unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&no_owner),
        Some(RefusalKind::NotPrivate)
    );

    store
        .set_domain_visibility("lab", true, "keeper")
        .await
        .unwrap();
    let owner = store
        .upsert_domain_member("lab", "keeper", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&owner),
        Some(RefusalKind::OwnerIsNotAMember)
    );
    assert!(format!("{owner:#}").contains("owns domain"), "{owner:#}");

    let ghost = store
        .upsert_domain_member("lab", "ghost", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&ghost),
        Some(RefusalKind::NoSuchAccount)
    );

    // A disabled account is the SAME kind as one that does not exist. The
    // two are told apart only in the message, which is what lets a surface
    // that must not distinguish them answer with one word-for-word reply
    // and be sure it has no second branch.
    store.set_disabled("mem", true).await.unwrap();
    let disabled = store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&disabled),
        Some(RefusalKind::NoSuchAccount),
        "an existing-but-disabled account is not a kind of its own"
    );
    assert!(
        format!("{disabled:#}").contains("is disabled"),
        "{disabled:#}"
    );

    // A principal that cannot be a login name at all carries its own kind,
    // which is what lets the one route taking a principal in the BODY
    // answer the 422 its documentation promises rather than a 500.
    let malformed = store
        .upsert_domain_member("lab", "  ", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&malformed),
        Some(RefusalKind::InvalidName)
    );

    // A domain with no acl row is shared as far as this table knows, so it
    // refuses as NotPrivate rather than as a fault. Named for what it
    // asserts: this leg is not the no-kind one.
    let unregistered = store
        .upsert_domain_member("nosuchdomain", "mem", MemberLevel::Editor, None, "keeper")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&unregistered),
        Some(RefusalKind::NotPrivate),
        "a domain with no acl row is shared as far as this table knows"
    );

    // And a failure that is nobody's doing carries no kind at all, so the
    // surfaces keep answering 500 for what is genuinely theirs. This is the
    // arm `rest::members::store_error`'s `None` branch answers, and without
    // a leg here nothing in the workspace pins it.
    let no_actor = store
        .upsert_domain_member("lab", "mem", MemberLevel::Editor, None, "  ")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&no_actor),
        None,
        "an empty actor is this server's own bug, not a refusal with a kind"
    );
    assert!(
        format!("{no_actor:#}").contains("needs an actor"),
        "{no_actor:#}"
    );
}

#[tokio::test]
async fn the_membership_tables_survive_a_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    {
        let store = AuthStore::open(&path).await.unwrap();
        members_cast(&store).await;
        store
            .set_domain_visibility("lab", true, "keeper")
            .await
            .unwrap();
        store
            .upsert_domain_member("lab", "mem", MemberLevel::Manager, None, "keeper")
            .await
            .unwrap();
    }
    // The schema is idempotent DDL, so opening the same file again applies
    // it a second time and must change nothing.
    let store = AuthStore::open(&path).await.unwrap();
    assert_eq!(
        store.domain_visibility("lab").await.unwrap().unwrap().owner,
        "keeper"
    );
    assert_eq!(
        store.memberships_of("mem").await.unwrap(),
        vec![("lab".to_string(), MemberLevel::Manager)]
    );
}

#[test]
fn member_levels_round_trip_through_text_and_json() {
    for level in [
        MemberLevel::Viewer,
        MemberLevel::Editor,
        MemberLevel::Manager,
    ] {
        assert_eq!(level.to_string().parse::<MemberLevel>().unwrap(), level);
        assert_eq!(
            serde_json::to_string(&level).unwrap(),
            format!("\"{}\"", level.as_str())
        );
    }
    assert_eq!(
        " Manager ".parse::<MemberLevel>().unwrap(),
        MemberLevel::Manager
    );
    assert!("keeper".parse::<MemberLevel>().is_err());
    assert_eq!(
        member_level_from_db("nonsense"),
        MemberLevel::Viewer,
        "an unreadable row never fails open"
    );
}

#[test]
fn a_domain_name_is_trimmed_and_never_empty() {
    assert_eq!(normalize_domain("  lab ").unwrap(), "lab");
    assert_eq!(normalize_domain("My Domain").unwrap(), "My Domain");
    assert!(normalize_domain("   ").is_err());
}

// --- identity links ---------------------------------------------------

/// An account provisioned from an identity is found back by that
/// identity, and by nothing else.
#[tokio::test]
async fn a_provisioned_identity_is_found_back_by_its_pair() {
    let (store, _guard) = test_store().await;
    let user = store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "ada",
            Some("Ada Lovelace"),
            Some("ada@example.test"),
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert_eq!(user.name, "ada");
    assert_eq!(user.display, "Ada Lovelace");
    assert_eq!(user.email.as_deref(), Some("ada@example.test"));
    assert_eq!(user.role, Role::Viewer);

    let found = store
        .linked_user("https://idp.example", "sub-1")
        .await
        .unwrap()
        .expect("the pair names the account");
    assert_eq!(found.name, "ada");
    assert!(
        store
            .linked_user("https://idp.example", "sub-2")
            .await
            .unwrap()
            .is_none(),
        "another subject at the same issuer is another person"
    );
    assert!(
        store
            .linked_user("https://other.example", "sub-1")
            .await
            .unwrap()
            .is_none(),
        "the same subject at another issuer is another person"
    );
    let links = store.identity_links("ada").await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].issuer, "https://idp.example");
    assert_eq!(links[0].subject, "sub-1");
    assert_eq!(links[0].linked_by, "jit");
    assert!(!links[0].linked_at.is_empty());
}

/// A link whose account is gone is swept the moment it is looked up,
/// rather than left dormant for the next account to take the name.
///
/// The direct analogue of
/// `an_orphaned_session_row_is_swept_rather_than_inherited`, and for the
/// same reason: an identity link is a credential, and a dormant one signs
/// a stranger into whoever next holds that login name.
#[tokio::test]
async fn an_orphaned_identity_link_is_swept_rather_than_inherited() {
    let (store, _guard) = test_store().await;
    store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "ada",
            Some("Ada"),
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    // Delete only the account row. Both removal paths are transactional
    // and sweep the links themselves, so this is the state a differently
    // built binary writing the same file, a hand edit, or a future
    // deletion path that forgets the sweep would leave behind.
    store
        .db
        .execute(
            "DELETE FROM users WHERE name = ?1",
            vec![Value::Text("ada".to_string())],
        )
        .await
        .unwrap();
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_none()
    );
    store
        .add_user("ada", "Ada The Second", None, Role::Admin, "pw")
        .await
        .unwrap();
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_none(),
        "the orphan must have been swept, not left dormant"
    );
    assert!(
        store.identity_links("ada").await.unwrap().is_empty(),
        "and the account that took the name inherits nothing"
    );
}

/// The engine enforces both constraints, not only `insert_link`'s checks:
/// a raw duplicate pair and a second identity for one account at one
/// issuer are both refused by the database itself.
#[tokio::test]
async fn the_database_itself_refuses_a_duplicate_pair_and_a_second_identity() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "pw")
        .await
        .unwrap();
    let insert = |subject: &str, user: &str| {
        store.db.execute(
            "INSERT INTO identity_link (issuer, subject, \"user\", linked_at, linked_by)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            vec![
                Value::Text("https://idp.example".to_string()),
                Value::Text(subject.to_string()),
                Value::Text(user.to_string()),
                Value::Text("2026-09-07T00:00:00Z".to_string()),
                Value::Text("test".to_string()),
            ],
        )
    };
    insert("sub-1", "ada").await.unwrap();
    assert!(
        insert("sub-1", "ada").await.is_err(),
        "the primary key on (issuer, subject) is the engine's, not ours"
    );
    assert!(
        insert("sub-2", "ada").await.is_err(),
        "and one account holds one identity per issuer, by unique index"
    );
}

/// A name already taken is uniquified rather than joined, and the
/// suffixes keep counting.
#[tokio::test]
async fn provisioning_uniquifies_a_taken_name() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    let second = store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "Ada",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert_eq!(second.name, "ada-2");
    let third = store
        .provision_linked_user(
            "https://idp.example",
            "sub-2",
            "ada",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert_eq!(third.name, "ada-3");
    assert_eq!(
        store.user("ada").await.unwrap().unwrap().role,
        Role::Admin,
        "the account that held the name is untouched"
    );
}

/// Provisioning honours the account cap, and refuses in words that name
/// the setting.
#[tokio::test]
async fn provisioning_refuses_past_the_account_cap() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    let err = store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "grace",
            None,
            None,
            Role::Viewer,
            1,
        )
        .await
        .unwrap_err();
    let message = format!("{err:#}");
    assert!(message.contains("auth.max_users"), "{message}");
    assert!(store.user("grace").await.unwrap().is_none());
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_none(),
        "a refused provisioning leaves no link behind"
    );
}

/// A pair belongs to one account, and an account holds one identity per
/// issuer. Both refusals name what is in the way.
#[tokio::test]
async fn a_pair_links_once_and_an_account_holds_one_identity_per_issuer() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .add_user("grace", "Grace", None, Role::Editor, "correct horse")
        .await
        .unwrap();
    store
        .link_identity("https://idp.example", "sub-1", "ada", "ada")
        .await
        .unwrap();

    let err = store
        .link_identity("https://idp.example", "sub-1", "grace", "grace")
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("already linked"), "{err:#}");

    let err = store
        .link_identity("https://idp.example", "sub-2", "ada", "ada")
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("already holds"), "{err:#}");

    // A second issuer is a second identity, and that is allowed.
    store
        .link_identity("https://other.example", "sub-2", "ada", "ada")
        .await
        .unwrap();
    assert_eq!(store.identity_links("ada").await.unwrap().len(), 2);
    assert!(store.identity_links("grace").await.unwrap().is_empty());
}

/// Linking refuses a name that is nobody, so a link can never point at an
/// account that does not exist.
#[tokio::test]
async fn an_identity_cannot_be_linked_to_a_name_that_is_nobody() {
    let (store, _guard) = test_store().await;
    let err = store
        .link_identity("https://idp.example", "sub-1", "ghost", "admin")
        .await
        .unwrap_err();
    assert!(format!("{err:#}").contains("no such user"), "{err:#}");
}

/// Unlinking removes the link and says whether there was one.
#[tokio::test]
async fn unlinking_reports_whether_it_removed_anything() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .link_identity("https://idp.example", "sub-1", "ada", "ada")
        .await
        .unwrap();
    assert!(
        store
            .unlink_identity("https://idp.example", "ada")
            .await
            .unwrap()
    );
    assert!(
        !store
            .unlink_identity("https://idp.example", "ada")
            .await
            .unwrap(),
        "the second unlink had nothing to remove"
    );
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_none()
    );
}

/// An account provisioned by a sign-on has no password, so its identity
/// link is the only way into it: unlinking the last one is refused, in
/// words that name the command which gives it a second way in.
#[tokio::test]
async fn unlinking_the_last_way_into_an_account_is_refused() {
    let (store, _guard) = test_store().await;
    store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "ada",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    let err = store
        .unlink_identity("https://idp.example", "ada")
        .await
        .unwrap_err();
    let message = format!("{err:#}");
    assert!(
        message.contains("crystalline users passwd"),
        "the refusal teaches the way out: {message}"
    );
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::LastCredential),
        "the refusal travels as a type, not as prose"
    );
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_some(),
        "a refused unlink removes nothing"
    );
}

/// The rule is about the last way IN, not about the last link: an account
/// with a password may unlink everything, and an account with two
/// identities may unlink one of them.
#[tokio::test]
async fn a_second_way_in_is_what_makes_an_unlink_allowed() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .link_identity("https://idp.example", "sub-1", "ada", "ada")
        .await
        .unwrap();
    assert!(
        store
            .unlink_identity("https://idp.example", "ada")
            .await
            .unwrap(),
        "a password is the other way in"
    );

    store
        .provision_linked_user(
            "https://idp.example",
            "sub-2",
            "grace",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    store
        .link_identity("https://other.example", "sub-9", "grace", "grace")
        .await
        .unwrap();
    assert!(
        store
            .unlink_identity("https://idp.example", "grace")
            .await
            .unwrap(),
        "the other identity is the other way in"
    );
    let err = store
        .unlink_identity("https://other.example", "grace")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::LastCredential),
        "the one that is left is the last way in"
    );
}

/// An unlink that removes nothing is not a refusal: the account keeps
/// whatever it had, so there is nothing to protect it from.
#[tokio::test]
async fn an_unlink_at_an_issuer_holding_no_link_is_a_no_op() {
    let (store, _guard) = test_store().await;
    store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "ada",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert!(
        !store
            .unlink_identity("https://elsewhere.example", "ada")
            .await
            .unwrap(),
        "there was no link at that issuer to remove"
    );
}

/// The admin repair: an operator whose provider re-issued its subjects
/// unlinks the stale identity and links the new one, which needs a way
/// past the guard. Forcing it is the only way, and it does strand the
/// account until the link is remade.
#[tokio::test]
async fn a_forced_unlink_is_the_admin_repair() {
    let (store, _guard) = test_store().await;
    store
        .provision_linked_user(
            "https://idp.example",
            "old-sub",
            "ada",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert!(
        store
            .unlink_identity_force("https://idp.example", "ada")
            .await
            .unwrap()
    );
    assert!(store.identity_links("ada").await.unwrap().is_empty());
    store
        .link_identity("https://idp.example", "new-sub", "ada", LINKED_BY_CLI)
        .await
        .unwrap();
    let found = store
        .linked_user("https://idp.example", "new-sub")
        .await
        .unwrap()
        .expect("the repaired pair names the account");
    assert_eq!(found.name, "ada");
    assert_eq!(
        store.identity_links("ada").await.unwrap()[0].linked_by,
        LINKED_BY_CLI
    );
}

/// Whether an account has a local password, which is what the unlink rule
/// and the profile card both turn on. A name that is nobody has none.
#[tokio::test]
async fn a_password_is_reported_without_being_read_back() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .provision_linked_user(
            "https://idp.example",
            "sub-1",
            "grace",
            None,
            None,
            Role::Viewer,
            100,
        )
        .await
        .unwrap();
    assert!(store.has_password("ada").await.unwrap());
    assert!(!store.has_password("grace").await.unwrap());
    assert!(!store.has_password("nobody").await.unwrap());
    store.set_password("grace", "correct horse").await.unwrap();
    assert!(
        store.has_password("grace").await.unwrap(),
        "`users passwd` is what gives a provisioned account a second way in"
    );
}

/// Removing an account takes its identity links with it, so the pair
/// cannot resurrect a deleted account on the next sign-in.
#[tokio::test]
async fn removing_an_account_takes_its_identity_links() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .add_user("grace", "Grace", None, Role::Admin, "correct horse")
        .await
        .unwrap();
    store
        .link_identity("https://idp.example", "sub-1", "ada", "ada")
        .await
        .unwrap();
    store
        .link_identity("https://idp.example", "sub-2", "grace", "grace")
        .await
        .unwrap();

    store.remove_user("ada").await.unwrap();
    assert!(
        store
            .linked_user("https://idp.example", "sub-1")
            .await
            .unwrap()
            .is_none()
    );
    store.remove_user_force("grace").await.unwrap();
    assert!(
        store
            .linked_user("https://idp.example", "sub-2")
            .await
            .unwrap()
            .is_none()
    );
}

/// Presentation data refreshes, and an absent claim clears nothing.
#[tokio::test]
async fn refreshing_presentation_never_clears_what_was_not_sent() {
    let (store, _guard) = test_store().await;
    store
        .add_user(
            "ada",
            "Ada",
            Some("ada@example.test"),
            Role::Editor,
            "correct horse",
        )
        .await
        .unwrap();
    let refreshed = store
        .refresh_presentation("ada", Some("Ada L"), Some("new@example.test"))
        .await
        .unwrap();
    assert_eq!(refreshed.display, "Ada L");
    assert_eq!(refreshed.email.as_deref(), Some("new@example.test"));
    assert_eq!(refreshed.role, Role::Editor, "presentation moves no role");

    let untouched = store.refresh_presentation("ada", None, None).await.unwrap();
    assert_eq!(untouched.display, "Ada L");
    assert_eq!(untouched.email.as_deref(), Some("new@example.test"));
}

/// Two first sign-ins for one subject leave one account and one link.
///
/// The property the single transaction buys: the loser's account insert
/// rolls back with its refused link, rather than staying behind as an
/// orphan nobody can sign into. An implementation that created the account
/// and linked it in two calls would pass every other test here and leave
/// `ada-2` behind on this one.
#[tokio::test]
async fn two_first_sign_ins_for_one_subject_leave_one_account_and_one_link() {
    let (store, _guard) = test_store().await;
    let provision = || {
        store.provision_linked_user(
            "https://idp.example",
            "sub-1",
            "ada",
            Some("Ada"),
            None,
            Role::Viewer,
            100,
        )
    };
    let outcomes = {
        let (first, second) = tokio::join!(provision(), provision());
        [first, second]
    };
    assert_eq!(
        outcomes.iter().filter(|outcome| outcome.is_ok()).count(),
        1,
        "exactly one of the two races provisions"
    );
    let refusal = outcomes
        .iter()
        .find_map(|outcome| outcome.as_ref().err())
        .expect("the loser is refused");
    assert!(
        format!("{refusal:#}").contains("already linked"),
        "{refusal:#}"
    );
    assert_eq!(
        store.list_users().await.unwrap().len(),
        1,
        "the loser left no orphan account behind"
    );
    assert_eq!(store.identity_links("ada").await.unwrap().len(), 1);
}

/// The resource every grant below is issued for: the origin the MCP
/// transport is served from, no trailing slash, which is the spelling
/// [`normalize_resource`] settles on.
const OAUTH_RESOURCE: &str = "https://crystal.example";

/// The OAuth cast every grant test starts from: one account and one
/// registered client.
async fn oauth_cast(store: &AuthStore) -> OauthClient {
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    store
        .register_oauth_client(
            "Some Client",
            Some("https://client.example"),
            &["https://client.example/callback".to_string()],
        )
        .await
        .unwrap()
}

/// One integer column of one grant row, for the assertions that are about
/// what the database holds rather than about what a method answered.
async fn grant_int(store: &AuthStore, id: i64, column: &str) -> i64 {
    let row = store
        .db
        .query_first(
            &format!("SELECT {column} FROM oauth_grants WHERE id = ?1"),
            vec![Value::Integer(id)],
        )
        .await
        .unwrap()
        .expect("the grant row is there");
    match row.get_value(0) {
        Ok(Value::Integer(n)) => n,
        other => panic!("unexpected {column}: {other:?}"),
    }
}

/// The same for a text column.
async fn grant_text(store: &AuthStore, id: i64, column: &str) -> String {
    let row = store
        .db
        .query_first(
            &format!("SELECT {column} FROM oauth_grants WHERE id = ?1"),
            vec![Value::Integer(id)],
        )
        .await
        .unwrap()
        .expect("the grant row is there");
    cell_text(&row, 0).unwrap_or_default()
}

/// One text column of the token row a refresh token is stored under.
async fn token_text(store: &AuthStore, refresh_token: &str, column: &str) -> Option<String> {
    let row = store
        .db
        .query_first(
            &format!("SELECT {column} FROM oauth_grant_token WHERE refresh_hash = ?1"),
            vec![Value::Text(token_hash(refresh_token))],
        )
        .await
        .unwrap()
        .expect("the token row is there");
    cell_text(&row, 0)
}

/// The same for an integer column, `None` for NULL.
async fn token_int(store: &AuthStore, refresh_token: &str, column: &str) -> Option<i64> {
    let row = store
        .db
        .query_first(
            &format!("SELECT {column} FROM oauth_grant_token WHERE refresh_hash = ?1"),
            vec![Value::Text(token_hash(refresh_token))],
        )
        .await
        .unwrap()
        .expect("the token row is there");
    match row.get_value(0) {
        Ok(Value::Integer(n)) => Some(n),
        _ => None,
    }
}

async fn count_rows(store: &AuthStore, table: &str) -> i64 {
    let row = store
        .db
        .query_first(&format!("SELECT COUNT(*) FROM {table}"), vec![])
        .await
        .unwrap()
        .expect("COUNT(*) always answers");
    match row.get_value(0) {
        Ok(Value::Integer(n)) => n,
        other => panic!("unexpected COUNT(*) result: {other:?}"),
    }
}

/// A password reset also ends what an agent was given under the old
/// password: the account's MCP tokens and its OAuth grants, access and
/// refresh token alike. Another account's credentials are left alone.
#[tokio::test]
async fn a_password_reset_revokes_mcp_tokens_and_oauth_grants() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    store
        .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let token = store.issue_mcp_token("ada", "agent").await.unwrap();
    let grant = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let other_token = store.issue_mcp_token("bob", "agent").await.unwrap();
    let other_grant = store
        .issue_oauth_grant("bob", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    // An earlier ended grant, whose record the reset must keep.
    let earlier = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    assert!(store.revoke_oauth_grant("ada", earlier.id).await.unwrap());

    store.set_password("ada", "a new password").await.unwrap();

    // The person sees on the Connected clients card why the client was
    // signed out, and the record of the earlier revoke is still there.
    let ended = store.list_ended_oauth_grants("ada").await.unwrap();
    let reasons: Vec<_> = ended
        .iter()
        .map(|g| (g.id, g.ended.as_ref().unwrap().reason))
        .collect();
    assert_eq!(ended.len(), 2, "{reasons:?}");
    assert!(reasons.contains(&(grant.id, OauthGrantEndReason::PasswordReset)));
    assert!(reasons.contains(&(earlier.id, OauthGrantEndReason::Revoked)));
    assert_eq!(
        count_rows(&store, "oauth_grant_token").await,
        1,
        "only bob's"
    );
    assert!(
        store
            .list_ended_oauth_grants("bob")
            .await
            .unwrap()
            .is_empty()
    );

    assert!(
        store.mcp_token_user(&token.token).await.unwrap().is_none(),
        "the MCP token from before the reset is dead"
    );
    assert!(store.list_mcp_tokens("ada").await.unwrap().is_empty());
    assert!(
        store
            .oauth_access_user(&grant.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the OAuth access token from before the reset is dead"
    );
    assert!(
        matches!(
            store
                .refresh_oauth_grant(&grant.refresh_token, &client.client_id, None)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ),
        "and its refresh token cannot mint a new one"
    );
    assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());

    assert!(
        store
            .mcp_token_user(&other_token.token)
            .await
            .unwrap()
            .is_some(),
        "another account keeps its MCP token"
    );
    assert!(
        store
            .oauth_access_user(&other_grant.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some(),
        "and its OAuth grant"
    );
}

/// Removing an account takes its ended-grant records with it: there is
/// nobody left to read them, and the name may be claimed again.
#[tokio::test]
async fn removing_an_account_drops_its_ended_grant_records() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store.set_password("ada", "a new password").await.unwrap();
    assert_eq!(store.list_ended_oauth_grants("ada").await.unwrap().len(), 1);
    store.remove_user("ada").await.unwrap();
    assert_eq!(count_rows(&store, "oauth_grant_ended").await, 0);
}

/// A refused reset revokes nothing: the account that does not exist takes
/// no other account's tokens with it, and the transaction rolls back.
#[tokio::test]
async fn a_refused_password_reset_keeps_every_token() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let token = store.issue_mcp_token("ada", "agent").await.unwrap();
    let grant = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    assert!(store.set_password("ghost", "pw").await.is_err());
    assert!(store.mcp_token_user(&token.token).await.unwrap().is_some());
    assert!(
        store
            .list_ended_oauth_grants("ada")
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        store
            .oauth_access_user(&grant.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some()
    );
}

/// The audience rule and the expiry, the two properties an access token
/// carries beyond "this is ada". The expiry is reached by winding the
/// column back through the connection, because `issue_oauth_grant` takes
/// no TTL: the thirty days and the hour are the store's, not a caller's.
#[tokio::test]
async fn an_oauth_grant_resolves_only_for_its_resource_and_until_it_expires() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    assert!(issued.access_token.starts_with(OAUTH_ACCESS_PREFIX));
    assert!(issued.refresh_token.starts_with(OAUTH_REFRESH_PREFIX));
    assert_eq!(issued.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
    assert_eq!(
        token_text(&store, &issued.refresh_token, "access_hash").await,
        Some(token_hash(&issued.access_token)),
        "only the hash is written, the same way an MCP token is stored"
    );
    assert!(
        grant_text(&store, issued.id, "access_hash")
            .await
            .starts_with(OAUTH_PLACEHOLDER_PREFIX),
        "the grant row's own token columns hold only placeholders"
    );

    let user = store
        .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
        .await
        .unwrap()
        .expect("the token resolves for the resource it was issued for");
    assert_eq!(user.name, "ada");
    assert!(
        store
            .oauth_access_user(&issued.access_token, "https://crystal.example/")
            .await
            .unwrap()
            .is_some(),
        "one trailing slash is the same resource"
    );
    assert!(
        store
            .oauth_access_user(&issued.access_token, "https://other.example")
            .await
            .unwrap()
            .is_none(),
        "a token issued for one resource never opens another"
    );
    assert!(
        store
            .oauth_access_user("coa_not-a-token", OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none()
    );

    let listed = store.list_oauth_grants("ada").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert!(
        listed[0].last_used.is_some(),
        "resolving stamped the grant, which is what the management list shows"
    );

    store.set_disabled("ada", true).await.unwrap();
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "a disabled account's grant resolves for nobody"
    );
    store.set_disabled("ada", false).await.unwrap();
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some(),
        "and re-enabling hands it back, the way a disabled account's MCP tokens come back"
    );

    store
        .db
        .execute(
            "UPDATE oauth_grant_token SET access_expires_at = 1 WHERE grant_id = ?1",
            vec![Value::Integer(issued.id)],
        )
        .await
        .unwrap();
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "an expired access token resolves for nobody"
    );
}

/// A grant is a row about an account and a registration, so neither may be
/// invented by the write. The transaction is what makes the two checks
/// mean anything at the moment of the insert.
#[tokio::test]
async fn a_grant_needs_a_live_account_and_a_registered_client() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let ghost = store
        .issue_oauth_grant("ghost", &client.client_id, OAUTH_RESOURCE)
        .await
        .expect_err("an account that is nobody gets no grant");
    assert!(format!("{ghost:#}").contains("no such user"), "{ghost:#}");

    let stranger = store
        .issue_oauth_grant("ada", "coc_nobody", OAUTH_RESOURCE)
        .await
        .expect_err("a registration that does not exist gets no grant");
    assert!(
        format!("{stranger:#}").contains("no such oauth client"),
        "{stranger:#}"
    );

    store.set_disabled("ada", true).await.unwrap();
    let disabled = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .expect_err("a disabled account gets no new grant");
    assert!(format!("{disabled:#}").contains("disabled"), "{disabled:#}");
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
}

/// What a rotation is: one grant row moved along, both tokens replaced,
/// the predecessor's access token dead on the spot and the refresh window
/// restarted. Presenting the predecessor's *refresh* token is a replay
/// rather than a miss, so that half is
/// [`a_replayed_refresh_token_revokes_the_whole_grant`]'s.
#[tokio::test]
async fn a_refresh_rotates_both_tokens_and_the_predecessor_stops_working() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let first = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    // Bring the refresh window in close, so "the thirty days start at
    // rotation" cannot pass by accident on the window the issue opened.
    store
        .db
        .execute(
            "UPDATE oauth_grants SET refresh_expires_at = ?2 WHERE id = ?1",
            vec![
                Value::Integer(first.id),
                Value::Integer(chrono::Utc::now().timestamp() + 60),
            ],
        )
        .await
        .unwrap();

    let outcome = store
        .refresh_oauth_grant(&first.refresh_token, &client.client_id, None)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(second) = outcome else {
        panic!("a live refresh token rotates, got {outcome:?}");
    };
    assert_eq!(
        second.id, first.id,
        "a rotation moves one grant along rather than forking it"
    );
    assert_ne!(second.access_token, first.access_token);
    assert_ne!(second.refresh_token, first.refresh_token);
    assert_eq!(second.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
    assert!(
        store
            .oauth_access_user(&first.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the predecessor's access token stops the moment its successor exists"
    );
    assert!(
        store
            .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        grant_int(&store, first.id, "refresh_expires_at").await
            > chrono::Utc::now().timestamp() + OAUTH_REFRESH_TTL_SECS - 60,
        "the new refresh token's thirty days start at the rotation"
    );
    assert!(
        token_int(&store, &first.refresh_token, "rotated_at")
            .await
            .is_some(),
        "a rotated row is kept with its rotation time, which is what makes a replay detectable"
    );
    assert!(
        token_text(&store, &first.refresh_token, "access_hash")
            .await
            .is_none(),
        "and its access token is gone"
    );
    assert_eq!(
        token_text(&store, &second.refresh_token, "rotated_from").await,
        Some(token_hash(&first.refresh_token)),
        "the successor names what it was minted from"
    );
    assert_eq!(store.list_oauth_grants("ada").await.unwrap().len(), 1);
}

/// A rotated refresh token coming back is evidence that a copy of it
/// leaked, so the grant it belonged to is revoked rather than refreshed -
/// and it is evidence whoever presents it, which is why the rule keys on
/// the token alone while a rotation keys on the token and its client.
#[tokio::test]
async fn a_replayed_refresh_token_revokes_the_whole_grant() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let stranger = store
        .register_oauth_client(
            "Another Client",
            None,
            &["https://x.example/cb".to_string()],
        )
        .await
        .unwrap();
    let first = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let outcome = store
        .refresh_oauth_grant(&first.refresh_token, &client.client_id, None)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(second) = outcome else {
        panic!("a live refresh token rotates, got {outcome:?}");
    };

    let replay = store
        .refresh_oauth_grant(&first.refresh_token, &stranger.client_id, None)
        .await
        .unwrap();
    match replay {
        RefreshOutcome::Replayed { grant, .. } => assert_eq!(grant, first.id),
        other => panic!("a rotated refresh token is a replay, got {other:?}"),
    }
    assert!(
        store
            .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the revocation took the successor's access token with it"
    );
    assert!(
        matches!(
            store
                .refresh_oauth_grant(&second.refresh_token, &client.client_id, None)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ),
        "and its refresh token, because the whole row is gone"
    );
    assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    let ended = store.list_ended_oauth_grants("ada").await.unwrap();
    assert_eq!(ended.len(), 1, "the replay is recorded");
    assert_eq!(
        ended[0].ended.as_ref().map(|e| e.reason),
        Some(OauthGrantEndReason::Replayed)
    );
}

/// A token two rotations back is a replay, even inside the grace window.
/// The token it was rotated into has itself been rotated, so whoever
/// held the answer has moved on and does not need the old token any
/// more: its return is evidence of a copy, not of a parallel session.
#[tokio::test]
async fn a_token_two_rotations_back_is_a_replay() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let first = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let outcome = store
        .refresh_oauth_grant_at(&first.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(second) = outcome else {
        panic!("a live refresh token rotates, got {outcome:?}");
    };
    let outcome = store
        .refresh_oauth_grant_at(&second.refresh_token, &client.client_id, None, now + 1)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(third) = outcome else {
        panic!("the successor rotates in its turn, got {outcome:?}");
    };
    assert!(
        store
            .oauth_access_user(&third.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some()
    );

    match store
        .refresh_oauth_grant_at(&first.refresh_token, &client.client_id, None, now + 2)
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed {
            grant,
            since_rotation,
        } => {
            assert_eq!(grant, first.id);
            assert_eq!(since_rotation, 2, "seconds since its own rotation");
        }
        other => panic!("the token two rotations back is a replay, got {other:?}"),
    }
    assert!(
        store
            .oauth_access_user(&third.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the replay revoked the grant, the newest pair with it"
    );
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
}

/// The three ways a refresh misses without being a replay. None of them
/// may touch the grant: an unknown token is a stranger, a live token at
/// the wrong registration is a client mixing up its credentials, and an
/// expired one is a client that waited too long. Only a token that was
/// actually rotated revokes anything.
#[tokio::test]
async fn an_unknown_refresh_token_is_neither_rotated_nor_a_replay() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let stranger = store
        .register_oauth_client(
            "Another Client",
            None,
            &["https://x.example/cb".to_string()],
        )
        .await
        .unwrap();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();

    for (token, client_id, why) in [
        (
            "cor_nothing".to_string(),
            client.client_id.clone(),
            "a token nobody issued",
        ),
        (
            issued.refresh_token.clone(),
            stranger.client_id.clone(),
            "a live token at the wrong registration",
        ),
    ] {
        assert!(
            matches!(
                store
                    .refresh_oauth_grant(&token, &client_id, None)
                    .await
                    .unwrap(),
                RefreshOutcome::Unknown
            ),
            "{why} rotates nothing"
        );
    }

    store.set_disabled("ada", true).await.unwrap();
    assert!(
        matches!(
            store
                .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ),
        "a disabled account refreshes nothing"
    );
    store.set_disabled("ada", false).await.unwrap();

    store
        .db
        .execute(
            "UPDATE oauth_grants SET refresh_expires_at = 1 WHERE id = ?1",
            vec![Value::Integer(issued.id)],
        )
        .await
        .unwrap();
    assert!(
        matches!(
            store
                .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ),
        "an expired refresh token rotates nothing"
    );
    assert_eq!(
        count_rows(&store, "oauth_grants").await,
        1,
        "and none of the four refusals deleted the grant"
    );
}

/// Revoking is one delete, so both of a grant's tokens stop together, and
/// removing the account sweeps what is left - the resurrection hazard
/// `mcp_tokens` documents, one table over: a grant row that outlived its
/// account would be inherited by the next holder of the freed name.
#[tokio::test]
async fn revoking_a_grant_stops_both_tokens_and_removing_the_account_sweeps_them() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();

    let listed = store.list_oauth_grants("ada").await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, issued.id);
    assert_eq!(listed[0].client_id, client.client_id);
    assert_eq!(listed[0].client_name, "Some Client");
    assert_eq!(
        listed[0].redirect_host, "client.example",
        "the card names the host the client redirects to"
    );
    assert!(listed[0].last_used.is_none());
    assert!(
        chrono::DateTime::parse_from_rfc3339(&listed[0].refresh_expires_at).is_ok(),
        "the expiry a person reads is RFC 3339: {}",
        listed[0].refresh_expires_at
    );

    store
        .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    assert!(
        !store.revoke_oauth_grant("bob", issued.id).await.unwrap(),
        "another account's revoke never reaches this grant"
    );
    assert!(store.revoke_oauth_grant("ada", issued.id).await.unwrap());
    assert!(
        !store.revoke_oauth_grant("ada", issued.id).await.unwrap(),
        "a second revoke removes nothing"
    );
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        matches!(
            store
                .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
                .await
                .unwrap(),
            RefreshOutcome::Unknown
        ),
        "one deleted row stops both tokens at once"
    );

    let second = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store.remove_user("ada").await.unwrap();
    assert!(
        store
            .oauth_access_user(&second.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        count_rows(&store, "oauth_grants").await,
        0,
        "the removal swept the row rather than leaving it for the next holder of the name"
    );
    assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
}

/// The forced removal path sweeps the same rows the guarded one does. The
/// two are separate statements that have to be kept in step, which is why
/// every sweep on this file carries a test on both.
#[tokio::test]
async fn force_removing_an_account_sweeps_its_oauth_grants_too() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store.remove_user_force("ada").await.unwrap();
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
}

/// A registration is what a client gets instead of a secret, so it has to
/// come back exactly as it was written, including the order of its
/// redirect uris - the token endpoint compares against them.
#[tokio::test]
async fn a_registration_is_read_back_by_its_client_id_and_touched() {
    let (store, _guard) = test_store().await;
    assert_eq!(store.count_oauth_clients().await.unwrap(), 0);
    let uris = vec![
        "http://127.0.0.1:33418/callback".to_string(),
        "https://client.example/cb".to_string(),
    ];
    let client = store
        .register_oauth_client("Some Client", Some("https://client.example"), &uris)
        .await
        .unwrap();
    assert!(client.client_id.starts_with(OAUTH_CLIENT_PREFIX));
    assert_eq!(
        client.client_id.len(),
        OAUTH_CLIENT_PREFIX.len() + 32,
        "a client id is the prefix plus 32 hex characters"
    );
    assert!(client.last_used.is_none());

    let read = store
        .oauth_client(&client.client_id)
        .await
        .unwrap()
        .expect("the registration reads back by its id");
    assert_eq!(read.client_name, "Some Client");
    assert_eq!(read.client_uri.as_deref(), Some("https://client.example"));
    assert_eq!(read.redirect_uris, uris);
    assert_eq!(read.created_at, client.created_at);
    assert!(store.oauth_client("coc_nobody").await.unwrap().is_none());
    assert_eq!(store.count_oauth_clients().await.unwrap(), 1);

    store.touch_oauth_client(&client.client_id).await.unwrap();
    let touched = store
        .oauth_client(&client.client_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        touched.last_used.is_some(),
        "an authorization a person allowed stamps the registration"
    );
    store
        .touch_oauth_client("coc_nobody")
        .await
        .expect("touching a registration that is gone is a no-op, not an error");

    assert!(
        store
            .register_oauth_client("Some Client", None, &[])
            .await
            .is_err(),
        "a registration with nowhere to redirect is refused"
    );
    let second = store
        .register_oauth_client("Another", None, &["https://a.example/cb".to_string()])
        .await
        .unwrap();
    assert_ne!(second.client_id, client.client_id);
    assert_eq!(store.count_oauth_clients().await.unwrap(), 2);
}

/// Registrations arrive one per fresh connection and nobody deletes them,
/// so the store prunes the ones that never became a connection. A
/// registration somebody is still connected through is never pruned, and
/// an authorization a person allowed buys another thirty days.
#[tokio::test]
async fn an_unused_registration_is_pruned_after_thirty_days() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let fresh = store
        .register_oauth_client("Fresh", None, &["https://fresh.example/cb".to_string()])
        .await
        .unwrap();
    let stale = store
        .register_oauth_client("Stale", None, &["https://stale.example/cb".to_string()])
        .await
        .unwrap();
    let used = store
        .register_oauth_client("Used", None, &["https://used.example/cb".to_string()])
        .await
        .unwrap();
    let issued = store
        .issue_oauth_grant("ada", &used.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();

    // Age two of the three through the connection: the thirty days are the
    // store's, and no method winds the clock back.
    let long_ago = "2020-01-01T00:00:00+00:00";
    for id in [&stale.client_id, &used.client_id] {
        store
            .db
            .execute(
                "UPDATE oauth_clients SET created_at = ?2, last_used = NULL
                 WHERE client_id = ?1",
                vec![Value::Text(id.clone()), Value::Text(long_ago.to_string())],
            )
            .await
            .unwrap();
    }
    assert_eq!(store.count_oauth_clients().await.unwrap(), 3);
    assert_eq!(
        store.prune_oauth_clients().await.unwrap(),
        1,
        "only the old registration that never produced a grant goes"
    );
    assert!(
        store
            .oauth_client(&stale.client_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .oauth_client(&fresh.client_id)
            .await
            .unwrap()
            .is_some(),
        "a registration inside the window stays"
    );
    assert!(
        store.oauth_client(&used.client_id).await.unwrap().is_some(),
        "and so does an old one somebody is still connected through"
    );
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some(),
        "the prune left the live connection alone"
    );

    // An authorization somebody allowed stamps `last_used`, and that is
    // what keeps an old registration alive for another thirty days. Nothing
    // an unauthenticated request does reaches this column: see
    // `only_an_allowed_authorization_stamps_a_registration_as_used`.
    store
        .db
        .execute(
            "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
            vec![
                Value::Text(fresh.client_id.clone()),
                Value::Text(long_ago.to_string()),
            ],
        )
        .await
        .unwrap();
    store.touch_oauth_client(&fresh.client_id).await.unwrap();
    assert_eq!(store.prune_oauth_clients().await.unwrap(), 0);
    assert_eq!(store.count_oauth_clients().await.unwrap(), 2);
}

/// **Reading a registration is not using it**: only an authorization that
/// a person actually allowed moves a row off the one-hour clock.
///
/// The distinction is the whole of why the short clock works. Everything an
/// authorization *request* does to a registration is read it - look it up
/// by the id the caller named, compare the redirect uri it presented - and
/// that request carries no identity: the person has not signed in yet, and
/// a caller that registered the client is authorizing against its own row,
/// so every check on that leg passes by construction. If reading stamped
/// `last_used`, one extra unauthenticated request per row would move it to
/// the thirty-day branch, and the fill-and-hold the short clock exists to
/// prevent would be back at the cost of one GET.
///
/// So [`AuthStore::touch_oauth_client`] is called when consent is ALLOWED
/// and when a grant is issued or refreshed, never on the authorize request,
/// and nothing else in this file writes that column. This test is the
/// contract in executable form for the two tasks that hold those call
/// sites.
#[tokio::test]
async fn only_an_allowed_authorization_stamps_a_registration_as_used() {
    let (store, _guard) = test_store().await;
    let client = store
        .register_oauth_client("Asker", None, &["https://asker.example/cb".to_string()])
        .await
        .unwrap();
    assert!(client.last_used.is_none());

    // Everything the authorize leg does to the row before a person has
    // decided anything, twice over.
    for _ in 0..2 {
        let read = store
            .oauth_client(&client.client_id)
            .await
            .unwrap()
            .expect("the registration reads back");
        assert!(
            read.last_used.is_none(),
            "reading a registration never stamps it"
        );
    }

    // Two hours later that row is still on the short clock, because
    // nothing it has been through counts as an authorization.
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    store
        .db
        .execute(
            "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
            vec![
                Value::Text(client.client_id.clone()),
                Value::Text(two_hours_ago.clone()),
            ],
        )
        .await
        .unwrap();
    assert_eq!(
        store.prune_oauth_clients().await.unwrap(),
        1,
        "an hour of being asked about is not an hour of being used"
    );

    // The same row, with the one thing that does count: a person allowed
    // it, which is what the consent endpoint stamps.
    let allowed = store
        .register_oauth_client("Allowed", None, &["https://allowed.example/cb".to_string()])
        .await
        .unwrap();
    store
        .db
        .execute(
            "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
            vec![
                Value::Text(allowed.client_id.clone()),
                Value::Text(two_hours_ago),
            ],
        )
        .await
        .unwrap();
    store.touch_oauth_client(&allowed.client_id).await.unwrap();
    assert_eq!(store.prune_oauth_clients().await.unwrap(), 0);
    assert!(
        store
            .oauth_client(&allowed.client_id)
            .await
            .unwrap()
            .is_some(),
        "an allowed authorization buys the thirty days"
    );
}

/// **A registration nobody ever authorized expires within the hour**, and
/// only one somebody did gets the thirty days.
///
/// The two clocks exist because the two rows mean different things. A row
/// that never reached the authorize endpoint is the residue of a client
/// that registered and walked away - or of an anonymous caller filling the
/// table, which is the only way the cap is ever reached - and keeping it
/// for thirty days is what would make that filling stick. A row that HAS
/// authorized is somebody's client between connections, and thirty idle
/// days is the right patience for it.
#[tokio::test]
async fn a_registration_that_never_authorized_expires_within_the_hour() {
    let (store, _guard) = test_store().await;
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let never = store
        .register_oauth_client("Never", None, &["https://never.example/cb".to_string()])
        .await
        .unwrap();
    let authorized = store
        .register_oauth_client("Asked", None, &["https://asked.example/cb".to_string()])
        .await
        .unwrap();
    let connected = store
        .register_oauth_client("Live", None, &["https://live.example/cb".to_string()])
        .await
        .unwrap();
    store
        .issue_oauth_grant("ada", &connected.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let just_now = store
        .register_oauth_client("Fresh", None, &["https://fresh.example/cb".to_string()])
        .await
        .unwrap();

    // Two hours ago for all three of the old ones, and the one that
    // authorized did so half an hour ago: well past the hour, nowhere near
    // the thirty days.
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    let half_hour_ago = (chrono::Utc::now() - chrono::Duration::minutes(30)).to_rfc3339();
    for id in [
        &never.client_id,
        &authorized.client_id,
        &connected.client_id,
    ] {
        store
            .db
            .execute(
                "UPDATE oauth_clients SET created_at = ?2 WHERE client_id = ?1",
                vec![Value::Text(id.clone()), Value::Text(two_hours_ago.clone())],
            )
            .await
            .unwrap();
    }
    store
        .db
        .execute(
            "UPDATE oauth_clients SET last_used = ?2 WHERE client_id = ?1",
            vec![
                Value::Text(authorized.client_id.clone()),
                Value::Text(half_hour_ago),
            ],
        )
        .await
        .unwrap();

    assert_eq!(
        store.prune_oauth_clients().await.unwrap(),
        1,
        "only the registration nobody authorized, holding nothing, goes"
    );
    assert!(
        store
            .oauth_client(&never.client_id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        store
            .oauth_client(&authorized.client_id)
            .await
            .unwrap()
            .is_some(),
        "a registration somebody authorized gets the thirty days"
    );
    assert!(
        store
            .oauth_client(&connected.client_id)
            .await
            .unwrap()
            .is_some(),
        "and one somebody is connected through is never collected at all"
    );
    assert!(
        store
            .oauth_client(&just_now.client_id)
            .await
            .unwrap()
            .is_some(),
        "a registration made a moment ago has its hour"
    );

    // The same row, thirty idle days later.
    store
        .db
        .execute(
            "UPDATE oauth_clients SET last_used = ?2 WHERE client_id = ?1",
            vec![
                Value::Text(authorized.client_id.clone()),
                Value::Text("2020-01-01T00:00:00+00:00".to_string()),
            ],
        )
        .await
        .unwrap();
    assert_eq!(store.prune_oauth_clients().await.unwrap(), 1);
    assert!(
        store
            .oauth_client(&authorized.client_id)
            .await
            .unwrap()
            .is_none()
    );
}

/// The two unhashed copies of a live credential must never be one
/// `tracing::debug!` or one failed assertion away from a log file, and the
/// enum that carries a rotation's answer must not undo that.
#[tokio::test]
async fn an_issued_oauth_grant_never_prints_its_secrets() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let text = format!("{issued:?}");
    for (prefix, token) in [
        (OAUTH_ACCESS_PREFIX, &issued.access_token),
        (OAUTH_REFRESH_PREFIX, &issued.refresh_token),
    ] {
        let secret = token
            .strip_prefix(prefix)
            .expect("a token carries its prefix");
        assert!(!text.contains(secret), "the secret is redacted: {text}");
    }
    assert!(text.contains("redacted"), "and says so: {text}");
    assert!(
        text.contains(&issued.id.to_string()),
        "while the id still prints: {text}"
    );

    let rotated = store
        .refresh_oauth_grant(&issued.refresh_token, &client.client_id, None)
        .await
        .unwrap();
    let printed = format!("{rotated:?}");
    let RefreshOutcome::Rotated(next) = &rotated else {
        panic!("a live refresh token rotates, got {printed}");
    };
    let secret = next
        .access_token
        .strip_prefix(OAUTH_ACCESS_PREFIX)
        .expect("a token carries its prefix");
    assert!(
        !printed.contains(secret),
        "the outcome redacts what it wraps: {printed}"
    );
}

/// A grant past its refresh window can never work again: no rotation, no
/// replay, and its access token died an hour into the thirty days. So it
/// is neither a connection to show a person nor a row to keep - and while
/// it is kept, it pins its registration against the client prune, which is
/// what makes the ordering of the two prunes load bearing.
#[tokio::test]
async fn an_expired_grant_is_pruned_and_never_listed_as_a_connection() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let live = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let dead = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    close_refresh_window(&store, dead.id).await;

    let listed = store.list_oauth_grants("ada").await.unwrap();
    assert_eq!(
        listed.len(),
        1,
        "a grant that cannot work again is not a connection anybody can act on"
    );
    assert_eq!(listed[0].id, live.id);
    assert_eq!(store.prune_oauth_grants().await.unwrap(), 1);
    assert_eq!(count_rows(&store, "oauth_grants").await, 1);
    assert!(
        store
            .oauth_access_user(&live.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some(),
        "the prune left the live grant alone"
    );

    // A registration abandoned after one connection: its dead grant holds
    // it against the client prune until the grant prune runs first.
    let abandoned = store
        .register_oauth_client("Abandoned", None, &["https://gone.example/cb".to_string()])
        .await
        .unwrap();
    let stranded = store
        .issue_oauth_grant("ada", &abandoned.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    close_refresh_window(&store, stranded.id).await;
    // Age *both* registrations past the window, so what keeps the first
    // one is its live grant rather than its freshness - which is the
    // property the last assertion is about.
    for id in [&abandoned.client_id, &client.client_id] {
        store
            .db
            .execute(
                "UPDATE oauth_clients SET created_at = '2020-01-01T00:00:00+00:00'
                 WHERE client_id = ?1",
                vec![Value::Text(id.clone())],
            )
            .await
            .unwrap();
    }

    assert_eq!(
        store.prune_oauth_clients().await.unwrap(),
        0,
        "while the dead grant is there it pins the registration"
    );
    assert_eq!(store.prune_oauth_grants().await.unwrap(), 1);
    assert_eq!(
        store.prune_oauth_clients().await.unwrap(),
        1,
        "and the registration goes in the same pass, once its grant has"
    );
    assert!(
        store
            .oauth_client(&client.client_id)
            .await
            .unwrap()
            .is_some(),
        "the registration somebody is still connected through stays, \
         old as it is, because its live grant is what holds it"
    );
}

/// Close a grant's refresh window through the connection, the only way to
/// reach an expiry the store owns.
async fn close_refresh_window(store: &AuthStore, id: i64) {
    store
        .db
        .execute(
            "UPDATE oauth_grants SET refresh_expires_at = 1 WHERE id = ?1",
            vec![Value::Integer(id)],
        )
        .await
        .unwrap();
}

/// Two parallel sessions of one client refresh with the same token: the
/// first rotates, the second arrives inside the grace window and gets a
/// pair of its own. Both pairs work, both refresh tokens are live, and the
/// grant survives. The write lock decides the order, so racing requests
/// reach the same two answers.
#[tokio::test]
async fn two_parallel_refreshes_inside_the_window_both_succeed() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        store.refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now),
        store.refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now),
    );
    let mut pairs = Vec::new();
    let mut reissued = 0;
    for outcome in [one.unwrap(), two.unwrap()] {
        match outcome {
            RefreshOutcome::Rotated(pair) => pairs.push(pair),
            RefreshOutcome::Reissued {
                grant,
                since_rotation,
            } => {
                assert_eq!(since_rotation, 0);
                reissued += 1;
                pairs.push(grant);
            }
            other => panic!("both parallel refreshes succeed, got {other:?}"),
        }
    }
    assert_eq!(reissued, 1, "one rotates, the other is reissued");
    assert_ne!(pairs[0].refresh_token, pairs[1].refresh_token);
    assert_ne!(pairs[0].access_token, pairs[1].access_token);
    for pair in &pairs {
        assert_eq!(pair.id, issued.id, "one grant, not a fork");
        assert_eq!(pair.expires_in, OAUTH_ACCESS_TTL_SECS as u64);
        assert!(
            store
                .oauth_access_user(&pair.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some(),
            "each session's access token works"
        );
    }
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the rotated token's own access token stopped"
    );
    // Each new refresh token is live in its own right.
    for (i, pair) in pairs.iter().enumerate() {
        let outcome = store
            .refresh_oauth_grant_at(&pair.refresh_token, &client.client_id, None, now + 10)
            .await
            .unwrap();
        assert!(
            matches!(outcome, RefreshOutcome::Rotated(_)),
            "session {i} rotates its own token, got {outcome:?}"
        );
    }
    assert_eq!(store.list_oauth_grants("ada").await.unwrap().len(), 1);
    assert!(
        store
            .list_ended_oauth_grants("ada")
            .await
            .unwrap()
            .is_empty(),
        "nothing ended"
    );
}

/// The same token presented again 60 seconds or more after its rotation
/// is a replay: the grant is revoked and the record says why. 59 seconds
/// is still inside the window.
#[tokio::test]
async fn the_same_token_after_the_window_revokes_the_grant() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let outcome = store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(rotated) = outcome else {
        panic!("a live refresh token rotates, got {outcome:?}");
    };
    let outcome = store
        .refresh_oauth_grant_at(
            &issued.refresh_token,
            &client.client_id,
            None,
            now + OAUTH_REFRESH_GRACE_SECS - 1,
        )
        .await
        .unwrap();
    assert!(
        matches!(
            outcome,
            RefreshOutcome::Reissued {
                since_rotation: 59,
                ..
            }
        ),
        "59 seconds is inside the window, got {outcome:?}"
    );
    match store
        .refresh_oauth_grant_at(
            &issued.refresh_token,
            &client.client_id,
            None,
            now + OAUTH_REFRESH_GRACE_SECS,
        )
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed {
            grant,
            since_rotation,
        } => {
            assert_eq!(grant, issued.id);
            assert_eq!(since_rotation, OAUTH_REFRESH_GRACE_SECS);
        }
        other => panic!("60 seconds after the rotation is a replay, got {other:?}"),
    }
    assert!(
        store
            .oauth_access_user(&rotated.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none(),
        "the whole grant is gone"
    );
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
    let ended = store.list_ended_oauth_grants("ada").await.unwrap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended[0].id, issued.id);
    assert_eq!(ended[0].client_name, "Some Client");
    assert_eq!(ended[0].redirect_host, "client.example");
    assert_eq!(
        ended[0].ended.as_ref().map(|e| e.reason),
        Some(OauthGrantEndReason::Replayed)
    );
}

/// Inside the window, the token still has to come from the client it was
/// issued to: another client presenting it is a replay, not a parallel
/// session. And a disabled account gets nothing, as on the live path.
#[tokio::test]
async fn the_grace_window_keeps_the_client_and_account_checks() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let stranger = store
        .register_oauth_client(
            "Another Client",
            None,
            &["https://x.example/cb".to_string()],
        )
        .await
        .unwrap();
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();

    store.set_disabled("ada", true).await.unwrap();
    assert!(matches!(
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 1)
            .await
            .unwrap(),
        RefreshOutcome::Unknown
    ));
    store.set_disabled("ada", false).await.unwrap();
    assert!(matches!(
        store
            .refresh_oauth_grant_at(
                &issued.refresh_token,
                &client.client_id,
                Some("https://other.example"),
                now + 1
            )
            .await
            .unwrap(),
        RefreshOutcome::WrongTarget
    ));
    assert!(matches!(
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &stranger.client_id, None, now + 1)
            .await
            .unwrap(),
        RefreshOutcome::Replayed { .. }
    ));
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
}

/// The live refresh tokens of a grant, counted in the database.
async fn live_tokens(store: &AuthStore) -> i64 {
    store
        .count_tokens(
            "SELECT COUNT(*) FROM oauth_grant_token WHERE rotated_at IS NULL",
            vec![],
        )
        .await
        .unwrap()
}

/// Whether the row of `refresh_token` is still in the database.
async fn token_row_exists(store: &AuthStore, refresh_token: &str) -> bool {
    store
        .db
        .query_first(
            "SELECT 1 FROM oauth_grant_token WHERE refresh_hash = ?1",
            vec![Value::Text(token_hash(refresh_token))],
        )
        .await
        .unwrap()
        .is_some()
}

/// A copy of one rotated token presented over and over inside the window
/// gets at most two pairs of its own (a retry and a parallel session);
/// the third presentation is a replay that ends the grant. At no point is
/// a session's live token deleted to make room, so the real session is
/// never pushed out while the copy keeps the grant.
#[tokio::test]
async fn a_flood_of_copies_inside_the_window_revokes_and_evicts_nobody() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(real) = store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap()
    else {
        panic!("a live refresh token rotates");
    };
    let mut copies = Vec::new();
    for i in 1..=OAUTH_REISSUES_PER_ROTATION {
        match store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + i)
            .await
            .unwrap()
        {
            RefreshOutcome::Reissued { grant, .. } => copies.push(grant),
            other => panic!("presentation {i} inside the window is reissued, got {other:?}"),
        }
        assert!(
            token_row_exists(&store, &real.refresh_token).await,
            "the real session's token is never deleted"
        );
        assert!(
            store
                .oauth_access_user(&real.access_token, OAUTH_RESOURCE)
                .await
                .unwrap()
                .is_some()
        );
    }
    assert_eq!(live_tokens(&store).await, 1 + OAUTH_REISSUES_PER_ROTATION);

    match store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 5)
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed {
            grant,
            since_rotation,
        } => {
            assert_eq!(grant, issued.id);
            assert_eq!(since_rotation, 5);
        }
        other => panic!("the third copy inside the window is a replay, got {other:?}"),
    }
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
    assert_eq!(
        store.list_ended_oauth_grants("ada").await.unwrap()[0]
            .ended
            .as_ref()
            .map(|e| e.reason),
        Some(OauthGrantEndReason::Replayed)
    );
}

/// The live set has a hard ceiling, and reaching it never deletes a
/// session: a reissue into a full set is a replay.
#[tokio::test]
async fn a_reissue_into_a_full_live_set_is_a_replay() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();
    // Sessions left behind earlier, written straight into the table.
    for i in 1..OAUTH_LIVE_TOKENS_MAX {
        store
            .db
            .execute(
                "INSERT INTO oauth_grant_token
                     (refresh_hash, grant_id, access_hash, access_expires_at,
                      refresh_expires_at, issued_at, rotated_at, rotated_from)
                 VALUES (?1, ?2, NULL, 0, ?3, ?4, NULL, NULL)",
                vec![
                    Value::Text(format!("filler{i}")),
                    Value::Integer(issued.id),
                    Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                    Value::Integer(now - 100),
                ],
            )
            .await
            .unwrap();
    }
    assert_eq!(live_tokens(&store).await, OAUTH_LIVE_TOKENS_MAX);
    assert!(matches!(
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 1)
            .await
            .unwrap(),
        RefreshOutcome::Replayed { .. }
    ));
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);
}

/// Rotating a chain many times never makes an old token forgotten: the
/// rotated hashes are kept by time, not by count, so a token nine and more
/// rotations back is still a replay.
#[tokio::test]
async fn a_token_many_rotations_back_still_replays() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let mut current = issued.clone();
    for i in 0..12 {
        match store
            .refresh_oauth_grant_at(&current.refresh_token, &client.client_id, None, now + i)
            .await
            .unwrap()
        {
            RefreshOutcome::Rotated(next) => current = next,
            other => panic!("a live token rotates, got {other:?}"),
        }
    }
    assert_eq!(live_tokens(&store).await, 1);
    assert_eq!(count_rows(&store, "oauth_grant_token").await, 13);
    match store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 3600)
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed { grant, .. } => assert_eq!(grant, issued.id),
        other => panic!("a token twelve rotations back is a replay, got {other:?}"),
    }
}

/// A rotated token is forgotten only once thirty days have passed since
/// its rotation, by which time its own window has closed as well.
#[tokio::test]
async fn a_rotated_token_is_forgotten_after_thirty_days() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(second) = store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap()
    else {
        panic!("a live refresh token rotates");
    };
    store
        .db
        .execute(
            "UPDATE oauth_grant_token SET rotated_at = ?2 WHERE refresh_hash = ?1",
            vec![
                Value::Text(token_hash(&issued.refresh_token)),
                Value::Integer(now - OAUTH_REFRESH_TTL_SECS + 10),
            ],
        )
        .await
        .unwrap();
    let RefreshOutcome::Rotated(third) = store
        .refresh_oauth_grant_at(&second.refresh_token, &client.client_id, None, now + 1)
        .await
        .unwrap()
    else {
        panic!("a live refresh token rotates");
    };
    assert!(
        token_row_exists(&store, &issued.refresh_token).await,
        "younger than thirty days: kept"
    );
    store
        .refresh_oauth_grant_at(&third.refresh_token, &client.client_id, None, now + 20)
        .await
        .unwrap();
    assert!(
        !token_row_exists(&store, &issued.refresh_token).await,
        "thirty days after its rotation it is pruned"
    );
}

/// The window of a rotated token closes once any pair minted from it has
/// been rotated, a sibling from inside the window included.
#[tokio::test]
async fn a_siblings_rotation_closes_the_window_for_its_parent() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();
    let RefreshOutcome::Reissued { grant: sibling, .. } = store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 1)
        .await
        .unwrap()
    else {
        panic!("inside the window the token is reissued");
    };
    assert!(matches!(
        store
            .refresh_oauth_grant_at(&sibling.refresh_token, &client.client_id, None, now + 2)
            .await
            .unwrap(),
        RefreshOutcome::Rotated(_)
    ));
    assert!(matches!(
        store
            .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now + 3)
            .await
            .unwrap(),
        RefreshOutcome::Replayed { .. }
    ));
}

/// A clock stepped back makes the age negative, and a negative age is
/// outside the window rather than a window that never closes.
#[tokio::test]
async fn a_rotation_time_in_the_future_is_no_reissue() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let now = chrono::Utc::now().timestamp();
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now)
        .await
        .unwrap();
    match store
        .refresh_oauth_grant_at(&issued.refresh_token, &client.client_id, None, now - 5)
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed { since_rotation, .. } => assert_eq!(since_rotation, -5),
        other => panic!("a negative age is a replay, got {other:?}"),
    }
}

/// The daemon and the CLI hold two store handles on one file; two
/// refreshes of one token through both at the same moment still get one
/// rotation and one reissue, and the grant lives.
#[tokio::test]
async fn parallel_refreshes_through_two_stores_on_one_file_both_succeed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    let daemon = AuthStore::open(&path).await.unwrap();
    let cli = AuthStore::open(&path).await.unwrap();
    let client = oauth_cast(&daemon).await;
    let issued = daemon
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        daemon.refresh_oauth_grant(&issued.refresh_token, &client.client_id, None),
        cli.refresh_oauth_grant(&issued.refresh_token, &client.client_id, None),
    );
    let outcomes = [one.unwrap(), two.unwrap()];
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, RefreshOutcome::Rotated(_)))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(
        outcomes
            .iter()
            .filter(|o| matches!(o, RefreshOutcome::Reissued { .. }))
            .count(),
        1,
        "{outcomes:?}"
    );
    assert_eq!(daemon.list_oauth_grants("ada").await.unwrap().len(), 1);
    assert_eq!(cli.list_oauth_grants("ada").await.unwrap().len(), 1);
}

/// The record of an ended grant: written for a replay (above) and for an
/// explicit revoke here, listed newest first, left out after seven days,
/// and never carrying a token.
#[tokio::test]
async fn an_explicit_revoke_is_recorded_and_listed_for_a_while() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let one = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    let two = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    assert!(store.revoke_oauth_grant("ada", one.id).await.unwrap());
    assert!(
        store
            .revoke_oauth_grant_by_token(&two.refresh_token, &client.client_id)
            .await
            .unwrap()
    );
    assert!(
        !store.revoke_oauth_grant("ada", one.id).await.unwrap(),
        "an ended grant cannot be revoked again"
    );
    assert_eq!(count_rows(&store, "oauth_grant_token").await, 0);
    let ended = store.list_ended_oauth_grants("ada").await.unwrap();
    let reasons: Vec<_> = ended
        .iter()
        .map(|g| (g.id, g.ended.as_ref().unwrap().reason))
        .collect();
    assert!(reasons.contains(&(one.id, OauthGrantEndReason::Revoked)));
    assert!(reasons.contains(&(two.id, OauthGrantEndReason::ClientRevoked)));
    assert!(store.list_oauth_grants("ada").await.unwrap().is_empty());
    let json = serde_json::to_string(&ended).unwrap();
    assert!(json.contains("\"reason\":\"revoked\""), "{json}");
    assert!(!json.contains(OAUTH_ACCESS_PREFIX) && !json.contains(OAUTH_REFRESH_PREFIX));

    let later = chrono::Utc::now().timestamp() + OAUTH_ENDED_KEPT_SECS + 1;
    assert!(
        store
            .list_ended_oauth_grants_at("ada", later)
            .await
            .unwrap()
            .is_empty(),
        "after seven days the record is no longer listed"
    );
    store
        .add_user("bob", "Bob", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    assert!(
        store
            .list_ended_oauth_grants("bob")
            .await
            .unwrap()
            .is_empty()
    );
}

/// The rotation time of a migrated predecessor is read off the access
/// token's issue. A predecessor rotated ten seconds before the upgrade is
/// inside the window afterwards and is reissued, not a replay.
#[tokio::test]
async fn a_migrated_predecessor_inside_the_window_is_reissued() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    let previous = format!("{OAUTH_REFRESH_PREFIX}{}", random_hex());
    let (id, client_id) = {
        let store = AuthStore::open(&path).await.unwrap();
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        let id = store
            .returned_id(
                "INSERT INTO oauth_grants
                     (\"user\", client_id, resource, access_hash, access_expires_at,
                      refresh_hash, previous_refresh_hash, refresh_expires_at, created_at)
                 VALUES ('ada', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
                vec![
                    Value::Text(client.client_id.clone()),
                    Value::Text(OAUTH_RESOURCE.to_string()),
                    Value::Text(token_hash("coa_current")),
                    Value::Integer(now + OAUTH_ACCESS_TTL_SECS - 10),
                    Value::Text(token_hash("cor_current")),
                    Value::Text(token_hash(&previous)),
                    Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .unwrap();
        (id, client.client_id)
    };
    let store = AuthStore::open(&path).await.unwrap();
    let rotated_at = token_int(&store, &previous, "rotated_at").await.unwrap();
    let now = chrono::Utc::now().timestamp();
    assert!(
        (now - 12..=now - 8).contains(&rotated_at),
        "the rotation time is the current access token's issue: {rotated_at} vs {now}"
    );
    match store
        .refresh_oauth_grant(&previous, &client_id, None)
        .await
        .unwrap()
    {
        RefreshOutcome::Reissued { grant, .. } => assert_eq!(grant.id, id),
        other => panic!("inside the window after the upgrade, got {other:?}"),
    }
}

/// A file written before 0.32.1 holds each grant's pair in `oauth_grants`
/// itself, with the predecessor in `previous_refresh_hash`. Opening it
/// moves both into `oauth_grant_token`: the access token keeps working,
/// the refresh token rotates, and the predecessor is still a replay
/// (its rotation time is read off the access token's issue, an hour ago
/// here, far outside the window).
#[tokio::test]
async fn an_old_store_with_a_previous_refresh_hash_keeps_working() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    let access = format!("{OAUTH_ACCESS_PREFIX}{}", random_hex());
    let refresh = format!("{OAUTH_REFRESH_PREFIX}{}", random_hex());
    let previous = format!("{OAUTH_REFRESH_PREFIX}{}", random_hex());
    let id = {
        let store = AuthStore::open(&path).await.unwrap();
        let client = oauth_cast(&store).await;
        let now = chrono::Utc::now().timestamp();
        // What the 0.32.0 issue and rotation wrote, column by column.
        store
            .returned_id(
                "INSERT INTO oauth_grants
                     (\"user\", client_id, resource, access_hash, access_expires_at,
                      refresh_hash, previous_refresh_hash, refresh_expires_at, created_at)
                 VALUES ('ada', ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING id",
                vec![
                    Value::Text(client.client_id.clone()),
                    Value::Text(OAUTH_RESOURCE.to_string()),
                    Value::Text(token_hash(&access)),
                    Value::Integer(now + 60),
                    Value::Text(token_hash(&refresh)),
                    Value::Text(token_hash(&previous)),
                    Value::Integer(now + OAUTH_REFRESH_TTL_SECS),
                    Value::Text(chrono::Utc::now().to_rfc3339()),
                ],
            )
            .await
            .unwrap()
    };
    let store = AuthStore::open(&path).await.unwrap();
    // A second open moves nothing twice.
    drop(store);
    let store = AuthStore::open(&path).await.unwrap();
    assert_eq!(count_rows(&store, "oauth_grant_token").await, 2);
    assert!(
        grant_text(&store, id, "refresh_hash")
            .await
            .starts_with(OAUTH_PLACEHOLDER_PREFIX)
    );
    let client_id = store.list_oauth_grants("ada").await.unwrap()[0]
        .client_id
        .clone();
    assert!(
        store
            .oauth_access_user(&access, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_some(),
        "the access token minted before the upgrade still resolves"
    );
    let outcome = store
        .refresh_oauth_grant(&refresh, &client_id, None)
        .await
        .unwrap();
    let RefreshOutcome::Rotated(next) = outcome else {
        panic!("the refresh token minted before the upgrade rotates, got {outcome:?}");
    };
    assert_eq!(next.id, id);
    match store
        .refresh_oauth_grant(&previous, &client_id, None)
        .await
        .unwrap()
    {
        RefreshOutcome::Replayed { grant, .. } => assert_eq!(grant, id),
        other => panic!("the old predecessor is still a replay, got {other:?}"),
    }
}

/// The host on the consent screen and the grant card is a fact a person is
/// asked to recognize, so a uri that names no host must not get to write
/// that line itself.
#[test]
fn a_redirect_uri_with_no_host_never_shows_its_own_text() {
    assert_eq!(redirect_host("https://client.example/cb"), "client.example");
    assert_eq!(
        redirect_host("http://127.0.0.1:33418/callback"),
        "127.0.0.1:33418"
    );
    for uri in [
        "javascript:alert(1)",
        "data:text/html,<b>your bank</b>",
        "not a url at all",
    ] {
        assert_eq!(
            redirect_host(uri),
            NO_REDIRECT_HOST,
            "a uri with no host says so rather than speaking for itself: {uri}"
        );
    }
}

/// The empty string is not an audience. Refusing it at both ends means the
/// two halves of the check cannot agree on nothing and let a token
/// through, even if an origin derivation upstream ever produced one.
#[tokio::test]
async fn an_empty_resource_is_refused_on_both_sides() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    for resource in ["", "/", "   "] {
        assert!(
            store
                .issue_oauth_grant("ada", &client.client_id, resource)
                .await
                .is_err(),
            "a grant with no audience is refused: {resource:?}"
        );
    }
    assert_eq!(count_rows(&store, "oauth_grants").await, 0);

    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .db
        .execute(
            "UPDATE oauth_grants SET resource = '' WHERE id = ?1",
            vec![Value::Integer(issued.id)],
        )
        .await
        .unwrap();
    for resource in ["", "/"] {
        assert!(
            store
                .oauth_access_user(&issued.access_token, resource)
                .await
                .unwrap()
                .is_none(),
            "an empty origin opens nothing, whatever a row holds: {resource:?}"
        );
    }
}

/// A grant whose registration vanished stays listable, so it stays
/// revocable: its tokens still resolve, and a list that dropped the row
/// would leave the account no way to stop them.
#[tokio::test]
async fn a_grant_outlives_a_vanished_registration_rather_than_hiding() {
    let (store, _guard) = test_store().await;
    let client = oauth_cast(&store).await;
    let issued = store
        .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
        .await
        .unwrap();
    store
        .db
        .execute("DELETE FROM oauth_clients", vec![])
        .await
        .unwrap();

    let listed = store.list_oauth_grants("ada").await.unwrap();
    assert_eq!(listed.len(), 1, "the grant is still listed");
    assert_eq!(listed[0].id, issued.id);
    assert!(
        !listed[0].client_name.is_empty(),
        "with something a person can read where the registration was"
    );
    assert!(store.revoke_oauth_grant("ada", issued.id).await.unwrap());
    assert!(
        store
            .oauth_access_user(&issued.access_token, OAUTH_RESOURCE)
            .await
            .unwrap()
            .is_none()
    );
}

/// The resource comparison is the audience check, so its tolerance is
/// exactly one trailing slash: nothing is lowercased and no other
/// canonicalization happens, because an origin that differs in any other
/// way is a different origin.
#[test]
fn a_resource_tolerates_one_trailing_slash_and_nothing_else() {
    assert_eq!(
        normalize_resource("https://crystal.example/"),
        "https://crystal.example"
    );
    assert_eq!(
        normalize_resource("  https://crystal.example  "),
        "https://crystal.example"
    );
    assert_eq!(
        normalize_resource("https://crystal.example//"),
        "https://crystal.example/",
        "one slash, not every slash"
    );
    assert_eq!(
        normalize_resource("https://Crystal.Example"),
        "https://Crystal.Example",
        "case is the caller's business, not the store's"
    );
    assert_eq!(normalize_resource("/"), "");
}

/// Every account a share-link test needs: the author who holds a draft,
/// the colleague the link is for, and a third party who finds the link.
async fn grant_cast(store: &AuthStore) {
    for name in ["alice", "bob", "carol"] {
        store
            .add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
}

/// A share-link is for one person, and the first one to open it is that
/// person for good.
///
/// The whole of the sharing rule: a grant is minted for a draft rather
/// than for a name, so the author never has to know the login of whoever
/// they are handing it to - and the moment somebody opens it, it belongs
/// to that account and to nobody else. A second account presenting the
/// same token is told the same thing an invented one is told, because a
/// link that admitted a second reader would be a draft shared with
/// whoever the first reader forwarded it to.
#[tokio::test]
async fn a_grant_binds_its_first_redeemer_and_refuses_a_second() {
    let (store, _guard) = test_store().await;
    grant_cast(&store).await;
    let minted = store
        .mint_overlay_grant("team", "plan.md", "alice", None)
        .await
        .unwrap();
    assert!(minted.token.starts_with(DRAFT_LINK_PREFIX));

    let bound = store
        .redeem_overlay_grant(&minted.token, "bob")
        .await
        .unwrap()
        .expect("the first redeemer binds it");
    assert_eq!(
        (
            bound.domain.as_str(),
            bound.path.as_str(),
            bound.owner.as_str()
        ),
        ("team", "plan.md", "alice")
    );
    assert_eq!(bound.grantee.as_deref(), Some("bob"));

    assert!(
        store
            .redeem_overlay_grant(&minted.token, "bob")
            .await
            .unwrap()
            .is_some(),
        "and the person it bound to keeps it"
    );
    assert!(
        store
            .redeem_overlay_grant(&minted.token, "carol")
            .await
            .unwrap()
            .is_none(),
        "while anybody else is told what an invented token is told"
    );
    assert_eq!(
        store
            .overlay_grant_for("bob", "team", "plan.md")
            .await
            .unwrap()
            .as_deref(),
        Some("alice"),
        "the bound account sees whose draft it is"
    );
    assert!(
        store
            .overlay_grant_for("carol", "team", "plan.md")
            .await
            .unwrap()
            .is_none(),
        "and nobody else sees anything"
    );
}

/// The two ways a grant ends, and they end it the same way: the token
/// still exists and resolves to nothing.
#[tokio::test]
async fn a_revoked_or_expired_grant_redeems_to_none() {
    let (store, _guard) = test_store().await;
    grant_cast(&store).await;

    let revoked = store
        .mint_overlay_grant("team", "plan.md", "alice", None)
        .await
        .unwrap();
    let listed = store
        .overlay_grants_of("alice", "team", "plan.md")
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "the author sees what they minted");
    assert!(
        store
            .revoke_overlay_grant("bob", revoked.id, false)
            .await
            .unwrap()
            .is_none(),
        "and nobody else can take it back"
    );
    let swept = store
        .mint_overlay_grant("team", "plan.md", "alice", None)
        .await
        .unwrap();
    assert!(
        store
            .revoke_overlay_grant("bob", swept.id, true)
            .await
            .unwrap()
            .is_some(),
        "an admin closes a link on this instance whoever minted it"
    );
    assert!(
        store
            .redeem_overlay_grant(&swept.token, "bob")
            .await
            .unwrap()
            .is_none(),
        "and the row was stamped, not only answered"
    );
    let taken = store
        .revoke_overlay_grant("alice", revoked.id, false)
        .await
        .unwrap()
        .expect("its author takes it back");
    assert_eq!(
        (
            taken.domain.as_str(),
            taken.owner.as_str(),
            taken.path.as_str()
        ),
        ("team", "alice", "plan.md"),
        "and the row says which draft it was on, which is what ends the sessions inside it"
    );
    assert!(
        taken.revoked_at.is_some(),
        "answered as it now stands, taken back"
    );
    assert!(
        store
            .revoke_overlay_grant("alice", revoked.id, false)
            .await
            .unwrap()
            .is_none(),
        "and taking back what is already back is nothing at all"
    );
    assert!(
        store
            .redeem_overlay_grant(&revoked.token, "bob")
            .await
            .unwrap()
            .is_none(),
        "a revoked link admits nobody"
    );

    let expired = store
        .mint_overlay_grant(
            "team",
            "plan.md",
            "alice",
            Some("2020-01-01T00:00:00Z".to_string()),
        )
        .await
        .unwrap();
    assert!(
        store
            .redeem_overlay_grant(&expired.token, "bob")
            .await
            .unwrap()
            .is_none(),
        "and neither does one whose window has closed"
    );

    let live = store
        .mint_overlay_grant("team", "plan.md", "alice", None)
        .await
        .unwrap();
    store
        .redeem_overlay_grant(&live.token, "bob")
        .await
        .unwrap()
        .expect("a live one still binds");
    assert_eq!(
        store.end_domain_overlay_grants("team").await.unwrap(),
        2,
        "and the domain leaving review mode ends every link nobody had \
         revoked: the live one and the expired one, whose window had closed \
         but whose row was still open"
    );
    assert!(
        store
            .overlay_grant_for("bob", "team", "plan.md")
            .await
            .unwrap()
            .is_none(),
        "so the account it was bound to holds nothing any more"
    );
}

/// The link is the credential, so it may never reach a log line - the rule
/// [`IssuedMcpToken`] already keeps, kept the same way and asserted on the
/// random half for the same reason.
#[tokio::test]
async fn minted_grant_debug_redacts_the_token() {
    let (store, _guard) = test_store().await;
    grant_cast(&store).await;
    let minted = store
        .mint_overlay_grant("team", "plan.md", "alice", None)
        .await
        .unwrap();
    let secret = minted
        .token
        .strip_prefix(DRAFT_LINK_PREFIX)
        .expect("a link carries the prefix");
    let text = format!("{minted:?}");
    assert!(!text.contains(secret), "the secret is redacted: {text}");
    assert!(text.contains("redacted"), "and says so: {text}");
    assert!(
        text.contains(&minted.id.to_string()),
        "while the id still prints: {text}"
    );
}

#[tokio::test]
async fn one_github_id_links_to_one_account_per_host() {
    let (auth, _guard) = test_store().await;
    for name in ["ann", "bea"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    let link = auth
        .link_github("ann", "GitHub.com", 42, "ann-gh")
        .await
        .unwrap();
    assert_eq!(link.host, "github.com");
    assert_eq!(link.github_id, 42);
    assert!(link.token_live);
    let err = auth
        .link_github("bea", "github.com", 42, "other")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::GithubIdTaken)
    );
    assert!(err.to_string().contains(
        "this GitHub account is already linked to another Crystalline account; an admin can unlink it there"
    ));
    // The same id on another host is another person.
    auth.link_github("bea", "ghe.example.com", 42, "bea-ghe")
        .await
        .unwrap();
    assert_eq!(
        auth.account_for_github("github.com", 42)
            .await
            .unwrap()
            .as_deref(),
        Some("ann")
    );
    assert_eq!(
        auth.account_for_github("ghe.example.com", 42)
            .await
            .unwrap()
            .as_deref(),
        Some("bea")
    );
    assert_eq!(
        auth.account_for_github("github.com", 7).await.unwrap(),
        None
    );
    assert_eq!(auth.github_links().await.unwrap().len(), 2);
}

#[tokio::test]
async fn relinking_replaces_the_row() {
    let (auth, _guard) = test_store().await;
    auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("ann", "github.com", 1, "old")
        .await
        .unwrap();
    auth.mark_github_token_gone("ann").await.unwrap();
    let link = auth
        .link_github("ann", "github.com", 2, "new")
        .await
        .unwrap();
    assert_eq!(link.github_id, 2);
    assert!(link.token_live, "a fresh link has a live token");
    let all = auth.github_links().await.unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].login, "new");
    assert_eq!(
        auth.account_for_github("github.com", 1).await.unwrap(),
        None
    );
    // The same id again is a relink of one's own row, not a conflict.
    auth.link_github("ann", "github.com", 2, "renamed")
        .await
        .unwrap();
    assert_eq!(
        auth.github_link_of("ann").await.unwrap().unwrap().login,
        "renamed"
    );
    auth.set_github_login("ann", "again").await.unwrap();
    assert_eq!(
        auth.github_link_of("ann").await.unwrap().unwrap().login,
        "again"
    );
    assert!(auth.unlink_github("ann").await.unwrap());
    assert!(!auth.unlink_github("ann").await.unwrap());
}

#[tokio::test]
async fn removing_a_user_removes_its_link() {
    let (auth, _guard) = test_store().await;
    auth.add_user("root", "Root", None, Role::Admin, "pw12345678")
        .await
        .unwrap();
    for name in ["ann", "bea"] {
        auth.add_user(name, name, None, Role::Editor, "pw12345678")
            .await
            .unwrap();
    }
    auth.link_github("ann", "github.com", 5, "ann")
        .await
        .unwrap();
    auth.link_github("bea", "github.com", 6, "bea")
        .await
        .unwrap();
    auth.remove_user("ann").await.unwrap();
    auth.remove_user_force("bea").await.unwrap();
    assert!(auth.github_links().await.unwrap().is_empty());
    auth.add_user("cy", "Cy", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("cy", "github.com", 5, "cy").await.unwrap();
}

#[tokio::test]
async fn marking_the_token_gone_keeps_the_link() {
    let (auth, _guard) = test_store().await;
    auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("ann", "github.com", 9, "ann")
        .await
        .unwrap();
    assert!(auth.mark_github_token_gone("ann").await.unwrap());
    let link = auth.github_link_of("ann").await.unwrap().unwrap();
    assert!(!link.token_live);
    assert_eq!(link.github_id, 9);
    assert!(!auth.mark_github_token_gone("nobody").await.unwrap());
}

#[tokio::test]
async fn an_old_database_gains_the_table_on_open() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("web-auth.db");
    {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let conn = db.connect().unwrap();
        let start = SCHEMA
            .find("CREATE TABLE IF NOT EXISTS github_link")
            .unwrap();
        let end = start + SCHEMA[start..].find("github_link_host_id").unwrap();
        let end = end + SCHEMA[end..].find(";\n").unwrap() + 2;
        let old = format!("{}{}", &SCHEMA[..start], &SCHEMA[end..]);
        assert!(!old.contains("github_link"));
        conn.execute_batch(&old).await.unwrap();
    }
    let auth = AuthStore::open(&path).await.unwrap();
    auth.add_user("ann", "Ann", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    auth.link_github("ann", "github.com", 3, "ann")
        .await
        .unwrap();
    assert_eq!(auth.github_links().await.unwrap().len(), 1);
}

// ---- Per-domain Reflection collaborators (Task 3.4) ----

async fn reflection_grant_cast(auth: &AuthStore) {
    for name in ["ann", "bea"] {
        auth.add_user(name, name, None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn a_reflection_grant_is_stored_listed_and_removed() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    auth.set_reflection_grant("eng", "Ann", ReflectionLevel::Reader, "boss")
        .await
        .unwrap();
    auth.set_reflection_grant("eng", "bea", ReflectionLevel::Contributor, "boss")
        .await
        .unwrap();
    // A second write changes the level of the one row.
    auth.set_reflection_grant("eng", "ann", ReflectionLevel::Contributor, "keeper")
        .await
        .unwrap();
    let grants = auth.reflection_grants("eng").await.unwrap();
    let seen: Vec<(&str, ReflectionLevel, &str)> = grants
        .iter()
        .map(|g| (g.account.as_str(), g.level, g.added_by.as_str()))
        .collect();
    assert_eq!(
        seen,
        vec![
            ("ann", ReflectionLevel::Contributor, "keeper"),
            ("bea", ReflectionLevel::Contributor, "boss"),
        ]
    );
    assert_eq!(
        auth.reflection_grant("eng", "ann").await.unwrap(),
        Some(ReflectionLevel::Contributor)
    );
    assert_eq!(auth.reflection_grant("ops", "ann").await.unwrap(), None);
    assert!(auth.remove_reflection_grant("eng", "ANN").await.unwrap());
    assert!(!auth.remove_reflection_grant("eng", "ann").await.unwrap());
    assert_eq!(auth.reflection_grant("eng", "ann").await.unwrap(), None);
}

#[tokio::test]
async fn a_default_grant_is_refused() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    let err = auth
        .set_reflection_grant("eng", "ann", ReflectionLevel::Default, "boss")
        .await
        .unwrap_err();
    assert_eq!(
        StoreRefusal::kind_of(&err),
        Some(RefusalKind::EmptyMembership)
    );
    assert!(format!("{err}").contains("use remove"), "{err}");
    assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
}

#[tokio::test]
async fn a_grant_names_a_live_account() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    auth.set_disabled("bea", true).await.unwrap();
    for name in ["nobody", "bea", crate::scope::OWNER_IDENTITY_NAME] {
        let err = auth
            .set_reflection_grant("eng", name, ReflectionLevel::Reader, "boss")
            .await
            .unwrap_err();
        assert_eq!(
            StoreRefusal::kind_of(&err),
            Some(RefusalKind::NoSuchAccount),
            "{name}: {err}"
        );
    }
    assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
}

/// A shared domain has no acl, member or share-link row, so the grants
/// alone must carry the rename. Stale grants under the new name go.
#[tokio::test]
async fn renaming_a_domain_carries_its_grants() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "boss")
        .await
        .unwrap();
    auth.set_reflection_grant("ops", "bea", ReflectionLevel::Contributor, "boss")
        .await
        .unwrap();
    auth.rename_domain("eng", "ops").await.unwrap();
    assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
    let moved: Vec<String> = auth
        .reflection_grants("ops")
        .await
        .unwrap()
        .into_iter()
        .map(|g| g.account)
        .collect();
    assert_eq!(moved, vec!["ann".to_string()]);
    // Again after a crash: nothing under the old name, nothing changes.
    auth.rename_domain("eng", "ops").await.unwrap();
    assert_eq!(auth.reflection_grants("ops").await.unwrap().len(), 1);
}

#[tokio::test]
async fn forgetting_a_domain_drops_its_grants() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "boss")
        .await
        .unwrap();
    auth.set_reflection_grant("ops", "ann", ReflectionLevel::Reader, "boss")
        .await
        .unwrap();
    assert!(!auth.forget_domain("eng").await.unwrap());
    assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
    assert_eq!(auth.reflection_grants("ops").await.unwrap().len(), 1);
}

/// A grant left behind would be inherited by the next account to take
/// the name, as a membership row would.
#[tokio::test]
async fn removing_an_account_drops_its_grants() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    for name in ["ann", "bea"] {
        auth.set_reflection_grant("eng", name, ReflectionLevel::Reader, "boss")
            .await
            .unwrap();
    }
    auth.remove_user("ann").await.unwrap();
    auth.remove_user_force("bea").await.unwrap();
    auth.add_user("ann", "ann", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    assert!(auth.reflection_grants("eng").await.unwrap().is_empty());
    assert_eq!(auth.reflection_grant("eng", "ann").await.unwrap(), None);
}

/// Making a domain shared forgets its members, never its Reflection
/// collaborators: a grant does not depend on the visibility.
#[tokio::test]
async fn a_grant_outlives_a_visibility_change_and_a_transfer() {
    let (auth, _guard) = test_store().await;
    reflection_grant_cast(&auth).await;
    auth.set_domain_visibility("eng", true, "bea")
        .await
        .unwrap();
    auth.set_reflection_grant("eng", "ann", ReflectionLevel::Reader, "bea")
        .await
        .unwrap();
    auth.add_user("cy", "cy", None, Role::Viewer, "pw12345678")
        .await
        .unwrap();
    auth.transfer_domain("eng", "cy").await.unwrap();
    auth.set_domain_visibility("eng", false, "cy")
        .await
        .unwrap();
    assert_eq!(
        auth.reflection_grant("eng", "ann").await.unwrap(),
        Some(ReflectionLevel::Reader)
    );
}

/// The first-admin claim runs inside `BEGIN IMMEDIATE`, as the other
/// writes do, so the same text is one transaction on Postgres too.
#[tokio::test]
async fn provisioning_writes_run_in_a_transaction() {
    let (auth, _guard) = test_store().await;
    let before = auth.db.begins();
    assert!(
        auth.add_first_admin("root", "Root", "rootpw1234")
            .await
            .unwrap()
    );
    assert_eq!(auth.db.begins(), before + 1);
    let before = auth.db.begins();
    auth.ensure_user("ada", Role::Viewer, usize::MAX)
        .await
        .unwrap();
    auth.register_oauth_client("c", None, &["https://x.test/cb".to_string()])
        .await
        .unwrap();
    assert_eq!(auth.db.begins(), before + 2);
}

/// Turso stamps `last_seen_at` and `last_used` on every lookup, as it always
/// has: the 60 s threshold is the Postgres arm's alone.
#[tokio::test]
async fn turso_stamps_on_every_lookup() {
    let dir = tempfile::tempdir().unwrap();
    let store = AuthStore::open(&dir.path().join("web-auth.db"))
        .await
        .unwrap();
    store
        .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
        .await
        .unwrap();
    let session = store.create_session("ada", 3600).await.unwrap();
    store.session_user(&session.token).await.unwrap().unwrap();
    let first = store.user("ada").await.unwrap().unwrap().last_seen;
    store.session_user(&session.token).await.unwrap().unwrap();
    let second = store.user("ada").await.unwrap().unwrap().last_seen;
    assert!(first.is_some());
    assert_ne!(first, second, "every lookup stamps on Turso");
}

/// The Postgres arm's own tests: each runs in a scratch schema of its own
/// when `CRYSTALLINE_TEST_POSTGRES_URL` is set, whatever
/// `CRYSTALLINE_TEST_AUTH_BACKEND` names, and skips with a note otherwise.
mod postgres {
    use super::*;
    use crate::auth_store::db::pg_test_store;

    fn pg_url() -> Option<String> {
        let url = std::env::var("CRYSTALLINE_TEST_POSTGRES_URL")
            .ok()
            .filter(|u| !u.is_empty());
        if url.is_none() {
            eprintln!("skipped: CRYSTALLINE_TEST_POSTGRES_URL is not set");
        }
        url
    }

    /// The tables a migration's SQL creates.
    fn created_tables(sql: &str) -> Vec<String> {
        let mut out = Vec::new();
        let mut rest = sql;
        while let Some(at) = rest.find("CREATE TABLE ") {
            rest = &rest[at + "CREATE TABLE ".len()..];
            let name = rest.trim_start_matches("IF NOT EXISTS ");
            let name: String = name
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                .collect();
            out.push(name);
        }
        out
    }

    /// The auth, index and instance tables share a schema, so their names
    /// must not meet, and a wipe of the index never touches an account or
    /// the owner row.
    #[test]
    fn auth_index_instance_tables_disjoint_and_not_wiped() {
        use crystalline_index::postgres::{INDEX_SET, INSTANCE_TABLES, WIPE_TABLES};
        let auth_created: Vec<String> = AUTH_MIGRATIONS
            .steps
            .iter()
            .flat_map(|m| created_tables(m.sql))
            .collect();
        let mut listed: Vec<&str> = AUTH_TABLES.to_vec();
        listed.sort_unstable();
        let mut created: Vec<&str> = auth_created.iter().map(String::as_str).collect();
        created.sort_unstable();
        assert_eq!(
            listed, created,
            "AUTH_TABLES lists what the auth set creates"
        );
        let index: Vec<String> = INDEX_SET
            .steps
            .iter()
            .flat_map(|m| created_tables(m.sql))
            .collect();
        assert!(index.iter().any(|t| t == "engram"), "{index:?}");
        let ledgers = [AUTH_MIGRATIONS.ledger, INDEX_SET.ledger];
        for table in AUTH_TABLES.iter().chain(&[AUTH_MIGRATIONS.ledger]) {
            assert!(
                !index.iter().any(|t| t == table),
                "{table} is also an index table"
            );
            assert!(
                !INSTANCE_TABLES.contains(table),
                "{table} is an instance table"
            );
            assert!(!WIPE_TABLES.contains(table), "a wipe would clear {table}");
        }
        for table in INSTANCE_TABLES {
            assert!(!WIPE_TABLES.contains(table), "a wipe would clear {table}");
        }
        assert_ne!(ledgers[0], ledgers[1]);
    }

    /// The write lock mixes in the schema, and it is the `auth-write` key.
    #[test]
    fn auth_write_lock_key_is_the_schema_mixed_lock_key() {
        use crystalline_index::postgres::lock_key;
        assert_eq!(
            auth_write_lock(Some("a")),
            lock_key(Some("a"), "auth-write")
        );
        assert_ne!(auth_write_lock(Some("a")), auth_write_lock(Some("b")));
        assert_ne!(auth_write_lock(None), lock_key(None, "instance-owner"));
    }

    /// BOOL comes back as `Integer(0 or 1)`, the integer types as `Integer`,
    /// text as `Text` and a typed NULL as `Null`; a type the store does not
    /// read is an error, not a guess.
    #[tokio::test]
    async fn decoder_maps_bool_to_integer() {
        let Some(url) = pg_url() else { return };
        let (store, _guard) = pg_test_store(&url, "dec").await;
        let row = store
            .db
            .query_first(
                "SELECT true, false, CAST(NULL AS BOOLEAN), CAST(7 AS SMALLINT),
                        CAST(8 AS INTEGER), CAST(9 AS BIGINT), 'x', ?1 IS NULL",
                vec![Value::Null],
            )
            .await
            .unwrap()
            .unwrap();
        let cells: Vec<Value> = (0..8).map(|i| row.get_value(i).unwrap()).collect();
        assert_eq!(
            cells,
            vec![
                Value::Integer(1),
                Value::Integer(0),
                Value::Null,
                Value::Integer(7),
                Value::Integer(8),
                Value::Integer(9),
                Value::Text("x".to_string()),
                Value::Integer(1),
            ]
        );
        let err = store
            .db
            .query("SELECT CAST(1 AS NUMERIC)", vec![])
            .await
            .err()
            .expect("a NUMERIC column is refused");
        assert!(format!("{err:#}").contains("NUMERIC"), "{err:#}");
        // A NULL into a BIGINT column is written with the portable cast.
        store
            .db
            .execute(
                "INSERT INTO github_delivery (id, received_at) VALUES (?1, COALESCE(CAST(?2 AS BIGINT), 5))",
                vec![Value::Text("d".to_string()), Value::Null],
            )
            .await
            .unwrap();
    }

    /// A password login on Postgres: `password_present` selects a boolean,
    /// which only works because BOOL decodes to an integer.
    #[tokio::test]
    async fn password_login_works_on_postgres() {
        let Some(url) = pg_url() else { return };
        let (store, _guard) = pg_test_store(&url, "pwl").await;
        assert!(store.db.is_postgres());
        store
            .add_user("ada", "Ada", None, Role::Admin, "correct horse")
            .await
            .unwrap();
        assert!(store.has_password("ada").await.unwrap());
        let user = store
            .verify_password("ada", "correct horse")
            .await
            .unwrap()
            .expect("the right password logs in");
        assert_eq!(user.role, Role::Admin);
        assert!(
            store
                .verify_password("ada", "wrong")
                .await
                .unwrap()
                .is_none()
        );
        let session = store.create_session("ada", 3600).await.unwrap();
        let (user, _csrf) = store.session_user(&session.token).await.unwrap().unwrap();
        assert_eq!(user.name, "ada");
        store.ensure_user("bob", Role::Viewer, 10).await.unwrap();
        assert!(!store.has_password("bob").await.unwrap());
        assert!(store.passwordless_users().await.unwrap().contains("bob"));
    }

    /// Two stores (a daemon and a `users` command) racing for the first
    /// admin: exactly one wins, and the table holds one account.
    #[tokio::test]
    async fn two_stores_add_first_admin_once() {
        let Some(url) = pg_url() else { return };
        for _ in 0..5 {
            let (a, guard) = pg_test_store(&url, "faa").await;
            let b = guard.second().await;
            let (one, two) = tokio::join!(
                a.add_first_admin("ada", "Ada", "pw12345678"),
                b.add_first_admin("bob", "Bob", "pw12345678"),
            );
            assert!(one.unwrap() ^ two.unwrap(), "exactly one first admin");
            assert_eq!(a.user_count().await.unwrap(), 1);
            assert_eq!(a.list_users().await.unwrap()[0].role, Role::Admin);
        }
    }

    /// The account cap and the client cap count inside the write lock, so
    /// two stores never both take the last place.
    #[tokio::test]
    async fn user_cap_and_client_cap_hold_under_two_stores() {
        let Some(url) = pg_url() else { return };
        for _ in 0..5 {
            let (a, guard) = pg_test_store(&url, "cap").await;
            let b = guard.second().await;
            let (one, two) = tokio::join!(
                a.ensure_user_created("ada", Role::Viewer, 1),
                b.ensure_user_created("bob", Role::Viewer, 1),
            );
            assert!(
                one.is_ok() ^ two.is_ok(),
                "exactly one account under a cap of 1"
            );
            let refused = one.err().or(two.err()).unwrap();
            assert_eq!(
                StoreRefusal::kind_of(&refused),
                Some(RefusalKind::CapReached)
            );
            assert_eq!(a.user_count().await.unwrap(), 1);
            let uris = ["https://client.example/cb".to_string()];
            let (one, two) = tokio::join!(
                a.register_oauth_client_within("One", None, &uris, Some(1)),
                b.register_oauth_client_within("Two", None, &uris, Some(1)),
            );
            let (one, two) = (one.unwrap(), two.unwrap());
            assert!(
                one.is_some() ^ two.is_some(),
                "exactly one client under a cap of 1"
            );
            assert_eq!(a.count_oauth_clients().await.unwrap(), 1);
        }
    }

    /// Two stores presenting one refresh token at the same moment: the first
    /// rotates it, the second, seeing the committed rotation, gets a pair of
    /// its own inside the grace window.
    #[tokio::test]
    async fn concurrent_refresh_gives_one_rotated_one_reissued() {
        let Some(url) = pg_url() else { return };
        for _ in 0..5 {
            let (a, guard) = pg_test_store(&url, "ref").await;
            let b = guard.second().await;
            a.add_user("ada", "Ada", None, Role::Editor, "pw12345678")
                .await
                .unwrap();
            let client = a
                .register_oauth_client("Client", None, &["https://c.example/cb".to_string()])
                .await
                .unwrap();
            let grant = a
                .issue_oauth_grant("ada", &client.client_id, OAUTH_RESOURCE)
                .await
                .unwrap();
            let now = chrono::Utc::now().timestamp();
            let (one, two) = tokio::join!(
                a.refresh_oauth_grant_at(&grant.refresh_token, &client.client_id, None, now),
                b.refresh_oauth_grant_at(&grant.refresh_token, &client.client_id, None, now),
            );
            let outcomes = [one.unwrap(), two.unwrap()];
            let rotated = outcomes
                .iter()
                .filter(|o| matches!(o, RefreshOutcome::Rotated(_)))
                .count();
            let reissued = outcomes
                .iter()
                .filter(|o| matches!(o, RefreshOutcome::Reissued { .. }))
                .count();
            assert_eq!((rotated, reissued), (1, 1), "{outcomes:?}");
        }
    }

    /// On Postgres a lookup stamps `last_seen_at` and `last_used` only when
    /// the stamp is older than 60 s.
    #[tokio::test]
    async fn stamp_skipped_under_60s() {
        let Some(url) = pg_url() else { return };
        let (store, _guard) = pg_test_store(&url, "stm").await;
        store
            .add_user("ada", "Ada", None, Role::Editor, "pw12345678")
            .await
            .unwrap();
        let session = store.create_session("ada", 3600).await.unwrap();
        store.session_user(&session.token).await.unwrap().unwrap();
        let first = store.user("ada").await.unwrap().unwrap().last_seen;
        assert!(first.is_some(), "a first sighting is stamped");
        store.session_user(&session.token).await.unwrap().unwrap();
        store.mark_seen("ada").await.unwrap();
        assert_eq!(store.user("ada").await.unwrap().unwrap().last_seen, first);
        let old = (chrono::Utc::now() - chrono::Duration::seconds(61)).to_rfc3339();
        store
            .db
            .execute(
                "UPDATE users SET last_seen_at = ?1 WHERE name = 'ada'",
                vec![Value::Text(old.clone())],
            )
            .await
            .unwrap();
        store.session_user(&session.token).await.unwrap().unwrap();
        let moved = store.user("ada").await.unwrap().unwrap().last_seen.unwrap();
        assert!(moved > old, "{moved} after {old}");

        let issued = store.issue_mcp_token("ada", "agent").await.unwrap();
        store.mcp_token_user(&issued.token).await.unwrap().unwrap();
        let used = store.list_mcp_tokens("ada").await.unwrap()[0]
            .last_used
            .clone();
        assert!(used.is_some());
        store.mcp_token_user(&issued.token).await.unwrap().unwrap();
        assert_eq!(
            store.list_mcp_tokens("ada").await.unwrap()[0].last_used,
            used
        );
    }

    /// The write lock is per schema: a transaction held open in one schema
    /// does not hold up a writer in another, and does hold up a second
    /// writer in its own.
    #[tokio::test]
    async fn auth_write_lock_is_schema_mixed() {
        let Some(url) = pg_url() else { return };
        let (a, guard_a) = pg_test_store(&url, "wla").await;
        let (b, _guard_b) = pg_test_store(&url, "wlb").await;
        let a2 = guard_a.second().await;
        a.db.begin_immediate().await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), b.db.begin_immediate())
            .await
            .expect("another schema's writer is not held up")
            .unwrap();
        b.db.finish(Ok(())).await.unwrap();
        let same = tokio::spawn(async move {
            a2.db.begin_immediate().await.unwrap();
            a2.db.finish(Ok(())).await.unwrap();
        });
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert!(!same.is_finished(), "the same schema's writer waits");
        a.db.finish(Ok(())).await.unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(5), same)
            .await
            .expect("the waiting writer goes ahead once the lock is released")
            .unwrap();
    }

    /// A lost connection is replaced on the next statement, but never inside
    /// a transaction: that one fails and stays failed until it is finished.
    #[tokio::test]
    async fn a_lost_connection_reconnects_but_never_inside_a_transaction() {
        let Some(url) = pg_url() else { return };
        let (store, guard) = pg_test_store(&url, "rec").await;
        let killer = guard.second().await;
        let pid = |row: Option<AuthRow>| match row.unwrap().get_value(0).unwrap() {
            Value::Integer(n) => n,
            other => panic!("{other:?}"),
        };
        let kill = async |pid: i64| {
            killer
                .db
                .query_first(
                    "SELECT pg_terminate_backend(CAST(?1 AS INTEGER))",
                    vec![Value::Integer(pid)],
                )
                .await
                .unwrap();
        };
        let first = pid(store
            .db
            .query_first("SELECT pg_backend_pid()", vec![])
            .await
            .unwrap());
        kill(first).await;
        // The first statement after the kill may fail; the one after it
        // runs on a new connection.
        let _ = store.db.query_first("SELECT 1", vec![]).await;
        let second = pid(store
            .db
            .query_first("SELECT pg_backend_pid()", vec![])
            .await
            .unwrap());
        assert_ne!(first, second);

        store.db.begin_immediate().await.unwrap();
        kill(second).await;
        assert!(store.db.query_first("SELECT 1", vec![]).await.is_err());
        let err = store
            .db
            .query_first("SELECT 1", vec![])
            .await
            .err()
            .unwrap();
        assert!(
            format!("{err:#}").contains("inside a transaction"),
            "{err:#}"
        );
        assert!(store.db.finish(Ok(())).await.is_err());
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        assert_eq!(store.user_count().await.unwrap(), 1);
    }

    /// An opener that may not migrate is told whether the accounts tables
    /// are behind or ahead of this binary; one that may, migrates, and is
    /// refused by tables a newer binary wrote.
    #[tokio::test]
    async fn no_migrate_open_reports_behind_and_too_new() {
        use crystalline_core::config::AccountsLocation;
        use crystalline_index::postgres::PgSessionOptions;
        let Some(url) = pg_url() else { return };
        let scratch = crate::auth_store::pg::AuthScratch::new(&url, "nom");
        let loc = AccountsLocation::Postgres(scratch.config());
        let opts = PgSessionOptions::default();
        match AuthStore::open_at_no_migrate(&loc, &opts).await {
            Err(AuthOpenError::Behind { found: 0, need: 1 }) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("an unmigrated schema opened"),
        }
        let store = AuthStore::open_at(&loc, &opts).await.unwrap();
        AuthStore::open_at_no_migrate(&loc, &opts).await.unwrap();
        store
            .db
            .execute(
                "INSERT INTO auth_schema_migration (version, applied_at) VALUES (99, 'later')",
                vec![],
            )
            .await
            .unwrap();
        match AuthStore::open_at_no_migrate(&loc, &opts).await {
            Err(AuthOpenError::TooNew {
                found: 99,
                known: 1,
            }) => {}
            Err(other) => panic!("{other}"),
            Ok(_) => panic!("a newer schema opened"),
        }
        let err = AuthStore::open_at(&loc, &opts).await.err().unwrap();
        assert!(
            format!("{err:#}").contains("newer than this binary"),
            "{err:#}"
        );
        assert!(scratch.name().starts_with("nom_"));
    }

    /// The orphan prune runs when a store opens: a session whose account
    /// vanished is gone once another store opens the schema.
    #[tokio::test]
    async fn the_orphan_prune_runs_at_open() {
        let Some(url) = pg_url() else { return };
        let (store, guard) = pg_test_store(&url, "opo").await;
        store
            .add_user("ada", "Ada", None, Role::Viewer, "pw12345678")
            .await
            .unwrap();
        store.create_session("ada", 3600).await.unwrap();
        store
            .db
            .execute("DELETE FROM users WHERE name = 'ada'", vec![])
            .await
            .unwrap();
        let count = async |s: &AuthStore| match s
            .db
            .query_first("SELECT COUNT(*) FROM sessions", vec![])
            .await
            .unwrap()
            .unwrap()
            .get_value(0)
            .unwrap()
        {
            Value::Integer(n) => n,
            other => panic!("{other:?}"),
        };
        assert_eq!(count(&store).await, 1);
        let other = guard.second().await;
        assert_eq!(count(&other).await, 0);
    }
}
