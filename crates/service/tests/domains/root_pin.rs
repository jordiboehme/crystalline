//! The one-time pin of the old default domains root: an install whose file
//! domains live under `~/Documents/Crystalline` and that sets no
//! `domains_root` gets `domains_root: ~/Documents/Crystalline` written once,
//! so new domains keep landing beside the old ones; every other install gets
//! the hidden `~/.crystalline`.
//!
//! Every test runs under [`ScratchStateDir`], so `~` is a scratch home and
//! nothing reaches the developer's real `~/Documents` or `~/.crystalline`.
//! Domain names are unique per test, because a plain `cargo test` run shares
//! one scratch home across the tests of this binary.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::support::ScratchStateDir;
use crystalline_core::config::root_pin::{PinRecord, config_key};
use crystalline_core::config::{DomainEntry, GlobalConfig};
use crystalline_index::TursoStore;
use crystalline_service::Engine;
use crystalline_service::engine::{ConfigureAction, EngineError, PinCause};
use tokio::sync::Mutex;

const MANIFEST: &str = "---\ntype: manifest\ntitle: kb\npermalink: manifest\ntags:\n  - manifest\nstatus: stable\nrecorded_at: 2026-11-25\n---\n\n# kb\n\n## Scope\n\n- old notes\n\n## When to Use\n\n- route here\n";

struct Fixture {
    _scratch: ScratchStateDir,
    tmp: tempfile::TempDir,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Fixture {
        let scratch = ScratchStateDir::acquire();
        let home = scratch.home().to_path_buf();
        Fixture {
            _scratch: scratch,
            tmp: tempfile::tempdir().unwrap(),
            home,
        }
    }

    fn config_path(&self) -> PathBuf {
        self.tmp.path().join("config").join("config.yaml")
    }

    fn state_dir(&self) -> PathBuf {
        self.tmp.path().join("state")
    }

    /// A file domain `name` in the old default root, with its MANIFEST.
    fn documents_domain(&self, name: &str) -> PathBuf {
        let dir = self.home.join("Documents").join("Crystalline").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("MANIFEST.md"), MANIFEST).unwrap();
        dir
    }

    fn save(&self, cfg: &GlobalConfig) {
        crystalline_core::config::save_yaml(&self.config_path(), cfg).unwrap();
    }

    fn on_disk(&self) -> GlobalConfig {
        crystalline_core::config::load_yaml(&self.config_path()).unwrap()
    }

    async fn engine(&self, cfg: GlobalConfig) -> Engine {
        let store = TursoStore::open_in_memory().await.unwrap();
        Engine::new(
            Arc::new(Mutex::new(store)),
            cfg,
            None,
            Some(self.config_path()),
        )
        .with_state_dir(self.state_dir())
    }

    fn recorded(&self) -> Option<bool> {
        let (record, problem) = PinRecord::load(&self.state_dir());
        assert!(problem.is_none(), "{problem:?}");
        let key = config_key(Some(&self.config_path()));
        record
            .checked
            .iter()
            .find(|c| c.config == key)
            .map(|c| c.pinned)
    }
}

/// A pre-0.37 configuration: one domain in the old default root (written in
/// its tilde form, as an old install may have it), no `domains_root`.
fn legacy(dir: &Path) -> GlobalConfig {
    let mut cfg = GlobalConfig::default();
    cfg.domains.insert("kb".into(), DomainEntry::file(dir));
    cfg
}

fn pinned_root(cfg: &GlobalConfig) -> Option<&Path> {
    cfg.domains_root.as_deref()
}

#[tokio::test]
async fn a_documents_install_is_pinned_once_and_saved() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("pin-once"));
    f.save(&cfg);
    let engine = f.engine(cfg).await;

    assert_eq!(
        engine.pin_legacy_domains_root(PinCause::Start).unwrap(),
        Some(true)
    );
    assert_eq!(
        pinned_root(&f.on_disk()),
        Some(Path::new("~/Documents/Crystalline"))
    );
    let text = std::fs::read_to_string(f.config_path()).unwrap();
    assert!(
        text.contains("domains_root: ~/Documents/Crystalline"),
        "{text}"
    );
    assert_eq!(f.recorded(), Some(true));
    assert_eq!(
        engine.config().domains_root(),
        f.home.join("Documents/Crystalline")
    );

    // A second check finds the record and saves nothing.
    let before = std::fs::read(f.config_path()).unwrap();
    let mtime = std::fs::metadata(f.config_path())
        .unwrap()
        .modified()
        .unwrap();
    assert_eq!(
        engine.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert_eq!(std::fs::read(f.config_path()).unwrap(), before);
    assert_eq!(
        std::fs::metadata(f.config_path())
            .unwrap()
            .modified()
            .unwrap(),
        mtime
    );
}

#[tokio::test]
async fn an_unset_after_the_pin_is_never_undone() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("pin-unset"));
    f.save(&cfg);
    let engine = f.engine(cfg).await;
    assert_eq!(
        engine.pin_legacy_domains_root(PinCause::Start).unwrap(),
        Some(true)
    );
    engine
        .configure(&ConfigureAction::Unset {
            key: "domains_root".into(),
        })
        .await
        .unwrap();
    assert_eq!(pinned_root(&f.on_disk()), None);

    // A new engine (the next start) over the same file and state folder.
    let again = f.engine(f.on_disk()).await;
    assert_eq!(
        again.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert_eq!(pinned_root(&f.on_disk()), None, "the record wins");
    assert_eq!(again.config().domains_root(), f.home.join(".crystalline"));
}

#[tokio::test]
async fn nothing_else_is_pinned() {
    // A root already set.
    let f = Fixture::new();
    let mut cfg = legacy(&f.documents_domain("pin-rooted"));
    cfg.domains_root = Some(PathBuf::from("/srv/kb"));
    f.save(&cfg);
    assert_eq!(
        f.engine(cfg)
            .await
            .pin_legacy_domains_root(PinCause::Start)
            .unwrap(),
        Some(false)
    );
    assert_eq!(pinned_root(&f.on_disk()), Some(Path::new("/srv/kb")));
    assert_eq!(f.recorded(), Some(false));

    // No domains.
    let f = Fixture::new();
    f.save(&GlobalConfig::default());
    assert_eq!(
        f.engine(GlobalConfig::default())
            .await
            .pin_legacy_domains_root(PinCause::Start)
            .unwrap(),
        Some(false)
    );
    assert_eq!(pinned_root(&f.on_disk()), None);

    // Domains elsewhere.
    let f = Fixture::new();
    let elsewhere = f.tmp.path().join("kb");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let cfg = legacy(&elsewhere);
    f.save(&cfg);
    assert_eq!(
        f.engine(cfg)
            .await
            .pin_legacy_domains_root(PinCause::Start)
            .unwrap(),
        Some(false)
    );
    assert_eq!(pinned_root(&f.on_disk()), None);
    assert_eq!(f.recorded(), Some(false));
}

#[tokio::test]
async fn a_read_only_or_configless_engine_is_skipped() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("pin-skip"));
    f.save(&cfg);
    let read_only = f.engine(cfg.clone()).await.with_read_only(true);
    assert_eq!(
        read_only.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert_eq!(pinned_root(&f.on_disk()), None);
    assert_eq!(f.recorded(), None);

    // No configuration of its own: the shape of a test or one-shot engine.
    let store = TursoStore::open_in_memory().await.unwrap();
    let configless =
        Engine::new(Arc::new(Mutex::new(store)), cfg, None, None).with_state_dir(f.state_dir());
    assert_eq!(
        configless.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert_eq!(f.recorded(), None);
}

#[tokio::test]
async fn a_config_that_does_not_parse_is_refused_and_not_recorded() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("pin-broken"));
    f.save(&cfg);
    let engine = f.engine(cfg).await;
    std::fs::write(f.config_path(), "domains: [not: a map\n").unwrap();
    assert!(engine.pin_legacy_domains_root(PinCause::Start).is_err());
    assert!(!PinRecord::path(&f.state_dir()).exists());
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_config_is_left_alone_at_start() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("pin-link"));
    let real = f.tmp.path().join("dotfiles").join("config.yaml");
    crystalline_core::config::save_yaml(&real, &cfg).unwrap();
    std::fs::create_dir_all(f.config_path().parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&real, f.config_path()).unwrap();
    let engine = f.engine(cfg).await;
    assert_eq!(
        engine.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert!(
        std::fs::symlink_metadata(f.config_path())
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(f.recorded(), None, "the next registration checks again");
}

#[tokio::test]
async fn a_new_domain_lands_beside_the_old_ones_and_pins() {
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("beside-old"));
    f.save(&cfg);
    let engine = f.engine(cfg).await;
    let added = engine
        .domain_add_local(Some("beside-new"), None)
        .await
        .unwrap();
    let root = PathBuf::from(added["root"].as_str().unwrap());
    assert_eq!(
        root,
        std::fs::canonicalize(f.home.join("Documents/Crystalline/beside-new")).unwrap()
    );
    let on_disk = f.on_disk();
    assert_eq!(
        pinned_root(&on_disk),
        Some(Path::new("~/Documents/Crystalline"))
    );
    assert!(on_disk.domains.contains_key("beside-new"));
    assert_eq!(f.recorded(), Some(true));
}

#[tokio::test]
async fn a_fresh_install_creates_the_hidden_root() {
    let f = Fixture::new();
    f.save(&GlobalConfig::default());
    let engine = f.engine(GlobalConfig::default()).await;
    let added = engine
        .domain_add_local(Some("fresh-hidden"), None)
        .await
        .unwrap();
    let root = PathBuf::from(added["root"].as_str().unwrap());
    let hidden = f.home.join(".crystalline");
    assert_eq!(
        root,
        std::fs::canonicalize(hidden.join("fresh-hidden")).unwrap()
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&hidden).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
    }
    assert_eq!(pinned_root(&f.on_disk()), None);
    assert_eq!(f.recorded(), Some(false));
}

#[tokio::test]
async fn a_first_add_into_documents_is_recorded_and_never_pinned() {
    let f = Fixture::new();
    f.save(&GlobalConfig::default());
    let engine = f.engine(GlobalConfig::default()).await;
    engine
        .domain_add_local(
            Some("first-docs"),
            Some("~/Documents/Crystalline/first-docs"),
        )
        .await
        .unwrap();
    assert_eq!(f.recorded(), Some(false));
    // The next start finds the record, though a domain now lives there.
    let again = f.engine(f.on_disk()).await;
    assert_eq!(
        again.pin_legacy_domains_root(PinCause::Start).unwrap(),
        None
    );
    assert_eq!(pinned_root(&f.on_disk()), None);
}

#[cfg(unix)]
#[tokio::test]
async fn a_due_pin_that_cannot_be_saved_refuses_the_add() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let cfg = legacy(&f.documents_domain("refused-old"));
    f.save(&cfg);
    let engine = f.engine(cfg).await;
    let folder = f.config_path().parent().unwrap().to_path_buf();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    let err = engine.domain_add_local(Some("refused-new"), None).await;
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(err.is_err(), "{err:?}");
    assert!(!f.home.join("Documents/Crystalline/refused-new").exists());
    assert!(!f.home.join(".crystalline/refused-new").exists());
    assert_eq!(pinned_root(&f.on_disk()), None);
    assert_eq!(f.recorded(), None);
}

#[tokio::test]
async fn a_file_at_the_root_refuses_the_add() {
    let f = Fixture::new();
    // A configured root behaves the same; it keeps this test off the shared
    // scratch home's `.crystalline`.
    let root = f.tmp.path().join(".crystalline");
    std::fs::write(&root, "not a folder").unwrap();
    let cfg = GlobalConfig {
        domains_root: Some(root.clone()),
        ..GlobalConfig::default()
    };
    f.save(&cfg);
    let engine = f.engine(cfg).await;
    let err = engine
        .domain_add_local(Some("on-a-file"), None)
        .await
        .unwrap_err();
    assert!(matches!(err, EngineError::Invalid(_)), "{err:?}");
    assert_eq!(
        err.to_string(),
        format!(
            "The domains root {} is a file, not a folder. Move it away, or choose another place with `crystalline config set domains_root <folder>`.",
            root.display()
        )
    );
    assert!(!f.on_disk().domains.contains_key("on-a-file"));
}
