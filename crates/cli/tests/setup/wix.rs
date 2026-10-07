//! The MSI's half of the Windows daemon task, read from the WiX source: the
//! build itself only runs on the Windows legs of the release workflow, and
//! this keeps the source honest on every platform.

const WXS: &str = include_str!("../../wix/main.wxs");

#[test]
fn the_msi_registers_the_task_after_the_files_and_removes_it_before_them() {
    for needle in [
        "<CustomAction Id='RegisterDaemonTask'",
        "ExeCommand='daemon-task register --all-users'",
        "<CustomAction Id='UnregisterDaemonTask'",
        "ExeCommand='daemon-task unregister --all-users'",
        "<Custom Action='RegisterDaemonTask' After='InstallFiles'>NOT REMOVE~=\"ALL\"</Custom>",
        // A major upgrade removes the old product with REMOVE="ALL" and
        // UPGRADINGPRODUCTCODE set. That removal must leave the task alone,
        // so a register the new version gets refused never leaves the
        // machine without one. The old product's cached MSI decides this, so
        // the condition shipped now governs every later upgrade.
        "<Custom Action='UnregisterDaemonTask' Before='RemoveFiles'>REMOVE~=\"ALL\" AND NOT UPGRADINGPRODUCTCODE</Custom>",
    ] {
        assert!(WXS.contains(needle), "main.wxs lacks {needle}");
    }
    assert_eq!(
        WXS.matches("FileKey='CrystallineExe'").count(),
        2,
        "both run the installed binary"
    );
    assert_eq!(WXS.matches("Execute='deferred'").count(), 2);
    assert_eq!(
        WXS.matches("Impersonate='no'").count(),
        2,
        "both run as the installer, elevated"
    );
    assert_eq!(
        WXS.matches("Return='ignore'").count(),
        2,
        "a task Windows refuses must never roll back the install"
    );
    assert!(
        WXS.contains("<MajorUpgrade Schedule='afterInstallInitialize'"),
        "the old product is removed before the new files go in"
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
