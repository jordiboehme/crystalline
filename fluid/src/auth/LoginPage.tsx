/**
 * The login screen.
 *
 * The one screen outside the app frame, so the one screen that has to say what
 * this is, and it has to say two things rather than one: which product this is,
 * and which of its faces you are standing at. So the card reads top to bottom
 * as the gem, CRYSTALLINE, Fluid, and the line about what the pairing is for.
 * Crystalline is what was learned and kept; fluid is what a person and a model
 * bring to the moment; this is where the two think in the same place. The line
 * says that once, quietly, and no other screen repeats it.
 *
 * The gem is the CLI banner's own gem, cut down to a size that fits a login
 * card. The full block-letter wordmark beside it in the README and the serve
 * banner is 87 columns wide, which is more than twice this card at any legible
 * monospace size, so the word is set as ordinary letter-spaced text instead:
 * readable beats faithful, and a wordmark that is real text is a wordmark a
 * screen reader can read. Only the gem is art, and art is what `aria-hidden`
 * is for.
 *
 * On an instance with no accounts at all there is nothing to log in to, so the
 * card carries the first-run wizard instead of the credentials form. Same
 * route, same gem, same wordmark: the door does not move because the instance
 * is new, and once the first admin exists the wizard is gone for good.
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
import { FirstRunSetup } from "./FirstRunSetup";
import type { FromLocation } from "./RequireAuth";
import { CARD_FIELD, CARD_LABEL } from "./card";
import { LOGIN_MUTATION_KEY } from "./keys";

/**
 * The gem from the CLI's startup banner, at a sixth of its width.
 *
 * Same silhouette and the same shading vocabulary the terminal draws it with -
 * a flat table, a faceted crown, then the pavilion tapering to a point - so
 * somebody who has seen the banner recognizes this, and somebody who has not
 * still sees a cut stone. Held as one string rather than assembled at render:
 * this route is eager, and a constant costs a few hundred bytes once.
 */
const GEM = ` ▄▄▄▄▄▄▄▄▄▄▄
▐░░▒▒▓█▓▒▒░░▌
  ▀█░▒█▒░█▀
   ▀█▒█▒█▀
    ▀███▀
      ▀`;

export default function LoginPage() {
  const { user, capabilities, login } = useAuth();
  const navigate = useNavigate();
  const location = useLocation();
  const nameField = useId();
  const passwordField = useId();
  const [name, setName] = useState("");
  const [password, setPassword] = useState("");
  // What the setup endpoint said when it refused for good, kept here rather
  // than in the wizard: the wizard is what disappears when the probe is
  // re-read, and its last words are the reason this form is on screen instead.
  const [setupClosed, setSetupClosed] = useState<string | null>(null);

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

  // Nobody has an account here yet, so there is nothing to log in to: the card
  // asks for the first admin instead.
  const firstRun = capabilities.needsSetup && !user;

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
          The banner gem, decorative on purpose: the word it stands over is
          right beneath it as text, so a screen reader is spared six lines of
          block characters that say nothing it can pronounce.

          Centered as one block (`mx-auto w-fit`) rather than line by line: the
          taper is drawn with leading spaces, so centering each line on its own
          width would walk the point three columns to the right of the crown it
          hangs under.
        */}
        <pre
          aria-hidden="true"
          className="text-caption mx-auto mb-3 w-fit leading-none font-mono text-accent-600 select-none dark:text-accent-400"
        >
          {GEM}
        </pre>
        {/*
          Text, not art: the product's name has to survive being listened to.
          The letter spacing is what makes it read as the wordmark from the
          terminal banner rather than as a heading, which is what the name
          below it is.
        */}
        <p className="text-caption text-center font-mono tracking-[0.35em] text-slate-600 select-none dark:text-slate-400">
          CRYSTALLINE
        </p>
        <h1 className="text-display mt-1 text-center text-slate-900 dark:text-slate-50">
          Fluid
        </h1>
        <p className="mt-1 text-center text-sm text-slate-600 dark:text-slate-400">
          mind-meld your fluid thoughts with your AI's crystalline intelligence
        </p>

        {/*
          Why this card changed under somebody who was mid-wizard: another
          browser, or a terminal, created the first admin first. The server's
          own sentence says it, and it stays until this tab is done with it.

          The region is mounted for the whole of the first-run flow rather than
          created with its text, because it is filled in the same commit that
          swaps the wizard for the login form and moves focus into it: a live
          region born with its content, next to a focus move, is announced
          unreliably. An instance that is already set up never enters this flow
          and gets the card exactly as it was.
        */}
        {(firstRun || setupClosed !== null) && (
          <p
            role="status"
            className={
              setupClosed === null
                ? undefined
                : "mt-6 rounded border border-slate-300 bg-slate-100 px-3 py-2 text-sm text-slate-700 dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300"
            }
          >
            {setupClosed}
          </p>
        )}

        {firstRun ? (
          <FirstRunSetup onGone={setSetupClosed} />
        ) : (
          <form
            className="mt-8 flex flex-col gap-4"
            onSubmit={(event) => {
              event.preventDefault();
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
                autoComplete="current-password"
                required
                value={password}
                onChange={(event) => {
                  setPassword(event.target.value);
                }}
                className={CARD_FIELD}
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
              The app's own primary tier, in the app's own accent: this screen
              is the first thing anybody sees of it, and it was the last one
              still wearing the blue the tiers replaced.
            */}
            <button
              type="submit"
              disabled={attempt.isPending}
              className={`py-2 ${BUTTON.primary}`}
            >
              Log in
            </button>
          </form>
        )}

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
