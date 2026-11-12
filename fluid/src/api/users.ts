/**
 * The user admin surface: list, create, edit, reset, remove. Admin only; the
 * server refuses everyone else, and the screen never renders for them.
 */

import { api, encodeSegment } from "./client";
import type {
  CreateUserBody,
  PatchUserBody,
  User,
  UserResponse,
  UsersResponse,
} from "./model";

/** The cache key of the account list. */
export const USERS_QUERY_KEY = ["users"] as const;

/** Every account, by name. */
export async function fetchUsers(): Promise<User[]> {
  const listing = await api<UsersResponse>("/users");
  return listing.users;
}

/** Add an account. */
export async function createUser(body: CreateUserBody): Promise<User> {
  const created = await api<UserResponse>("/users", {
    method: "POST",
    body: JSON.stringify(body),
  });
  return created.user;
}

/** Change a role, a display name or the disabled flag. */
export async function patchUser(
  name: string,
  body: PatchUserBody,
): Promise<User> {
  const patched = await api<UserResponse>(`/users/${encodeSegment(name)}`, {
    method: "PATCH",
    body: JSON.stringify(body),
  });
  return patched.user;
}

/** Replace a password, revoking the account's sessions, MCP tokens and OAuth grants. */
export async function resetPassword(
  name: string,
  password: string,
): Promise<User> {
  const reset = await api<UserResponse>(
    `/users/${encodeSegment(name)}/password`,
    {
      method: "POST",
      body: JSON.stringify({ password }),
    },
  );
  return reset.user;
}

/** Delete an account and every session it holds. */
export async function deleteUser(name: string): Promise<void> {
  await api(`/users/${encodeSegment(name)}`, { method: "DELETE" });
}

/** What ending a removed account's drafts did in one domain. */
export interface DiscardedDrafts {
  domain: string;
  discarded: number;
  /** Paths left because they are open in a live editor. */
  left: string[];
  /** Set when another instance hosts this domain. */
  not_hosted?: boolean;
  /** What to do when another instance hosts this domain. */
  where?: string;
  /** Why this domain stopped, after what `discarded` says went. */
  error?: string;
}

/**
 * End every draft a removed account left under its name, so the name can be used
 * again. Refused for a name with an account.
 */
export async function discardUserDrafts(
  name: string,
): Promise<{ domains: DiscardedDrafts[]; error?: string }> {
  const report = await api<{ domains?: DiscardedDrafts[]; error?: string }>(
    `/users/${encodeSegment(name)}/drafts`,
    { method: "DELETE" },
  );
  return {
    domains: report.domains ?? [],
    ...(report.error ? { error: report.error } : {}),
  };
}
