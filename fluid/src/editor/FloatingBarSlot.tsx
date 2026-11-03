/**
 * Where a screen puts the floating action bar. Eager and tiny: the bar itself
 * is a chunk of its own (`FloatingBar.tsx`, with the grid's action path behind
 * it), asked for only once a first context exists, so a page nobody selects
 * on never loads it. After that it stays mounted and the context it is handed
 * (null included) decides whether it draws.
 */

import type { ReactElement } from "react";
import { Suspense, lazy, useState } from "react";

import type { FloatingBarProps } from "./FloatingBar";

const FloatingBar = lazy(() => import("./FloatingBar"));

export function FloatingBarSlot(props: FloatingBarProps): ReactElement | null {
  const [seen, setSeen] = useState(false);
  if (!seen && props.context !== null) {
    // Set during render, React's pattern for state derived from a prop.
    setSeen(true);
  }
  if (!seen && props.context === null) return null;
  return (
    <Suspense fallback={null}>
      <FloatingBar {...props} />
    </Suspense>
  );
}
