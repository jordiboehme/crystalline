/**
 * What the probe's `update_available` may put on the page (spec S30).
 *
 * The server sends the newer version as a plain `x.y.z`, and this side checks
 * it again: a value of any other shape (a tag name, markup, a pre-release)
 * reads as no update at all, so nothing the payload carries ever becomes text
 * or a link here unchecked. The link and the command are built on this side,
 * from the checked version and a fixed string.
 */

/** A stable release version and nothing else. */
const PLAIN_VERSION = /^\d+\.\d+\.\d+$/;

/** The command that installs the update, run on the server. */
export const UPDATE_COMMAND = "crystalline update now";

/** The newer version the probe names, or null when it names none or a bad one. */
export function updateAvailableOf(
  value: string | null | undefined,
): string | null {
  return typeof value === "string" && PLAIN_VERSION.test(value) ? value : null;
}

/** The release page of a version, the same page `crystalline update check` names. */
export function releaseNotesUrl(version: string): string {
  return `https://github.com/jordiboehme/crystalline/releases/tag/v${version}`;
}
