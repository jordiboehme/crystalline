# Claude Desktop

The Claude Desktop extension connects Claude Desktop to the Crystalline on your computer. Install Crystalline first, then run one command to build the extension and open it in Claude Desktop.

## macOS

1. Install Crystalline with [Homebrew](https://brew.sh): `brew install jordiboehme/tap/crystalline`.
2. Run `crystalline install claude-desktop` in a terminal.
3. Claude Desktop shows an install dialog. Confirm it there.

If you do not want the command to open Claude Desktop, add `--no-open`. It then also does not show the skill zip in Finder. Open Claude Desktop, go to Settings > Extensions > Advanced settings > Install Extension... and pick the file the command printed.

Do you use only Claude Desktop, and neither Claude Code nor the `crystalline` command? Then take `crystalline-claude-desktop-only-v<version>-macos.mcpb` instead. It carries Crystalline inside and needs nothing else. Its daemon runs while Claude Desktop is open and stops five seconds after it quits. To update it, install the newer bundle over the old one.

## Windows

1. Download `crystalline-<version>-windows-amd64.msi` (or `-windows-arm64.msi` on an Arm device) from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) and run it.
2. Run `crystalline install claude-desktop` in a terminal.
3. Claude Desktop shows an install dialog. Confirm it there.

With `--no-open` the command does not open Claude Desktop and does not show the skill zip in File Explorer. On Windows the command may also not be able to open the file in Claude Desktop. Then it prints the path and the steps: open Claude Desktop, go to Settings > Extensions > Advanced settings > Install Extension... and pick the file. You can also double-click the file in File Explorer.

Claude Desktop runs its extensions inside its own app package. So the extension never runs the Crystalline knowledge stack itself: it connects to the daemon outside the package. The MSI registers a Windows task, `\Crystalline\Daemon`, for all users. It starts the daemon when you sign in. Inside Claude Desktop's package, `crystalline mcp` only connects to that daemon and never starts one itself. If the task is missing (it was deleted, or the MSI could not register it), run `crystalline doctor --fix` in a terminal. It registers a task for your user, but only for the crystalline the MSI installed in its default folder, `Program Files\Crystalline\bin`. For a copy anywhere else it reports the task and registers none. Install the MSI, or run it again, to get the task. When the machine-wide task is broken, `--fix` asks for an administrator instead of fixing it. For details, run `crystalline doctor`.

When you sign out or shut down, the daemon stops cleanly.

A `crystalline` command that runs inside Claude Desktop, for example from Claude Code's shell there, also works only through that daemon: it sends its changes to the daemon, and without one it says why and asks you to run it in a terminal outside Claude Desktop. Setup steps such as `crystalline connect`, `crystalline install` and `crystalline doctor --fix` always belong in a terminal outside Claude Desktop.

## What the command builds

The command builds a small extension on your computer and writes it to `crystalline.mcpb` in the `claude-desktop` folder of Crystalline's state folder. The extension carries no Crystalline. It runs the first crystalline on your `PATH` (or the one you started the command with, when `PATH` has none), and the command prints which one. When that one has moved, the extension falls back to its own search. A new extension installs over the one from an older release. Only an extension from 0.7.3 to 0.17.x has another id, so Claude Desktop keeps it as a second Crystalline beside the new one. Remove the older one in Settings > Extensions.

Crystalline cannot see what Claude Desktop has installed. It only remembers what it did last. So each run builds the extension again and opens it again.

## Updating

When you update Crystalline (`brew upgrade crystalline` on macOS, the newest MSI on Windows), the extension uses the new version at once. Crystalline also builds the extension again for the new version. The next time Claude Desktop starts the extension, Claude Desktop shows a dialog to update it. Confirm it there. On Windows the daemon opens the dialog, because Claude Desktop's own package may not write the file.

Crystalline asks once per version. If you said no, or no dialog came, run `crystalline install claude-desktop` in a terminal. `crystalline doctor` says when an update for this version was not offered yet. Without a record from `crystalline install claude-desktop` (an extension you downloaded, for example), nothing is offered: run the command once.

The Desktop-only bundle is different: it carries Crystalline, so to update it, install the newer bundle over the old one.

## Remove it

Run `crystalline uninstall claude-desktop`. It deletes the built extension and Crystalline's record of it. Then remove the extension in Settings > Extensions in Claude Desktop. The skill zip in your Downloads folder stays, and so does the skill in Claude Desktop until you delete it under Customize > Skills.

## How it starts

It starts with no domains: the agent creates one with the `add_domain` tool whenever it needs somewhere to capture knowledge - a folder of markdown files under your `Documents/Crystalline` folder, a database-backed domain or a GitHub team domain. Onboarding is automatic on every connection (see [Session onboarding](../learning-loop.md#session-onboarding)). The daemon serves the web UI at `http://localhost:7411` by default, where the first visit creates your admin account. The [Claude Desktop extension scenario](../deployment.md#claude-desktop-extension) shows how it works underneath.

## "Install Crystalline first"

The extension first runs the crystalline that `crystalline install claude-desktop` pinned. When that is gone, it looks in the usual places: the two Homebrew folders and then your `PATH` on macOS, `Program Files` and then your `PATH` on Windows. When it finds none, or finds one older than 0.24.0, Claude says so in the chat. It names what is missing and the command or download that fixes it. Install or update, run `crystalline install claude-desktop` again if it asks, then restart Claude Desktop. Nothing is lost: your knowledge stays where it was.

## A second state inside Claude Desktop on Windows

Extensions before 0.24.0 carried their own Crystalline. Restart Claude Desktop after you update the extension. Until then it can still use the old extension's Crystalline. On Windows that one could keep a second state inside Claude Desktop's package (`%LOCALAPPDATA%\Packages\Claude_...\LocalCache\Roaming\crystalline`). Before 0.30.0 a `crystalline` command that ran inside Claude Desktop, such as `crystalline domain add` from Claude Code's shell there, could create or extend that state too. You notice it when searches find no domains while the list of domains at the start of a session shows them. `crystalline doctor` tells you when there is one: how many domains it registers, which of them the daemon does not serve, how big its index is and when it last changed. To merge it into your normal state, quit Claude Desktop and run:

    crystalline doctor --fix --merge-desktop-state

It adds the domains it registers to yours and moves database-backed domains that exist only there. It never overwrites or deletes anything. It renames the folder to `crystalline.merged-<date>` only when everything was merged. A domain name that means two different things on the two sides is reported, and then nothing is renamed until you sort it out. A team domain comes over with its team state. Team edits you did not share yet are still there: share them as usual after the merge. If team domains are turned off on this computer, turn them on first with `crystalline config set github.enabled true`.

## The companion skill

`crystalline install claude-desktop` saves the skill as `crystalline-intelligence.zip` in your Downloads folder and shows it in Finder or File Explorer. If it cannot write there, it saves the zip in Crystalline's state folder and prints the path. Then open Customize > Skills in Claude Desktop (enable the Skills capability there if it is off) and upload the zip as-is. It contains the `crystalline-intelligence` folder; do not unpack it. A run never overwrites a zip: it reuses an identical file and gives a changed skill the next free name, such as `crystalline-intelligence 2.zip`. If you uploaded the older `crystalline-memory` skill, delete it there once the new one is up - Desktop keeps uploaded skills side by side, and the two teach the same lessons twice. If you prefer to upload by hand, the `crystalline-claude-desktop-skill-v<version>.zip` on the release page is the same skill. Routing itself needs no skill - the server's instructions deliver it automatically; the skill adds capture and collaboration best practices.
