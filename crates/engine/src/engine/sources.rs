//! Reflection sources from other domains: how a write names one, the facts
//! a page's sources are held against, and what a reader may see of them.
//!
//! A source with no `domain` is an engram of the page's own domain, as
//! every source was before; one with a `domain` names another domain by its
//! canonical name. Its facts are read with one seek per cited permalink in
//! that domain, never a scan of it. A domain that cannot be checked here
//! (not registered on this instance, or a local copy a connected server
//! hides) makes its sources `unavailable`, never `missing`.

use crystalline_core::names::NameTable;
use crystalline_core::reflection as r;

use super::reflection::EngramFact;
use super::*;
use crate::scope::Scope;

/// What a queue item says about sources in a domain the caller cannot
/// open. One sentence whatever their state, so the item tells nothing of
/// them but that they need a person who may open their domain.
pub(crate) const HIDDEN_SOURCE_NOTE: &str = "a source in a domain you cannot open has changed";

/// What a write is refused with when it names a source in a domain fewer
/// people may read than the page's domain ([`Engine::readers_cover`]). The
/// writer may open that domain (the refusal for one it may not comes first
/// and is an unknown domain's), so naming the source tells it nothing new.
pub(crate) const TOO_WIDE_REFUSAL: &str = "this source is in a domain fewer people may read than this page's domain; cite it from a page in that domain instead";

/// What a write is refused with when it names a source in another domain
/// on a stack that could not open its accounts store: who reads which
/// domain is unknown there, so the check fails closed.
pub(crate) const UNKNOWN_READERS_REFUSAL: &str = "who may read each domain is unknown here because the accounts store could not be opened, so a source outside this page's domain is refused until it opens; remove that source and the text drawn from it, or try again once the accounts store is back";

/// What a `source_too_wide` item says to a caller who may open every
/// domain the item is about.
pub(crate) const TOO_WIDE_NOTE: &str = "this section cites a domain fewer people may read than this page's domain, so its text reaches people who may not read that domain; remove that source and the text drawn from it. The page stays as it is until a curator does";

/// What a `source_unavailable` item says to a caller who may open the
/// domain, with how to clear it.
pub(crate) fn unavailable_note(domain: &str) -> String {
    format!(
        "sources in {domain} cannot be checked here; keep them while the domain may come back, or drop one by writing its section's sources without it"
    )
}

/// The facts a set of sources is held against: the page's own domain's,
/// and per other domain (by the name a source stores) its cited engrams'
/// facts, or `None` when that domain cannot be checked here.
#[derive(Debug, Default)]
pub(crate) struct SourceFacts {
    /// The page's own domain, by permalink.
    pub own: HashMap<String, EngramFact>,
    foreign: HashMap<String, Option<HashMap<String, EngramFact>>>,
    /// The page's domain by its local name.
    own_domain: String,
    /// Each stored domain name by the local name this instance registers it
    /// under, for every name that resolves here.
    local: HashMap<String, String>,
}

impl SourceFacts {
    /// The engram `src` names, when its domain was read and holds it.
    pub(crate) fn fact(&self, src: &r::SourceRef) -> Option<&EngramFact> {
        match src.domain.as_deref() {
            None => self.own.get(&src.engram),
            Some(domain) => self.foreign.get(domain)?.as_ref()?.get(&src.engram),
        }
    }

    /// The name a reader reaches `src`'s domain by on this instance, when it
    /// is not the page's own domain: the local name the stored (canonical)
    /// name resolves to, or the stored name when it resolves to nothing
    /// here. `None` for a source of the page's own domain. What every
    /// answer shows and links by, since routes and every domain verb take
    /// the local name; the canonical one stays in the stored record.
    pub(crate) fn shown_domain<'a>(&'a self, src: &'a r::SourceRef) -> Option<&'a str> {
        let stored = src.domain.as_deref()?;
        let shown = self.local.get(stored).map_or(stored, String::as_str);
        (shown != self.own_domain).then_some(shown)
    }

    /// The local name of `src`'s domain when it is another domain than the
    /// page's and can be checked here: the domain whose readers
    /// [`Engine::readers_cover`] is asked about. `None` for a source of the
    /// page's own domain and for one whose domain cannot be checked here,
    /// whose readers nobody here knows.
    pub(crate) fn checked_domain(&self, src: &r::SourceRef) -> Option<&str> {
        let stored = src.domain.as_deref()?;
        if !matches!(self.foreign.get(stored), Some(Some(_))) {
            return None;
        }
        let local = self.local.get(stored)?;
        (*local != self.own_domain).then_some(local.as_str())
    }

    /// Whether `src` is in a domain that cannot be checked here.
    pub(crate) fn unavailable(&self, src: &r::SourceRef) -> bool {
        src.domain
            .as_deref()
            .is_some_and(|domain| !matches!(self.foreign.get(domain), Some(Some(_))))
    }
}

/// What one reader may see of the sources of another domain: the sources
/// of a domain the reader may read engrams in (knowledge Read, the
/// `hidden_for` rule), and for a reader with no restriction every source.
/// The page's own sources are the caller's `engrams_visible` to decide.
pub(crate) struct SourceSight {
    /// `None`: the reader sees every domain, also one not registered here.
    hidden: Option<HashSet<String>>,
    table: Arc<NameTable>,
}

impl SourceSight {
    /// Whether the reader may see `src`. A domain name that resolves to no
    /// registered domain is seen only by a reader with no restriction, so a
    /// name no rights decision covers is never shown to anyone else.
    pub(crate) fn sees(&self, src: &r::SourceRef) -> bool {
        let Some(domain) = src.domain.as_deref() else {
            return true;
        };
        match &self.hidden {
            None => true,
            Some(hidden) => self
                .table
                .resolve(domain)
                .is_some_and(|local| !hidden.contains(local)),
        }
    }

    /// `text` with the sources this reader may not see taken out of its
    /// `reflection:` block; the text as it is when there are none. What
    /// `read_reflection` answers as `markdown`, so the raw block never names
    /// a domain or an engram the reader may not see.
    pub(crate) fn shown_text(&self, text: &str) -> String {
        let mut prov = r::read_provenance(text);
        let mut cut = false;
        for record in prov.values_mut() {
            let before = record.sources.len();
            record.sources.retain(|src| self.sees(src));
            cut |= record.sources.len() != before;
        }
        if !cut {
            return text.to_string();
        }
        r::write_provenance(text, &prov)
            .unwrap_or_else(|_| super::reflection::without_provenance(text))
    }
}

/// One entry of a write's `sources`, resolved.
#[derive(Debug, Clone)]
pub(crate) struct NamedSource {
    /// The name the record stores: `None` for the page's own domain, else
    /// the domain's canonical name.
    pub domain: Option<String>,
    pub permalink: String,
}

impl NamedSource {
    /// How a refusal names the source: the permalink, or the address.
    pub(crate) fn said(&self) -> String {
        match &self.domain {
            None => self.permalink.clone(),
            Some(domain) => format!(
                "{}{domain}/{}",
                crystalline_core::address::SCHEME,
                self.permalink
            ),
        }
    }

    /// The record this source becomes, with `checksum`.
    pub(crate) fn source_ref(&self, checksum: &str) -> r::SourceRef {
        r::SourceRef {
            domain: self.domain.clone(),
            engram: self.permalink.clone(),
            checksum: checksum.to_string(),
        }
    }
}

impl Engine {
    /// What `scope` may see of other domains' sources.
    pub(crate) async fn source_sight(&self, scope: &Scope) -> Result<SourceSight> {
        let hidden = self.hidden_domains(scope).await?.map(|mut hidden| {
            hidden.extend(self.shadowed_domains());
            hidden
        });
        Ok(SourceSight {
            hidden,
            table: self.name_table_now().await,
        })
    }

    /// The registered local domain a stored source domain names, when it can
    /// be checked here: a name that resolves to a domain this instance
    /// registered and a connected server does not hide.
    fn checkable_domain(&self, table: &NameTable, name: &str) -> Option<String> {
        let local = table.resolve(name)?;
        (self.knows_domain(local) && !self.shadowed_domains().contains(local))
            .then(|| local.to_string())
    }

    /// Whether a page of domain `page` may carry text drawn from domain
    /// `source` (both local names): whether every reader of the page may
    /// read text drawn from `source` anyway. The rule and who counts as a
    /// reader are [`crate::scope::DomainAccess::readers_cover`]'s.
    ///
    /// It never looks at who asks: the machine owner, who reads every
    /// domain, is held to it too, since the page's readers are the ones the
    /// text reaches. Every serving stack (the daemon, with or without its
    /// HTTP surface, and the embedded stdio stack) installs a resolver
    /// before it answers anything. One that tried and could not open the
    /// accounts store fails closed: no other domain is covered. Only an
    /// engine that never tried (a one-shot command or a test engine, where
    /// no accounts store is in play) answers that every page covers every
    /// domain.
    pub(crate) async fn readers_cover(&self, page: &str, source: &str) -> Result<bool> {
        let Some(access) = self.domain_access.get() else {
            // Failed closed: a stack that could not open its accounts store
            // knows no reader, so no other domain is covered.
            return Ok(self.domain_access_unavailable.get().is_none());
        };
        access
            .readers_cover(page, source)
            .await
            .map_err(|e| EngineError::Internal(e.to_string()))
    }

    /// The pages of `domain` (by path inside `reflections/`, sorted) with a
    /// section that cites a domain fewer people may read than `domain`: the
    /// pages the queue names with `source_too_wide`, for every source, seen
    /// by whom or not. A source whose domain cannot be checked here is not
    /// counted, as in the queue.
    pub(crate) async fn too_wide_pages(&self, domain: &str) -> Result<Vec<String>> {
        let rows = {
            let store = self.store.lock().await;
            let Some(id) = store.domain_id(domain).await? else {
                return Ok(Vec::new());
            };
            store.list_reflections(id).await?
        };
        let table = self.name_table_now().await;
        let mut covers: HashMap<String, bool> = HashMap::new();
        let mut out = Vec::new();
        for row in rows {
            let mut wide = false;
            for record in r::read_provenance(&row.content).into_values() {
                for src in &record.sources {
                    let Some(name) = src.domain.as_deref() else {
                        continue;
                    };
                    let Some(local) = self.checkable_domain(&table, name) else {
                        continue;
                    };
                    if local == domain {
                        continue;
                    }
                    let covered = match covers.get(&local) {
                        Some(covered) => *covered,
                        None => {
                            let covered = self.readers_cover(domain, &local).await?;
                            covers.insert(local, covered);
                            covered
                        }
                    };
                    wide |= !covered;
                }
            }
            if wide {
                out.push(row.path);
            }
        }
        out.sort();
        Ok(out)
    }

    /// The entries of a write's `sources`, each resolved: a bare permalink
    /// is the page's own, and so is an address whose domain (in any
    /// spelling) is the page's. Another domain must be one `scope` may read
    /// engrams in; one it may not see, one nobody registered here and one a
    /// connected server serves are refused with the bytes an unknown domain
    /// gets, naming the spelling the writer used, so a private domain's
    /// existence never shows. Then it must be one every reader of the page
    /// may read ([`Engine::readers_cover`]), whoever writes: a page never
    /// carries text drawn from a domain fewer people may read. That check
    /// comes second, so its refusal only ever names a domain the writer may
    /// open.
    pub(crate) async fn resolve_source_names<'a>(
        &self,
        own: &str,
        scope: &Scope,
        names: impl Iterator<Item = &'a String>,
    ) -> Result<HashMap<String, NamedSource>> {
        let mut out = HashMap::new();
        let mut context: Option<(Arc<NameTable>, HashSet<String>)> = None;
        for name in names {
            if out.contains_key(name) {
                continue;
            }
            let (spelled, permalink) =
                match r::parse_source_name(name).map_err(EngineError::Invalid)? {
                    r::SourceName::Own(permalink) => {
                        out.insert(
                            name.clone(),
                            NamedSource {
                                domain: None,
                                permalink,
                            },
                        );
                        continue;
                    }
                    r::SourceName::Address { domain, permalink } => (domain, permalink),
                };
            if context.is_none() {
                context = Some((self.name_table_now().await, self.hidden_for(scope).await?));
            }
            let (table, hidden) = context.as_ref().expect("filled above");
            let Some(local) = table
                .resolve(&spelled)
                .filter(|local| self.knows_domain(local) && !hidden.contains(*local))
            else {
                return Err(self.unknown_domain(&spelled, hidden));
            };
            let domain = if local == own {
                None
            } else {
                if let Some(why) = self.domain_access_unavailable.get()
                    && self.domain_access.get().is_none()
                {
                    return Err(EngineError::Invalid(format!(
                        "'{name}': {UNKNOWN_READERS_REFUSAL} ({why})"
                    )));
                }
                if !self.readers_cover(own, local).await? {
                    return Err(EngineError::Invalid(format!(
                        "'{name}': {TOO_WIDE_REFUSAL}"
                    )));
                }
                Some(table.normalize(&spelled).unwrap_or(&spelled).to_string())
            };
            out.insert(name.clone(), NamedSource { domain, permalink });
        }
        Ok(out)
    }

    /// The facts `sources` are held against. `own` is the page domain's
    /// facts when the caller read them already (the queue's scan of the
    /// whole domain); otherwise the page's own sources are read one seek
    /// each. Every other domain is read one seek per cited permalink, under
    /// one store lock per domain, and a domain that cannot be checked here
    /// is recorded as such.
    pub(crate) async fn source_facts<'a>(
        &self,
        own_domain: &str,
        own_id: DomainId,
        own: Option<HashMap<String, EngramFact>>,
        sources: impl Iterator<Item = &'a r::SourceRef>,
    ) -> Result<SourceFacts> {
        let mut own_wanted: BTreeSet<String> = BTreeSet::new();
        let mut foreign_wanted: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for src in sources {
            match &src.domain {
                None => {
                    own_wanted.insert(src.engram.clone());
                }
                Some(domain) => {
                    foreign_wanted
                        .entry(domain.clone())
                        .or_default()
                        .insert(src.engram.clone());
                }
            }
        }
        let own = match own {
            Some(facts) => facts,
            None => {
                self.cited_facts(own_id, own_wanted.into_iter().collect())
                    .await?
            }
        };
        let mut foreign = HashMap::new();
        let mut local = HashMap::new();
        if !foreign_wanted.is_empty() {
            let table = self.name_table_now().await;
            for (name, permalinks) in foreign_wanted {
                if let Some(resolved) = table.resolve(&name) {
                    local.insert(name.clone(), resolved.to_string());
                }
                let id = match self.checkable_domain(&table, &name) {
                    Some(local) => self.store.lock().await.domain_id(&local).await?,
                    None => None,
                };
                let facts = match id {
                    Some(id) => Some(
                        self.cited_facts(id, permalinks.into_iter().collect())
                            .await?,
                    ),
                    None => None,
                };
                foreign.insert(name, facts);
            }
        }
        Ok(SourceFacts {
            own,
            foreign,
            own_domain: own_domain.to_string(),
            local,
        })
    }
}
