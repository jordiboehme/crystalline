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

use crate::mounts::{Announcement, Hidden, HiddenReason, LocalDomain, Mount, MountTable, assign};
use crate::server_client::{Connection, ForwardedAgent, RemoteFailure};
use crate::source_cache::{
    Fetched, HOOK_STATUS_FILE, ROUTING_FILE, STALE_AFTER, cached_offers, fetch_cached, read_cached,
    stale_line, stale_or_missing,
};
use crate::sources::{
    REMOTE_TOKEN_ENV, SourceRecord, SourcesFile, env_source, load_sources, update_sources,
};

/// Whether an open connection still belongs to `record`: the same server,
/// account and sign-in. A disconnect and a new connect under the same name,
/// or a fresh sign-in, is another connection.
fn same_sign_in(open: &SourceRecord, record: &SourceRecord) -> bool {
    open.url == record.url
        && open.account == record.account
        && open.kind == record.kind
        && open.connected_at == record.connected_at
        && open.from_env == record.from_env
}

/// Open one source's connection on a blocking thread (one bounded keychain
/// read), within `limit`.
async fn open_connection(
    record: SourceRecord,
    dir: PathBuf,
    token: Option<String>,
    limit: Duration,
) -> Result<Arc<Connection>, RemoteFailure> {
    let name = record.name.clone();
    let opening = tokio::task::spawn_blocking(move || {
        Connection::open_with(record, &dir, move |var| {
            (var == REMOTE_TOKEN_ENV).then(|| token.clone()).flatten()
        })
    });
    match tokio::time::timeout(limit, opening).await {
        Ok(Ok(opened)) => opened.map(Arc::new),
        Ok(Err(e)) => Err(RemoteFailure::Credential(e.to_string())),
        Err(_) => Err(RemoteFailure::Credential(format!(
            "reading the sign-in for {name} on this machine did not finish in time"
        ))),
    }
}

/// What the mounts did to a local domain just registered, for the note of
/// its report. Kept typed until it is shown, so each caller gets the
/// rendering it may see: the machine owner the sentence with the source's
/// name, anyone else the same sentence naming no server (which servers this
/// machine is connected to is the owner's business).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MountNote {
    /// A source holds `wanted`, so the domain was registered as `name`.
    Renamed {
        /// The name asked for.
        wanted: String,
        /// The name it got.
        name: String,
        /// The source that holds `wanted`.
        source: String,
    },
    /// It is the same domain as a mount of `source`, so it is hidden.
    HiddenCopy {
        /// The local domain.
        name: String,
        /// The source whose mount hides it.
        source: String,
    },
    /// A different domain under a name `source` gave out first.
    Collision {
        /// The local domain.
        name: String,
        /// The source that gave the name out first.
        source: String,
    },
}

impl MountNote {
    /// The sentence, naming the source when `name_source` is set.
    pub fn render(&self, name_source: bool) -> String {
        match (self, name_source) {
            (
                MountNote::Renamed {
                    wanted,
                    name,
                    source,
                },
                true,
            ) => format!(
                "'{wanted}' is a domain from {source} on this machine, so this local domain is registered as '{name}'"
            ),
            (MountNote::Renamed { wanted, name, .. }, false) => format!(
                "'{wanted}' is a domain from a connected server on this machine, so this local domain is registered as '{name}'"
            ),
            (MountNote::HiddenCopy { name, source }, true) => format!(
                "the local domain '{name}' is hidden while {source} is connected; disconnect {source} to use it again"
            ),
            (MountNote::HiddenCopy { name, .. }, false) => format!(
                "the local domain '{name}' is hidden while the server it comes from is connected"
            ),
            (MountNote::Collision { name, source }, true) => Announcement::LocalShadowed {
                local: name.clone(),
                source: source.clone(),
            }
            .to_string(),
            (MountNote::Collision { name, .. }, false) => format!(
                "the local domain '{name}' has a name a connected server gave out first; it is hidden until you change its name in config.yaml"
            ),
        }
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
    table: Arc<MountTable>,
    failures: BTreeMap<String, String>,
}

/// The sources of this machine.
pub struct SourceSet {
    remote_dir: PathBuf,
    env_source: Option<SourceRecord>,
    env_token: Option<String>,
    inner: RwLock<Inner>,
    /// This machine's own domains. Apart from `inner`, so the save in
    /// [`SourceSet::refresh`] reads them as they are when it decides names.
    local: Arc<RwLock<Vec<LocalDomain>>>,
    /// Held while a table is computed, so two rebuilds never swap in an
    /// older table after a newer one. Readers of the table never take it.
    rebuilding: std::sync::Mutex<()>,
    /// Never held across an await.
    connections: std::sync::Mutex<BTreeMap<String, Arc<Connection>>>,
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
                table: Arc::new(MountTable::default()),
                failures: BTreeMap::new(),
            }),
            local: Arc::new(RwLock::new(local)),
            rebuilding: std::sync::Mutex::new(()),
            connections: std::sync::Mutex::new(BTreeMap::new()),
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
                table: Arc::new(MountTable::default()),
                failures: BTreeMap::new(),
            }),
            local: Arc::new(RwLock::new(Vec::new())),
            rebuilding: std::sync::Mutex::new(()),
            connections: std::sync::Mutex::new(BTreeMap::new()),
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

    fn local(&self) -> Vec<LocalDomain> {
        self.local.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn connections(&self) -> std::sync::MutexGuard<'_, BTreeMap<String, Arc<Connection>>> {
        self.connections.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The saved file with the environment's source after it.
    fn combined(&self, file: &SourcesFile) -> SourcesFile {
        let mut combined = file.clone();
        if let Some(env) = &self.env_source {
            combined.sources.push(env.clone());
        }
        combined
    }

    /// Recompute the table from the caches, without writing anything. The
    /// disk reads and the assignment run outside the table's lock, so a read
    /// that asks what is hidden never waits on them.
    ///
    /// A source with no usable cache (none yet, one this build cannot read,
    /// or one answered for another account) offers nothing, so it mounts
    /// nothing. Its reserved names still hold, though: every local domain
    /// under one of them is hidden until the source answers again, so a
    /// copy is never answered in the server's place while it is down
    /// (decision D19).
    fn rebuild(&self) -> Vec<Announcement> {
        let _one_at_a_time = self.rebuilding.lock().unwrap_or_else(|e| e.into_inner());
        let mut combined = self.combined(&self.read().file);
        let local = self.local();
        let offers = cached_offers(&combined, &self.remote_dir);
        let (mut table, said) = assign(&mut combined, &local, &offers);
        for source in &combined.sources {
            if offers.contains_key(&source.name) {
                continue;
            }
            for reserved in &source.mounts {
                if local.iter().any(|d| d.name == reserved.local)
                    && !table
                        .hidden(&reserved.local)
                        .any(|h| h.source == source.name)
                {
                    table.shadowed.push(Hidden {
                        local: reserved.local.clone(),
                        source: source.name.clone(),
                        reason: HiddenReason::Copy,
                        by: reserved.local.clone(),
                    });
                }
            }
        }
        self.write().table = Arc::new(table);
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

    /// The source that holds `local` on this machine: the one that mounts it,
    /// or the one whose `sources.json` record keeps it for a domain it does
    /// not offer right now. `None` when no source holds that name.
    pub fn holder_of(&self, local: &str) -> Option<String> {
        if let Some(source) = self.table().source_of(local) {
            return Some(source.to_string());
        }
        self.records()
            .into_iter()
            .find(|s| s.mounts.iter().any(|m| m.local == local))
            .map(|s| s.name)
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
        *self.local.write().unwrap_or_else(|e| e.into_inner()) = local;
        self.rebuild()
    }

    /// [`SourceSet::set_local`] when `local` differs from the domains the
    /// table was built with, `None` (and no rebuild) when it does not: what
    /// the daemon calls on every routed call and on its poller's cheap tick,
    /// so a hand edit of config.yaml reaches the table without a restart.
    pub fn set_local_if_changed(&self, local: Vec<LocalDomain>) -> Option<Vec<Announcement>> {
        if self.local() == local {
            return None;
        }
        Some(self.set_local(local))
    }

    /// [`SourceSet::reload`] when `sources.json` on disk differs from the
    /// file this set holds, `None` otherwise: a `connect` or `disconnect`
    /// that ran while the daemon could not be told, or a hand edit, reaches
    /// the running daemon on its poller's cheap tick. A file that cannot be
    /// read is left to the next full reload.
    pub fn reload_if_changed(&self) -> Option<Vec<Announcement>> {
        // [`SourceSet::empty`] has no folder to read.
        if self.remote_dir.as_os_str().is_empty() {
            return None;
        }
        let on_disk = load_sources(&self.remote_dir).ok()?;
        if on_disk == self.read().file {
            return None;
        }
        Some(self.reload())
    }

    /// `sources.json` changed under this process (a CLI `connect`,
    /// `disconnect` or rename): read it again and forget the connections of
    /// sources that are gone.
    pub fn reload(&self) -> Vec<Announcement> {
        let file = load_sources(&self.remote_dir).unwrap_or_else(|e| {
            tracing::warn!("the connected servers could not be read, so none are used: {e}");
            SourcesFile::default()
        });
        let records = self.combined(&file).sources;
        self.write().file = file;
        self.connections().retain(|name, open| {
            records
                .iter()
                .any(|r| &r.name == name && same_sign_in(open.source(), r))
        });
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
        if self
            .env_source
            .as_ref()
            .is_some_and(|e| e.name == mount.source)
        {
            return Err(format!(
                "'{local}' comes from {}, the server CRYSTALLINE_REMOTE_URL names, and its \
                 domains are named again at every start, so a new name would not last; \
                 connect that server with crystalline connect to keep a name",
                mount.source
            ));
        }
        let taken = self
            .local()
            .iter()
            .any(|d| d.name == new || d.aliases.iter().any(|a| a == new))
            || table.mount(new).is_some()
            || self
                .read()
                .file
                .sources
                .iter()
                .any(|s| s.mounts.iter().any(|m| m.local == new));
        if taken {
            return Err(format!(
                "the name '{new}' is taken on this machine; pick another"
            ));
        }
        crystalline_core::config::registration::validate_domain_name(new)?;
        let renamed = update_sources(&self.remote_dir, |file| {
            if let Some(source) = file.find_mut(&mount.source)
                && let Some(record) = source.mounts.iter_mut().find(|m| m.remote == mount.remote)
            {
                record.local = new.to_string();
                return Ok(true);
            }
            Ok(false)
        })
        .map_err(|e| e.to_string())?;
        self.reload();
        if renamed {
            Ok(())
        } else {
            Err(format!(
                "{} is no longer connected on this machine, so '{local}' was not renamed",
                mount.source
            ))
        }
    }

    /// The open connection to `record`, when it is still that sign-in's.
    fn open_for(&self, record: &SourceRecord) -> Option<Arc<Connection>> {
        self.connections()
            .get(&record.name)
            .filter(|open| same_sign_in(open.source(), record))
            .cloned()
    }

    /// Keep a connection just opened, replacing one of an earlier sign-in.
    fn keep(&self, opened: &Arc<Connection>) {
        self.connections()
            .insert(opened.source().name.clone(), opened.clone());
    }

    /// The connection to one source, opened on first use and opened again
    /// when the source's record changed (a new sign-in, or a disconnect and a
    /// connect under the same name). The keychain read runs on a blocking
    /// thread, within `limit`.
    async fn connection(
        &self,
        name: &str,
        limit: Duration,
    ) -> Result<Arc<Connection>, RemoteFailure> {
        let record = self.record(name).ok_or_else(|| {
            RemoteFailure::Refused(format!("no connected server is called {name}"))
        })?;
        if let Some(open) = self.open_for(&record) {
            return Ok(open);
        }
        let opened = open_connection(
            record,
            self.remote_dir.clone(),
            self.env_token.clone(),
            limit,
        )
        .await?;
        self.keep(&opened);
        Ok(opened)
    }

    /// Ask every source at once, each within `deadline`, opening the
    /// connections it needs inside that time too.
    async fn fetch_all(
        &self,
        cmd: &'static str,
        file: &'static str,
        deadline: Duration,
    ) -> Vec<(String, Fetched)> {
        let mut tasks = tokio::task::JoinSet::new();
        let mut names = BTreeMap::new();
        let records = self.records();
        for record in records.clone() {
            let open = self.open_for(&record);
            let dir = self.remote_dir.clone();
            let token = self.env_token.clone();
            let name = record.name.clone();
            let handle = tasks.spawn(async move {
                let (opened, fresh) = match open {
                    Some(open) => (Ok(open), false),
                    None => (
                        open_connection(record.clone(), dir.clone(), token, deadline).await,
                        true,
                    ),
                };
                let fetched = match &opened {
                    Ok(connection) => fetch_cached(connection, cmd, file, deadline).await,
                    Err(failure) => {
                        let host_dir = record.host_dir(&dir);
                        let cached = read_cached(&host_dir, file, &record.account);
                        stale_or_missing(&host_dir, file, cached, failure.clone())
                    }
                };
                let kept = opened.ok().filter(|_| fresh);
                (record.name, kept, fetched)
            });
            names.insert(handle.id(), name);
        }
        let mut out = Vec::new();
        while let Some(done) = tasks.join_next_with_id().await {
            match done {
                Ok((_, (name, kept, fetched))) => {
                    if let Some(opened) = kept {
                        self.keep(&opened);
                    }
                    out.push((name, fetched));
                }
                Err(e) => {
                    let name = names.get(&e.id()).cloned().unwrap_or_default();
                    tracing::warn!("asking {name} stopped before it answered: {e}");
                    out.push((
                        name.clone(),
                        Fetched::Missing(RemoteFailure::Refused(format!(
                            "asking {name} stopped before it answered"
                        ))),
                    ));
                }
            }
        }
        out.sort_by_key(|(name, _)| records.iter().position(|s| &s.name == name));
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
        let local = self.local.clone();
        let dir = self.remote_dir.clone();
        let env = self.env_source.clone();
        let saved = tokio::task::spawn_blocking(move || {
            update_sources(&dir, |file| {
                // Read under the sources lock, as the domains stand now.
                let local = local.read().unwrap_or_else(|e| e.into_inner()).clone();
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
        // A name is used only once it is written down (decision D13): when
        // the save failed, the table stays as it was.
        match saved {
            Ok(Ok(file)) => {
                self.write().file = file;
                self.rebuild();
            }
            Ok(Err(e)) => {
                tracing::warn!("the names of the connected servers could not be saved: {e}")
            }
            Err(e) => tracing::warn!("the names of the connected servers could not be saved: {e}"),
        }
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
        let started = std::time::Instant::now();
        let connection = self.connection(source, deadline).await?;
        let left = deadline.saturating_sub(started.elapsed());
        connection.tool_within(tool, args, agent, left).await
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

#[cfg(test)]
mod tests {
    use super::*;

    fn both(note: MountNote) -> (String, String) {
        (note.render(true), note.render(false))
    }

    #[test]
    fn a_renamed_note_names_the_source_only_for_the_owner() {
        let (owner, other) = both(MountNote::Renamed {
            wanted: "open".into(),
            name: "open-local".into(),
            source: "acme".into(),
        });
        assert_eq!(
            owner,
            "'open' is a domain from acme on this machine, so this local domain is registered as 'open-local'"
        );
        assert_eq!(
            other,
            "'open' is a domain from a connected server on this machine, so this local domain is registered as 'open-local'"
        );
    }

    #[test]
    fn a_hidden_copy_note_names_the_source_only_for_the_owner() {
        let (owner, other) = both(MountNote::HiddenCopy {
            name: "platform-local".into(),
            source: "acme".into(),
        });
        assert_eq!(
            owner,
            "the local domain 'platform-local' is hidden while acme is connected; disconnect acme to use it again"
        );
        assert_eq!(
            other,
            "the local domain 'platform-local' is hidden while the server it comes from is connected"
        );
    }

    #[test]
    fn a_collision_note_names_the_source_only_for_the_owner() {
        let (owner, other) = both(MountNote::Collision {
            name: "open".into(),
            source: "acme".into(),
        });
        assert_eq!(
            owner,
            "the local domain 'open' has a name acme gave out first; it is hidden until you change its name in config.yaml or disconnect acme"
        );
        assert_eq!(
            other,
            "the local domain 'open' has a name a connected server gave out first; it is hidden until you change its name in config.yaml"
        );
    }
}
