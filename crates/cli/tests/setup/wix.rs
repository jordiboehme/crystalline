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
    // Present and inside the sequence only: MSI orders the actions by their
    // Before and After attributes, never by where they are written, and
    // `the_task_actions_run_in_the_order_their_anchors_give` checks those.
    at(ROLLBACK);
    at(REGISTER);
    at(UNREGISTER);
    at(END);
}

/// Where `action` lands in the install execute sequence of `wxs`, resolved
/// the way `Before` and `After` place it: the two standard actions the task
/// actions hang off at their sequence numbers, a custom action just before
/// or after its anchor.
fn sequence_position(wxs: &str, action: &str) -> i64 {
    match action {
        "RemoveFiles" => 3500 * 100,
        "InstallFiles" => 4000 * 100,
        _ => {
            let open = format!("<Custom Action='{action}' ");
            let at = wxs
                .find(&open)
                .unwrap_or_else(|| panic!("no <Custom> for {action}"))
                + open.len();
            let (relation, rest) = wxs[at..].split_once("='").unwrap();
            let anchor = rest.split('\'').next().unwrap();
            match relation {
                "Before" => sequence_position(wxs, anchor) - 1,
                "After" => sequence_position(wxs, anchor) + 10,
                other => panic!("{action} is placed with {other}, not Before or After"),
            }
        }
    }
}

const TASK_ORDER: [&str; 6] = [
    "EndDaemonTasks",
    "UnregisterDaemonTask",
    "RemoveFiles",
    "InstallFiles",
    "RollbackRegisterDaemonTask",
    "RegisterDaemonTask",
];

fn in_task_order(wxs: &str) -> bool {
    let at: Vec<i64> = TASK_ORDER
        .iter()
        .map(|action| sequence_position(wxs, action))
        .collect();
    at.windows(2).all(|pair| pair[0] < pair[1])
}

/// The daemons end before their tasks go, the tasks go before the files,
/// and the rollback is scheduled before the register it undoes, all after
/// the new files are in: the order MSI runs them, from the attributes.
#[test]
fn the_task_actions_run_in_the_order_their_anchors_give() {
    assert!(in_task_order(WXS), "{TASK_ORDER:?}");
    let moved = WXS.replace(
        "Action='UnregisterDaemonTask' Before='RemoveFiles'",
        "Action='UnregisterDaemonTask' After='InstallFiles'",
    );
    assert_ne!(moved, WXS, "the anchor this red relies on is in main.wxs");
    assert!(
        !in_task_order(&moved),
        "an unregister moved after the new files is caught"
    );
}

/// The value of `name='...'` in one tag's text.
fn attribute<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    let start = tag.find(&format!(" {name}='"))? + name.len() + 3;
    let len = tag[start..].find('\'')?;
    Some(&tag[start..start + len])
}

/// Every `File` in the source with its `Name`, the `Id` of the `Directory`
/// it lies in and the `Id` of its `Component`, read from the nesting.
fn files(wxs: &str) -> Vec<(String, String, String)> {
    let mut directories: Vec<String> = Vec::new();
    let mut component = String::new();
    let mut found = Vec::new();
    for piece in wxs.split('<').skip(1) {
        let Some(end) = piece.find('>') else { continue };
        let tag = &piece[..end];
        if tag.starts_with("Directory ") && !tag.ends_with('/') {
            directories.push(attribute(tag, "Id").unwrap_or_default().to_owned());
        } else if tag.starts_with("/Directory") {
            directories.pop();
        } else if tag.starts_with("Component ") {
            component = attribute(tag, "Id").unwrap_or_default().to_owned();
        } else if tag.starts_with("File ") {
            found.push((
                attribute(tag, "Name").unwrap_or_default().to_owned(),
                directories.last().cloned().unwrap_or_default(),
                component.clone(),
            ));
        }
    }
    found
}

/// The marker detection reads beside the canonical exe (`install-source`
/// in `bin\`, saying `msi`) is installed by a component of the always-on
/// feature, in the very directory `crystalline.exe` goes to. The
/// UpgradeCode stays the one every released MSI carries.
#[test]
fn the_msi_installs_the_marker_beside_the_exe() {
    let marker = crystalline_core::update::MARKER_FILE;
    let shipped = concat!(env!("CARGO_MANIFEST_DIR"), "/wix/install-source");
    assert_eq!(
        std::fs::read_to_string(shipped).ok().as_deref(),
        Some("msi\n"),
        "{shipped} says msi"
    );
    let files = files(WXS);
    let exe = files
        .iter()
        .find(|(name, ..)| name == "crystalline.exe")
        .expect("main.wxs installs crystalline.exe");
    let installed = files
        .iter()
        .find(|(name, ..)| name == marker)
        .unwrap_or_else(|| panic!("main.wxs installs no {marker} file"));
    assert_eq!(installed.1, exe.1, "the marker lies beside the exe");
    let feature_start = WXS
        .find("<Feature Id='Binaries'")
        .expect("the Binaries feature");
    let feature = &WXS[feature_start..];
    assert!(
        feature.contains(&format!("<ComponentRef Id='{}'/>", installed.2)),
        "the Binaries feature installs the {} component",
        installed.2
    );
    let tag_start = WXS
        .find(&format!("Name='{marker}'"))
        .expect("the marker's File tag");
    let tag_end = tag_start + WXS[tag_start..].find("/>").expect("a closed tag");
    assert!(
        WXS[tag_start..tag_end].contains(&format!(
            "Source='$(var.CargoTargetDir)\\..\\crates\\cli\\wix\\{marker}'"
        )),
        "the marker comes from crates/cli/wix, not from the working directory"
    );
    assert!(WXS.contains("UpgradeCode='A04060D9-E5E1-4E8E-8751-2DCA3D8896CC'"));
}
