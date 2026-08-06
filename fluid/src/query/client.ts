/**
 * The data layer's two policies, which are what make it behave against this
 * API.
 *
 * One: a failure the server decided is not retried. The default is `retry: 1`,
 * which is right for a dropped connection and wrong for a 401 or a 403 - those
 * are answers, not accidents, and asking again only doubles the wait before
 * the screen that says so.
 *
 * Two: a 401 arriving mid-session means the session ended while the app was
 * open, so the identity the app is holding is stale. Rather than guess, the
 * capability probe is invalidated and asked again; whatever it answers then
 * drives the redirect, exactly as it does at startup.
 */

import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";

import { ApiProblem } from "../api/client";
import { ME_QUERY_KEY } from "../auth/keys";

/** How often a failed request is tried again before the screen hears about it. */
const RETRIES = 1;

/** Whether this failure is the server's decision rather than a mishap. */
function isDecided(error: unknown): boolean {
  return (
    error instanceof ApiProblem && error.status >= 400 && error.status < 500
  );
}

/** Whether an expired session is what this failure means. */
function isExpiredSession(error: unknown): boolean {
  return error instanceof ApiProblem && error.status === 401;
}

/**
 * Whether this key belongs to the auth surface itself.
 *
 * The probe and the login attempt are exempt from the re-probe, and the probe
 * has to be: recovering from its own 401 by refetching it would spin.
 */
function isAuthKey(key: readonly unknown[] | undefined): boolean {
  return key?.[0] === ME_QUERY_KEY[0];
}

/** Build a client. One per mount, so no cache outlives the app that filled it. */
export function createQueryClient(): QueryClient {
  const client: QueryClient = new QueryClient({
    defaultOptions: {
      queries: {
        retry: (failureCount, error) =>
          !isDecided(error) && failureCount < RETRIES,
        refetchOnWindowFocus: true,
      },
      mutations: {
        retry: (failureCount, error) =>
          !isDecided(error) && failureCount < RETRIES,
      },
    },
    queryCache: new QueryCache({
      onError: (error, query) => {
        if (isExpiredSession(error) && !isAuthKey(query.queryKey)) {
          void client.invalidateQueries({ queryKey: ME_QUERY_KEY });
        }
      },
    }),
    mutationCache: new MutationCache({
      onError: (error, _variables, _context, mutation) => {
        if (
          isExpiredSession(error) &&
          !isAuthKey(mutation.options.mutationKey)
        ) {
          void client.invalidateQueries({ queryKey: ME_QUERY_KEY });
        }
      },
    }),
  });
  return client;
}
