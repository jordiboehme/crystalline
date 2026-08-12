/**
 * The GitHub connection, the only settings section this app ships.
 *
 * One credential, two ways to hand it over: the device sign-in for somebody
 * sitting in front of a browser, and a personal access token for an instance
 * nobody is sitting in front of. The screen never sees a token again after it
 * is sent - it is held in a field of this component until the server takes it,
 * in one place and no other (see the mutation below, which deliberately takes
 * no variables), and no answer on this surface echoes token material back.
 *
 * The device flow finishes somewhere else, so this polls while one is running
 * and stops the moment it is not: the status route is the flow's own poll, and
 * asking it on a timer forever would be a request every three seconds for a
 * screen that is simply open.
 */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useId, useRef, useState } from "react";

import {
  GITHUB_STATUS_KEY,
  disconnectGithub,
  fetchGithubStatus,
  startGithubConnect,
  submitGithubToken,
} from "../api/admin";
import type { GithubPending } from "../api/admin";
import { problemDetail } from "../api/client";
import { useAuth } from "../auth/AuthContext";
import {
  BUTTON,
  CONTROL_HEIGHT,
  FIELD,
  FOCUS_RING,
} from "../components/primitives";
import NotFound from "./NotFound";

/** How often the status is asked again while a device flow is running. */
const POLL_MS = 3000;

/** What the screen is currently saying, and whether it is bad news. */
interface Notice {
  kind: "problem" | "done";
  text: string;
}

/** The screen's one primary: the way in most people will take. */
const CONNECT_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.primary}`;

const SECONDARY_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.secondary}`;

const DANGER_BUTTON = `${CONTROL_HEIGHT} ${BUTTON.destructive}`;

export default function GithubSettings() {
  const { capabilities } = useAuth();
  if (!capabilities.canAdminister) {
    return <NotFound />;
  }
  return <GithubPanel />;
}

function GithubPanel() {
  const queryClient = useQueryClient();
  const tokenField = useId();
  const [token, setToken] = useState("");
  const [notice, setNotice] = useState<Notice | null>(null);

  const status = useQuery({
    queryKey: GITHUB_STATUS_KEY,
    queryFn: fetchGithubStatus,
    // Only while something is running. The flow ends in another window, so
    // there is no event to wait for - and no reason to ask once it has.
    refetchInterval: (query) => (query.state.data?.pending ? POLL_MS : false),
  });

  const invalidate = () =>
    queryClient.invalidateQueries({ queryKey: GITHUB_STATUS_KEY });

  const connect = useMutation({
    mutationFn: startGithubConnect,
    onSuccess: () => {
      setNotice(null);
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const withToken = useMutation({
    /*
      A closure over the field rather than `mutationFn: submitGithubToken` with
      the token passed to `mutate`. What is passed to `mutate` becomes the
      mutation's `variables`, and query-core writes those into mutation state
      and never clears them: the `pending` case sets them, no later case takes
      them back, and the entry only leaves the cache when it is garbage
      collected - five minutes after this screen unmounts, and not at all while
      it is open. That would leave the token readable through
      `getMutationCache()` long after the field said it was gone, which is
      exactly what the sentence at the top of this file promises it is not.
      Taking no variables at all means there is nothing to leave behind.
    */
    mutationFn: () => submitGithubToken(token),
    onSuccess: () => {
      // Only now: a token the server refused is a token whose owner is about
      // to paste a corrected one, and clearing the field would make the fix
      // start from nothing.
      setToken("");
      setNotice({ kind: "done", text: "The token is stored." });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const forget = useMutation({
    mutationFn: disconnectGithub,
    onSuccess: () => {
      setNotice({
        kind: "done",
        text: "The stored credential is gone. GitHub itself stays switched on, so connecting again is all it takes.",
      });
      void invalidate();
    },
    onError: (error: Error) => {
      setNotice({ kind: "problem", text: problemDetail(error) });
    },
  });

  const connection = status.data ?? null;
  const pending = connection?.pending ?? null;
  /*
    A failure the server reports once, said out loud - unless a flow is running
    right now. The connect answer can carry an EARLIER flow's failure beside
    the fresh pending block (the once-reported slot heals on that very read),
    and putting a sentence about the last attempt over the code somebody is
    about to type would be the screen contradicting itself.
  */
  const failure = pending === null ? (connection?.error ?? null) : null;

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-6">
      <header>
        <h1 className="text-display">GitHub</h1>
        <p className="mt-1 text-sm text-slate-500 dark:text-slate-400">
          What team domains are tracked with. Registering one downloads a
          repository's shared knowledge, and keeping it in step needs the same
          connection; nothing else on this instance uses it.
        </p>
      </header>

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

      {status.error && (
        <p
          role="alert"
          className="rounded bg-red-50 px-3 py-2 text-sm text-red-800 dark:bg-red-950 dark:text-red-200"
        >
          {problemDetail(status.error)}
        </p>
      )}

      <section
        aria-labelledby="github-connection"
        className="flex flex-col gap-4 rounded border border-slate-200 p-4 dark:border-slate-800"
      >
        <h2 id="github-connection" className="text-section">
          Connection
        </h2>

        {status.isPending ? (
          <p className="text-sm text-slate-500 dark:text-slate-400">
            Reading the connection
          </p>
        ) : (
          <p className="text-sm">
            {connection?.connected
              ? `Connected as ${connection.user ?? "an account GitHub did not name"} (${connection.tokenStore ?? "an unnamed store"})`
              : "Not connected"}
          </p>
        )}

        {/*
          The server's own sentence about the attempt that just ended. It
          arrives once and is cleared on the read that carried it, so it is
          shown as it was said rather than restated.
        */}
        {failure !== null && (
          <p
            role="alert"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
          >
            {failure}
          </p>
        )}

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
            <span className="text-sm text-slate-500 dark:text-slate-400">
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
      </section>

      <section
        aria-labelledby="github-token"
        className="flex flex-col gap-3 rounded border border-slate-200 p-4 dark:border-slate-800"
      >
        <h2 id="github-token" className="text-section">
          Connect with a token instead
        </h2>
        <p className="text-sm text-slate-500 dark:text-slate-400">
          For a server nobody is sitting in front of. The token is sent once and
          stored by the server; this screen never shows it again.
        </p>
        <form
          className="flex flex-wrap items-end gap-3"
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
        </form>
      </section>
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
        The code is the one thing on this screen somebody has to read off and
        retype, so it is set large and in the mono face every identifier in
        this app wears, with the letters spaced apart.
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
      <p className="text-sm text-slate-500 dark:text-slate-400">
        Waiting for the browser confirmation.
        {minutes > 0 &&
          ` The code is good for about ${String(minutes)} minutes.`}
      </p>
    </div>
  );
}

/**
 * Forgetting the credential, behind a second press.
 *
 * Two steps rather than a browser confirm, for the reason the account screen
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
          <span className="text-sm text-slate-500 dark:text-slate-400">
            Team domains stay registered; syncing them stops until this instance
            is connected again.
          </span>
        </>
      )}
    </div>
  );
}
