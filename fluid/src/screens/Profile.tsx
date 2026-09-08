/**
 * Your own profile: two cards, both about what this account is trusted to act
 * as elsewhere. GitHub identity is the account your shared work is opened
 * under; Agent access is the tokens an agent authenticates the daemon with,
 * acting as you. Both promise the same thing about a secret in transit: the
 * screen never shows it back after it is sent or issued.
 *
 * The GitHub half is the personal counterpart of the settings screen next
 * door. That one manages the MACHINE's credential and only an admin opens it;
 * this one manages the caller's own, and everybody who can share has one - the
 * account sharing acts as is a fact about a person rather than about the
 * instance. A pasted personal access token is held in a field of that card
 * until the server takes it, in one place and no other (see the mutation
 * below, which deliberately takes no variables), and no answer on that surface
 * echoes token material back. Its device flow finishes somewhere else, so that
 * card polls while one is running and stops the moment it is not: the
 * identity route is the flow's own poll, and asking it on a timer forever
 * would be a request every three seconds for a screen that is simply open.
 *
 * The MCP half is the opposite direction: the server hands the secret to the
 * card, exactly once, on issue and on rotate. See {@link AgentAccessCard} for
 * where that secret lives and how it leaves.
 *
 * A third card appears only on an instance with single sign-on configured, or
 * on an account that already holds an identity: the provider identity this
 * account signs in with. It is the one card whose primary action leaves the
 * app - linking is a whole sign-on against the provider, because nothing short
 * of one proves the identity is the caller's - and the one whose refusal is
 * load bearing: an account with no password cannot give up its last identity,
 * since that would leave an account nobody can sign in to. The server owns
 * that rule and says so in its own words, which name the command that gives
 * the account a password first.
 *
 * A fourth card, {@link OauthGrantsCard}, sits directly under the agent
 * access one: the clients this account connected through OAuth rather than
 * through a pasted token - a hosted client like Claude, or a local agent that
 * ran the loopback flow. It is the opposite direction of both cards above it,
 * connection-wise: nothing here is issued or pasted, only listed and revoked,
 * because a grant is minted by the authorization code flow at `/authorize`,
 * never by this screen.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy } from "lucide-react";
import { Dialog } from "radix-ui";
import { useEffect, useId, useRef, useState } from "react";

import {
  MY_GITHUB_IDENTITY_KEY,
  connectMyGithubIdentityToken,
  disconnectMyGithubIdentity,
  fetchMyGithubIdentity,
  startMyGithubIdentityDevice,
} from "../api/admin";
import type { GithubIdentity, GithubPending } from "../api/admin";
import { ApiProblem, problemDetail } from "../api/client";
import {
  MCP_TOKENS_KEY,
  fetchMcpTokens,
  issueMcpToken,
  revokeMcpToken,
  rotateMcpToken,
} from "../api/mcpTokens";
import type {
  IdentityLink,
  IssuedMcpToken,
  McpTokenInfo,
  OauthGrantInfo,
  User,
} from "../api/model";
import {
  OAUTH_GRANTS_KEY,
  fetchOauthGrants,
  revokeOauthGrant,
} from "../api/oauth";
import {
  IDENTITY_LINKS_KEY,
  PROVIDERS_KEY,
  fetchIdentityLinks,
  fetchProviders,
  startSsoLink,
  unlinkIdentity,
} from "../api/sso";
import { useAuth } from "../auth/AuthContext";
import {
  BUTTON,
  CONTROL_HEIGHT,
  FIELD,
  FOCUS_RING,
  IconButton,
} from "../components/primitives";
import { formatDay } from "../format";
import NotFound from "./NotFound";

/** How often the identity is asked again while a device flow is running. */
const POLL_MS = 3000;

/** What the card is currently saying, and whether it is bad news. */
interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** The card's one primary: the way in most people will take. */
const CONNECT_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.primary}`;

const SECONDARY_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.secondary}`;

const DANGER_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.destructive}`;

const MUTED = "text-sm text-slate-500 dark:text-slate-400";

export default function Profile() {
  const { user, capabilities } = useAuth();
  // A profile is an account's, and an anonymous session has none. The address
  // then says exactly as much as a mistyped one does.
  if (!user) {
    return <NotFound />;
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6">
      <header>
        <h1 className="text-display">{user.display}</h1>
        <p className={`mt-1 ${MUTED}`}>
          {user.name} ({capabilities.role})
        </p>
      </header>
      <GithubIdentityCard />
      <SsoIdentityCard user={user} />
      <AgentAccessCard />
      <OauthGrantsCard />
    </div>
  );
}

/**
 * The GitHub identity this account shares as.
 *
 * Gated on the role rather than on `canWrite`, which folds in whether the
 * instance takes writes at all: a read-only instance still has an identity to
 * show, it simply refuses every verb that would change one, and those are left
 * out below instead of the whole card. A viewer is the other case and is a
 * different sentence: the server refuses them this route, so the card does not
 * go and ask - it says why there is nothing here for them.
 */
function GithubIdentityCard() {
  const { capabilities } = useAuth();
  const sharer =
    capabilities.role === "editor" || capabilities.role === "admin";

  return (
    <section
      aria-labelledby="github-identity"
      className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div>
        <h2 id="github-identity" className="text-section">
          GitHub identity
        </h2>
        <p className={`mt-1 ${MUTED}`}>
          The account your shared work is opened under. Proposals you share from
          this instance carry your GitHub name, so it needs write access to the
          repositories your team domains track.
        </p>
      </div>
      {sharer ? (
        <IdentityPanel />
      ) : (
        <p className={MUTED}>
          Sharing is not available for viewer accounts, so there is no GitHub
          identity to connect here.
        </p>
      )}
    </section>
  );
}

/**
 * The single sign-on identity this account signs in with.
 *
 * Absent entirely when there is nothing to say - no provider configured and no
 * link held - because a card explaining a feature this instance does not have
 * is noise on the one screen that is about this account. An account that still
 * holds a link from a provider an operator has since turned off keeps the card,
 * so the link is visible and can be given up.
 *
 * Linking starts with a POST and ends with a navigation, which is why it is a
 * button rather than a plain link: the POST is what records, server side, that
 * this journey is linking an identity to THIS account rather than signing
 * somebody in, and it is under the CSRF check like every other POST. Only what
 * comes back from it is navigated to, with the whole page: a fetch would follow
 * the redirect to the provider in the background and land nowhere anybody can
 * type a password into. The journey ends on the home screen rather than here,
 * because the callback is the ordinary sign-in path and mints a session the
 * same way.
 */
function SsoIdentityCard({ user }: { user: User }) {
  const queryClient = useQueryClient();
  const [notice, setNotice] = useState<Notice | null>(null);

  const providers = useQuery({
    queryKey: PROVIDERS_KEY,
    queryFn: fetchProviders,
    retry: false,
  });
  const identities = useQuery({
    queryKey: IDENTITY_LINKS_KEY,
    queryFn: fetchIdentityLinks,
  });

  const unlink = useMutation({
    mutationFn: (issuer: string) => unlinkIdentity(issuer),
    onSuccess: () => {
      setNotice({ kind: "done", text: "The identity is unlinked." });
      void queryClient.invalidateQueries({ queryKey: IDENTITY_LINKS_KEY });
    },
    onError: (error: Error) => {
      // Including the refusal that keeps an account reachable, whose words
      // name `crystalline users passwd`. A house message pasted over it would
      // leave somebody stuck with no idea what to do next.
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const link = useMutation({
    // Never retried: the POST records a pending link server side, and a second
    // one would leave a journey nobody finishes taking up a slot.
    retry: false,
    mutationFn: startSsoLink,
    onSuccess: (started) => {
      // The whole page leaves for the provider. Not a fetch: what follows is a
      // redirect out and a redirect back, and only a real navigation can carry
      // somebody to a password field.
      window.location.assign(started.location);
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const links = identities.data?.links ?? [];
  const configured = providers.data?.oidc.enabled === true;
  const providerName = providers.data?.oidc.name ?? "single sign-on";
  // A read that has not landed is not an account with nothing linked, and a
  // read that FAILED is not one either: saying "no provider identity linked"
  // to a person whose account signs in through one would be the opposite of
  // the truth, and it would offer them a link button they must not press. So
  // the card waits for the answer and shows the failure when there is one.
  const unknown = identities.isPending || identities.isError;
  if (!configured && (unknown || links.length === 0)) {
    return null;
  }
  // One identity per provider is the rule, so a configured provider this
  // account has not linked yet is exactly the case the link button is for.
  const canLink = configured && !unknown && links.length === 0;

  return (
    <section
      aria-labelledby="sso-identity"
      className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div>
        <h2 id="sso-identity" className="text-section">
          SSO identity
        </h2>
        <p className={`mt-1 ${MUTED}`}>
          The provider identity you sign in with. Linking one is deliberate:
          nobody is ever put into this account because an address happened to
          match.
        </p>
      </div>

      {notice && (
        <p
          role={notice.kind === "problem" ? "alert" : "status"}
          className={
            notice.kind === "problem"
              ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
              : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
          }
        >
          {notice.text}
        </p>
      )}

      {identities.isError && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(identities.error)}
        </p>
      )}

      {unknown ? (
        <p className={MUTED}>
          {identities.isPending
            ? "Reading your provider identities"
            : "This account's provider identities could not be read."}
        </p>
      ) : links.length > 0 ? (
        <ul className="flex flex-col gap-2">
          {links.map((link: IdentityLink) => (
            <li
              key={link.issuer}
              className="flex flex-wrap items-center justify-between gap-3 rounded border border-slate-200 px-3 py-2 dark:border-slate-800"
            >
              <div className="min-w-0">
                <p className="truncate text-sm text-slate-900 dark:text-slate-100">
                  {link.issuer}
                </p>
                <p className={MUTED}>
                  {linkedByLine(link, user.name)} on {formatDay(link.linked_at)}
                </p>
              </div>
              <button
                type="button"
                disabled={unlink.isPending}
                onClick={() => {
                  unlink.mutate(link.issuer);
                }}
                className={DANGER_BUTTON}
              >
                Unlink
              </button>
            </li>
          ))}
        </ul>
      ) : (
        <p className={MUTED}>
          This account has no provider identity linked, so it signs in with its
          password.
        </p>
      )}

      {canLink && (
        <div>
          <button
            type="button"
            disabled={link.isPending}
            onClick={() => {
              link.mutate();
            }}
            className={CONNECT_BUTTON}
          >
            Link {providerName}
          </button>
        </div>
      )}
      {identities.data?.has_password === false && links.length > 0 && (
        <p className={MUTED}>
          This account has no password, so this identity is its only way in.
          Giving it up is refused until an administrator sets a password with{" "}
          <code>crystalline users passwd</code>.
        </p>
      )}
    </section>
  );
}

/**
 * Who made one link, in words rather than in the stored marker.
 *
 * Three markers are known - a first sign-on, the command line, and the account
 * doing it itself - and anything else is shown as itself rather than folded
 * into one of them: a fourth writer would otherwise be rendered as a sentence
 * that is not true.
 */
function linkedByLine(link: IdentityLink, self: string): string {
  if (link.linked_by === "jit") {
    return "Linked when this account was created by a first sign-on";
  }
  if (link.linked_by === "cli") {
    return "Linked by an administrator";
  }
  if (link.linked_by === self) {
    return "Linked from this profile";
  }
  return `Linked by ${link.linked_by}`;
}

/** The card proper, for a session that may have an identity of its own. */
function IdentityPanel() {
  const { capabilities } = useAuth();
  const queryClient = useQueryClient();
  const tokenField = useId();
  const [token, setToken] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);

  const identity = useQuery({
    queryKey: MY_GITHUB_IDENTITY_KEY,
    queryFn: fetchMyGithubIdentity,
    // Only while something is running. The flow ends in another window, so
    // there is no event to wait for - and no reason to ask once it has. The
    // read itself reaches nothing but this instance's own credential store, so
    // it needs none of the freshness ceremony a live origin probe does.
    refetchInterval: (query) => (query.state.data?.pending ? POLL_MS : false),
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: MY_GITHUB_IDENTITY_KEY });

  const connect = useMutation({
    mutationFn: startMyGithubIdentityDevice,
    onSuccess: () => {
      setNotice(null);
      void invalidate();
    },
    onError: (error: Error) => {
      // Including the one refusal that is nobody's mistake: there is one
      // sign-in slot per instance, and somebody else is in it. The server says
      // so in its own words, and waiting is the whole of the fix.
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const withToken = useMutation({
    /*
      A closure over the field rather than the token passed to `mutate`, for
      the reason the settings screen spells out: what is passed to `mutate`
      becomes the mutation's `variables`, and query-core writes those into
      mutation state and never clears them. Taking no variables at all means
      there is nothing to leave behind.
    */
    mutationFn: () => connectMyGithubIdentityToken(token),
    onSuccess: () => {
      // Only now: a token the server refused is a token whose owner is about
      // to paste a corrected one, and clearing the field would make the fix
      // start from nothing.
      setToken("");
      setNotice({ kind: "done", text: "Your token is stored." });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const forget = useMutation({
    mutationFn: disconnectMyGithubIdentity,
    onSuccess: () => {
      setNotice({
        kind: "done",
        text: "Your credential is gone. Sharing needs one, so connect again before you share.",
      });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const connection = identity.data ?? null;
  const pending = connection?.pending ?? null;
  /*
    A failure the server reports once, said out loud - unless a flow is running
    right now. The connect answer can carry an EARLIER flow's failure beside
    the fresh pending block, and putting a sentence about the last attempt over
    the code somebody is about to type would be the card contradicting itself.
  */
  const failure = pending === null ? (connection?.error ?? null) : null;

  return (
    <>
      {notice && (
        <p
          role={notice.kind === "problem" ? "alert" : "status"}
          className={
            notice.kind === "problem"
              ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
              : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
          }
        >
          {notice.text}
        </p>
      )}

      {identity.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(identity.error)}
        </p>
      )}

      {identity.isPending ? (
        <p className={MUTED}>Reading your GitHub identity</p>
      ) : (
        <Standing identity={connection} />
      )}

      {/*
        The server's own sentence about the attempt that just ended. It arrives
        once and is cleared on the read that carried it, so it is shown as it
        was said rather than restated.
      */}
      {failure !== null && (
        <p
          role="alert"
          className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
        >
          {failure}
        </p>
      )}

      {capabilities.readOnly ? (
        // Every verb below is refused on a read-only instance, so none of them
        // is drawn: the app offers no door that will not open.
        <p className={MUTED}>
          This instance is read only, so nothing here can be connected or
          disconnected.
        </p>
      ) : (
        <>
          {pending === null ? (
            <div className="flex flex-wrap items-center gap-3">
              <button
                type="button"
                disabled={connect.isPending}
                onClick={() => {
                  connect.mutate();
                }}
                className={CONNECT_BUTTON}
              >
                Connect with GitHub
              </button>
              <span className={MUTED}>
                Signs in through a browser, with a short code.
              </span>
            </div>
          ) : (
            <DeviceFlow pending={pending} />
          )}

          {connection?.connected === true && (
            <Disconnect
              pending={forget.isPending}
              onDisconnect={() => {
                forget.mutate();
              }}
            />
          )}

          <form
            className="flex flex-wrap items-end gap-3 border-t border-slate-200 pt-4 dark:border-slate-800"
            onSubmit={(event) => {
              event.preventDefault();
              // No argument: the mutation reads the field itself, so the token
              // never becomes a variable anything keeps.
              withToken.mutate();
            }}
          >
            <div className="flex flex-col gap-1">
              <label
                htmlFor={tokenField}
                className="text-xs text-slate-500 dark:text-slate-400"
              >
                Personal access token
              </label>
              <input
                id={tokenField}
                type="password"
                required
                autoComplete="off"
                value={token}
                onChange={(event) => {
                  setToken(event.target.value);
                }}
                className={`w-72 ${FIELD}`}
              />
            </div>
            <button
              type="submit"
              disabled={withToken.isPending}
              className={SECONDARY_BUTTON}
            >
              Connect with token
            </button>
            <span className={MUTED}>
              For a browser that cannot finish the sign-in. Sent once and stored
              by the server; this screen never shows it again.
            </span>
          </form>
        </>
      )}
    </>
  );
}

/** Whether an identity is on file, whose it is and since when. */
function Standing({ identity }: { identity: GithubIdentity | null }) {
  if (identity?.connected !== true) {
    return <p className="text-sm">Not connected</p>;
  }
  const store = identity.tokenStore ?? "an unnamed store";
  const since =
    identity.connectedAt === null
      ? ""
      : `since ${formatDay(identity.connectedAt)} `;
  return (
    <div className="flex flex-col gap-1">
      <p className="text-sm">
        {identity.login === null
          ? "Connected as an account GitHub did not name"
          : `Connected as @${identity.login}`}
      </p>
      <p className={MUTED}>{`Connected ${since}(${store})`}</p>
    </div>
  );
}

/**
 * A device sign-in, mid-flight: the code, where to type it, and the fact that
 * the app is now waiting on a window somebody else has to visit.
 */
function DeviceFlow({ pending }: { pending: GithubPending }) {
  const minutes = Math.round(pending.expiresInSecs / 60);
  return (
    <div className="flex flex-col items-start gap-2">
      {/*
        The code is the one thing here somebody has to read off and retype, so
        it is set large and in the mono face every identifier in this app
        wears, with the letters spaced apart.
      */}
      <p className="font-mono text-display tracking-widest">
        {pending.userCode}
      </p>
      <a
        href={pending.verificationUrl}
        target="_blank"
        rel="noreferrer"
        className={`text-sm text-accent-700 underline underline-offset-2 hover:no-underline dark:text-accent-400 ${FOCUS_RING}`}
      >
        Open github.com and enter the code
      </a>
      <p className={MUTED}>
        Waiting for the browser confirmation.
        {minutes > 0 &&
          ` The code is good for about ${String(minutes)} minutes.`}
      </p>
    </div>
  );
}

/**
 * Forgetting your credential, behind a second press.
 *
 * Two steps rather than a browser confirm, for the reason the settings screen
 * gives: a dialog the browser owns cannot be reached by a test, cannot be
 * styled and cannot be dismissed by the keyboard the way the rest of this can.
 */
function Disconnect({
  pending,
  onDisconnect,
}: {
  pending: boolean;
  onDisconnect: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);

  /** Give up on the pending disconnect, and hand the focus back to what asked. */
  function abandon() {
    setConfirming(false);
    trigger.current?.focus();
  }

  return (
    <div
      className="flex flex-wrap items-center gap-2"
      onKeyDown={(event) => {
        if (event.key === "Escape" && confirming) {
          event.stopPropagation();
          abandon();
        }
      }}
      onBlur={(event) => {
        // Only when the focus actually landed somewhere else: a `focusout`
        // with no destination is what a click looks like mid-flight, and
        // taking the confirmation away there would eat the second press this
        // exists to require.
        const next = event.relatedTarget;
        if (
          confirming &&
          next instanceof Node &&
          !event.currentTarget.contains(next)
        ) {
          setConfirming(false);
        }
      }}
    >
      <button
        ref={trigger}
        type="button"
        aria-expanded={confirming}
        disabled={pending}
        onClick={() => {
          setConfirming(true);
        }}
        className={DANGER_BUTTON}
      >
        Disconnect
      </button>
      {confirming && (
        <>
          <button
            type="button"
            autoFocus
            onClick={() => {
              setConfirming(false);
              onDisconnect();
            }}
            className={DANGER_BUTTON}
          >
            Confirm disconnect
          </button>
          <button type="button" onClick={abandon} className={SECONDARY_BUTTON}>
            Keep
          </button>
          <span className={MUTED}>
            Your shared proposals stay where they are; sharing again needs a
            connected identity.
          </span>
        </>
      )}
    </div>
  );
}

/** How long a clipboard outcome stays announced. */
const COPY_CONFIRMED_FOR_MS = 2000;

/**
 * The card an agent's own MCP token lives behind: issue, rotate, revoke.
 *
 * Shown to every signed-in account, viewers included - an agent acts as the
 * account that issued its token, so a viewer's agent can read but never
 * write, which is a fact about the token rather than about this card.
 *
 * Every verb here is offered even on a read-only instance, unlike the
 * identity card above: `service.read_only` protects the knowledge, and a
 * token is account state rather than knowledge, in the accounts database
 * beside the password that logs the same person in. It is also the one place
 * an agent most needs to be issued a token - a read-only team server still
 * runs with `auth.mcp` on, and a reading agent cannot connect at all without
 * one. Nothing here asks the server whether that is allowed before offering
 * it, the same rule the account-admin screen follows: the server decides,
 * every time, and a refusal comes back in its own words through `notice`.
 *
 * The one secret this screen ever holds is the freshly issued or rotated
 * token, and it lives in {@link reveal} alone - a plain field of this
 * component, never a browser store, and never React Query's own data either.
 * `useMutation` caches whatever a `mutationFn` returns for as long as the
 * mutation itself is kept (`reset()` clears an observer's own view of it, but
 * the `Mutation` entry it points at stays in the `MutationCache` regardless,
 * which a query-core mutation resolves to a promise before this component
 * could act on it). So `issue` and `rotate` below never let the secret reach
 * that return value at all: their `mutationFn` calls the route, hands the raw
 * reply to `reveal` as a side effect, and returns only the row's id - a
 * number with nothing to hold. Dismissing the dialog clears `reveal`, which is
 * what takes the token out of the DOM for good.
 */
// Exported, unlike every other card in this file, so a test can mount it
// under a `QueryClient` of its own and inspect that client's
// `MutationCache` directly - the only way to pin that the secret returned by
// `issue`/`rotate` above never becomes the value React Query retains (see
// their `mutationFn` comments). Nothing here needs router or auth context, so
// the export costs nothing beyond this one seam.
export function AgentAccessCard() {
  const queryClient = useQueryClient();
  const labelField = useId();
  const [label, setLabel] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);
  const [reveal, setReveal] = useState<IssuedMcpToken | null>(null);

  const tokens = useQuery({
    queryKey: MCP_TOKENS_KEY,
    queryFn: fetchMcpTokens,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: MCP_TOKENS_KEY });

  const issue = useMutation({
    // Never retried: the client default inherits `retry: 1` for anything but
    // a 4xx, which is right for a query and wrong here - a POST that reached
    // the server and committed, then failed on the way back (a dropped
    // connection, or a 2xx whose body could not be read), would otherwise be
    // sent again and mint a second token whose secret nobody sees. The house
    // idiom for exactly this (`ProposalsCard.tsx`, `FirstRunSetup.tsx`).
    retry: false,
    mutationFn: async (issuedLabel: string) => {
      const issued = await issueMcpToken(issuedLabel);
      setReveal(issued);
      return issued.id;
    },
    onSuccess: () => {
      setLabel("");
      setNotice(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const rotate = useMutation({
    // Same reasoning as `issue`: a retried rotate is a second live secret for
    // one request.
    retry: false,
    mutationFn: async (id: number) => {
      const issued = await rotateMcpToken(id);
      setReveal(issued);
      return issued.id;
    },
    onSuccess: () => {
      setNotice(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const revoke = useMutation({
    mutationFn: (id: number) => revokeMcpToken(id),
    onSuccess: () => {
      setNotice({ kind: "done", text: "The token is revoked." });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const rows = tokens.data ?? [];

  return (
    <section
      aria-labelledby="agent-access"
      className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div>
        <h2 id="agent-access" className="text-section">
          Agent access
        </h2>
        <p className={`mt-1 ${MUTED}`}>
          The tokens an agent authenticates the daemon with. An agent acts as
          the account that issued its token, so yours can do whatever your own
          account can and nothing more.
        </p>
      </div>

      {notice && (
        <p
          role={notice.kind === "problem" ? "alert" : "status"}
          className={
            notice.kind === "problem"
              ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
              : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
          }
        >
          {notice.text}
        </p>
      )}

      {tokens.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(tokens.error)}
        </p>
      )}

      {tokens.isPending ? (
        <p className={MUTED}>Reading your tokens</p>
      ) : rows.length === 0 ? (
        <p className={MUTED}>No tokens issued yet.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Your MCP tokens</caption>
            <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
              <tr>
                <th scope="col" className="px-2 py-2">
                  Label
                </th>
                <th scope="col" className="px-2 py-2">
                  Created
                </th>
                <th scope="col" className="px-2 py-2">
                  Last used
                </th>
                <th scope="col" className="px-2 py-2">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {rows.map((token) => (
                <TokenRow
                  key={token.id}
                  token={token}
                  rotating={rotate.isPending && rotate.variables === token.id}
                  revoking={revoke.isPending && revoke.variables === token.id}
                  onRotate={() => {
                    rotate.mutate(token.id);
                  }}
                  onRevoke={() => {
                    revoke.mutate(token.id);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}

      <form
        className="flex flex-wrap items-end gap-3 border-t border-slate-200 pt-4 dark:border-slate-800"
        onSubmit={(event) => {
          event.preventDefault();
          issue.mutate(label);
        }}
      >
        <div className="flex flex-col gap-1">
          <label
            htmlFor={labelField}
            className="text-xs text-slate-500 dark:text-slate-400"
          >
            Label
          </label>
          <input
            id={labelField}
            required
            autoComplete="off"
            placeholder="laptop, or the agent it is for"
            value={label}
            onChange={(event) => {
              setLabel(event.target.value);
            }}
            className={`w-56 ${FIELD}`}
          />
        </div>
        <button
          type="submit"
          disabled={issue.isPending}
          className={CONNECT_BUTTON}
        >
          Issue token
        </button>
      </form>

      {reveal && (
        <RevealDialog
          issued={reveal}
          onClose={() => {
            setReveal(null);
          }}
        />
      )}
    </section>
  );
}

/** One token row: what it is called, when it was issued, when it last resolved. */
function TokenRow({
  token,
  rotating,
  revoking,
  onRotate,
  onRevoke,
}: {
  token: McpTokenInfo;
  rotating: boolean;
  revoking: boolean;
  onRotate: () => void;
  onRevoke: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);

  /** Give up on the pending revoke, and hand the focus back to what asked. */
  function abandon() {
    setConfirming(false);
    trigger.current?.focus();
  }

  return (
    <tr className="align-top">
      <th scope="row" className="px-2 py-2 font-normal">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {token.label}
        </span>
      </th>
      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {formatDay(token.created_at)}
        </span>
      </td>
      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {token.last_used == null ? (
            <span className="text-slate-500 dark:text-slate-400">Never</span>
          ) : (
            formatDay(token.last_used)
          )}
        </span>
      </td>
      <td className="px-2 py-2">
        {/*
          Two steps rather than a browser confirm, for the reason every other
          destructive control on this screen gives: a dialog the browser owns
          cannot be reached by a test, cannot be styled and cannot be
          dismissed by the keyboard the way this can.
        */}
        <div
          className="flex flex-wrap items-center gap-2"
          onKeyDown={(event) => {
            if (event.key === "Escape" && confirming) {
              event.stopPropagation();
              abandon();
            }
          }}
          onBlur={(event) => {
            const next = event.relatedTarget;
            if (
              confirming &&
              next instanceof Node &&
              !event.currentTarget.contains(next)
            ) {
              setConfirming(false);
            }
          }}
        >
          <button
            type="button"
            aria-label={`Rotate ${token.label}`}
            disabled={rotating || confirming}
            onClick={onRotate}
            className={SECONDARY_BUTTON}
          >
            Rotate
          </button>
          {/*
            Rendered unconditionally, the way `Disconnect` next door renders
            its own trigger: `abandon()` below focuses this ref, and a ref
            behind a ternary's other branch is null the moment `confirming`
            flips true, which sends Escape and Keep's focus to
            `document.body` instead of back to this button.
          */}
          <button
            ref={trigger}
            type="button"
            aria-label={`Revoke ${token.label}`}
            aria-expanded={confirming}
            disabled={revoking}
            onClick={() => {
              setConfirming(true);
            }}
            className={DANGER_BUTTON}
          >
            Revoke
          </button>
          {confirming && (
            <>
              <button
                type="button"
                autoFocus
                aria-label={`Confirm revoke ${token.label}`}
                disabled={revoking}
                onClick={() => {
                  setConfirming(false);
                  onRevoke();
                }}
                className={DANGER_BUTTON}
              >
                Confirm revoke
              </button>
              <button
                type="button"
                onClick={abandon}
                className={SECONDARY_BUTTON}
              >
                Keep
              </button>
            </>
          )}
        </div>
      </td>
    </tr>
  );
}

/**
 * The one-time reveal: the freshly issued or rotated token, a copy button and
 * the exact line to paste into an agent's own MCP registration.
 *
 * The token lives only in `issued`, which the caller holds in a plain field
 * rather than in any cache - see {@link AgentAccessCard}. Closing this dialog
 * (Done, the overlay, Escape) unmounts it, which is what takes the token out
 * of the DOM for good; there is no way back into a dismissed reveal.
 */
function RevealDialog({
  issued,
  onClose,
}: {
  issued: IssuedMcpToken;
  onClose: () => void;
}) {
  const [copied, setCopied] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (copied === "idle") {
      return;
    }
    const timer = setTimeout(() => {
      setCopied("idle");
    }, COPY_CONFIRMED_FOR_MS);
    return () => {
      clearTimeout(timer);
    };
  }, [copied]);

  return (
    <Dialog.Root
      open
      onOpenChange={(next) => {
        if (!next) {
          onClose();
        }
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-slate-900/40" />
        <Dialog.Content className="fixed top-1/2 left-1/2 z-50 w-[min(32rem,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 rounded border border-slate-200 bg-white p-4 shadow-xl dark:border-slate-700 dark:bg-slate-900">
          <Dialog.Title className="text-lg font-semibold">
            {issued.label}
          </Dialog.Title>
          <Dialog.Description className={`mt-1 ${MUTED}`}>
            Shown once. Copy it now - this screen never shows it again.
          </Dialog.Description>

          <div className="mt-3 flex items-center gap-2">
            <code className="flex-1 overflow-x-auto rounded border border-slate-200 bg-slate-50 px-2 py-1 text-sm dark:border-slate-800 dark:bg-slate-900">
              {issued.token}
            </code>
            <IconButton
              label="Copy token"
              icon={Copy}
              onClick={() => {
                void (async () => {
                  try {
                    await navigator.clipboard.writeText(issued.token);
                    setCopied("copied");
                  } catch {
                    setCopied("failed");
                  }
                })();
              }}
            />
          </div>
          <span
            role="status"
            aria-live="polite"
            aria-label="Copy token result"
            className="mt-1 block text-xs text-slate-500 dark:text-slate-400"
          >
            {copied === "copied"
              ? "Copied"
              : copied === "failed"
                ? "Copy refused"
                : ""}
          </span>

          <p className="mt-4 text-sm">
            Add this as header{" "}
            <code className="rounded bg-slate-100 px-1 py-0.5 dark:bg-slate-800">
              Authorization: Bearer {issued.token}
            </code>{" "}
            to the crystalline entry in your agent&apos;s MCP registration.
          </p>

          <div className="mt-4 flex justify-end">
            <button
              type="button"
              onClick={onClose}
              className={SECONDARY_BUTTON}
            >
              Done
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/**
 * The card an account's connected OAuth clients live behind: which client
 * connected, since when, when it last used the connection, when its refresh
 * token expires, and a way to revoke it.
 *
 * Absent entirely on an instance that has never turned `auth.oauth` on -
 * `capabilities.oauth`, the probe's own rendering signal, the same role
 * `canShare` plays for the share surfaces - the same call `SsoIdentityCard`
 * makes for a feature this instance does not have: a card offering to manage
 * clients that can never exist is noise on the one screen that is about this
 * account. The query itself is gated the same way (`enabled:
 * capabilities.oauth`), so an instance that never serves OAuth pays no round
 * trip for a card it will never draw.
 *
 * Shown to every signed-in account, viewers included, and offered on a
 * read-only instance too - a grant is account state in the accounts database,
 * the same reason {@link AgentAccessCard} is and for the same server-side
 * rule (`revoke_my_oauth_grant` is `read_only_exempt`). Never a token: only
 * hashes are stored server side, so this listing carries `client_name`,
 * `redirect_host`, `created_at`, `last_used` and `refresh_expires_at`, and
 * nothing more, ever.
 *
 * Nothing here mints a grant. The only way one comes to exist is the
 * authorization code flow at `/authorize`, which is a client's journey, not
 * an action on this screen - this card only ever lists what already happened
 * there and lets it be taken back.
 *
 * A 404 on revoke is not shown as a failure: the server's non-idempotent
 * `DELETE` answers 404 for a grant that is already gone, whichever tab or
 * reason took it, and the button's job is done either way - the list is
 * refreshed and the server's own sentence is shown as a neutral notice, the
 * same `kind: "done"` path a genuine revoke takes, never `role="alert"` red
 * text over a row that no longer means anything.
 */
// Exported the same way `AgentAccessCard` is: so a test can mount it on its
// own. Unlike that card, this one needs the auth context for its gate.
export function OauthGrantsCard() {
  const { capabilities } = useAuth();
  const queryClient = useQueryClient();
  const [notice, setNotice] = useState<Notice | null>(null);

  const grants = useQuery({
    queryKey: OAUTH_GRANTS_KEY,
    queryFn: fetchOauthGrants,
    enabled: capabilities.oauth,
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: OAUTH_GRANTS_KEY });

  const revoke = useMutation({
    mutationFn: (id: number) => revokeOauthGrant(id),
    onSuccess: () => {
      setNotice({ kind: "done", text: "The client is disconnected." });
      void invalidate();
    },
    onError: (error: Error) => {
      // A grant that is already gone - revoked from another tab, expired,
      // whatever - is not a failure of this press: the button's job (making
      // sure this client is disconnected) is done either way. Refresh and
      // say so in the neutral notice, the same as a genuine revoke, rather
      // than leaving a red error over a row that no longer means anything.
      if (error instanceof ApiProblem && error.status === 404) {
        setNotice({ kind: "done", text: problemDetail(error) });
        void invalidate();
        return;
      }
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  if (!capabilities.oauth) {
    return null;
  }

  const rows = grants.data ?? [];

  return (
    <section
      aria-labelledby="oauth-grants"
      className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
    >
      <div>
        <h2 id="oauth-grants" className="text-section">
          Connected clients
        </h2>
        <p className={`mt-1 ${MUTED}`}>
          The clients you connected through OAuth. Each one can do whatever your
          own account can, until you revoke it here.
        </p>
      </div>

      {notice && (
        <p
          role={notice.kind === "problem" ? "alert" : "status"}
          className={
            notice.kind === "problem"
              ? "rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
              : "rounded border border-slate-200 bg-slate-50 px-3 py-2 text-sm text-slate-700 dark:border-slate-800 dark:bg-slate-900 dark:text-slate-300"
          }
        >
          {notice.text}
        </p>
      )}

      {grants.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(grants.error)}
        </p>
      )}

      {grants.isPending ? (
        <p className={MUTED}>Reading your connected clients</p>
      ) : rows.length === 0 ? (
        <p className={MUTED}>No client connected yet.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full text-left text-sm">
            <caption className="sr-only">Your connected OAuth clients</caption>
            <thead className="text-caption font-semibold text-slate-500 dark:text-slate-400">
              <tr>
                <th scope="col" className="px-2 py-2">
                  Client
                </th>
                <th scope="col" className="px-2 py-2">
                  Redirects to
                </th>
                <th scope="col" className="px-2 py-2">
                  Granted
                </th>
                <th scope="col" className="px-2 py-2">
                  Last use
                </th>
                <th scope="col" className="px-2 py-2">
                  Refresh expires
                </th>
                <th scope="col" className="px-2 py-2">
                  Actions
                </th>
              </tr>
            </thead>
            <tbody className="divide-y divide-slate-200 dark:divide-slate-800">
              {rows.map((grant) => (
                <GrantRow
                  key={grant.id}
                  grant={grant}
                  revoking={revoke.isPending && revoke.variables === grant.id}
                  onRevoke={() => {
                    revoke.mutate(grant.id);
                  }}
                />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

/** One connected client: what it is called, where it goes, and when it was last used. */
function GrantRow({
  grant,
  revoking,
  onRevoke,
}: {
  grant: OauthGrantInfo;
  revoking: boolean;
  onRevoke: () => void;
}) {
  const [confirming, setConfirming] = useState(false);
  const trigger = useRef<HTMLButtonElement>(null);

  /** Give up on the pending revoke, and hand the focus back to what asked. */
  function abandon() {
    setConfirming(false);
    trigger.current?.focus();
  }

  return (
    <tr className="align-top">
      <th scope="row" className="px-2 py-2 font-normal">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {grant.client_name}
        </span>
      </th>
      <td className="px-2 py-2">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {grant.redirect_host}
        </span>
      </td>
      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {formatDay(grant.created_at)}
        </span>
      </td>
      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {grant.last_used == null ? (
            <span className="text-slate-500 dark:text-slate-400">Never</span>
          ) : (
            formatDay(grant.last_used)
          )}
        </span>
      </td>
      <td className="px-2 py-2 tabular-nums">
        <span className={`flex ${CONTROL_HEIGHT} items-center`}>
          {formatDay(grant.refresh_expires_at)}
        </span>
      </td>
      <td className="px-2 py-2">
        {/*
          Two steps rather than a browser confirm, the reason every other
          destructive control on this screen gives; `aria-disabled` rather
          than `disabled` on both presses below, the newer of this app's two
          idioms for that (`MembersCard.tsx`'s own `DestructiveAction`) - a
          mutation in flight is a passing state rather than a certainty this
          side already holds, but keeping the trigger reachable and its state
          announced costs nothing while it lasts.
        */}
        <div
          className="flex flex-wrap items-center gap-2"
          onKeyDown={(event) => {
            if (event.key === "Escape" && confirming) {
              event.stopPropagation();
              abandon();
            }
          }}
          onBlur={(event) => {
            const next = event.relatedTarget;
            if (
              confirming &&
              next instanceof Node &&
              !event.currentTarget.contains(next)
            ) {
              setConfirming(false);
            }
          }}
        >
          <button
            ref={trigger}
            type="button"
            aria-label={`Revoke ${grant.client_name} (#${grant.id})`}
            aria-expanded={confirming}
            aria-disabled={revoking}
            onClick={() => {
              if (revoking) {
                return;
              }
              setConfirming(true);
            }}
            className={`${DANGER_BUTTON} aria-disabled:cursor-default aria-disabled:opacity-50`}
          >
            Revoke
          </button>
          {confirming && (
            <>
              <button
                type="button"
                autoFocus
                aria-label={`Confirm revoke ${grant.client_name} (#${grant.id})`}
                aria-disabled={revoking}
                onClick={() => {
                  if (revoking) {
                    return;
                  }
                  setConfirming(false);
                  onRevoke();
                }}
                className={`${DANGER_BUTTON} aria-disabled:cursor-default aria-disabled:opacity-50`}
              >
                Confirm revoke
              </button>
              <button
                type="button"
                onClick={abandon}
                className={SECONDARY_BUTTON}
              >
                Keep
              </button>
            </>
          )}
        </div>
      </td>
    </tr>
  );
}
