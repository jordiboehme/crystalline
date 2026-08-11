/**
 * The login screen.
 *
 * The one screen outside the app frame, so the one screen that has to say what
 * this is: the mark, the name and a line about the pairing the app is for.
 * Crystalline is what was learned and kept; fluid is what a person and a model
 * bring to the moment; this is where the two think in the same place. The line
 * says that once, quietly, and no other screen repeats it.
 *
 * The one rule worth stating: when the server refuses, its own `detail` is
 * what is shown, word for word. That text is product copy written where the
 * decision was made ("the name or password is wrong" is deliberately one
 * message for every way a login can fail, so nothing is learned about which
 * accounts exist), and a house message pasted over it would say less and
 * sometimes say something untrue.
 */

import { useMutation } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Link, Navigate, useLocation, useNavigate } from "react-router";

import { ApiProblem } from "../api/client";
import { BUTTON, FOCUS_RING } from "../components/primitives";
import { useAuth } from "./AuthContext";
import type { FromLocation } from "./RequireAuth";
import { LOGIN_MUTATION_KEY } from "./keys";

export default function LoginPage() {
  const { user, capabilities, login } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const nameField = useId();
  const passwordField = useId();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");

  // Where to go once this works: back to whatever `RequireAuth` interrupted,
  // or the home screen for someone who came here directly.
  const from = (location.state as FromLocation | null)?.from;
  const destination = from ? `${from.pathname}${from.search}${from.hash}` : "/";

  const attempt = useMutation({
    // Named so the expired-session recovery in the query layer leaves it
    // alone: a refused login is not an expired session.
    mutationKey: LOGIN_MUTATION_KEY,
    mutationFn: () => login(name, password),
    onSuccess: () => {
      void navigate(destination, { replace: true });
    },
  });

  // Someone who is already signed in has no business on this screen; the
  // anonymous viewer does, since logging in is how they stop being anonymous.
  if (user && !attempt.isPending) {
    return <Navigate to={destination} replace />;
  }

  const problem = attempt.error;
  const message =
    problem instanceof ApiProblem
      ? problem.detail
      : problem
        ? "the login could not be sent"
        : null;

  return (
    <main className="flex min-h-screen items-center justify-center bg-slate-50 p-6 dark:bg-slate-950">
      <div className="w-full max-w-sm">
        {/*
          A cut gem in the CLI's own ASCII spirit, and decorative on purpose:
          it is the app's mark, and the name it stands over is right beneath
          it, so a screen reader is spared six lines of slashes.
        */}
        <pre
          aria-hidden="true"
          className="text-caption mb-4 text-center leading-tight font-mono text-accent-600 select-none dark:text-accent-400"
        >
          {`   /\\
  /  \\
 /____\\
 \\    /
  \\  /
   \\/`}
        </pre>
        <h1 className="text-display text-center text-slate-900 dark:text-slate-50">
          Fluid
        </h1>
        <p className="mt-1 text-center text-sm text-slate-600 dark:text-slate-400">
          where fluid thought and crystalline memory think as one
        </p>

        <form
          className="mt-8 flex flex-col gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            attempt.mutate();
          }}
        >
          <div className="flex flex-col gap-1">
            <label
              htmlFor={nameField}
              className="text-sm font-medium text-slate-700 dark:text-slate-300"
            >
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
              className={`rounded border border-slate-300 bg-white px-3 py-2 text-slate-900 outline-none dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100 ${FOCUS_RING}`}
            />
          </div>

          <div className="flex flex-col gap-1">
            <label
              htmlFor={passwordField}
              className="text-sm font-medium text-slate-700 dark:text-slate-300"
            >
              Password
            </label>
            <input
              id={passwordField}
              name="password"
              type="password"
              autoComplete="current-password"
              required
              value={password}
              onChange={(event) => {
                setPassword(event.target.value);
              }}
              className={`rounded border border-slate-300 bg-white px-3 py-2 text-slate-900 outline-none dark:border-slate-700 dark:bg-slate-900 dark:text-slate-100 ${FOCUS_RING}`}
            />
          </div>

          {message && (
            <p
              role="alert"
              className="rounded border border-red-300 bg-red-50 px-3 py-2 text-sm text-red-800 dark:border-red-900 dark:bg-red-950 dark:text-red-200"
            >
              {message}
            </p>
          )}

          {/*
            The app's own primary tier, in the app's own accent: this screen is
            the first thing anybody sees of it, and it was the last one still
            wearing the blue the tiers replaced.
          */}
          <button
            type="submit"
            disabled={attempt.isPending}
            className={`py-2 ${BUTTON.primary}`}
          >
            Log in
          </button>
        </form>

        {capabilities.anonymous && (
          <p className="mt-6 text-sm text-slate-600 dark:text-slate-400">
            This instance allows reading without an account.{" "}
            <Link
              to="/"
              className={`rounded underline underline-offset-2 hover:text-slate-900 dark:hover:text-slate-100 ${FOCUS_RING}`}
            >
              Browse anonymously
            </Link>
          </p>
        )}
      </div>
    </main>
  );
}
