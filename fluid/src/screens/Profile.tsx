/**
 * Your own profile: who this session is, and the GitHub identity it shares as.
 *
 * The personal counterpart of the settings screen next door. That one manages
 * the MACHINE's credential and only an admin opens it; this one manages the
 * caller's own, and everybody who can share has one - the account sharing acts
 * as is a fact about a person rather than about the instance.
 *
 * The same promise holds on this side of it: the screen never sees a token
 * again after it is sent. It is held in a field of this component until the
 * server takes it, in one place and no other (see the mutation below, which
 * deliberately takes no variables), and no answer on this surface echoes token
 * material back.
 *
 * The device flow finishes somewhere else, so this polls while one is running
 * and stops the moment it is not: the identity route is the flow's own poll,
 * and asking it on a timer forever would be a request every three seconds for a
 * screen that is simply open.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";

import {
  MY_GITHUB_IDENTITY_KEY,
  connectMyGithubIdentityToken,
  disconnectMyGithubIdentity,
  fetchMyGithubIdentity,
  startMyGithubIdentityDevice,
} from "../api/admin";
import type { GithubIdentity, GithubPending } from "../api/admin";
import { problemDetail } from "../api/client";
import { useAuth } from "../auth/AuthContext";
import {
  BUTTON,
  CONTROL_HEIGHT,
  FIELD,
  FOCUS_RING,
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
