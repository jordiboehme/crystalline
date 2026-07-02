//! Config parsing, atomic save round-trip, tilde expansion and default paths.

use crystalline_core::config::{
    self, DomainEntry, GlobalConfig, HttpSetting, load_yaml, save_yaml,
};

#[test]
fn parse_global_config_with_http_bool() {
    let yaml = "\
domains:
  gardening:
    path: ~/kb/gardening
  astronomy:
    path: /data/astronomy
service:
  http: false
embeddings:
  provider: local
  model: bge-small-en-v1.5
prompt:
  rules:
    \"~/git/product/**\":
      include:
      - product
      - gardening
";
    let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
    // Domain order is preserved.
    let names: Vec<&str> = cfg.domains.keys().map(String::as_str).collect();
    assert_eq!(names, ["gardening", "astronomy"]);
    assert_eq!(cfg.service.unwrap().http, Some(HttpSetting::Enabled(false)));
    let emb = cfg.embeddings.unwrap();
    assert_eq!(emb.provider, "local");
    let rule = cfg
        .prompt
        .unwrap()
        .rules
        .get("~/git/product/**")
        .cloned()
        .unwrap();
    assert_eq!(rule.include.unwrap(), ["product", "gardening"]);
}

#[test]
fn parse_http_as_address_string() {
    let yaml = "service:\n  http: 127.0.0.1:7411\n";
    let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(
        cfg.service.unwrap().http,
        Some(HttpSetting::Address("127.0.0.1:7411".into()))
    );
}

#[test]
fn atomic_save_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("config.yaml");

    let mut cfg = GlobalConfig::default();
    cfg.domains.insert(
        "gardening".into(),
        DomainEntry {
            path: "/data/gardening".into(),
        },
    );
    save_yaml(&path, &cfg).unwrap();
    assert!(path.exists());

    let loaded: GlobalConfig = load_yaml(&path).unwrap();
    assert_eq!(loaded, cfg);

    // No temporary file is left behind.
    let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty());
}

#[test]
fn expand_tilde_resolves_home() {
    let expanded = config::expand_tilde("~/kb/gardening");
    assert!(expanded.is_absolute());
    assert!(expanded.ends_with("kb/gardening"));
    // A non-tilde path is returned unchanged.
    assert_eq!(
        config::expand_tilde("/abs/path"),
        std::path::Path::new("/abs/path")
    );
}

#[test]
fn default_paths_are_namespaced() {
    let config = config::config_dir().unwrap();
    assert!(config.ends_with("crystalline"));
    assert!(
        config::global_config_path()
            .unwrap()
            .ends_with("crystalline/config.yaml")
    );
    assert!(
        config::index_db_path()
            .unwrap()
            .ends_with("crystalline/index.db")
    );
    assert!(
        config::service_lock_path()
            .unwrap()
            .ends_with("crystalline/service.lock")
    );
    assert!(
        config::models_dir()
            .unwrap()
            .ends_with("crystalline/models")
    );
}

#[test]
fn per_domain_config_parses_verify_overrides() {
    let yaml = "\
verify:
  rules:
    T004: error
    Q002: warn
  token_budget: 2500
  required_files:
  - path: MANIFEST.md
    require_frontmatter: true
    required_sections:
    - Scope
    - When to Use
    sections:
      When to Use:
        min_top_level_bullets: 1
        fallback_section: Scope
        max_bullet_length: 120
";
    let cfg: config::DomainConfig = serde_yaml_ng::from_str(yaml).unwrap();
    let verify = cfg.verify.unwrap();
    assert_eq!(verify.rules.get("T004").map(String::as_str), Some("error"));
    assert_eq!(verify.token_budget, Some(2500));
    let rf = &verify.required_files[0];
    assert_eq!(rf.path, "MANIFEST.md");
    assert_eq!(rf.require_frontmatter, Some(true));
    assert_eq!(rf.required_sections, ["Scope", "When to Use"]);
    let section = rf.sections.get("When to Use").unwrap();
    assert_eq!(section.min_top_level_bullets, Some(1));
    assert_eq!(section.fallback_section.as_deref(), Some("Scope"));
    assert_eq!(section.max_bullet_length, Some(120));
}

#[test]
fn repo_config_preferred_domains() {
    let yaml = "preferred_domains:\n- product\n- gardening\n";
    let cfg: config::RepoConfig = serde_yaml_ng::from_str(yaml).unwrap();
    assert_eq!(cfg.preferred_domains, ["product", "gardening"]);
}
