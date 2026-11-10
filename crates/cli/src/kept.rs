//! `crystalline kept`: the texts this machine's server kept when a
//! co-editing room could not save.
//!
//! When the last person leaves a room and its final save is refused or ends
//! in a conflict, the server keeps the room's text in the state folder
//! (`<state>/kept/`, layout in [`crystalline_core::kept`]) rather than drop
//! it. These commands read that folder directly, so they work whether or not
//! a daemon runs. Nothing deletes a kept text on its own: `drop` is the
//! person's decision, and it asks first.

use std::io::{IsTerminal, Write as _};
use std::path::PathBuf;

use anyhow::{Result, bail};
use crystalline_core::kept::{self, KeptText};
use serde_json::{Value, json};

use crate::KeptCommand;

/// How old a kept text gets before `doctor` warns about it.
pub const STALE_DAYS: i64 = 30;

/// This machine's kept folder.
pub fn root() -> Result<PathBuf> {
    Ok(crystalline_core::config::state_dir()?.join(kept::KEPT_DIR))
}

/// Every kept text on this machine, oldest first. None when the state folder
/// cannot be named.
pub fn scan_here() -> Vec<KeptText> {
    root().map(|root| kept::scan(&root)).unwrap_or_default()
}

/// The `crystalline status` line, only when something is kept.
pub fn status_line(found: &[KeptText]) -> Option<String> {
    (!found.is_empty()).then(|| format!("Kept texts: {} (crystalline kept list)", found.len()))
}

/// One kept text as JSON, with the file it is in.
pub fn to_json(k: &KeptText) -> Value {
    json!({
        "id": k.id,
        "domain": k.domain,
        "owner": k.owner,
        "removed": k.removed,
        "unfinished": k.unfinished,
        "kind": k.kind,
        "path": k.path,
        "kept_at": k.kept_at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "bytes": k.bytes,
        "sender": k.sender,
        "file": k.file.display().to_string(),
    })
}

/// "eng reflection guides/a.md", plus "(ada's draft)" for a text kept from
/// a review draft and "(removed domain)" for one whose domain was removed
/// since: what a kept text was.
pub fn what(k: &KeptText) -> String {
    let mut out = format!("{} {} {}", k.domain, k.kind.as_str(), k.path);
    if let Some(owner) = &k.owner {
        out.push_str(&format!(" ({owner}'s draft)"));
    }
    if let Some(sender) = &k.sender {
        out.push_str(&format!(" (sent by {})", sender.name));
    }
    if k.removed {
        out.push_str(" (removed domain)");
    }
    if k.unfinished {
        out.push_str(" (unfinished keep)");
    }
    out
}

/// "2026-10-15 08:00 UTC".
pub fn when(k: &KeptText) -> String {
    k.kept_at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// Run one `kept` subcommand.
pub fn run(command: KeptCommand, json: bool) -> Result<()> {
    let root = root()?;
    match command {
        KeptCommand::List { domain } => {
            let found: Vec<KeptText> = kept::scan(&root)
                .into_iter()
                .filter(|k| domain.as_deref().is_none_or(|d| k.domain == d))
                .collect();
            if json {
                let rows: Vec<Value> = found.iter().map(to_json).collect();
                println!("{}", json!({ "kept": rows }));
                return Ok(());
            }
            if found.is_empty() {
                match &domain {
                    Some(d) => println!("No kept texts for '{d}'."),
                    None => println!("No kept texts."),
                }
                return Ok(());
            }
            for k in &found {
                println!(
                    "{}  {}  kept {}  {}",
                    k.id,
                    what(k),
                    when(k),
                    crate::cmd::human_size(k.bytes)
                );
                println!("    {}", k.file.display());
                if kept::is_file_copy(k) {
                    println!("    A file, not text. Upload it again where it belongs.");
                }
            }
            println!();
            if found.iter().any(|k| !kept::is_file_copy(k)) {
                println!(
                    "Show one with: crystalline kept show <id>. Copy the text back into its page or engram, then: crystalline kept drop <id>"
                );
            } else {
                println!("Once a file is back where it belongs: crystalline kept drop <id>");
            }
        }
        KeptCommand::Show { id } => {
            let k = find(&root, &id)?;
            let text = match kept::read(&k) {
                Ok(text) => text,
                Err(e) if e.kind() == std::io::ErrorKind::InvalidData => {
                    bail!("{}", kept::not_text(&k))
                }
                Err(e) => return Err(e.into()),
            };
            if json {
                let mut row = to_json(&k);
                row["text"] = json!(text);
                println!("{row}");
            } else {
                // The text as it was kept, nothing around it, so it can be
                // redirected into a file.
                print!("{text}");
                std::io::stdout().flush()?;
            }
        }
        KeptCommand::Drop { id, yes } => {
            if json && !yes {
                bail!("--json requires --yes");
            }
            let k = find(&root, &id)?;
            if !yes {
                if !std::io::stdin().is_terminal() {
                    bail!("not a terminal; pass --yes");
                }
                print!(
                    "Drop the text kept for {} on {}? It may be the last copy. [y/N] ",
                    what(&k),
                    when(&k)
                );
                std::io::stdout().flush()?;
                let mut answer = String::new();
                std::io::stdin().read_line(&mut answer)?;
                if !matches!(answer.trim(), "y" | "Y") {
                    println!("aborted");
                    return Ok(());
                }
            }
            kept::remove(&root, &k)?;
            if json {
                println!("{}", json!({ "dropped": to_json(&k) }));
            } else {
                println!("Dropped the text kept for {} on {}.", what(&k), when(&k));
            }
        }
    }
    Ok(())
}

fn find(root: &std::path::Path, id: &str) -> Result<KeptText> {
    match kept::find(root, id) {
        Some(k) => Ok(k),
        None => bail!("no kept text '{id}'; crystalline kept list shows them"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_shows_only_when_something_is_kept() {
        assert_eq!(status_line(&[]), None);
        let dir = tempfile::tempdir().unwrap();
        let at = chrono::Utc::now();
        kept::write(
            dir.path(),
            "eng",
            None,
            kept::KeptKind::Engram,
            "a.md",
            at,
            "x",
        )
        .unwrap();
        let found = kept::scan(dir.path());
        assert_eq!(
            status_line(&found).as_deref(),
            Some("Kept texts: 1 (crystalline kept list)")
        );
    }
}
