//! Engine-level tests for `configure` with a domain: the view of how one
//! domain behaves, and the policy and rule writes behind it.

use std::path::Path;
use std::sync::Arc;

use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::Scope;
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
    assert_eq!(keys, ["generated_indexes", "sharing", "domain_name"]);
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
    assert_eq!(view["rules"]["catalog"].as_array().unwrap().len(), 58);
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
