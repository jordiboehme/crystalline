/**
 * What a person sees when their first sign-in cannot make an account yet. The
 * server is fine, and trying again would not help: only an admin can act, and the
 * sentence says to ask one. It names nothing about the name's past.
 */

export interface NoAccountYetProps {
  /** The server's own sentence. */
  detail: string;
}

export function NoAccountYet({ detail }: NoAccountYetProps) {
  return (
    <main className="flex min-h-screen items-center justify-center bg-slate-50 p-6 dark:bg-slate-950">
      <div className="w-full max-w-md rounded border border-slate-300 bg-white p-6 dark:border-slate-700 dark:bg-slate-900">
        <h1 className="text-lg font-semibold text-slate-900 dark:text-slate-50">
          No account yet
        </h1>
        <p className="mt-2 text-sm text-slate-600 dark:text-slate-400">
          {detail}
        </p>
      </div>
    </main>
  );
}
