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

use anyhow::{Result, anyhow, bail};
use crystalline_core::config::{
    self, DomainEntry, GlobalConfig, NameOrigin,
    registration::{
        Registration, RegistrationRequest, canonical_root, case_twin, case_twin_refusal,
        choose_domain_name, decide_registration, derive_domain_name, fold_domain_name,
        needs_manifest_write_back, validate_domain_name,
    },
};
use crystalline_core::names::{NameInput, NameTable};
use serde::{Deserialize, Serialize};

use crate::overlay::LoadedConfig;

/// What a registration did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    /// The name the domain is registered under.
    pub name: String,
    /// A file domain's canonical root; `None` for a virtual domain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<PathBuf>,
    /// Whether an existing registration was adopted rather than written.
    pub adopted: bool,
    /// The MANIFEST-declared name this registration could not take, when a
    /// derived name stands in for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shadowed: Option<String>,
    /// A virtual domain's MANIFEST, to scaffold into the database.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Lines for the person, which the CLI prints to stderr: a MANIFEST name
    /// write-back that was skipped or failed, kept texts that could not be
    /// set aside.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// Register the local file domain `name` at `path` (or, without a path, at
/// `<domains_root>/<name>`), or without a name the folder at `path` under a
/// name its MANIFEST declares or its basename gives. The folder must hold a
/// `MANIFEST.md` already. `kept_root` is this machine's `<state>/kept`
/// folder: kept texts no registered domain owns are set aside there before
/// the name is taken.
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
) -> Result<Registered> {
    // Mutate the file truth and save it back to the resolved path; the
    // environment overlay is never written.
    let loaded = crate::overlay::load(config_override)?;
    let mut notes = Vec::new();

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
            let existing = loaded.file.domains.get(name).cloned();
            if existing.is_none() {
                validate_domain_name(name)
                    .map_err(|e| anyhow!("{e}, or point at a folder with --path <dir>"))?;
            }

            let root = match path {
                Some(p) => p.to_path_buf(),
                None => {
                    crate::origin::default_domain_folder(&loaded.effective.domains_root(), name)
                }
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
                    if let Some(twin) = case_twin(name, None, loaded.file.domains.iter()) {
                        bail!("{}", case_twin_refusal(name, &twin));
                    }
                    notes.extend(set_aside_orphan(
                        kept_root,
                        name,
                        loaded.file.domains.keys(),
                    ));
                    let entry =
                        DomainEntry::file(abs.clone()).with_name_origin(NameOrigin::Explicit);
                    let mut cfg = loaded.file;
                    cfg.domains.insert(name.to_string(), entry.clone());
                    config::save_yaml(&loaded.path, &cfg).map_err(|e| {
                        anyhow!("failed to save config {}: {e}", loaded.path.display())
                    })?;
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

            match existing_file_domain_at(&abs, &loaded.file) {
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
                            || case_twin(c, None, loaded.file.domains.iter()).is_some()
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
                        loaded.file.domains.keys(),
                    ));
                    let entry = DomainEntry::file(abs.clone()).with_name_origin(choice.origin);
                    let mut cfg = loaded.file;
                    cfg.domains.insert(choice.name.clone(), entry.clone());
                    config::save_yaml(&loaded.path, &cfg).map_err(|e| {
                        anyhow!("failed to save config {}: {e}", loaded.path.display())
                    })?;
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
) -> Result<Registered> {
    let loaded = crate::overlay::load(config_override)?;
    if let Some(env) = loaded.overlay.env_domain(name) {
        bail!(
            "domain '{name}' is defined by the environment variable {}; unset it to manage this domain in the config file",
            env.var
        );
    }
    let adopted = match decide_registration(
        name,
        loaded.file.domains.get(name),
        &RegistrationRequest::Virtual,
    ) {
        Registration::Adopt => true,
        Registration::Conflict(message) => bail!("{message}"),
        Registration::Register => {
            validate_domain_name(name).map_err(|e| anyhow!(e))?;
            if let Some(twin) = case_twin(name, None, loaded.file.domains.iter()) {
                bail!("{}", case_twin_refusal(name, &twin));
            }
            false
        }
    };
    let mut notes = Vec::new();
    if !adopted {
        notes.extend(set_aside_orphan(
            kept_root,
            name,
            loaded.file.domains.keys(),
        ));
        let mut cfg = loaded.file;
        cfg.domains.insert(
            name.to_string(),
            DomainEntry::virtual_domain().with_name_origin(NameOrigin::Explicit),
        );
        config::save_yaml(&loaded.path, &cfg)
            .map_err(|e| anyhow!("failed to save config {}: {e}", loaded.path.display()))?;
    }
    let today = chrono::Utc::now()
        .date_naive()
        .format("%Y-%m-%d")
        .to_string();
    Ok(Registered {
        name: name.to_string(),
        root: None,
        adopted,
        shadowed: None,
        manifest: Some(crystalline_core::manifest_template(name, &today)),
        notes,
    })
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
    crystalline_core::kept::set_aside(root, name, chrono::Utc::now())
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
