//! The first admin from the environment (spec A9, issue #126).
//!
//! In a container the first admin used to be made by hand: the browser form
//! needs the setup token from the log, and `users add --password-stdin` needs
//! a pipe a compose service cannot give. `serve` now reads an admin name and
//! password from the environment, or from files named by `_FILE` variants
//! (Docker secrets), and creates that admin only while no account exists.
//!
//! The reading ([`from_env`]) takes the environment and the file reader as
//! closures, so no test touches the process environment. The store part
//! ([`seed`]) is a function of its own for the same reason. [`seed_from_environment`]
//! joins the two for `run_serve`.
//!
//! The password never appears in an error, a log line or a `Debug` print: the
//! messages name the variable, the path and the name, never the value.

use std::io;
use std::path::Path;

use crystalline_core::secret_env::secret_source;

use crate::rest::{AuthStore, RefusalKind, StoreRefusal};

const NAME: &str = "CRYSTALLINE_ADMIN_NAME";
const NAME_FILE: &str = "CRYSTALLINE_ADMIN_NAME_FILE";
const PASSWORD: &str = "CRYSTALLINE_ADMIN_PASSWORD";
const PASSWORD_FILE: &str = "CRYSTALLINE_ADMIN_PASSWORD_FILE";

/// The admin the environment asks for.
pub(crate) struct AdminSeed {
    pub(crate) name: String,
    pub(crate) password: String,
    /// The variable the name came from (the plain one or its `_FILE` form).
    pub(crate) name_var: &'static str,
    /// The variable the password came from.
    pub(crate) password_var: &'static str,
}

impl std::fmt::Debug for AdminSeed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AdminSeed")
            .field("name", &self.name)
            .field("password", &"(redacted)")
            .field("name_var", &self.name_var)
            .field("password_var", &self.password_var)
            .finish()
    }
}

/// What [`seed`] did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Seeded {
    /// The admin was created by this call.
    Created,
    /// An account exists already (or another opener won the slot): nothing
    /// was written, the variables are ignored.
    Ignored,
}

/// Read the admin from the environment. `Ok(None)` when none of the four
/// variables is set.
pub(crate) fn from_env(
    var: impl Fn(&str) -> Option<String>,
    read: impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<AdminSeed>, String> {
    let name = secret_source(NAME, &var)?;
    let password = secret_source(PASSWORD, &var)?;
    let (name, password) = match (name, password) {
        (None, None) => return Ok(None),
        (Some(n), None) => {
            return Err(format!(
                "{} is set but no admin password is: set {PASSWORD} or {PASSWORD_FILE} as well",
                n.var()
            ));
        }
        (None, Some(p)) => {
            return Err(format!(
                "{} is set but no admin name is: set {NAME} or {NAME_FILE} as well",
                p.var()
            ));
        }
        (Some(n), Some(p)) => (n, p),
    };
    let name_var = if name.from_file() { NAME_FILE } else { NAME };
    let password_var = if password.from_file() {
        PASSWORD_FILE
    } else {
        PASSWORD
    };
    let name_text = name.resolve(&read)?;
    let password_text = password.resolve(&read)?;
    if password_text.is_empty() {
        return Err(format!(
            "the admin password from {password_var} is empty; pick one with at least one character"
        ));
    }
    Ok(Some(AdminSeed {
        name: name_text,
        password: password_text,
        name_var,
        password_var,
    }))
}

/// The password variable that is set, plain first, for the line that says it
/// is being ignored. Independent of [`from_env`], so a configuration that
/// would be refused still names its variable.
fn password_variable(var: &impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    [PASSWORD, PASSWORD_FILE]
        .into_iter()
        .find(|k| var(k).is_some_and(|v| !v.is_empty()))
}

/// Whether any of the four variables is set.
fn any_set(var: &impl Fn(&str) -> Option<String>) -> bool {
    [NAME, NAME_FILE, PASSWORD, PASSWORD_FILE]
        .into_iter()
        .any(|k| var(k).is_some_and(|v| !v.is_empty()))
}

/// Create the admin in `store`, only while no account exists.
///
/// `outcome` is what [`from_env`] answered for a set configuration. An existing
/// account makes the variables irrelevant, so it wins over a refusal in
/// `outcome`: a later mistake in them never stops a running installation.
pub(crate) async fn seed(
    store: &AuthStore,
    outcome: Result<AdminSeed, String>,
) -> Result<Seeded, String> {
    let accounts = store.user_count().await.map_err(|e| {
        format!("the accounts database could not be read, so the admin from the environment was not created ({e:#})")
    })?;
    if accounts > 0 {
        return Ok(Seeded::Ignored);
    }
    let seed = outcome?;
    match store
        .add_first_admin(&seed.name, &seed.name, &seed.password)
        .await
    {
        Ok(true) => Ok(Seeded::Created),
        Ok(false) => Ok(Seeded::Ignored),
        Err(e) if StoreRefusal::kind_of(&e) == Some(RefusalKind::InvalidName) => Err(format!(
            "the admin name from {} cannot be used: {e:#}",
            seed.name_var
        )),
        Err(e) => Err(format!(
            "the admin from the environment was not created ({e:#})"
        )),
    }
}

/// The line said when the admin was created.
fn created_line(name: &str) -> String {
    format!("created the first admin '{name}' from the environment")
}

/// The warning said once per start when an account exists and a password
/// variable is still set. A name alone is ignored silently.
fn ignored_line(var: &str) -> String {
    format!("an account exists already, so {var} is ignored; you can remove it")
}

/// What `run_serve` calls: read the process environment, open the accounts
/// database at `accounts` and seed it. Returns the line to say when the admin
/// was created, `None` otherwise. The store is dropped before this returns, so
/// the setup token's own open stays the only handle.
pub(crate) async fn seed_from_environment(
    accounts: Option<&Path>,
) -> anyhow::Result<Option<String>> {
    let var = |k: &str| std::env::var(k).ok();
    if !any_set(&var) {
        return Ok(None);
    }
    let outcome = from_env(var, |p| std::fs::read_to_string(p));
    let Some(path) = accounts else {
        anyhow::bail!(
            "the accounts database could not be located, so the admin from the environment was not created"
        );
    };
    let store = AuthStore::open(path).await.map_err(|e| {
        anyhow::anyhow!(
            "the accounts database at {} could not be opened, so the admin from the environment was not created ({e:#})",
            path.display()
        )
    })?;
    let name = outcome
        .as_ref()
        .ok()
        .and_then(|s| s.as_ref())
        .map(|s| s.name.clone())
        .unwrap_or_default();
    // Something is set, so `from_env` answered a seed or a refusal, never `None`.
    let outcome = outcome.and_then(|s| s.ok_or_else(|| "no admin is configured".to_string()));
    let seeded = seed(&store, outcome).await;
    drop(store);
    match seeded.map_err(anyhow::Error::msg)? {
        Seeded::Created => Ok(Some(created_line(&name))),
        Seeded::Ignored => {
            if let Some(v) = password_variable(&var) {
                tracing::warn!("{}", ignored_line(v));
            }
            Ok(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn files(pairs: &[(&str, &str)]) -> impl Fn(&Path) -> io::Result<String> {
        let map: HashMap<PathBuf, String> = pairs
            .iter()
            .map(|(k, v)| (PathBuf::from(k), v.to_string()))
            .collect();
        move |p| {
            map.get(p)
                .cloned()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
    }

    #[test]
    fn nothing_set_is_no_seed() {
        assert!(from_env(vars(&[]), files(&[])).unwrap().is_none());
    }

    #[test]
    fn a_name_and_a_password_make_a_seed() {
        let seed = from_env(vars(&[(NAME, "ada"), (PASSWORD, "s3cret")]), files(&[]))
            .unwrap()
            .unwrap();
        assert_eq!(seed.name, "ada");
        assert_eq!(seed.password, "s3cret");
        assert_eq!(seed.name_var, NAME);
        assert_eq!(seed.password_var, PASSWORD);
    }

    #[test]
    fn a_file_loses_one_trailing_line_break_and_nothing_else() {
        for (content, want) in [
            ("pw\n", "pw"),
            ("pw\r\n", "pw"),
            ("pw\n\n", "pw\n"),
            ("pw ", "pw "),
        ] {
            let seed = from_env(
                vars(&[(NAME, "ada"), (PASSWORD_FILE, "/run/secrets/pw")]),
                files(&[("/run/secrets/pw", content)]),
            )
            .unwrap()
            .unwrap();
            assert_eq!(seed.password, want, "content {content:?}");
            assert_eq!(seed.password_var, PASSWORD_FILE);
        }
        let seed = from_env(
            vars(&[(NAME_FILE, "/n"), (PASSWORD, "pw")]),
            files(&[("/n", "ada\n")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!((seed.name.as_str(), seed.name_var), ("ada", NAME_FILE));
    }

    #[test]
    fn an_empty_variable_counts_as_unset() {
        let none = from_env(
            vars(&[(NAME, ""), (PASSWORD, ""), (PASSWORD_FILE, "")]),
            files(&[]),
        )
        .unwrap();
        assert!(none.is_none());
        // An empty plain value next to a file form is not "both set".
        let seed = from_env(
            vars(&[(NAME, "ada"), (PASSWORD, ""), (PASSWORD_FILE, "/p")]),
            files(&[("/p", "pw")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(seed.password, "pw");
    }

    #[test]
    fn a_variable_and_its_file_form_together_are_refused() {
        let err = from_env(
            vars(&[(NAME, "ada"), (NAME_FILE, "/n"), (PASSWORD, "pw")]),
            files(&[]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "CRYSTALLINE_ADMIN_NAME and CRYSTALLINE_ADMIN_NAME_FILE are both set; keep one"
        );
        let err = from_env(
            vars(&[(NAME, "ada"), (PASSWORD, "pw"), (PASSWORD_FILE, "/p")]),
            files(&[]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "CRYSTALLINE_ADMIN_PASSWORD and CRYSTALLINE_ADMIN_PASSWORD_FILE are both set; keep one"
        );
    }

    #[test]
    fn a_name_without_a_password_is_refused() {
        for var in [NAME, NAME_FILE] {
            let err = from_env(vars(&[(var, "ada")]), files(&[])).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "{var} is set but no admin password is: set CRYSTALLINE_ADMIN_PASSWORD or CRYSTALLINE_ADMIN_PASSWORD_FILE as well"
                )
            );
        }
    }

    #[test]
    fn a_password_without_a_name_is_refused() {
        for var in [PASSWORD, PASSWORD_FILE] {
            let err = from_env(vars(&[(var, "hunter2-value")]), files(&[])).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "{var} is set but no admin name is: set CRYSTALLINE_ADMIN_NAME or CRYSTALLINE_ADMIN_NAME_FILE as well"
                )
            );
            if var == PASSWORD {
                assert!(!err.contains("hunter2-value"), "{err}");
            }
        }
    }

    #[test]
    fn an_unreadable_file_is_refused_naming_the_path() {
        let err = from_env(
            vars(&[(NAME, "ada"), (PASSWORD_FILE, "/run/secrets/missing")]),
            files(&[]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "CRYSTALLINE_ADMIN_PASSWORD_FILE names /run/secrets/missing, which cannot be read (no such file)"
        );
    }

    #[test]
    fn an_empty_password_file_is_refused() {
        let err = from_env(
            vars(&[(NAME, "ada"), (PASSWORD_FILE, "/p")]),
            files(&[("/p", "\n")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "the admin password from CRYSTALLINE_ADMIN_PASSWORD_FILE is empty; pick one with at least one character"
        );
    }

    #[test]
    fn the_debug_print_never_shows_the_password() {
        let seed = from_env(
            vars(&[(NAME, "ada"), (PASSWORD, "very-secret-value")]),
            files(&[]),
        )
        .unwrap()
        .unwrap();
        let shown = format!("{seed:?}");
        assert!(shown.contains("ada"), "{shown}");
        assert!(shown.contains("(redacted)"), "{shown}");
        assert!(!shown.contains("very-secret-value"), "{shown}");
    }

    #[test]
    fn the_ignored_warning_names_the_password_variable_only() {
        assert_eq!(password_variable(&vars(&[(NAME, "ada")])), None);
        assert_eq!(
            password_variable(&vars(&[(PASSWORD_FILE, "/p")])),
            Some(PASSWORD_FILE)
        );
        assert_eq!(
            ignored_line(PASSWORD),
            "an account exists already, so CRYSTALLINE_ADMIN_PASSWORD is ignored; you can remove it"
        );
    }

    fn seed_of(name: &str, password: &str) -> AdminSeed {
        AdminSeed {
            name: name.to_string(),
            password: password.to_string(),
            name_var: NAME,
            password_var: PASSWORD,
        }
    }

    async fn store() -> (tempfile::TempDir, AuthStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        (dir, store)
    }

    #[tokio::test]
    async fn an_empty_store_gets_the_admin_and_the_password_works() {
        let (_dir, store) = store().await;
        let done = seed(&store, Ok(seed_of("ada", "correct horse"))).await;
        assert_eq!(done, Ok(Seeded::Created));
        let user = store
            .verify_password("ada", "correct horse")
            .await
            .unwrap()
            .expect("the password verifies");
        assert_eq!(user.role, crate::rest::Role::Admin);
        assert!(
            store
                .verify_password("ada", "wrong")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[tokio::test]
    async fn an_existing_account_is_never_changed() {
        let (_dir, store) = store().await;
        assert!(store.add_first_admin("ada", "Ada", "old").await.unwrap());
        let done = seed(&store, Ok(seed_of("ada", "new"))).await;
        assert_eq!(done, Ok(Seeded::Ignored));
        assert!(store.verify_password("ada", "old").await.unwrap().is_some());
        assert!(store.verify_password("ada", "new").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn a_deleted_admin_is_not_brought_back() {
        let (_dir, store) = store().await;
        assert!(store.add_first_admin("bob", "Bob", "pw").await.unwrap());
        let done = seed(&store, Ok(seed_of("ada", "new"))).await;
        assert_eq!(done, Ok(Seeded::Ignored));
        assert!(store.verify_password("ada", "new").await.unwrap().is_none());
        assert_eq!(store.user_count().await.unwrap(), 1);
    }

    #[tokio::test]
    async fn a_bad_configuration_is_ignored_once_an_account_exists() {
        let (_dir, store) = store().await;
        assert!(store.add_first_admin("bob", "Bob", "pw").await.unwrap());
        let done = seed(&store, Err("a half done configuration".to_string())).await;
        assert_eq!(done, Ok(Seeded::Ignored));
    }

    #[tokio::test]
    async fn a_bad_configuration_is_refused_while_no_account_exists() {
        let (_dir, store) = store().await;
        let done = seed(&store, Err("a half done configuration".to_string())).await;
        assert_eq!(done, Err("a half done configuration".to_string()));
    }

    #[tokio::test]
    async fn a_name_the_store_refuses_is_refused_with_its_reason() {
        let (_dir, store) = store().await;
        let err = seed(&store, Ok(seed_of("ada lovelace", "pw")))
            .await
            .unwrap_err();
        assert!(
            err.starts_with("the admin name from CRYSTALLINE_ADMIN_NAME cannot be used: "),
            "{err}"
        );
        assert!(err.len() > "the admin name from CRYSTALLINE_ADMIN_NAME cannot be used: ".len());
        assert_eq!(store.user_count().await.unwrap(), 0);
    }

    #[test]
    fn the_file_forms_follow_the_shared_suffix() {
        assert_eq!(crystalline_core::secret_env::file_var(NAME), NAME_FILE);
        assert_eq!(
            crystalline_core::secret_env::file_var(PASSWORD),
            PASSWORD_FILE
        );
    }
}
