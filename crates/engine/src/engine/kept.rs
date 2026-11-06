//! Kept texts for the people who may handle them (the layout is in
//! [`crystalline_core::kept`]).
//!
//! A kept text holds what a person typed into a page or an engram, so it
//! goes only to somebody who may write that source: a Reflection text needs
//! `edit` on the domain's Reflection (`curate` for PACT.md), the right its
//! room needs, and an engram text needs the right to write engrams there. A
//! thread comment a refused write kept goes to its sender (matched on the
//! account recorded beside it, never on a display name) while they read the
//! Reflection, and to everybody who edits the Reflection; one with no
//! recorded sender is for editors only. Each entry is decided by its own kind, and a text
//! kept from a review draft goes to the draft's owner only. Neither right
//! asks the Reflection's state: it is often off exactly because a switch-off
//! is why the text was kept.

use crystalline_core::kept::{self, KeptKind, KeptText};

use super::*;
use crate::scope::{DomainRight, ReflectionCaller, ReflectionRight, Scope};

/// The refusal for a caller who may handle neither kind of kept text.
const NO_KEPT_RIGHT: &str = "kept texts need write access to this domain's engrams or access \
                             to its Reflection";

/// Which kinds of kept text a caller may handle on one domain.
#[derive(Clone, Copy, Debug)]
struct KeptRights {
    engram: bool,
    reflection: ReflectionRight,
}

impl KeptRights {
    /// Whether these rights may handle `k` for the account `mine`: an
    /// engram text needs write access to the engrams; a Reflection text
    /// needs what saving its page in a room needs, `edit`, and `curate` for
    /// PACT.md (as `require_reflection_edit` decides for the room); a kept
    /// comment needs `edit`, or `read` and being its recorded sender.
    fn allows(self, k: &KeptText, mine: Option<&str>) -> bool {
        match k.kind {
            KeptKind::Engram => self.engram,
            KeptKind::Reflection => {
                let need = if k.path == crystalline_core::reflection::PACT_FILE {
                    ReflectionRight::Curate
                } else {
                    ReflectionRight::Edit
                };
                self.reflection >= need
            }
            KeptKind::Comment => {
                self.reflection >= ReflectionRight::Edit
                    || (self.reflection >= ReflectionRight::Read
                        && mine.is_some()
                        && k.sender.as_ref().map(|s| s.account.as_str()) == mine)
            }
        }
    }
}

impl Engine {
    /// The kept texts of `domain` that `scope` may handle, oldest first.
    /// [`EngineError::Forbidden`] when it may handle neither kind, and the
    /// usual unknown-domain answer for a domain it may not see.
    pub async fn kept_texts(&self, domain: &str, scope: &Scope) -> Result<Vec<KeptText>> {
        let rights = self.kept_rights(domain, scope).await?;
        Ok(self.kept_of(domain, scope, rights))
    }

    /// One kept text of `domain` with its text. [`EngineError::NotFound`]
    /// for an id that names none, and for one of a kind `scope` may not
    /// handle, so a refusal tells nothing about what is kept.
    pub async fn kept_text(
        &self,
        domain: &str,
        id: &str,
        scope: &Scope,
    ) -> Result<(KeptText, String)> {
        let found = self.kept_entry(domain, id, scope).await?;
        let text = kept::read(&found).map_err(|source| EngineError::Io {
            path: found.file.display().to_string(),
            source,
        })?;
        Ok((found, text))
    }

    /// Delete one kept text of `domain`, with the same rights and answers
    /// as [`Engine::kept_text`]. Only ever on a person's request.
    pub async fn drop_kept_text(&self, domain: &str, id: &str, scope: &Scope) -> Result<()> {
        let found = self.kept_entry(domain, id, scope).await?;
        let root = self.kept_root()?;
        remove_found(&root, &found)
    }

    async fn kept_entry(&self, domain: &str, id: &str, scope: &Scope) -> Result<KeptText> {
        let rights = self.kept_rights(domain, scope).await?;
        self.kept_of(domain, scope, rights)
            .into_iter()
            .find(|k| k.id == id)
            .ok_or_else(|| {
                EngineError::NotFound(format!("no kept text '{id}' in domain '{domain}'"))
            })
    }

    /// What `scope` may handle on `domain`: refuses a domain it may not see
    /// first (one it reaches only through the Reflection counts as seen, as
    /// on every Reflection route), then a caller with neither right.
    async fn kept_rights(&self, domain: &str, scope: &Scope) -> Result<KeptRights> {
        let caller = ReflectionCaller::person(scope.clone(), "");
        let (reflection, engrams_visible) = self.caller_reflection_right(domain, &caller).await?;
        let rights = KeptRights {
            engram: engrams_visible && self.write_right(scope, domain).await? >= DomainRight::Write,
            reflection,
        };
        if !rights.engram && rights.reflection < ReflectionRight::Read {
            return Err(EngineError::Forbidden(NO_KEPT_RIGHT.to_string()));
        }
        Ok(rights)
    }

    /// `domain`'s kept texts of the kinds `rights` allows: the domain's
    /// own, and the ones kept from `scope`'s own review drafts. Another
    /// person's draft is theirs alone, whatever the rights. None when this
    /// process knows no state folder.
    fn kept_of(&self, domain: &str, scope: &Scope, rights: KeptRights) -> Vec<KeptText> {
        let Some(root) = self.kept_dir() else {
            return Vec::new();
        };
        let mine = crate::scope::overlay_actor(scope);
        kept::scan(&root)
            .into_iter()
            .filter(|k| {
                k.domain == domain
                    && !k.removed
                    && !k.unfinished
                    && rights.allows(k, mine.as_deref())
                    && (k.owner.is_none() || k.owner == mine)
            })
            .collect()
    }

    /// Move a removed domain's kept texts aside, so a domain registered
    /// later under the same name never reaches them; `crystalline kept list`
    /// still shows them, marked as from a removed domain. Best effort, like
    /// the journal sweep beside it: a failure is logged.
    pub(crate) fn set_aside_kept(&self, name: &str) {
        let Some(root) = self.kept_dir() else {
            return;
        };
        match kept::set_aside(&root, name, chrono::Utc::now()) {
            Ok(Some(to)) => tracing::info!(
                domain = name,
                kept = %to.display(),
                "the removed domain's kept texts are set aside here"
            ),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                domain = name,
                %err,
                "the removed domain's kept texts could not be set aside now; they are set aside when a domain of this name is next registered or renamed to"
            ),
        }
    }

    /// Before `name` becomes a domain's name (a new registration, or a
    /// rename's target before its journal exists), set aside a
    /// `kept/<name>` folder no registered domain owns: texts of a domain
    /// removed before removals set them aside, or whose set-aside failed.
    /// The new domain never reaches them. A folder whose name folds onto a
    /// registered domain's is that domain's on a disk that ignores case and
    /// stays. Never on a rename's replay: by then `kept/<name>` may hold the
    /// renamed domain's own texts.
    pub(crate) fn set_aside_orphan_kept(&self, name: &str) {
        let Some(root) = self.kept_dir() else {
            return;
        };
        if !root.join(name).is_dir() {
            return;
        }
        let folded = crystalline_core::config::registration::fold_domain_name(name);
        if self
            .registered_domain_entries()
            .keys()
            .any(|n| crystalline_core::config::registration::fold_domain_name(n) == folded)
        {
            return;
        }
        match kept::set_aside(&root, name, chrono::Utc::now()) {
            Ok(Some(to)) => tracing::info!(
                domain = name,
                kept = %to.display(),
                "kept texts no registered domain owns are set aside before the name is taken"
            ),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                domain = name,
                %err,
                "kept texts no registered domain owns could not be set aside; the domain taking this name may see them"
            ),
        }
    }

    /// At start, before anything is served: set aside every `kept/<name>`
    /// folder no registered domain owns, as `Engine::set_aside_orphan_kept`
    /// does for one name, so a domain that reached the configuration
    /// without going through a registration (a hand-edited file) never
    /// sees texts left under its name. Runs only while this process holds
    /// the state directory, and leaves both names of a rename journal still
    /// waiting alone (an opener finishes the journal first, so none should
    /// be left; this is the belt to that).
    pub fn set_aside_orphan_kept_all(&self) {
        if !self.holds_state_dir() {
            return;
        }
        let Some(root) = self.kept_dir() else {
            return;
        };
        let pending = self
            .journal_state_dir()
            .ok()
            .and_then(|dir| crate::rename::RenameJournal::load(&dir).ok().flatten())
            .map(|j| [j.old, j.new]);
        let Ok(entries) = std::fs::read_dir(&root) else {
            return;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if name.starts_with('.') || !entry.path().is_dir() {
                continue;
            }
            if pending.as_ref().is_some_and(|names| names.contains(&name)) {
                continue;
            }
            self.set_aside_orphan_kept(&name);
        }
    }

    /// A domain this engine first meets in the configuration file after it
    /// started (a hand edit, another process): its `kept/<name>` folder,
    /// if any, predates the registration, so it is set aside before the
    /// domain is served. Unlike [`Engine::set_aside_orphan_kept`] it does not
    /// ask the registrations, which already carry the new name.
    pub(crate) fn set_aside_kept_for_discovered(&self, name: &str) {
        if !self.holds_state_dir() {
            return;
        }
        let Some(root) = self.kept_dir() else {
            return;
        };
        if !root.join(name).is_dir() {
            return;
        }
        // Another registered domain whose name differs only in letter case
        // answers to the same folder on a disk that ignores case: those
        // texts are its own.
        let folded = crystalline_core::config::registration::fold_domain_name(name);
        if self.registered_domain_entries().keys().any(|other| {
            other != name
                && crystalline_core::config::registration::fold_domain_name(other) == folded
        }) {
            return;
        }
        match kept::set_aside(&root, name, chrono::Utc::now()) {
            Ok(Some(to)) => tracing::info!(
                domain = name,
                kept = %to.display(),
                "kept texts left under the name of a domain added to the configuration are set aside"
            ),
            Ok(None) => {}
            Err(err) => tracing::warn!(
                domain = name,
                %err,
                "kept texts left under the name of a domain added to the configuration could not be set aside; it may see them"
            ),
        }
    }

    /// The kept texts a name's drafts count: see [`kept::held_by`]. Fails naming
    /// the domain whose folder could not be read.
    pub(super) fn kept_held_by(&self, account: &str) -> Result<Vec<KeptText>> {
        let Some(root) = self.kept_dir() else {
            return Ok(Vec::new());
        };
        kept::held_by(&root, account).map_err(|domain| {
            tracing::warn!("the kept texts of '{account}' in '{domain}' could not be read");
            EngineError::Internal(format!(
                "the kept texts of domain '{domain}' could not be read"
            ))
        })
    }

    fn kept_root(&self) -> Result<std::path::PathBuf> {
        self.kept_dir()
            .ok_or_else(|| EngineError::Internal("this process knows no state folder".to_string()))
    }
}

/// Delete `found`. A file that went between the lookup and the delete (a
/// second drop of the same text at the same time) is the same miss as an
/// id that names nothing.
fn remove_found(root: &std::path::Path, found: &KeptText) -> Result<()> {
    kept::remove(root, found).map_err(|source| match source.kind() {
        std::io::ErrorKind::NotFound => EngineError::NotFound(format!(
            "no kept text '{}' in domain '{}'",
            found.id, found.domain
        )),
        _ => EngineError::Io {
            path: found.file.display().to_string(),
            source,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The second of two drops of one text answers not found, never an
    /// internal error.
    #[test]
    fn a_second_drop_of_the_same_text_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        kept::write(
            root,
            "eng",
            None,
            KeptKind::Engram,
            "a.md",
            chrono::Utc::now(),
            "x",
        )
        .unwrap();
        let found = kept::scan(root).remove(0);
        remove_found(root, &found).unwrap();
        let err = remove_found(root, &found).unwrap_err();
        assert!(matches!(err, EngineError::NotFound(_)), "{err:?}");
    }
}
