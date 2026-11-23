/**
 * What the users screen and an account's detail panel share: the roles in
 * order, the words for a sign-in kind, and the shape of a notice.
 */

import type { Role, User } from "../api/model";

/** The roles an account can hold, least privileged first, as the API orders them. */
export const ROLES: readonly Role[] = ["none", "viewer", "editor", "admin"];

/** What the screen is currently saying, and whether it is bad news. */
export interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** How an account signs in, in the words the table and the panel use. */
export function signInWords(kind: User["sign_in"]): string {
  switch (kind) {
    case "password":
      return "Password";
    case "oidc":
      return "SSO";
    case "proxy":
      return "Proxy";
    case "token_only":
      return "Token only";
    default:
      return "Unknown";
  }
}
