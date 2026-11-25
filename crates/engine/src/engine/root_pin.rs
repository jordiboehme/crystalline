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

    /// [`Engine::pin_legacy_domains_root`] at the start of a registration.
    /// A failure is held, not answered: it refuses the registration only
    /// once the add knows it registers a new domain
    /// ([`PinBeforeAdd::refuse_a_new_domain`]), because that domain would
    /// otherwise be placed from the unpinned root. Adding a domain that is
    /// already registered places nothing and goes through.
    pub(super) fn pin_before_registration(&self) -> PinBeforeAdd {
        match self.pin_legacy_domains_root(PinCause::Registration) {
            Ok(_) => PinBeforeAdd { failed: None },
            Err(e) => PinBeforeAdd { failed: Some(e) },
        }
    }

    /// The root a new domain without a folder lands under, read from the
    /// configuration as it is persisted now: another process (the CLI's own
    /// `domain add`, the ctl `domain_register` in file mode, a `config set`
    /// without the daemon) may have set or pinned it since this engine read
    /// it. After a pin that failed (`pin`), the root is planned as the pin
    /// would have set it, so an add can still tell a domain that is already
    /// registered there from a new one.
    pub(super) fn fresh_domains_root(&self, pin: &PinBeforeAdd) -> Result<PathBuf> {
        let guard = self.file_config.read().unwrap();
        let mut file = self.fresh_file_config(&guard)?;
        if pin.failed.is_some() && root_pin::is_due_here(&*self.persisted_view(&file)?) {
            root_pin::pin(&mut file);
        }
        Ok(self.effective_of(&file)?.domains_root())
    }
}

/// The outcome of [`Engine::pin_before_registration`].
pub(super) struct PinBeforeAdd {
    failed: Option<EngineError>,
}

impl PinBeforeAdd {
    /// Before a new domain is registered or anything is created for it: the
    /// held failure of the pin, if there is one. An add that only answers an
    /// existing registration never asks, and the failure is a warning.
    pub(super) fn refuse_a_new_domain(&mut self) -> Result<()> {
        match self.failed.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

impl Drop for PinBeforeAdd {
    fn drop(&mut self) {
        if let Some(e) = &self.failed {
            tracing::warn!(
                error = %e,
                "checking the domains root failed; the next start or registration checks again"
            );
        }
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
