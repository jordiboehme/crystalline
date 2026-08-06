/**
 * What a browser sees when Crystalline is not answering.
 *
 * A blank page is the failure this replaces: a request that never arrived
 * leaves the app with nothing to render, and "nothing" reads as a broken app
 * rather than an unreachable server. This says which of the two it is and
 * offers the only useful next move.
 */

export interface ServerDownProps {
  /** What went wrong, in the client's or the server's own words. */
  detail?: string;
  /** Ask again. Omitted where there is nothing sensible to retry. */
  onRetry?: () => void;
}

export function ServerDown({ detail, onRetry }: ServerDownProps) {
  return (
    <main className="flex min-h-screen items-center justify-center bg-slate-50 p-6 dark:bg-slate-950">
      <div className="w-full max-w-md rounded border border-amber-300 bg-amber-50 p-6 dark:border-amber-900 dark:bg-amber-950">
        <h1 className="text-lg font-semibold text-amber-900 dark:text-amber-100">
          Cannot reach Crystalline
        </h1>
        <p className="mt-2 text-sm text-amber-900 dark:text-amber-200">
          Fluid reached no answer from the server it talks to. It may be down,
          or this browser may be offline.
        </p>
        {detail && (
          <p className="mt-3 rounded bg-amber-100 px-3 py-2 font-mono text-xs text-amber-900 dark:bg-amber-900 dark:text-amber-100">
            {detail}
          </p>
        )}
        {onRetry && (
          <button
            type="button"
            onClick={onRetry}
            className="mt-4 rounded bg-amber-900 px-3 py-2 text-sm font-medium text-amber-50 hover:bg-amber-800 focus-visible:ring-2 focus-visible:ring-amber-500 focus-visible:outline-none dark:bg-amber-100 dark:text-amber-950 dark:hover:bg-white"
          >
            Try again
          </button>
        )}
      </div>
    </main>
  );
}
