//! A domain row's `read_only` says whether THIS caller can write there: the
//! domain lock, a viewer's role and a domain off the caller's write list all
//! mark it, and a writable row leaves the key out (spec A-5, B-6). The
//! routing block marks the same rows "(read-only)".

use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig, ResponseFormat, ServiceConfig};
use crystalline_index::TursoStore;
use crystalline_service::params::{BrowseParams, ListDomainsParams, ReadParams};
use crystalline_service::rest::{AuthStore, MemberLevel, Role};
use crystalline_service::{DomainAccess, DomainRight, Engine, Scope};
use serde_json::Value;
use tokio::sync::Mutex;

const NOTE: &str = "---\ntype: engram\ntitle: Note\npermalink: note\ntags:\n  - eng\nstatus: stable\nrecorded_at: 2026-01-01\n---\n\n# Note\n\n- [fact] a fact #eng\n";

fn manifest(name: &str, read_only: bool) -> String {
    let line = if read_only { "read_only: on\n" } else { "" };
    format!(
        "---\ntype: manifest\ntitle: {name}\npermalink: manifest\ntags:\n  - manifest\nstatus: current\nrecorded_at: 2026-01-01\n{line}---\n\n# {name}\n\n## Scope\n\n- Everything about {name}\n\n## When to Use\n\n- Route here for {name} questions\n"
    )
}

/// Shared file domains, each holding `note`, with `read_only: on` on the
/// ones `locked` names, and an accounts store holding `accounts`. Every
/// domain is shared (no account owns it), so an instance editor writes
/// every one that is not locked and a viewer writes none.
async fn instance(
    domains: &[&str],
    locked: &[&str],
    accounts: &[(&str, Role, Option<&[&str]>)],
) -> (tempfile::TempDir, Arc<Engine>) {
    instance_with(domains, locked, accounts, &[], false).await
}

/// A private domain for [`instance_with`]: its name, its owner and its
/// members with their levels.
type Private<'a> = (&'a str, &'a str, &'a [(&'a str, MemberLevel)]);

/// [`instance`], with the domains `private` names made private (owner and
/// members as given), and with `break_write_list` the accounts store's
/// write-list table is broken after the accounts are created and before the
/// resolver is installed, so every write-list read fails.
async fn instance_with(
    domains: &[&str],
    locked: &[&str],
    accounts: &[(&str, Role, Option<&[&str]>)],
    private: &[Private<'_>],
    break_write_list: bool,
) -> (tempfile::TempDir, Arc<Engine>) {
    instance_full(domains, locked, accounts, private, break_write_list, &[]).await
}

/// [`instance_with`], with the accounts `disabled` names switched off after
/// they are created.
async fn instance_full(
    domains: &[&str],
    locked: &[&str],
    accounts: &[(&str, Role, Option<&[&str]>)],
    private: &[Private<'_>],
    break_write_list: bool,
    disabled: &[&str],
) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let mut cfg = GlobalConfig::default();
    for name in domains {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("MANIFEST.md"),
            manifest(name, locked.contains(name)),
        )
        .unwrap();
        std::fs::write(dir.join("note.md"), NOTE).unwrap();
        cfg.domains.insert(name.to_string(), DomainEntry::file(dir));
    }
    cfg.service = Some(ServiceConfig {
        response_format: Some(ResponseFormat::Json),
        ..ServiceConfig::default()
    });
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(Engine::new(
        Arc::new(Mutex::new(store)),
        cfg,
        None,
        Some(config_path),
    ));
    engine.sync(None).await.unwrap();
    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role, writes) in accounts {
        auth.add_user(name, name, None, *role, "pw12345678")
            .await
            .unwrap();
        if let Some(writes) = writes {
            let patterns: Vec<String> = writes.iter().map(|w| w.to_string()).collect();
            auth.set_write_list(name, &patterns).await.unwrap();
        }
    }
    for name in disabled {
        auth.set_disabled(name, true).await.unwrap();
    }
    for (domain, owner, members) in private {
        auth.set_domain_visibility(domain, true, owner)
            .await
            .unwrap();
        for (member, level) in *members {
            auth.upsert_domain_member(domain, member, *level, None, owner)
                .await
                .unwrap();
        }
    }
    let auth = if break_write_list {
        // No store holds the file while the table is replaced.
        drop(auth);
        break_the_write_list_table(&root.join("web-auth.db")).await;
        Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap())
    } else {
        auth
    };
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
    (tmp, engine)
}

fn user(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: false,
    }
}

/// The domains `list_domains` marks for `scope`, sorted, after checking that
/// every unmarked row leaves the key out rather than saying `false`.
async fn marked(engine: &Engine, scope: &Scope) -> Vec<String> {
    let listed = engine
        .list_domains(&ListDomainsParams::default(), scope)
        .await
        .unwrap();
    let mut out = Vec::new();
    for row in listed["domains"].as_array().unwrap() {
        match row.get("read_only") {
            Some(Value::Bool(true)) => out.push(row["name"].as_str().unwrap().to_string()),
            None => {}
            other => panic!("a row says read_only only as true, never {other:?}: {row}"),
        }
    }
    out.sort();
    out
}

/// Spec A-5 asks for the cost of one rights question per domain per call:
/// `list_domains` over 100 domains for a signed-in editor, timed before
/// this task and after it. The timing is the output (a mean per call);
/// the assertions are the ones it can honestly make: all 100 rows answer
/// and exactly the two locked rows carry `read_only: true`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "timing for the plan report; run by hand before and after Task 14"]
async fn list_domains_timing_with_100_domains() {
    let names: Vec<String> = (0..100).map(|i| format!("d{i:03}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let (_tmp, engine) = instance(
        &refs,
        &["d007", "d042"],
        &[
            ("ed", Role::Editor, None),
            ("bot", Role::Editor, Some(&["d0*"][..])),
        ],
    )
    .await;
    for account in ["ed", "bot"] {
        let scope = user(account);
        engine
            .list_domains(&ListDomainsParams::default(), &scope)
            .await
            .unwrap();
        let start = std::time::Instant::now();
        for _ in 0..20 {
            engine
                .list_domains(&ListDomainsParams::default(), &scope)
                .await
                .unwrap();
        }
        eprintln!(
            "list_domains over 100 domains as {account}: {:?} per call",
            start.elapsed() / 20
        );
        let listed = engine
            .list_domains(&ListDomainsParams::default(), &scope)
            .await
            .unwrap();
        let rows = listed["domains"].as_array().unwrap();
        assert_eq!(rows.len(), 100, "every domain answers for {account}");
        let mut locked: Vec<&str> = rows
            .iter()
            .filter(|r| r.get("read_only") == Some(&serde_json::Value::Bool(true)))
            .map(|r| r["name"].as_str().unwrap())
            .collect();
        locked.sort();
        assert_eq!(
            locked,
            ["d007", "d042"],
            "exactly the locked rows carry read_only: true for {account}"
        );
    }
}

/// Who sees which rows marked. The lock marks a row for everybody; a viewer
/// writes nothing, so every row is marked for it; an editor with a write
/// list writes only what the list names; an editor without a list writes
/// every unlocked shared domain, so only the lock is marked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_row_is_marked_wherever_this_caller_cannot_write() {
    let (_tmp, engine) = instance(
        &["agents-bot", "archive", "eng"],
        &["archive"],
        &[
            ("ed", Role::Editor, None),
            ("viewer", Role::Viewer, None),
            ("bot", Role::Editor, Some(&["agents-bot"][..])),
            ("crew", Role::Editor, Some(&["agents-*"][..])),
        ],
    )
    .await;
    assert_eq!(marked(&engine, &Scope::Unrestricted).await, ["archive"]);
    assert_eq!(marked(&engine, &user("ed")).await, ["archive"]);
    assert_eq!(
        marked(&engine, &user("viewer")).await,
        ["agents-bot", "archive", "eng"]
    );
    assert_eq!(marked(&engine, &user("bot")).await, ["archive", "eng"]);
    assert_eq!(marked(&engine, &user("crew")).await, ["archive", "eng"]);
}

/// A write list whose entries name no registered domain matches nothing,
/// so every domain is off the list and every row is marked. The control is
/// an editor without a list, who sees no mark at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_account_with_only_unknown_entries_sees_every_domain_read_only() {
    let (_tmp, engine) = instance(
        &["agents-bot", "eng", "ops"],
        &[],
        &[
            ("ghost", Role::Editor, Some(&["nowhere", "gone-*"][..])),
            ("ed", Role::Editor, None),
        ],
    )
    .await;
    assert_eq!(
        marked(&engine, &user("ghost")).await,
        ["agents-bot", "eng", "ops"]
    );
    assert!(marked(&engine, &user("ed")).await.is_empty());
}

/// The MCP open tier (anonymous, `auth.mcp` off) writes through the
/// carve-out in `refuse_unwritable`, so a row is marked for it only where
/// the domain is locked. The control shows that `write_right` alone would
/// have marked the writable domain too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_open_tier_list_domains_marks_only_locked_domains() {
    let (_tmp, engine) = instance(&["archive", "eng"], &["archive"], &[]).await;
    assert!(!engine.auth_mcp());
    assert!(
        engine.write_right(&Scope::Anonymous, "eng").await.unwrap() < DomainRight::Write,
        "the control: the rights resolver alone caps the open tier"
    );
    assert_eq!(marked(&engine, &Scope::Anonymous).await, ["archive"]);
}

/// Browse, read and the routing block say the same thing as the listing,
/// for the caller they answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_read_and_the_routing_block_mark_the_callers_unwritable_domains() {
    let (_tmp, engine) = instance(
        &["agents-bot", "archive", "eng"],
        &["archive"],
        &[("bot", Role::Editor, Some(&["agents-bot"][..]))],
    )
    .await;
    let bot = user("bot");
    let browse = |domain: &str| BrowseParams {
        domain: domain.into(),
        path: None,
        depth: None,
        glob: None,
    };
    let read = |domain: &str| ReadParams {
        identifier: "note".into(),
        domain: Some(domain.into()),
        share_link: None,
        include_images: None,
    };
    for (domain, expected) in [("eng", true), ("archive", true), ("agents-bot", false)] {
        let browsed = engine.browse_domain(&browse(domain), &bot).await.unwrap();
        let read_back = engine.read_engram(&read(domain), &bot).await.unwrap();
        for (what, answer) in [("browse", &browsed), ("read", &read_back)] {
            if expected {
                assert_eq!(answer["read_only"], true, "{what} {domain}: {answer}");
            } else {
                assert!(
                    answer.get("read_only").is_none(),
                    "{what} {domain}: {answer}"
                );
            }
        }
    }
    let block = engine.routing_text_scoped(&bot).await.unwrap();
    assert!(block.contains("- archive (read-only): "), "{block}");
    assert!(block.contains("- eng (read-only): "), "{block}");
    assert!(block.contains("- agents-bot: "), "{block}");
    let model = engine.routing_model_scoped(&bot).await.unwrap();
    let flags: Vec<(String, bool)> = model
        .domains
        .iter()
        .map(|d| (d.name.clone(), d.read_only))
        .collect();
    assert_eq!(
        flags,
        [
            ("agents-bot".to_string(), false),
            ("archive".to_string(), true),
            ("eng".to_string(), true)
        ]
    );
}

/// Replace the write-list table with one whose columns are wrong, so every
/// write-list read fails and nothing else does. Sequential, like
/// `mcp_auth.rs`'s `break_the_visibility_table`: no store holds the file
/// while this runs.
async fn break_the_write_list_table(path: &std::path::Path) {
    let name = path.to_string_lossy().to_string();
    let db = match turso::Builder::new_local(&name)
        .experimental_multiprocess_wal(true)
        .build()
        .await
    {
        Ok(db) => db,
        Err(_) => turso::Builder::new_local(&name).build().await.unwrap(),
    };
    let conn = db.connect().unwrap();
    conn.execute_batch(
        "DROP TABLE account_write_domain; CREATE TABLE account_write_domain (junk TEXT);",
    )
    .await
    .unwrap();
}

/// B-1 item 7: a read whose read_only mark cannot be decided is served
/// without the mark; it does not fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mark_that_cannot_be_decided_leaves_the_read_standing() {
    let (_tmp, engine) = instance_with(
        &["eng"],
        &[],
        &[("ed", Role::Editor, None)],
        &[],
        true, // break the write-list table before the resolver is installed
    )
    .await;
    let (logs, _guard) = crate::support::capture_logs();
    let scope = user("ed");
    let listed = engine
        .list_domains(&ListDomainsParams::default(), &scope)
        .await
        .expect("the listing still answers");
    let row = &listed["domains"][0];
    assert_eq!(row["name"], "eng");
    assert!(row.get("read_only").is_none(), "{row}");
    let browsed = engine
        .browse_domain(
            &BrowseParams {
                domain: "eng".into(),
                path: None,
                depth: None,
                glob: None,
            },
            &scope,
        )
        .await
        .expect("browse still answers");
    assert!(browsed.get("read_only").is_none(), "{browsed}");
    let read = engine
        .read_engram(
            &ReadParams {
                identifier: "note".into(),
                domain: Some("eng".into()),
                share_link: None,
                include_images: None,
            },
            &scope,
        )
        .await
        .expect("the read still answers");
    assert!(read.get("read_only").is_none(), "{read}");
    assert!(
        logs.any_contains("the read_only mark could not be decided"),
        "the undecided mark is logged as a warning: {:?}",
        logs.lines()
    );
}

/// A Viewer's mark does not depend on the write list (a role below Write is
/// read-only before the list is read), so a broken write-list table must not
/// unmark it. Task 11's batched marks must give the same answer, and since
/// the batch does not read the list for a Viewer either, nothing is logged.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_stays_marked_when_the_write_list_table_is_broken() {
    let (_tmp, engine) =
        instance_with(&["eng"], &[], &[("vw", Role::Viewer, None)], &[], true).await;
    let (logs, _guard) = crate::support::capture_logs();
    let listed = engine
        .list_domains(&ListDomainsParams::default(), &user("vw"))
        .await
        .expect("the listing still answers");
    assert_eq!(listed["domains"][0]["read_only"], true, "{listed}");
    assert!(
        !logs.any_contains("could not be read in one go"),
        "no warning for a list a Viewer's answer never needs: {:?}",
        logs.lines()
    );
}

/// An instance Viewer who is an Editor member of a private domain is capped to
/// read by the role, so the write list is not read and the row stays marked
/// over a broken table.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_with_editor_membership_stays_marked_over_a_broken_write_list_table() {
    let (_tmp, engine) = instance_with(
        &["lab"],
        &[],
        &[("ed", Role::Editor, None), ("vw", Role::Viewer, None)],
        &[("lab", "ed", &[("vw", MemberLevel::Editor)][..])],
        true,
    )
    .await;
    let listed = engine
        .list_domains(&ListDomainsParams::default(), &user("vw"))
        .await
        .expect("the listing still answers");
    assert_eq!(listed["domains"][0]["read_only"], true, "{listed}");
    assert!(engine.read_only_mark(&user("vw"), "lab").await);
}

/// B-3: the batched marks are exactly the per-row write answer, for every
/// kind of caller and every domain, a private one with members included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batched_marks_equal_the_per_row_answer() {
    let (_tmp, engine) = instance_full(
        &["agents-bot", "eng", "lab", "locked"],
        &["locked"],
        &[
            ("ed", Role::Editor, None),
            ("bot", Role::Editor, Some(&["agents-*"][..])),
            ("viewer", Role::Viewer, None),
            ("boss", Role::Admin, None),
            ("member", Role::Editor, None),
            ("gone", Role::Editor, None),
        ],
        &[("lab", "ed", &[("member", MemberLevel::Viewer)][..])],
        false,
        &["gone"],
    )
    .await;
    // The anonymous scope below is the open tier: `auth.mcp` is off here.
    assert!(!engine.auth_mcp());
    let admin = Scope::User {
        account: "boss".into(),
        admin: true,
    };
    for scope in [
        user("ed"),
        user("bot"),
        user("viewer"),
        user("member"),
        user("nobody"),
        user("gone"),
        admin,
        Scope::Anonymous,
        Scope::Unrestricted,
    ] {
        let listed = engine
            .list_domains(
                &ListDomainsParams {
                    include_routing: true,
                },
                &scope,
            )
            .await
            .unwrap();
        if scope == user("gone") {
            // A disabled account still lists the domains, every one marked.
            let rows = listed["domains"].as_array().unwrap();
            assert!(
                !rows.is_empty(),
                "the disabled account lists rows: {listed}"
            );
            assert!(
                rows.iter().all(|row| row["read_only"] == true),
                "every row of a disabled account is read-only: {listed}"
            );
        }
        for row in listed["domains"].as_array().unwrap() {
            let name = row["name"].as_str().unwrap();
            // The listing seeds each lock from the MANIFEST under the root it
            // lists; the per-row answer reads the lock through the domain's
            // own content source. The two roots must be one.
            let root = std::path::Path::new(row["path"].as_str().unwrap());
            assert_eq!(
                crystalline_core::read_only_at(root).is_on(),
                engine.domain_is_read_only(name).await,
                "{name}: {row}"
            );
            let per_row = engine.write_block(&scope, name).await.unwrap().is_some();
            assert_eq!(
                row.get("read_only") == Some(&serde_json::Value::Bool(true)),
                per_row,
                "{scope:?} on {name}: {row}"
            );
        }
    }
}

/// B-3: over a broken write-list table the batched listing gives exactly
/// the per-row read answer (`read_only_mark`, which never fails), the
/// Viewer's mark included.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batched_marks_equal_the_per_row_answer_over_a_broken_write_list_table() {
    let (_tmp, engine) = instance_with(
        &["eng", "lab", "locked"],
        &["locked"],
        &[
            ("ed", Role::Editor, None),
            ("viewer", Role::Viewer, None),
            ("vedit", Role::Viewer, None),
            ("boss", Role::Admin, None),
        ],
        &[(
            "lab",
            "ed",
            &[
                ("viewer", MemberLevel::Viewer),
                ("vedit", MemberLevel::Editor),
            ][..],
        )],
        true,
    )
    .await;
    let admin = Scope::User {
        account: "boss".into(),
        admin: true,
    };
    for scope in [user("ed"), user("viewer"), user("vedit"), admin] {
        let listed = engine
            .list_domains(
                &ListDomainsParams {
                    include_routing: true,
                },
                &scope,
            )
            .await
            .unwrap();
        let rows = listed["domains"].as_array().unwrap();
        assert!(!rows.is_empty(), "{scope:?}: {listed}");
        for row in rows {
            let name = row["name"].as_str().unwrap();
            let per_row = engine.read_only_mark(&scope, name).await;
            assert_eq!(
                row.get("read_only") == Some(&serde_json::Value::Bool(true)),
                per_row,
                "{scope:?} on {name}: {row}"
            );
            if scope == user("vedit") {
                assert!(per_row, "{scope:?} on {name} stays marked: {row}");
            }
        }
    }
}
