/**
 * Where a screen puts the element popover host. Small on purpose: the host
 * and its popovers are a chunk of their own (`ElementPopover.tsx`), asked for
 * only once a first request exists, so a page nobody clicks a link on never
 * loads it. After that it stays mounted and the request it is handed (null
 * included) decides whether it draws.
 */

import type { EditorView } from "@codemirror/view";
import type { ReactElement } from "react";
import { Suspense, lazy, useState } from "react";

import type { ElementRequest } from "../elementEdit";
import type { TitleOption } from "../wikilinkChips";

const ElementPopover = lazy(() => import("./ElementPopover"));

export interface ElementPopoverProps {
  /** The main view; the request names the view it came from. */
  main: EditorView | null;
  /** The open request, as the buffer's element edit listener told it. */
  request: ElementRequest | null;
  /**
   * The title matches for a wikilink target, as the `[[` completion offers
   * them (`wikilinkTitleOptions`); absent on a Reflection page. Handed in by
   * the screen, which holds the query client.
   */
  titles?: (term: string) => Promise<TitleOption[]>;
}

export function ElementPopoverSlot(
  props: ElementPopoverProps,
): ReactElement | null {
  const [seen, setSeen] = useState(false);
  if (!seen && props.request !== null) {
    // Set during render, React's pattern for state derived from a prop.
    setSeen(true);
  }
  if (!seen && props.request === null) return null;
  return (
    <Suspense fallback={null}>
      <ElementPopover {...props} />
    </Suspense>
  );
}
