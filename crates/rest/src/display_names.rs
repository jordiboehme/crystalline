//! Display names beside the login names an answer shows.
//!
//! Stored data keeps the login name: it is the stable identity, it is written
//! to files that are backed up, and a display name can change. So an answer
//! that names a person gets the display name resolved when it is built, as an
//! optional field beside the login: `display` inside an object that already
//! has `name` (a thread entry's `author`), or `<field>_display` beside a plain
//! string field (`changed_by_display`, `owner_display`). A name with no
//! account, or an account whose display name is its login, gets no field, and
//! the UI shows the login as it is.
//!
//! The lookup adds a readable form to a name the answer already shows, so it
//! checks no rights of its own. When the accounts store cannot be read the
//! answer goes out without display names rather than failing.

use std::collections::{BTreeSet, HashMap};

use serde_json::{Map, Value};

use crate::auth_store::AuthStore;

/// Login name to display name, for the names that have one.
pub(crate) type Displays = HashMap<String, String>;

/// The display names of `names`; empty when the store cannot be read.
pub(crate) async fn lookup<'a>(
    auth: &AuthStore,
    names: impl IntoIterator<Item = &'a str>,
) -> Displays {
    match auth.display_names(names).await {
        Ok(found) => found,
        Err(e) => {
            tracing::warn!(error = %e, "display names could not be read; answering logins only");
            Displays::new()
        }
    }
}

/// The display name of one login, or `None`.
pub(crate) async fn one(auth: &AuthStore, name: &str) -> Option<String> {
    lookup(auth, [name]).await.remove(name)
}

/// Set `<field>_display` on `obj` when its string field `field` names an
/// account with a display name.
pub(crate) fn set_beside(obj: &mut Map<String, Value>, field: &str, displays: &Displays) {
    let display = obj
        .get(field)
        .and_then(Value::as_str)
        .and_then(|name| displays.get(name))
        .cloned();
    if let Some(display) = display {
        obj.insert(format!("{field}_display"), Value::String(display));
    }
}

/// The value of `<field>_display` for a login, as an `Option` for a typed
/// answer.
pub(crate) fn beside(name: Option<&str>, displays: &Displays) -> Option<String> {
    name.and_then(|n| displays.get(n)).cloned()
}

/// Add display names to a Reflection answer, wherever it names a person:
/// a person's `author` (`{ name, agent }`, in thread entries), the entries
/// of `people` (the summary), the logins of `changed_by` (queue items, as
/// `changed_by_display`, a map from login to display name), `disclosed.by`
/// (as `disclosed.by_display`) and `refresh_requested.by` (as
/// `refresh_requested.by_display`). An agent keeps its name: only a
/// person's name is looked up.
pub(crate) async fn enrich_reflection(auth: &AuthStore, answer: &mut Value) {
    let mut names = BTreeSet::new();
    collect(answer, &mut names);
    if names.is_empty() {
        return;
    }
    let displays = lookup(auth, names.iter().map(String::as_str)).await;
    if displays.is_empty() {
        return;
    }
    annotate(answer, &displays);
}

/// Add `<key>_display` beside every string field named one of `keys`,
/// anywhere in `answer`, when it names an account with a display name other
/// than its login.
pub(crate) async fn enrich_fields(auth: &AuthStore, answer: &mut Value, keys: &[&str]) {
    fn collect(v: &Value, keys: &[&str], names: &mut BTreeSet<String>) {
        match v {
            Value::Object(obj) => {
                for (k, c) in obj {
                    if keys.contains(&k.as_str())
                        && let Some(n) = c.as_str()
                    {
                        names.insert(n.to_string());
                    }
                    collect(c, keys, names);
                }
            }
            Value::Array(items) => items.iter().for_each(|i| collect(i, keys, names)),
            _ => {}
        }
    }
    fn annotate(v: &mut Value, keys: &[&str], displays: &Displays) {
        match v {
            Value::Object(obj) => {
                for key in keys {
                    set_beside(obj, key, displays);
                }
                obj.values_mut().for_each(|c| annotate(c, keys, displays));
            }
            Value::Array(items) => items.iter_mut().for_each(|i| annotate(i, keys, displays)),
            _ => {}
        }
    }
    let mut names = BTreeSet::new();
    collect(answer, keys, &mut names);
    if names.is_empty() {
        return;
    }
    let displays = lookup(auth, names.iter().map(String::as_str)).await;
    if !displays.is_empty() {
        annotate(answer, keys, &displays);
    }
}

/// Add display names to an answer about draft holders (the sync status's
/// `drafts`, the domain's `actors`, a review-mode plan): `actor_display`
/// beside every string `actor`, and `actors_display`, a map from login to
/// display name, beside every list of names `actors`. A draft holder is an
/// account, an agent or the machine owner; only an account has a display
/// name.
pub(crate) async fn enrich_actors(auth: &AuthStore, answer: &mut Value) {
    let mut names = BTreeSet::new();
    collect_actors(answer, &mut names);
    if names.is_empty() {
        return;
    }
    let displays = lookup(auth, names.iter().map(String::as_str)).await;
    if !displays.is_empty() {
        annotate_actors(answer, &displays);
    }
}

fn collect_actors(v: &Value, names: &mut BTreeSet<String>) {
    match v {
        Value::Object(obj) => {
            if let Some(a) = obj.get("actor").and_then(Value::as_str) {
                names.insert(a.to_string());
            }
            for a in obj
                .get("actors")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                names.insert(a.to_string());
            }
            obj.values().for_each(|c| collect_actors(c, names));
        }
        Value::Array(items) => items.iter().for_each(|i| collect_actors(i, names)),
        _ => {}
    }
}

fn annotate_actors(v: &mut Value, displays: &Displays) {
    match v {
        Value::Object(obj) => {
            set_beside(obj, "actor", displays);
            let listed: Option<Map<String, Value>> =
                obj.get("actors").and_then(Value::as_array).map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|n| {
                            displays
                                .get(n)
                                .map(|d| (n.to_string(), Value::String(d.clone())))
                        })
                        .collect()
                });
            if let Some(map) = listed.filter(|m| !m.is_empty()) {
                obj.insert("actors_display".to_string(), Value::Object(map));
            }
            obj.values_mut().for_each(|c| annotate_actors(c, displays));
        }
        Value::Array(items) => items.iter_mut().for_each(|i| annotate_actors(i, displays)),
        _ => {}
    }
}

/// The account a change's actor names: the login inside an OKF person
/// actor (`human:<login>`), else the label as it is (an account's own name
/// on a change announced by scope).
pub(crate) fn actor_account(actor: &str) -> &str {
    human_login(actor).unwrap_or(actor)
}

/// The login inside an OKF actor that names a person (`human:<login>`).
fn human_login(actor: &str) -> Option<&str> {
    let (kind, login) = actor.split_once(':')?;
    (kind.eq_ignore_ascii_case("human") && !login.is_empty()).then_some(login)
}

/// Add display names to an engram read: `draft_owner_display` beside
/// `draft_owner`, and `actor_display`, a map from each person's actor in the
/// frontmatter's `generated.by` and `verified[].by` (`human:<login>`) to that
/// person's display name. The frontmatter itself is left as written.
pub(crate) async fn enrich_engram(auth: &AuthStore, read: &mut Value) {
    let Some(obj) = read.as_object_mut() else {
        return;
    };
    let fm = obj.get("frontmatter");
    let extra = fm.and_then(|f| f.get("extra"));
    let mut actors: Vec<String> = Vec::new();
    if let Some(by) = fm
        .and_then(|f| f.get("generated"))
        .and_then(|g| g.get("by"))
        .and_then(Value::as_str)
    {
        actors.push(by.to_string());
    }
    // The trail is a typed field, or under `extra` when its entries are not
    // in the typed shape (a bare date, say): a reader shows either.
    for v in [fm, extra]
        .into_iter()
        .flatten()
        .filter_map(|f| f.get("verified").and_then(Value::as_array))
        .flatten()
    {
        if let Some(by) = v.get("by").and_then(Value::as_str) {
            actors.push(by.to_string());
        }
    }
    actors.retain(|a| human_login(a).is_some());
    actors.sort();
    actors.dedup();
    let owner = obj.get("draft_owner").and_then(Value::as_str);
    if actors.is_empty() && owner.is_none() {
        return;
    }
    let displays = lookup(
        auth,
        actors.iter().filter_map(|a| human_login(a)).chain(owner),
    )
    .await;
    set_beside(obj, "draft_owner", &displays);
    let by_actor: Map<String, Value> = actors
        .iter()
        .filter_map(|a| {
            let display = displays.get(human_login(a)?)?;
            Some((a.clone(), Value::String(display.clone())))
        })
        .collect();
    if !by_actor.is_empty() {
        obj.insert("actor_display".to_string(), Value::Object(by_actor));
    }
}

/// A person `{ name, agent: false }`: its name.
fn person_name(v: &Value) -> Option<&str> {
    let obj = v.as_object()?;
    if obj.get("agent").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    obj.get("name").and_then(Value::as_str)
}

fn collect(v: &Value, names: &mut BTreeSet<String>) {
    match v {
        Value::Object(obj) => {
            for (key, child) in obj {
                match key.as_str() {
                    "author" => {
                        if let Some(n) = person_name(child) {
                            names.insert(n.to_string());
                        }
                    }
                    "people" => {
                        for p in child.as_array().into_iter().flatten() {
                            if let Some(n) = person_name(p) {
                                names.insert(n.to_string());
                            }
                        }
                    }
                    "changed_by" => {
                        for n in child.as_array().into_iter().flatten() {
                            if let Some(n) = n.as_str() {
                                names.insert(n.to_string());
                            }
                        }
                    }
                    "disclosed" | "refresh_requested" => {
                        if let Some(n) = child.get("by").and_then(Value::as_str) {
                            names.insert(n.to_string());
                        }
                    }
                    _ => {}
                }
                collect(child, names);
            }
        }
        Value::Array(items) => items.iter().for_each(|i| collect(i, names)),
        _ => {}
    }
}

fn set_person(v: &mut Value, displays: &Displays) {
    let Some(display) = person_name(v).and_then(|n| displays.get(n)).cloned() else {
        return;
    };
    if let Some(obj) = v.as_object_mut() {
        obj.insert("display".to_string(), Value::String(display));
    }
}

fn annotate(v: &mut Value, displays: &Displays) {
    match v {
        Value::Object(obj) => {
            let changed_by: Option<Map<String, Value>> = obj
                .get("changed_by")
                .and_then(Value::as_array)
                .map(|logins| {
                    logins
                        .iter()
                        .filter_map(Value::as_str)
                        .filter_map(|n| {
                            displays
                                .get(n)
                                .map(|d| (n.to_string(), Value::String(d.clone())))
                        })
                        .collect()
                });
            if let Some(map) = changed_by.filter(|m| !m.is_empty()) {
                obj.insert("changed_by_display".to_string(), Value::Object(map));
            }
            for (key, child) in obj.iter_mut() {
                match key.as_str() {
                    "author" => set_person(child, displays),
                    "people" => {
                        if let Some(items) = child.as_array_mut() {
                            items.iter_mut().for_each(|p| set_person(p, displays));
                        }
                    }
                    "disclosed" | "refresh_requested" => {
                        if let Some(d) = child.as_object_mut() {
                            set_beside(d, "by", displays);
                        }
                    }
                    _ => {}
                }
                annotate(child, displays);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|i| annotate(i, displays)),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn displays() -> Displays {
        HashMap::from([
            ("ada".to_string(), "Ada Lovelace".to_string()),
            ("claude".to_string(), "Not An Agent".to_string()),
        ])
    }

    #[test]
    fn a_reflection_answer_gets_display_names_beside_each_person() {
        let mut v = json!({
            "threads": [ { "entries": [
                { "author": { "name": "ada", "agent": false } },
                { "author": { "name": "claude", "agent": true } },
                { "author": { "name": "bob", "agent": false } },
            ] } ],
            "people": [ { "name": "ada", "agent": false } ],
            "queue": [ { "changed_by": ["ada", "bob"] }, { "changed_by": ["bob"] } ],
            "sections": [ { "disclosed": { "by": "ada", "at": "2026-11-24" } } ],
            "page": { "refresh_requested": { "by": "ada", "at": "2026-11-25T14:03:07.412Z" } },
        });
        let mut names = BTreeSet::new();
        collect(&v, &mut names);
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["ada".to_string(), "bob".to_string()],
            "an agent's name is never looked up"
        );
        annotate(&mut v, &displays());
        let entries = &v["threads"][0]["entries"];
        assert_eq!(entries[0]["author"]["display"], "Ada Lovelace");
        assert!(entries[1]["author"].get("display").is_none());
        assert!(entries[2]["author"].get("display").is_none());
        assert_eq!(v["people"][0]["display"], "Ada Lovelace");
        assert_eq!(
            v["queue"][0]["changed_by_display"],
            json!({ "ada": "Ada Lovelace" })
        );
        assert!(v["queue"][1].get("changed_by_display").is_none());
        assert_eq!(v["sections"][0]["disclosed"]["by_display"], "Ada Lovelace");
        assert_eq!(v["page"]["refresh_requested"]["by_display"], "Ada Lovelace");
    }
}
