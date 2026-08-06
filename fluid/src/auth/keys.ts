/**
 * The cache key of the capability probe.
 *
 * Its own module because both the auth provider that owns the probe and the
 * query layer that recovers from an expired session name it, and a shared
 * constant is the only way those two stay talking about the same query.
 */

/** `GET /auth/me`, the one query the whole app hangs off. */
export const ME_QUERY_KEY = ["auth", "me"] as const;

/** The login attempt, so the expired-session recovery can leave it alone. */
export const LOGIN_MUTATION_KEY = ["auth", "login"] as const;
