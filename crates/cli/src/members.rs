//! `crystalline domain members`, `domain visibility` and `domain transfer`:
//! who may reach a private domain, administered from the machine that holds
//! the files.
//!
//! **This command bypasses the role gates the web API enforces, by design.**
//! Whoever can run it already has the domain's files on disk and the accounts
//! database beside them, so a check here would protect nothing - it is the
//! same reasoning `crate::users` rests on, and the same [`Scope::Unrestricted`]
//! every CLI verb is served as. What it means in practice is that the machine
//! operator administers *every* domain, including a private one they were
//! never invited to, and the help text says so rather than leaving it to be
//! discovered.
//!
//! Two things this surface owns that the store cannot:
//!
//! * **the registry check.** The accounts database keys its records by domain
//!   name and knows nothing about which domains exist, so a typo would
//!   otherwise mint an acl row for a domain nobody registered - a private
//!   domain with no content, invisible until somebody registered that name and
//!   found it already closed. Every verb here resolves the name against the
//!   global config first;
//! * **the owner check, before anything else happens.** `domain add --private`
//!   registers a domain and then closes it; an owner that turns out not to
//!   exist between those two steps would leave a REGISTERED and SHARED domain
//!   behind, the opposite of what was asked for. So the account is resolved
//!   before the registration starts.
//!
//! [`Scope::Unrestricted`]: crystalline_service::Scope

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crystalline_service::rest::{AuthStore, DomainMember, MemberLevel, VisibilityWrite};

use crate::{MembersCommand, VisibilityArg};

/// Open the accounts database in the state directory, the same file
/// `crystalline users` writes and the daemon reads.
async fn store() -> Result<AuthStore> {
    let path = crystalline_core::config::web_auth_db_path()?;
    AuthStore::open(&path).await
}

/// Resolve a domain name against the global config, exactly as it is spelled
/// there.
///
/// Deliberately an exact match rather than a folded one: the engine keys its
/// domain map on the literal name, so `Lab` and `lab` are two registrations,
/// and a record written for one must never be able to close the other.
fn require_registered(name: &str, config: Option<&Path>) -> Result<String> {
    let loaded = crate::cmd::load(config)?;
    let name = name.trim();
    if loaded.effective.domains.contains_key(name) {
        return Ok(name.to_string());
    }
    bail!("no domain named '{name}' is registered; list them with `crystalline domain list`")
}

/// Refuse an account the store does not have, or has disabled, before any
/// other work happens.
///
/// The store refuses these too, inside the transaction that would have done
/// the write - this is the same answer one step earlier, so that a `domain add
/// --private` never gets as far as registering a domain it then cannot close.
pub async fn require_account(store: &AuthStore, account: &str) -> Result<String> {
    let name = account.trim().to_lowercase();
    match store.user(&name).await? {
        None => bail!(
            "no account named '{name}'; add one with `crystalline users add {name} --role editor`"
        ),
        Some(user) if user.disabled => {
            bail!("account '{name}' is disabled; enable it with `crystalline users enable {name}`")
        }
        Some(user) => Ok(user.name),
    }
}

/// The first half of `domain add --private`: resolve the account that will own
/// the domain, before the domain exists.
///
/// Split from [`close_new_domain`] because the ORDER is the point (see the
/// module docs): the account is resolved before the registration starts and
/// the acl row is written after it succeeded, so a name nobody has an account
/// for refuses without a domain having been registered, and an acl row is
/// never written for a name that failed to register.
pub async fn check_private_owner(owner: &str) -> Result<String> {
    let store = store().await?;
    require_account(&store, owner).await
}

/// Close a freshly registered domain, naming `owner`. Called only after the
/// registration reported success.
///
/// The name is resolved against the registry the registration just wrote,
/// rather than taken as typed. That is the same rule the REST path follows by
/// reading the resulting name out of the engine's own report: an acl row for a
/// name the registry does not hold is the orphan record every verb here exists
/// to prevent, and the two surfaces must not differ on it the day `domain add`
/// learns to derive a name it was not given.
///
/// **No rollback here, unlike the REST path, and deliberately.** The web
/// surface unregisters the domain when this write fails, because the browser
/// has no other way to finish the job and a half-made state would sit there
/// invisible. At a terminal there is a person who was just told what happened
/// and has both next steps in the error - close it by hand, or unregister it -
/// and who may well prefer to keep a domain that registered correctly. The
/// realistic failure is gone before this point anyway:
/// [`check_private_owner`] resolved the account before the registration
/// started.
///
/// Under `--json` the confirmation goes to STDERR, the convention
/// `crate::users`' token printing already follows: the registration itself
/// already wrote one JSON document to stdout, and a second one after it would
/// leave a piped stdout holding two, which is not a JSON document at all.
pub async fn close_new_domain(
    domain: &str,
    owner: &str,
    config: Option<&Path>,
    json: bool,
) -> Result<()> {
    let domain = &require_registered(domain, config)?;
    let store = store().await?;
    let written = store
        .set_domain_visibility(domain, true, owner)
        .await
        .with_context(|| {
            format!(
                "domain '{domain}' is registered and SHARED: making it private failed. \
                 Close it with `crystalline domain visibility {domain} private --owner \
                 {}`, or drop it with `crystalline domain remove {domain}`",
                owner.trim().to_lowercase()
            )
        })?;
    // The name can already carry a visibility record: a domain that was private
    // under an earlier registration whose records outlived it, or one closed
    // against a name nobody had registered yet. Owned by the account just named
    // it is what was asked for. Owned by anybody else it is not, and this write
    // no longer takes it over, so saying "private, owned by <you>" would be a
    // lie about who can see the domain that was just created.
    if let VisibilityWrite::AlreadyPrivate { owner: held } = written
        && held != owner.trim().to_lowercase()
    {
        bail!(
            "domain '{domain}' is registered, and the name already carried a \
             private-domain record owned by '{held}' - so it is PRIVATE TO \
             '{held}' rather than to '{}'. Hand it over with `crystalline domain \
             transfer {domain} {}`, or drop it with `crystalline domain remove \
             {domain}`",
            owner.trim().to_lowercase(),
            owner.trim().to_lowercase()
        );
    }
    let line = format!(
        "Domain '{domain}' is private, owned by '{}'. Only its owner, the \
         accounts invited into it and instance admins see it.",
        owner.trim().to_lowercase()
    );
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
    Ok(())
}

/// `crystalline domain members <domain> ...`.
pub async fn run(domain: String, command: MembersCommand, json: bool) -> Result<()> {
    // Each subcommand carries its own `--config`, the way every other domain
    // verb does, so it can be typed after the verb rather than between the
    // domain and it.
    let config = match &command {
        MembersCommand::List { config }
        | MembersCommand::Add { config, .. }
        | MembersCommand::Remove { config, .. } => config.clone(),
    };
    let domain = require_registered(&domain, config.as_deref())?;
    let store = store().await?;
    match command {
        MembersCommand::List { .. } => {
            let acl = store.domain_visibility(&domain).await?;
            let members = match &acl {
                Some(_) => store.domain_members(&domain).await?,
                // A shared domain has no membership at all: every account's
                // right on it comes from its instance role, so any rows left
                // over would say nothing true.
                None => Vec::new(),
            };
            let owner = acl
                .as_ref()
                .map(|acl| acl.owner.clone())
                .filter(|owner| !owner.is_empty());
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": if acl.is_some() { "private" } else { "shared" },
                        "owner": owner,
                        "members": members,
                    }),
                    true,
                );
            } else {
                print_members(&domain, acl.is_some(), owner.as_deref(), &members);
            }
        }
        MembersCommand::Add { user, level, .. } => {
            let level: MemberLevel = level.into();
            let user = require_account(&store, &user).await?;
            store
                .upsert_domain_member(&domain, &user, level, "cli")
                .await?;
            println!("'{user}' is a {level} on domain '{domain}'.");
        }
        MembersCommand::Remove { user, .. } => {
            let name = user.trim().to_lowercase();
            // The owner holds no membership row, so a plain "not a member"
            // would be true and useless. Name the verb that does change it.
            if let Some(acl) = store.domain_visibility(&domain).await?
                && acl.owner == name
            {
                bail!(
                    "'{name}' owns domain '{domain}' and holds no membership row; \
                     hand the domain on with `crystalline domain transfer {domain} <account>`"
                );
            }
            if !store.remove_domain_member(&domain, &name).await? {
                bail!("'{name}' is not a member of domain '{domain}'");
            }
            println!("Removed '{name}' from domain '{domain}'.");
        }
    }
    Ok(())
}

/// `crystalline domain visibility <domain> private|default`.
pub async fn visibility(
    domain: String,
    visibility: VisibilityArg,
    owner: Option<String>,
    config: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let domain = require_registered(&domain, config.as_deref())?;
    let store = store().await?;
    match visibility {
        VisibilityArg::Private => {
            // The store needs a live account to name as the owner, and there
            // is no calling account here to fall back on: the CLI is the
            // machine, not a person.
            let Some(owner) = owner else {
                bail!(
                    "making domain '{domain}' private needs an account to own it: \
                     pass --owner <account> (`crystalline users list` shows them)"
                );
            };
            let owner = require_account(&store, &owner).await?;
            // A domain that is already private is left exactly as it stands,
            // owner and members included: this verb states a visibility, and
            // handing a domain on is `crystalline domain transfer`. Saying so
            // matters most here, because the owner reported back is the one it
            // already had rather than the one just asked for.
            let owner = match store.set_domain_visibility(&domain, true, &owner).await? {
                VisibilityWrite::Written => owner,
                VisibilityWrite::AlreadyPrivate { owner: held } => {
                    if json {
                        crate::print_value(
                            &serde_json::json!({
                                "domain": domain,
                                "visibility": "private",
                                "owner": held,
                                "changed": false,
                            }),
                            true,
                        );
                    } else {
                        println!(
                            "Domain '{domain}' was already private, owned by '{held}'. \
                             Nothing changed: its members are as they were. Hand it to \
                             somebody else with `crystalline domain transfer {domain} \
                             <account>`."
                        );
                    }
                    return Ok(());
                }
            };
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": "private",
                        "owner": owner,
                        "changed": true,
                    }),
                    true,
                );
            } else {
                println!(
                    "Domain '{domain}' is private, owned by '{owner}'. Only its \
                     owner, the accounts invited into it and instance admins see it."
                );
            }
        }
        VisibilityArg::Default => {
            if owner.is_some() {
                bail!(
                    "`domain visibility {domain} default` takes no --owner: a shared \
                     domain has no owner"
                );
            }
            // The owner argument is ignored on this side by the store, which
            // is why the line above refuses one rather than passing it along:
            // a caller who supplied one meant something this cannot do.
            store.set_domain_visibility(&domain, false, "cli").await?;
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": "shared",
                        "owner": serde_json::Value::Null,
                    }),
                    true,
                );
            } else {
                println!(
                    "Domain '{domain}' is shared: every account sees it again, and \
                     who was invited into it is forgotten."
                );
            }
        }
    }
    Ok(())
}

/// `crystalline domain transfer <domain> <new-owner>`.
pub async fn transfer(
    domain: String,
    new_owner: String,
    config: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    let domain = require_registered(&domain, config.as_deref())?;
    let store = store().await?;
    let new_owner = require_account(&store, &new_owner).await?;
    store.transfer_domain(&domain, &new_owner).await?;
    if json {
        crate::print_value(
            &serde_json::json!({
                "domain": domain,
                "visibility": "private",
                "owner": new_owner,
            }),
            true,
        );
    } else {
        println!(
            "Domain '{domain}' now belongs to '{new_owner}'. The previous owner \
             keeps nothing: invite them back with `crystalline domain members \
             {domain} add <account>` if they should stay."
        );
    }
    Ok(())
}

/// One line per member: who, at what level, put there by whom and when.
///
/// The owner is stated above the table rather than folded into it, because it
/// is a property of the domain and not a membership row - and an owner nobody
/// holds any more (its account was removed) is stated as exactly that rather
/// than as a blank cell, which is the one rendering that could be mistaken for
/// an account whose name is empty.
fn print_members(domain: &str, private: bool, owner: Option<&str>, members: &[DomainMember]) {
    if !private {
        println!("Domain '{domain}' is shared: every account sees it.");
        println!(
            "Make it private with: crystalline domain visibility {domain} private --owner <account>"
        );
        return;
    }
    match owner {
        Some(owner) => println!("Domain '{domain}' is private, owned by '{owner}'."),
        None => println!(
            "Domain '{domain}' is private and owned by nobody: its owner's account \
             was removed, so only instance admins reach it. Give it an owner with: \
             crystalline domain transfer {domain} <account>"
        ),
    }
    println!();
    if members.is_empty() {
        println!(
            "Nobody else is invited. Invite one with: \
             crystalline domain members {domain} add <account> --level editor"
        );
        return;
    }
    let rows: Vec<[String; 4]> = members
        .iter()
        .map(|m| {
            [
                m.principal.clone(),
                m.level.to_string(),
                m.added_by.clone(),
                m.added_at.clone(),
            ]
        })
        .collect();
    let header = ["NAME", "LEVEL", "ADDED BY", "ADDED AT"];
    // The last column is never padded, so no line carries trailing whitespace.
    let widths: Vec<usize> = (0..3)
        .map(|c| {
            rows.iter()
                .map(|r| r[c].chars().count())
                .chain(std::iter::once(header[c].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: [&str; 4]| {
        let mut out = String::new();
        for (c, cell) in cells.iter().enumerate().take(3) {
            out.push_str(&format!("{cell:<width$}  ", width = widths[c]));
        }
        out.push_str(cells[3]);
        println!("{}", out.trim_end());
    };
    line(header);
    for row in &rows {
        line([&row[0], &row[1], &row[2], &row[3]]);
    }
    println!();
    println!(
        "{} member{} of '{domain}'. Remove one with: \
         crystalline domain members {domain} remove <account>",
        members.len(),
        if members.len() == 1 { "" } else { "s" }
    );
}
