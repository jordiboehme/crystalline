//! The mount table: every domain this machine offers, with its one local
//! name and its one source.
//!
//! [`assign`] is the whole of the naming rule (decision D13), and it is pure:
//! it reads the saved sources (with every name each of them handed out), the
//! local domains and each source's current domain list, and answers the table
//! plus the sentences worth telling the person. New names it decides are
//! written into the sources it was handed, so the caller persists them and
//! the next call reads them back instead of deciding again.
//!
//! [`MountTable::route`] is the routing rule (D21, D22) over the names a call
//! carries, and [`translate_answer`] is the inbound half of the address
//! translation (D18). The outbound half runs on the engine's own params
//! (`DomainArgs::localize_domains`), in the service crate.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::sources::{MountRecord, SourcesFile};

/// What makes two copies of a team domain the same domain: the repository it
/// tracks, the folder in it and the branch, on one forge.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OriginIdentity {
    /// The forge host, `github.com` or a GitHub Enterprise host.
    pub forge: String,
    /// `owner/name`, lower case.
    pub repository: String,
    /// The folder inside the repository, no leading or trailing slash; empty
    /// for the repository root.
    #[serde(default)]
    pub path: String,
    /// The branch.
    pub branch: String,
}

impl OriginIdentity {
    /// The identity of a local domain's origin, on the forge `api_url` names
    /// (absent is GitHub.com).
    pub fn of(
        origin: &crystalline_core::config::OriginConfig,
        api_url: Option<&str>,
    ) -> OriginIdentity {
        let base = crate::github::auth::auth_base(api_url);
        OriginIdentity {
            forge: base,
            repository: origin.repo.clone(),
            path: origin.path.clone().unwrap_or_default(),
            branch: origin.branch().to_string(),
        }
        .normalized()
    }

    /// This identity in the one spelling two of them are compared in.
    pub fn normalized(&self) -> OriginIdentity {
        let forge = self.forge.trim();
        let forge = forge.split_once("://").map_or(forge, |(_, rest)| rest);
        OriginIdentity {
            forge: forge.trim_end_matches('/').to_ascii_lowercase(),
            repository: self
                .repository
                .trim()
                .trim_matches('/')
                .to_ascii_lowercase(),
            path: self.path.trim().trim_matches('/').to_string(),
            branch: self.branch.trim().to_string(),
        }
    }

    /// Whether two identities name the same domain.
    pub fn same_as(&self, other: &OriginIdentity) -> bool {
        self.normalized() == other.normalized()
    }
}

/// One domain a source offers, as its routing model names it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteDomain {
    /// Its name on the server.
    pub name: String,
    /// Its routing bullets.
    #[serde(default)]
    pub bullets: Vec<String>,
    /// Its origin identity, when it tracks a repository.
    #[serde(default)]
    pub origin: Option<OriginIdentity>,
}

/// One domain registered on this machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LocalDomain {
    /// Its local name.
    pub name: String,
    /// Its machine-local former names, still accepted as input.
    pub aliases: Vec<String>,
    /// Its origin identity, when it is a team domain.
    pub origin: Option<OriginIdentity>,
}

/// One mounted domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mount {
    /// Its name on this machine.
    pub local: String,
    /// Its name on its server.
    pub remote: String,
    /// The source's name.
    pub source: String,
    /// Its routing bullets, from the source's routing model.
    pub bullets: Vec<String>,
    /// Whether it is the same domain as a local copy (same origin), which
    /// is hidden while this source is connected. True whatever name either
    /// of them has; the hidden copy is in [`MountTable::shadowed`] with
    /// [`HiddenReason::Copy`].
    pub replaces_local: bool,
}

/// A domain a source offers that is not mounted, because an earlier source
/// already offers the same domain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Skipped {
    /// The source that offers it again.
    pub source: String,
    /// Its name there.
    pub remote: String,
    /// The source whose copy is mounted.
    pub kept_by: String,
}

/// Something about the names worth telling the person.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Announcement {
    /// A domain got a local name of its own because its name was taken.
    Renamed {
        /// The source.
        source: String,
        /// Its name on the server.
        remote: String,
        /// Its name here.
        local: String,
    },
    /// A server's copy replaces the local copy of the same domain.
    ReplacesLocal {
        /// The source.
        source: String,
        /// The shared name.
        local: String,
    },
    /// A domain an earlier source already offers is left out.
    Skipped {
        /// The later source.
        source: String,
        /// Its name there.
        remote: String,
        /// The source whose copy is mounted.
        kept_by: String,
    },
    /// A different local domain holds a name a source gave out first (a
    /// hand edit of config.yaml gave it that name after the mount held it,
    /// decision D13). The mount keeps the name, and the local domain is
    /// hidden until its name changes in config.yaml or the source is
    /// disconnected. Not the hidden copy of the same domain, which is
    /// [`Announcement::ReplacesLocal`].
    LocalShadowed {
        /// The local domain.
        local: String,
        /// The source that gave the name out first.
        source: String,
    },
}

impl std::fmt::Display for Announcement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Announcement::Renamed {
                source,
                remote,
                local,
            } => write!(
                f,
                "{source}: the domain '{remote}' is called '{local}' on this machine, because '{remote}' was already taken"
            ),
            Announcement::ReplacesLocal { source, local } => write!(
                f,
                "{source}: '{local}' is the same domain as your local '{local}'; the server's copy is used while you are connected, and your local copy stays untouched and comes back on disconnect"
            ),
            Announcement::Skipped {
                source,
                remote,
                kept_by,
            } => write!(
                f,
                "{source}: '{remote}' is the same domain {kept_by} already offers; it is left out"
            ),
            Announcement::LocalShadowed { local, source } => write!(
                f,
                "the local domain '{local}' has a name {source} gave out first; it is hidden until you change its name in config.yaml or disconnect {source}"
            ),
        }
    }
}

/// Why a local domain is hidden.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HiddenReason {
    /// It is the same domain (same origin) as a mount of the source: the
    /// server's copy is used while the source is connected, and the local
    /// copy comes back on disconnect.
    Copy,
    /// It is a different domain whose name the source gave out first: it is
    /// hidden until its name changes in config.yaml or the source is
    /// disconnected (D13).
    Collision,
}

/// One local domain that is hidden, the source that hides it and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hidden {
    /// The local domain's name.
    pub local: String,
    /// The source whose mount hides it.
    pub source: String,
    /// Why.
    pub reason: HiddenReason,
    /// The local name of the mount that hides it: the same as `local` for a
    /// collision, and for a copy whatever name the mount has.
    pub by: String,
}

/// Every domain this machine offers from a source, and the local copies
/// that are hidden.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MountTable {
    /// The source names, in connect order.
    pub sources: Vec<String>,
    /// The mounted domains, by source in connect order, by remote name inside.
    pub mounts: Vec<Mount>,
    /// The domains left out because an earlier source offers them.
    pub skipped: Vec<Skipped>,
    /// Local domains hidden while their source is connected, one row per
    /// local domain, source and reason, in assignment order. A local domain
    /// can have two rows (a collision with one source and a copy on
    /// another), and it is visible again only when every row is gone.
    pub shadowed: Vec<Hidden>,
}

/// How a tool reaches domains, for [`MountTable::route`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolShape {
    /// One domain: read, write, edit, split, delete, browse, validate,
    /// infer_schema, vocabulary.
    OneDomain,
    /// Every domain unless a `domains` filter narrows it: search,
    /// recent_activity, list_domains, evolve_engrams.
    AllDomains,
    /// `build_context`: the anchor's domain first, then a filter.
    Anchored,
    /// `move_engram`: the source domain and the destination.
    Move,
    /// A collaboration tool: share, update, origin status, resolve,
    /// withdraw, discard.
    Collab,
    /// A tool that acts on this machine itself.
    LocalOnly,
}

/// Where a call goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// To this machine's engine, exactly as before.
    Local,
    /// To one source only.
    Remote {
        /// The source's name.
        source: String,
    },
    /// To the local engine (when `local`) and these sources, in parallel.
    FanOut {
        /// Whether the local engine answers a part.
        local: bool,
        /// The sources asked, in connect order.
        sources: Vec<String>,
    },
    /// Not at all, with the sentence that says why.
    Refused(String),
}

/// The names of one source's mounted domains, both ways.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NameMap {
    /// The source.
    pub source: String,
    /// Local name to remote name.
    pub to_remote: BTreeMap<String, String>,
    /// Remote name to local name.
    pub to_local: BTreeMap<String, String>,
}

impl NameMap {
    /// The remote name of a local one this source mounts.
    pub fn remote_of(&self, local: &str) -> Option<&str> {
        self.to_remote.get(local).map(String::as_str)
    }
}

impl MountTable {
    /// The mount a local name names.
    pub fn mount(&self, local: &str) -> Option<&Mount> {
        self.mounts.iter().find(|m| m.local == local)
    }

    /// Whether a local domain is hidden.
    pub fn is_hidden(&self, local: &str) -> bool {
        self.shadowed.iter().any(|h| h.local == local)
    }

    /// Why a local domain is hidden: every row for it.
    pub fn hidden<'a>(&'a self, local: &'a str) -> impl Iterator<Item = &'a Hidden> {
        self.shadowed.iter().filter(move |h| h.local == local)
    }

    /// The source a local name comes from, `None` for a local domain.
    pub fn source_of(&self, local: &str) -> Option<&str> {
        self.mount(local).map(|m| m.source.as_str())
    }

    /// Every mount of one source.
    pub fn of_source<'a>(&'a self, source: &'a str) -> impl Iterator<Item = &'a Mount> {
        self.mounts.iter().filter(move |m| m.source == source)
    }

    /// One source's names, both ways.
    pub fn names(&self, source: &str) -> NameMap {
        let mut map = NameMap {
            source: source.to_string(),
            ..NameMap::default()
        };
        for mount in self.of_source(source) {
            map.to_remote
                .insert(mount.local.clone(), mount.remote.clone());
            map.to_local
                .insert(mount.remote.clone(), mount.local.clone());
        }
        map
    }

    /// The sources that mount at least one domain, in connect order.
    fn live_sources(&self) -> Vec<String> {
        self.sources
            .iter()
            .filter(|s| self.mounts.iter().any(|m| &m.source == *s))
            .cloned()
            .collect()
    }

    /// Where a call to `tool` that names `named` goes. A name no source
    /// mounts is local, so the engine answers an unknown one in its own words.
    pub fn route(&self, tool: &str, shape: ToolShape, named: &[String]) -> Route {
        let mounted: Vec<&Mount> = named.iter().filter_map(|n| self.mount(n)).collect();
        match shape {
            ToolShape::LocalOnly => match mounted.first() {
                Some(m) => Route::Refused(format!(
                    "'{}' comes from {}; {tool} acts on this machine's own domains. Disconnect the source with crystalline disconnect {}",
                    m.local, m.source, m.source
                )),
                None => Route::Local,
            },
            ToolShape::Collab => match mounted.first() {
                Some(m) => Route::Refused(format!(
                    "'{}' comes from {}, which shares this domain; share changes there",
                    m.local, m.source
                )),
                None => Route::Local,
            },
            ToolShape::OneDomain | ToolShape::Anchored => {
                match named.first().and_then(|n| self.mount(n)) {
                    Some(m) => Route::Remote {
                        source: m.source.clone(),
                    },
                    None => Route::Local,
                }
            }
            ToolShape::Move => {
                let ends: Vec<Option<&str>> = named.iter().map(|n| self.source_of(n)).collect();
                match ends.as_slice() {
                    [] => Route::Local,
                    [first, rest @ ..] if rest.iter().all(|e| e == first) => match first {
                        Some(source) => Route::Remote {
                            source: source.to_string(),
                        },
                        None => Route::Local,
                    },
                    _ => Route::Refused(format!(
                        "{tool} cannot move an engram between sources: read it, write it into the destination, then delete it"
                    )),
                }
            }
            ToolShape::AllDomains => {
                if named.is_empty() {
                    // No source mounts anything right now (down at start
                    // with no cache, or offering nothing): a local answer,
                    // byte for byte what it was, not a merge of one part.
                    let sources = self.live_sources();
                    if sources.is_empty() {
                        return Route::Local;
                    }
                    return Route::FanOut {
                        local: true,
                        sources,
                    };
                }
                let local = named.iter().any(|n| self.mount(n).is_none());
                let sources: Vec<String> = self
                    .sources
                    .iter()
                    .filter(|s| mounted.iter().any(|m| &m.source == *s))
                    .cloned()
                    .collect();
                match (local, sources.as_slice()) {
                    (true, []) => Route::Local,
                    (false, [only]) => Route::Remote {
                        source: only.clone(),
                    },
                    _ => Route::FanOut { local, sources },
                }
            }
        }
    }
}

/// Build the mount table (decision D13). `sources` is updated in place with
/// every name decided for the first time; persist it afterwards.
///
/// The same domain (same origin) is mounted once: an established mount (one
/// whose source already handed out a name for it) keeps serving it, and
/// only among sources offering it for the first time does the one
/// connected first win. Every other copy is skipped with a note.
pub fn assign(
    sources: &mut SourcesFile,
    local: &[LocalDomain],
    remote: &BTreeMap<String, Vec<RemoteDomain>>,
) -> (MountTable, Vec<Announcement>) {
    let local_names: BTreeSet<String> = local.iter().map(|d| d.name.clone()).collect();
    let mut taken: BTreeSet<String> = local_names.clone();
    for domain in local {
        taken.extend(domain.aliases.iter().cloned());
    }
    // Every name a source handed out is reserved before anything new is
    // decided, whether or not its domain is offered right now.
    let mut mount_names: BTreeSet<String> = BTreeSet::new();
    for source in &sources.sources {
        mount_names.extend(source.mounts.iter().map(|m| m.local.clone()));
    }
    taken.extend(mount_names.iter().cloned());
    // Established mounts claim their origin first, in connect order, so a
    // domain never moves to another source on its own.
    let mut claimed: Vec<(OriginIdentity, String, String)> = Vec::new();
    for source in &sources.sources {
        let Some(offered) = remote.get(&source.name) else {
            continue;
        };
        for domain in offered {
            if let Some(identity) = &domain.origin
                && source.mounts.iter().any(|m| m.remote == domain.name)
                && !claimed.iter().any(|(id, _, _)| id.same_as(identity))
            {
                claimed.push((identity.clone(), source.name.clone(), domain.name.clone()));
            }
        }
    }
    let mut table = MountTable {
        sources: sources.names(),
        ..MountTable::default()
    };
    let mut said = Vec::new();
    for source in sources.sources.iter_mut() {
        let Some(offered) = remote.get(&source.name) else {
            continue;
        };
        let mut offered = offered.clone();
        offered.sort_by(|a, b| a.name.cmp(&b.name));
        for domain in offered {
            if let Some(identity) = &domain.origin
                && let Some((_, kept_by, _)) = claimed.iter().find(|(id, by, remote)| {
                    id.same_as(identity) && (by != &source.name || remote != &domain.name)
                })
            {
                said.push(Announcement::Skipped {
                    source: source.name.clone(),
                    remote: domain.name.clone(),
                    kept_by: kept_by.clone(),
                });
                table.skipped.push(Skipped {
                    source: source.name.clone(),
                    remote: domain.name.clone(),
                    kept_by: kept_by.clone(),
                });
                continue;
            }
            let copy = domain.origin.as_ref().and_then(|identity| {
                local
                    .iter()
                    .find(|l| l.origin.as_ref().is_some_and(|o| o.same_as(identity)))
            });
            let persisted = source
                .mounts
                .iter()
                .find(|m| m.remote == domain.name)
                .map(|m| m.local.clone());
            let name = match persisted {
                Some(name) => name,
                None => {
                    let name = match copy {
                        // The copy's name, unless a mount already holds it.
                        Some(copy) if !mount_names.contains(&copy.name) => copy.name.clone(),
                        _ if !taken.contains(&domain.name) => domain.name.clone(),
                        _ => {
                            let base = format!("{}-{}", domain.name, source.name);
                            let name = if taken.contains(&base) {
                                (2..)
                                    .map(|n| format!("{base}-{n}"))
                                    .find(|n| !taken.contains(n))
                                    .expect("an unbounded count finds a free name")
                            } else {
                                base
                            };
                            said.push(Announcement::Renamed {
                                source: source.name.clone(),
                                remote: domain.name.clone(),
                                local: name.clone(),
                            });
                            name
                        }
                    };
                    if copy.is_some() {
                        said.push(Announcement::ReplacesLocal {
                            source: source.name.clone(),
                            local: name.clone(),
                        });
                    }
                    source.mounts.push(MountRecord {
                        remote: domain.name.clone(),
                        local: name.clone(),
                    });
                    taken.insert(name.clone());
                    mount_names.insert(name.clone());
                    name
                }
            };
            if let Some(copy) = copy {
                table.shadowed.push(Hidden {
                    local: copy.name.clone(),
                    source: source.name.clone(),
                    reason: HiddenReason::Copy,
                    by: name.clone(),
                });
            }
            // A different local domain under the mount's name (a hand edit
            // of config.yaml): the mount keeps the name.
            if local_names.contains(&name) && copy.is_none_or(|c| c.name != name) {
                table.shadowed.push(Hidden {
                    local: name.clone(),
                    source: source.name.clone(),
                    reason: HiddenReason::Collision,
                    by: name.clone(),
                });
                said.push(Announcement::LocalShadowed {
                    local: name.clone(),
                    source: source.name.clone(),
                });
            }
            if let Some(identity) = &domain.origin
                && !claimed.iter().any(|(id, _, _)| id.same_as(identity))
            {
                claimed.push((identity.clone(), source.name.clone(), domain.name.clone()));
            }
            table.mounts.push(Mount {
                replaces_local: copy.is_some(),
                local: name,
                remote: domain.name,
                source: source.name.clone(),
                bullets: domain.bullets,
            });
        }
    }
    (table, said)
}

/// The keys whose subtree is somebody's text, never walked.
const TEXT_KEYS: &[&str] = &[
    "content",
    "snippet",
    "frontmatter",
    "observations",
    "text",
    "finding",
    "evidence",
    "fix",
    "line_text",
    "counterpart_line_text",
    "markdown",
    "body",
];

/// The keys whose subtree points at the server's own Fluid or carries the
/// server's own words, never rewritten.
const SERVER_KEYS: &[&str] = &["web_url", "web_url_template", "error", "web_url_note"];

/// Keys whose string value is a domain name.
const NAME_KEYS: &[&str] = &[
    "domain",
    "destination_domain",
    "canonical_name",
    "src_domain",
];

/// Keys whose array of strings are domain names.
const NAME_LIST_KEYS: &[&str] = &["domains", "aliases", "pending_domains"];

/// Rewrite every structured field of a source's answer from the source's
/// names to this machine's (decision D18). A name the source does not mount
/// here is left as it is.
pub fn translate_answer(value: &mut Value, names: &NameMap) {
    walk(value, names);
}

fn walk(value: &mut Value, names: &NameMap) {
    match value {
        Value::Object(map) => {
            // A domain listing row names itself under `name`.
            let is_domain_row = map.contains_key("canonical_name");
            for (k, v) in map.iter_mut() {
                if TEXT_KEYS.contains(&k.as_str()) || SERVER_KEYS.contains(&k.as_str()) {
                    continue;
                }
                if NAME_KEYS.contains(&k.as_str()) || (is_domain_row && k == "name") {
                    if let Value::String(s) = v
                        && let Some(local) = names.to_local.get(s.as_str())
                    {
                        *s = local.clone();
                    }
                    continue;
                }
                if NAME_LIST_KEYS.contains(&k.as_str())
                    && let Value::Array(items) = v
                    && items.iter().all(Value::is_string)
                {
                    for item in items.iter_mut() {
                        if let Value::String(s) = item
                            && let Some(local) = names.to_local.get(s.as_str())
                        {
                            *s = local.clone();
                        }
                    }
                    continue;
                }
                walk(v, names);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, names);
            }
        }
        Value::String(s) => *s = translate_addresses(s, names),
        _ => {}
    }
}

/// Every `crystalline://<remote>/...` and a bare `crystalline://<remote>` in
/// `text` with the local name, and nothing else.
fn translate_addresses(text: &str, names: &NameMap) -> String {
    map_authorities(text, &names.to_local)
}

/// `address` with its authority mapped through `map` (local name to the
/// source's name, for a call on its way out) when it is a `crystalline://`
/// address whose authority `map` names; unchanged otherwise.
pub fn translate_address_to(address: &str, map: &BTreeMap<String, String>) -> String {
    map_authorities(address, map)
}

/// Every `crystalline://` authority in `text` that `map` names, replaced by
/// what it maps to. The authority is the run of domain characters right
/// after the scheme, with a trailing `.` left out, the way relink reads it
/// ([`crystalline_core::relink::is_domain_char`]): `crystalline://a-b` is
/// never taken for `a`, and `crystalline://a, ...` or `crystalline://a.` in
/// prose is `a`.
fn map_authorities(text: &str, map: &BTreeMap<String, String>) -> String {
    const SCHEME: &str = "crystalline://";
    if !text.contains(SCHEME) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find(SCHEME) {
        out.push_str(&rest[..at + SCHEME.len()]);
        let tail = &rest[at + SCHEME.len()..];
        let run = tail
            .find(|c: char| !crystalline_core::relink::is_domain_char(c))
            .unwrap_or(tail.len());
        let authority = tail[..run].trim_end_matches('.');
        match map.get(authority) {
            Some(mapped) => out.push_str(mapped),
            None => out.push_str(authority),
        }
        rest = &tail[authority.len()..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server_token::CredentialKind;
    use crate::sources::{MountRecord, SourceRecord, SourcesFile};
    use serde_json::json;

    fn source(name: &str) -> SourceRecord {
        SourceRecord {
            url: format!("https://crystalline.{name}.com"),
            name: name.to_string(),
            account: "ada".to_string(),
            kind: CredentialKind::Token,
            token_endpoint: None,
            revocation_endpoint: None,
            connected_at: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            mounts: Vec::new(),
            from_env: false,
        }
    }

    fn origin(repo: &str) -> Option<OriginIdentity> {
        Some(OriginIdentity {
            forge: "github.com".to_string(),
            repository: repo.to_string(),
            path: String::new(),
            branch: "main".to_string(),
        })
    }

    fn remote(name: &str, repo: Option<&str>) -> RemoteDomain {
        RemoteDomain {
            name: name.to_string(),
            bullets: vec![format!("Route here for {name} questions")],
            origin: repo.and_then(origin),
        }
    }

    fn local(name: &str, repo: Option<&str>) -> LocalDomain {
        LocalDomain {
            name: name.to_string(),
            aliases: Vec::new(),
            origin: repo.and_then(origin),
        }
    }

    fn served(pairs: &[(&str, Vec<RemoteDomain>)]) -> BTreeMap<String, Vec<RemoteDomain>> {
        pairs
            .iter()
            .map(|(n, d)| (n.to_string(), d.clone()))
            .collect()
    }

    fn names(table: &MountTable) -> Vec<(String, String, String)> {
        table
            .mounts
            .iter()
            .map(|m| (m.source.clone(), m.remote.clone(), m.local.clone()))
            .collect()
    }

    #[test]
    fn an_origin_identity_ignores_case_slashes_and_the_default_branch() {
        let config = crystalline_core::config::OriginConfig {
            repo: "/Acme/Platform/".to_string(),
            path: Some("/docs/".to_string()),
            branch: None,
            poll_secs: None,
        };
        let id = OriginIdentity::of(&config, None);
        assert_eq!(id.forge, "github.com");
        assert_eq!(id.repository, "acme/platform");
        assert_eq!(id.path, "docs");
        assert_eq!(id.branch, "main");
        let ghes = OriginIdentity::of(&config, Some("https://GitHub.Acme.example/api/v3"));
        assert_eq!(ghes.forge, "github.acme.example");
        assert!(!id.same_as(&ghes), "another forge is another repository");
        let from_wire = OriginIdentity {
            forge: "GitHub.com".to_string(),
            repository: "acme/Platform".to_string(),
            path: "docs/".to_string(),
            branch: "main".to_string(),
        };
        assert!(
            id.same_as(&from_wire),
            "a server's spelling is normalized too"
        );
    }

    #[test]
    fn local_domains_come_first_and_a_taken_name_gets_the_source_suffix() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        let (table, said) = assign(
            &mut file,
            &[local("jordi", None)],
            &served(&[(
                "acme",
                vec![remote("jordi", None), remote("runbooks", None)],
            )]),
        );
        assert_eq!(
            names(&table),
            vec![
                ("acme".into(), "jordi".into(), "jordi-acme".into()),
                ("acme".into(), "runbooks".into(), "runbooks".into()),
            ]
        );
        assert_eq!(
            said,
            vec![Announcement::Renamed {
                source: "acme".into(),
                remote: "jordi".into(),
                local: "jordi-acme".into(),
            }]
        );
        assert_eq!(
            said[0].to_string(),
            "acme: the domain 'jordi' is called 'jordi-acme' on this machine, because 'jordi' was already taken"
        );
        assert_eq!(
            file.sources[0].mounts.len(),
            2,
            "every assignment is persisted, not only the renamed one"
        );
        assert!(table.shadowed.is_empty());
    }

    #[test]
    fn an_alias_and_a_suffixed_name_are_taken_too() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        let mut mine = local("jordi", None);
        mine.aliases.push("jordi-acme".to_string());
        let (table, _) = assign(
            &mut file,
            &[mine],
            &served(&[("acme", vec![remote("jordi", None)])]),
        );
        assert_eq!(table.mounts[0].local, "jordi-acme-2");
    }

    /// Review focus 2: whoever comes later gets the suffix, and a name once
    /// given is read back, never decided again.
    #[test]
    fn a_name_never_changes_when_a_later_source_or_local_domain_brings_it() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        let acme = vec![remote("jordi", None)];
        let (table, _) = assign(&mut file, &[], &served(&[("acme", acme.clone())]));
        assert_eq!(table.mounts[0].local, "jordi");

        // A second server brings the same name: it gets the suffix.
        file.sources.push(source("beta"));
        let both = served(&[
            ("acme", acme.clone()),
            ("beta", vec![remote("jordi", None)]),
        ]);
        let (table, _) = assign(&mut file, &[], &both);
        assert_eq!(
            names(&table),
            vec![
                ("acme".into(), "jordi".into(), "jordi".into()),
                ("beta".into(), "jordi".into(), "jordi-beta".into()),
            ]
        );

        // A local domain registered later under the mounted name is the
        // latecomer: the mount keeps the name and the local one is shadowed.
        let (table, said) = assign(&mut file, &[local("jordi", None)], &both);
        assert_eq!(table.mounts[0].local, "jordi", "the first name stands");
        assert_eq!(
            table.hidden("jordi").collect::<Vec<_>>(),
            vec![&Hidden {
                local: "jordi".into(),
                source: "acme".into(),
                reason: HiddenReason::Collision,
                by: "jordi".into(),
            }]
        );
        assert!(said.contains(&Announcement::LocalShadowed {
            local: "jordi".into(),
            source: "acme".into(),
        }));
        assert_eq!(
            Announcement::LocalShadowed {
                local: "jordi".into(),
                source: "acme".into(),
            }
            .to_string(),
            "the local domain 'jordi' has a name acme gave out first; it is hidden until you change its name in config.yaml or disconnect acme",
            "no rename command is offered (ruling F8 revised)"
        );

        // beta's domain disappears and comes back: same name again, and
        // nothing else can take it in between.
        let (table, _) = assign(
            &mut file,
            &[],
            &served(&[("acme", acme.clone()), ("beta", vec![])]),
        );
        assert_eq!(
            table.mounts.len(),
            1,
            "a domain that is gone is not mounted"
        );
        file.sources.push(source("gamma"));
        let three = served(&[
            ("acme", acme.clone()),
            ("beta", vec![]),
            ("gamma", vec![remote("jordi-beta", None)]),
        ]);
        let (table, _) = assign(&mut file, &[], &three);
        assert_eq!(
            table.mount("jordi-beta-gamma").map(|m| m.source.as_str()),
            Some("gamma"),
            "the reserved name is still beta's"
        );
        let (table, _) = assign(&mut file, &[], &both);
        assert_eq!(
            table.mount("jordi-beta").map(|m| m.source.as_str()),
            Some("beta")
        );
    }

    #[test]
    fn the_same_domain_locally_and_on_a_server_mounts_the_servers_and_hides_the_local() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        let (table, said) = assign(
            &mut file,
            &[local("platform", Some("acme/platform"))],
            &served(&[("acme", vec![remote("platform-team", Some("acme/platform"))])]),
        );
        let mount = table
            .mount("platform")
            .expect("mounted under the local name");
        assert_eq!(mount.remote, "platform-team");
        assert!(mount.replaces_local);
        assert_eq!(
            table.shadowed,
            vec![Hidden {
                local: "platform".into(),
                source: "acme".into(),
                reason: HiddenReason::Copy,
                by: "platform".into(),
            }]
        );
        assert_eq!(
            said,
            vec![Announcement::ReplacesLocal {
                source: "acme".into(),
                local: "platform".into(),
            }]
        );
        // Disconnected: the source is gone, so is the shadow.
        file.sources.clear();
        let (table, _) = assign(
            &mut file,
            &[local("platform", Some("acme/platform"))],
            &served(&[]),
        );
        assert!(table.shadowed.is_empty(), "the local copy is back");
    }

    #[test]
    fn the_same_domain_on_two_servers_is_mounted_from_the_first_connected() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        file.sources.push(source("beta"));
        let (table, said) = assign(
            &mut file,
            &[],
            &served(&[
                ("acme", vec![remote("platform", Some("acme/platform"))]),
                ("beta", vec![remote("plat", Some("Acme/Platform"))]),
            ]),
        );
        assert_eq!(
            names(&table),
            vec![("acme".into(), "platform".into(), "platform".into())]
        );
        assert_eq!(
            table.skipped,
            vec![Skipped {
                source: "beta".into(),
                remote: "plat".into(),
                kept_by: "acme".into(),
            }]
        );
        assert!(
            said.iter()
                .any(|a| matches!(a, Announcement::Skipped { .. }))
        );
        assert!(
            file.sources[1].mounts.is_empty(),
            "a skipped domain hands out no name"
        );
    }

    /// Fix round 1, I2: the copy stays hidden as a copy and the mount still
    /// replaces it, whatever name the mount was given since.
    #[test]
    fn a_mount_renamed_since_still_replaces_and_hides_the_local_copy() {
        let mut file = SourcesFile::default();
        let mut acme = source("acme");
        acme.mounts.push(MountRecord {
            remote: "platform-team".into(),
            local: "platform-x".into(),
        });
        file.sources.push(acme);
        let (table, said) = assign(
            &mut file,
            &[local("platform", Some("acme/platform"))],
            &served(&[("acme", vec![remote("platform-team", Some("acme/platform"))])]),
        );
        let mount = table
            .mount("platform-x")
            .expect("the persisted name stands");
        assert!(mount.replaces_local);
        assert!(table.mount("platform").is_none());
        assert_eq!(
            table.shadowed,
            vec![Hidden {
                local: "platform".into(),
                source: "acme".into(),
                reason: HiddenReason::Copy,
                by: "platform-x".into(),
            }]
        );
        assert!(said.is_empty(), "nothing new was decided: {said:?}");
    }

    /// Fix round 1, I3: a copy's name a mount already holds is not handed
    /// out a second time, and a different domain under a mount's name is a
    /// collision, not a copy.
    #[test]
    fn a_copy_never_gets_a_name_a_mount_already_holds() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        file.sources.push(source("beta"));
        let (table, _) = assign(
            &mut file,
            &[],
            &served(&[("acme", vec![remote("platform", None)]), ("beta", vec![])]),
        );
        assert_eq!(table.mounts[0].local, "platform");

        let (table, said) = assign(
            &mut file,
            &[local("platform", Some("acme/platform"))],
            &served(&[
                ("acme", vec![remote("platform", None)]),
                ("beta", vec![remote("plat", Some("acme/platform"))]),
            ]),
        );
        assert_eq!(
            names(&table),
            vec![
                ("acme".into(), "platform".into(), "platform".into()),
                ("beta".into(), "plat".into(), "plat".into()),
            ]
        );
        let locals: BTreeSet<&str> = table.mounts.iter().map(|m| m.local.as_str()).collect();
        assert_eq!(
            locals.len(),
            table.mounts.len(),
            "no two mounts share a name"
        );
        assert!(table.mount("plat").unwrap().replaces_local);
        assert_eq!(
            table.hidden("platform").collect::<Vec<_>>(),
            vec![
                &Hidden {
                    local: "platform".into(),
                    source: "acme".into(),
                    reason: HiddenReason::Collision,
                    by: "platform".into(),
                },
                &Hidden {
                    local: "platform".into(),
                    source: "beta".into(),
                    reason: HiddenReason::Copy,
                    by: "plat".into(),
                },
            ]
        );
        assert!(said.contains(&Announcement::LocalShadowed {
            local: "platform".into(),
            source: "acme".into(),
        }));
        assert!(said.contains(&Announcement::ReplacesLocal {
            source: "beta".into(),
            local: "plat".into(),
        }));
    }

    /// Fix round 1, M4: an established mount stays with its source and its
    /// name when an earlier-connected source starts offering the same domain.
    #[test]
    fn an_established_mount_stays_when_an_earlier_source_offers_the_same_domain() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        file.sources.push(source("beta"));
        let (table, _) = assign(
            &mut file,
            &[],
            &served(&[
                ("acme", vec![]),
                ("beta", vec![remote("plat", Some("acme/platform"))]),
            ]),
        );
        assert_eq!(
            names(&table),
            vec![("beta".into(), "plat".into(), "plat".into())]
        );

        let (table, said) = assign(
            &mut file,
            &[],
            &served(&[
                ("acme", vec![remote("platform", Some("acme/platform"))]),
                ("beta", vec![remote("plat", Some("acme/platform"))]),
            ]),
        );
        assert_eq!(
            names(&table),
            vec![("beta".into(), "plat".into(), "plat".into())]
        );
        assert_eq!(
            table.skipped,
            vec![Skipped {
                source: "acme".into(),
                remote: "platform".into(),
                kept_by: "beta".into(),
            }]
        );
        assert!(said.contains(&Announcement::Skipped {
            source: "acme".into(),
            remote: "platform".into(),
            kept_by: "beta".into(),
        }));
        assert!(file.sources[0].mounts.is_empty(), "acme hands out no name");
    }

    #[test]
    fn a_route_goes_to_the_one_source_that_holds_the_domain() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        file.sources.push(source("beta"));
        let (table, _) = assign(
            &mut file,
            &[local("notes", None)],
            &served(&[
                ("acme", vec![remote("runbooks", None)]),
                ("beta", vec![remote("specs", None)]),
            ]),
        );
        let n = |names: &[&str]| names.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            table.route("read_engram", ToolShape::OneDomain, &n(&["notes"])),
            Route::Local
        );
        assert_eq!(
            table.route("write_engram", ToolShape::OneDomain, &n(&["runbooks"])),
            Route::Remote {
                source: "acme".into()
            }
        );
        assert_eq!(
            table.route("read_engram", ToolShape::OneDomain, &[]),
            Route::Local,
            "D22"
        );
        assert_eq!(
            table.route("search_engrams", ToolShape::AllDomains, &[]),
            Route::FanOut {
                local: true,
                sources: vec!["acme".into(), "beta".into()]
            }
        );
        assert_eq!(
            table.route("search_engrams", ToolShape::AllDomains, &n(&["specs"])),
            Route::Remote {
                source: "beta".into()
            },
            "a domains filter asks only the source that holds them"
        );
        assert_eq!(
            table.route(
                "search_engrams",
                ToolShape::AllDomains,
                &n(&["notes", "specs"])
            ),
            Route::FanOut {
                local: true,
                sources: vec!["beta".into()]
            }
        );
        assert_eq!(
            table.route(
                "build_context",
                ToolShape::Anchored,
                &n(&["runbooks", "notes"])
            ),
            Route::Remote {
                source: "acme".into()
            },
            "build_context follows its anchor"
        );
        assert_eq!(
            table.route(
                "move_engram",
                ToolShape::Move,
                &n(&["runbooks", "runbooks"])
            ),
            Route::Remote {
                source: "acme".into()
            }
        );
        let Route::Refused(why) =
            table.route("move_engram", ToolShape::Move, &n(&["notes", "runbooks"]))
        else {
            panic!("a move between sources is refused");
        };
        assert!(why.contains("between sources"), "{why}");
        let Route::Refused(why) =
            table.route("share_changes", ToolShape::Collab, &n(&["runbooks"]))
        else {
            panic!("the collaboration tools refuse a mounted domain");
        };
        assert_eq!(
            why,
            "'runbooks' comes from acme, which shares this domain; share changes there"
        );
        let Route::Refused(why) =
            table.route("remove_domain", ToolShape::LocalOnly, &n(&["specs"]))
        else {
            panic!("a machine-local tool refuses a mounted domain");
        };
        assert_eq!(
            why,
            "'specs' comes from beta; remove_domain acts on this machine's own domains. Disconnect the source with crystalline disconnect beta"
        );
        assert_eq!(
            table.route("remove_domain", ToolShape::LocalOnly, &n(&["notes"])),
            Route::Local
        );
        assert_eq!(
            MountTable {
                sources: vec!["acme".into()],
                ..MountTable::default()
            }
            .route("search_engrams", ToolShape::AllDomains, &[]),
            Route::Local,
            "a source that mounts nothing makes no fan-out"
        );
    }

    /// Review focus 5, the pure half: every structured field moves to the
    /// local name, and nothing a person or an agent wrote is touched.
    #[test]
    fn the_answer_walk_translates_every_structured_field_and_no_body_text() {
        let mut file = SourcesFile::default();
        file.sources.push(source("acme"));
        let (table, _) = assign(
            &mut file,
            &[local("jordi", None)],
            &served(&[("acme", vec![remote("jordi", None)])]),
        );
        let map = table.names("acme");
        assert_eq!(map.remote_of("jordi-acme"), Some("jordi"));
        let mut answer = json!({
            "domain": "jordi",
            "url": "crystalline://jordi/runbooks/deploy",
            "related": "build_context anchor crystalline://jordi/runbooks/deploy to explore linked knowledge",
            "web_url": "https://crystalline.acme.com/d/jordi/e/runbooks/deploy",
            "content": "See crystalline://jordi/other and [[jordi:Other]] in domain jordi.",
            "markdown": "# jordi\n\ncrystalline://jordi/p",
            "body": { "domain": "jordi", "text": "crystalline://jordi/p" },
            "frontmatter": { "domain": "jordi", "url": "crystalline://jordi/x" },
            "relations": [{ "line": 3, "rel_type": "relates_to", "target": { "domain": "jordi", "target": "Other" }, "resolved": true }],
            "links": [{ "line": 4, "target": { "domain": null, "target": "Plain" }, "resolved": false }],
            "inbound": { "count": 1, "refs": [{ "domain": "jordi", "path": "a.md", "kind": "link" }] },
            "hits": [{ "domain": "jordi", "permalink": "p", "snippet": "jordi says crystalline://jordi/p" }],
            "scope": { "domains": ["jordi"] },
            "pending_domains": ["jordi"],
            "domains": [{ "name": "jordi", "canonical_name": "jordi", "aliases": ["jordi-old"] }],
            "error": "your access to 'jordi' is viewer",
        });
        translate_answer(&mut answer, &map);
        assert_eq!(answer["domain"], "jordi-acme");
        assert_eq!(answer["url"], "crystalline://jordi-acme/runbooks/deploy");
        assert_eq!(
            answer["related"],
            "build_context anchor crystalline://jordi-acme/runbooks/deploy to explore linked knowledge"
        );
        assert_eq!(answer["relations"][0]["target"]["domain"], "jordi-acme");
        assert_eq!(
            answer["links"][0]["target"]["domain"],
            serde_json::Value::Null
        );
        assert_eq!(answer["inbound"]["refs"][0]["domain"], "jordi-acme");
        assert_eq!(answer["hits"][0]["domain"], "jordi-acme");
        assert_eq!(answer["scope"]["domains"], json!(["jordi-acme"]));
        assert_eq!(answer["pending_domains"], json!(["jordi-acme"]));
        assert_eq!(answer["domains"][0]["name"], "jordi-acme");
        assert_eq!(answer["domains"][0]["canonical_name"], "jordi-acme");
        // Untouched: the server's own page, every body text, the server's words.
        assert_eq!(
            answer["web_url"],
            "https://crystalline.acme.com/d/jordi/e/runbooks/deploy"
        );
        assert_eq!(
            answer["content"],
            "See crystalline://jordi/other and [[jordi:Other]] in domain jordi."
        );
        assert_eq!(answer["frontmatter"]["url"], "crystalline://jordi/x");
        assert_eq!(answer["markdown"], "# jordi\n\ncrystalline://jordi/p");
        assert_eq!(
            answer["body"],
            json!({ "domain": "jordi", "text": "crystalline://jordi/p" })
        );
        assert_eq!(
            answer["hits"][0]["snippet"],
            "jordi says crystalline://jordi/p"
        );
        assert_eq!(answer["error"], "your access to 'jordi' is viewer");
    }

    #[test]
    fn an_address_of_a_longer_name_is_not_mistaken_for_a_shorter_one() {
        let map = NameMap {
            source: "acme".into(),
            to_remote: [("a-x".to_string(), "a".to_string())].into(),
            to_local: [("a".to_string(), "a-x".to_string())].into(),
        };
        let mut answer = json!({ "url": "crystalline://a-b/p", "anchor": "crystalline://a" });
        translate_answer(&mut answer, &map);
        assert_eq!(
            answer["url"], "crystalline://a-b/p",
            "a-b is another domain"
        );
        assert_eq!(answer["anchor"], "crystalline://a-x");
    }

    #[test]
    fn an_outbound_address_takes_the_sources_name() {
        let map: BTreeMap<String, String> =
            [("jordi-acme".to_string(), "jordi".to_string())].into();
        assert_eq!(
            translate_address_to("crystalline://jordi-acme/x", &map),
            "crystalline://jordi/x"
        );
        assert_eq!(
            translate_address_to("crystalline://jordi-acme", &map),
            "crystalline://jordi"
        );
        assert_eq!(translate_address_to("plain", &map), "plain");
        assert_eq!(
            translate_address_to("crystalline://jordi-acme-2/x", &map),
            "crystalline://jordi-acme-2/x",
            "a longer name is another domain"
        );
    }

    #[test]
    fn an_authority_ends_where_the_domain_characters_end() {
        let map: BTreeMap<String, String> = [("a".to_string(), "b".to_string())].into();
        assert_eq!(
            translate_address_to("see crystalline://a, then crystalline://a.", &map),
            "see crystalline://b, then crystalline://b."
        );
        assert_eq!(
            translate_address_to("crystalline://a#frag", &map),
            "crystalline://b#frag"
        );
        assert_eq!(
            translate_address_to("crystalline://a.b/x", &map),
            "crystalline://a.b/x"
        );
    }
}
