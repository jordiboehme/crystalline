/**
 * What the app knows about who is using it, and the hook that reads it.
 *
 * The provider that fills this in lives next door in `AuthProvider.tsx`: a
 * module that exports a component may export nothing else if fast refresh is
 * to work (the repo's lint config enforces it), and the hook is what screens
 * import, so the hook is what stays here.
 */

import { createContext, use } from "react";

import type { Role, User } from "../api/model";

/**
 * What this session may do, resolved once so no screen has to re-derive it
 * from the raw probe.
 */
export interface Capabilities {
  /** Served without an account, because the instance allows anonymous reading. */
  anonymous: boolean;
  /** The instance refuses content mutations, whoever is asking. */
  readOnly: boolean;
  /** The account's role, or null for the anonymous viewer. */
  role: Role | null;
  /**
   * Whether this session may change content: an editor or an admin, on an
   * instance that is not read only. The anonymous viewer never may.
   */
  canWrite: boolean;
  /** Whether this session may administer accounts. */
  canAdminister: boolean;
  /** The version of the server that answered the probe. */
  serverVersion: string;
}

/** The auth surface every screen sees. */
export interface AuthValue {
  /** The signed-in account, or null when there is none. */
  user: User | null;
  /** What this session may do. */
  capabilities: Capabilities;
  /** Exchange credentials for a session. Rejects with the `ApiProblem` the server sent. */
  login: (name: string, password: string) => Promise<void>;
  /** End the session and drop everything it was allowed to see. */
  logout: () => Promise<void>;
}

export const AuthContext = createContext<AuthValue | null>(null);

/** Who is using the app. Throws outside an `AuthProvider`, which is a wiring bug. */
export function useAuth(): AuthValue {
  const value = use(AuthContext);
  if (!value) {
    throw new Error("useAuth was called outside an AuthProvider");
  }
  return value;
}
