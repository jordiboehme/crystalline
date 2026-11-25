//! Change one domain's provisioning decision: the pure core of
//! `crystalline provision allow`, `deny`, `skip` and `take`.
//!
//! [`change_selection`] takes the decision as `config.yaml` holds it, the
//! change and the artifact names, and what the domain ships now (the light
//! listing, [`crate::provision::list_artifacts`]). It returns the decision to
//! write, always [`Provision::normalized`], plus plain sentences for names that
//! changed nothing. It reads no file and writes none; the engine holds the
//! configuration lock around the write.
//!
//! What each change does to each decision (`N` is the names given):
//!
//! | Decision before | `skip N` | `take N` |
//! | --- | --- | --- |
//! | undecided | `{except: N}` | `{only: N}` |
//! | `false` | no change, one note | `{only: N}` |
//! | `true` or `{except: E, also: A}` | in `A`: leaves `A`; else joins `E` | in `E`: leaves `E`; optional and not in `A`: joins `A`; else "already on" |
//! | `{only: O}` | leaves `O`; not in `O`: "already off" | joins `O` |
//!
//! `allow` writes plain `true`, except that it keeps the optional artifacts
//! the person took: from `{except: E, also: A}` it writes `{also: A}`, from
//! `{only: O}` it writes `{also: ...}` with the names in `O` the listing marks
//! optional. `deny` always writes plain `false`.

use crate::config::Provision;
use crate::manifest::ArtifactType;
use crate::provision::model::{ArtifactListing, parse_artifact_key};

/// What a call does to a domain's decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionChange {
    /// Install everything except optional artifacts that were not taken.
    /// Clears `except` and `only`, keeps the taken optional artifacts.
    Allow,
    /// Install nothing: plain `false`.
    Deny,
    /// Leave the named artifacts out.
    Skip,
    /// Install the named artifacts.
    Take,
}

/// The decision [`change_selection`] computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionOutcome {
    /// The decision to write, always normalized.
    pub decision: Provision,
    /// Plain sentences for names that changed nothing, such as
    /// "`skills/tide-tables` is already on".
    pub notes: Vec<String>,
}

impl SelectionOutcome {
    /// Whether the decision is written as a map, which an older Crystalline
    /// binary cannot read (see [`Provision::is_plain_bool`]).
    pub fn writes_map(&self) -> bool {
        !self.decision.is_plain_bool()
    }
}

/// Compute `domain`'s new decision for `change`.
///
/// - `current` is the decision `config.yaml` holds (`None`: undecided).
/// - `names` are artifact keys (`<kind>/<name>`) for [`SelectionChange::Skip`]
///   and [`SelectionChange::Take`]; a repeated name counts once. `Allow` and
///   `Deny` ignore them.
/// - `shipped` and `present_kinds` are what
///   [`crate::provision::list_artifacts`] reports for the domain.
///
/// Every name is checked before anything changes, and one bad name refuses
/// the whole call:
///
/// - a name already in one of the decision's lists is always accepted, so a
///   stale or hand-edited name stays removable;
/// - otherwise a malformed key is refused;
/// - otherwise a name of a kind whose source root is missing (not in
///   `present_kinds`) is accepted, since its artifacts may not be on this
///   machine yet;
/// - otherwise a name the domain does not ship is refused, naming what it
///   ships.
///
/// Names compare exactly (case-sensitive), as in [`Provision::selects`].
pub fn change_selection(
    domain: &str,
    current: Option<&Provision>,
    change: SelectionChange,
    names: &[String],
    shipped: &[ArtifactListing],
    present_kinds: &[ArtifactType],
) -> Result<SelectionOutcome, String> {
    let lookup = Lookup {
        shipped,
        present_kinds,
    };
    let (decision, notes) = match change {
        SelectionChange::Allow => (allow(current, &lookup), Vec::new()),
        SelectionChange::Deny => (Provision::Deny, Vec::new()),
        SelectionChange::Skip | SelectionChange::Take => {
            let names = distinct(names);
            if names.is_empty() {
                let verb = if change == SelectionChange::Skip {
                    "skip"
                } else {
                    "take"
                };
                return Err(format!(
                    "`{verb}` needs at least one artifact name, for example `skills/tide-tables`."
                ));
            }
            check_names(domain, current, &names, shipped, present_kinds)?;
            if change == SelectionChange::Skip {
                skip(domain, current, &names, &lookup)
            } else {
                take(current, &names, &lookup)
            }
        }
    };
    Ok(SelectionOutcome {
        decision: decision.normalized(),
        notes,
    })
}

/// The names in their given order, each once.
fn distinct(names: &[String]) -> Vec<&str> {
    let mut seen: Vec<&str> = Vec::with_capacity(names.len());
    for name in names {
        if !seen.contains(&name.as_str()) {
            seen.push(name);
        }
    }
    seen
}

/// Refuse the first malformed name, then every well-formed name the domain
/// does not ship, in one sentence.
fn check_names(
    domain: &str,
    current: Option<&Provision>,
    names: &[&str],
    shipped: &[ArtifactListing],
    present_kinds: &[ArtifactType],
) -> Result<(), String> {
    let listed = |name: &str| current.is_some_and(|d| d.names().any(|n| n == name));
    let mut unknown = Vec::new();
    for &name in names {
        if listed(name) {
            continue;
        }
        let Some((kind, _)) = parse_artifact_key(name) else {
            return Err(format!(
                "`{name}` is not an artifact name; use `<kind>/<name>`, for example `skills/tide-tables`."
            ));
        };
        if present_kinds.contains(&kind) && !shipped.iter().any(|a| a.key == name) {
            unknown.push(name);
        }
    }
    if unknown.is_empty() {
        return Ok(());
    }
    let unknown = backticked(&unknown);
    let mut ships: Vec<&str> = shipped.iter().map(|a| a.key.as_str()).collect();
    ships.sort_unstable();
    ships.dedup();
    let ships = if ships.is_empty() {
        "It ships no artifacts.".to_string()
    } else {
        format!(
            "It ships {}. See them with `crystalline provision status`.",
            backticked(&ships)
        )
    };
    Err(format!(
        "The `{domain}` domain does not ship {unknown}. {ships}"
    ))
}

/// The names in backticks, joined with commas.
fn backticked(names: &[&str]) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// What the listing says about one name.
struct Lookup<'a> {
    shipped: &'a [ArtifactListing],
    present_kinds: &'a [ArtifactType],
}

impl Lookup<'_> {
    /// Whether `take` should record `name` in `also`: the listing marks it
    /// optional, or its kind's root is missing, so the mark cannot be read
    /// and the taken name is kept in case it is optional. For a required
    /// artifact an `also` entry changes nothing.
    fn may_be_optional(&self, name: &str) -> bool {
        match self.shipped.iter().find(|a| a.key == name) {
            Some(listing) => listing.optional,
            None => parse_artifact_key(name)
                .is_some_and(|(kind, _)| !self.present_kinds.contains(&kind)),
        }
    }

    /// Whether the listing marks `name` optional. Unlike
    /// [`Lookup::may_be_optional`], a name the listing cannot see is not
    /// optional here: `skip` must record a name that may be required.
    fn is_optional(&self, name: &str) -> bool {
        self.shipped.iter().any(|a| a.key == name && a.optional)
    }
}

/// Whether `list` holds `name`.
fn holds(list: &[String], name: &str) -> bool {
    list.iter().any(|n| n == name)
}

/// Remove `name` from `list`; whether it was there.
fn remove(list: &mut Vec<String>, name: &str) -> bool {
    let before = list.len();
    list.retain(|n| n != name);
    list.len() != before
}

/// The note for a name that is already installed.
fn already_on(name: &str) -> String {
    format!("`{name}` is already on")
}

/// The note for a name that is already left out.
fn already_off(name: &str) -> String {
    format!("`{name}` is already off")
}

/// `allow`: plain `true`, keeping the optional artifacts that were taken.
/// From `{only}` a name is kept in `also` when it may be optional, the same
/// test `take` applies: the listing marks it optional, or its kind's root is
/// missing so the mark cannot be read.
fn allow(current: Option<&Provision>, lookup: &Lookup<'_>) -> Provision {
    match current {
        Some(Provision::All { also, .. }) => Provision::All {
            except: Vec::new(),
            also: also.clone(),
        },
        Some(Provision::Only(only)) => Provision::All {
            except: Vec::new(),
            also: only
                .iter()
                .filter(|name| lookup.may_be_optional(name))
                .cloned()
                .collect(),
        },
        None | Some(Provision::Deny) => Provision::all(),
    }
}

/// `skip`: leave the names out. Under `true` or an `except` list (and on an
/// undecided domain, which `skip` decides as "everything else"), an optional
/// name that was not taken is already off: it gets the note and no `except`
/// entry, so no map is written for it.
fn skip(
    domain: &str,
    current: Option<&Provision>,
    names: &[&str],
    lookup: &Lookup<'_>,
) -> (Provision, Vec<String>) {
    let mut notes = Vec::new();
    let decision = match current {
        Some(Provision::Deny) => {
            notes.push(format!(
                "the `{domain}` domain is denied, so nothing of it is installed"
            ));
            Provision::Deny
        }
        None | Some(Provision::All { .. }) => {
            let (mut except, mut also) = match current {
                Some(Provision::All { except, also }) => (except.clone(), also.clone()),
                _ => (Vec::new(), Vec::new()),
            };
            for &name in names {
                if remove(&mut also, name) {
                    continue;
                }
                if holds(&except, name) || lookup.is_optional(name) {
                    notes.push(already_off(name));
                } else {
                    except.push(name.to_string());
                }
            }
            Provision::All { except, also }
        }
        Some(Provision::Only(only)) => {
            let mut only = only.clone();
            for &name in names {
                if !remove(&mut only, name) {
                    notes.push(already_off(name));
                }
            }
            Provision::Only(only)
        }
    };
    (decision, notes)
}

/// `take`: install the names.
fn take(
    current: Option<&Provision>,
    names: &[&str],
    lookup: &Lookup<'_>,
) -> (Provision, Vec<String>) {
    let mut notes = Vec::new();
    let decision = match current {
        None | Some(Provision::Deny) => {
            Provision::Only(names.iter().map(|n| n.to_string()).collect())
        }
        Some(Provision::All { except, also }) => {
            let (mut except, mut also) = (except.clone(), also.clone());
            for &name in names {
                let was_out = remove(&mut except, name);
                if lookup.may_be_optional(name) && !holds(&also, name) {
                    also.push(name.to_string());
                } else if !was_out {
                    notes.push(already_on(name));
                }
            }
            Provision::All { except, also }
        }
        Some(Provision::Only(only)) => {
            let mut only = only.clone();
            for &name in names {
                if holds(&only, name) {
                    notes.push(already_on(name));
                } else {
                    only.push(name.to_string());
                }
            }
            Provision::Only(only)
        }
    };
    (decision, notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOMAIN: &str = "harbor";

    /// Every kind is present unless a test says otherwise.
    const ALL_KINDS: &[ArtifactType] = &[
        ArtifactType::Skills,
        ArtifactType::Commands,
        ArtifactType::Agents,
        ArtifactType::Mcps,
    ];

    fn row(key: &str, optional: bool) -> ArtifactListing {
        let (kind, name) = parse_artifact_key(key).expect("a well-formed key");
        ArtifactListing {
            kind,
            key: key.to_string(),
            name: name.to_string(),
            optional,
        }
    }

    /// The harbor domain: two required artifacts and two optional ones.
    fn shipped() -> Vec<ArtifactListing> {
        vec![
            row("skills/tide-tables", false),
            row("skills/charts", true),
            row("commands/charts/plot-route", false),
            row("mcps/lighthouse", true),
        ]
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|n| n.to_string()).collect()
    }

    fn except_also(except: &[&str], also: &[&str]) -> Provision {
        Provision::All {
            except: names(except),
            also: names(also),
        }
    }

    fn only(list: &[&str]) -> Provision {
        Provision::Only(names(list))
    }

    fn run(
        current: Option<&Provision>,
        change: SelectionChange,
        given: &[&str],
    ) -> Result<SelectionOutcome, String> {
        change_selection(
            DOMAIN,
            current,
            change,
            &names(given),
            &shipped(),
            ALL_KINDS,
        )
    }

    fn ok(
        current: Option<&Provision>,
        change: SelectionChange,
        given: &[&str],
    ) -> SelectionOutcome {
        run(current, change, given).expect("the change is accepted")
    }

    // --- skip and take, one test per cell --------------------------------------

    #[test]
    fn skip_on_undecided_writes_except() {
        let out = ok(
            None,
            SelectionChange::Skip,
            &["skills/tide-tables", "mcps/lighthouse"],
        );
        // `mcps/lighthouse` is optional: deciding "everything else" leaves it
        // out already, so it needs no `except` entry.
        assert_eq!(out.decision, except_also(&["skills/tide-tables"], &[]));
        assert_eq!(out.notes, ["`mcps/lighthouse` is already off"]);
        assert!(out.writes_map());
    }

    #[test]
    fn skip_of_an_optional_name_that_is_off_changes_nothing() {
        let out = ok(
            Some(&Provision::all()),
            SelectionChange::Skip,
            &["mcps/lighthouse"],
        );
        assert_eq!(out.decision, Provision::all());
        assert_eq!(out.notes, ["`mcps/lighthouse` is already off"]);
        assert!(!out.writes_map());

        let out = ok(None, SelectionChange::Skip, &["skills/charts"]);
        assert_eq!(out.decision, Provision::all());
        assert_eq!(out.notes, ["`skills/charts` is already off"]);
        assert!(!out.writes_map());

        let current = except_also(&["skills/tide-tables"], &[]);
        let out = ok(Some(&current), SelectionChange::Skip, &["skills/charts"]);
        assert_eq!(out.decision, current);
        assert_eq!(out.notes, ["`skills/charts` is already off"]);
    }

    #[test]
    fn skip_of_a_name_of_a_missing_kind_still_writes_except() {
        // The mark of an artifact whose root is missing cannot be read, and it
        // may be required, so `skip` records it.
        let out = change_selection(
            DOMAIN,
            Some(&Provision::all()),
            SelectionChange::Skip,
            &names(&["agents/scout"]),
            &shipped(),
            &[
                ArtifactType::Skills,
                ArtifactType::Commands,
                ArtifactType::Mcps,
            ],
        )
        .expect("the change is accepted");
        assert_eq!(out.decision, except_also(&["agents/scout"], &[]));
        assert!(out.notes.is_empty());
    }

    #[test]
    fn take_on_undecided_writes_only() {
        let out = ok(
            None,
            SelectionChange::Take,
            &["skills/tide-tables", "mcps/lighthouse"],
        );
        assert_eq!(
            out.decision,
            only(&["mcps/lighthouse", "skills/tide-tables"])
        );
        assert!(out.notes.is_empty());
    }

    #[test]
    fn skip_on_false_changes_nothing_and_notes_it() {
        let out = ok(
            Some(&Provision::Deny),
            SelectionChange::Skip,
            &["skills/tide-tables", "skills/charts"],
        );
        assert_eq!(out.decision, Provision::Deny);
        assert_eq!(
            out.notes,
            ["the `harbor` domain is denied, so nothing of it is installed"]
        );
        assert!(!out.writes_map());
    }

    #[test]
    fn take_on_false_writes_only() {
        let out = ok(
            Some(&Provision::Deny),
            SelectionChange::Take,
            &["skills/charts"],
        );
        assert_eq!(out.decision, only(&["skills/charts"]));
        assert!(out.notes.is_empty());
    }

    #[test]
    fn skip_on_true_writes_except() {
        let out = ok(
            Some(&Provision::all()),
            SelectionChange::Skip,
            &["skills/tide-tables"],
        );
        assert_eq!(out.decision, except_also(&["skills/tide-tables"], &[]));
        assert!(out.notes.is_empty());
    }

    #[test]
    fn take_on_true_moves_optional_names_to_also() {
        let out = ok(
            Some(&Provision::all()),
            SelectionChange::Take,
            &["mcps/lighthouse", "skills/charts"],
        );
        assert_eq!(
            out.decision,
            except_also(&[], &["mcps/lighthouse", "skills/charts"])
        );
        assert!(out.notes.is_empty());
    }

    #[test]
    fn take_on_true_of_a_required_name_notes_already_on() {
        let out = ok(
            Some(&Provision::all()),
            SelectionChange::Take,
            &["skills/tide-tables"],
        );
        assert_eq!(out.decision, Provision::all());
        assert_eq!(out.notes, ["`skills/tide-tables` is already on"]);
        assert!(!out.writes_map());
    }

    #[test]
    fn skip_removes_from_also_first() {
        let current = except_also(
            &["skills/tide-tables"],
            &["skills/charts", "mcps/lighthouse"],
        );
        let out = ok(
            Some(&current),
            SelectionChange::Skip,
            &[
                "skills/charts",
                "commands/charts/plot-route",
                "skills/tide-tables",
            ],
        );
        assert_eq!(
            out.decision,
            except_also(
                &["commands/charts/plot-route", "skills/tide-tables"],
                &["mcps/lighthouse"]
            )
        );
        assert_eq!(out.notes, ["`skills/tide-tables` is already off"]);
    }

    #[test]
    fn skip_of_the_last_except_name_writes_true() {
        let current = except_also(&[], &["skills/charts"]);
        let out = ok(Some(&current), SelectionChange::Skip, &["skills/charts"]);
        assert_eq!(out.decision, Provision::all());
        assert!(!out.writes_map());
    }

    #[test]
    fn take_removes_from_except_and_adds_an_optional_name_to_also() {
        let current = except_also(
            &["skills/tide-tables", "skills/charts"],
            &["mcps/lighthouse"],
        );
        let out = ok(
            Some(&current),
            SelectionChange::Take,
            &[
                "skills/tide-tables",
                "skills/charts",
                "mcps/lighthouse",
                "commands/charts/plot-route",
            ],
        );
        // The required skill leaves `except`; the optional one leaves it and
        // joins `also`; the taken MCP and the required command were on.
        assert_eq!(
            out.decision,
            except_also(&[], &["mcps/lighthouse", "skills/charts"])
        );
        assert_eq!(
            out.notes,
            [
                "`mcps/lighthouse` is already on",
                "`commands/charts/plot-route` is already on",
            ]
        );
    }

    #[test]
    fn skip_under_only_removes_and_the_last_one_writes_false() {
        let current = only(&["skills/charts", "skills/tide-tables"]);
        let out = ok(
            Some(&current),
            SelectionChange::Skip,
            &["skills/charts", "mcps/lighthouse"],
        );
        assert_eq!(out.decision, only(&["skills/tide-tables"]));
        assert_eq!(out.notes, ["`mcps/lighthouse` is already off"]);

        let out = ok(
            Some(&out.decision),
            SelectionChange::Skip,
            &["skills/tide-tables"],
        );
        assert_eq!(out.decision, Provision::Deny);
        assert!(out.notes.is_empty());
    }

    #[test]
    fn take_under_only_adds() {
        let current = only(&["skills/tide-tables"]);
        let out = ok(
            Some(&current),
            SelectionChange::Take,
            &["mcps/lighthouse", "skills/tide-tables"],
        );
        assert_eq!(
            out.decision,
            only(&["mcps/lighthouse", "skills/tide-tables"])
        );
        assert_eq!(out.notes, ["`skills/tide-tables` is already on"]);
    }

    #[test]
    fn a_repeated_name_counts_once() {
        let out = ok(
            Some(&Provision::all()),
            SelectionChange::Take,
            &["skills/tide-tables", "skills/tide-tables"],
        );
        assert_eq!(out.notes, ["`skills/tide-tables` is already on"]);
    }

    // --- which names are accepted ----------------------------------------------

    #[test]
    fn a_name_the_domain_does_not_ship_is_refused_naming_what_it_ships() {
        let err = run(None, SelectionChange::Skip, &["skills/tide-table"]).unwrap_err();
        assert_eq!(
            err,
            "The `harbor` domain does not ship `skills/tide-table`. It ships \
             `commands/charts/plot-route`, `mcps/lighthouse`, `skills/charts`, \
             `skills/tide-tables`. See them with `crystalline provision status`."
        );
    }

    #[test]
    fn a_domain_that_ships_nothing_says_so_in_the_refusal() {
        let err = change_selection(
            DOMAIN,
            None,
            SelectionChange::Take,
            &names(&["skills/tide-tables"]),
            &[],
            &[ArtifactType::Skills],
        )
        .unwrap_err();
        assert_eq!(
            err,
            "The `harbor` domain does not ship `skills/tide-tables`. It ships no artifacts."
        );
    }

    #[test]
    fn names_compare_exactly() {
        let err = run(None, SelectionChange::Take, &["skills/Tide-Tables"]).unwrap_err();
        assert!(err.starts_with("The `harbor` domain does not ship `skills/Tide-Tables`."));
    }

    #[test]
    fn a_stale_listed_name_can_be_removed() {
        // `skills/gone` is no longer shipped, but the decision names it.
        let current = except_also(&["skills/gone"], &["mcps/gone"]);
        let out = ok(Some(&current), SelectionChange::Take, &["skills/gone"]);
        assert_eq!(out.decision, except_also(&[], &["mcps/gone"]));
        let out = ok(Some(&out.decision), SelectionChange::Skip, &["mcps/gone"]);
        assert_eq!(out.decision, Provision::all());

        let current = only(&["skills/gone", "skills/tide-tables"]);
        let out = ok(Some(&current), SelectionChange::Skip, &["skills/gone"]);
        assert_eq!(out.decision, only(&["skills/tide-tables"]));
    }

    #[test]
    fn a_malformed_name_already_in_a_list_can_be_removed() {
        let current = except_also(&["skills"], &[]);
        let out = ok(Some(&current), SelectionChange::Take, &["skills"]);
        assert_eq!(out.decision, Provision::all());
    }

    #[test]
    fn a_malformed_name_is_refused() {
        for (given, shown) in [
            ("skills", "skills"),
            ("tools/x", "tools/x"),
            ("skills/a/b", "skills/a/b"),
            ("skills/", "skills/"),
        ] {
            let err = run(Some(&Provision::all()), SelectionChange::Take, &[given]).unwrap_err();
            assert_eq!(
                err,
                format!(
                    "`{shown}` is not an artifact name; use `<kind>/<name>`, for example `skills/tide-tables`."
                )
            );
        }
    }

    #[test]
    fn a_malformed_name_is_refused_even_on_a_denied_domain() {
        let err = run(Some(&Provision::Deny), SelectionChange::Skip, &["skills"]).unwrap_err();
        assert!(err.starts_with("`skills` is not an artifact name"));
    }

    #[test]
    fn one_bad_name_refuses_the_whole_change() {
        let err = run(
            None,
            SelectionChange::Skip,
            &["skills/tide-tables", "skills/nope", "mcps/lighthouse"],
        )
        .unwrap_err();
        assert!(err.starts_with("The `harbor` domain does not ship `skills/nope`."));
        let err = run(
            None,
            SelectionChange::Take,
            &["skills/tide-tables", "skills"],
        )
        .unwrap_err();
        assert!(err.starts_with("`skills` is not an artifact name"));
    }

    #[test]
    fn every_unshipped_name_is_named_in_one_refusal() {
        let err = run(None, SelectionChange::Take, &["skills/a", "mcps/b"]).unwrap_err();
        assert!(err.starts_with("The `harbor` domain does not ship `skills/a`, `mcps/b`."));
    }

    #[test]
    fn take_accepts_any_well_formed_name_of_a_kind_whose_root_is_missing() {
        let present = [ArtifactType::Skills];
        let out = change_selection(
            DOMAIN,
            None,
            SelectionChange::Take,
            &names(&["agents/quartermaster", "skills/tide-tables"]),
            &shipped(),
            &present,
        )
        .unwrap();
        assert_eq!(
            out.decision,
            only(&["agents/quartermaster", "skills/tide-tables"])
        );

        // Under `true` the mark of such a name cannot be read, so it is kept
        // in `also` in case it is optional.
        let out = change_selection(
            DOMAIN,
            Some(&Provision::all()),
            SelectionChange::Take,
            &names(&["agents/quartermaster"]),
            &shipped(),
            &present,
        )
        .unwrap();
        assert_eq!(out.decision, except_also(&[], &["agents/quartermaster"]));

        // A present kind still refuses an unshipped name.
        let err = change_selection(
            DOMAIN,
            None,
            SelectionChange::Take,
            &names(&["skills/nope"]),
            &shipped(),
            &present,
        )
        .unwrap_err();
        assert!(err.starts_with("The `harbor` domain does not ship `skills/nope`."));
    }

    #[test]
    fn skip_and_take_need_at_least_one_name() {
        assert_eq!(
            run(None, SelectionChange::Skip, &[]).unwrap_err(),
            "`skip` needs at least one artifact name, for example `skills/tide-tables`."
        );
        assert_eq!(
            run(Some(&Provision::all()), SelectionChange::Take, &[]).unwrap_err(),
            "`take` needs at least one artifact name, for example `skills/tide-tables`."
        );
    }

    // --- allow and deny ----------------------------------------------------------

    #[test]
    fn allow_after_except_keeps_also() {
        let current = except_also(&["skills/tide-tables"], &["mcps/lighthouse"]);
        let out = ok(Some(&current), SelectionChange::Allow, &[]);
        assert_eq!(out.decision, except_also(&[], &["mcps/lighthouse"]));
        assert!(out.writes_map());

        let current = except_also(&["skills/tide-tables"], &[]);
        let out = ok(Some(&current), SelectionChange::Allow, &[]);
        assert_eq!(out.decision, Provision::all());
        assert!(!out.writes_map());
    }

    #[test]
    fn allow_after_only_keeps_the_optional_names_as_also() {
        let current = only(&["skills/charts", "skills/tide-tables", "mcps/lighthouse"]);
        let out = ok(Some(&current), SelectionChange::Allow, &[]);
        assert_eq!(
            out.decision,
            except_also(&[], &["mcps/lighthouse", "skills/charts"])
        );

        let current = only(&["skills/tide-tables"]);
        let out = ok(Some(&current), SelectionChange::Allow, &[]);
        assert_eq!(out.decision, Provision::all());
    }

    #[test]
    fn allow_after_only_keeps_a_name_of_a_missing_kind_as_also() {
        // `agents/scout` cannot be looked up (the agents root is not on this
        // machine), so it may be optional: `allow` keeps it, as `take` would
        // have recorded it.
        let current = only(&["agents/scout", "skills/charts", "skills/tide-tables"]);
        let out = change_selection(
            DOMAIN,
            Some(&current),
            SelectionChange::Allow,
            &[],
            &shipped(),
            &[
                ArtifactType::Skills,
                ArtifactType::Commands,
                ArtifactType::Mcps,
            ],
        )
        .expect("the change is accepted");
        assert_eq!(
            out.decision,
            except_also(&[], &["agents/scout", "skills/charts"])
        );
    }

    #[test]
    fn allow_on_undecided_writes_true() {
        for current in [None, Some(&Provision::all()), Some(&Provision::Deny)] {
            let out = ok(current, SelectionChange::Allow, &[]);
            assert_eq!(out.decision, Provision::all());
            assert!(out.notes.is_empty());
        }
    }

    #[test]
    fn deny_drops_every_list() {
        for current in [
            None,
            Some(&Provision::all()),
            Some(&except_also(&["skills/tide-tables"], &["mcps/lighthouse"])),
            Some(&only(&["skills/charts"])),
        ] {
            let out = ok(current, SelectionChange::Deny, &[]);
            assert_eq!(out.decision, Provision::Deny);
            assert!(out.notes.is_empty());
        }
    }
}
