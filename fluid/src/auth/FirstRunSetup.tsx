/**
 * The first-run wizard: the login card's other form.
 *
 * An instance nobody has an account on cannot be logged into, so on that one
 * instance the login screen asks for a first admin instead. It is the same
 * card, under the same gem, on the same route - there is no second door to
 * find, and once the account exists this form is gone forever.
 *
 * Two things here are load-bearing. The confirmation field is checked in the
 * browser, because a mistyped password nobody can see is a locked instance
 * that has to be repaired from a terminal. And the setup-token field appears
 * only when the server's refusal carried the `token_required` member: a
 * loopback visitor never needs a token, and a daemon that never printed one
 * refuses without the member, so offering the field on the strength of the
 * refusal's prose would hand somebody an input that cannot help them. The
 * prose is shown, word for word, and decides nothing.
 */

import { useMutation } from "@tanstack/react-query";
import { useId, useState } from "react";

import { ApiProblem } from "../api/client";
import { BUTTON } from "../components/primitives";
import { useAuth } from "./AuthContext";
import { CARD_FIELD, CARD_LABEL } from "./card";
import { SETUP_MUTATION_KEY } from "./keys";

/** Whether this refusal is one a setup token could answer. */
function asksForAToken(error: unknown): boolean {
  return (
    error instanceof ApiProblem &&
    error.status === 403 &&
    error.extensions.token_required === true
  );
}

export function FirstRunSetup({
  onGone,
}: {
  /**
   * Someone else created the first admin while this form was open. The
   * server's own explanation goes up to the login screen, which is what the
   * card becomes once the re-read probe lands.
   */
  onGone: (detail: string) => void;
}) {
  const { setup } = useAuth();
  const nameField = useId();
  const passwordField = useId();
  const confirmField = useId();
  const tokenField = useId();
  const tokenHelp = useId();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [token, setToken] = useState("");
  const [tokenAsked, setTokenAsked] = useState(false);

  const mismatch = password !== confirm;

  const attempt = useMutation({
    // Under the auth family's key, so the expired-session recovery leaves it
    // alone: there is no session to have expired yet.
    mutationKey: SETUP_MUTATION_KEY,
    mutationFn: () =>
      setup(name, password, tokenAsked ? token.trim() || undefined : undefined),
    onError: (error: Error) => {
      if (asksForAToken(error)) {
        setTokenAsked(true);
      }
      if (error instanceof ApiProblem && error.status === 410) {
        onGone(error.detail);
      }
    },
  });

  const problem = attempt.error;
  const message =
    problem instanceof ApiProblem
      ? // The 410 is the one refusal this form does not get to answer, so its
        // detail is shown by the login form it collapses into rather than
        // twice, here and there.
        problem.status === 410
        ? null
        : problem.detail
      : problem
        ? "the account could not be created"
        : null;

  return (
    <>
      <h2 className="text-title mt-8 text-center text-slate-900 dark:text-slate-50">
        Welcome
      </h2>
      <p className="mt-1 text-center text-sm text-slate-600 dark:text-slate-400">
        Nobody has an account on this instance yet. The one you create here is
        its first admin.
      </p>

      <form
        className="mt-6 flex flex-col gap-4"
        onSubmit={(event) => {
          event.preventDefault();
          // Belt as well as braces: the button is disabled on a mismatch, and
          // a form can still be submitted from the keyboard.
          if (mismatch) {
            return;
          }
          attempt.mutate();
        }}
      >
        <div className="flex flex-col gap-1">
          <label htmlFor={nameField} className={CARD_LABEL}>
            Name
          </label>
          <input
            id={nameField}
            name="name"
            autoComplete="username"
            autoFocus
            required
            value={name}
            onChange={(event) => {
              setName(event.target.value);
            }}
            className={CARD_FIELD}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label htmlFor={passwordField} className={CARD_LABEL}>
            Password
          </label>
          <input
            id={passwordField}
            name="password"
            type="password"
            // Both password fields say `new-password`: this account does not
            // exist yet, so a password manager should be offering to make one
            // rather than filling in one it remembers.
            autoComplete="new-password"
            required
            value={password}
            onChange={(event) => {
              setPassword(event.target.value);
            }}
            className={CARD_FIELD}
          />
        </div>

        <div className="flex flex-col gap-1">
          <label htmlFor={confirmField} className={CARD_LABEL}>
            Confirm password
          </label>
          <input
            id={confirmField}
            name="confirm"
            type="password"
            autoComplete="new-password"
            required
            value={confirm}
            onChange={(event) => {
              setConfirm(event.target.value);
            }}
            className={CARD_FIELD}
          />
          {confirm !== "" && mismatch && (
            <p role="alert" className="text-sm text-red-700 dark:text-red-300">
              the passwords do not match
            </p>
          )}
        </div>

        {tokenAsked && (
          <div className="flex flex-col gap-1">
            <label htmlFor={tokenField} className={CARD_LABEL}>
              Setup token
            </label>
            <input
              id={tokenField}
              name="token"
              autoComplete="off"
              autoFocus
              required
              aria-describedby={tokenHelp}
              value={token}
              onChange={(event) => {
                setToken(event.target.value);
              }}
              className={`font-mono ${CARD_FIELD}`}
            />
            <p
              id={tokenHelp}
              className="text-sm text-slate-600 dark:text-slate-400"
            >
              crystalline serve prints this token once at startup: look in the
              terminal it was started from, or in the daemon log when it runs in
              the background.
            </p>
          </div>
        )}

        {message && (
          <p
            role="alert"
            className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
          >
            {message}
          </p>
        )}

        <button
          type="submit"
          disabled={attempt.isPending || mismatch}
          className={`py-2 ${BUTTON.primary}`}
        >
          Create admin account
        </button>
      </form>
    </>
  );
}
