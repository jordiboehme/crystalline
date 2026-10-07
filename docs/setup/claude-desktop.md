# Claude Desktop

The Claude Desktop extension connects Claude Desktop to the Crystalline on your computer. Install Crystalline first, then the extension.

## macOS

1. Install Crystalline with [Homebrew](https://brew.sh): `brew install jordiboehme/tap/crystalline`.
2. Download `crystalline-v<version>.mcpb` from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest).
3. In Claude Desktop, open Settings > Extensions > Advanced settings > Install Extension... and pick the file.

Do you use only Claude Desktop, and neither Claude Code nor the `crystalline` command? Then take `crystalline-desktop-only-v<version>-macos.mcpb` instead. It carries Crystalline inside and needs nothing else. Its daemon runs while Claude Desktop is open and stops five seconds after it quits. To update it, install the newer bundle over the old one.

## Windows

1. Download `crystalline-<version>-windows-amd64.msi` (or `-windows-arm64.msi` on an Arm device) from the [latest release](https://github.com/jordiboehme/crystalline/releases/latest) and run it. Keep the default install folder, `Program Files\Crystalline\bin`. `crystalline doctor` does not recognise a custom folder.
2. Download `crystalline-v<version>.mcpb` from the same release.
3. In Claude Desktop, open Settings > Extensions > Advanced settings > Install Extension... and pick the file.

Claude Desktop runs its extensions inside its own app package. So the extension never runs the Crystalline knowledge stack itself: it connects to the daemon outside the package. The MSI registers a Windows task, `\Crystalline\Daemon`, for all users. It starts the daemon when you sign in. Inside Claude Desktop's package, `crystalline mcp` only connects to that daemon and never starts one itself. If the task is missing (it was deleted, or the MSI could not register it), run `crystalline doctor --fix` in a terminal. It registers a task for your user. The MSI logs only an exit code for these steps, so use `crystalline doctor` to see details.

When you sign out or shut down, the daemon stops cleanly.

## How it starts

It starts with no domains: the agent creates one with the `add_domain` tool whenever it needs somewhere to capture knowledge - a folder of markdown files under your `Documents/Crystalline` folder, a database-backed domain or a GitHub team domain. Onboarding is automatic on every connection (see [Session onboarding](../learning-loop.md#session-onboarding)). The daemon serves the web UI at `http://localhost:7411` by default, where the first visit creates your admin account. The [Claude Desktop extension scenario](../deployment.md#claude-desktop-extension) shows how it works underneath.

## "Install Crystalline first"

The extension looks for Crystalline in the usual places: the two Homebrew folders and then your `PATH` on macOS, `Program Files` and then your `PATH` on Windows. When it finds none, or finds one older than 0.24.0, Claude says so in the chat. It names what is missing and the command or download that fixes it. Install or update, then restart Claude Desktop. Nothing is lost: your knowledge stays where it was.

## Moving from an older extension on Windows

Extensions before 0.24.0 carried their own Crystalline. On Windows that one could keep a second state inside Claude Desktop's package (`%LOCALAPPDATA%\Packages\Claude_...\LocalCache\Roaming\crystalline`). `crystalline doctor` tells you when there is one: how many domains it registers, how big its index is and when it last changed. To merge it into your normal state, quit Claude Desktop and run:

    crystalline doctor --fix --merge-desktop-state

It adds the domains it registers to yours and moves database-backed domains that exist only there. It never overwrites or deletes anything. It renames the folder to `crystalline.merged-<date>` only when everything was merged. A domain name that means two different things on the two sides is reported, and then nothing is renamed until you sort it out. Team edits made through the old extension that you did not share yet: share them from the old extension before you remove it.

## The companion skill

Download `crystalline-claude-desktop-skill-v<version>.zip` from the latest release, then open Settings > Capabilities > Skills (enable the Skills capability there if it is off) and upload the zip as-is (it contains the `crystalline-intelligence` folder; do not unpack it). If you uploaded an earlier release's skill, delete the old `crystalline-memory` entry there once the new one is up - Desktop keeps uploaded skills side by side, and the two teach the same lessons twice. Routing itself needs no skill - the server's instructions deliver it automatically; the skill adds capture and collaboration best practices.
