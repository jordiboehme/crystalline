/**
 * The diff pane behind a lazy import.
 *
 * A module of its own because the pane's own module default-exports the
 * component the import resolves to, and the seam every screen mounts is a
 * named one.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const Pane = lazy(() => import("./DiffPane"));

/** The diff pane behind a lazy import: the merge package never rides the entry bundle. */
export function DiffPane(props: {
  domain: string;
  path: string;
  headingId: string;
}): ReactElement {
  return (
    <Suspense
      fallback={
        <p className="text-sm text-slate-500 dark:text-slate-400">
          Reading both sides
        </p>
      }
    >
      <Pane {...props} />
    </Suspense>
  );
}
