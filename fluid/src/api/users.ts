/**
 * The user admin surface: list, create, edit, reset, remove. Admin only; the
 * server refuses everyone else, and the screen never renders for them.
 */

import { api, encodeSegment } from "./client";
import type {
  BulkBody,
  BulkResult,
  CreateUserBody,
  GithubUnlinkResponse,
  KindFilter,
  MyProfileBody,
  PatchUserBody,
  Role,
  SortColumn,
  SortDir,
  StateFilter,
  User,
  UserResponse,
  UsersResponse,
} from "./model";

/** The cache key of the account list. */
export const USERS_QUERY_KEY = ["users"] as const;

/**
 * One page of the account list, as the users table asks for it. Every field
 * left `null` is a filter not applied; the dates are days (`YYYY-MM-DD`) and
 * go out as midnight UTC.
 */
export interface UsersPageRequest {
  q: string;
  role: Role | null;
  state: StateFilter | null;
  kind: KindFilter | null;
  github: boolean | null;
  seenAfter: string | null;
  seenBefore: string | null;
  sort: SortColumn;
  dir: SortDir;
  page: number;
  size: number;
}

/** One page of accounts and how many match in all. */
export interface UsersPage {
  users: User[];
  total: number;
}

/** The cache key of one page, under the list's own key so a refresh reaches both. */
export function usersPageKey(request: UsersPageRequest) {
  return [...USERS_QUERY_KEY, "page", request] as const;
}

/** The cache key of one account, as its detail panel reads it. */
export function userKey(name: string) {
  return [...USERS_QUERY_KEY, "one", name] as const;
}

/** One page of accounts, searched, filtered and sorted. */
export async function fetchUsersPage(
  request: UsersPageRequest,
): Promise<UsersPage> {
  const params = new URLSearchParams();
  if (request.q.trim() !== "") params.set("q", request.q.trim());
  if (request.role) params.set("role", request.role);
  if (request.state) params.set("state", request.state);
  if (request.kind) params.set("kind", request.kind);
  if (request.github !== null) params.set("github", String(request.github));
  if (request.seenAfter) {
    params.set("seen_after", `${request.seenAfter}T00:00:00Z`);
  }
  if (request.seenBefore) {
    params.set("seen_before", `${request.seenBefore}T00:00:00Z`);
  }
  params.set("sort", request.sort);
  params.set("dir", request.dir);
  params.set("page", String(request.page));
  params.set("limit", String(request.size));
  const listing = await api<UsersResponse>(`/users?${params.toString()}`);
  return { users: listing.users, total: listing.total };
}

/**
 * One account by its login name, or `null` when there is none. There is no
 * route for a single account, so this searches by the name and keeps the exact
 * match: a search for `eddy` also finds `eddy2`.
 */
export async function fetchUser(name: string): Promise<User | null> {
  const params = new URLSearchParams({ q: name });
  const listing = await api<UsersResponse>(`/users?${params.toString()}`);
  const wanted = name.toLowerCase();
  return (
    listing.users.find((account) => account.name.toLowerCase() === wanted) ??
    null
  );
}

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

/**
 * Change your own display name. Blank resets it to your login name; the server
 * trims it and refuses one that is too long or holds an invisible character.
 */
export async function patchMyProfile(body: MyProfileBody): Promise<User> {
  const patched = await api<UserResponse>("/me", {
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

/**
 * One change for many accounts at once. The answer has one entry per account,
 * in the order sent; a refused one carries the server's sentence.
 */
export async function bulkUsers(body: BulkBody): Promise<BulkResult[]> {
  const answer = await api<{ results: BulkResult[] }>("/users/bulk", {
    method: "POST",
    body: JSON.stringify(body),
  });
  return answer.results;
}

/**
 * Remove another account's GitHub link. Answers the warning to pass on when
 * the link is gone here but GitHub did not revoke the token, else `null`.
 */
export async function unlinkUserGithub(name: string): Promise<string | null> {
  const answer = await api<GithubUnlinkResponse>(
    `/users/${encodeSegment(name)}/github-link`,
    { method: "DELETE" },
  );
  return answer.warning ?? null;
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
