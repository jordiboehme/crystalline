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

/** The command that turns the daily update check on, run on the server. */
export const UPDATE_AUTO_ON_COMMAND = "crystalline update auto on";

/** Why the server's update check is off, as the probe names it. */
export type UpdateCheckOff =
  | "not_turned_on"
  | "no_update_notifier"
  | "ci"
  | "docker"
  | "desktop_extension"
  | "no_state_folder";

/** What the server's last update check found. */
export type UpdateLastCheck = "up_to_date" | "newer" | "failed";

/** The state of the server's daily update check, as the page may show it. */
export interface UpdateCheckState {
  /** Whether the check runs. */
  on: boolean;
  /** Why it is off; null when it is on. */
  offReason: UpdateCheckOff | null;
  /**
   * Whether the server is a .deb install, whose service reads its settings
   * from `/etc/default/crystalline`.
   */
  deb: boolean;
  /** When the last check ran, as the server sent it; null when it is off or has not run. */
  lastChecked: string | null;
  /**
   * What the last check found; null when it is off, has not run, or found
   * something this page does not know.
   */
  lastCheck: UpdateLastCheck | null;
}

const OFF_REASONS: readonly UpdateCheckOff[] = [
  "not_turned_on",
  "no_update_notifier",
  "ci",
  "docker",
  "desktop_extension",
  "no_state_folder",
];

const LAST_CHECKS: readonly UpdateLastCheck[] = [
  "up_to_date",
  "newer",
  "failed",
];

/**
 * The probe's `update_check`, or null when it names none or a bad one. Only
 * known words pass, so nothing the payload carries becomes text here
 * unchecked: the page builds every sentence itself. A check that is on with
 * an off reason, or off without one, reads as none. A time that does not
 * parse is dropped, and with it what the check found. A result word this
 * page does not know drops only the result: the time stays.
 */
export function updateCheckOf(value: unknown): UpdateCheckState | null {
  if (typeof value !== "object" || value === null) {
    return null;
  }
  const raw = value as Record<string, unknown>;
  if (typeof raw.on !== "boolean") {
    return null;
  }
  const offReason = OFF_REASONS.find((reason) => reason === raw.off_reason);
  if (raw.on === (offReason !== undefined)) {
    return null;
  }
  const lastChecked =
    raw.on &&
    typeof raw.last_checked === "string" &&
    !Number.isNaN(new Date(raw.last_checked).getTime())
      ? raw.last_checked
      : null;
  const lastCheck =
    lastChecked === null
      ? null
      : (LAST_CHECKS.find((found) => found === raw.last_check) ?? null);
  return {
    on: raw.on,
    offReason: offReason ?? null,
    deb: raw.install === "deb",
    lastChecked,
    lastCheck,
  };
}
