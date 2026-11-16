//! The persisted configuration: where the truth of the settings and the domain
//! registry lives and how a save reaches it.
//!
//! [`PersistedConfig::File`] is today's behaviour, `config.yaml` written with
//! [`save_yaml`]. [`PersistedConfig::Document`] keeps the configuration in a
//! [`ConfigDocument`] (one JSONB row in Postgres) on top of an optional,
//! read-only `config.yaml` base. This module is pure: it holds the merge of the
//! two, the refusal that keeps a base-only domain in the file that owns it and
//! the rule that the bootstrap `database` block is never persisted. Nothing
//! here reads or writes a database on its own.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crystalline_core::config::{GlobalConfig, save_yaml};

use crate::engine::{EngineError, Result};

/// How long a save to the document may take before the caller gets an error
/// and memory stays as it was.
pub const CONFIG_WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// The key of the bootstrap block that decides the mode and so never lives in
/// the document.
const DATABASE_KEY: &str = "database";

/// The store of the configuration document: one JSON body, replaced whole.
pub trait ConfigDocument: Send + Sync {
    /// The stored document, `None` when there is none yet.
    fn load(&self) -> Result<Option<serde_json::Value>>;

    /// Replace the stored document with `body`, saved by `by`. An error leaves
    /// the stored document as it was, except
    /// [`EngineError::ConfigMaybeStored`]: the outcome is unknown, so the
    /// caller reads the document again.
    fn save(&self, body: &serde_json::Value, by: &str) -> Result<()>;
}

/// Where a configuration is persisted.
#[derive(Clone)]
pub enum PersistedConfig {
    /// The whole configuration is the `config.yaml` at this path.
    File(PathBuf),
    /// The configuration is a document; `base` is the read-only `config.yaml`
    /// underneath it, when there is one.
    Document {
        /// The document store.
        doc: Arc<dyn ConfigDocument>,
        /// The read-only base file.
        base: Option<PathBuf>,
    },
}

impl std::fmt::Debug for PersistedConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PersistedConfig::File(p) => f.debug_tuple("File").field(p).finish(),
            PersistedConfig::Document { base, .. } => {
                f.debug_struct("Document").field("base", base).finish()
            }
        }
    }
}

impl PersistedConfig {
    /// Persist `cfg`, saved by `caller`. The file arm writes exactly what
    /// [`save_yaml`] writes. The document arm writes the configuration as one
    /// JSON body without its `database` block. `cfg` is only read, so a failed
    /// save leaves the caller's memory as it was.
    pub fn save(&self, cfg: &GlobalConfig, caller: &str) -> Result<()> {
        match self {
            PersistedConfig::File(path) => save_yaml(path, cfg).map_err(|e| {
                EngineError::Internal(format!("failed to save config {}: {e}", path.display()))
            }),
            PersistedConfig::Document { doc, .. } => {
                let body = document_body(cfg)?;
                doc.save(&body, caller)
            }
        }
    }

    /// The read-only base file of a document, when there is one.
    pub fn base_path(&self) -> Option<&Path> {
        match self {
            PersistedConfig::File(_) => None,
            PersistedConfig::Document { base, .. } => base.as_deref(),
        }
    }
}

/// The JSON body a document save stores for `cfg`: the whole configuration
/// without its `database` block, which is never persisted.
pub fn document_body(cfg: &GlobalConfig) -> Result<serde_json::Value> {
    let mut body = serde_json::to_value(cfg)
        .map_err(|e| EngineError::Internal(format!("failed to encode the configuration: {e}")))?;
    strip_database(&mut body);
    Ok(body)
}

/// Remove the `database` block from a document body. Whether one was there.
pub fn strip_database(doc: &mut serde_json::Value) -> bool {
    doc.as_object_mut()
        .is_some_and(|map| map.remove(DATABASE_KEY).is_some())
}

/// Read a stored document body as a configuration. A `database` key found in
/// it is dropped with a warning (the bootstrap block comes from the file and
/// the environment only); the flag says whether one was dropped.
pub fn config_from_document(mut body: serde_json::Value) -> Result<(GlobalConfig, bool)> {
    let dropped = strip_database(&mut body);
    if dropped {
        tracing::warn!(
            "the stored configuration document has a 'database' block; it is ignored, the database settings come from config.yaml and the environment"
        );
    }
    let cfg = serde_json::from_value(body).map_err(|e| {
        EngineError::Internal(format!("the stored configuration document is invalid: {e}"))
    })?;
    Ok((cfg, dropped))
}

/// The effective file-level configuration: `doc` on top of `base`. Maps merge
/// key by key, scalars and sequences replace, and `domains` merge by name
/// where a document entry replaces the whole base entry (keeping the base
/// entry's position; document-only domains follow). The `database` block is
/// the base's: a document has none.
pub fn merge(base: &GlobalConfig, doc: &GlobalConfig) -> GlobalConfig {
    let mut merged: GlobalConfig = match (settings_value(base), settings_value(doc)) {
        (serde_json::Value::Object(mut b), serde_json::Value::Object(d)) => {
            merge_maps(&mut b, d);
            serde_json::from_value(serde_json::Value::Object(b))
                .expect("two valid configurations merge into a valid one")
        }
        _ => unreachable!("a configuration serializes to an object"),
    };
    merged.domains = base.domains.clone();
    for (name, entry) in &doc.domains {
        merged.domains.insert(name.clone(), entry.clone());
    }
    merged.database = base.database.clone();
    merged
}

/// `cfg` as a JSON object without the keys `merge` handles itself.
fn settings_value(cfg: &GlobalConfig) -> serde_json::Value {
    let mut v = serde_json::to_value(cfg).expect("a configuration serializes");
    if let Some(map) = v.as_object_mut() {
        map.remove("domains");
        map.remove(DATABASE_KEY);
    }
    v
}

/// Fold `over` into `into`: objects merge key by key, anything else replaces.
fn merge_maps(
    into: &mut serde_json::Map<String, serde_json::Value>,
    over: serde_json::Map<String, serde_json::Value>,
) {
    for (key, value) in over {
        match (into.get_mut(&key), value) {
            (Some(serde_json::Value::Object(a)), serde_json::Value::Object(b)) => {
                merge_maps(a, b);
            }
            (_, value) => {
                into.insert(key, value);
            }
        }
    }
}

/// Refuse removing or renaming `name` when it is a base-only domain: it comes
/// from the read-only `config.yaml` at `base_path`, which only the operator
/// edits. A domain the document also names is the document's to change (its
/// entry shadows the base one), so it passes.
pub fn refuse_base_only_domain(
    base: &GlobalConfig,
    doc: &GlobalConfig,
    name: &str,
    base_path: &Path,
) -> Result<()> {
    if base.domains.contains_key(name) && !doc.domains.contains_key(name) {
        return Err(EngineError::Refused(format!(
            "domain '{name}' comes from config.yaml at {}; edit that file",
            base_path.display()
        )));
    }
    Ok(())
}

/// What a failed side-channel save means to the engine: the unknown outcome
/// stays distinguishable, everything else is an internal error.
#[cfg(feature = "postgres")]
fn save_error(e: crystalline_index::IndexError) -> EngineError {
    match e {
        crystalline_index::IndexError::MaybeStored(text) => EngineError::ConfigMaybeStored(text),
        other => EngineError::Internal(format!("failed to save the configuration: {other}")),
    }
}

#[cfg(feature = "postgres")]
impl ConfigDocument for crystalline_index::postgres::SideChannel {
    fn load(&self) -> Result<Option<serde_json::Value>> {
        self.load_config_blocking(CONFIG_WRITE_TIMEOUT)
            .map(|doc| doc.map(|d| d.body))
            .map_err(|e| EngineError::Internal(format!("failed to read the configuration: {e}")))
    }

    fn save(&self, body: &serde_json::Value, by: &str) -> Result<()> {
        self.save_config_blocking(body, by, CONFIG_WRITE_TIMEOUT)
            .map_err(save_error)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use crystalline_core::config::{DatabaseConfig, DomainEntry, SearchConfig, ServiceConfig};

    use super::*;
    use crate::overlay::EnvOverlay;

    /// An in-memory document store that can be told to fail.
    #[derive(Default)]
    struct MemDoc {
        body: Mutex<Option<serde_json::Value>>,
        fail: Mutex<Option<EngineError>>,
    }

    impl ConfigDocument for MemDoc {
        fn load(&self) -> Result<Option<serde_json::Value>> {
            Ok(self.body.lock().unwrap().clone())
        }
        fn save(&self, body: &serde_json::Value, _by: &str) -> Result<()> {
            if let Some(e) = self.fail.lock().unwrap().take() {
                return Err(e);
            }
            *self.body.lock().unwrap() = Some(body.clone());
            Ok(())
        }
    }

    fn domain(path: &str) -> DomainEntry {
        DomainEntry {
            path: Some(PathBuf::from(path)),
            ..Default::default()
        }
    }

    fn search(salience: Option<f64>, retired: Option<f64>) -> Option<SearchConfig> {
        Some(SearchConfig {
            salience_weight: salience,
            retired_weight: retired,
        })
    }

    #[test]
    fn precedence_flags_env_document_base_defaults() {
        let base = GlobalConfig {
            search: search(Some(0.1), Some(0.1)),
            service: Some(ServiceConfig {
                ui: Some(false),
                ..Default::default()
            }),
            ..Default::default()
        };
        let doc = GlobalConfig {
            search: search(Some(0.2), None),
            ..Default::default()
        };
        let env = EnvOverlay::from_vars([(
            "CRYSTALLINE_SEARCH_SALIENCE_WEIGHT".to_string(),
            "0.3".to_string(),
        )])
        .unwrap();
        let effective = env.apply(&merge(&base, &doc));
        let s = effective.search.unwrap();
        // The environment beats the document, which beats the base.
        assert_eq!(s.salience_weight, Some(0.3));
        // A key only the base has comes through underneath the document.
        assert_eq!(s.retired_weight, Some(0.1));
        assert_eq!(effective.service.unwrap().ui, Some(false));
        // Defaults: a key nobody sets stays absent. Flags are not part of the
        // configuration; they are applied on top by the engine.
        assert!(effective.embeddings.is_none());
        let doc_only = merge(&base, &doc);
        assert_eq!(doc_only.search.unwrap().salience_weight, Some(0.2));
    }

    #[test]
    fn domain_entry_replaces_whole_base_entry() {
        let mut base = GlobalConfig::default();
        let mut entry = domain("/base/a");
        entry.provision = Some(true);
        base.domains.insert("a".into(), entry);
        base.domains.insert("b".into(), domain("/base/b"));
        let mut doc = GlobalConfig::default();
        doc.domains.insert("a".into(), domain("/doc/a"));
        doc.domains.insert("c".into(), domain("/doc/c"));
        let m = merge(&base, &doc);
        let names: Vec<_> = m.domains.keys().cloned().collect();
        assert_eq!(names, ["a", "b", "c"]);
        // The base's `provision` did not leak into the replacing entry.
        assert_eq!(m.domains["a"], domain("/doc/a"));
        assert_eq!(m.domains["b"], domain("/base/b"));
    }

    #[test]
    fn sequences_and_scalars_replace() {
        let base = GlobalConfig {
            service: Some(ServiceConfig {
                allowed_hosts: Some(vec!["a".into(), "b".into()]),
                read_only: Some(true),
                ..Default::default()
            }),
            domains_root: Some(PathBuf::from("/base")),
            ..Default::default()
        };
        let doc = GlobalConfig {
            service: Some(ServiceConfig {
                allowed_hosts: Some(vec!["c".into()]),
                ..Default::default()
            }),
            domains_root: Some(PathBuf::from("/doc")),
            ..Default::default()
        };
        let m = merge(&base, &doc);
        let svc = m.service.unwrap();
        assert_eq!(svc.allowed_hosts, Some(vec!["c".to_string()]));
        // Maps merge: the base's scalar the document does not name stays.
        assert_eq!(svc.read_only, Some(true));
        assert_eq!(m.domains_root, Some(PathBuf::from("/doc")));
    }

    #[test]
    fn removing_base_only_domain_is_refused_with_path() {
        let mut base = GlobalConfig::default();
        base.domains.insert("x".into(), domain("/base/x"));
        base.domains.insert("y".into(), domain("/base/y"));
        let mut doc = GlobalConfig::default();
        doc.domains.insert("y".into(), domain("/doc/y"));
        doc.domains.insert("z".into(), domain("/doc/z"));
        let path = Path::new("/etc/crystalline/config.yaml");
        let err = refuse_base_only_domain(&base, &doc, "x", path).unwrap_err();
        assert!(matches!(err, EngineError::Refused(_)));
        assert_eq!(
            err.to_string(),
            "domain 'x' comes from config.yaml at /etc/crystalline/config.yaml; edit that file"
        );
        // A shadowed base domain and a document-only domain are the document's.
        refuse_base_only_domain(&base, &doc, "y", path).unwrap();
        refuse_base_only_domain(&base, &doc, "z", path).unwrap();
    }

    #[test]
    fn database_block_never_persisted() {
        let mem = Arc::new(MemDoc::default());
        let persisted = PersistedConfig::Document {
            doc: mem.clone(),
            base: None,
        };
        let cfg = GlobalConfig {
            database: Some(DatabaseConfig {
                url: Some("postgres://u:p@h/db".into()),
                ..Default::default()
            }),
            domains_root: Some(PathBuf::from("/d")),
            ..Default::default()
        };
        persisted.save(&cfg, "test").unwrap();
        let stored = mem.load().unwrap().unwrap();
        assert!(stored.get("database").is_none(), "{stored}");
        assert_eq!(stored["domains_root"], "/d");
        // Nor does the merge take one from a document.
        let m = merge(&GlobalConfig::default(), &cfg);
        assert!(m.database.is_none());
    }

    #[test]
    fn database_key_in_document_dropped_with_warning() {
        let body = serde_json::json!({
            "database": { "backend": "postgres", "url": "postgres://x" },
            "domains_root": "/d"
        });
        let (cfg, dropped) = config_from_document(body).unwrap();
        assert!(dropped);
        assert!(cfg.database.is_none());
        assert_eq!(cfg.domains_root, Some(PathBuf::from("/d")));
        let (_, dropped) = config_from_document(serde_json::json!({})).unwrap();
        assert!(!dropped);
        let mut v = serde_json::json!({ "database": {} });
        assert!(strip_database(&mut v));
        assert!(!strip_database(&mut v));
    }

    #[test]
    fn file_arm_writes_byte_for_byte_as_today() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = GlobalConfig {
            database: Some(DatabaseConfig {
                url: Some("/x/index.db".into()),
                ..Default::default()
            }),
            search: search(Some(1.5), None),
            ..Default::default()
        };
        cfg.domains.insert("a".into(), domain("/a"));
        cfg.domains.insert("b".into(), domain("/b"));
        let ours = dir.path().join("ours/config.yaml");
        let theirs = dir.path().join("theirs/config.yaml");
        PersistedConfig::File(ours.clone())
            .save(&cfg, "test")
            .unwrap();
        save_yaml(&theirs, &cfg).unwrap();
        assert_eq!(
            std::fs::read(&ours).unwrap(),
            std::fs::read(&theirs).unwrap()
        );
        // Unlike the document arm, the file keeps its database block.
        assert!(
            std::fs::read_to_string(&ours)
                .unwrap()
                .contains("database:")
        );
    }

    #[test]
    fn failed_document_save_leaves_memory_unchanged() {
        let mem = Arc::new(MemDoc::default());
        let persisted = PersistedConfig::Document {
            doc: mem.clone(),
            base: None,
        };
        let mut cfg = GlobalConfig::default();
        cfg.domains.insert("a".into(), domain("/a"));
        persisted.save(&cfg, "test").unwrap();
        let before = mem.load().unwrap();
        let memory = cfg.clone();

        *mem.fail.lock().unwrap() = Some(EngineError::Internal("down".into()));
        let mut next = cfg.clone();
        next.domains.insert("b".into(), domain("/b"));
        let err = persisted.save(&next, "test").unwrap_err();
        assert!(matches!(err, EngineError::Internal(_)));
        assert_eq!(mem.load().unwrap(), before);
        assert_eq!(cfg, memory);

        // An unknown outcome stays distinguishable.
        *mem.fail.lock().unwrap() = Some(EngineError::ConfigMaybeStored("maybe".into()));
        let err = persisted.save(&next, "test").unwrap_err();
        assert!(matches!(err, EngineError::ConfigMaybeStored(_)));
    }

    #[cfg(feature = "postgres")]
    #[test]
    fn a_maybe_stored_save_is_its_own_engine_error() {
        let e = save_error(crystalline_index::IndexError::MaybeStored("x".into()));
        assert!(matches!(e, EngineError::ConfigMaybeStored(_)));
        let e = save_error(crystalline_index::IndexError::NotHost("x".into()));
        assert!(matches!(e, EngineError::Internal(_)));
    }
}
