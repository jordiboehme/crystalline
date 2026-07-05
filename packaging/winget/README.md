# winget packaging

Crystalline ships on the Windows Package Manager as `JordiBoehme.Crystalline` (publisher `Jordi Böhme`, moniker `crystalline`). One manifest carries two installers: the release MSI first (machine scope, for administrators) and the release zip as a nested portable installer with the command alias `crystalline` (user scope, no administrator rights needed). The community repository [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs) is the source of truth; nothing version-specific is checked in here.

The `winget` job in `.github/workflows/release.yml` submits an updated manifest on every tagged release, after the GitHub release is published. It skips itself with a notice when the `WINGET_GITHUB_TOKEN` secret is unset (forks), when the package does not exist in winget-pkgs yet (the first submission is manual, below) and when the version was already submitted, so re-running a release workflow is safe.

## Secret

`WINGET_GITHUB_TOKEN` is a GitHub classic personal access token with only the `public_repo` scope, stored as an Actions secret. wingetcreate uses it to fork winget-pkgs and open the pull request; fine-grained tokens are not supported by wingetcreate.

## First submission (manual, once)

New packages go through winget-pkgs moderation, which takes days, so the first version is submitted by hand from any Windows machine after the first MSI-bearing release is public:

1. Install the tool: `winget install wingetcreate` (or download `https://aka.ms/wingetcreate/latest`).
2. Generate the manifests, with the MSI first and the zip second:

   ```powershell
   wingetcreate new `
     "https://github.com/jordiboehme/crystalline/releases/download/vX.Y.Z/crystalline-vX.Y.Z-windows-amd64.msi|x64|machine" `
     "https://github.com/jordiboehme/crystalline/releases/download/vX.Y.Z/crystalline-vX.Y.Z-windows-amd64.zip|x64"
   ```

3. Interactive answers: identifier `JordiBoehme.Crystalline`, publisher `Jordi Böhme`, package name `Crystalline`, license `AGPL-3.0-or-later`, license URL `https://github.com/jordiboehme/crystalline/blob/main/LICENSE`, package URL `https://github.com/jordiboehme/crystalline`, moniker `crystalline`, short description "Local-first knowledge management for humans and AI agents." For the zip installer: nested installer type `portable`, relative file path `crystalline.exe` (the zip is flat) and portable command alias `crystalline`.
4. Review the generated manifests and validate them: `winget validate --manifest <dir>`.
5. Submit with the token (wingetcreate offers to submit, or run `wingetcreate submit --token <PAT> <dir>`) and watch the validation pipeline on the winget-pkgs pull request.

Once the first manifest merges, `winget install JordiBoehme.Crystalline` works and every later tag flows through the CI job automatically. Drop the moderation qualifier from the README's Windows install section at that point.
