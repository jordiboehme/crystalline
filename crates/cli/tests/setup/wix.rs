//! The MSI's half of the Windows daemon task, read from the WiX source: the
//! build itself only runs on the Windows legs of the release workflow, and
//! this keeps the source honest on every platform.

const WXS: &str = include_str!("../../wix/main.wxs");

const ROLLBACK: &str = "<Custom Action='RollbackRegisterDaemonTask' Before='RegisterDaemonTask'>NOT Installed AND NOT WIX_UPGRADE_DETECTED AND NOT REMOVE~=\"ALL\"</Custom>";
const REGISTER: &str =
    "<Custom Action='RegisterDaemonTask' After='InstallFiles'>NOT REMOVE~=\"ALL\"</Custom>";
// A major upgrade removes the old product with REMOVE="ALL" and
// UPGRADINGPRODUCTCODE set. That removal must leave the task alone, so a
// register the new version gets refused never leaves the machine without
// one. The old product's cached MSI decides this, so the condition shipped
// now governs every later upgrade.
const UNREGISTER: &str = "<Custom Action='UnregisterDaemonTask' Before='RemoveFiles'>REMOVE~=\"ALL\" AND NOT UPGRADINGPRODUCTCODE</Custom>";
const END: &str =
    "<Custom Action='EndDaemonTasks' Before='UnregisterDaemonTask'>REMOVE~=\"ALL\"</Custom>";
const END_SCRIPT: &str = include_str!("../../wix/end-daemon-tasks.ps1");

/// Standard base64, as PowerShell's -EncodedCommand reads it.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The action runs the checked-in script as it is: Windows PowerShell from
/// the system folder, the script base64-encoded so MSI's formatted text
/// never reads a bracket in it as a property.
#[test]
fn the_msi_ends_running_daemon_tasks_with_the_checked_in_script() {
    let utf16: Vec<u8> = END_SCRIPT
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    let command = format!(
        "ExeCommand='\"[System64Folder]WindowsPowerShell\\v1.0\\powershell.exe\" -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}'",
        base64(&utf16)
    );
    assert!(
        WXS.contains(&command),
        "main.wxs must run end-daemon-tasks.ps1 exactly; the ExeCommand is:\n{command}"
    );
    assert!(WXS.contains("<CustomAction Id='EndDaemonTasks'"));
    assert!(
        WXS.contains("Directory='TARGETDIR'"),
        "a Type 34 action: no file of this product"
    );
    for needle in [
        r"'\Crystalline\'",
        "'Daemon'",
        r"'\'",
        "'Crystalline Daemon for *'",
        "/End /TN",
        "exit 0",
    ] {
        assert!(END_SCRIPT.contains(needle), "the script lacks {needle}");
    }
}

#[test]
fn the_msi_registers_the_task_after_the_files_and_removes_it_before_them() {
    for needle in [
        "<CustomAction Id='RegisterDaemonTask'",
        "ExeCommand='daemon-task register --all-users'",
        "<CustomAction Id='RollbackRegisterDaemonTask'",
        "<CustomAction Id='UnregisterDaemonTask'",
    ] {
        assert!(WXS.contains(needle), "main.wxs lacks {needle}");
    }
    assert_eq!(
        WXS.matches("ExeCommand='daemon-task unregister --all-users'")
            .count(),
        2,
        "the uninstall and the rollback of a first install both remove it"
    );
    assert_eq!(
        WXS.matches("FileKey='CrystallineExe'").count(),
        3,
        "all three run the installed binary"
    );
    assert_eq!(WXS.matches("Execute='deferred'").count(), 3);
    assert_eq!(WXS.matches("Execute='rollback'").count(), 1);
    assert_eq!(
        WXS.matches("Impersonate='no'").count(),
        4,
        "all four run as the installer, elevated"
    );
    assert_eq!(
        WXS.matches("Return='ignore'").count(),
        4,
        "a task Windows refuses must never roll back the install"
    );
    assert!(
        WXS.contains("<MajorUpgrade Schedule='afterInstallInitialize'"),
        "the old product is removed before the new files go in"
    );
    assert!(
        !WXS.contains("<Upgrade "),
        "MajorUpgrade alone sets WIX_UPGRADE_DETECTED, which the rollback reads"
    );
    assert!(
        WXS.contains("InstallScope='perMachine'"),
        "the task is a machine task, so the install is per machine"
    );
    assert!(
        WXS.contains("UpgradeCode='A04060D9-E5E1-4E8E-8751-2DCA3D8896CC'"),
        "never changes"
    );
}

#[test]
fn the_msi_schedules_all_four_inside_the_install_execute_sequence() {
    let open = "<InstallExecuteSequence>";
    let close = "</InstallExecuteSequence>";
    assert_eq!(WXS.matches(open).count(), 1);
    assert_eq!(WXS.matches(close).count(), 1);
    let start = WXS.find(open).unwrap() + open.len();
    let end = WXS.find(close).unwrap();
    assert!(start < end, "the sequence element closes after it opens");
    let at = |needle: &str| {
        assert_eq!(WXS.matches(needle).count(), 1, "exactly one {needle}");
        let i = WXS.find(needle).unwrap();
        assert!(
            start <= i && i + needle.len() <= end,
            "{needle} sits outside <InstallExecuteSequence>"
        );
        i
    };
    let rollback = at(ROLLBACK);
    let register = at(REGISTER);
    at(UNREGISTER);
    at(END);
    assert!(
        rollback < register,
        "the rollback is written before the register it undoes"
    );
}
