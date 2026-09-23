# Release notes

One file per released tag, named after the tag, for example `v0.19.3.md`.

The file is the one source of truth for that release's GitHub Release page.
It is written in the release PR before the tag is pushed, and a later
correction is a commit to the file: pushing it to main applies the change to
the already-published page, no manual edit needed.

House format: a motto as the first line (an H1; every release from v0.7.1 on
has one), one summary line, then `New & Noteworthy`, `Fixes` and `Breaking
changes` sections as they apply. No em or en dashes, plain '-' instead. Only
things a reader can know: no internal plan names or working notes.

This README is not a notes file: every check and the sync workflow match
`release-notes/v*.md` only.
