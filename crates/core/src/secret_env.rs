//! Secrets from the environment, as a value or as a file a variable names.
//!
//! A container keeps a secret out of `docker inspect` by mounting it as a
//! file (a Docker secret under `/run/secrets/`) and naming that file in the
//! variable's `_FILE` form. Every variable with a file form is read here, so
//! there is one set of rules: an empty value counts as unset, both forms set
//! is refused naming both, a file that cannot be read is refused naming the
//! variable and the path, and a file loses exactly one trailing line break
//! (`\r\n` or `\n`) and nothing else.
//!
//! The environment and the file system come in as closures, so no test
//! touches the process environment; [`process_var`] is the one ambient read.
//! A message names variables and paths, never a value.

use std::io;
use std::path::{Path, PathBuf};

/// The suffix of a variable's file form.
pub const FILE_SUFFIX: &str = "_FILE";

/// The secrets the environment overlay and the remote-server lookup resolve
/// with their file form. The first admin's pair and the setup token have a
/// file form too and are read by their own code through [`secret_source`].
/// The names are spelled out here because this crate cannot name the
/// overlay's or the remote crate's constants; their own tests pin that the
/// spellings agree.
pub const FILE_BACKED: [&str; 3] = [
    "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET",
    "CRYSTALLINE_GITHUB_TOKEN",
    "CRYSTALLINE_REMOTE_TOKEN",
];

/// The file form of `plain`: `CRYSTALLINE_GITHUB_TOKEN` becomes
/// `CRYSTALLINE_GITHUB_TOKEN_FILE`.
pub fn file_var(plain: &str) -> String {
    format!("{plain}{FILE_SUFFIX}")
}

/// `text` without exactly one trailing line break, `\r\n` or `\n`.
pub fn trim_one_line_break(text: &str) -> &str {
    text.strip_suffix("\r\n")
        .or_else(|| text.strip_suffix('\n'))
        .unwrap_or(text)
}

/// Which of a variable and its file form is set, before any file is read.
pub struct SecretSource {
    var: String,
    from_file: bool,
    raw: String,
}

/// The raw value is the secret itself for a plain variable, so it is never
/// printed.
impl std::fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SecretSource")
            .field("var", &self.var)
            .field("from_file", &self.from_file)
            .field("raw", &"(redacted)")
            .finish()
    }
}

impl SecretSource {
    /// The variable that is set: the plain one or its file form.
    pub fn var(&self) -> &str {
        &self.var
    }

    /// Whether the value comes from a file.
    pub fn from_file(&self) -> bool {
        self.from_file
    }

    /// The secret: the plain value as is, or the file's content minus one
    /// trailing line break.
    pub fn resolve(self, read: &impl Fn(&Path) -> io::Result<String>) -> Result<String, String> {
        if !self.from_file {
            return Ok(self.raw);
        }
        let path = PathBuf::from(&self.raw);
        let text = read(&path).map_err(|e| {
            format!(
                "{} names {}, which cannot be read ({e})",
                self.var,
                path.display()
            )
        })?;
        Ok(trim_one_line_break(&text).to_string())
    }
}

/// Which form of `plain` is set. `Ok(None)` when neither is, an empty value
/// counting as unset.
pub fn secret_source(
    plain: &str,
    var: &impl Fn(&str) -> Option<String>,
) -> Result<Option<SecretSource>, String> {
    let file = file_var(plain);
    let get = |k: &str| var(k).filter(|v| !v.is_empty());
    match (get(plain), get(&file)) {
        (Some(_), Some(_)) => Err(format!("{plain} and {file} are both set; keep one")),
        (Some(raw), None) => Ok(Some(SecretSource {
            var: plain.to_string(),
            from_file: false,
            raw,
        })),
        (None, Some(raw)) => Ok(Some(SecretSource {
            var: file,
            from_file: true,
            raw,
        })),
        (None, None) => Ok(None),
    }
}

/// [`secret_source`] and [`SecretSource::resolve`] in one step.
pub fn read_secret(
    plain: &str,
    var: &impl Fn(&str) -> Option<String>,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Result<Option<String>, String> {
    secret_source(plain, var)?
        .map(|source| source.resolve(read))
        .transpose()
}

/// The value of `name`, with the file form resolved for a [`FILE_BACKED`]
/// variable. A refused pair or an unreadable file reads as unset here: the
/// environment overlay refuses both at every start, so a process that got
/// this far has neither.
pub fn resolved_var(
    name: &str,
    var: &impl Fn(&str) -> Option<String>,
    read: &impl Fn(&Path) -> io::Result<String>,
) -> Option<String> {
    if FILE_BACKED.contains(&name) {
        return read_secret(name, var, read).ok().flatten();
    }
    var(name)
}

/// [`resolved_var`] over this process's environment and file system: what
/// every `|name| std::env::var(name).ok()` closure that may be asked for
/// `CRYSTALLINE_REMOTE_TOKEN` uses instead.
pub fn process_var(name: &str) -> Option<String> {
    resolved_var(name, &|k: &str| std::env::var(k).ok(), &|p: &Path| {
        std::fs::read_to_string(p)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

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

    const VAR: &str = "CRYSTALLINE_GITHUB_TOKEN";

    #[test]
    fn the_plain_form_is_the_value_as_set() {
        let got = read_secret(VAR, &vars(&[(VAR, "ghp_plain\n")]), &files(&[])).unwrap();
        assert_eq!(
            got.as_deref(),
            Some("ghp_plain\n"),
            "a plain value is never trimmed"
        );
    }

    #[test]
    fn a_file_loses_one_crlf_or_one_lf_and_nothing_else() {
        for (content, want) in [
            ("ghp_secret\r\n", "ghp_secret"),
            ("ghp_secret\n", "ghp_secret"),
            ("ghp_secret", "ghp_secret"),
            ("ghp_secret\n\n", "ghp_secret\n"),
            ("ghp_secret ", "ghp_secret "),
        ] {
            let got = read_secret(
                VAR,
                &vars(&[("CRYSTALLINE_GITHUB_TOKEN_FILE", "/run/secrets/gh")]),
                &files(&[("/run/secrets/gh", content)]),
            )
            .unwrap();
            assert_eq!(got.as_deref(), Some(want), "content {content:?}");
        }
    }

    #[test]
    fn an_empty_value_counts_as_unset() {
        assert_eq!(
            read_secret(VAR, &vars(&[(VAR, "")]), &files(&[])).unwrap(),
            None
        );
        let got = read_secret(
            VAR,
            &vars(&[(VAR, ""), ("CRYSTALLINE_GITHUB_TOKEN_FILE", "/f")]),
            &files(&[("/f", "ghp_file")]),
        )
        .unwrap();
        assert_eq!(
            got.as_deref(),
            Some("ghp_file"),
            "an empty plain form is not 'both set'"
        );
    }

    #[test]
    fn both_forms_set_is_refused_naming_both_and_no_value() {
        let err = read_secret(
            VAR,
            &vars(&[
                (VAR, "ghp_value_one"),
                ("CRYSTALLINE_GITHUB_TOKEN_FILE", "/f"),
            ]),
            &files(&[("/f", "ghp_value_two")]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "CRYSTALLINE_GITHUB_TOKEN and CRYSTALLINE_GITHUB_TOKEN_FILE are both set; keep one"
        );
    }

    #[test]
    fn an_unreadable_file_is_refused_naming_the_variable_and_the_path() {
        let err = read_secret(
            VAR,
            &vars(&[("CRYSTALLINE_GITHUB_TOKEN_FILE", "/run/secrets/missing")]),
            &files(&[]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            "CRYSTALLINE_GITHUB_TOKEN_FILE names /run/secrets/missing, which cannot be read (no such file)"
        );
    }

    #[test]
    fn the_source_says_which_variable_is_set_and_its_debug_hides_the_value() {
        let plain = secret_source(VAR, &vars(&[(VAR, "ghp_hidden_value")]))
            .unwrap()
            .unwrap();
        assert_eq!((plain.var(), plain.from_file()), (VAR, false));
        assert!(!format!("{plain:?}").contains("ghp_hidden_value"));
        let file = secret_source(VAR, &vars(&[("CRYSTALLINE_GITHUB_TOKEN_FILE", "/f")]))
            .unwrap()
            .unwrap();
        assert_eq!(
            (file.var(), file.from_file()),
            ("CRYSTALLINE_GITHUB_TOKEN_FILE", true)
        );
    }

    #[test]
    fn the_lookup_resolves_the_file_form_of_a_file_backed_variable_only() {
        let env = vars(&[
            ("CRYSTALLINE_REMOTE_TOKEN_FILE", "/run/secrets/cmt"),
            ("CRYSTALLINE_REMOTE_URL", "https://kb.example.com"),
            ("CRYSTALLINE_REMOTE_URL_FILE", "/run/secrets/url"),
        ]);
        let read = files(&[
            ("/run/secrets/cmt", "cmt_from_file\r\n"),
            ("/run/secrets/url", "x"),
        ]);
        assert_eq!(
            resolved_var("CRYSTALLINE_REMOTE_TOKEN", &env, &read).as_deref(),
            Some("cmt_from_file")
        );
        assert_eq!(
            resolved_var("CRYSTALLINE_REMOTE_URL", &env, &read).as_deref(),
            Some("https://kb.example.com"),
            "a variable with no file form is read as it is"
        );
        // A refused pair reads as unset here; the start already refused it.
        let both = vars(&[
            ("CRYSTALLINE_REMOTE_TOKEN", "cmt_a"),
            ("CRYSTALLINE_REMOTE_TOKEN_FILE", "/run/secrets/cmt"),
        ]);
        assert_eq!(resolved_var("CRYSTALLINE_REMOTE_TOKEN", &both, &read), None);
    }

    #[test]
    fn the_file_backed_variables_are_the_three_secrets() {
        assert_eq!(
            FILE_BACKED,
            [
                "CRYSTALLINE_AUTH_OIDC_CLIENT_SECRET",
                "CRYSTALLINE_GITHUB_TOKEN",
                "CRYSTALLINE_REMOTE_TOKEN",
            ]
        );
        assert_eq!(
            file_var("CRYSTALLINE_REMOTE_TOKEN"),
            "CRYSTALLINE_REMOTE_TOKEN_FILE"
        );
    }
}
