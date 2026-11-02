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

use crystalline_service::rest::{
    AuthStore, DomainMember, MemberLevel, ReflectionGrant, ReflectionLevel, VisibilityWrite,
};

use crate::{MembersCommand, ReflectionGrantCommand, VisibilityArg};

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

/// What decides access to a registered domain.
struct AccessRule {
    /// A GitHub App decides it: an App is configured (usable or not) and the
    /// domain has a GitHub origin, the rule the engine's governor follows.
    governed: bool,
    /// The `CRYSTALLINE_DOMAIN_<NAME>_READERS` variable that sets the
    /// domain's readers on every start while an App governs it, when the
    /// environment defines the domain with an origin.
    env_readers_var: Option<String>,
}

/// The one answer every membership change on a domain GitHub governs gets,
/// the web API's sentence.
const FOLLOWS_GITHUB: &str = "access to this domain follows GitHub; change it on GitHub";

/// What decides access to the registered domain `name`: a running daemon's
/// answer, since it may have the App or the domain from an environment this
/// command does not share (a service manager's environment file), and this
/// process's own read only when no daemon runs or `--config` names another
/// file, as `crystalline github app status` decides.
///
/// The local read loads the file fresh, with the environment overlay on
/// top, so the effective config holds the domains the engine counts: its
/// start-up config and the ones registered in the file since.
async fn access_rule(name: &str, config: Option<&Path>) -> Result<AccessRule> {
    if config.is_none()
        && let Some(answer) = crystalline_service::client::ctl_if_running(serde_json::json!({
            "v": 1, "cmd": "github_governs", "domain": name,
        }))
        .await?
    {
        return Ok(AccessRule {
            governed: answer
                .get("governed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
            env_readers_var: answer
                .get("env_readers_var")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
        });
    }
    let loaded = crate::cmd::load(config)?;
    let has_origin = loaded
        .effective
        .domains
        .get(name)
        .is_some_and(|entry| entry.origin.is_some());
    let env_readers_var = loaded
        .overlay
        .env_domains()
        .find(|(domain, env)| domain.as_str() == name && env.entry.origin.is_some())
        .map(|(_, env)| env.readers_var());
    Ok(AccessRule {
        governed: has_origin && crate::cmd::github_app_decides_access(config)?,
        env_readers_var,
    })
}

/// Refuse a membership or owner change on a domain GitHub governs: the rows
/// would decide nothing now and come back into force the day the App is
/// removed.
async fn refuse_governed(name: &str, config: Option<&Path>) -> Result<()> {
    if access_rule(name, config).await?.governed {
        bail!("{FOLLOWS_GITHUB}");
    }
    Ok(())
}

/// [`require_registered`] for a Reflection grant, which only a domain kept
/// as files takes: a virtual domain keeps no Reflection.
fn require_kept_as_files(name: &str, config: Option<&Path>) -> Result<String> {
    let loaded = crate::cmd::load(config)?;
    let name = name.trim().to_string();
    let Some(entry) = loaded.effective.domains.get(&name) else {
        bail!("no domain named '{name}' is registered; list them with `crystalline domain list`")
    };
    if entry.is_virtual() || entry.file_path().is_none() {
        bail!(
            "Reflection collaborators are for domains kept as files; '{name}' is kept in \
             the database"
        );
    }
    Ok(name)
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
    // The Reflection grants resolve the domain themselves: only a domain
    // kept as files takes one.
    let command = match command {
        MembersCommand::Reflection { command } => return reflection(domain, command).await,
        other => other,
    };
    // Each subcommand carries its own `--config`, the way every other domain
    // verb does, so it can be typed after the verb rather than between the
    // domain and it.
    let config = match &command {
        MembersCommand::List { config }
        | MembersCommand::Add { config, .. }
        | MembersCommand::Remove { config, .. } => config.clone(),
        MembersCommand::Reflection { .. } => unreachable!("answered above"),
    };
    let domain = require_registered(&domain, config.as_deref())?;
    if !matches!(command, MembersCommand::List { .. }) {
        refuse_governed(&domain, config.as_deref()).await?;
    }
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
            // Reflection collaborators live on shared and private domains
            // alike.
            let grants = store.reflection_grants(&domain).await?;
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": if acl.is_some() { "private" } else { "shared" },
                        "owner": owner,
                        "members": members,
                        "reflection_collaborators": grants,
                    }),
                    true,
                );
            } else {
                print_members(&domain, acl.is_some(), owner.as_deref(), &members);
                print_grants(&domain, &grants);
            }
        }
        MembersCommand::Add {
            user,
            level,
            reflection,
            ..
        } => {
            let level: MemberLevel = level.into();
            let user = require_account(&store, &user).await?;
            store
                .upsert_domain_member(&domain, &user, level, reflection.map(Into::into), "cli")
                .await?;
            // Read back, since without `--reflection` the stored level stays.
            let reflection = store.member_reflection(&domain, &user).await?;
            println!("'{user}' is a {level} on domain '{domain}' with Reflection {reflection}.");
        }
        MembersCommand::Reflection { .. } => unreachable!("answered above"),
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
    readers_changed().await;
    Ok(())
}

/// `crystalline domain members <domain> reflection add|remove`: a per-domain
/// Reflection collaborator, on any domain kept as files.
async fn reflection(domain: String, command: ReflectionGrantCommand) -> Result<()> {
    match command {
        ReflectionGrantCommand::Add {
            user,
            level,
            config,
        } => {
            let domain = require_kept_as_files(&domain, config.as_deref())?;
            let store = store().await?;
            let user = require_account(&store, &user).await?;
            let level: ReflectionLevel = level.into();
            store
                .set_reflection_grant(&domain, &user, level, "cli")
                .await?;
            println!(
                "'{user}' is a Reflection {level} on domain '{domain}'. It sees no engram \
                 there it could not see before."
            );
        }
        ReflectionGrantCommand::Remove { user, config } => {
            let domain = require_kept_as_files(&domain, config.as_deref())?;
            let store = store().await?;
            let name = user.trim().to_lowercase();
            if !store.remove_reflection_grant(&domain, &name).await? {
                bail!("'{name}' holds no Reflection grant on domain '{domain}'");
            }
            println!("Took the Reflection grant on domain '{domain}' from '{name}'.");
        }
    }
    readers_changed().await;
    Ok(())
}

/// The Reflection collaborators below the membership table, when there are
/// any.
fn print_grants(domain: &str, grants: &[ReflectionGrant]) {
    if grants.is_empty() {
        return;
    }
    println!();
    println!("Reflection collaborators of '{domain}':");
    for grant in grants {
        println!(
            "  {} ({}, added by {} at {})",
            grant.account, grant.level, grant.added_by, grant.added_at
        );
    }
}

/// Tell a running daemon that who may read a domain changed, so it looks
/// for Reflection pages that now cite a domain fewer people may read and
/// backs them up off the branch at once. Never starts a daemon, and a
/// daemon that is not there or does not know the command changes nothing:
/// its recount every few minutes finds the same pages.
async fn readers_changed() {
    let _ = crystalline_service::ctl_if_running_passive(
        serde_json::json!({ "v": 1, "cmd": "readers_changed" }),
    )
    .await;
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
    let rule = access_rule(&domain, config.as_deref()).await?;
    // A governed domain the environment defines applies its readers again
    // on every start, so a change here would not last. Without an App the
    // variable changes nothing, and the domain is like any other.
    if rule.governed
        && let Some(var) = &rule.env_readers_var
    {
        bail!("Set by {var}; change it there.");
    }
    let store = store().await?;
    match visibility {
        VisibilityArg::Private if rule.governed => {
            // GitHub decides access here: the record names the reserved
            // machine owner, as the connect, the env readers and the Members
            // card record it, and no account becomes an owner.
            let written = store
                .set_governed_private(&domain, crystalline_service::engine::OWNER_IDENTITY_NAME)
                .await?;
            let changed = matches!(written, VisibilityWrite::Written);
            let note = owner.map(|owner| {
                format!(
                    "--owner {} was not used: GitHub decides access to domain '{domain}'.",
                    owner.trim().to_lowercase()
                )
            });
            if json {
                if let Some(note) = &note {
                    eprintln!("Note: {note}");
                }
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": "private",
                        "owner": serde_json::Value::Null,
                        "changed": changed,
                    }),
                    true,
                );
            } else {
                if let Some(note) = &note {
                    println!("Note: {note}");
                }
                println!(
                    "Domain '{domain}' is private: only the people with access on GitHub \
                     read it."
                );
            }
        }
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
            // On a domain a GitHub App governs only the private record goes,
            // as on the Members card: the membership rows decide nothing
            // there and are kept for the day the App is removed.
            let governed = rule.governed;
            if governed {
                store.take_back_governed_private(&domain).await?;
            } else {
                store.set_domain_visibility(&domain, false, "cli").await?;
            }
            if json {
                crate::print_value(
                    &serde_json::json!({
                        "domain": domain,
                        "visibility": "shared",
                        "owner": serde_json::Value::Null,
                        "members_kept": governed,
                    }),
                    true,
                );
            } else if governed {
                println!(
                    "Domain '{domain}' is shared: everyone signed in reads it. A GitHub \
                     App decides access here, so its membership rows are kept."
                );
            } else {
                println!(
                    "Domain '{domain}' is shared: every account sees it again, and \
                     who was invited into it is forgotten."
                );
            }
        }
    }
    readers_changed().await;
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
    refuse_governed(&domain, config.as_deref()).await?;
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
    readers_changed().await;
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
    let rows: Vec<[String; 5]> = members
        .iter()
        .map(|m| {
            [
                m.principal.clone(),
                m.level.to_string(),
                m.reflection.to_string(),
                m.added_by.clone(),
                m.added_at.clone(),
            ]
        })
        .collect();
    let header = ["NAME", "LEVEL", "REFLECTION", "ADDED BY", "ADDED AT"];
    // The last column is never padded, so no line carries trailing whitespace.
    let widths: Vec<usize> = (0..4)
        .map(|c| {
            rows.iter()
                .map(|r| r[c].chars().count())
                .chain(std::iter::once(header[c].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: [&str; 5]| {
        let mut out = String::new();
        for (c, cell) in cells.iter().enumerate().take(4) {
            out.push_str(&format!("{cell:<width$}  ", width = widths[c]));
        }
        out.push_str(cells[4]);
        println!("{}", out.trim_end());
    };
    line(header);
    for row in &rows {
        line([&row[0], &row[1], &row[2], &row[3], &row[4]]);
    }
    println!();
    println!(
        "{} member{} of '{domain}'. Remove one with: \
         crystalline domain members {domain} remove <account>",
        members.len(),
        if members.len() == 1 { "" } else { "s" }
    );
}
