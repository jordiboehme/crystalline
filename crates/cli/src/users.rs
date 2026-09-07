//! `crystalline users`: the accounts that may sign in to the web API and the
//! web UI the daemon serves at 127.0.0.1:7411 by default.
//!
//! The accounts live in their own small database in the state directory
//! (`web-auth.db`), never in the index: credentials are not knowledge and must
//! survive a `reindex --full`. Every command here opens that file directly
//! rather than going through the daemon, which is safe by construction - the
//! store serializes its writers across processes - so account management works
//! whether or not a daemon is running, and a running daemon picks the change
//! up on its next lookup without a restart.
//!
//! Except on Windows, where the database engine has no cross-process
//! coordination on its default IO backend yet, so a running daemon holds the
//! file exclusively: a command here then fails at open time with the message
//! `legacy_open_error` in `crystalline-service`'s `rest::auth_store` writes,
//! which points at the web UI or at stopping the daemon.

use std::io::{IsTerminal, Read, Write};

use anyhow::{Context, Result, bail};

use crystalline_service::rest::{AuthStore, Role, User};

use crate::UsersCommand;

/// Run one `users` subcommand against the auth database in the state
/// directory. `json` switches `list` to machine-readable output; the editing
/// commands always confirm in one human line, since there is nothing to
/// script off beyond the exit code.
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
        } => {
            let password = read_password(password_stdin)?;
            // The login name as typed makes the better default display name:
            // the store folds the login name but keeps this one as given.
            let display = display.unwrap_or_else(|| name.trim().to_string());
            let role: Role = role.into();
            if let Err(e) = store
                .add_user(&name, &display, email.as_deref(), role, &password)
                .await
            {
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
            println!("Added user '{}' with role {role}.", stored_name(&name));
        }
        UsersCommand::List => {
            let users = store.list_users().await?;
            if json {
                crate::print_value(&serde_json::json!({ "users": users }), true);
            } else {
                print_users(&users);
            }
        }
        UsersCommand::Passwd {
            name,
            password_stdin,
        } => {
            let password = read_password(password_stdin)?;
            store.set_password(&name, &password).await?;
            println!(
                "Changed the password for '{}' and signed out its sessions.",
                stored_name(&name)
            );
        }
        UsersCommand::Role { name, role } => {
            let role: Role = role.into();
            store.set_role(&name, role).await?;
            println!("'{}' is now {role}.", stored_name(&name));
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
                print_issued_token(&issued, &stored_name(&name), json, Some(id));
            } else {
                // The account must exist before a token is minted for it; the
                // store says so itself, so a mistyped name is reported rather
                // than silently issuing against nothing.
                let label = label.unwrap_or_else(|| "cli".to_string());
                let issued = store.issue_mcp_token(&name, &label).await?;
                print_issued_token(&issued, &stored_name(&name), json, None);
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
/// Silent by design (a debug log, findable with `RUST_LOG`): the command's own
/// confirmation line is about the account, and a person who never connected an
/// identity - the common case - must not be told about a credential that was
/// never there. The name is folded exactly as the auth store folds it, so the
/// credential swept is the one that account's own connect would have written;
/// a name the credential allowlist cannot address never had one.
///
/// A running daemon is told, exactly as `connect github --disconnect` tells
/// it: this is the second writer of the credential store, and it is the
/// "person leaving" case - the daemon's process cache would otherwise go on
/// holding a token for an account that no longer exists until it restarted.
/// The telling stays silent whatever it comes to, unlike the disconnect verb's:
/// there the caller asked to forget a credential and a daemon that refused is
/// news, while here the sweep is a side effect they were never told about, so
/// a line about an unreachable daemon would name a credential the operator
/// does not know was in play.
async fn sweep_personal_credential(name: &str) {
    let account = stored_name(name);
    if !crystalline_remote::valid_identity_name(&account) {
        tracing::debug!("'{account}' cannot address a credential; nothing to forget");
        return;
    }
    let identity = crystalline_remote::TokenIdentity::Personal(account);
    // The host the credential is filed under, from the same config the connect
    // path reads. A config that will not load is not worth failing an account
    // change over: github.com is where all but the Enterprise Server installs
    // keep it anyway.
    let host = crate::cmd::load(None)
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
        });
    match crystalline_core::config::origins_state_dir() {
        Ok(dir) => crate::cmd::forget_credential(&identity, host.as_deref(), &dir),
        Err(e) => tracing::debug!("could not resolve the state directory to sweep: {e}"),
    }
    if let crate::cmd::DaemonNotice::Refused(e) = crate::cmd::notify_daemon_forgot(&identity).await
    {
        tracing::debug!("a running daemon still holds the swept credential: {e}");
    }
}

/// Print a freshly issued (or rotated) token, which is the one and only time
/// anybody sees it, and say what to do with it.
///
/// The teaching line is the point of the command: a token nobody knows where
/// to paste is a token that gets pasted somewhere worse. `replaced` names the
/// id a rotation retired, so the operator can tell the two ids apart.
///
/// `--json` prints the same three fields as an object, for provisioning
/// scripts; the teaching goes to stderr there, so a piped stdout stays valid
/// JSON.
fn print_issued_token(
    issued: &crystalline_service::rest::IssuedMcpToken,
    account: &str,
    json: bool,
    replaced: Option<i64>,
) {
    if json {
        crate::print_value(
            &serde_json::json!({
                "id": issued.id,
                "token": issued.token,
                "label": issued.label,
            }),
            true,
        );
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

/// One line per account: name, role, whether it is disabled, display name,
/// email and when it was last seen, columns aligned to the widest entry.
fn print_users(users: &[User]) {
    if users.is_empty() {
        println!("No users yet. Add one with: crystalline users add <name> --role admin");
        return;
    }
    let rows: Vec<[String; 6]> = users
        .iter()
        .map(|u| {
            [
                u.name.clone(),
                u.role.to_string(),
                if u.disabled { "disabled" } else { "active" }.to_string(),
                u.display.clone(),
                u.email.clone().unwrap_or_default(),
                u.last_seen.clone().unwrap_or_default(),
            ]
        })
        .collect();
    let header = ["NAME", "ROLE", "STATUS", "DISPLAY", "EMAIL", "LAST SEEN"];
    // The last column is never padded, so a trailing empty last-seen leaves no
    // trailing whitespace behind.
    let widths: Vec<usize> = (0..5)
        .map(|c| {
            rows.iter()
                .map(|r| r[c].chars().count())
                .chain(std::iter::once(header[c].len()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let line = |cells: [&str; 6]| {
        let mut out = String::new();
        for (c, cell) in cells.iter().enumerate().take(5) {
            out.push_str(&format!("{cell:<width$}  ", width = widths[c]));
        }
        out.push_str(cells[5]);
        println!("{}", out.trim_end());
    };
    line(header);
    for row in &rows {
        line([&row[0], &row[1], &row[2], &row[3], &row[4], &row[5]]);
    }
    println!();
    println!(
        "{} account{}.",
        users.len(),
        if users.len() == 1 { "" } else { "s" }
    );
}

/// Collect the password: from stdin under `--password-stdin`, otherwise by
/// asking at the terminal.
///
/// There is no hidden-input dependency in this workspace and the brief did not
/// want one added, so the typed password is echoed - the prompt says so rather
/// than letting anyone assume otherwise. A non-terminal run without
/// `--password-stdin` refuses instead of hanging on a pipe that will never
/// carry an answer.
fn read_password(from_stdin: bool) -> Result<String> {
    let password = if from_stdin {
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
            bail!("not a terminal; pass --password-stdin to read the password from stdin");
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
