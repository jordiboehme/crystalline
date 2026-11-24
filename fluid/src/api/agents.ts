/**
 * The agents of the loops: identities of their own for scheduled reflect and
 * evolve runs on another machine, each with up to two live tokens. Admin
 * only; the server refuses everyone else.
 *
 * A token comes back from the server once, in the answer that made it, and
 * never again. Callers keep it in a plain component field and nowhere else.
 */

import { api, encodeSegment } from "./client";
import type {
  AgentsResponse,
  CreateAgentBody,
  CreatedAgentResponse,
  IssueAgentTokenBody,
  IssuedAgentTokenResponse,
} from "./model";

/** The cache key of the agent list. */
export const AGENTS_KEY = ["agents"] as const;

/** Every agent with its tokens, and whether `auth.mcp` is on. */
export async function fetchAgents(): Promise<AgentsResponse> {
  return api<AgentsResponse>("/agents");
}

/** Make an agent and its first token. */
export async function createAgent(
  body: CreateAgentBody,
): Promise<CreatedAgentResponse> {
  return api<CreatedAgentResponse>("/agents", {
    method: "POST",
    body: JSON.stringify(body),
  });
}

/** Issue another token for `name`. The server allows two live at once. */
export async function issueAgentToken(
  name: string,
  body: IssueAgentTokenBody,
): Promise<IssuedAgentTokenResponse> {
  return api<IssuedAgentTokenResponse>(
    `/agents/${encodeSegment(name)}/tokens`,
    { method: "POST", body: JSON.stringify(body) },
  );
}

/** Revoke one token of `name`. It stops working at once. */
export async function revokeAgentToken(
  name: string,
  id: number,
): Promise<void> {
  await api(
    `/agents/${encodeSegment(name)}/tokens/${encodeSegment(String(id))}`,
    {
      method: "DELETE",
    },
  );
}

/** Remove an agent, its tokens and its drafts. */
export async function removeAgent(name: string): Promise<void> {
  await api(`/agents/${encodeSegment(name)}`, { method: "DELETE" });
}
