/**
 * Account search for the people picker: login and display name by prefix,
 * at most ten results, for a caller who manages the named domain (an admin
 * may leave the domain out). A query under two characters answers nothing,
 * so the picker does not ask.
 */

import { api } from "./client";
import type { AccountSearchResponse } from "./model";

/** The shortest query the server searches for. */
export const SEARCH_MIN_CHARS = 2;

/** The cache key of one search. */
export function accountSearchKey(
  domain: string,
  q: string,
): readonly unknown[] {
  return ["account-search", domain, q];
}

/** Find accounts by login or display name, each marked when already a member of `domain`. */
export async function searchAccounts(
  q: string,
  domain: string,
): Promise<AccountSearchResponse> {
  const params = new URLSearchParams({ q, domain });
  return api<AccountSearchResponse>(`/accounts/search?${params.toString()}`);
}
