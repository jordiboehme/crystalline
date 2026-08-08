/**
 * The editor's safety net: the buffer snapshots to browser storage until a
 * successful save, so a crash, a closed tab or an in-app navigation loses at
 * most a second of typing. Per user and per engram, because two accounts on
 * one browser are two people, and offering one the other's half-thought would
 * be worse than offering nothing.
 *
 * Storage failures are swallowed on write and read: a full or blocked
 * localStorage degrades to "no draft", never to a broken editor.
 */

export interface Draft {
  /** The full buffer text as last seen. */
  content: string;
  /** The server checksum the session was based on when this was written. */
  baseChecksum: string;
  /** When the snapshot was taken, RFC 3339. */
  savedAt: string;
}

/** How long a pause in typing writes a snapshot, ms. */
export const DRAFT_DEBOUNCE_MS = 1000;

/** The storage key of one person's draft of one engram. */
export function draftKey(
  user: string,
  domain: string,
  permalink: string,
): string {
  return `fluid.draft.${user}.${domain}/${permalink}`;
}

/** The stored draft, or null when there is none or it will not parse. */
export function readDraft(
  user: string,
  domain: string,
  permalink: string,
): Draft | null {
  let raw: string | null;
  try {
    raw = localStorage.getItem(draftKey(user, domain, permalink));
  } catch {
    return null;
  }
  if (raw === null) {
    return null;
  }
  try {
    const parsed = JSON.parse(raw) as Partial<Draft>;
    if (
      typeof parsed.content !== "string" ||
      typeof parsed.baseChecksum !== "string"
    ) {
      return null;
    }
    return {
      content: parsed.content,
      baseChecksum: parsed.baseChecksum,
      savedAt: typeof parsed.savedAt === "string" ? parsed.savedAt : "",
    };
  } catch {
    return null;
  }
}

/** Write a snapshot; a refused storage write is a lost snapshot, not an error. */
export function writeDraft(
  user: string,
  domain: string,
  permalink: string,
  draft: Draft,
): void {
  try {
    localStorage.setItem(
      draftKey(user, domain, permalink),
      JSON.stringify(draft),
    );
  } catch {
    // Full or blocked storage: the unload prompt still guards the work.
  }
}

/** Drop the snapshot, after a successful save or an explicit discard. */
export function clearDraft(
  user: string,
  domain: string,
  permalink: string,
): void {
  try {
    localStorage.removeItem(draftKey(user, domain, permalink));
  } catch {
    // Nothing to do: an unremovable draft re-offers itself, harmlessly.
  }
}
