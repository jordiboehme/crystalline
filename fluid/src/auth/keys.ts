/**
 * The cache key of the capability probe, and the mutation keys that exempt an
 * auth attempt from the expired-session recovery.
 *
 * Its own module because both the auth provider that owns each mutation and
 * the query layer that recovers from an expired session (`../query/client.ts`)
 * name them, and a shared constant is the only way those two stay talking
 * about the same query.
 *
 * Worth knowing where a mutation key like these has to be spelled: the
 * recovery inspects the mutation that FAILED, so the key belongs on the
 * mutation that issues the request - the provider's, since that is the one
 * that actually talks to the server. A screen's wrapper mutation around it
 * carries the key too, because it fails whenever the request underneath it
 * does, but carrying it there ALONE leaves the recovery blind to the mutation
 * that matters: the wrapper never fails on its own, so the key has to ride
 * both, not just the one that is easier to reach from the screen.
 */

/** `GET /auth/me`, the one query the whole app hangs off. */
export const ME_QUERY_KEY = ["auth", "me"] as const;

/** The login attempt, so the expired-session recovery can leave it alone. */
export const LOGIN_MUTATION_KEY = ["auth", "login"] as const;

/**
 * The first-run setup attempt. Under the same first segment as the other two,
 * which is what the expired-session recovery reads: creating the first admin
 * is no more an expired session than a refused login is.
 */
export const SETUP_MUTATION_KEY = ["auth", "setup"] as const;
