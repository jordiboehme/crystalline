//! One table from every spelling of a domain to its local name.
//!
//! A domain has three kinds of names. Its **local name** is this machine's
//! config key and the internal key for storage, state directories and URLs.
//! Its **canonical name** is the valid `domain_name` its MANIFEST declares,
//! else the local name; content carries it. Its **aliases** are former local
//! names, accepted as input only. [`NameTable`] maps every spelling to one
//! local name with a fixed precedence:
//!
//! 1. local names always win: the exact registration owns its name;
//! 2. canonical names, unless a local name already owns that spelling - the
//!    claiming domain is then *shadowed*. Two domains claiming one canonical
//!    name that no domain is registered under: the name resolves nowhere and
//!    the table records a [`NameConflict`];
//! 3. aliases, unless the spelling is taken by a local or canonical name, is
//!    contested, or is claimed as an alias by two domains - such an alias is
//!    dropped and recorded as a [`DroppedAlias`] for the caller to warn about.
//!
//! Matching is on the exact spelling only: no case folding, no trimming and
//! no close-match guessing. The table is pure and rebuilt whenever the set of
//! registered domains or a declared name changes.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::config::registration::validate_domain_name;
use crate::config::{DomainEntry, GlobalConfig};
use crate::manifest::domain_name_at;

/// The names one registered domain brings to the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameInput {
    /// The config key on this machine.
    pub local: String,
    /// The MANIFEST `domain_name`, if any. An invalid value is ignored.
    pub canonical: Option<String>,
    /// Machine-local former names.
    pub aliases: Vec<String>,
}

/// The [`NameInput`]s a machine's own registered configuration contributes,
/// one per registered domain: a file domain's canonical name is read straight
/// off its `MANIFEST.md` on disk, since content there is the whole of what a
/// standalone reader (no daemon, no index) ever has to ask; a virtual
/// domain's declared name lives in its MANIFEST engram, in the database, so it
/// comes from `virtual_names` instead - keyed by local name, and empty (or
/// missing an entry) whenever no index was reachable to read it from, in
/// which case the domain contributes only its local name, same as an
/// unsynced file domain. Aliases always come from the registration entry
/// itself, whichever kind of domain it is.
///
/// The shared building block behind [`NameTable::from_config`], and behind a
/// caller (a fresh registration's collision check, most notably) that needs
/// to union the inputs of more than one [`GlobalConfig`] before building one
/// table over all of them.
pub fn config_name_inputs<'a>(
    domains: impl IntoIterator<Item = (&'a String, &'a DomainEntry)>,
    virtual_names: &BTreeMap<String, String>,
) -> Vec<NameInput> {
    domains
        .into_iter()
        .map(|(local, entry)| NameInput {
            local: local.clone(),
            canonical: if entry.is_virtual() {
                virtual_names.get(local).cloned()
            } else {
                entry.file_path().and_then(|root| domain_name_at(&root))
            },
            aliases: entry.aliases.clone(),
        })
        .collect()
}

/// What a person is told when a domain registered here declares a canonical
/// name another domain here already uses as its local name: this one is
/// registered as `local`, and links that name `canonical` still reach the
/// other one. One sentence for the engine's registration report and the
/// CLI's direct path.
pub fn shadowed_note(canonical: &str, local: &str) -> String {
    format!(
        "'{canonical}' is already a domain here, so this one is registered as '{local}'; links \
         that name '{canonical}' still reach the other domain. Rename one of them to line them \
         up."
    )
}

/// A canonical name claimed by more than one domain, none of them
/// registered under it. The name resolves nowhere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameConflict {
    pub name: String,
    /// The claiming local names, sorted.
    pub claimants: Vec<String>,
}

/// An alias that does not resolve because its spelling is taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedAlias {
    /// The local name of the domain that listed the alias.
    pub domain: String,
    pub alias: String,
    /// The local name of the domain that owns the spelling, or `None` when
    /// no single domain owns it (a contested canonical name, or an alias two
    /// domains share).
    pub held_by: Option<String>,
}

/// Every spelling of every registered domain, resolved to its local name.
#[derive(Debug, Clone, Default)]
pub struct NameTable {
    /// Resolvable spelling to local name.
    spellings: BTreeMap<String, String>,
    /// Local name to its declared valid canonical name, else itself.
    canonical_of: HashMap<String, String>,
    shadowed: HashSet<String>,
    effective_aliases: HashMap<String, Vec<String>>,
    conflicts: Vec<NameConflict>,
    dropped_aliases: Vec<DroppedAlias>,
}

impl NameTable {
    /// Builds the table. The result does not depend on the order of
    /// `inputs`, with one exception: a local name listed twice counts once,
    /// and the entry that counts is the one that comes first in `inputs`.
    pub fn build(inputs: &[NameInput]) -> NameTable {
        let mut sorted: Vec<&NameInput> = inputs.iter().collect();
        sorted.sort_by(|a, b| a.local.cmp(&b.local));
        sorted.dedup_by(|a, b| a.local == b.local);

        let mut table = NameTable::default();

        // 1. Local names.
        for input in &sorted {
            table
                .spellings
                .insert(input.local.clone(), input.local.clone());
            let canonical = input
                .canonical
                .as_deref()
                .filter(|c| validate_domain_name(c).is_ok())
                .unwrap_or(&input.local);
            table
                .canonical_of
                .insert(input.local.clone(), canonical.to_string());
        }

        // 2. Canonical names differing from the local name.
        let mut claims: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for input in &sorted {
            let canonical = table.canonical_of[&input.local].as_str();
            if canonical != input.local {
                claims.entry(canonical).or_default().push(&input.local);
            }
        }
        let mut contested: BTreeSet<String> = BTreeSet::new();
        let mut granted: Vec<(String, String)> = Vec::new();
        for (name, claimants) in claims {
            if table.spellings.contains_key(name) {
                table
                    .shadowed
                    .extend(claimants.iter().map(|c| c.to_string()));
            } else if claimants.len() > 1 {
                contested.insert(name.to_string());
                table.conflicts.push(NameConflict {
                    name: name.to_string(),
                    claimants: claimants.iter().map(|c| c.to_string()).collect(),
                });
            } else {
                granted.push((name.to_string(), claimants[0].to_string()));
            }
        }
        table.spellings.extend(granted);

        // 3. Aliases, each listed once per domain, never repeating the
        //    domain's own local or canonical name.
        let mut listed: Vec<(&str, Vec<&str>)> = Vec::new();
        let mut alias_claims: HashMap<&str, usize> = HashMap::new();
        for input in &sorted {
            let canonical = table.canonical_of[&input.local].as_str();
            let mut own: Vec<&str> = Vec::new();
            for alias in &input.aliases {
                let alias = alias.as_str();
                if alias == input.local || alias == canonical || own.contains(&alias) {
                    continue;
                }
                own.push(alias);
                *alias_claims.entry(alias).or_default() += 1;
            }
            listed.push((&input.local, own));
        }
        let mut granted: Vec<(String, String)> = Vec::new();
        for (local, aliases) in listed {
            let mut effective = Vec::new();
            for alias in aliases {
                let held_by = if let Some(owner) = table.spellings.get(alias) {
                    Some(Some(owner.clone()))
                } else if contested.contains(alias) || alias_claims[alias] > 1 {
                    Some(None)
                } else {
                    None
                };
                match held_by {
                    Some(held_by) => table.dropped_aliases.push(DroppedAlias {
                        domain: local.to_string(),
                        alias: alias.to_string(),
                        held_by,
                    }),
                    None => {
                        granted.push((alias.to_string(), local.to_string()));
                        effective.push(alias.to_string());
                    }
                }
            }
            table.effective_aliases.insert(local.to_string(), effective);
        }
        table.spellings.extend(granted);
        table
            .dropped_aliases
            .sort_by(|a, b| (&a.domain, &a.alias).cmp(&(&b.domain, &b.alias)));
        table
    }

    /// The table built from one machine's own registered configuration alone
    /// (see [`config_name_inputs`]): every domain this machine's config
    /// registers, each carrying its own file domain's declared canonical
    /// name, read straight off its MANIFEST since a caller with no index has
    /// nothing else to ask, and its recorded aliases. A virtual domain's
    /// canonical name comes from `virtual_names` (its local name to its
    /// declared name, when known), empty for a caller with no index reachable
    /// to read one from; such a domain then contributes only its local name,
    /// the same answer an unsynced file domain gets too.
    ///
    /// The single building block behind every place on the CLI and the
    /// service's standalone (no-daemon) commands that used to build this same
    /// table by hand: `domain list`'s own NAME/ALIASES/`shadowed` columns, a
    /// standalone command's own-name resolution before it acts, and a fresh
    /// registration's collision check (by way of [`config_name_inputs`]
    /// directly, since that check unions more than one [`GlobalConfig`]).
    pub fn from_config(cfg: &GlobalConfig, virtual_names: &BTreeMap<String, String>) -> NameTable {
        NameTable::build(&config_name_inputs(&cfg.domains, virtual_names))
    }

    /// The local name `spelling` resolves to, if any. Exact spelling only.
    pub fn resolve(&self, spelling: &str) -> Option<&str> {
        self.spellings.get(spelling).map(String::as_str)
    }

    /// The canonical name of the domain registered as `local`: its declared
    /// valid `domain_name`, else `local` itself. `None` for an unknown local
    /// name. A shadowed or contested canonical is still returned here; use
    /// [`NameTable::normalize`] to decide what content may carry.
    pub fn canonical(&self, local: &str) -> Option<&str> {
        self.canonical_of.get(local).map(String::as_str)
    }

    /// Whether the domain's canonical name is owned by another domain's
    /// local name on this machine.
    pub fn is_shadowed(&self, local: &str) -> bool {
        self.shadowed.contains(local)
    }

    /// The aliases of `local` that resolve, after drops.
    pub fn aliases(&self, local: &str) -> &[String] {
        self.effective_aliases
            .get(local)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Canonical names claimed by more than one domain, sorted by name.
    pub fn conflicts(&self) -> &[NameConflict] {
        &self.conflicts
    }

    /// Aliases that do not resolve, sorted by domain, then alias.
    pub fn dropped_aliases(&self) -> &[DroppedAlias] {
        &self.dropped_aliases
    }

    /// Every resolvable `(spelling, local name)` pair, sorted by spelling.
    pub fn spellings(&self) -> Vec<(String, String)> {
        self.spellings
            .iter()
            .map(|(spelling, local)| (spelling.clone(), local.clone()))
            .collect()
    }

    /// The canonical name to write in place of `spelling`, or `None` when
    /// `spelling` is already canonical, resolves nowhere, or its domain's
    /// canonical name does not resolve back to that domain here (shadowed or
    /// contested): content never gets a spelling that points elsewhere.
    pub fn normalize(&self, spelling: &str) -> Option<&str> {
        let local = self.resolve(spelling)?;
        let canonical = self.canonical(local)?;
        (canonical != spelling && self.resolve(canonical) == Some(local)).then_some(canonical)
    }
}
