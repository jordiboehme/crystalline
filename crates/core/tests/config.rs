//! Config parsing, atomic save round-trip, tilde expansion and default paths.

use std::sync::Mutex;

use crystalline_core::config::{
    self, DomainEntry, GitHubConfig, GlobalConfig, HttpSetting, OriginConfig, load_yaml, save_yaml,
};

/// Guards every test that reads or mutates `CRYSTALLINE_MODELS_DIR`. Env vars
/// are process-global and cargo test runs functions from this file on
/// multiple threads, so `default_paths_are_namespaced` and
/// `models_dir_env_override` both take this lock for their duration to avoid
/// observing each other's env var state.
static MODELS_DIR_ENV_LOCK: Mutex<()> = Mutex::new(());

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
    cfg.domains
        .insert("gardening".into(), DomainEntry::file("/data/gardening"));
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
    let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
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
        config::service_info_path()
            .unwrap()
            .ends_with("crystalline/service.json")
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

#[test]
fn legacy_config_without_origin_or_github_round_trips_byte_identical() {
    // A config predating this feature (bare domain paths, no origin or github
    // blocks) must serialize back exactly as before: no origin or github keys
    // anywhere in the output.
    let mut domains = std::collections::BTreeMap::new();
    domains.insert("eng", "/knowledge/eng");
    let mut cfg = GlobalConfig::default();
    for (name, path) in domains {
        cfg.domains
            .insert(name.to_string(), DomainEntry::file(path));
    }
    let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
    assert_eq!(yaml, "domains:\n  eng:\n    path: /knowledge/eng\n");
    let back: GlobalConfig = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(back, cfg);
}

#[test]
fn domains_root_round_trips_and_is_absent_by_default() {
    // Absent by default: it never appears in a config that does not set it, and
    // the resolver falls back to the built-in Documents/Crystalline default.
    let cfg = GlobalConfig::default();
    let yaml = serde_yaml_ng::to_string(&cfg).unwrap();
    assert!(!yaml.contains("domains_root"), "{yaml}");
    assert!(
        cfg.domains_root()
            .display()
            .to_string()
            .ends_with("Documents/Crystalline"),
        "{}",
        cfg.domains_root().display()
    );

    // Set: serializes as one line and round-trips.
    let with_root = GlobalConfig {
        domains_root: Some(std::path::PathBuf::from("/srv/knowledge")),
        ..GlobalConfig::default()
    };
    let yaml = serde_yaml_ng::to_string(&with_root).unwrap();
    assert!(yaml.contains("domains_root: /srv/knowledge\n"), "{yaml}");
    let back: GlobalConfig = serde_yaml_ng::from_str(&yaml).unwrap();
    assert_eq!(back, with_root);
    assert_eq!(back.domains_root().display().to_string(), "/srv/knowledge");
}

#[test]
fn origin_and_github_config_round_trip() {
    let yaml = "\
domains:
  brand:
    path: ~/Documents/Crystalline/brand
    origin:
      repo: acme/brand-knowledge
      path: knowledge
      branch: main
      poll_secs: 600
github:
  enabled: true
  poll_secs: 300
  api_url: https://github.example.com/api/v3
  oauth_client_id: abc123
";
    let cfg: GlobalConfig = serde_yaml_ng::from_str(yaml).unwrap();

    let entry = cfg.domains.get("brand").unwrap();
    let origin = entry.origin.as_ref().expect("origin block");
    assert_eq!(origin.repo, "acme/brand-knowledge");
    assert_eq!(origin.path.as_deref(), Some("knowledge"));
    assert_eq!(origin.branch.as_deref(), Some("main"));
    assert_eq!(origin.poll_secs, Some(600));

    let gh = cfg.github.as_ref().expect("github block");
    assert_eq!(gh.enabled, Some(true));
    assert_eq!(gh.poll_secs, Some(300));
    assert_eq!(
        gh.api_url.as_deref(),
        Some("https://github.example.com/api/v3")
    );
    assert_eq!(gh.oauth_client_id.as_deref(), Some("abc123"));

    // Re-serializing and re-parsing reaches a fixed point.
    let re_yaml = serde_yaml_ng::to_string(&cfg).unwrap();
    let back: GlobalConfig = serde_yaml_ng::from_str(&re_yaml).unwrap();
    assert_eq!(back, cfg);
}

#[test]
fn github_enabled_defaults_false_and_reflects_explicit_value() {
    let cfg = GlobalConfig::default();
    assert!(!cfg.github_enabled());

    let enabled_cfg = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(true),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    assert!(enabled_cfg.github_enabled());

    let explicit_false = GlobalConfig {
        github: Some(GitHubConfig {
            enabled: Some(false),
            ..GitHubConfig::default()
        }),
        ..GlobalConfig::default()
    };
    assert!(!explicit_false.github_enabled());
}

#[test]
fn origin_branch_defaults_to_main_when_absent() {
    let origin = OriginConfig {
        repo: "acme/brand-knowledge".to_string(),
        path: None,
        branch: None,
        poll_secs: None,
    };
    assert_eq!(origin.branch(), "main");

    let origin_explicit = OriginConfig {
        branch: Some("develop".to_string()),
        ..origin
    };
    assert_eq!(origin_explicit.branch(), "develop");
}

#[test]
fn origin_state_paths_are_namespaced_under_the_state_dir() {
    assert!(
        config::origins_state_dir()
            .unwrap()
            .ends_with("crystalline/origins")
    );
    assert!(
        config::origin_state_dir("brand")
            .unwrap()
            .ends_with("crystalline/origins/brand")
    );
}

#[test]
fn models_dir_env_override() {
    let _guard = MODELS_DIR_ENV_LOCK.lock().unwrap();
    // Preserve and restore whatever the surrounding environment already had,
    // so this test leaves no state behind for anything that runs after it.
    let previous = std::env::var("CRYSTALLINE_MODELS_DIR").ok();

    // Unset: falls back to the default cache-dir-based path.
    unsafe {
        std::env::remove_var("CRYSTALLINE_MODELS_DIR");
    }
    let default = config::models_dir().unwrap();
    assert!(default.ends_with("crystalline/models"));

    // Empty: treated the same as unset, not as a literal empty path.
    unsafe {
        std::env::set_var("CRYSTALLINE_MODELS_DIR", "");
    }
    let empty = config::models_dir().unwrap();
    assert_eq!(empty, default);

    // Set to an absolute path: used verbatim, taking priority over the default.
    let dir = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("CRYSTALLINE_MODELS_DIR", dir.path());
    }
    let overridden = config::models_dir().unwrap();
    assert_eq!(overridden, dir.path());

    // Set with a leading tilde: expanded via the same helper as other paths.
    unsafe {
        std::env::set_var("CRYSTALLINE_MODELS_DIR", "~/kb/models-override");
    }
    let expanded = config::models_dir().unwrap();
    assert!(expanded.is_absolute());
    assert!(expanded.ends_with("kb/models-override"));

    match previous {
        Some(v) => unsafe { std::env::set_var("CRYSTALLINE_MODELS_DIR", v) },
        None => unsafe { std::env::remove_var("CRYSTALLINE_MODELS_DIR") },
    }
}
