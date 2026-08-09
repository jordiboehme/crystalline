/**
 * The launcher's own seam: what every "New engram" button imports.
 *
 * The dialog's own weight - the Radix dialog primitive, the folder-picker
 * tree walk, the create mutation - lives behind a lazy import in
 * `CreateEngramDialogBody.tsx`, so an app that never opens the dialog never
 * pays for it. `open` is always true while this is mounted; the launcher
 * mounts it only once "New engram" has been clicked and unmounts it again
 * through `onClose`.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const CreateEngramDialogBody = lazy(() => import("./CreateEngramDialogBody"));

export interface CreateEngramDialogProps {
  domain: string;
  /** The folder the launcher was looking at; "" is the root. */
  initialFolder: string;
  onClose: () => void;
}

export function CreateEngramDialog(
  props: CreateEngramDialogProps,
): ReactElement {
  return (
    <Suspense fallback={null}>
      <CreateEngramDialogBody {...props} />
    </Suspense>
  );
}
