/**
 * The launcher's own seam for registering a domain.
 *
 * The dialog primitive lives behind a lazy import in
 * `CreateDomainDialogBody.tsx`, so a session that never registers a domain -
 * which is every session but an admin's, and most of those - never pays for
 * Radix's dialog code or for the form inside it. `open` is always true while
 * this is mounted; the launcher mounts it only once "New domain" has been
 * pressed and unmounts it again through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const CreateDomainDialogBody = lazy(() => import("./CreateDomainDialogBody"));

export interface CreateDomainDialogProps {
  onClose: () => void;
}

export function CreateDomainDialog(
  props: CreateDomainDialogProps,
): ReactElement {
  return (
    <Suspense
      fallback={
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-900/40">
          <p className="rounded border border-slate-200 bg-white px-4 py-2 text-sm text-slate-600 shadow-xl dark:border-slate-700 dark:bg-slate-900 dark:text-slate-300">
            Opening the new domain dialog
          </p>
        </div>
      }
    >
      <CreateDomainDialogBody {...props} />
    </Suspense>
  );
}
