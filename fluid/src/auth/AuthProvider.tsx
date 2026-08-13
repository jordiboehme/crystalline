/**
 * The bootstrap probe and everything that hangs off it.
 *
 * `GET /auth/me` is the first request the app makes and the only one whose
 * answer changes which app you get. Every outcome it has is handled here and
 * each lands somewhere deliberate:
 *
 * - an account, or the anonymous viewer: the app, with what they may do;
 * - no identity at all, or a 401: the login screen, by way of `RequireAuth`;
 * - a 403, which is a disabled account behind an SSO proxy: a screen that says
 *   so, and no redirect - sending it to a login form it cannot use is the
 *   loop this exists to avoid;
 * - a request that never arrived, or a server error: the server-down banner,
 *   with a way to try again.
 *
 * The probe is also where the CSRF token comes from. The session cookie is
 * `HttpOnly`, so a reloaded tab cannot read its token back out; the server
 * reissues it here, and handing it straight to the client on the way past is
 * what keeps a reload from locking the tab out of every write. Behind an SSO
 * proxy the same call mints the session outright, so the trusted-header mode
 * gets its token from here too: there is one CSRF rule for every identity
 * mode, and no write goes out without a token whichever mode the app is in.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import type { ReactNode } from "react";

import { ApiProblem, api, setCsrfToken } from "../api/client";
import type { LoginResponse, MeResponse, SetupBody } from "../api/model";
import { AccountDisabled } from "../components/AccountDisabled";
import { ServerDown } from "../components/ServerDown";
import { AuthContext } from "./AuthContext";
import type { AuthValue, Capabilities } from "./AuthContext";
import { LOGIN_MUTATION_KEY, ME_QUERY_KEY, SETUP_MUTATION_KEY } from "./keys";

/**
 * Run the probe, and hand the token it carries to the client on the way past.
 *
 * The token is fed here rather than in an effect on purpose: it arrives with
 * the data, and a write that fired between the data landing and an effect
 * running would go out without it.
 */
async function probe(): Promise<MeResponse> {
  const me = await api<MeResponse>("/auth/me");
  setCsrfToken(me.csrf ?? null);
  return me;
}

/** Read what a probe answer means for what this session may do. */
function capabilitiesOf(me: MeResponse | undefined): Capabilities {
  const role = me?.user?.role ?? null;
  const readOnly = me?.read_only ?? false;
  return {
    anonymous: me?.anonymous ?? false,
    readOnly,
    role,
    canWrite: !readOnly && (role === "editor" || role === "admin"),
    canAdminister: role === "admin",
    needsSetup: me?.needs_setup ?? false,
    serverVersion: me?.version ?? "",
  };
}

/**
 * Hold the probe's answer, and render the app or the screen that answer calls
 * for.
 */
export function AuthProvider({ children }: { children: ReactNode }) {
  const queryClient = useQueryClient();
  const me = useQuery({ queryKey: ME_QUERY_KEY, queryFn: probe });

  const loginMutation = useMutation({
    // The key rides the mutation that issues the request, which is the one the
    // expired-session recovery inspects: a refusal here is not an expired
    // session, since nobody has a session yet.
    mutationKey: LOGIN_MUTATION_KEY,
    mutationFn: async ({ name, password }: Credentials) => {
      const session = await api<LoginResponse>("/auth/login", {
        method: "POST",
        body: JSON.stringify({ name, password }),
      });
      setCsrfToken(session.csrf);
      return session;
    },
    onSuccess: async () => {
      // The probe carries more than login answers with (whether the instance
      // is read only, which version it is), so the identity is re-read rather
      // than assembled here out of half a payload.
      await queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
    },
  });
  const { mutateAsync: runLogin } = loginMutation;

  const login = useCallback(
    async (name: string, password: string) => {
      await runLogin({ name, password });
    },
    [runLogin],
  );

  const setupMutation = useMutation({
    // The key rides the mutation that issues the request, which is the one the
    // expired-session recovery inspects: a refusal here is not an expired
    // session, since nobody has a session yet.
    mutationKey: SETUP_MUTATION_KEY,
    // No automatic retry on the one request that creates an account. A 500 or
    // a dropped connection is for the person to decide about, and the default
    // would turn one submit into a pair of POSTs the server has to race
    // against itself.
    retry: 0,
    mutationFn: async ({ name, password, token }: FirstAdmin) => {
      // The token is omitted rather than sent empty: an instance that never
      // printed one treats "no token configured" as a closed path, and a
      // caller that has none has nothing to say about it.
      const body: SetupBody = { name, password, ...(token ? { token } : {}) };
      const session = await api<LoginResponse>("/auth/setup", {
        method: "POST",
        body: JSON.stringify(body),
      });
      setCsrfToken(session.csrf);
      return session;
    },
    onSuccess: async () => {
      // Same reasoning as login: the identity is re-read from the probe, which
      // is also what flips `needs_setup` false and takes the wizard away.
      await queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
    },
    onError: async (error: Error) => {
      // A 410 says the setup slot closed while this form was open, which makes
      // the probe's `needs_setup` a stale answer. Re-reading it is what turns
      // the wizard back into the login form the person now needs; every other
      // refusal leaves the instance exactly as the probe described it.
      if (error instanceof ApiProblem && error.status === 410) {
        await queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
      }
    },
  });
  const { mutateAsync: runSetup } = setupMutation;

  const setup = useCallback(
    async (name: string, password: string, token?: string) => {
      await runSetup({ name, password, token });
    },
    [runSetup],
  );

  const logout = useCallback(async () => {
    try {
      await api("/auth/logout", { method: "POST" });
    } finally {
      // Whatever the server said, this browser is done with that session: the
      // token goes, and so does every cached answer it was allowed to see, so
      // the next person at this keyboard starts from the server rather than
      // from someone else's cache.
      setCsrfToken(null);
      queryClient.removeQueries({
        predicate: (query) => query.queryKey[0] !== ME_QUERY_KEY[0],
      });
      // The probe is refetched rather than dropped, because its answer is what
      // decides where the app goes next: the login screen on an instance that
      // requires an account, the anonymous viewer's app on one that does not.
      await queryClient.refetchQueries({ queryKey: ME_QUERY_KEY });
    }
  }, [queryClient]);

  const problem = me.error;
  // A 401 says there is no identity, and it says so about now: a probe that
  // succeeded before the session expired must not keep the app looking signed
  // in on the strength of a stale answer.
  const refused = problem instanceof ApiProblem && problem.status === 401;
  const identity = refused ? undefined : me.data;

  const value = useMemo<AuthValue>(
    () => ({
      user: identity?.user ?? null,
      capabilities: capabilitiesOf(identity),
      login,
      setup,
      logout,
    }),
    [identity, login, setup, logout],
  );

  if (me.isPending) {
    return <Booting />;
  }

  // A failure replaces the app only when there is nothing to replace. Once the
  // app is up, a probe that fails in the background (the window regained focus
  // while the server was restarting) leaves it up rather than throwing away
  // what someone was reading; the screens' own queries say what they hit. A
  // 401 is not a failure to show at all: it means "log in", which the gate
  // below turns into a redirect.
  if (problem && !refused && me.data === undefined) {
    if (problem instanceof ApiProblem && problem.status === 403) {
      return <AccountDisabled detail={problem.detail} />;
    }
    return (
      <ServerDown
        detail={
          problem instanceof ApiProblem ? problem.detail : problem.message
        }
        onRetry={() => void me.refetch()}
      />
    );
  }

  return <AuthContext value={value}>{children}</AuthContext>;
}

/** What login takes. A single object, so the two strings cannot swap places. */
interface Credentials {
  name: string;
  password: string;
}

/** What first-run setup takes: credentials, plus a token when one was asked for. */
interface FirstAdmin extends Credentials {
  token?: string | undefined;
}

/**
 * The moment before the probe answers.
 *
 * Marked `aria-busy` rather than given a live region: this is the whole
 * document loading, not an update to announce, and a `status` role here would
 * also be the first thing a test looking for a notice would find.
 */
function Booting() {
  return (
    <div
      aria-busy="true"
      className="flex min-h-screen items-center justify-center text-sm text-slate-500 dark:text-slate-400"
    >
      Loading Fluid
    </div>
  );
}
