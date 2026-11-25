//! The one-time pin of the old default domains root, from the engine's side
//! (see [`crystalline_core::config::root_pin`] for the decision and the
//! record).

use super::*;
use crystalline_core::config::root_pin::{self, PinRecord};

/// Who asks for the check, named in the log line of a pin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PinCause {
    /// The daemon's start.
    Start,
    /// A registration, which saves the configuration anyway.
    Registration,
}

impl Engine {
    /// Write `domains_root: ~/Documents/Crystalline` into this engine's
    /// configuration once, when its file domains live there and no root is
    /// set, so new domains keep landing beside the existing ones.
    ///
    /// `Ok(None)` when skipped: a read-only instance, an engine with no
    /// configuration of its own (a test or one-shot engine, which would
    /// otherwise write the machine's global file), no state folder for the
    /// record, or a configuration checked before. A `config.yaml` that is a
    /// symbolic link (a dotfiles checkout) is pinned through the link, which
    /// stays a link ([`crystalline_core::config::save_yaml`]).
    /// `Ok(Some(pinned))` once
    /// the check ran and is recorded. An error when the file does not parse
    /// (nothing recorded, the next start tries again) or when the pin was due
    /// and could not be saved.
    pub fn pin_legacy_domains_root(&self, cause: PinCause) -> Result<Option<bool>> {
        if self.read_only {
            return Ok(None);
        }
        if self.config_path.is_none() && self.persisted.is_none() {
            return Ok(None);
        }
        let Ok(state_dir) = self.journal_state_dir() else {
            return Ok(None);
        };
        let in_document = self.config_in_document();
        let file_path = if in_document {
            None
        } else {
            self.config_file_path()
        };
        let key = root_pin::config_key(file_path.as_deref());
        let (record, problem) = PinRecord::load(&state_dir);
        if let Some(problem) = problem {
            tracing::warn!("domains root record: {problem}");
        }
        if record.checked(&key) {
            return Ok(None);
        }
        let pinned = {
            let mut guard = self.file_config.write().unwrap();
            let mut file = self.fresh_file_config(&guard)?;
            let due = root_pin::is_due_here(&*self.persisted_view(&file)?);
            if due {
                root_pin::pin(&mut file);
                self.commit_config(&mut guard, file)?;
            }
            due
        };
        if pinned {
            tracing::info!(?cause, "{}", root_pin::PINNED_NOTE);
        }
        if let Err(e) = PinRecord::record(&state_dir, &key, pinned) {
            tracing::warn!(error = %e, "writing the domains root record failed");
        }
        Ok(Some(pinned))
    }

    /// [`Engine::pin_legacy_domains_root`] at the start of a registration:
    /// a due pin that cannot be saved refuses the registration, because the
    /// new domain would otherwise be placed from the unpinned root.
    pub(super) fn pin_before_registration(&self) -> Result<()> {
        self.pin_legacy_domains_root(PinCause::Registration)
            .map(drop)
    }
}

/// Make sure the domains root can take a new domain created at the default
/// placement `<root>/<name>`, creating it when missing (owner only and hidden
/// for `~/.crystalline`). A thing at the root that is not a folder refuses
/// the add.
pub(super) fn prepare_default_root(root: &Path) -> Result<()> {
    use crystalline_core::config::root_folder::{RootProblem, prepare_domains_root};
    match prepare_domains_root(root) {
        Ok(done) => {
            if let Some(warning) = done.warning {
                tracing::warn!("{warning}");
            }
            Ok(())
        }
        Err(e @ RootProblem::NotAFolder(_)) => Err(EngineError::Invalid(e.to_string())),
        Err(e) => Err(EngineError::Internal(e.to_string())),
    }
}
