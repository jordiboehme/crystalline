//! The servers this machine offers domains from: `<state_dir>/remote/sources.json`.
//!
//! One record per server `crystalline connect` signed in to, in the order
//! they were connected. That order is load bearing: when two sources bring a
//! domain of the same name, or the same domain, the one connected first keeps
//! it ([`crate::mounts::assign`] decides it). Each record also holds every
//! name it handed out to a domain of its server ([`MountRecord`]), so a name,
//! once given, never changes on its own: the next assignment reads it back
//! before it decides anything new.
//!
//! The credential is not here. It lives in the keychain or the host folder's
//! `credential.json` ([`crate::server_token`]); this file holds nothing
//! secret and is safe to show.
//!
//! Several processes write this file (the CLI's `connect`, `disconnect` and
//! `domain rename --local`, the daemon's poller when a server grows a
//! domain). [`update_sources`] is the only writer: it takes an exclusive lock
//! on `sources.lock`, re-reads the file under it, applies the change and
//! saves atomically, so no writer loses another's change.
//!
//! The environment's source (`CRYSTALLINE_REMOTE_URL` with
//! `CRYSTALLINE_REMOTE_TOKEN`) is never written here. [`env_source`] builds
//! it at every load, after the saved ones, and [`update_sources`] drops it
//! before it saves, so a CI job or a container stays stateless.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::error::RemoteError;
use crate::server_token::{CredentialKind, server_folder, server_key};

/// The file, in `<state_dir>/remote/`.
pub const SOURCES_FILE: &str = "sources.json";
/// The lock every writer of [`SOURCES_FILE`] holds.
pub const SOURCES_LOCK: &str = "sources.lock";
/// The format version this build writes and reads.
pub const SOURCES_VERSION: u32 = 1;
/// The word a local domain registered later under a mounted name is
/// suffixed with (`notes-local`). Reserved, so no source is ever called it.
pub const LOCAL_SUFFIX: &str = "local";
/// The environment's server URL; a source only together with
/// [`REMOTE_TOKEN_ENV`].
pub const REMOTE_URL_ENV: &str = "CRYSTALLINE_REMOTE_URL";
/// The environment's personal MCP token for [`REMOTE_URL_ENV`].
pub const REMOTE_TOKEN_ENV: &str = "CRYSTALLINE_REMOTE_TOKEN";
/// The domains the environment's source takes, by their names on its
/// server, comma separated. Unset or empty: every domain it offers.
pub const REMOTE_DOMAINS_ENV: &str = "CRYSTALLINE_REMOTE_DOMAINS";

/// Host labels that name what the machine is rather than whose it is, so a
/// default source name skips them: `crystalline.acme.com` is `acme`.
const GENERIC_LABELS: &[&str] = &[
    "crystalline",
    "knowledge",
    "kb",
    "www",
    "api",
    "mcp",
    "server",
    "app",
    "team",
];

/// The longest source name.
const MAX_NAME_CHARS: usize = 32;

/// How long a writer waits for another writer's lock.
const LOCK_WAIT: Duration = Duration::from_secs(15);

/// One name a source handed out: its server's domain `remote` is `local` on
/// this machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MountRecord {
    /// The domain's name on its server.
    pub remote: String,
    /// The domain's name on this machine.
    pub local: String,
}

/// One connected server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceRecord {
    /// The server's base address: scheme, host, optional port and the path it
    /// is served under, no trailing slash.
    pub url: String,
    /// Its short name on this machine, used in notes and collision names.
    pub name: String,
    /// The account the server resolved the credential to.
    pub account: String,
    /// How this machine signed in.
    pub kind: CredentialKind,
    /// Where an OAuth refresh goes.
    #[serde(default)]
    pub token_endpoint: Option<String>,
    /// Where `disconnect` revokes an OAuth grant.
    #[serde(default)]
    pub revocation_endpoint: Option<String>,
    /// When the sign-in happened.
    pub connected_at: DateTime<Utc>,
    /// Every name this source handed out, in the order it handed them out.
    #[serde(default)]
    pub mounts: Vec<MountRecord>,
    /// The domains this source takes, by their names on the server, sorted;
    /// `None` takes every domain it offers. Set by `connect --domains`,
    /// kept by a `connect` without it, cleared by `--all-domains`. Absent in
    /// a file 0.23.0 wrote, which therefore takes all, and never written
    /// when `None`, so 0.23.0 reads a file this build wrote.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domains: Option<Vec<String>>,
    /// Built from the environment rather than read from the file.
    #[serde(skip)]
    pub from_env: bool,
}

impl SourceRecord {
    /// The keychain key for this server (see [`server_key`]).
    pub fn key(&self) -> String {
        server_key(&self.url)
    }

    /// `<remote_dir>/<folder>`: the credential fallback, the refresh lock and
    /// the cached answers, in the folder [`server_folder`] names for the key.
    pub fn host_dir(&self, remote_dir: &Path) -> PathBuf {
        remote_dir.join(server_folder(&self.key()))
    }
}

/// The whole file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourcesFile {
    /// Always [`SOURCES_VERSION`] for what this build writes.
    pub v: u32,
    /// The sources, in connect order.
    #[serde(default)]
    pub sources: Vec<SourceRecord>,
}

impl Default for SourcesFile {
    fn default() -> SourcesFile {
        SourcesFile {
            v: SOURCES_VERSION,
            sources: Vec::new(),
        }
    }
}

/// `url` as a comparison key: the server key, which folds the host's case and
/// a trailing slash away and keeps the path.
fn same_server(a: &str, b: &str) -> bool {
    server_key(a) == server_key(b)
}

impl SourcesFile {
    /// The source named `name_or_url`: by its name, or by its URL in any
    /// spelling of the same origin.
    pub fn find(&self, name_or_url: &str) -> Option<&SourceRecord> {
        self.sources.iter().find(|s| {
            s.name == name_or_url
                || (name_or_url.contains("://") && same_server(&s.url, name_or_url))
        })
    }

    /// [`SourcesFile::find`], mutably.
    pub fn find_mut(&mut self, name_or_url: &str) -> Option<&mut SourceRecord> {
        self.sources.iter_mut().find(|s| {
            s.name == name_or_url
                || (name_or_url.contains("://") && same_server(&s.url, name_or_url))
        })
    }

    /// Every source name, in connect order.
    pub fn names(&self) -> Vec<String> {
        self.sources.iter().map(|s| s.name.clone()).collect()
    }

    /// Add a source, or refresh the sign-in of one already connected to the
    /// same server. A refreshed one keeps its place, its name and every name
    /// it handed out: signing in again is not connecting anew.
    pub fn upsert(&mut self, record: SourceRecord) -> &SourceRecord {
        let at = self
            .sources
            .iter()
            .position(|s| same_server(&s.url, &record.url));
        match at {
            Some(at) => {
                let existing = &mut self.sources[at];
                existing.url = record.url;
                existing.account = record.account;
                existing.kind = record.kind;
                existing.token_endpoint = record.token_endpoint;
                existing.revocation_endpoint = record.revocation_endpoint;
                existing.connected_at = record.connected_at;
                existing.domains = record.domains;
                &self.sources[at]
            }
            None => {
                self.sources.push(record);
                self.sources.last().expect("just pushed")
            }
        }
    }

    /// Take a source out, answering it.
    pub fn remove(&mut self, name_or_url: &str) -> Option<SourceRecord> {
        let name = self.find(name_or_url)?.name.clone();
        let at = self.sources.iter().position(|s| s.name == name)?;
        Some(self.sources.remove(at))
    }
}

/// `<state_dir>/remote`.
pub fn remote_dir() -> Result<PathBuf, RemoteError> {
    crystalline_core::config::state_dir()
        .map(|dir| dir.join("remote"))
        .map_err(|e| RemoteError::State(format!("no state directory for the sources: {e}")))
}

/// The saved sources. A missing file is no sources; a file this build cannot
/// read, or one a newer build wrote, is an error and is never overwritten.
pub fn load_sources(remote_dir: &Path) -> Result<SourcesFile, RemoteError> {
    let path = remote_dir.join(SOURCES_FILE);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SourcesFile::default()),
        Err(e) => return Err(e.into()),
    };
    let file: SourcesFile = serde_json::from_slice(&bytes)
        .map_err(|e| RemoteError::State(format!("{} is not valid: {e}", path.display())))?;
    if file.v != SOURCES_VERSION {
        return Err(RemoteError::State(format!(
            "{} was written by a newer Crystalline (format {}); upgrade this one, or move the file away to start over",
            path.display(),
            file.v
        )));
    }
    Ok(file)
}

/// Change the saved sources: lock, re-read, apply `f`, save atomically. The
/// one writer of the file, so two processes never lose each other's change.
/// Blocking; async callers run it on `spawn_blocking`.
pub fn update_sources<T>(
    remote_dir: &Path,
    f: impl FnOnce(&mut SourcesFile) -> Result<T, RemoteError>,
) -> Result<T, RemoteError> {
    std::fs::create_dir_all(remote_dir)?;
    let _lock = SourcesLock::acquire(&remote_dir.join(SOURCES_LOCK))?;
    let mut file = load_sources(remote_dir)?;
    let answer = f(&mut file)?;
    // The environment's source lives only as long as the process does.
    file.sources.retain(|s| !s.from_env);
    file.v = SOURCES_VERSION;
    let json = serde_json::to_vec_pretty(&file)
        .map_err(|e| RemoteError::State(format!("could not write the sources: {e}")))?;
    crystalline_core::config::save_bytes(&remote_dir.join(SOURCES_FILE), &json)
        .map_err(|e| RemoteError::State(format!("could not save the sources: {e}")))?;
    Ok(answer)
}

/// The exclusive lock on [`SOURCES_LOCK`], released on drop.
struct SourcesLock {
    file: std::fs::File,
}

impl SourcesLock {
    fn acquire(path: &Path) -> Result<SourcesLock, RemoteError> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        let deadline = Instant::now() + LOCK_WAIT;
        loop {
            match fs4::FileExt::try_lock(&file) {
                Ok(()) => return Ok(SourcesLock { file }),
                Err(fs4::TryLockError::WouldBlock) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                Err(fs4::TryLockError::WouldBlock) => {
                    return Err(RemoteError::State(
                        "another crystalline process holds the sources file and has not finished"
                            .to_string(),
                    ));
                }
                Err(fs4::TryLockError::Error(e)) => return Err(e.into()),
            }
        }
    }
}

impl Drop for SourcesLock {
    fn drop(&mut self) {
        let _ = fs4::FileExt::unlock(&self.file);
    }
}

/// Whether `name` may name a source: 1 to 32 characters of `a-z`, `0-9` and
/// `-`, not starting or ending with `-`, and never [`LOCAL_SUFFIX`].
pub fn valid_source_name(name: &str) -> Result<(), String> {
    if name == LOCAL_SUFFIX {
        return Err(format!(
            "'{LOCAL_SUFFIX}' is reserved: it marks a local domain registered under a name a source already gave out"
        ));
    }
    let shaped = !name.is_empty()
        && name.chars().count() <= MAX_NAME_CHARS
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !name.starts_with('-')
        && !name.ends_with('-');
    if shaped {
        Ok(())
    } else {
        Err(format!(
            "'{name}' cannot name a source: use 1 to {MAX_NAME_CHARS} lower-case letters, digits and hyphens, not starting or ending with a hyphen"
        ))
    }
}

/// The name a source gets when nobody names it: the leftmost host label that
/// is not generic and not the last one (`crystalline.acme.com` is `acme`),
/// `server` for an address or a host with nothing else to say, counted up
/// past any name in `taken`.
pub fn default_source_name(origin: &str, taken: &[String]) -> String {
    let rest = origin.split_once("://").map_or(origin, |(_, rest)| rest);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if authority.starts_with('[') {
        ""
    } else {
        authority.split(':').next().unwrap_or("")
    }
    .to_ascii_lowercase();
    let is_address =
        host.parse::<std::net::IpAddr>().is_ok() || host == "localhost" || host.is_empty();
    let labels: Vec<&str> = host.split('.').collect();
    let candidate = if is_address || labels.len() < 2 {
        None
    } else {
        labels[..labels.len() - 1]
            .iter()
            .find(|label| !GENERIC_LABELS.contains(label) && valid_source_name(label).is_ok())
            .map(|label| label.to_string())
    };
    let base = candidate.unwrap_or_else(|| "server".to_string());
    if !taken.contains(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken.contains(name))
        .expect("an unbounded count finds a free name")
}

/// The environment's server address, normalized, or why it cannot be. The
/// reason never repeats the value: a token pasted into the wrong variable
/// would otherwise land in the log on every load.
fn env_url(raw: &str) -> Result<String, String> {
    use crystalline_core::base::{BaseProblem, PathProblem, PublicBase};
    let problem = match crate::sign_in::normalize_server_url(raw) {
        Ok(url) => return Ok(url),
        Err(problem) => problem,
    };
    let reason = if raw.contains('\\') {
        "it has a backslash, and a server address uses '/' only"
    } else if let crate::sign_in::SignInError::InsecureUrl { .. } = problem {
        "plain http is allowed only to this machine"
    } else {
        match PublicBase::parse(raw) {
            Err(BaseProblem::Path(PathProblem::Character(_))) => {
                "its path may use only lower-case letters, digits, '.', '_' and '-'"
            }
            Err(BaseProblem::Path(PathProblem::DotSegment(_))) => {
                "its path has a '.' or '..' segment, which a browser resolves away"
            }
            Err(BaseProblem::Path(PathProblem::Reserved(_))) => {
                "its path starts with a segment the server answers at itself"
            }
            Err(BaseProblem::Path(PathProblem::EmptySegment)) => {
                "its path has an empty segment (two slashes in a row)"
            }
            Err(BaseProblem::Path(PathProblem::QueryOrFragment)) => {
                "it carries a query or a fragment"
            }
            _ => "it is not a usable server address",
        }
    };
    Err(reason.to_string())
}

/// A `--domains` value or [`REMOTE_DOMAINS_ENV`]: names separated by commas,
/// each passing the rule every local domain name passes, sorted and without
/// duplicates. An empty list is refused: a source that takes nothing is a
/// source to disconnect.
pub fn parse_domain_list(raw: &str) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    let parts: Vec<&str> = raw.split(',').map(str::trim).collect();
    if parts.iter().all(|p| p.is_empty()) {
        return Err("name at least one domain, for example alpha,beta".to_string());
    }
    // No refusal repeats the value or a part of it: a token pasted into the
    // wrong place would otherwise land on the terminal, or in the daemon log
    // on every load for the environment variable. The position says which
    // name it is.
    for (at, part) in parts.into_iter().enumerate() {
        let position = at + 1;
        if part.is_empty() {
            return Err(format!(
                "name {position} in the list is empty (two commas in a row)"
            ));
        }
        if crystalline_core::config::registration::validate_domain_name(part).is_err() {
            return Err(format!(
                "name {position} in the list cannot name a domain: {}",
                crystalline_core::config::registration::DOMAIN_NAME_RULE
            ));
        }
        names.push(part.to_string());
    }
    names.sort();
    names.dedup();
    Ok(names)
}

/// [`REMOTE_DOMAINS_ENV`] as a list, or the warning that leaves the
/// environment source out. The warning names the variable and the rule and
/// never the value: it lands in the daemon log on every load, and the value
/// may be a token pasted into the wrong variable.
fn env_domains(raw: &str) -> Result<Vec<String>, String> {
    parse_domain_list(raw).map_err(|e| {
        format!(
            "{REMOTE_DOMAINS_ENV}: {e}; the server from {REMOTE_URL_ENV} is left out rather than taking every domain"
        )
    })
}

/// The source the environment adds, when both variables are set. Never
/// saved; named like any other, past `taken`.
pub fn env_source(env: impl Fn(&str) -> Option<String>, taken: &[String]) -> Option<SourceRecord> {
    let raw = env(REMOTE_URL_ENV)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())?;
    let url = match env_url(&raw) {
        Ok(url) => url,
        Err(reason) => {
            tracing::warn!("{REMOTE_URL_ENV}: {reason}; using the value as written");
            raw.trim_end_matches('/').to_string()
        }
    };
    env(REMOTE_TOKEN_ENV).filter(|v| !v.trim().is_empty())?;
    let domains = match env(REMOTE_DOMAINS_ENV)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
    {
        None => None,
        Some(raw) => match env_domains(&raw) {
            Ok(list) => Some(list),
            Err(warning) => {
                tracing::warn!("{warning}");
                return None;
            }
        },
    };
    Some(SourceRecord {
        name: default_source_name(&url, taken),
        url,
        account: String::new(),
        kind: CredentialKind::Token,
        token_endpoint: None,
        revocation_endpoint: None,
        connected_at: Utc::now(),
        mounts: Vec::new(),
        domains,
        from_env: true,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    fn record(url: &str, name: &str) -> SourceRecord {
        SourceRecord {
            url: url.to_string(),
            name: name.to_string(),
            account: "ada".to_string(),
            kind: CredentialKind::Token,
            token_endpoint: None,
            revocation_endpoint: None,
            connected_at: DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            mounts: Vec::new(),
            domains: None,
            from_env: false,
        }
    }

    #[test]
    fn a_missing_file_is_no_sources_and_a_saved_one_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load_sources(dir.path()).unwrap(), SourcesFile::default());
        update_sources(dir.path(), |file| {
            file.upsert(record("https://crystalline.acme.com", "acme"));
            Ok(())
        })
        .unwrap();
        let loaded = load_sources(dir.path()).unwrap();
        assert_eq!(loaded.v, SOURCES_VERSION);
        assert_eq!(loaded.names(), vec!["acme".to_string()]);
        let text = std::fs::read_to_string(dir.path().join(SOURCES_FILE)).unwrap();
        assert!(text.contains("\"v\": 1"), "{text}");
        assert!(!text.contains("from_env"), "never written: {text}");
    }

    #[test]
    fn the_environment_source_is_never_written_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let env = |name: &str| match name {
            REMOTE_URL_ENV => Some("https://kb.envcorp.com".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_env".to_string()),
            _ => None,
        };
        update_sources(dir.path(), |file| {
            file.upsert(record("https://crystalline.acme.com", "acme"));
            file.upsert(env_source(env, &file.names()).unwrap());
            assert_eq!(file.sources.len(), 2, "the change itself sees it");
            Ok(())
        })
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join(SOURCES_FILE)).unwrap();
        assert!(!text.contains("envcorp"), "{text}");
        assert_eq!(
            load_sources(dir.path()).unwrap().names(),
            vec!["acme".to_string()]
        );
    }

    #[test]
    fn a_file_from_a_newer_crystalline_is_refused_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(SOURCES_FILE), r#"{"v":2,"sources":[]}"#).unwrap();
        let err = load_sources(dir.path()).unwrap_err();
        assert!(err.to_string().contains("newer Crystalline"), "{err}");
        let err = update_sources(dir.path(), |_| Ok(())).unwrap_err();
        assert!(err.to_string().contains("newer Crystalline"), "{err}");
        let text = std::fs::read_to_string(dir.path().join(SOURCES_FILE)).unwrap();
        assert_eq!(text, r#"{"v":2,"sources":[]}"#, "left exactly as it was");
    }

    #[test]
    fn the_default_name_is_the_most_specific_label_that_says_something() {
        assert_eq!(
            default_source_name("https://crystalline.acme.com", &[]),
            "acme"
        );
        assert_eq!(default_source_name("https://acme.com", &[]), "acme");
        assert_eq!(
            default_source_name("https://kb.team.example.org", &[]),
            "example"
        );
        assert_eq!(
            default_source_name("https://crystalline.boehme.network", &[]),
            "boehme"
        );
        assert_eq!(
            default_source_name("https://Knowledge.ACME.io:8443", &[]),
            "acme"
        );
        assert_eq!(default_source_name("http://127.0.0.1:7411", &[]), "server");
        assert_eq!(default_source_name("http://localhost:7411", &[]), "server");
        assert_eq!(default_source_name("http://[::1]:7411", &[]), "server");
        assert_eq!(
            default_source_name("https://crystalline.com", &[]),
            "server"
        );
    }

    #[test]
    fn a_taken_default_name_counts_up() {
        let taken = vec!["acme".to_string(), "acme-2".to_string()];
        assert_eq!(default_source_name("https://kb.acme.com", &taken), "acme-3");
        assert_eq!(
            default_source_name("http://127.0.0.1:7412", &["server".to_string()]),
            "server-2"
        );
    }

    #[test]
    fn a_source_name_is_short_lower_case_and_never_local() {
        for good in ["acme", "acme-2", "a", "x1-y2"] {
            assert!(valid_source_name(good).is_ok(), "{good}");
        }
        for bad in [
            "",
            "Acme",
            "-acme",
            "acme-",
            "ac me",
            "a/b",
            "local",
            &"a".repeat(33),
        ] {
            assert!(valid_source_name(bad).is_err(), "{bad}");
        }
        assert!(
            valid_source_name("local").unwrap_err().contains("reserved"),
            "the word a late local domain is suffixed with"
        );
    }

    #[test]
    fn connecting_the_same_server_again_keeps_its_name_and_its_mounts() {
        let mut file = SourcesFile::default();
        let mut first = record("https://crystalline.acme.com", "acme");
        first.mounts.push(MountRecord {
            remote: "jordi".to_string(),
            local: "jordi-acme".to_string(),
        });
        file.upsert(first);
        let mut again = record("https://crystalline.acme.com/", "acme-2");
        again.account = "bob".to_string();
        let kept = file.upsert(again);
        assert_eq!(kept.name, "acme", "the name it was given stays");
        assert_eq!(kept.account, "bob", "the new sign-in wins");
        assert_eq!(kept.mounts.len(), 1, "the names it handed out stay");
        assert_eq!(file.sources.len(), 1);
    }

    #[test]
    fn a_source_is_found_by_its_name_or_its_url() {
        let mut file = SourcesFile::default();
        file.upsert(record("https://crystalline.acme.com", "acme"));
        file.upsert(record("https://kb.other.org", "other"));
        assert_eq!(file.find("acme").unwrap().name, "acme");
        assert_eq!(file.find("https://kb.other.org/").unwrap().name, "other");
        assert_eq!(file.find("https://KB.OTHER.ORG").unwrap().name, "other");
        assert!(file.find("nobody").is_none());
        assert_eq!(file.remove("other").unwrap().name, "other");
        assert_eq!(file.names(), vec!["acme".to_string()]);
    }

    #[test]
    fn the_environment_adds_one_source_only_with_both_variables() {
        let only_url = |name: &str| {
            (name == REMOTE_URL_ENV).then(|| "https://crystalline.acme.com".to_string())
        };
        assert!(
            env_source(only_url, &[]).is_none(),
            "a URL alone is not a source"
        );
        let both = |name: &str| match name {
            REMOTE_URL_ENV => Some(" https://crystalline.acme.com/ ".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_env".to_string()),
            _ => None,
        };
        let source = env_source(both, &["acme".to_string()]).unwrap();
        assert!(source.from_env);
        assert_eq!(source.url, "https://crystalline.acme.com");
        assert_eq!(
            source.name, "acme-2",
            "named like any other, never a duplicate"
        );
        assert_eq!(source.kind, CredentialKind::Token);
        assert!(source.mounts.is_empty());
    }

    /// Review focus 3: the CLI and the daemon both rewrite this file. Each
    /// write re-reads it under the lock, so none of them loses another's.
    #[test]
    fn two_writers_of_the_sources_file_never_lose_a_mount() {
        let dir = tempfile::tempdir().unwrap();
        update_sources(dir.path(), |file| {
            file.upsert(record("https://crystalline.acme.com", "acme"));
            Ok(())
        })
        .unwrap();
        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = dir.path().to_path_buf();
                std::thread::spawn(move || {
                    update_sources(&path, |file| {
                        let source = file.find_mut("acme").unwrap();
                        // Wide enough that two writers without a lock overlap.
                        std::thread::sleep(std::time::Duration::from_millis(20));
                        source.mounts.push(MountRecord {
                            remote: format!("d{i}"),
                            local: format!("d{i}"),
                        });
                        Ok(())
                    })
                    .unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }
        let file = load_sources(dir.path()).unwrap();
        assert_eq!(file.find("acme").unwrap().mounts.len(), 8, "{file:?}");
    }

    #[test]
    fn a_host_folder_is_the_server_folder_under_the_remote_folder() {
        let root = record("http://127.0.0.1:7411", "server");
        assert_eq!(
            root.host_dir(Path::new("/state/remote")),
            Path::new("/state/remote").join("127.0.0.1_7411")
        );
        let prefixed = record("https://example.com/crystalline", "example");
        assert_eq!(
            prefixed.host_dir(Path::new("/state/remote")),
            Path::new("/state/remote").join("example.com~crystalline")
        );
    }

    #[test]
    fn two_paths_on_one_host_are_two_sources() {
        let mut file = SourcesFile::default();
        file.upsert(record("https://example.com/crystalline", "example"));
        file.upsert(record("https://example.com/other", "example-2"));
        assert_eq!(file.sources.len(), 2, "the second never replaces the first");
        assert_eq!(
            file.find("https://example.com/crystalline/").unwrap().name,
            "example"
        );
        assert_eq!(
            file.find("https://example.com/other").unwrap().name,
            "example-2"
        );
        assert!(file.find("https://example.com").is_none());
    }

    #[test]
    fn the_default_name_is_the_host_word_whatever_the_path() {
        assert_eq!(
            default_source_name("https://kb.acme.com/crystalline", &[]),
            "acme"
        );
    }

    #[test]
    fn the_environment_source_keeps_its_path() {
        let env = |name: &str| match name {
            REMOTE_URL_ENV => Some("https://KB.acme.com/crystalline/".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_x".to_string()),
            _ => None,
        };
        assert_eq!(
            env_source(env, &[]).unwrap().url,
            "https://kb.acme.com/crystalline"
        );
    }

    #[test]
    fn an_environment_url_the_rules_refuse_keeps_its_0_23_0_spelling() {
        let env = |name: &str| match name {
            REMOTE_URL_ENV => Some("http://kb.internal:7411/".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_x".to_string()),
            _ => None,
        };
        assert_eq!(env_source(env, &[]).unwrap().url, "http://kb.internal:7411");
    }

    #[test]
    fn the_environment_warning_never_repeats_the_value() {
        for value in [
            "cmt_SECRET1",
            "https://kb.example/Cmt_SECRET2",
            "http://kb.internal/cmt_SECRET3",
            "https://kb.example\\cmt_SECRET4",
            "https://kb.example/cmt_SECRET5/../x",
            "https://kb.example/cmt_SECRET6?x=1",
        ] {
            let reason = env_url(value).unwrap_err();
            assert!(!reason.contains("SECRET"), "{value}: {reason}");
        }
        assert_eq!(
            env_url("https://kb.example/crystalline/").unwrap(),
            "https://kb.example/crystalline"
        );
        let reason = env_url("https://kb.example/Crystalline").unwrap_err();
        assert!(
            reason.contains("may use only lower-case letters, digits, '.', '_' and '-'"),
            "{reason}"
        );
    }

    /// The domain list is checked like the url: a token pasted into
    /// `--domains` or the variable is repeated by no refusal and by no
    /// warning, because a warning lands in the daemon log on every load.
    #[test]
    fn a_domain_list_refusal_never_repeats_the_value() {
        let long = format!("cmt_SECRET3{}", "0".repeat(64));
        for value in [
            "alpha,cmt_SECRET1 x".to_string(),
            "cmt_SECRET2,,beta".to_string(),
            format!("alpha,{long}"),
            "https://kb.example/cmt_SECRET4".to_string(),
            "cmt_SECRET5/../x".to_string(),
        ] {
            let reason = parse_domain_list(&value).unwrap_err();
            assert!(!reason.contains("SECRET"), "{value}: {reason}");
            let warning = env_domains(&value).unwrap_err();
            assert!(!warning.contains("SECRET"), "{value}: {warning}");
            assert!(warning.starts_with(REMOTE_DOMAINS_ENV), "{warning}");
        }
        let reason = parse_domain_list("alpha,Not A Name").unwrap_err();
        assert!(reason.contains("name 2"), "says which name: {reason}");
        assert!(
            reason.contains("use letters, digits"),
            "says the rule: {reason}"
        );
        assert_eq!(env_domains("beta, alpha").unwrap(), vec!["alpha", "beta"]);
    }

    #[test]
    fn a_domain_list_is_checked_sorted_and_deduplicated() {
        assert_eq!(
            parse_domain_list("gamma, alpha,beta,alpha").unwrap(),
            vec!["alpha", "beta", "gamma"]
        );
        for bad in ["", " , ", "alpha,,beta", "Bad Name", "../x"] {
            assert!(parse_domain_list(bad).is_err(), "{bad:?}");
        }
        let err = parse_domain_list("").unwrap_err();
        assert!(err.contains("at least one"), "{err}");
    }

    #[test]
    fn a_sources_file_from_0_23_0_loads_as_all_and_is_written_back_without_a_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(SOURCES_FILE),
            r#"{ "v": 1, "sources": [{ "url": "https://crystalline.acme.com", "name": "acme",
                 "account": "ada", "kind": "token", "connected_at": "2026-10-05T12:00:00Z",
                 "mounts": [{ "remote": "open", "local": "open" }] }] }"#,
        )
        .unwrap();
        let file = load_sources(dir.path()).unwrap();
        assert_eq!(file.sources[0].domains, None, "no field is all");
        update_sources(dir.path(), |_| Ok(())).unwrap();
        let written = std::fs::read_to_string(dir.path().join(SOURCES_FILE)).unwrap();
        assert!(
            !written.contains("\"domains\""),
            "no list is invented: {written}"
        );
    }

    #[test]
    fn a_list_round_trips_and_a_reconnect_record_carries_its_list() {
        let dir = tempfile::tempdir().unwrap();
        update_sources(dir.path(), |file| {
            let mut acme = record("https://crystalline.acme.com", "acme");
            acme.domains = Some(vec!["open".into()]);
            file.upsert(acme);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            load_sources(dir.path()).unwrap().sources[0].domains,
            Some(vec!["open".to_string()])
        );
        let mut file = load_sources(dir.path()).unwrap();
        let mut again = record("https://crystalline.acme.com", "acme");
        again.domains = None;
        file.upsert(again);
        assert_eq!(
            file.sources[0].domains, None,
            "upsert takes the list the caller decided"
        );
    }

    #[test]
    fn the_environment_list_sets_the_list_of_the_environment_source() {
        let env = |name: &str| match name {
            REMOTE_URL_ENV => Some("https://kb.acme.com".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_x".to_string()),
            REMOTE_DOMAINS_ENV => Some("beta,alpha".to_string()),
            _ => None,
        };
        assert_eq!(
            env_source(env, &[]).unwrap().domains,
            Some(vec!["alpha".to_string(), "beta".to_string()])
        );
        let empty = |name: &str| match name {
            REMOTE_DOMAINS_ENV => Some(String::new()),
            other => env(other),
        };
        assert_eq!(
            env_source(empty, &[]).unwrap().domains,
            None,
            "an empty variable is unset"
        );
    }

    #[test]
    fn a_bad_environment_list_leaves_the_environment_source_out() {
        let env = |name: &str| match name {
            REMOTE_URL_ENV => Some("https://kb.acme.com".to_string()),
            REMOTE_TOKEN_ENV => Some("cmt_x".to_string()),
            REMOTE_DOMAINS_ENV => Some("alpha,Not A Name".to_string()),
            _ => None,
        };
        assert!(
            env_source(env, &[]).is_none(),
            "never all instead of a list it could not read"
        );
    }
}
