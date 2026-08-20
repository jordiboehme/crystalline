/**
 * The engram write surface: create, full-document save, retire, move, hard
 * delete and the dry-run validation. Editor role everywhere; the server
 * decides on every request and its refusals are surfaced verbatim.
 *
 * The If-Match discipline: the detail read's `checksum` is the version being
 * replaced, quoted here per RFC 9110. A 412 carries the version the server
 * holds now as problem extensions, which `conflictOf` turns into something a
 * merge view can hold - the caller's own text is never touched by this module.
 */

import { ApiProblem, api, encodeSegment, engramPath } from "./client";
import type { EngramDetail } from "./engram";
import { readEngramDetail } from "./engram";
import { asObject, asString, asStrings } from "./json";
import type {
  CreateEngramBody,
  MoveBody,
  RetireBody,
  ValidateBody,
  ValidateResponse,
} from "./model";

/** What the server holds now, out of a 412's problem extensions. */
export interface SaveConflict {
  /** The current version's checksum, unquoted: the next If-Match token. */
  currentChecksum: string;
  /** The full markdown the server holds now. */
  currentContent: string;
  /** The refusal, in the server's words. */
  detail: string;
}

/** Read a failure as a save conflict, or null for any other failure. */
export function conflictOf(error: unknown): SaveConflict | null {
  if (!(error instanceof ApiProblem) || error.status !== 412) {
    return null;
  }
  const etag = asString(error.extensions.current_etag);
  const content = error.extensions.current_content;
  if (etag === null || typeof content !== "string") {
    return null;
  }
  return {
    currentChecksum: etag.replace(/^"|"$/g, ""),
    currentContent: content,
    detail: error.detail,
  };
}

/** The quoted strong validator an If-Match carries. */
function ifMatch(checksum: string): Record<string, string> {
  return { "If-Match": `"${checksum}"` };
}

/** Create an engram; the answer is the detail read of what landed. */
export async function createEngram(
  domain: string,
  body: CreateEngramBody,
): Promise<EngramDetail> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/engrams`,
    {
      method: "POST",
      body: JSON.stringify(body),
    },
  );
  return readEngramDetail(payload, domain, "");
}

/**
 * Save the complete file text, guarded by the checksum of the version it is
 * based on. The answer is the detail read of what landed - AT ITS PERMALINK
 * AFTER THE WRITE, so a save that renamed the engram through its frontmatter
 * comes back with the new address and the caller follows it.
 */
export async function saveEngram(
  domain: string,
  permalink: string,
  content: string,
  checksum: string,
): Promise<EngramDetail> {
  const payload = await api<unknown>(engramPath(domain, permalink), {
    method: "PUT",
    headers: ifMatch(checksum),
    body: JSON.stringify({ content }),
  });
  return readEngramDetail(payload, domain, permalink);
}

/** Hard delete, guarded the same way a save is. */
export async function deleteEngram(
  domain: string,
  permalink: string,
  checksum: string,
): Promise<void> {
  await api(engramPath(domain, permalink), {
    method: "DELETE",
    headers: ifMatch(checksum),
  });
}

/** What a retirement settled on. */
export interface RetireReceipt {
  permalink: string;
  status: string;
  successor: string | null;
}

/** Guided retirement; no If-Match, matching the endpoint's contract. */
export async function retireEngram(
  domain: string,
  body: RetireBody,
): Promise<RetireReceipt> {
  const payload = await api<unknown>(
    `/domains/${encodeSegment(domain)}/retire`,
    {
      method: "POST",
      body: JSON.stringify(body),
    },
  );
  const record = asObject(payload);
  return {
    permalink: asString(record?.permalink) ?? body.permalink,
    status: asString(record?.status) ?? body.status,
    successor: asString(record?.successor),
  };
}

/** Where a move landed, as an address a router can follow. */
export interface MoveReceipt {
  domain: string;
  permalink: string;
  crossDomain: boolean;
  linksRewritten: number;
  /**
   * What the move could not carry with it, in the engine's own words: an
   * attachment the engram still references that stayed in the old domain. The
   * move happened either way, so these are notices rather than failures, and a
   * server that sends none - an older one, or one with nothing to say - leaves
   * this empty.
   */
  attachmentWarnings: string[];
}

/**
 * Move an engram. The receipt names the destination as a file path; the
 * permalink is that path without its `.md` suffix, which is the rule the
 * engine derives permalinks by.
 */
export async function moveEngram(
  domain: string,
  body: MoveBody,
): Promise<MoveReceipt> {
  const payload = await api<unknown>(`/domains/${encodeSegment(domain)}/move`, {
    method: "POST",
    body: JSON.stringify(body),
  });
  const record = asObject(payload);
  const to = asObject(record?.to);
  const path = asString(to?.path) ?? body.destination;
  return {
    domain: asString(to?.domain) ?? body.destination_domain ?? domain,
    permalink: path.replace(/\.md$/, ""),
    crossDomain: record?.cross_domain === true,
    linksRewritten:
      typeof record?.links_rewritten === "number" ? record.links_rewritten : 0,
    attachmentWarnings: asStrings(record?.attachment_warnings),
  };
}

/** The dry run: the findings a save would raise, without writing. */
export async function validateDocument(
  body: ValidateBody,
): Promise<ValidateResponse> {
  return api<ValidateResponse>("/validate", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/**
 * The cache key of one dry run: the content decides the answer as much as
 * the address does, so it is part of the key rather than a value the
 * `queryFn` closes over - two different buffers must never share one
 * cached report.
 */
export function validateKey(
  domain: string,
  path: string | null,
  content: string,
): readonly unknown[] {
  return ["validate", domain, path, content];
}
