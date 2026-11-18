//! The steps `crystalline update now` prints on Windows (S22). Nothing is
//! run there: the MSI or the zip is downloaded and verified, and the steps
//! that install it are printed for Windows PowerShell 5.1 and PowerShell 7,
//! with exact paths, and saved as `steps.txt` in the download folder (U3).
//!
//! The printers are pure (paths and facts in, text out) and compiled on
//! every platform, so their tests run everywhere. Paths are composed as
//! text with `\`, never with `Path::join` or `Path::parent`, which do not
//! know `\` off Windows.

use std::path::Path;

use crystalline_core::update::display_path;
use crystalline_core::version::Precedence;
use crystalline_service::daemon_task::MACHINE_TASK_NAME;
use crystalline_service::instance::{LockInfo, StartMode};

/// The name the zip steps give the running exe, and the leftover the next
/// `update now` and `doctor` remove.
pub(crate) const OLD_EXE: &str = "crystalline.old.exe";

/// The file the steps are saved to in the download folder.
pub(crate) const STEPS_FILE: &str = "steps.txt";

/// How the running daemon was started, read from its record, so the MSI
/// steps can say how to start it again after `ctl shutdown` (S22).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DaemonStart {
    /// Started by the Windows task.
    pub by_task: bool,
    /// Started by hand: the arguments after `crystalline` (`serve` and its
    /// options, the global `--db` first). Empty for a daemon a client or
    /// the task started on demand.
    pub serve_args: Vec<String>,
    /// The names of the `CRYSTALLINE_*` variables the daemon read from its
    /// environment; never their values.
    pub env: Vec<String>,
}

/// The start of the daemon `info` records, `None` for a holder that is not
/// a daemon. The caller checks that its process is alive.
pub(crate) fn daemon_start(info: &LockInfo) -> Option<DaemonStart> {
    if info.standalone.is_some() {
        return None;
    }
    let by_task = info.runs_in.as_ref().is_some_and(|runs| runs.from_task);
    let by_hand = !by_task && info.started_by == Some(StartMode::Serve);
    let start = info.start.clone().unwrap_or_default();
    let mut serve_args = Vec::new();
    if by_hand {
        if let Some(db) = &start.db {
            serve_args.extend(["--db".to_string(), db.clone()]);
        }
        serve_args.push("serve".to_string());
        if start.exit_when_idle {
            serve_args.push("--exit-when-idle".to_string());
        }
        if start.read_only {
            serve_args.push("--read-only".to_string());
        }
        if let Some(http) = &start.http {
            serve_args.extend(["--http".to_string(), http.clone()]);
        }
        for host in &start.allowed_hosts {
            serve_args.extend(["--allowed-host".to_string(), host.clone()]);
        }
        if let Some(config) = &start.config {
            serve_args.extend(["--config".to_string(), config.clone()]);
        }
    }
    Some(DaemonStart {
        by_task,
        serve_args,
        env: if by_hand { start.env } else { Vec::new() },
    })
}

/// `text` as one PowerShell word in single quotes: every single quote
/// doubled, the typographic ones too (U+2018 to U+201B), which PowerShell
/// also reads as single quotes. Nothing in single quotes is expanded.
pub(crate) fn ps_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('\'');
    for c in text.chars() {
        if matches!(c, '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}') {
            out.push(c);
        }
        out.push(c);
    }
    out.push('\'');
    out
}

/// An argument as PowerShell passes it to a program: as it is when every
/// character is taken literally, else [`ps_quoted`]. A word that starts
/// with `-` stays bare only as a flag (letters and dashes): Windows
/// PowerShell 5.1 splits a bare `-a.b` into `-a` and `.b`.
fn ps_word(text: &str) -> String {
    let flag = text.starts_with('-') && text.chars().all(|c| c.is_ascii_lowercase() || c == '-');
    let plain = !text.is_empty()
        && !text.starts_with('@')
        && (!text.starts_with('-') || flag)
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || r"-_.:/\=+".contains(c));
    if plain {
        text.to_string()
    } else {
        ps_quoted(text)
    }
}

/// `path` as Windows text, without a verbatim prefix and without trailing
/// backslashes, so `\name` can follow. A drive root keeps its backslash
/// out too: `C:` + `\name` is `C:\name`.
fn folder_text(path: &Path) -> String {
    display_path(path).trim_end_matches('\\').to_string()
}

/// The `APPLICATIONFOLDER=` argument for msiexec. A root without a space
/// goes unquoted (`APPLICATIONFOLDER=D:\`), which msiexec reads to the next
/// space whichever way it treats backslashes. A root with a space is quoted,
/// and never ends in a backslash there, which would escape the closing
/// quote: only a drive root ends in one, and it holds no space.
fn application_folder(root: &Path) -> String {
    let text = display_path(root);
    if text.contains(' ') {
        format!("APPLICATIONFOLDER=\"{}\"", text.trim_end_matches('\\'))
    } else {
        format!("APPLICATIONFOLDER={text}")
    }
}

/// The step that closes the sessions, each of which holds crystalline.exe
/// open through its bridge.
const CLOSE_SESSIONS: &str = "Close every agent session that uses Crystalline (Claude Code, Claude Desktop, Codex, Cursor and others): each one holds crystalline.exe open.";

/// The MSI steps (S22): close the sessions and stop the daemon, run
/// msiexec with the install root as `APPLICATIONFOLDER` (S3) and wait for
/// its exit code (U4), what the code means, then start the daemon again as
/// it was started and run `status`. `default_folder` adds the double click
/// as the other way, which only an install in the default folder may use.
pub(crate) fn msi_steps(
    download: &Path,
    msi: &str,
    install_root: &Path,
    daemon: Option<&DaemonStart>,
    default_folder: bool,
) -> String {
    let exe = ps_quoted(&format!(
        r"{}\bin\crystalline.exe",
        folder_text(install_root)
    ));
    let package = format!(r"{}\{msi}", folder_text(download));
    let mut out =
        String::from("Install it with these steps in Windows PowerShell or PowerShell 7:\n");
    if daemon.is_some() {
        out.push_str(&format!(
            "1. {CLOSE_SESSIONS} Then stop the daemon, or the installer may ask for a reboot:\n   & {exe} ctl shutdown\n"
        ));
    } else {
        out.push_str(&format!("1. {CLOSE_SESSIONS}\n"));
    }
    out.push_str(&format!(
        "2. Run the installer and wait for it. Windows asks for administrator rights; the installer has no Authenticode signature, so the prompt names an unknown publisher.\n   $p = Start-Process msiexec.exe -Wait -PassThru -ArgumentList '/i', {}, '/passive', '/norestart', {}; $p.ExitCode\n",
        ps_quoted(&format!("\"{package}\"")),
        ps_quoted(&application_folder(install_root)),
    ));
    if default_folder {
        out.push_str(
            "   Crystalline is in the default folder, so a double click on the .msi works too.\n",
        );
    }
    out.push_str("3. 0 means installed. 3010 means installed and active after a reboot: reboot first, then go on with step 4.\n   1602 means cancelled, for example a no at the administrator prompt: nothing changed.\n   1618 means another installation is running: wait for it and run step 2 again.\n   Any other number means it did not install.\n");
    out.push_str(&format!(
        "4. {}\n   & {exe} status\n",
        start_again(daemon, &exe)
    ));
    out
}

/// How step 4 of the MSI steps starts the daemon again.
fn start_again(daemon: Option<&DaemonStart>, exe: &str) -> String {
    let Some(daemon) = daemon else {
        return "No daemon was running. Check the install:".to_string();
    };
    // The machine task, even for a daemon a per-user task started: every
    // MSI install registers it again (RegisterDaemonTask in main.wxs).
    if daemon.by_task {
        return format!(
            "Start the daemon again with its task, then check:\n   schtasks.exe /Run /TN {}",
            ps_quoted(MACHINE_TASK_NAME)
        );
    }
    if daemon.serve_args.is_empty() {
        return "The next agent session starts the daemon again. Check the install:".to_string();
    }
    let args: Vec<String> = daemon.serve_args.iter().map(|arg| ps_word(arg)).collect();
    let mut text = String::from("Start the daemon again the way it was started");
    if !daemon.env.is_empty() {
        text.push_str(&format!(
            ", with these variables set as they were then: {}",
            daemon.env.join(", ")
        ));
    }
    text.push_str(&format!(
        ". Then check in another window:\n   & {exe} {}",
        args.join(" ")
    ));
    text
}

/// The zip steps (S22): close the sessions, rename the running exe to
/// [`OLD_EXE`], copy the new one in, check its version, and run `status`,
/// which hands the daemon over to the new version with its start options.
pub(crate) fn zip_steps(download: &Path, canonical_exe: &Path, new_version: &Precedence) -> String {
    let exe = ps_quoted(&display_path(canonical_exe));
    let new = ps_quoted(&format!(r"{}\crystalline.exe", folder_text(download)));
    let mut out = String::from(
        "Replace crystalline.exe with these steps in Windows PowerShell or PowerShell 7:\n",
    );
    out.push_str(&format!("1. {CLOSE_SESSIONS}\n"));
    out.push_str(&format!(
        "2. Rename-Item -LiteralPath {exe} -NewName {}\n",
        ps_quoted(OLD_EXE)
    ));
    out.push_str(&format!(
        "3. Copy-Item -LiteralPath {new} -Destination {exe}\n"
    ));
    out.push_str(&format!(
        "4. & {exe} --version\n   It must print crystalline {}.\n",
        new_version.render()
    ));
    out.push_str(&format!(
        "5. & {exe} status\n   It hands the daemon over to the new version, started as before.\n"
    ));
    out
}

/// Whether `root` is the MSI's default folder, `<ProgramFiles>\Crystalline`,
/// compared without case. `program_files` is `%ProgramFiles%`.
pub(crate) fn is_default_folder(root: &Path, program_files: Option<&str>) -> bool {
    let Some(program_files) = program_files.filter(|dir| !dir.is_empty()) else {
        return false;
    };
    let default = format!(r"{}\Crystalline", program_files.trim_end_matches('\\'));
    folder_text(root).eq_ignore_ascii_case(&default)
}

/// The text written to `steps.txt`: the printed steps with Windows line
/// endings, after a UTF-8 byte order mark, which Windows PowerShell 5.1
/// needs to read a path that is not ASCII.
pub(crate) fn steps_file_bytes(steps: &str) -> Vec<u8> {
    let mut bytes = "\u{feff}".as_bytes().to_vec();
    bytes.extend_from_slice(steps.replace('\n', "\r\n").as_bytes());
    bytes
}

/// Remove a [`OLD_EXE`] an earlier update left beside `canonical_exe`: only
/// a plain file of that exact name. Best effort: an exe still running or a
/// folder this user may not write keeps it. Hands back the path when one
/// is still there.
pub(crate) fn remove_old_exe(canonical_exe: &Path) -> Option<std::path::PathBuf> {
    let old = canonical_exe.parent()?.join(OLD_EXE);
    let meta = std::fs::symlink_metadata(&old).ok()?;
    if !meta.is_file() {
        return None;
    }
    match std::fs::remove_file(&old) {
        Ok(()) => None,
        Err(_) => Some(old),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crystalline_service::instance::StartOptions;
    use crystalline_service::runs_in::RunsIn;

    const DOWNLOAD: &str = r"C:\Users\a\AppData\Local\crystalline\cache\updates\v0.34.0";
    const MSI: &str = "crystalline-v0.34.0-windows-amd64.msi";

    fn by_hand(args: &[&str]) -> DaemonStart {
        DaemonStart {
            by_task: false,
            serve_args: args.iter().map(|arg| arg.to_string()).collect(),
            env: Vec::new(),
        }
    }

    fn msi(root: &str, daemon: Option<&DaemonStart>, default_folder: bool) -> String {
        msi_steps(
            Path::new(DOWNLOAD),
            MSI,
            Path::new(root),
            daemon,
            default_folder,
        )
    }

    #[test]
    fn msi_steps_default_folder() {
        let spawned = by_hand(&[]);
        let text = msi(r"C:\Program Files\Crystalline", Some(&spawned), true);
        assert_eq!(
            text,
            r#"Install it with these steps in Windows PowerShell or PowerShell 7:
1. Close every agent session that uses Crystalline (Claude Code, Claude Desktop, Codex, Cursor and others): each one holds crystalline.exe open. Then stop the daemon, or the installer may ask for a reboot:
   & 'C:\Program Files\Crystalline\bin\crystalline.exe' ctl shutdown
2. Run the installer and wait for it. Windows asks for administrator rights; the installer has no Authenticode signature, so the prompt names an unknown publisher.
   $p = Start-Process msiexec.exe -Wait -PassThru -ArgumentList '/i', '"C:\Users\a\AppData\Local\crystalline\cache\updates\v0.34.0\crystalline-v0.34.0-windows-amd64.msi"', '/passive', '/norestart', 'APPLICATIONFOLDER="C:\Program Files\Crystalline"'; $p.ExitCode
   Crystalline is in the default folder, so a double click on the .msi works too.
3. 0 means installed. 3010 means installed and active after a reboot: reboot first, then go on with step 4.
   1602 means cancelled, for example a no at the administrator prompt: nothing changed.
   1618 means another installation is running: wait for it and run step 2 again.
   Any other number means it did not install.
4. The next agent session starts the daemon again. Check the install:
   & 'C:\Program Files\Crystalline\bin\crystalline.exe' status
"#
        );
        // With no daemon running there is nothing to stop.
        let text = msi(r"C:\Program Files\Crystalline", None, true);
        assert!(!text.contains("ctl shutdown"), "{text}");
        assert!(text.contains("open.\n2. Run the installer"), "{text}");
        assert!(
            text.contains("4. No daemon was running. Check the install:"),
            "{text}"
        );
    }

    #[test]
    fn msi_steps_custom_folder_keep_applicationfolder() {
        let spawned = by_hand(&[]);
        let text = msi(r"D:\Tools\Crystalline", Some(&spawned), false);
        assert!(
            text.contains(r"'APPLICATIONFOLDER=D:\Tools\Crystalline'; $p.ExitCode"),
            "{text}"
        );
        assert!(
            text.contains(r"& 'D:\Tools\Crystalline\bin\crystalline.exe' ctl shutdown"),
            "{text}"
        );
        assert!(!text.contains("double click"), "{text}");
    }

    #[test]
    fn a_root_ending_in_a_backslash() {
        // A drive root (the only root that ends in a backslash) goes
        // unquoted: no backslash ever stands before a closing quote.
        let text = msi(r"D:\", None, false);
        assert!(
            text.contains(r"'APPLICATIONFOLDER=D:\'; $p.ExitCode"),
            "{text}"
        );
        assert!(
            text.contains(r"& 'D:\bin\crystalline.exe' status"),
            "{text}"
        );
        assert!(!text.contains("\\\""), "{text}");
        // Without a space the value is unquoted too; with one it is quoted
        // and never ends in a backslash.
        let text = msi(r"D:\Tools\Crystalline", None, false);
        assert!(
            text.contains(r"'APPLICATIONFOLDER=D:\Tools\Crystalline'"),
            "{text}"
        );
        let text = msi(r"D:\My Tools\", None, false);
        assert!(
            text.contains(r#"'APPLICATIONFOLDER="D:\My Tools"'"#),
            "{text}"
        );
        assert!(!text.contains("\\\""), "{text}");
        // A download folder with a trailing backslash gives one separator.
        let text = msi_steps(
            Path::new(r"C:\cache\"),
            MSI,
            Path::new(r"C:\Program Files\Crystalline"),
            None,
            true,
        );
        assert!(text.contains(&format!(r#"'"C:\cache\{MSI}"'"#)), "{text}");
    }

    #[test]
    fn steps_quote_paths_with_spaces() {
        let root = r"C:\Program Files\My Tools\Crystalline";
        let text = msi(root, None, false);
        assert!(
            text.contains(r#"'APPLICATIONFOLDER="C:\Program Files\My Tools\Crystalline"'"#),
            "{text}"
        );
        assert!(
            text.contains(r"& 'C:\Program Files\My Tools\Crystalline\bin\crystalline.exe' status"),
            "{text}"
        );
        // Single quotes, the typographic ones too, are doubled; nothing in
        // them is expanded, so `$` and backticks stay as they are.
        let exe = Path::new("C:\\Users\\O\u{2019}Brien\\it's $HOME `x\\Jörg\\crystalline.exe");
        let zip = zip_steps(
            Path::new(r"C:\Users\O'Brien\cache\v0.34.0"),
            exe,
            &Precedence::parse("0.34.0").unwrap(),
        );
        assert!(
            zip.contains(
                "Rename-Item -LiteralPath 'C:\\Users\\O\u{2019}\u{2019}Brien\\it''s $HOME `x\\Jörg\\crystalline.exe' -NewName 'crystalline.old.exe'"
            ),
            "{zip}"
        );
        assert!(
            zip.contains(
                r"Copy-Item -LiteralPath 'C:\Users\O''Brien\cache\v0.34.0\crystalline.exe'"
            ),
            "{zip}"
        );
        assert_eq!(
            ps_quoted("\u{2018}\u{201A}\u{201B}"),
            "'\u{2018}\u{2018}\u{201A}\u{201A}\u{201B}\u{201B}'"
        );
    }

    #[test]
    fn msi_steps_name_the_daemon_start() {
        let task = DaemonStart {
            by_task: true,
            serve_args: Vec::new(),
            env: Vec::new(),
        };
        let text = msi(r"C:\Program Files\Crystalline", Some(&task), true);
        assert!(
            text.contains(
                "4. Start the daemon again with its task, then check:\n   schtasks.exe /Run /TN '\\Crystalline\\Daemon'\n   & 'C:\\Program Files\\Crystalline\\bin\\crystalline.exe' status\n"
            ),
            "{text}"
        );

        let serve = by_hand(&["serve", "--http", "0.0.0.0:7411"]);
        let text = msi(r"C:\Program Files\Crystalline", Some(&serve), true);
        assert!(
            text.contains(
                "4. Start the daemon again the way it was started. Then check in another window:\n   & 'C:\\Program Files\\Crystalline\\bin\\crystalline.exe' serve --http 0.0.0.0:7411\n   & 'C:\\Program Files\\Crystalline\\bin\\crystalline.exe' status\n"
            ),
            "{text}"
        );

        let mut quoted = by_hand(&["--db", r"C:\data dir\index.db", "serve", "--read-only"]);
        quoted.env = vec!["CRYSTALLINE_DATABASE_URL".to_string()];
        let text = msi(r"C:\Program Files\Crystalline", Some(&quoted), true);
        assert!(
            text.contains(
                "the way it was started, with these variables set as they were then: CRYSTALLINE_DATABASE_URL. Then"
            ),
            "{text}"
        );
        assert!(
            text.contains(r"crystalline.exe' --db 'C:\data dir\index.db' serve --read-only"),
            "{text}"
        );

        let spawned = by_hand(&[]);
        let text = msi(r"C:\Program Files\Crystalline", Some(&spawned), true);
        assert!(
            text.contains("4. The next agent session starts the daemon again. Check the install:"),
            "{text}"
        );
    }

    #[test]
    fn a_value_starting_with_a_dash_is_quoted_and_a_flag_is_not() {
        assert_eq!(ps_word("--allowed-host"), "--allowed-host");
        assert_eq!(ps_word("-x.lan"), "'-x.lan'");
        assert_eq!(ps_word("-1"), "'-1'");
        assert_eq!(ps_word("0.0.0.0:7411"), "0.0.0.0:7411");
    }

    #[test]
    fn zip_steps_rename_and_copy_the_canonical_exe() {
        let text = zip_steps(
            Path::new(DOWNLOAD),
            Path::new(r"\\?\C:\Users\a\tools\crystalline\crystalline.exe"),
            &Precedence::parse("0.34.0").unwrap(),
        );
        assert_eq!(
            text,
            r"Replace crystalline.exe with these steps in Windows PowerShell or PowerShell 7:
1. Close every agent session that uses Crystalline (Claude Code, Claude Desktop, Codex, Cursor and others): each one holds crystalline.exe open.
2. Rename-Item -LiteralPath 'C:\Users\a\tools\crystalline\crystalline.exe' -NewName 'crystalline.old.exe'
3. Copy-Item -LiteralPath 'C:\Users\a\AppData\Local\crystalline\cache\updates\v0.34.0\crystalline.exe' -Destination 'C:\Users\a\tools\crystalline\crystalline.exe'
4. & 'C:\Users\a\tools\crystalline\crystalline.exe' --version
   It must print crystalline 0.34.0.
5. & 'C:\Users\a\tools\crystalline\crystalline.exe' status
   It hands the daemon over to the new version, started as before.
"
        );
    }

    #[test]
    fn the_default_folder_is_program_files_without_case() {
        let root = Path::new(r"C:\Program Files\Crystalline");
        assert!(is_default_folder(root, Some(r"C:\Program Files")));
        assert!(is_default_folder(
            Path::new(r"\\?\c:\program files\crystalline\"),
            Some(r"C:\Program Files\")
        ));
        assert!(!is_default_folder(root, Some(r"D:\Program Files")));
        assert!(!is_default_folder(
            Path::new(r"C:\Program Files\Tools\Crystalline"),
            Some(r"C:\Program Files")
        ));
        assert!(!is_default_folder(root, None));
        assert!(!is_default_folder(root, Some("")));
    }

    fn record(
        started_by: Option<StartMode>,
        from_task: bool,
        start: Option<StartOptions>,
    ) -> LockInfo {
        serde_json::from_value::<LockInfo>(serde_json::json!({
            "pid": 1, "socket_path": "p", "version": "0.34.0", "started_at": "t",
        }))
        .map(|mut info| {
            info.started_by = started_by;
            info.runs_in = Some(RunsIn {
                from_task,
                ..RunsIn::default()
            });
            info.start = start;
            info
        })
        .unwrap()
    }

    #[test]
    fn the_daemon_start_comes_from_its_record() {
        let options = StartOptions {
            db: Some(r"C:\d\index.db".to_string()),
            config: Some(r"C:\c\config.yaml".to_string()),
            read_only: true,
            http: Some("0.0.0.0:7411".to_string()),
            allowed_hosts: vec!["box.lan".to_string()],
            exit_when_idle: false,
            env: vec!["CRYSTALLINE_SERVICE_HTTP".to_string()],
            partial: false,
        };
        let hand = daemon_start(&record(
            Some(StartMode::Serve),
            false,
            Some(options.clone()),
        ))
        .unwrap();
        assert!(!hand.by_task);
        assert_eq!(
            hand.serve_args,
            [
                "--db",
                r"C:\d\index.db",
                "serve",
                "--read-only",
                "--http",
                "0.0.0.0:7411",
                "--allowed-host",
                "box.lan",
                "--config",
                r"C:\c\config.yaml"
            ]
        );
        assert_eq!(hand.env, ["CRYSTALLINE_SERVICE_HTTP"]);

        // The task's daemon records an autostart; only `from_task` tells.
        let task = daemon_start(&record(
            Some(StartMode::Autostart),
            true,
            Some(options.clone()),
        ))
        .unwrap();
        assert_eq!(
            (task.by_task, task.serve_args.len(), task.env.len()),
            (true, 0, 0)
        );

        let spawned =
            daemon_start(&record(Some(StartMode::Autostart), false, Some(options))).unwrap();
        assert_eq!((spawned.by_task, spawned.serve_args.len()), (false, 0));

        let old = daemon_start(&record(Some(StartMode::Serve), false, None)).unwrap();
        assert_eq!(old.serve_args, ["serve"]);

        let mut holder = record(None, false, None);
        holder.standalone = Some("crystalline domain rename".to_string());
        assert_eq!(daemon_start(&holder), None);
    }

    #[test]
    fn only_a_plain_old_exe_beside_the_exe_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("crystalline.exe");
        std::fs::write(&exe, b"current").unwrap();
        assert_eq!(remove_old_exe(&exe), None, "nothing there");
        std::fs::write(dir.path().join(OLD_EXE), b"old").unwrap();
        std::fs::write(dir.path().join("crystalline.old.exe.bak"), b"other").unwrap();
        assert_eq!(remove_old_exe(&exe), None);
        assert!(!dir.path().join(OLD_EXE).exists());
        assert!(dir.path().join("crystalline.old.exe.bak").exists());
        assert_eq!(std::fs::read(&exe).unwrap(), b"current");
        std::fs::create_dir(dir.path().join(OLD_EXE)).unwrap();
        assert_eq!(remove_old_exe(&exe), None, "a folder is left alone");
        assert!(dir.path().join(OLD_EXE).is_dir());
    }

    #[test]
    fn the_steps_file_is_the_text_after_a_byte_order_mark() {
        let bytes = steps_file_bytes("1. Jörg\n   & x\n");
        assert_eq!(&bytes[..3], [0xEF, 0xBB, 0xBF]);
        assert_eq!(
            std::str::from_utf8(&bytes[3..]).unwrap(),
            "1. Jörg\r\n   & x\r\n"
        );
    }
}
