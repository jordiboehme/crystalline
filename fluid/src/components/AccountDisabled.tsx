/**
 * What a disabled account sees.
 *
 * The 403 this answers comes from an identity the server already resolved and
 * then refused, which on this API means a trusted-header account that has been
 * disabled. A login form would be useless to them - the proxy supplies the
 * identity, so signing in cannot change anything, and a redirect to `/login`
 * would bounce straight back here. So the app stops, says what happened in the
 * server's own words, and names the one thing that can fix it.
 */

export interface AccountDisabledProps {
  /** The server's problem detail, shown as written. */
  detail: string;
}

export function AccountDisabled({ detail }: AccountDisabledProps) {
  return (
    <main className="flex min-h-screen items-center justify-center bg-slate-50 p-6 dark:bg-slate-950">
      <div className="w-full max-w-md rounded border border-slate-300 bg-white p-6 dark:border-slate-700 dark:bg-slate-900">
        <h1 className="text-lg font-semibold text-slate-900 dark:text-slate-50">
          This account is disabled
        </h1>
        <p className="mt-2 text-sm text-slate-600 dark:text-slate-400">
          Crystalline refused this identity. An administrator can re-enable the
          account.
        </p>
        <p className="mt-3 rounded bg-slate-100 px-3 py-2 font-mono text-xs text-slate-700 dark:bg-slate-800 dark:text-slate-300">
          {detail}
        </p>
      </div>
    </main>
  );
}
