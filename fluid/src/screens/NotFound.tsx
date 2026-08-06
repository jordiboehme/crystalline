/**
 * An address this app does not answer at.
 *
 * Inside the layout on purpose: a mistyped URL is a wrong turn, not a broken
 * app, so the frame and the domain list stay where they are.
 */

import { Link } from "react-router";

export default function NotFound() {
  return (
    <div>
      <h1 className="text-xl font-semibold">Nothing here</h1>
      <p className="mt-2 text-sm text-slate-600 dark:text-slate-400">
        Fluid has no screen at this address.{" "}
        <Link to="/" className="underline underline-offset-2">
          Back to the home screen
        </Link>
      </p>
    </div>
  );
}
