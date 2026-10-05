//! The sources of one machine at run time: the saved records plus the
//! environment's, the mount table over them, and one lazily opened
//! connection per source.
//!
//! The daemon holds one for its whole life (installed on its engine), and a
//! one-shot CLI command loads its own from disk; neither touches the network
//! to build the table, which comes from each source's cached routing model.
//! [`SourceSet::refresh`] is the only thing that asks the servers for their
//! domain lists, and it writes every name it decides into `sources.json`
//! before it uses it (decision D13).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

use crate::mounts::{Announcement, LocalDomain, Mount, MountTable, assign};
use crate::server_client::{Connection, ForwardedAgent, RemoteFailure};
use crate::source_cache::{
    Fetched, HOOK_STATUS_FILE, ROUTING_FILE, STALE_AFTER, cached_offers, fetch_cached, read_cached,
    stale_line, write_cached,
};
use crate::sources::{
    REMOTE_TOKEN_ENV, SourceRecord, SourcesFile, env_source, load_sources, update_sources,
};

/// What a fetch answers for a source whose connection could not be opened
/// (no credential, a keychain that failed): its cache served stale with the
/// failure recorded, so a reader with no daemon names it too.
fn not_opened(host_dir: &Path, file: &str, account: &str, failure: RemoteFailure) -> Fetched {
    match read_cached(host_dir, file, account) {
        Some(mut cached) => {
            cached.last_failure = Some(failure.to_string());
            let _ = write_cached(host_dir, file, &cached);
            Fetched::Stale { cached, failure }
        }
        None => Fetched::Missing(failure),
    }
}

/// The mounted part of the routing block, and one staleness line per source
/// whose part may be out of date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountedRouting {
    /// Every mounted domain, with its bullets.
    pub domains: Vec<Mount>,
    /// The notes, one per stale source.
    pub stale: Vec<String>,
}

struct Inner {
    file: SourcesFile,
    local: Vec<LocalDomain>,
    table: Arc<MountTable>,
    failures: BTreeMap<String, String>,
}

/// The sources of this machine.
pub struct SourceSet {
    remote_dir: PathBuf,
    env_source: Option<SourceRecord>,
    env_token: Option<String>,
    inner: RwLock<Inner>,
    connections: tokio::sync::Mutex<BTreeMap<String, Arc<Connection>>>,
}

impl SourceSet {
    /// The sources saved under `remote_dir`, plus the environment's, with the
    /// table built from their caches. A file this build cannot read is no
    /// sources and a warning, never a reason to stop.
    pub fn load(
        remote_dir: PathBuf,
        local: Vec<LocalDomain>,
        env: impl Fn(&str) -> Option<String>,
    ) -> SourceSet {
        let file = load_sources(&remote_dir).unwrap_or_else(|e| {
            tracing::warn!("the connected servers could not be read, so none are used: {e}");
            SourcesFile::default()
        });
        let env_source = env_source(&env, &file.names());
        let env_token = env(REMOTE_TOKEN_ENV).map(|t| t.trim().to_string());
        let set = SourceSet {
            remote_dir,
            env_source,
            env_token,
            inner: RwLock::new(Inner {
                file,
                local,
                table: Arc::new(MountTable::default()),
                failures: BTreeMap::new(),
            }),
            connections: tokio::sync::Mutex::new(BTreeMap::new()),
        };
        set.rebuild();
        set
    }

    /// No sources at all: what an engine with nothing connected behaves as.
    /// Built directly, so it never reads a `sources.json` anywhere.
    pub fn empty() -> SourceSet {
        SourceSet {
            remote_dir: PathBuf::new(),
            env_source: None,
            env_token: None,
            inner: RwLock::new(Inner {
                file: SourcesFile::default(),
                local: Vec::new(),
                table: Arc::new(MountTable::default()),
                failures: BTreeMap::new(),
            }),
            connections: tokio::sync::Mutex::new(BTreeMap::new()),
        }
    }

    /// `<state_dir>/remote`, or what the test gave.
    pub fn remote_dir(&self) -> &Path {
        &self.remote_dir
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, Inner> {
        self.inner.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, Inner> {
        self.inner.write().unwrap_or_else(|e| e.into_inner())
    }

    /// The saved file with the environment's source after it.
    fn combined(&self, file: &SourcesFile) -> SourcesFile {
        let mut combined = file.clone();
        if let Some(env) = &self.env_source {
            combined.sources.push(env.clone());
        }
        combined
    }

    /// Recompute the table from the caches, without writing anything.
    fn rebuild(&self) -> Vec<Announcement> {
        let mut inner = self.write();
        let mut combined = self.combined(&inner.file);
        let offers = cached_offers(&combined, &self.remote_dir);
        let (table, said) = assign(&mut combined, &inner.local, &offers);
        inner.table = Arc::new(table);
        said
    }

    /// The mount table as it stands.
    pub fn table(&self) -> Arc<MountTable> {
        self.read().table.clone()
    }

    /// The local domains hidden while their source is connected: a copy of
    /// a mounted domain, or a different local domain under a name a source
    /// gave out first. Built from the caches, so a source that is down keeps
    /// them hidden; a hidden copy is never an offline fallback.
    pub fn shadowed(&self) -> BTreeSet<String> {
        self.read()
            .table
            .shadowed
            .iter()
            .map(|h| h.local.clone())
            .collect()
    }

    /// Whether this machine has no source at all.
    pub fn is_empty(&self) -> bool {
        self.read().file.sources.is_empty() && self.env_source.is_none()
    }

    /// Every source, in connect order, the environment's last.
    pub fn records(&self) -> Vec<SourceRecord> {
        self.combined(&self.read().file).sources
    }

    /// One source by name.
    pub fn record(&self, name: &str) -> Option<SourceRecord> {
        self.records().into_iter().find(|s| s.name == name)
    }

    /// A source's server URL.
    pub fn source_url(&self, name: &str) -> Option<String> {
        self.record(name).map(|s| s.url)
    }

    /// The last failure of each source that has one, for `status`.
    pub fn failures(&self) -> BTreeMap<String, String> {
        self.read().failures.clone()
    }

    /// This machine's own domains changed: recompute who is shadowed.
    pub fn set_local(&self, local: Vec<LocalDomain>) -> Vec<Announcement> {
        self.write().local = local;
        self.rebuild()
    }

    /// `sources.json` changed under this process (a CLI `connect`,
    /// `disconnect` or rename): read it again and forget the connections of
    /// sources that are gone.
    pub fn reload(&self) -> Vec<Announcement> {
        let file = load_sources(&self.remote_dir).unwrap_or_else(|e| {
            tracing::warn!("the connected servers could not be read, so none are used: {e}");
            SourcesFile::default()
        });
        let names = self.combined(&file).names();
        self.write().file = file;
        if let Ok(mut connections) = self.connections.try_lock() {
            connections.retain(|name, _| names.contains(name));
        }
        self.rebuild()
    }

    /// `crystalline domain rename <local> <new> --local` on a mounted
    /// domain: the override is the source's own record of that name. Refused
    /// when `new` is taken here, by a local domain, an alias or another
    /// mount.
    pub fn rename_local(&self, local: &str, new: &str) -> Result<(), String> {
        let table = self.table();
        let Some(mount) = table.mount(local).cloned() else {
            return Err(format!("'{local}' is not a domain from a connected server"));
        };
        let taken = {
            let inner = self.read();
            inner
                .local
                .iter()
                .any(|d| d.name == new || d.aliases.iter().any(|a| a == new))
                || inner
                    .file
                    .sources
                    .iter()
                    .any(|s| s.mounts.iter().any(|m| m.local == new))
        };
        if taken {
            return Err(format!(
                "the name '{new}' is taken on this machine; pick another"
            ));
        }
        crystalline_core::config::registration::validate_domain_name(new)?;
        update_sources(&self.remote_dir, |file| {
            if let Some(source) = file.find_mut(&mount.source)
                && let Some(record) = source.mounts.iter_mut().find(|m| m.remote == mount.remote)
            {
                record.local = new.to_string();
            }
            Ok(())
        })
        .map_err(|e| e.to_string())?;
        self.reload();
        Ok(())
    }

    /// The connection to one source, opened on first use. The keychain read
    /// runs on a blocking thread.
    async fn connection(&self, name: &str) -> Result<Arc<Connection>, RemoteFailure> {
        let mut connections = self.connections.lock().await;
        if let Some(open) = connections.get(name) {
            return Ok(open.clone());
        }
        let record = self.record(name).ok_or_else(|| {
            RemoteFailure::Refused(format!("no connected server is called {name}"))
        })?;
        let dir = self.remote_dir.clone();
        let token = self.env_token.clone();
        let opened = tokio::task::spawn_blocking(move || {
            Connection::open_with(record, &dir, move |var| {
                (var == REMOTE_TOKEN_ENV).then(|| token.clone()).flatten()
            })
        })
        .await
        .map_err(|e| RemoteFailure::Credential(e.to_string()))??;
        let opened = Arc::new(opened);
        connections.insert(name.to_string(), opened.clone());
        Ok(opened)
    }

    async fn fetch_all(
        &self,
        cmd: &'static str,
        file: &'static str,
        deadline: Duration,
    ) -> Vec<(String, Fetched)> {
        let mut tasks = tokio::task::JoinSet::new();
        for record in self.records() {
            let dir = self.remote_dir.clone();
            let opened = self.connection(&record.name).await;
            tasks.spawn(async move {
                let fetched = match opened {
                    Ok(connection) => fetch_cached(&connection, cmd, file, deadline).await,
                    Err(failure) => {
                        not_opened(&record.host_dir(&dir), file, &record.account, failure)
                    }
                };
                (record.name, fetched)
            });
        }
        let mut out = Vec::new();
        while let Some(done) = tasks.join_next().await {
            if let Ok(pair) = done {
                out.push(pair);
            }
        }
        let order = self.records();
        out.sort_by_key(|(name, _)| order.iter().position(|s| &s.name == name));
        out
    }

    /// Ask every source for its routing model (conditionally, through its
    /// etag), each within `deadline` and all at once; write every name the
    /// answers decide into `sources.json`; rebuild the table. Answers what
    /// each source came back with.
    pub async fn refresh(&self, deadline: Duration) -> Vec<(String, Fetched)> {
        let fetched = self
            .fetch_all("routing_bullets", ROUTING_FILE, deadline)
            .await;
        {
            let mut inner = self.write();
            for (name, outcome) in &fetched {
                match outcome {
                    Fetched::Fresh(_) => {
                        inner.failures.remove(name);
                    }
                    Fetched::Stale { failure, .. } | Fetched::Missing(failure) => {
                        inner.failures.insert(name.clone(), failure.to_string());
                    }
                }
            }
        }
        let local = self.read().local.clone();
        let dir = self.remote_dir.clone();
        let env = self.env_source.clone();
        let saved = tokio::task::spawn_blocking(move || {
            update_sources(&dir, |file| {
                let mut combined = file.clone();
                if let Some(env) = &env {
                    combined.sources.push(env.clone());
                }
                let offers = cached_offers(&combined, &dir);
                let _ = assign(&mut combined, &local, &offers);
                if env.is_some() {
                    combined.sources.pop();
                }
                *file = combined.clone();
                Ok(combined)
            })
        })
        .await;
        match saved {
            Ok(Ok(file)) => self.write().file = file,
            Ok(Err(e)) => {
                tracing::warn!("the names of the connected servers could not be saved: {e}")
            }
            Err(e) => tracing::warn!("the names of the connected servers could not be saved: {e}"),
        }
        self.rebuild();
        fetched
    }

    /// Ask every source for its maintenance status, for the Stop hook's cache.
    pub async fn refresh_hook_status(&self, deadline: Duration) -> Vec<(String, Fetched)> {
        self.fetch_all("hook_status", HOOK_STATUS_FILE, deadline)
            .await
    }

    /// One call to one source, in that source's names, within `deadline`
    /// overall. A source inside its down window answers its failure at once.
    pub async fn forward(
        &self,
        source: &str,
        tool: &str,
        args: Value,
        agent: &ForwardedAgent,
        deadline: Duration,
    ) -> Result<Value, RemoteFailure> {
        let connection = self.connection(source).await?;
        connection.tool_within(tool, args, agent, deadline).await
    }

    /// The mounted part of the routing block from the caches alone, for a
    /// hook with no daemon to ask (decision D20).
    pub fn mounted_routing_from_cache(&self) -> MountedRouting {
        let table = self.table();
        let mut stale = Vec::new();
        for record in self.records() {
            match read_cached(
                &record.host_dir(&self.remote_dir),
                ROUTING_FILE,
                &record.account,
            ) {
                Some(cached) => {
                    let old = Utc::now()
                        .signed_duration_since(cached.fetched_at)
                        .to_std()
                        .is_ok_and(|age| age > STALE_AFTER);
                    if cached.last_failure.is_some() || old {
                        stale.push(stale_line(
                            &record.name,
                            &record.url,
                            cached.fetched_at,
                            cached.last_failure.as_deref(),
                        ));
                    }
                }
                None => stale.push(format!(
                    "Note: {} ({}) has not answered yet, so its domains are not listed here.",
                    record.name, record.url
                )),
            }
        }
        MountedRouting {
            domains: table.mounts.clone(),
            stale,
        }
    }
}
