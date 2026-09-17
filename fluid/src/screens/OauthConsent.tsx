/**
 * The consent screen: the one page in this product where a single click hands
 * a client everything an account can do.
 *
 * A client that wants MCP access sends the browser here with `?request=<id>`,
 * an opaque id naming a pending authorization the server already validated -
 * the client, its redirect address and its PKCE challenge were all checked
 * before this page ever loads, so there is nothing left to ask about the
 * protocol. What this screen shows instead is exactly what a person deciding
 * whether to trust something needs: which client is asking, the address the
 * answer is sent to (with a warning when that address is this machine rather
 * than a service on the web), and which account is about to be granted.
 *
 * `RequireAuth` already carries this address to the login screen for whoever
 * is not signed in yet, and the login screen's single-sign-on button carries
 * it onward as `return_to`, so a provider sign-in lands back here rather than
 * on the home screen.
 *
 * A 404 on the read - the id is unknown, the request already expired, or it
 * was already decided - is one answer for all three, so nothing here tries to
 * tell them apart: it says to start again from the client. The Allow and Deny
 * buttons never fetch in place: what they return is a full redirect address,
 * appended with the client's code or its refusal, and only a whole-page
 * navigation (through {@link navigateTo}) can carry the browser back to a
 * client waiting somewhere else entirely.
 */

import { useMutation, useQuery } from "@tanstack/react-query";
import { useSearchParams } from "react-router";

import { ApiProblem, navigateTo, problemDetail } from "../api/client";
import { decideAuthorization, fetchAuthorization } from "../api/oauth";
import { BUTTON } from "../components/primitives";

const MUTED = "text-sm text-slate-500 dark:text-slate-400";

const PROBLEM = `rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800
  dark:border-red-900 dark:bg-red-950 dark:text-red-200`;

/** A 404 here is one fact for three causes: unknown, expired, or already decided. */
function isGone(error: Error): boolean {
  return error instanceof ApiProblem && error.status === 404;
}

export default function OauthConsent() {
  const [searchParams] = useSearchParams();
  const requestId = searchParams.get("request");

  const authorization = useQuery({
    queryKey: ["oauth-authorization", requestId],
    queryFn: () => fetchAuthorization(requestId ?? ""),
    enabled: requestId !== null,
    retry: false,
  });

  const decide = useMutation({
    // Never retried: a decision that reached the server and committed, then
    // failed on the way back, must not be sent a second time - the record is
    // single use, so a retry would only ever answer 404 for a request the
    // first attempt already spent.
    retry: false,
    mutationFn: (decision: "allow" | "deny") =>
      decideAuthorization(requestId ?? "", decision),
    onSuccess: (response) => {
      navigateTo(response.location);
    },
  });

  if (requestId === null) {
    return <Expired />;
  }
  if (authorization.isError && isGone(authorization.error)) {
    return <Expired />;
  }
  if (decide.isError && isGone(decide.error)) {
    return <Expired />;
  }
  if (authorization.isPending) {
    return <p className={MUTED}>Reading the request</p>;
  }
  if (authorization.isError) {
    return (
      <p role="alert" className={PROBLEM}>
        {problemDetail(authorization.error)}
      </p>
    );
  }

  const view = authorization.data;

  return (
    <div className="mx-auto flex w-full max-w-md flex-col gap-6">
      <header>
        <h1 className="text-display">Connect a client</h1>
        <p className={`mt-1 ${MUTED}`}>
          Allowing this hands over everything your account can do, until you
          revoke it again from your profile.
        </p>
      </header>

      <div className="flex flex-col gap-1 text-sm">
        <p>
          <span className="font-medium">{view.client_name}</span> wants to sign
          in as <span className="font-medium">{view.account}</span>.
        </p>
        <p className={MUTED}>
          It will be sent to{" "}
          <span className="font-mono">{view.redirect_host}</span>.
        </p>
      </div>

      {view.loopback && (
        <p
          role="alert"
          className="rounded border border-amber-300 bg-amber-50 px-3 py-2 text-sm text-amber-800 dark:border-amber-900 dark:bg-amber-950 dark:text-amber-200"
        >
          This client is a program on this computer, not a service on the web.
        </p>
      )}

      {decide.isError && (
        <p role="alert" className={PROBLEM}>
          {problemDetail(decide.error)}
        </p>
      )}

      <div className="flex gap-3">
        <button
          type="button"
          disabled={decide.isPending}
          onClick={() => {
            decide.mutate("allow");
          }}
          className={BUTTON.primary}
        >
          Allow
        </button>
        <button
          type="button"
          disabled={decide.isPending}
          onClick={() => {
            decide.mutate("deny");
          }}
          className={BUTTON.secondary}
        >
          Deny
        </button>
      </div>
    </div>
  );
}

/** What a request that is gone - unknown, expired, or already decided - shows. */
function Expired() {
  return (
    <div className="mx-auto flex w-full max-w-md flex-col gap-2">
      <h1 className="text-display">This request has expired</h1>
      <p className={MUTED}>Start again from the client that sent you here.</p>
    </div>
  );
}
