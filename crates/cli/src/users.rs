//! `crystalline users`: the accounts that may sign in to the web API and the
//! web UI the daemon serves at 127.0.0.1:7411 by default.
//!
//! The accounts live in their own small database in the state directory
//! (`web-auth.db`), never in the index: credentials are not knowledge and must
//! survive a `reindex --wipe`. Every command here opens that file directly
//! rather than going through the daemon, which is safe by construction - the
//! store serializes its writers across processes - so account management works
//! whether or not a daemon is running, and a running daemon picks the change
//! up on its next lookup without a restart.
//!
//! `users writes` keeps an account's write list in the same database; the
//! daemon reads it on the next request.
//!
//! Except on Windows, where the database engine has no cross-process
//! coordination on its default IO backend yet, so a running daemon holds the
//! file exclusively: a command here then fails at open time with the message
//! `legacy_open_error` in `crystalline-service`'s `rest::auth_store` writes,
//! which points at the web UI or at stopping the daemon.

use std::io::{IsTerminal, Read, Write};

use anyhow::{Context, Result, anyhow, bail};

use crystalline_core::config::registration::{
    DOMAIN_NAME_RULE, MAX_DOMAIN_NAME_CHARS, validate_domain_name,
};
use crystalline_service::rest::{AuthStore, LINKED_BY_CLI, ReflectionLevel, Role, User};
use crystalline_service::write_list_matches;

use crate::UsersCommand;

/// Run one `users` subcommand against the auth database in the state
/// directory. `json` switches `list`, `add` and `writes` to machine-readable
/// output; the other editing commands confirm in one human line.
pub async fn run(command: UsersCommand, json: bool) -> Result<()> {
    let path = crystalline_core::config::web_auth_db_path()?;
    let store = AuthStore::open(&path).await?;

    match command {
        UsersCommand::Add {
            name,
            display,
            email,
            role,
            password_stdin,
            password_file,
            mcp_token,
            writes,
            reflection,
        } => {
            // The login name as typed makes the better default display name:
            // the store folds the login name but keeps this one as given.
            let display = display.unwrap_or_else(|| name.trim().to_string());
            let role: Role = role.into();
            let reflection: ReflectionLevel = reflection.map(Into::into).unwrap_or_default();
            // Checked before anything is created, so a bad entry leaves no
            // account behind.
            let writes = writes.as_deref().map(parse_write_list).transpose()?;
            clear_name_for_new_account(&store, &name).await?;
            let added = match &mcp_token {
                // An account for an agent: no password to ask for, and the
                // token is issued in the same transaction as the account.
                Some(label) => store
                    .add_token_only_user_with_reflection(
                        &name,
                        &display,
                        email.as_deref(),
                        role,
                        reflection,
                        label,
                    )
                    .await
                    .map(Some),
                None => {
                    let password = read_password(password_stdin, password_file.as_deref())?;
                    store
                        .add_user_with_reflection(
                            &name,
                            &display,
                            email.as_deref(),
                            role,
                            reflection,
                            &password,
                        )
                        .await
                        .map(|()| None)
                }
            };
            let issued = match added {
                Ok(issued) => issued,
                Err(e) => {
                    // The store's primary key is what actually refuses a duplicate
                    // (and it is the only thing that can, without racing a second
                    // invocation); this only turns its constraint violation into a
                    // sentence, and re-raises anything else untouched.
                    if format!("{e:#}").contains("UNIQUE constraint") {
                        bail!(
                            "a user named '{}' already exists; \
                         change it with `crystalline users passwd` or `crystalline users role`",
                            stored_name(&name)
                        );
                    }
                    return Err(e);
                }
            };
            // An agent account must never stand without the list it was
            // created for: a failed list write takes the account back out.
            if let Some(entries) = &writes {
                keep_write_list_or_remove(&store, &name, entries).await?;
                warn_unmatched(entries);
            }
            match issued {
                Some(issued) => {
                    if !json {
                        println!(
                            "Added user '{}' with role {role} and no password.",
                            stored_name(&name)
                        );
                        if reflection != ReflectionLevel::Default {
                            println!("Reflection: {reflection}.");
                        }
                        if let Some(entries) = &writes {
                            println!("Write list: {}.", entries.join(", "));
                        }
                    }
                    print_issued_token(
                        &issued,
                        &stored_name(&name),
                        json,
                        None,
                        Some((role, reflection)),
                        writes.as_deref(),
                    );
                }
                None => {
                    if json {
                        crate::print_value(
                            &serde_json::json!({
                                "name": stored_name(&name),
                                "role": role.to_string(),
                                "reflection": reflection.to_string(),
                                "writes": writes,
                            }),
                            true,
                        );
                    } else {
                        if reflection == ReflectionLevel::Default {
                            println!("Added user '{}' with role {role}.", stored_name(&name));
                        } else {
                            println!(
                                "Added user '{}' with role {role} and Reflection {reflection}.",
                                stored_name(&name)
                            );
                        }
                        if let Some(entries) = &writes {
                            println!("Write list: {}.", entries.join(", "));
                        }
                    }
                }
            }
        }
        UsersCommand::List => {
            let users = store.list_users().await?;
            let passwordless = store.passwordless_users().await?;
            let mut writes = std::collections::HashMap::new();
            for user in &users {
                if let Some(list) = store.write_list(&user.name).await? {
                    writes.insert(user.name.clone(), list);
                }
            }
            if json {
                let rows: Vec<serde_json::Value> = users
                    .iter()
                    .map(|u| {
                        let mut row = serde_json::to_value(u).unwrap_or_default();
                        row["has_password"] = (!passwordless.contains(&u.name)).into();
                        // Only when there is a list, as `users list` shows
                        // nothing for an account without one. Never on
                        // `User` itself, which crosses the REST API.
                        if let Some(list) = writes.get(&u.name) {
                            row["writes"] = serde_json::json!(list);
                        }
                        row
                    })
                    .collect();
                crate::print_value(&serde_json::json!({ "users": rows }), true);
            } else {
                print_users(&users, &passwordless, &writes);
            }
        }
        UsersCommand::Passwd {
            name,
            password_stdin,
        } => {
            let password = read_password(password_stdin, None)?;
            store.set_password(&name, &password).await?;
            println!(
                "Changed the password for '{}'. Its sessions, MCP tokens and OAuth connections are revoked.",
                stored_name(&name)
            );
        }
        UsersCommand::Role { name, role } => {
            let role: Role = role.into();
            store.set_role(&name, role).await?;
            println!("'{}' is now {role}.", stored_name(&name));
        }
        UsersCommand::Reflection { name, level } => {
            let level: ReflectionLevel = level.into();
            store.set_reflection_level(&name, level).await?;
            println!("'{}' now has Reflection {level}.", stored_name(&name));
        }
        UsersCommand::Disable { name } => {
            store.set_disabled(&name, true).await?;
            sweep_personal_credential(&name).await;
            println!(
                "Disabled '{}' and revoked its sessions. Enabling it again does not bring them back.",
                stored_name(&name)
            );
        }
        UsersCommand::Enable { name } => {
            store.set_disabled(&name, false).await?;
            println!("Enabled '{}'.", stored_name(&name));
        }
        UsersCommand::Demote { name, role, force } => {
            let role: Role = role.into();
            if force {
                store.set_role_force(&name, role).await?;
                println!(
                    "'{}' is now {role} (forced). The installation may now have no \
                     admin; add one with `crystalline users role <name> admin`.",
                    stored_name(&name)
                );
            } else {
                store.set_role(&name, role).await?;
                println!("'{}' is now {role}.", stored_name(&name));
            }
        }
        UsersCommand::McpToken {
            name,
            label,
            list,
            revoke,
            rotate,
        } => {
            // Issuing checks the account inside its own transaction; the
            // other three branches would otherwise answer about an account
            // that does not exist - an empty listing, or "no such token" - and
            // a typo would read as a fact.
            if (list || revoke.is_some() || rotate.is_some()) && store.user(&name).await?.is_none()
            {
                bail!("no such user: '{}'", stored_name(&name));
            }
            if list {
                let tokens = store.list_mcp_tokens(&name).await?;
                if json {
                    crate::print_value(&serde_json::json!({ "tokens": tokens }), true);
                } else {
                    print_mcp_tokens(&tokens, &stored_name(&name));
                }
            } else if let Some(id) = revoke {
                if !store.revoke_mcp_token(&name, id).await? {
                    bail!(
                        "'{}' holds no MCP token {id}; list them with \
                         `crystalline users mcp-token {} --list`",
                        stored_name(&name),
                        stored_name(&name)
                    );
                }
                println!(
                    "Revoked MCP token {id} of '{}'. An agent still sending it is refused.",
                    stored_name(&name)
                );
            } else if let Some(id) = rotate {
                let issued = store.rotate_mcp_token(&name, id).await?;
                print_issued_token(&issued, &stored_name(&name), json, Some(id), None, None);
            } else {
                // The account must exist before a token is minted for it; the
                // store says so itself, so a mistyped name is reported rather
                // than silently issuing against nothing.
                let label = label.unwrap_or_else(|| "cli".to_string());
                let issued = store.issue_mcp_token(&name, &label).await?;
                print_issued_token(&issued, &stored_name(&name), json, None, None, None);
            }
        }
        UsersCommand::Writes {
            name,
            domains,
            clear,
        } => {
            let shown = stored_name(&name);
            if clear {
                store.clear_write_list(&name).await?;
                if json {
                    crate::print_value(&serde_json::json!({ "name": shown, "writes": null }), true);
                } else {
                    println!(
                        "Removed the write list of '{shown}'. It writes wherever its role and \
                         memberships allow."
                    );
                }
            } else if let Some(raw) = domains {
                let entries = parse_write_list(&raw)?;
                store.set_write_list(&name, &entries).await?;
                warn_unmatched(&entries);
                if json {
                    crate::print_value(
                        &serde_json::json!({ "name": shown, "writes": entries }),
                        true,
                    );
                } else {
                    println!("Set the write list of '{shown}': {}.", entries.join(", "));
                }
            } else {
                if store.user(&name).await?.is_none() {
                    bail!("no such user: '{shown}'");
                }
                let list = store.write_list(&name).await?;
                if json {
                    crate::print_value(&serde_json::json!({ "name": shown, "writes": list }), true);
                } else {
                    match list {
                        Some(list) => {
                            println!("Write list of '{shown}': {}.", list.join(", "))
                        }
                        None => println!(
                            "'{shown}' has no write list. It writes wherever its role and \
                             memberships allow."
                        ),
                    }
                }
            }
        }
        UsersCommand::Link {
            name,
            issuer,
            subject,
        } => {
            store
                .link_identity(&issuer, &subject, &name, LINKED_BY_CLI)
                .await?;
            println!(
                "Linked subject '{}' at {} to '{}'. A sign-in with that identity now lands \
                 in this account.",
                subject.trim(),
                issuer.trim(),
                stored_name(&name)
            );
        }
        UsersCommand::Unlink {
            name,
            issuer,
            force,
        } => {
            // The store owns the last-way-in rule, so `--force` is the only
            // thing decided here: which of its two entry points to call. The
            // refusal, when it comes, is the store's own sentence, which names
            // `crystalline users passwd`.
            let removed = if force {
                store.unlink_identity_force(&issuer, &name).await?
            } else {
                store.unlink_identity(&issuer, &name).await?
            };
            if !removed {
                bail!(
                    "'{}' holds no identity at {}",
                    stored_name(&name),
                    issuer.trim()
                );
            }
            println!(
                "Unlinked '{}' from {}. A sign-in from it now provisions a new account.",
                stored_name(&name),
                issuer.trim()
            );
            // Only reachable with --force, since that is the only way past the
            // guard, and worth saying out loud: the operator has just made an
            // account nobody can sign in to, on purpose, and either link step
            // or a password ends that.
            if store.identity_links(&name).await?.is_empty() && !store.has_password(&name).await? {
                println!(
                    "'{}' now has no way in at all: link an identity again, or give it a \
                     password with `crystalline users passwd {}`.",
                    stored_name(&name),
                    stored_name(&name)
                );
            }
        }
        UsersCommand::DiscardDrafts { name } => {
            let account = stored_name(&name);
            if store.user(&name).await?.is_some() {
                bail!(
                    "'{account}' is an account, and its drafts are its own; this is for a name whose account was removed"
                );
            }
            let report = crystalline_service::client::discard_account_drafts(&account).await?;
            if json {
                println!("{report}");
            } else {
                if let Some(error) = report["error"].as_str() {
                    println!("{error}");
                }
                let domains = report["domains"].as_array().cloned().unwrap_or_default();
                if domains.is_empty() && report["error"].is_null() {
                    println!("'{account}' holds no drafts.");
                }
                for row in domains {
                    let domain = row["domain"].as_str().unwrap_or("?");
                    println!(
                        "{domain}: {} discarded, {} left because they are open in an editor",
                        row["discarded"],
                        row["left"].as_array().map_or(0, Vec::len),
                    );
                    if let Some(place) = row["where"].as_str() {
                        println!("{domain}: {place}");
                    }
                    if let Some(error) = row["error"].as_str() {
                        println!("{domain}: this stopped here: {error}");
                    }
                }
            }
        }
        UsersCommand::Remove { name, force } => {
            if force {
                store.remove_user_force(&name).await?;
                sweep_personal_credential(&name).await;
                println!(
                    "Removed '{}' and every session it held (forced). The installation \
                     may now have no admin; add one with `crystalline users add <name> --role admin`.",
                    stored_name(&name)
                );
            } else {
                store.remove_user(&name).await?;
                sweep_personal_credential(&name).await;
                println!(
                    "Removed '{}' and every session it held.",
                    stored_name(&name)
                );
            }
            // The drafts stay: nothing deletes one without an admin asking.
            if let Ok(drafts) =
                crystalline_service::client::account_drafts(&stored_name(&name)).await
                && !drafts.is_empty()
            {
                let count: u64 = drafts.iter().map(|(_, n)| n).sum();
                println!(
                    "It still holds {count} drafts, which stay. A new account under this name is refused until they are ended with: crystalline users discard-drafts {}",
                    stored_name(&name)
                );
            }
        }
    }
    Ok(())
}

/// Forgets the GitHub identity an account connected for sharing, after that
/// account has been disabled or removed. Best effort in every direction: the
/// account is already gone from the auth store, and a credential this cannot
/// resolve or delete is an orphan rather than a failure - it addresses nothing
/// once no account carries the name, and reconnecting overwrites it.
///
/// Quiet about the delete (a debug log, findable with `RUST_LOG=debug`): the
/// command's own confirmation line is about the account, and a person who never
/// connected an identity - the common case - must not be told about a credential
/// that was never there. The name is folded exactly as the auth store folds it, so
/// the credential swept is the one that account's own connect would have written;
/// a name the credential allowlist cannot address never had one.
///
/// A running daemon is told, exactly as `connect github --disconnect` tells it:
/// this is the second writer of the credential store, and it is the "person
/// leaving" case - the daemon's process cache would otherwise go on holding a
/// token for an account that no longer exists until it restarted. A daemon that
/// refuses the message is a warning, because it may go on writing on that token.
async fn sweep_personal_credential(name: &str) {
    let account = stored_name(name);
    if !crystalline_remote::valid_identity_name(&account) {
        tracing::debug!("'{account}' cannot address a credential; nothing to forget");
        return;
    }
    let identity = crystalline_remote::TokenIdentity::Personal(account);
    let host = credential_host();
    match crystalline_core::config::origins_state_dir() {
        Ok(dir) => crate::cmd::forget_credential(&identity, host.as_deref(), &dir),
        Err(e) => tracing::debug!("could not resolve the state directory to sweep: {e}"),
    }
    if let crate::cmd::DaemonNotice::Refused(e) = crate::cmd::notify_daemon_forgot(&identity).await
    {
        eprintln!(
            "warning: a running daemon could not be told ({e}). Restart it, because it may still hold the GitHub token of '{}'.",
            stored_name(name)
        );
    }
}

/// The host a personal credential is filed under, from the same config the
/// connect path reads. A config that will not load is not worth failing an
/// account change over: github.com is where all but the Enterprise Server
/// installs keep it anyway.
fn credential_host() -> Option<String> {
    crate::cmd::load(None)
        .ok()
        .and_then(|loaded| {
            loaded
                .effective
                .github
                .as_ref()
                .and_then(|g| g.api_url.clone())
        })
        .and_then(|api_url| {
            crate::cmd::bare_host(&crystalline_remote::github::auth::auth_base(Some(&api_url)))
        })
}

/// The step before an account is made, while the name holds none, as REST takes
/// it (`Engine::clear_name_for_new_account`): a name with drafts is refused, links
/// the old person gave end, and a GitHub token left under the name goes, with a
/// running daemon told. Anything that cannot be read or deleted refuses: nothing
/// may stand on a token the store could not answer about. A name that holds an
/// account, or that no account can have, is left alone, so the insert answers it.
async fn clear_name_for_new_account(store: &AuthStore, name: &str) -> Result<()> {
    if crystalline_service::rest::normalize_new_account_name(name).is_err()
        || store.user(name).await?.is_some()
    {
        return Ok(());
    }
    let account = stored_name(name);
    let drafts = crystalline_service::client::account_drafts(&account)
        .await
        .map_err(|e| {
            anyhow!(
                "the drafts under '{account}' could not be read, so the account was not made: {e:#}"
            )
        })?;
    if !drafts.is_empty() {
        let held = drafts
            .iter()
            .map(|(domain, entries)| format!("{entries} in {domain}"))
            .collect::<Vec<_>>()
            .join(", ");
        bail!(
            "the name '{account}' still holds drafts of a removed account: {held}. No account was made, because the new person would inherit them. Use another name, or, once nobody needs those drafts, end them with: crystalline users discard-drafts {account}, and add the account again"
        );
    }
    store.end_overlay_grants_of_owner(&account).await?;
    sweep_personal_credential(name).await;
    if !crystalline_remote::valid_identity_name(&account) {
        return Ok(());
    }
    let identity = crystalline_remote::TokenIdentity::Personal(account.clone());
    let dir = crystalline_core::config::origins_state_dir().map_err(|e| {
        anyhow!("the state directory could not be found, so the account was not made: {e}")
    })?;
    let (_store, token) = crystalline_remote::TokenStore::resolve_and_load_for(
        &identity,
        credential_host().as_deref(),
        &dir,
    )
    .map_err(|e| {
        anyhow!(
            "the GitHub token store could not be read, so the account was not made: {e}. Unlock the keychain or fix the state directory, and add it again"
        )
    })?;
    if token.is_some() {
        bail!(
            "a GitHub token is still saved under '{account}' and could not be deleted, so the account was not made; fix the permissions of the state directory, or unlock the keychain, and add it again"
        );
    }
    Ok(())
}

/// Store the write list of an account created a moment ago, and take the
/// account back out when that fails: an agent account must never stand
/// without the list it was created for.
async fn keep_write_list_or_remove(
    store: &AuthStore,
    name: &str,
    entries: &[String],
) -> Result<()> {
    if let Err(e) = store.set_write_list(name, entries).await {
        let _ = store.remove_user_force(name).await;
        return Err(e.context("setting the write list; the account was not kept"));
    }
    Ok(())
}

/// Print a freshly issued (or rotated) token, which is the one and only time
/// anybody sees it, and say what to do with it.
///
/// The teaching line is the point of the command: a token nobody knows where
/// to paste is a token that gets pasted somewhere worse. `replaced` names the
/// id a rotation retired, so the operator can tell the two ids apart.
///
/// `--json` prints the id, the token and the label as an object (plus the
/// name, role and writes of an account created with it), for provisioning
/// scripts; the teaching goes to stderr there, so a piped stdout stays valid
/// JSON.
fn print_issued_token(
    issued: &crystalline_service::rest::IssuedMcpToken,
    account: &str,
    json: bool,
    replaced: Option<i64>,
    created_with_role: Option<(Role, ReflectionLevel)>,
    writes: Option<&[String]>,
) {
    if json {
        let mut object = serde_json::json!({
            "id": issued.id,
            "token": issued.token,
            "label": issued.label,
        });
        // A token issued together with its account also names the account.
        if let Some((role, reflection)) = created_with_role {
            object["name"] = account.into();
            object["role"] = role.to_string().into();
            object["reflection"] = reflection.to_string().into();
            object["writes"] = serde_json::json!(writes);
        }
        crate::print_value(&object, true);
        eprintln!("{TOKEN_TEACHING}");
        return;
    }
    match replaced {
        Some(old) => println!(
            "Rotated MCP token {old} of '{account}': it stops working now. \
             The replacement is id {} (label '{}'), shown once:",
            issued.id, issued.label
        ),
        None => println!(
            "Issued MCP token {} for '{account}' (label '{}'), shown once:",
            issued.id, issued.label
        ),
    }
    println!();
    println!("  {}", issued.token);
    println!();
    println!("{TOKEN_TEACHING}");
}

/// What to do with a token that was just printed. One sentence, worded the
/// same as the refusal an unauthenticated agent gets from the MCP gate, so the
/// person reading either one recognizes the other.
const TOKEN_TEACHING: &str =
    "Add it to the agent's MCP registration as header 'Authorization: Bearer <token>'.";

/// One line per token: id, label, when it was issued and when it was last
/// presented. Never a token: only its hash is stored, so there is nothing to
/// print.
fn print_mcp_tokens(tokens: &[crystalline_service::rest::McpTokenInfo], account: &str) {
    if tokens.is_empty() {
        println!(
            "'{account}' holds no MCP tokens. Issue one with: \
             crystalline users mcp-token {account}"
        );
        return;
    }
    let rows: Vec<[String; 4]> = tokens
        .iter()
        .map(|t| {
            [
                t.id.to_string(),
                t.label.clone(),
                t.created_at.clone(),
                t.last_used.clone().unwrap_or_else(|| "never".to_string()),
            ]
        })
        .collect();
    let header = ["ID", "LABEL", "CREATED", "LAST USED"];
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
        "{} token{} for '{account}'. Revoke one with: \
         crystalline users mcp-token {account} --revoke <id>",
        tokens.len(),
        if tokens.len() == 1 { "" } else { "s" }
    );
}

/// The form the store keys on, for confirmation messages only: it is what the
/// operator has to type in the next command. The store does this folding
/// itself and is the authority; this mirrors it rather than pre-normalizing
/// anything, so what is passed in stays exactly what the operator typed.
fn stored_name(name: &str) -> String {
    name.trim().to_lowercase()
}

/// One line per account: name, role, status, write list, display name, email
/// and when it was last seen, columns aligned to the widest entry.
fn print_users(
    users: &[User],
    passwordless: &std::collections::HashSet<String>,
    writes: &std::collections::HashMap<String, Vec<String>>,
) {
    if users.is_empty() {
        println!("No users yet. Add one with: crystalline users add <name> --role admin");
        return;
    }
    let rows: Vec<[String; 8]> = users
        .iter()
        .map(|u| {
            [
                u.name.clone(),
                u.role.to_string(),
                u.reflection.to_string(),
                match (u.disabled, passwordless.contains(&u.name)) {
                    (true, _) => "disabled",
                    (false, true) => "active, no password",
                    (false, false) => "active",
                }
                .to_string(),
                writes
                    .get(&u.name)
                    .map(|list| list.join(","))
                    .unwrap_or_default(),
                u.display.clone(),
                u.email.clone().unwrap_or_default(),
                u.last_seen.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let header = [
        "NAME",
        "ROLE",
        "REFLECTION",
        "STATUS",
        "WRITES",
        "DISPLAY",
        "EMAIL",
        "LAST SEEN",
    ];
    // The last column is never padded, so a trailing empty last-seen leaves no
    // trailing whitespace behind.
    let widths: Vec<usize> = (0..7)
        .map(|c| {
            rows.iter()
                .map(|r| r[c].chars().count())
                .chain(std::iter::once(header[c].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: [&str; 8]| {
        let mut out = String::new();
        for (c, cell) in cells.iter().enumerate().take(7) {
            out.push_str(&format!("{cell:<width$}  ", width = widths[c]));
        }
        out.push_str(cells[7]);
        println!("{}", out.trim_end());
    };
    line(header);
    for row in &rows {
        line([
            &row[0], &row[1], &row[2], &row[3], &row[4], &row[5], &row[6], &row[7],
        ]);
    }
    println!();
    println!(
        "{} account{}.",
        users.len(),
        if users.len() == 1 { "" } else { "s" }
    );
}

/// Split `--writes` / `users writes` input into checked entries: trimmed,
/// empties dropped, duplicates kept once, each one a domain name or a name
/// prefix with one `*` at the end. A list with no entry is refused: no list
/// at all is `--clear`.
fn parse_write_list(raw: &str) -> Result<Vec<String>> {
    let mut entries: Vec<String> = Vec::new();
    for entry in raw.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        check_write_entry(entry)?;
        if !entries.iter().any(|e| e == entry) {
            entries.push(entry.to_string());
        }
    }
    if entries.is_empty() {
        bail!(
            "the write list is empty: give at least one entry, or use --clear to remove the list"
        );
    }
    Ok(entries)
}

/// One entry: a domain name, or a name prefix with one `*` at the end. The
/// stem is checked with the domain-name rule; a prefix is checked with one
/// letter after it (unless the stem is already at the length limit), so
/// `agents-*` and `team.*` pass while `agents/*` does not. A bare `*` names
/// every domain, which is the same as no list, and is refused.
fn check_write_entry(entry: &str) -> Result<()> {
    let (stem, prefix) = match entry.strip_suffix('*') {
        Some(stem) => (stem, true),
        None => (entry, false),
    };
    // A prefix is checked as the shortest name it can match, the stem plus
    // one character. A stem already at the length limit can only match
    // itself, so it is checked as it is.
    let probe = if prefix && stem.chars().count() < MAX_DOMAIN_NAME_CHARS {
        format!("{stem}x")
    } else {
        stem.to_string()
    };
    if stem.is_empty() || stem.contains('*') || validate_domain_name(&probe).is_err() {
        bail!(
            "'{entry}' is not a domain name or a name prefix with one * at the end: \
             {DOMAIN_NAME_RULE}"
        );
    }
    Ok(())
}

/// Warn, on stderr, about every entry no registered domain matches. The entry
/// is kept: the domain may be created later. A configuration that cannot be
/// read is said once and the check is skipped, never an error, because the
/// accounts database does not depend on it.
fn warn_unmatched(entries: &[String]) {
    let registered: Vec<String> = match crystalline_service::overlay::load(None) {
        Ok(loaded) => loaded.effective.domains.keys().cloned().collect(),
        Err(e) => {
            eprintln!(
                "warning: the registered domains could not be read ({e:#}), so the write list \
                 was not checked against them"
            );
            return;
        }
    };
    for entry in entries {
        let entry_only = std::slice::from_ref(entry);
        if !registered.iter().any(|d| write_list_matches(entry_only, d)) {
            eprintln!(
                "warning: no registered domain matches '{entry}'; the entry is kept and counts \
                 once such a domain exists"
            );
        }
    }
}

/// Collect the password: from a file under `--password-file`, from stdin
/// under `--password-stdin`, otherwise by asking at the terminal.
///
/// There is no hidden-input dependency in this workspace and the brief did not
/// want one added, so the typed password is echoed - the prompt says so rather
/// than letting anyone assume otherwise. A non-terminal run without
/// `--password-stdin` or `--password-file` refuses instead of hanging on a
/// pipe that will never carry an answer.
fn read_password(from_stdin: bool, from_file: Option<&std::path::Path>) -> Result<String> {
    let password = if let Some(path) = from_file {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the password from {}", path.display()))?;
        crystalline_core::secret_env::trim_one_line_break(&text).to_string()
    } else if from_stdin {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .context("reading the password from stdin")?;
        // Exactly one trailing line ending goes: `echo secret | crystalline
        // ...` must not set the password to "secret\n", while a password that
        // ends in a space survives untouched.
        let mut password = buf.as_str();
        if let Some(stripped) = password.strip_suffix('\n') {
            password = stripped;
        }
        if let Some(stripped) = password.strip_suffix('\r') {
            password = stripped;
        }
        password.to_string()
    } else {
        if !std::io::stdin().is_terminal() {
            bail!("not a terminal; pass --password-stdin or --password-file to give the password");
        }
        print!("Password (visible while typing): ");
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin()
            .read_line(&mut answer)
            .context("reading the password")?;
        answer.trim_end_matches(['\r', '\n']).to_string()
    };
    if password.is_empty() {
        bail!("the password is empty; pick one with at least one character");
    }
    Ok(password)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// B-2: when storing the list fails, the new account is removed again.
    #[tokio::test]
    async fn a_failed_write_list_takes_the_new_account_back_out() {
        let dir = tempfile::tempdir().unwrap();
        let store = AuthStore::open(&dir.path().join("web-auth.db"))
            .await
            .unwrap();
        store
            .add_token_only_user("bot", "bot", None, Role::Editor, "bot")
            .await
            .unwrap();
        // The store refuses an empty list ("a write list needs at least one
        // entry"), the one failure a test can cause without a broken file.
        let err = keep_write_list_or_remove(&store, "bot", &[])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("the account was not kept"),
            "{err:#}"
        );
        assert!(
            store.user("bot").await.unwrap().is_none(),
            "the account is gone"
        );
    }
}
