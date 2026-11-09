/**
 * Kept texts: what a co-editing room held when its last save could not land,
 * and thread comments a refused write could not land. The server keeps them
 * in its state folder until a person copies them back and drops them.
 *
 * The server decides who sees what. A room's text goes to the people who may
 * write its source, a draft's text to the draft's owner only, and a comment
 * to its sender or to the Reflection's editors. A caller with no right at all
 * gets 403, and a 401 or 404 says the same thing from this side: nothing to
 * show. So {@link keptQuery} never retries, and the screens read every refusal
 * as an empty list rather than as an alert.
 */

import { queryOptions } from "@tanstack/react-query";

import { API_BASE, ApiProblem, api, encodeSegment } from "./client";
import type {
  KeptTextEntry,
  KeptTextResponse,
  KeptTextsResponse,
} from "./model";

export type { KeptTextEntry, KeptTextResponse } from "./model";

/** The cache key of one domain's kept texts. */
export function keptKey(domain: string) {
  return ["kept", domain] as const;
}

/** The cache key of one kept text with its text. */
export function keptTextKey(domain: string, id: string) {
  return ["kept", domain, id] as const;
}

function keptPath(domain: string, id?: string): string {
  const base = `/domains/${encodeSegment(domain)}/kept`;
  return id === undefined ? base : `${base}/${encodeSegment(id)}`;
}

/** The kept texts of `domain` this caller may handle, oldest first. */
export async function fetchKeptTexts(domain: string): Promise<KeptTextEntry[]> {
  const answer = await api<KeptTextsResponse>(keptPath(domain));
  return answer.kept;
}

/** One kept text with its text. */
export async function fetchKeptText(
  domain: string,
  id: string,
): Promise<KeptTextResponse> {
  return api<KeptTextResponse>(keptPath(domain, id));
}

/**
 * Where one kept copy is downloaded from, as its bytes.
 *
 * An address rather than a fetch: an anchor performs the cookie-authenticated
 * GET and the browser saves the file itself. The server always answers it as
 * a download named for the file, never rendered in place. This is the only
 * way a draft file kept as its bytes comes back to a person who cannot reach
 * the server's disk.
 */
export function keptDownloadUrl(domain: string, id: string): string {
  return `${API_BASE}${keptPath(domain, id)}/file`;
}

/** Delete one kept text. Only ever on a person's request. */
export async function dropKeptText(domain: string, id: string): Promise<void> {
  await api(keptPath(domain, id), { method: "DELETE" });
}

/**
 * Whether a failure only says "nothing for you here": signed out (401), no
 * right on this domain (403), or no such domain or text (404).
 */
export function isNoKeptRight(error: Error | null): boolean {
  return (
    error instanceof ApiProblem &&
    (error.status === 401 || error.status === 403 || error.status === 404)
  );
}

/** The listing of `domain`, shared by the editors and the settings screen. */
export function keptQuery(domain: string, enabled = true) {
  return queryOptions({
    queryKey: keptKey(domain),
    queryFn: () => fetchKeptTexts(domain),
    retry: false,
    enabled,
  });
}

/**
 * The newest kept text an editor may offer for one page: a room's text of
 * the same kind at the same path, from the domain's own text or from the
 * caller's own draft as `draft` says. A kept comment is never page text.
 */
export function keptForPage(
  entries: readonly KeptTextEntry[] | undefined,
  kind: "engram" | "reflection",
  path: string,
  draft: boolean,
): KeptTextEntry | null {
  if (entries === undefined) {
    return null;
  }
  let newest: KeptTextEntry | null = null;
  for (const entry of entries) {
    if (entry.kind === kind && entry.path === path && entry.draft === draft) {
      // The listing is oldest first, so the last match is the newest.
      newest = entry;
    }
  }
  return newest;
}

/** The words for what an entry is, as a row and a dialog title say it. */
export function keptKindLabel(kind: string): string {
  switch (kind) {
    case "comment":
      return "Comment";
    case "reflection":
      return "Page text";
    case "engram":
      return "Engram text";
    default:
      return "Kept text";
  }
}

/** Who the entry came from, when the server knows; null when it does not. */
export function keptFrom(entry: KeptTextEntry): string | null {
  if (entry.sender != null && entry.sender !== "") {
    return `From ${entry.sender}`;
  }
  return entry.draft ? "From your draft" : null;
}

/** What the editors' note says about one entry. */
export function keptNoticeSentence(entry: KeptTextEntry): string {
  if (entry.sender != null && entry.sender !== "") {
    return `The server kept unsaved text for this page. From ${entry.sender}.`;
  }
  return entry.draft
    ? "The server kept unsaved text from your draft."
    : "The server kept unsaved text for this page.";
}
