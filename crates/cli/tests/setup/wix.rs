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
    assert_eq!(WXS.matches("Execute='deferred'").count(), 2);
    assert_eq!(WXS.matches("Execute='rollback'").count(), 1);
    assert_eq!(
        WXS.matches("Impersonate='no'").count(),
        3,
        "all three run as the installer, elevated"
    );
    assert_eq!(
        WXS.matches("Return='ignore'").count(),
        3,
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
fn the_msi_schedules_all_three_inside_the_install_execute_sequence() {
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
    assert!(
        rollback < register,
        "the rollback is written before the register it undoes"
    );
}
