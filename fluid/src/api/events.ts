/**
 * The three frames `GET /api/v1/events` sends, read tolerantly like every
 * payload in this folder. The generated `EngramChanged`, `DomainChanged` and
 * `ChangeKind` in `types.ts` document the wire; the readers here are what
 * the app trusts.
 */

import { asObject, asString } from "./json";

export type ChangeKind = "added" | "modified" | "deleted" | "moved";

const KINDS: ReadonlySet<string> = new Set([
  "added",
  "modified",
  "deleted",
  "moved",
]);

export interface EngramChange {
  domain: string;
  /** After the change; for `deleted`, the permalink that went. */
  permalink: string;
  path: string;
  kind: ChangeKind;
  /** `moved` only. */
  from: { path: string; permalink: string } | null;
  checksum: string | null;
  /** Who changed it, as a label; null for a change found on disk. */
  actor: string | null;
  /** Set when the change is somebody's draft: the owner's own sessions only. */
  draftOf: string | null;
}

export interface DomainChange {
  domain: string;
  actor: string | null;
}

export type ChangeEvent =
  | { event: "engram"; change: EngramChange }
  | { event: "domain"; change: DomainChange }
  | { event: "reset" };

export function readEngramChange(value: unknown): EngramChange | null {
  const record = asObject(value);
  const domain = asString(record?.domain);
  const permalink = asString(record?.permalink);
  const path = asString(record?.path);
  const kind = asString(record?.kind);
  if (!domain || !permalink || !path || !kind || !KINDS.has(kind)) {
    return null;
  }
  const from = asObject(record?.from);
  const fromPath = asString(from?.path);
  const fromPermalink = asString(from?.permalink);
  return {
    domain,
    permalink,
    path,
    kind: kind as ChangeKind,
    from:
      fromPath && fromPermalink
        ? { path: fromPath, permalink: fromPermalink }
        : null,
    checksum: asString(record?.checksum),
    actor: asString(record?.actor),
    draftOf: asString(record?.draft_of),
  };
}

export function readDomainChange(value: unknown): DomainChange | null {
  const record = asObject(value);
  const domain = asString(record?.domain);
  return domain ? { domain, actor: asString(record?.actor) } : null;
}

/**
 * One frame, from its `event` name and `data` text. A frame that does not
 * parse is dropped with a warning in development and never thrown: a bad
 * frame is one missed refetch, and the next window focus covers it.
 */
export function parseFrame(event: string, data: string): ChangeEvent | null {
  if (event === "reset") {
    return { event: "reset" };
  }
  let value: unknown;
  try {
    value = JSON.parse(data);
  } catch {
    warn(event, data);
    return null;
  }
  if (event === "engram") {
    const change = readEngramChange(value);
    if (change) return { event, change };
  } else if (event === "domain") {
    const change = readDomainChange(value);
    if (change) return { event, change };
  }
  warn(event, data);
  return null;
}

function warn(event: string, data: string): void {
  if (import.meta.env.DEV) {
    console.warn(
      `dropped a change frame the app cannot read: ${event} ${data}`,
    );
  }
}
