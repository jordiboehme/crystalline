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
        UsersCommand::Remove { name, force } => {
            if force {
                store.remove_user_force(&name).await?;
                println!(
                    "Removed '{}' and every session it held (forced). The installation \
                     may now have no admin; add one with `crystalline users add <name> --role admin`.",
                    stored_name(&name)
                );
            } else {
                store.remove_user(&name).await?;
                println!(
                    "Removed '{}' and every session it held.",
                    stored_name(&name)
                );
            }
        }
    }
    Ok(())
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
