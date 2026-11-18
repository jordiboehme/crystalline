//! The macOS code signature check of a new binary, before it is ever run
//! (S9): `codesign --verify --strict` against the full requirement of the
//! release binaries, with their identifier and the Team ID. Keychain access
//! follows the designated requirement, which holds the identifier, so a
//! binary signed by the same team under another identifier is refused too.
//!
//! Compiled on every system so the requirement text is tested everywhere;
//! only a macOS update calls it.

use crystalline_core::update::{APPLE_TEAM_ID, CODESIGN_IDENTIFIER};

use super::client::UpdateError;
use super::spawn::Spawner;

/// The `codesign` that checks: the system's own, never one from `PATH`.
pub const CODESIGN: &str = "/usr/bin/codesign";

/// The code signing requirement of a release binary: its identifier, an
/// Apple anchor, a Developer ID certificate chain and the Team ID.
pub fn codesign_requirement() -> String {
    format!(
        "identifier \"{CODESIGN_IDENTIFIER}\" and anchor apple generic \
         and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
         and certificate leaf[subject.OU] = \"{APPLE_TEAM_ID}\""
    )
}

/// The arguments of the check of `bin`. The requirement is one argument,
/// `-R=<requirement>`: a leading `=` tells `codesign` it is the requirement
/// text and not a file holding it. No shell quoting: the spawner hands the
/// arguments over as they are.
pub fn codesign_args(bin: &str) -> Vec<String> {
    vec![
        "--verify".to_string(),
        "--strict".to_string(),
        format!("-R={}", codesign_requirement()),
        bin.to_string(),
    ]
}

/// How long `codesign` may take before it is killed and the check fails.
pub const CODESIGN_TIME_LIMIT: std::time::Duration = std::time::Duration::from_secs(60);

/// Check the code signature of `bin` against [`codesign_requirement`]. Any
/// failure, `codesign` itself missing or past [`CODESIGN_TIME_LIMIT`]
/// included, is [`UpdateError::Codesign`].
pub fn codesign_ok(spawner: &dyn Spawner, bin: &std::path::Path) -> Result<(), UpdateError> {
    let bin = bin.to_str().ok_or(UpdateError::Codesign)?;
    let args = codesign_args(bin);
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    spawner
        .output_within(CODESIGN, &args, CODESIGN_TIME_LIMIT)
        .map(|_| ())
        .map_err(|_| UpdateError::Codesign)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::spawn::Scripted;

    /// N2: codesign runs with a time limit, never unbounded.
    #[test]
    fn codesign_runs_with_a_time_limit() {
        struct BoundedOnly(std::sync::Mutex<Vec<u64>>);
        impl Spawner for BoundedOnly {
            fn run(&self, program: &str, _: &[&str]) -> std::io::Result<i32> {
                panic!("{program} run without a time limit")
            }
            fn output(&self, program: &str, _: &[&str]) -> std::io::Result<String> {
                panic!("{program} run without a time limit")
            }
            fn output_within(
                &self,
                _: &str,
                _: &[&str],
                limit: std::time::Duration,
            ) -> std::io::Result<String> {
                self.0.lock().unwrap().push(limit.as_secs());
                Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "slow"))
            }
        }
        let spawner = BoundedOnly(std::sync::Mutex::default());
        let err = codesign_ok(&spawner, std::path::Path::new("/b")).unwrap_err();
        assert!(matches!(err, UpdateError::Codesign), "{err}");
        assert_eq!(*spawner.0.lock().unwrap(), [CODESIGN_TIME_LIMIT.as_secs()]);
    }

    #[test]
    fn codesign_requirement_text() {
        let spawner = Scripted::new(|_, _| Ok(String::new()));
        codesign_ok(
            &spawner,
            std::path::Path::new("/opt/bin/.crystalline-new-7"),
        )
        .unwrap();
        assert_eq!(
            spawner.calls(),
            vec![
                "/usr/bin/codesign --verify --strict \
                 -R=identifier \"io.github.jordiboehme.crystalline\" and anchor apple generic \
                 and certificate 1[field.1.2.840.113635.100.6.2.6] exists \
                 and certificate leaf[field.1.2.840.113635.100.6.1.13] exists \
                 and certificate leaf[subject.OU] = \"77PA95K95V\" \
                 /opt/bin/.crystalline-new-7"
            ]
        );
        let args = codesign_args("/b");
        assert_eq!(args.len(), 4, "the requirement is one argument: {args:?}");
        assert!(!args[2].contains('\''), "no shell quoting: {}", args[2]);
    }

    #[test]
    fn a_signature_codesign_refuses_is_an_error() {
        let spawner = Scripted::new(|_, _| Err(std::io::Error::other("exit 3")));
        let err = codesign_ok(&spawner, std::path::Path::new("/b")).unwrap_err();
        assert!(matches!(err, UpdateError::Codesign), "{err}");
    }
}
