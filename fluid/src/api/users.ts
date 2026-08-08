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

/** Replace a password, revoking the account's sessions. */
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
