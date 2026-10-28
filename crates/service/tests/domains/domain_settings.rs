//! Engine-level tests for `configure` with a domain: the view of how one
//! domain behaves, and the policy and rule writes behind it.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{AuthConfig, DomainEntry, GlobalConfig, ReviewMode};
use crystalline_index::TursoStore;
use crystalline_service::engine::PolicyEdit;
use crystalline_service::params::DeleteParams;
use crystalline_service::rest::{AuthStore, Role};
use crystalline_service::{DomainAccess, Engine, EngineError, Scope};
use serde_json::{Value, json};
use tokio::sync::Mutex;

/// Every policy key and every section declared.
pub const EVERY_SECTION: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\ngenerated_indexes: shared\nsharing: direct\ndomain_name: engineering\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n\n## Provisioning\n\n- skills: skills\n\n## Tag Aliases\n\n- k8s -> kubernetes\n";

/// An engine with a file domain `eng` holding `manifest`, a virtual domain
/// `notes` with no MANIFEST yet, and whatever `tweak` adds. Everything lives
/// in the temp dir: config, token store, state.
pub async fn settings_engine(
    manifest: &str,
    tweak: impl FnOnce(&mut GlobalConfig, &Path),
) -> (tempfile::TempDir, Arc<Engine>) {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let eng = root.join("eng");
    std::fs::create_dir_all(&eng).unwrap();
    std::fs::write(eng.join("MANIFEST.md"), manifest).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(eng));
    cfg.domains
        .insert("notes".to_string(), DomainEntry::virtual_domain());
    tweak(&mut cfg, &root);
    let config_path = root.join("config.yaml");
    crystalline_core::config::save_yaml(&config_path, &cfg).unwrap();
    std::fs::create_dir_all(root.join("tokens")).unwrap();
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Arc::new(
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, Some(config_path))
            .with_token_store_dir(root.join("tokens"))
            .with_state_dir(root.join("state")),
    );
    engine.sync(None).await.unwrap();
    (tmp, engine)
}

fn row<'a>(rows: &'a Value, key: &str, value: &str) -> &'a Value {
    rows.as_array()
        .unwrap()
        .iter()
        .find(|r| r[key] == value)
        .unwrap_or_else(|| panic!("no row with {key} {value}: {rows}"))
}

#[tokio::test]
async fn the_view_of_a_file_domain_names_every_policy_section_and_part() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(view["domain"], "eng");
    assert_eq!(view["manifest"], "present");
    let keys: Vec<&str> = view["policies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["key"].as_str().unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "generated_indexes",
            "sharing",
            "domain_name",
            "read_only",
            "reflection",
            "reflection_home",
            "reflect_trigger",
            "evolve_trigger",
            "team"
        ]
    );
    let team = row(&view["policies"], "key", "team");
    assert_eq!(team["can_change"], false);
    assert!(
        team["how"]
            .as_str()
            .unwrap()
            .contains("written when the domain is connected to GitHub"),
        "{team}"
    );
    let home = row(&view["policies"], "key", "reflection_home");
    assert_eq!(home["can_change"], false);
    assert!(
        home["how"]
            .as_str()
            .unwrap()
            .contains("filled in when Reflection is switched on"),
        "{home}"
    );
    assert_eq!(
        row(&view["policies"], "key", "reflection")["can_change"],
        true
    );
    let sharing = row(&view["policies"], "key", "sharing");
    assert_eq!(sharing["declared"], "direct");
    assert_eq!(sharing["can_change"], true);
    assert_eq!(sharing["how"], "set with configure");
    let name = row(&view["policies"], "key", "domain_name");
    assert_eq!(name["can_change"], false);
    assert!(
        name["how"]
            .as_str()
            .unwrap()
            .contains("crystalline domain rename eng <new>"),
        "{name}"
    );
    let names: Vec<&str> = view["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["When to Use", "Scope", "Provisioning", "Tag Aliases"]
    );
    assert_eq!(
        row(&view["sections"], "name", "Scope")["how"],
        "edit_engram on the MANIFEST (section ## Scope)"
    );
    assert_eq!(view["routing"], "when_to_use");
    assert_eq!(view["missing"], json!([]));
    assert_eq!(view["rules"]["available"], true);
    assert_eq!(view["rules"]["file_present"], false);
    assert_eq!(view["rules"]["overrides"], json!([]));
    assert_eq!(view["rules"]["catalog"].as_array().unwrap().len(), 64);
    assert_eq!(view["how"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn the_rules_part_reads_the_file_as_written() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    std::fs::write(
        tmp.path().join("eng/.crystalline.yaml"),
        "verify:\n  rules:\n    E007: off\n    e008: off\n    X999: off\n    V105: off\n    T001: warnig\n  token_budget: 4000\n  token_budgets:\n    notes/a.md: 900\n",
    )
    .unwrap();
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    let rules = &view["rules"];
    assert_eq!(rules["file_present"], true);
    let e007 = row(&rules["overrides"], "rule", "E007");
    assert_eq!(e007["severity"], "off");
    assert_eq!(e007["default"], "warning");
    assert_eq!(e007["meaning"], "Every tag is lowercase-with-hyphens.");
    assert!(
        row(&rules["overrides"], "rule", "e008")["meaning"]
            .as_str()
            .unwrap()
            .contains("lower case"),
        "{rules}"
    );
    assert!(
        row(&rules["overrides"], "rule", "X999")["meaning"]
            .as_str()
            .unwrap()
            .contains("Not a verify rule id"),
        "{rules}"
    );
    assert!(
        row(&rules["overrides"], "rule", "V105")["meaning"]
            .as_str()
            .unwrap()
            .contains("Not a verify rule id"),
        "an evolve rule is no verify rule: {rules}"
    );
    assert_eq!(rules["token_budget"], 4000);
    assert_eq!(rules["token_budgets"]["notes/a.md"], 900);
    assert_eq!(rules["problems"].as_array().unwrap().len(), 1, "{rules}");
    assert!(rules["problems"][0].as_str().unwrap().contains("T001"));
}

/// The team marker is the engine's to write, never a person's: the policy
/// surfaces refuse it the way they refuse `reflection_home`.
#[tokio::test]
async fn the_team_marker_is_never_set_by_hand() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let err = engine
        .set_manifest_policies(
            "eng",
            &[("team".into(), "github".into())],
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("written when the domain is connected to GitHub"),
        "{err}"
    );
}

#[tokio::test]
async fn a_comment_only_file_is_present_with_no_overrides_and_no_problem() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    std::fs::write(tmp.path().join("eng/.crystalline.yaml"), "# tuned later\n").unwrap();
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(view["rules"]["file_present"], true);
    assert_eq!(view["rules"]["overrides"], json!([]));
    assert_eq!(view["rules"]["problems"], json!([]));
}

#[tokio::test]
async fn a_file_that_does_not_parse_is_one_problem() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    std::fs::write(
        tmp.path().join("eng/.crystalline.yaml"),
        "verify: [unclosed\n",
    )
    .unwrap();
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert!(
        view["rules"]["problems"][0]
            .as_str()
            .unwrap()
            .contains("does not parse")
    );
}

#[tokio::test]
async fn a_virtual_domain_has_no_rules_and_its_policies_are_the_defaults() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let view = engine
        .domain_settings("notes", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(view["manifest"], "missing");
    assert_eq!(
        view["rules"],
        json!({ "available": false, "note": crystalline_service::engine::VIRTUAL_NO_RULES })
    );
    assert_eq!(
        row(&view["policies"], "key", "sharing")["effective"],
        "proposal"
    );
    assert_eq!(
        row(&view["policies"], "key", "domain_name")["effective"],
        "notes"
    );
}

#[tokio::test]
async fn an_unknown_domain_is_the_unknown_domain_error() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let err = engine
        .domain_settings("nope", &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::UnknownDomain { .. }),
        "{err}"
    );
}

#[tokio::test]
async fn nobody_may_change_anything_on_a_read_only_instance() {
    let tmp = tempfile::tempdir().unwrap();
    let eng = tmp.path().join("eng");
    std::fs::create_dir_all(&eng).unwrap();
    std::fs::write(eng.join("MANIFEST.md"), EVERY_SECTION).unwrap();
    let mut cfg = GlobalConfig::default();
    cfg.domains
        .insert("eng".to_string(), DomainEntry::file(eng));
    let store = TursoStore::open_in_memory().await.unwrap();
    let engine = Engine::new(Arc::new(Mutex::new(store)), cfg, None, None)
        .with_token_store_dir(tmp.path().join("tokens"))
        .with_state_dir(tmp.path().join("state"))
        .with_read_only(true);
    engine.sync(None).await.unwrap();
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        row(&view["policies"], "key", "sharing")["can_change"],
        false
    );
}

fn unset(key: &str) -> PolicyEdit {
    PolicyEdit::Unset {
        key: key.to_string(),
    }
}

fn set(key: &str, value: &str) -> PolicyEdit {
    PolicyEdit::Set {
        key: key.to_string(),
        value: value.to_string(),
    }
}

#[tokio::test]
async fn unset_removes_the_declared_key_so_the_default_applies() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let out = engine
        .edit_manifest_policies("eng", &[unset("sharing")], &Scope::Unrestricted)
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    assert!(!on_disk.contains("sharing:"), "{on_disk}");
    assert!(
        on_disk.contains("generated_indexes: shared"),
        "the other keys stay"
    );
    assert_eq!(out["draft"], false);
    let view = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        row(&view["policies"], "key", "sharing")["effective"],
        "proposal"
    );
    assert_eq!(
        row(&view["policies"], "key", "sharing")["declared"],
        Value::Null
    );
}

#[tokio::test]
async fn unset_refuses_the_rename_key_and_an_unknown_key() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    let renamed = engine
        .edit_manifest_policies("eng", &[unset("domain_name")], &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        renamed
            .to_string()
            .contains("crystalline domain rename eng <new>"),
        "{renamed}"
    );
    let unknown = engine
        .edit_manifest_policies("eng", &[unset("zzz")], &Scope::Unrestricted)
        .await
        .unwrap_err();
    assert!(
        unknown.to_string().contains("is not a MANIFEST policy"),
        "{unknown}"
    );
}

#[tokio::test]
async fn the_open_tier_sets_a_policy_where_edit_engram_writes() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    assert!(!engine.auth_mcp());
    engine
        .edit_manifest_policies("eng", &[set("sharing", "proposal")], &Scope::Anonymous)
        .await
        .unwrap();
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    assert!(on_disk.contains("\nsharing: proposal\n"), "{on_disk}");
    let view = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(row(&view["policies"], "key", "sharing")["can_change"], true);
}

#[tokio::test]
async fn an_anonymous_caller_with_auth_mcp_on_is_refused() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |cfg, _| {
        cfg.auth = Some(AuthConfig {
            mcp: Some(true),
            ..AuthConfig::default()
        });
    })
    .await;
    let err = engine
        .edit_manifest_policies("eng", &[set("sharing", "proposal")], &Scope::Anonymous)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("only the owner of 'eng'"), "{err}");
}

#[tokio::test]
async fn the_open_tier_is_refused_in_a_reviewing_domain_as_edit_engram_is() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |cfg, _| {
        cfg.domains.get_mut("eng").unwrap().review = Some(ReviewMode::Overlay);
    })
    .await;
    let err = engine
        .edit_manifest_policies("eng", &[set("sharing", "proposal")], &Scope::Anonymous)
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains(crystalline_service::OVERLAY_NEEDS_IDENTITY),
        "{err}"
    );
    let view = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(
        row(&view["policies"], "key", "sharing")["can_change"],
        false
    );
}

/// The view reads the MANIFEST the way read_engram does: an actor in a
/// reviewing domain sees their own draft, the folder and every other reader
/// still see the domain's value.
#[tokio::test]
async fn a_draft_policy_shows_in_the_actors_view_and_not_in_the_folder() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |cfg, _| {
        cfg.domains.get_mut("eng").unwrap().review = Some(ReviewMode::Overlay);
    })
    .await;
    let out = engine
        .edit_manifest_policies("eng", &[set("sharing", "proposal")], &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(out["draft"], true, "{out}");
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    assert_eq!(on_disk, EVERY_SECTION, "the folder is unchanged");
    let mine = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(
        row(&mine["policies"], "key", "sharing")["declared"],
        "proposal"
    );
    let base = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(
        row(&base["policies"], "key", "sharing")["declared"],
        "direct"
    );
}

#[tokio::test]
async fn an_anonymous_caller_with_auth_mcp_on_may_change_nothing_in_the_view() {
    let (_tmp, engine) = settings_engine(EVERY_SECTION, |cfg, _| {
        cfg.auth = Some(AuthConfig {
            mcp: Some(true),
            ..AuthConfig::default()
        });
    })
    .await;
    let view = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(
        row(&view["policies"], "key", "sharing")["can_change"],
        false
    );
}

/// `eng` private to `keeper`, `notes` public, `ed` a signed-in editor who owns
/// neither.
async fn with_accounts(engine: &Engine, root: &Path) {
    let auth = Arc::new(AuthStore::open(&root.join("web-auth.db")).await.unwrap());
    for (name, role) in [("keeper", Role::Editor), ("ed", Role::Editor)] {
        auth.add_user(name, name, None, role, "pw12345678")
            .await
            .unwrap();
    }
    auth.set_domain_visibility("eng", true, "keeper")
        .await
        .unwrap();
    engine.set_domain_access(Arc::new(DomainAccess::new(auth)));
}

fn user(account: &str) -> Scope {
    Scope::User {
        account: account.into(),
        admin: false,
    }
}

#[tokio::test]
async fn a_signed_in_editor_who_does_not_own_the_domain_may_change_nothing() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    with_accounts(&engine, tmp.path()).await;
    let view = engine.domain_settings("notes", &user("ed")).await.unwrap();
    assert_eq!(
        row(&view["policies"], "key", "sharing")["can_change"],
        false
    );
    let owner = engine
        .domain_settings("eng", &user("keeper"))
        .await
        .unwrap();
    assert_eq!(
        row(&owner["policies"], "key", "sharing")["can_change"],
        true
    );
}

#[tokio::test]
async fn a_hidden_private_domain_is_not_found_and_never_a_view() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    with_accounts(&engine, tmp.path()).await;
    let err = engine
        .domain_settings("eng", &user("ed"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::UnknownDomain { .. }),
        "{err}"
    );
    let err = engine
        .edit_manifest_policies("eng", &[set("sharing", "proposal")], &user("ed"))
        .await
        .unwrap_err();
    assert!(
        matches!(err, crystalline_service::EngineError::UnknownDomain { .. }),
        "{err}"
    );
}

/// T5-a: an actor whose draft deletes the MANIFEST sees a domain with no
/// MANIFEST, the way read_engram answers them. The folder and every other
/// reader still see the domain's MANIFEST.
///
/// `delete_engram` refuses a file domain's MANIFEST, in a draft too (Jordi,
/// 2026-10-15), so the draft deletion is written as the row it would be: a
/// tombstone that is still there from a draft made before 0.27, or that an
/// overlay journal replay brought back, still reads this way.
#[tokio::test]
async fn a_draft_that_deletes_the_manifest_shows_it_missing_to_its_actor() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |cfg, _| {
        cfg.domains.get_mut("eng").unwrap().review = Some(ReviewMode::Overlay);
    })
    .await;
    let refused = engine
        .delete_engram_as(
            &DeleteParams {
                identifier: "manifest".to_string(),
                domain: "eng".to_string(),
                expected_checksum: None,
            },
            None,
            &Scope::Unrestricted,
        )
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("MANIFEST.md is required"),
        "{refused}"
    );
    {
        let store = engine.store();
        let store = store.lock().await;
        let id = store
            .upsert_domain(
                "eng",
                Some(&tmp.path().join("eng").to_string_lossy()),
                crystalline_index::DomainKind::File,
            )
            .await
            .unwrap();
        let mut record = crystalline_index::EngramRecord::from_engram(
            &crystalline_core::parse_engram(EVERY_SECTION).unwrap(),
            "MANIFEST.md",
            crystalline_index::FileStamp {
                mtime: 0,
                size: EVERY_SECTION.len() as u64,
                sha256: "0".repeat(64),
            },
        );
        record.content = EVERY_SECTION.to_string();
        record.tombstone = true;
        store.upsert_overlay(id, "owner", &record).await.unwrap();
    }
    let on_disk = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    assert_eq!(on_disk, EVERY_SECTION, "the folder is unchanged");

    let mine = engine
        .domain_settings("eng", &Scope::Unrestricted)
        .await
        .unwrap();
    assert_eq!(mine["manifest"], "missing", "{mine}");
    assert_eq!(
        row(&mine["policies"], "key", "sharing")["declared"],
        Value::Null,
        "the deleted MANIFEST declares nothing: {mine}"
    );

    let base = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(base["manifest"], "present");
    assert_eq!(
        row(&base["policies"], "key", "sharing")["declared"],
        "direct"
    );
}

const LOCKED: &str = "---\ntype: manifest\ntitle: eng\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-01-01\nread_only: on\n---\n\n# eng\n\n## Scope\n\n- Everything about eng\n\n## When to Use\n\n- Route here for eng questions\n";

/// The open tier sets the other owner keys where edit_engram writes, but not
/// `read_only`: switching it on is the owner's or an admin's on every tier
/// (spec A-1, D10). The configure view says so on the row.
#[tokio::test]
async fn the_open_tier_cannot_switch_read_only_on() {
    let (tmp, engine) = settings_engine(EVERY_SECTION, |_, _| {}).await;
    assert!(!engine.auth_mcp());
    let before = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    let err = engine
        .edit_manifest_policies("eng", &[set("read_only", "on")], &Scope::Anonymous)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Forbidden(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        "only the owner of 'eng' or an instance admin may change `read_only`"
    );
    let text = crystalline_service::params::EditParams {
        identifier: "manifest".into(),
        domain: "eng".into(),
        operation: "find_replace".into(),
        find_text: Some("recorded_at: 2026-01-01\n".into()),
        content: Some("recorded_at: 2026-01-01\nread_only: on\n".into()),
        ..Default::default()
    };
    let err = engine
        .edit_engram_as(&text, None, &Scope::Anonymous)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Forbidden(_)), "{err:?}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        before
    );
    let view = engine
        .domain_settings("eng", &Scope::Anonymous)
        .await
        .unwrap();
    assert_eq!(row(&view["policies"], "key", "sharing")["can_change"], true);
    assert_eq!(
        row(&view["policies"], "key", "read_only")["can_change"],
        false
    );
}

/// On a read-only domain configure takes the owner's switch-off alone and
/// nothing else, and a call that mixes the two writes nothing at all: not
/// the MANIFEST, not .crystalline.yaml (spec A-2). The switch-on alone is a
/// no-op that says so (B-1 item 2).
#[tokio::test]
async fn a_locked_domain_takes_only_the_switch_off_through_configure() {
    let (tmp, engine) = settings_engine(LOCKED, |_, _| {}).await;
    let yaml_path = tmp.path().join("eng/.crystalline.yaml");
    std::fs::write(&yaml_path, "verify:\n  rules:\n    E008: warning\n").unwrap();
    let manifest_before = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    let yaml_before = std::fs::read_to_string(&yaml_path).unwrap();
    let map = |pairs: &[(&str, &str)]| -> std::collections::BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    };
    for (set_keys, unset_keys) in [
        (map(&[("rules.E007", "off")]), vec![]),
        (map(&[("token_budget", "4000")]), vec![]),
        (map(&[("sharing", "direct")]), vec![]),
        (map(&[("read_only", "on"), ("sharing", "direct")]), vec![]),
        (map(&[("read_only", "off"), ("rules.E007", "off")]), vec![]),
        (map(&[("read_only", "off")]), vec!["rules.E008".to_string()]),
        (
            map(&[]),
            vec!["read_only".to_string(), "sharing".to_string()],
        ),
    ] {
        let err = engine
            .change_domain_settings("eng", &set_keys, &unset_keys, &Scope::Unrestricted)
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::DomainReadOnly(_)),
            "{set_keys:?} {unset_keys:?}: {err:?}"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
            manifest_before,
            "{set_keys:?} {unset_keys:?}"
        );
        assert_eq!(std::fs::read_to_string(&yaml_path).unwrap(), yaml_before);
    }
    // The switch-on alone changes nothing: it writes nothing and says so.
    let same = engine
        .change_domain_settings(
            "eng",
            &map(&[("read_only", "on")]),
            &[],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert_eq!(same["note"], "'eng' is already read-only", "{same}");
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
        manifest_before
    );
    // The switch-off alone lands, then the domain takes changes again.
    engine
        .change_domain_settings(
            "eng",
            &map(&[("read_only", "off")]),
            &[],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(!engine.domain_is_read_only("eng").await);
    engine
        .change_domain_settings(
            "eng",
            &map(&[("rules.E007", "off")]),
            &[],
            &Scope::Unrestricted,
        )
        .await
        .unwrap();
    assert!(
        std::fs::read_to_string(&yaml_path)
            .unwrap()
            .contains("E007"),
        "the rule override lands once the domain is open"
    );
}

/// The open tier cannot switch a locked domain back off either: the owner
/// gate for `read_only` in `require_policy_writer` is the engine guard, since
/// the switch-off reaches `for_unlock` and the MANIFEST already says `on`.
/// Both forms of the switch-off are refused and nothing is written.
#[tokio::test]
async fn the_open_tier_cannot_switch_read_only_off() {
    let (tmp, engine) = settings_engine(LOCKED, |_, _| {}).await;
    assert!(!engine.auth_mcp());
    let before = std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap();
    for edit in [set("read_only", "off"), unset("read_only")] {
        let err = engine
            .edit_manifest_policies("eng", std::slice::from_ref(&edit), &Scope::Anonymous)
            .await
            .unwrap_err();
        assert!(
            matches!(err, EngineError::Forbidden(_)),
            "{edit:?}: {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "only the owner of 'eng' or an instance admin may change `read_only`"
        );
        assert_eq!(
            std::fs::read_to_string(tmp.path().join("eng/MANIFEST.md")).unwrap(),
            before,
            "{edit:?}"
        );
        assert!(engine.domain_is_read_only("eng").await);
    }
}
