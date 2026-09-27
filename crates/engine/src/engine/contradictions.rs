//! The contradiction check on the engine side: which model the configured
//! profile runs, and (Task 6) the daemon pass that scores related pairs, the
//! lazy scorer and its idle drop, the pending cache and the status block.

use super::*;
use crystalline_index::nli::{NliModel, NliProfile, nli_model};

impl Engine {
    /// The NLI model the configured `evolve.contradictions` profile runs.
    /// `None` for `off`, and for a hand-edited value this build does not know,
    /// which reads as off rather than as a guess at a model.
    pub fn contradiction_model(&self) -> Option<&'static NliModel> {
        let cfg = self.config.read().unwrap();
        NliProfile::from_setting(cfg.evolve_contradictions()).map(nli_model)
    }
}
