/**
 * The caller's own MCP tokens: what an agent authenticates the daemon with
 * when it acts as this account, from its own registration's `Authorization`
 * header. Every signed-in account has this surface, viewers included - an
 * agent acts as the account that issued its token, so a viewer's agent is
 * read-only by construction.
 *
 * The listing never carries a secret: only its hash is stored, so `label`,
 * `created_at` and `last_used` are the whole of what a row can say. Issuing
 * and rotating are the one exception, and only for the single reply that
 * makes the token exist - it is never readable again after that.
 */

import { api, encodeSegment } from "./client";
import type { IssueMcpTokenBody, IssuedMcpToken, McpTokenInfo } from "./model";

/** The cache key of the caller's own MCP token list. */
export const MCP_TOKENS_KEY = ["me-mcp-tokens"] as const;

/** Every token this account has issued, newest first. Carries no secrets. */
export async function fetchMcpTokens(): Promise<McpTokenInfo[]> {
  return api<McpTokenInfo[]>("/me/mcp-tokens");
}

/** Issue a new token under `label`. The reply is the only place it is readable. */
export async function issueMcpToken(label: string): Promise<IssuedMcpToken> {
  const body: IssueMcpTokenBody = { label };
  return api<IssuedMcpToken>("/me/mcp-tokens", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/**
 * Replace one token with a fresh one, same id gone and a new one issued: the
 * old secret stops working and the new one exists in the same step, so an
 * agent whose token may have leaked is never left holding none at all.
 */
export async function rotateMcpToken(id: number): Promise<IssuedMcpToken> {
  return api<IssuedMcpToken>(
    `/me/mcp-tokens/${encodeSegment(String(id))}/rotate`,
    {
      method: "POST",
    },
  );
}

/** Revoke one token. It stops resolving at once. */
export async function revokeMcpToken(id: number): Promise<void> {
  await api(`/me/mcp-tokens/${encodeSegment(String(id))}`, {
    method: "DELETE",
  });
}
