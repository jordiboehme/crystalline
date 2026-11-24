/**
 * The page an invite link opens: `/invite#<token>`.
 *
 * The token is read from the address's fragment once, when the page opens,
 * and the fragment is taken off the address bar right away (a replace, so
 * the base path stays and Back does not bring it back). From then on it
 * lives only in this component's state and travels only in POST bodies:
 * never in a query, a request URL or storage. A fragment never reaches the
 * server, so it stays out of access logs and `Referer`.
 *
 * Opening the page redeems nothing: link previews open links too. The page
 * asks `POST /invites/confirm` what the link offers and shows who made it,
 * the domain, the level and when it ends. Every way a link can be bad gets
 * the same sentence. Then it offers the way in the server names:
 *
 * - signed in already: accept for this account (membership only);
 * - `local`, or a link an admin made for a local account: choose a login and
 *   a password, or sign in to an existing account here and accept as it;
 * - `oidc`: one button that starts the sign-on; the token stays on the
 *   server with the sign-in, and the browser comes back on the domain;
 * - `proxy`: the forwarded identity is signed in already, so this is the
 *   first case; without one there is nothing to offer but to sign in.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useEffect, useId, useState } from "react";
import { Link, useLocation, useNavigate } from "react-router";

import {
  ApiProblem,
  navigateTo,
  problemDetail,
  setCsrfToken,
} from "../api/client";
import { DOMAINS_QUERY_KEY } from "../api/domains";
import { confirmInvite, redeemInvite } from "../api/invites";
import type { InviteOffer, MemberLevel, RedeemResponse } from "../api/model";
import { useAuth } from "../auth/AuthContext";
import { CARD_FIELD, CARD_LABEL } from "../auth/card";
import { ME_QUERY_KEY } from "../auth/keys";
import { GemTrio } from "../components/GemGlyph";
import { BUTTON } from "../components/primitives";
import { formatInstant } from "../format";
import { domainRoute } from "../paths";
import { usePageTitle } from "../usePageTitle";

/** What every bad link reads, whatever made it bad. */
export const INVALID_LINK = "This invite link is not valid any more.";

/** The token in a fragment such as `#cin_...`, or null when there is none. */
// eslint-disable-next-line react-refresh/only-export-components
export function tokenFromHash(hash: string): string | null {
  const token = hash.startsWith("#") ? hash.slice(1) : hash;
  return token.trim() === "" ? null : token.trim();
}

const LEVEL_WORDS: Record<MemberLevel, string> = {
  none: "No access",
  viewer: "Viewer",
  editor: "Editor",
  manager: "Manager",
};

const ALERT =
  "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200";
const SUBMIT = `py-2 ${BUTTON.primary} aria-disabled:cursor-default aria-disabled:opacity-50`;

/** The sentence for a refused confirm: the one sentence for a bad link. */
function confirmProblem(error: Error): string {
  if (
    error instanceof ApiProblem &&
    (error.status === 404 || error.status === 410)
  ) {
    return INVALID_LINK;
  }
  return problemDetail(error);
}

export default function InviteRedeem() {
  usePageTitle("Invite");
  const location = useLocation();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { user } = useAuth();
  // Read once: the address bar loses the fragment on the next line.
  const [token] = useState(() => tokenFromHash(location.hash));
  const [result, setResult] = useState<RedeemResponse | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  // While "Sign in and join" runs, the sign-in lands before the redemption
  // does; the page keeps that form until both are done.
  const [joining, setJoining] = useState(false);

  useEffect(() => {
    if (location.hash !== "") {
      void navigate(
        { pathname: location.pathname, search: location.search, hash: "" },
        { replace: true },
      );
    }
  }, [location.hash, location.pathname, location.search, navigate]);

  const offer = useQuery({
    // No token in the key: the cache holds nothing that opens the link.
    queryKey: ["invite-offer"],
    queryFn: () => confirmInvite(token ?? ""),
    enabled: token !== null,
    retry: false,
    staleTime: Infinity,
    gcTime: 0,
    refetchOnWindowFocus: false,
  });

  /** After a redemption: the identity and the domain list changed. */
  function done(answer: RedeemResponse) {
    if (answer.csrf) {
      setCsrfToken(answer.csrf);
    }
    setProblem(null);
    setResult(answer);
    void queryClient.invalidateQueries({ queryKey: ME_QUERY_KEY });
    void queryClient.invalidateQueries({ queryKey: DOMAINS_QUERY_KEY });
  }

  let body: ReactNode;
  if (token === null) {
    body = <p className={ALERT}>{INVALID_LINK}</p>;
  } else if (result !== null) {
    body = <Joined result={result} />;
  } else if (offer.isPending) {
    body = <p className="text-sm text-muted">Reading the invite...</p>;
  } else if (offer.isError) {
    body = (
      <p role="alert" className={ALERT}>
        {confirmProblem(offer.error)}
      </p>
    );
  } else {
    body = (
      <>
        <OfferFacts offer={offer.data} />
        {problem !== null && (
          <p role="alert" className={ALERT}>
            {problem}
          </p>
        )}
        <WayIn
          token={token}
          offer={offer.data}
          signedIn={
            user === null || joining
              ? null
              : { name: user.name, display: user.display }
          }
          onJoining={setJoining}
          onRedeemed={done}
          onProblem={setProblem}
        />
      </>
    );
  }

  return (
    <main className="flex min-h-screen items-start justify-center bg-slate-50 p-4 pt-[10vh] sm:p-6 sm:pt-[12vh] dark:bg-slate-950">
      <div className="flex w-full max-w-md flex-col gap-5">
        <div className="mx-auto w-fit">
          <GemTrio size={64} />
        </div>
        <h1 className="text-display text-center text-slate-900 dark:text-slate-50">
          You are invited
        </h1>
        {body}
      </div>
    </main>
  );
}

/** Who made the link, what it gives and when it ends. */
function OfferFacts({ offer }: { offer: InviteOffer }) {
  const inviter =
    offer.inviter_display && offer.inviter_display !== offer.inviter
      ? `${offer.inviter_display} (${offer.inviter})`
      : offer.inviter;
  return (
    <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 rounded border border-slate-200 bg-white p-4 text-sm dark:border-slate-800 dark:bg-slate-900">
      <dt className="text-muted">From</dt>
      <dd className="break-words">{inviter}</dd>
      {offer.domain ? (
        <>
          <dt className="text-muted">Domain</dt>
          <dd className="break-words">{offer.domain}</dd>
          <dt className="text-muted">Access</dt>
          <dd>{LEVEL_WORDS[offer.level ?? "viewer"]}</dd>
        </>
      ) : (
        <>
          <dt className="text-muted">Account</dt>
          <dd>{`A new account with role ${offer.role}`}</dd>
        </>
      )}
      <dt className="text-muted">Expires</dt>
      <dd>{formatInstant(offer.expires_at)}</dd>
    </dl>
  );
}

/** The way in this link offers this browser. */
function WayIn({
  token,
  offer,
  signedIn,
  onJoining,
  onRedeemed,
  onProblem,
}: {
  token: string;
  offer: InviteOffer;
  signedIn: { name: string; display: string } | null;
  onJoining: (joining: boolean) => void;
  onRedeemed: (answer: RedeemResponse) => void;
  onProblem: (text: string | null) => void;
}) {
  const accept = useMutation({
    // Never retried: a redemption is single use.
    retry: false,
    mutationFn: () => redeemInvite({ token }),
    onSuccess: (answer) => {
      if (answer.outcome === "sign_in" && answer.location) {
        navigateTo(answer.location);
        return;
      }
      onRedeemed(answer);
    },
    onError: (error: Error) => {
      onProblem(problemDetail(error));
    },
  });

  if (signedIn !== null) {
    const who =
      signedIn.display && signedIn.display !== signedIn.name
        ? `${signedIn.display} (${signedIn.name})`
        : signedIn.name;
    return (
      <div className="flex flex-col gap-3">
        <p className="text-sm">{`You are signed in as ${who}.`}</p>
        <button
          type="button"
          aria-disabled={accept.isPending}
          onClick={() => {
            if (accept.isPending) return;
            onProblem(null);
            accept.mutate();
          }}
          className={SUBMIT}
        >
          Accept invite
        </button>
      </div>
    );
  }

  const local = offer.sign_in === "local" || offer.local_allowed;
  return (
    <div className="flex flex-col gap-6">
      {offer.sign_in === "oidc" && (
        <div className="flex flex-col gap-2">
          <button
            type="button"
            aria-disabled={accept.isPending}
            onClick={() => {
              if (accept.isPending) return;
              onProblem(null);
              accept.mutate();
            }}
            className={SUBMIT}
          >
            Continue with single sign-on
          </button>
          <p className="text-sm text-muted">
            {offer.domain
              ? `You come back signed in, on the page of ${offer.domain}.`
              : "You come back signed in, on the start page."}
          </p>
        </div>
      )}
      {offer.sign_in === "proxy" && (
        <p className="text-sm">
          Sign in through your organization first, then open the link again.
        </p>
      )}
      {local && (
        <>
          <NewAccount
            token={token}
            onRedeemed={onRedeemed}
            onProblem={onProblem}
          />
          <ExistingAccount
            token={token}
            onJoining={onJoining}
            onRedeemed={onRedeemed}
            onProblem={onProblem}
          />
        </>
      )}
    </div>
  );
}

/** Choose a login and a password: a new local account, signed in at once. */
function NewAccount({
  token,
  onRedeemed,
  onProblem,
}: {
  token: string;
  onRedeemed: (answer: RedeemResponse) => void;
  onProblem: (text: string | null) => void;
}) {
  const ids = useId();
  const [login, setLogin] = useState("");
  const [display, setDisplay] = useState("");
  const [password, setPassword] = useState("");
  const create = useMutation({
    retry: false,
    mutationFn: () =>
      redeemInvite({
        token,
        login: login.trim(),
        password,
        display_name: display.trim() === "" ? null : display.trim(),
      }),
    onSuccess: onRedeemed,
    onError: (error: Error) => {
      onProblem(problemDetail(error));
    },
  });
  return (
    <form
      aria-labelledby={`${ids}-title`}
      className="flex flex-col gap-3"
      onSubmit={(event) => {
        event.preventDefault();
        if (create.isPending) return;
        onProblem(null);
        create.mutate();
      }}
    >
      <h2 id={`${ids}-title`} className="text-section">
        New here
      </h2>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${ids}-login`} className={CARD_LABEL}>
          Choose a login name
        </label>
        <input
          id={`${ids}-login`}
          autoComplete="username"
          required
          value={login}
          onChange={(event) => {
            setLogin(event.target.value);
          }}
          className={CARD_FIELD}
        />
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${ids}-display`} className={CARD_LABEL}>
          Display name (optional)
        </label>
        <input
          id={`${ids}-display`}
          autoComplete="name"
          value={display}
          onChange={(event) => {
            setDisplay(event.target.value);
          }}
          className={CARD_FIELD}
        />
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${ids}-password`} className={CARD_LABEL}>
          Choose a password
        </label>
        <input
          id={`${ids}-password`}
          type="password"
          autoComplete="new-password"
          required
          value={password}
          onChange={(event) => {
            setPassword(event.target.value);
          }}
          className={CARD_FIELD}
        />
      </div>
      <button type="submit" aria-disabled={create.isPending} className={SUBMIT}>
        Create account and join
      </button>
    </form>
  );
}

/**
 * Sign in to an account this instance has, on this page, and accept as it.
 * The token stays in this page's state the whole time; leaving for the
 * sign-in screen would lose it.
 */
function ExistingAccount({
  token,
  onJoining,
  onRedeemed,
  onProblem,
}: {
  token: string;
  onJoining: (joining: boolean) => void;
  onRedeemed: (answer: RedeemResponse) => void;
  onProblem: (text: string | null) => void;
}) {
  const ids = useId();
  const { login } = useAuth();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const join = useMutation({
    retry: false,
    mutationFn: async () => {
      onJoining(true);
      await login(name.trim(), password);
      return redeemInvite({ token });
    },
    onSuccess: onRedeemed,
    onSettled: () => {
      onJoining(false);
    },
    onError: (error: Error) => {
      onProblem(problemDetail(error));
    },
  });
  return (
    <form
      aria-labelledby={`${ids}-title`}
      className="flex flex-col gap-3 border-t border-slate-200 pt-5 dark:border-slate-800"
      onSubmit={(event) => {
        event.preventDefault();
        if (join.isPending) return;
        onProblem(null);
        join.mutate();
      }}
    >
      <h2 id={`${ids}-title`} className="text-section">
        Have an account here
      </h2>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${ids}-name`} className={CARD_LABEL}>
          Name
        </label>
        <input
          id={`${ids}-name`}
          autoComplete="username"
          required
          value={name}
          onChange={(event) => {
            setName(event.target.value);
          }}
          className={CARD_FIELD}
        />
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${ids}-password`} className={CARD_LABEL}>
          Password
        </label>
        <input
          id={`${ids}-password`}
          type="password"
          autoComplete="current-password"
          required
          value={password}
          onChange={(event) => {
            setPassword(event.target.value);
          }}
          className={CARD_FIELD}
        />
      </div>
      <button type="submit" aria-disabled={join.isPending} className={SUBMIT}>
        Sign in and join
      </button>
    </form>
  );
}

/** What the redemption did, and the way on. */
function Joined({ result }: { result: RedeemResponse }) {
  const level = result.level ? LEVEL_WORDS[result.level] : null;
  return (
    <div className="flex flex-col gap-3">
      <p role="status" className="text-sm">
        {result.domain
          ? `You joined ${result.domain}${level ? ` as ${level.toLowerCase()}` : ""}.`
          : "Your account is ready."}
      </p>
      {result.domain ? (
        <Link to={domainRoute(result.domain)} className={SUBMIT}>
          {`Open ${result.domain}`}
        </Link>
      ) : (
        <Link to="/" className={SUBMIT}>
          Go to the start page
        </Link>
      )}
    </div>
  );
}
