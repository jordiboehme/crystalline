/**
 * What a share would carry, reasoned about rather than drawn.
 *
 * Three questions live here because two components ask them and neither owns
 * the answer: which paths are generated folder listings rather than knowledge
 * somebody wrote, which listings a chosen set of files would drag along with
 * it, and which files to tick for somebody the moment the dialog opens. The
 * list draws them and the dialog posts them, so the rules sit beside both
 * instead of inside either.
 *
 * A module of its own also keeps the list a component module again: a file
 * that exports a component may export nothing else if fast refresh is to work
 * (the repo's lint config enforces it), and these are exactly the exports that
 * would break that.
 */

import type { ShareChange } from "../api/admin";

/**
 * Whether a path is a generated folder listing rather than something somebody
 * wrote, read off its filename the way the engine reads it.
 */
export function isFolderIndex(path: string): boolean {
  return path === "index.md" || path.endsWith("/index.md");
}

/** The folder a domain-relative path lives in; the empty string at the root. */
export function folderOf(path: string): string {
  const at = path.lastIndexOf("/");
  return at < 0 ? "" : path.slice(0, at);
}

/** The changes a reader is actually deciding about: everything but a listing. */
export function substantive(changes: ShareChange[]): ShareChange[] {
  return changes.filter((change) => !isFolderIndex(change.path));
}

/**
 * How many generated folder listings the current selection would carry along.
 *
 * Recomputed rather than counted off the plan, because the plan counts the
 * whole delta's listings and unticking the last file of a folder takes that
 * folder's listing out of the share. It follows what actually goes over the
 * wire, in both of that wire's shapes. A share of everything sends no file
 * list, so every listing in the delta rides - a listing of a folder whose
 * files were all left alone included, since the delta holds it because the
 * folder really did change. A share of some of it sends the chosen paths, and
 * the engine adds the listing of each chosen file's own folder and no others.
 */
export function ridingIndexes(
  changes: ShareChange[],
  selected: ReadonlySet<string>,
): number {
  const real = substantive(changes);
  const listings = changes.filter((change) => isFolderIndex(change.path));
  if (real.every((change) => selected.has(change.path))) {
    return listings.length;
  }
  const folders = new Set(
    real
      .filter((change) => selected.has(change.path))
      .map((change) => folderOf(change.path)),
  );
  return listings.filter(
    (change) => selected.has(change.path) || folders.has(folderOf(change.path)),
  ).length;
}

/** The OKF actor a person writing through this instance is recorded as. */
function actorFor(account: string | null): string | null {
  return account === null || account === "" ? null : `human:${account}`;
}

/** What the dialog opens with ticked, and the sentence that explains it. */
export interface Preselection {
  /** The paths that start ticked. */
  paths: string[];
  /**
   * The line under the list, or null when nothing needs explaining: everything
   * is ticked, which is what a reader expects without being told.
   */
  hint: string | null;
}

/**
 * Which changes to tick for `account` when the dialog opens.
 *
 * **A heuristic default, correctable by every checkbox beside it.** The plan
 * says which actor last WROTE each file, which is not the same as who the
 * knowledge belongs to: it is simply the best guess available for "the work I
 * just did", and on a shared instance it is the difference between sharing
 * your own afternoon and sharing everybody's.
 *
 * The rule has two halves. Where at least one change was last written by this
 * session's own account, exactly those are ticked and the line says how much
 * was left out, so the reader knows there is more rather than assuming they
 * are seeing everything. Where none was - a solo instance, a session that has
 * written nothing, an older server that names no authors at all - everything
 * is ticked, which is what this dialog has always done.
 *
 * A deletion is never ticked by that first half, and it needs no rule of its
 * own to stay out: the file is gone, so nothing on disk attributes it, and an
 * unattributed change is somebody else's until a person says otherwise.
 */
export function preselect(
  changes: ShareChange[],
  account: string | null,
): Preselection {
  const own = actorFor(account);
  const real = substantive(changes);
  const mine =
    own === null
      ? []
      : real.filter((change) => change.lastAuthor === own).map((c) => c.path);
  if (mine.length === 0) {
    return { paths: real.map((change) => change.path), hint: null };
  }
  const others = real.length - mine.length;
  // Everything here is this session's own work, so the ticks are the ticks
  // this dialog always shows and there is nothing to explain. A line saying
  // "0 more from others" would be noise claiming to be information.
  if (others === 0) {
    return { paths: mine, hint: null };
  }
  return {
    paths: mine,
    hint: `Preselected your ${String(mine.length)} ${
      mine.length === 1 ? "change" : "changes"
    } - ${String(others)} more from others left unticked.`,
  };
}
