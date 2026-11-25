//! `crystalline domain add`'s registration of a local file domain or a
//! virtual domain, as free functions over the config file. The CLI runs
//! them in its own process, and the daemon runs the same code for the
//! `domain_register` ctl command, which a command inside Claude Desktop's
//! app package sends because it may not write the config itself (issue
//! 153). Both keep the CLI's rules: an existing `MANIFEST.md` is required,
//! a name is derived when none is given, and a conflict is refused naming
//! what holds the name.
//!
//! A path is used as it is given: the CLI makes it absolute before it asks
//! the daemon, which works in its own folder.

use std::path::{Path, PathBuf};

use crate::overlay::LoadedConfig;
use crate::persisted::PersistedConfig;
use anyhow::{Result, anyhow, bail};
use crystalline_core::config::{
    DomainEntry, GlobalConfig, NameOrigin,
    registration::{
        Registration, RegistrationRequest, canonical_root, case_twin, case_twin_refusal,
        choose_domain_name, decide_registration, derive_domain_name, fold_domain_name,
        needs_manifest_write_back, validate_domain_name,
    },
};
use crystalline_core::names::{NameInput, NameTable};

pub use crystalline_core::config::registration::Registered;

/// Register the local file domain `name` at `path` (or, without a path, at
/// `<domains_root>/<name>`), or without a name the folder at `path` under a
/// name its MANIFEST declares or its basename gives. The folder must hold a
/// `MANIFEST.md` already. `kept_root` is this machine's `<state>/kept`
/// folder: kept texts no registered domain owns are set aside there before
/// the name is taken. `state_dir` is this machine's state folder, where the
/// one-time domains root check is recorded
/// ([`crystalline_core::config::root_pin::PinRecord`]); `None` runs
/// the check without reading or writing a record.
///
/// The MANIFEST name write-back runs only for a fresh registration: an
/// adopted entry may be a team or review-mode domain, where a raw write
/// would bypass its pull or its draft. Best effort even there: a write-back
/// that fails after the registration saved becomes a note, never an error.
pub fn register_file_domain(
    name: Option<&str>,
    path: Option<&Path>,
    config_override: Option<&Path>,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
) -> Result<Registered> {
    register_file_domain_in(
        crate::overlay::load(config_override)?,
        name,
        path,
        kept_root,
        state_dir,
    )
}

/// [`register_file_domain`] over a configuration the caller loaded, with a
/// configuration document attached when it lives in the database: the
/// registration is checked against every persisted domain (the read-only
/// base's included) and saved through [`LoadedConfig::persisted`], so in
/// document mode only the document changes.
pub fn register_file_domain_in(
    loaded: LoadedConfig,
    name: Option<&str>,
    path: Option<&Path>,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
) -> Result<Registered> {
    let persisted = loaded.persisted()?;
    register_file_domain_saving(
        loaded,
        name,
        path,
        kept_root,
        state_dir,
        &mut |cfg, name| save_registration(&persisted, &cfg, name),
    )
}

/// What saves a registration: it gets the persisted truth with the new entry
/// in it and the name registered, and answers whether the save landed.
pub type SaveRegistration<'a> = &'a mut dyn FnMut(GlobalConfig, &str) -> Result<()>;

/// [`register_file_domain_in`] with the save handed in, for a caller that
/// persists the configuration its own way: the daemon's engine in document
/// mode, which saves under its own configuration lock and takes the saved
/// document into memory (`Engine::register_in_document`).
pub fn register_file_domain_saving(
    loaded: LoadedConfig,
    name: Option<&str>,
    path: Option<&Path>,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
    save: SaveRegistration<'_>,
) -> Result<Registered> {
    // Mutate the persisted truth and save it back; the environment overlay
    // is never written.
    let registered = loaded.persisted_view()?.into_owned();
    let mut notes = Vec::new();
    // Before the entry is added and before the default folder is derived.
    let pin = LegacyRootPin::check(&loaded, &registered, state_dir);
    let mut save = |mut cfg: GlobalConfig, name: &str| -> Result<()> {
        pin.apply(&mut cfg);
        save(cfg, name)?;
        pin.record();
        Ok(())
    };

    match name {
        Some(name) => {
            // An env-defined domain of the same name is refused: it is
            // managed by its variable, not the config file.
            if let Some(env) = loaded.overlay.env_domain(name) {
                bail!(
                    "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
                    env.var
                );
            }

            // The decision comes first: a name already registered to this
            // folder is adopted as it is, one registered elsewhere is
            // refused naming what holds it, and only a new name is checked
            // against the naming rules - so a name registered before the
            // rules existed can still be re-added.
            let existing = registered.domains.get(name).cloned();
            if existing.is_none() {
                validate_domain_name(name)
                    .map_err(|e| anyhow!("{e}, or point at a folder with --path <dir>"))?;
            }

            let root = match path {
                Some(p) => p.to_path_buf(),
                None => crate::origin::default_domain_folder(
                    &pin.domains_root(&loaded, &registered),
                    name,
                ),
            };

            let abs = domain_root_with_manifest(&root)?;

            match decide_registration(
                name,
                existing.as_ref(),
                &RegistrationRequest::File { root: &abs },
            ) {
                Registration::Adopt => Ok(Registered {
                    name: name.to_string(),
                    root: Some(abs),
                    adopted: true,
                    shadowed: None,
                    manifest: None,
                    notes,
                }),
                Registration::Conflict(message) => bail!("{message}"),
                Registration::Register => {
                    if let Some(twin) = case_twin(name, None, registered.domains.iter()) {
                        bail!("{}", case_twin_refusal(name, &twin));
                    }
                    notes.extend(set_aside_orphan(kept_root, name, registered.domains.keys()));
                    let entry =
                        DomainEntry::file(abs.clone()).with_name_origin(NameOrigin::Explicit);
                    let mut cfg = loaded.file;
                    cfg.domains.insert(name.to_string(), entry.clone());
                    save(cfg, name)?;
                    notes.extend(write_back_manifest_name_if_needed(&entry, &abs, name));
                    Ok(Registered {
                        name: name.to_string(),
                        root: Some(abs),
                        adopted: false,
                        shadowed: None,
                        manifest: None,
                        notes,
                    })
                }
            }
        }
        None => {
            let path =
                path.ok_or_else(|| anyhow!("name the domain, or point at its folder with --path"))?;

            let abs = domain_root_with_manifest(path)?;

            match existing_file_domain_at(&abs, &registered) {
                Some(existing_name) => Ok(Registered {
                    name: existing_name,
                    root: Some(abs),
                    adopted: true,
                    shadowed: None,
                    manifest: None,
                    notes,
                }),
                None => {
                    let table = build_name_table(&loaded);
                    let manifest_name = crystalline_core::domain_name_at(&abs);
                    let basename = abs
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| abs.display().to_string());
                    // A name folding onto a registered one is taken too: the
                    // two would share one state folder on a disk that ignores
                    // case.
                    let taken = |c: &str| {
                        table.resolve(c).is_some()
                            || case_twin(c, None, registered.domains.iter()).is_some()
                    };
                    let choice = choose_domain_name(
                        None,
                        manifest_name.as_deref(),
                        || derive_domain_name(&basename, taken),
                        taken,
                    );
                    notes.extend(set_aside_orphan(
                        kept_root,
                        &choice.name,
                        registered.domains.keys(),
                    ));
                    let entry = DomainEntry::file(abs.clone()).with_name_origin(choice.origin);
                    let mut cfg = loaded.file;
                    cfg.domains.insert(choice.name.clone(), entry.clone());
                    save(cfg, &choice.name)?;
                    notes.extend(write_back_manifest_name_if_needed(
                        &entry,
                        &abs,
                        &choice.name,
                    ));
                    Ok(Registered {
                        name: choice.name,
                        root: Some(abs),
                        adopted: false,
                        shadowed: choice.shadowed_canonical,
                        manifest: None,
                        notes,
                    })
                }
            }
        }
    }
}

/// Register a virtual domain in the global config (database-backed, no
/// path). The answer carries the MANIFEST markdown to scaffold into the
/// database and whether an existing registration of this name was adopted
/// rather than written.
///
/// A virtual domain always needs a name - there is no folder or repository
/// to derive one from - so a fresh registration is always
/// `name_origin: explicit`.
pub fn register_virtual_domain(
    name: &str,
    config_override: Option<&Path>,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
) -> Result<Registered> {
    register_virtual_domain_in(
        crate::overlay::load(config_override)?,
        name,
        kept_root,
        state_dir,
    )
}

/// [`register_virtual_domain`] over a configuration the caller loaded, as
/// [`register_file_domain_in`] is to [`register_file_domain`].
pub fn register_virtual_domain_in(
    loaded: LoadedConfig,
    name: &str,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
) -> Result<Registered> {
    let persisted = loaded.persisted()?;
    register_virtual_domain_saving(loaded, name, kept_root, state_dir, &mut |cfg, name| {
        save_registration(&persisted, &cfg, name)
    })
}

/// [`register_virtual_domain_in`] with the save handed in, as
/// [`register_file_domain_saving`] is to [`register_file_domain_in`].
pub fn register_virtual_domain_saving(
    loaded: LoadedConfig,
    name: &str,
    kept_root: Option<&Path>,
    state_dir: Option<&Path>,
    save: SaveRegistration<'_>,
) -> Result<Registered> {
    let registered = loaded.persisted_view()?.into_owned();
    let pin = LegacyRootPin::check(&loaded, &registered, state_dir);
    if let Some(env) = loaded.overlay.env_domain(name) {
        bail!(
            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
            env.var
        );
    }
    let adopted = match decide_registration(
        name,
        registered.domains.get(name),
        &RegistrationRequest::Virtual,
    ) {
        Registration::Adopt => true,
        Registration::Conflict(message) => bail!("{message}"),
        Registration::Register => {
            validate_domain_name(name).map_err(|e| anyhow!(e))?;
            if let Some(twin) = case_twin(name, None, registered.domains.iter()) {
                bail!("{}", case_twin_refusal(name, &twin));
            }
            false
        }
    };
    let mut notes = Vec::new();
    if !adopted {
        notes.extend(set_aside_orphan(kept_root, name, registered.domains.keys()));
        let mut cfg = loaded.file;
        cfg.domains.insert(
            name.to_string(),
            DomainEntry::virtual_domain().with_name_origin(NameOrigin::Explicit),
        );
        pin.apply(&mut cfg);
        save(cfg, name)?;
        pin.record();
    }
    let today = crystalline_core::time::today_utc().ymd();
    Ok(Registered {
        name: name.to_string(),
        root: None,
        adopted,
        shadowed: None,
        manifest: Some(crystalline_core::manifest_template(name, &today)),
        notes,
    })
}

/// The one-time domains root check of a registration
/// ([`crystalline_core::config::root_pin`]): decided on the persisted truth
/// before the new entry is added, applied to the configuration the
/// registration saves (so the pin lands in the same save and the same
/// layer), and recorded only once that save landed. A registration that
/// saves nothing (an adopted one) neither pins nor records.
struct LegacyRootPin {
    due: bool,
    /// Where to record the check, and under which key; `None` without a
    /// state folder or when the configuration was checked before.
    record: Option<(PathBuf, String)>,
}

impl LegacyRootPin {
    fn check(loaded: &LoadedConfig, registered: &GlobalConfig, state_dir: Option<&Path>) -> Self {
        use crystalline_core::config::root_pin::{self, PinRecord};
        let key = root_pin::config_key(match loaded.mode {
            crate::overlay::ConfigMode::File => Some(loaded.path.as_path()),
            crate::overlay::ConfigMode::Document => None,
        });
        let record = match state_dir {
            Some(dir) => {
                let (record, problem) = PinRecord::load(dir);
                if let Some(problem) = problem {
                    tracing::warn!("domains root record: {problem}");
                }
                if record.checked(&key) {
                    return LegacyRootPin {
                        due: false,
                        record: None,
                    };
                }
                Some((dir.to_path_buf(), key))
            }
            None => None,
        };
        LegacyRootPin {
            due: root_pin::is_due_here(registered),
            record,
        }
    }

    /// The root a domain without a folder lands under: the pinned one when
    /// the pin is due, else the effective one. The environment still wins.
    fn domains_root(&self, loaded: &LoadedConfig, registered: &GlobalConfig) -> PathBuf {
        if !self.due {
            return loaded.effective.domains_root();
        }
        let mut pinned = registered.clone();
        crystalline_core::config::root_pin::pin(&mut pinned);
        loaded.overlay.apply(&pinned).domains_root()
    }

    fn apply(&self, cfg: &mut GlobalConfig) {
        if self.due {
            crystalline_core::config::root_pin::pin(cfg);
        }
    }

    /// Record the check after the save landed; a failure is a warning.
    fn record(&self) {
        if self.due {
            tracing::info!("{}", crystalline_core::config::root_pin::PINNED_NOTE);
        }
        let Some((dir, key)) = &self.record else {
            return;
        };
        if let Err(e) = crystalline_core::config::root_pin::PinRecord::record(dir, key, self.due) {
            tracing::warn!(error = %e, "writing the domains root record failed");
        }
    }
}

/// Save `cfg`, the persisted truth with the registration of `name` in it.
/// When the database did not confirm the save, the document is read back:
/// a registration that landed is a plain success, so the command goes on to
/// index the domain; one that did not land, or a read back that fails too,
/// is an error that says so.
fn save_registration(persisted: &PersistedConfig, cfg: &GlobalConfig, name: &str) -> Result<()> {
    let detail = match persisted.save(cfg, "domain add") {
        Ok(()) => return Ok(()),
        Err(crate::engine::EngineError::ConfigMaybeStored(detail)) => detail,
        Err(other) => return Err(anyhow!("{other}")),
    };
    let read = match persisted {
        PersistedConfig::Document { doc, .. } => {
            crate::persisted::read_back(doc.as_ref(), cfg).map_err(|e| e.to_string())
        }
        PersistedConfig::File(_) => Err("there is no document to read back".to_string()),
    };
    match read {
        Ok((_, true)) => {
            tracing::warn!(
                "the database did not confirm saving the registration of '{name}' ({detail}); reading it back shows it was stored"
            );
            Ok(())
        }
        Ok((_, false)) => Err(anyhow!(
            "the database did not confirm saving the registration of '{name}' ({detail}); reading it back shows it was not stored, so '{name}' is not registered; try again"
        )),
        Err(why) => Err(anyhow!(
            "the database did not confirm saving the registration of '{name}' ({detail}), and reading it back failed ({why}); it may or may not be registered, so check `crystalline domain list` before trying again"
        )),
    }
}

/// Before `name` becomes a new domain's name, set aside a `kept/<name>`
/// folder no registered domain owns (see `Engine::set_aside_orphan_kept`,
/// the daemon's twin of this), so the new domain never reaches texts left
/// under the name. Best effort: a failure is the note it answers.
pub fn set_aside_orphan<'a>(
    kept_root: Option<&Path>,
    name: &str,
    registered: impl IntoIterator<Item = &'a String>,
) -> Option<String> {
    let root = kept_root?;
    if !root.join(name).is_dir() {
        return None;
    }
    let folded = fold_domain_name(name);
    if registered
        .into_iter()
        .any(|n| fold_domain_name(n) == folded)
    {
        return None;
    }
    crystalline_core::kept::set_aside(root, name, crystalline_core::time::now())
        .err()
        .map(|err| format!("note: kept texts left under '{name}' could not be set aside: {err}"))
}

/// `root` in its canonical form, once it holds a `MANIFEST.md`; refused
/// naming the command that scaffolds one otherwise.
fn domain_root_with_manifest(root: &Path) -> Result<PathBuf> {
    if !root.join("MANIFEST.md").exists() {
        bail!(
            "no MANIFEST.md at {}. Run: crystalline domain init {}",
            root.display(),
            root.display()
        );
    }
    Ok(std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()))
}

/// Declares `name` in `<root>/MANIFEST.md` when [`needs_manifest_write_back`]
/// says a fresh registration should. Answers the note for the person when
/// the write-back was skipped or failed.
fn write_back_manifest_name_if_needed(
    entry: &DomainEntry,
    root: &Path,
    name: &str,
) -> Option<String> {
    let manifest_declares = crystalline_core::domain_name_at(root).is_some();
    if !needs_manifest_write_back(entry, manifest_declares) {
        return None;
    }
    if crystalline_core::read_only_at(root).is_on() {
        return Some(format!(
            "note: '{name}' is read-only, so the name was not written into its MANIFEST."
        ));
    }
    write_back_manifest_name(root, name).err().map(|e| {
        format!("warning: writing the domain name back into the MANIFEST of '{name}' failed: {e}")
    })
}

/// The name of the file domain already rooted at `canonical`, if any: the
/// idempotency hook so pointing `domain add --path` at the same folder twice
/// adopts the existing registration - whatever name it holds - rather than
/// adding a second one over the same files.
pub fn existing_file_domain_at(canonical: &Path, cfg: &GlobalConfig) -> Option<String> {
    cfg.domains.iter().find_map(|(name, entry)| {
        (canonical_root(entry).as_deref() == Some(canonical)).then(|| name.clone())
    })
}

/// The name table a fresh registration must not collide with: every domain
/// this machine's file config and its effective (env-overlaid) config
/// currently hold, each carrying its own file domain's declared canonical
/// name - read straight off its MANIFEST, since this path has no index to
/// ask - and its recorded aliases. Mirrors the engine's own `build_table`
/// (`crates/engine/src/engine/names.rs`), so a check here and one
/// `Engine::domain_add_local` runs agree on what "taken" means.
///
/// A virtual domain contributes only its local name: its canonical name
/// lives in the database, which this path never opens.
pub fn build_name_table(loaded: &LoadedConfig) -> NameTable {
    let empty = std::collections::BTreeMap::new();
    let mut inputs: Vec<NameInput> = Vec::new();
    for cfg in [&loaded.file, &loaded.effective] {
        inputs.extend(crystalline_core::names::config_name_inputs(
            &cfg.domains,
            &empty,
        ));
    }
    NameTable::build(&inputs)
}

/// Rewrite `<root>/MANIFEST.md` to declare `domain_name: <name>`; a no-op
/// when it already reads that way.
fn write_back_manifest_name(root: &Path, name: &str) -> Result<()> {
    // A read-only domain keeps its MANIFEST as the team wrote it; the name
    // stays the registration's, exactly as the engine's write-back does.
    if crystalline_core::read_only_at(root).is_on() {
        return Ok(());
    }
    let manifest = root.join("MANIFEST.md");
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| anyhow!("reading {}: {e}", manifest.display()))?;
    // Written as LF, the whole file, like every write; compared in LF so a
    // CRLF MANIFEST that already declares the name is left alone.
    let current = crystalline_core::to_lf(&text);
    let updated =
        crystalline_core::to_lf(&crystalline_core::manifest::set_declared_name(&text, name))
            .into_owned();
    if updated != current {
        std::fs::write(&manifest, updated)
            .map_err(|e| anyhow!("writing {}: {e}", manifest.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::overlay::{ConfigMode, EnvOverlay};
    use crate::persisted::MemoryDocument;
    use crystalline_core::config;

    /// A configuration in document mode loaded from a config.yaml holding
    /// `base`, with a setting and a domain from the environment, and the
    /// document `doc` attached.
    fn loaded_in_document(
        dir: &Path,
        base: &GlobalConfig,
        doc: Arc<MemoryDocument>,
    ) -> LoadedConfig {
        let path = dir.join("config.yaml");
        config::save_yaml(&path, base).unwrap();
        let overlay = EnvOverlay::from_vars([
            (
                "CRYSTALLINE_SEARCH_SALIENCE_WEIGHT".to_string(),
                "0.3".to_string(),
            ),
            (
                "CRYSTALLINE_DOMAIN_FROM_ENV".to_string(),
                dir.display().to_string(),
            ),
        ])
        .unwrap();
        let mut loaded = LoadedConfig {
            path,
            effective: overlay.apply(base),
            file: base.clone(),
            overlay,
            mode: ConfigMode::Document,
            base: None,
            document: None,
        };
        let body = doc.stored();
        loaded.attach_document(doc, body).unwrap();
        loaded
    }

    #[test]
    fn register_virtual_domain_lands_in_document() {
        let tmp = tempfile::tempdir().unwrap();
        let mut base = GlobalConfig::default();
        base.domains
            .insert("base-only".into(), DomainEntry::virtual_domain());
        let mut held = GlobalConfig::default();
        held.domains
            .insert("doc-one".into(), DomainEntry::virtual_domain());
        let doc = Arc::new(MemoryDocument::holding(
            crate::persisted::document_body(&held).unwrap(),
        ));
        let yaml = tmp.path().join("config.yaml");

        let done = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "fresh",
            None,
            None,
        )
        .unwrap();
        assert!(!done.adopted);
        let stored = doc.stored().unwrap();
        let (cfg, _) = crate::persisted::config_from_document(stored.clone()).unwrap();
        let names: Vec<_> = cfg.domains.keys().map(String::as_str).collect();
        assert_eq!(names, ["doc-one", "fresh"], "{stored}");
        assert!(stored.get("search").is_none(), "{stored}");
        assert!(stored.get("database").is_none(), "{stored}");
        // The base file is never written.
        assert_eq!(config::load_yaml::<GlobalConfig>(&yaml).unwrap(), base);

        // A base domain is registered already: adopted, nothing written.
        let before = doc.stored();
        let done = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "base-only",
            None,
            None,
        )
        .unwrap();
        assert!(done.adopted);
        assert_eq!(doc.stored(), before);

        // An environment domain stays the variable's.
        let err = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "from-env",
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("environment variable"), "{err}");
    }

    /// A registration whose save the database did not confirm is read back:
    /// one that landed is a plain success, so `domain add` goes on to index
    /// the domain; one that did not land says so.
    #[test]
    fn an_unconfirmed_registration_is_read_back() {
        let tmp = tempfile::tempdir().unwrap();
        let base = GlobalConfig::default();
        let doc = Arc::new(MemoryDocument::default());

        *doc.fail_after_store.lock().unwrap() = Some(
            crate::engine::EngineError::ConfigMaybeStored("no answer".into()),
        );
        let done = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "landed",
            None,
            None,
        )
        .unwrap();
        assert_eq!(done.name, "landed");
        assert!(!done.adopted);

        *doc.fail.lock().unwrap() = Some(crate::engine::EngineError::ConfigMaybeStored(
            "no answer".into(),
        ));
        let err = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "lost",
            None,
            None,
        )
        .unwrap_err();
        assert!(err.to_string().contains("was not stored"), "{err}");

        *doc.fail.lock().unwrap() = Some(crate::engine::EngineError::ConfigMaybeStored(
            "no answer".into(),
        ));
        *doc.fail_load.lock().unwrap() = Some(crate::engine::EngineError::Internal("down".into()));
        let err = register_virtual_domain_in(
            loaded_in_document(tmp.path(), &base, doc.clone()),
            "unknown",
            None,
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("may or may not be registered"),
            "{err}"
        );
    }

    #[test]
    fn a_registration_in_document_mode_without_the_document_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.yaml");
        config::save_yaml(&path, &GlobalConfig::default()).unwrap();
        let loaded = LoadedConfig {
            path: path.clone(),
            file: GlobalConfig::default(),
            effective: GlobalConfig::default(),
            overlay: EnvOverlay::default(),
            mode: ConfigMode::Document,
            base: None,
            document: None,
        };
        let err = register_virtual_domain_in(loaded, "fresh", None, None).unwrap_err();
        assert!(err.to_string().contains("was not read yet"), "{err}");
        assert_eq!(
            config::load_yaml::<GlobalConfig>(&path).unwrap(),
            GlobalConfig::default(),
            "nothing written into the base"
        );
    }
}
