/**
 * The launcher's own seam for the shortcut map.
 *
 * The dialog primitive lives behind a lazy import in `HelpOverlayBody.tsx`,
 * the same split every other dialog in this app uses: the frame mounts this on
 * every screen, so a help overlay nobody opened must not put Radix's dialog
 * code in the chunk that draws the first page.
 */

import type { ReactElement } from "react";
import { Suspense, lazy } from "react";

const HelpOverlayBody = lazy(() => import("./HelpOverlayBody"));

export interface HelpOverlayProps {
  open: boolean;
  onClose: () => void;
}

export function HelpOverlay({ open, onClose }: HelpOverlayProps): ReactElement {
  return (
    <Suspense fallback={null}>
      {open && <HelpOverlayBody onClose={onClose} />}
    </Suspense>
  );
}
