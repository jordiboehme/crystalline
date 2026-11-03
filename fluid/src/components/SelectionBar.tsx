/**
 * The bar at a selection on a Reflection page: a small `role="toolbar"`
 * named "Selection actions" that stands just above the selected text (below
 * it when there is no room above), with one action, "Comment on selection".
 *
 * It follows the editor's Quick format bar (`editor/FloatingBar.tsx`) in look
 * and keyboard model, without its editor: it never takes focus on its own
 * (its button is out of the Tab order), Alt+F10 moves focus to it, and
 * Escape hands focus back to where it came from with the page's selection as
 * it was. Its mousedown is cancelled, so a click keeps the selection.
 *
 * The threads decide when it is drawn (`ReflectionThreads`): they read the
 * selection, and the bar only stands where they say. It is mounted only
 * while it is shown, so Alt+F10 does nothing without it. Placed absolutely
 * in the page's flow, it moves with the text as the page scrolls.
 */

import { MessageSquarePlus } from "lucide-react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
  ReactElement,
} from "react";
import { useEffect, useLayoutEffect, useRef } from "react";

import { FOCUS_RING } from "./primitives";
import type { SelectionBox } from "./selectionBox";
import { SELECTION_BAR_ATTR } from "./selectionBox";

/** The gap between the bar and the selection it stands by, in pixels. */
const GAP = 6;

/** The bottom of the app's sticky header, which the bar must not go under. */
function ceiling(): number {
  for (const header of document.querySelectorAll("header")) {
    const position = getComputedStyle(header).position;
    if (position === "sticky" || position === "fixed") {
      return Math.max(0, header.getBoundingClientRect().bottom);
    }
  }
  return 0;
}

export function SelectionBar({
  box,
  onComment,
  onDismiss,
}: {
  box: SelectionBox;
  onComment: () => void;
  /** Escape: the bar goes until the selection changes. */
  onDismiss: () => void;
}): ReactElement {
  const bar = useRef<HTMLDivElement>(null);
  const button = useRef<HTMLButtonElement>(null);
  /** Where Alt+F10 came from, and the selection then: Escape returns there. */
  const origin = useRef<{ focus: Element | null; range: Range | null } | null>(
    null,
  );
  const dismissNow = useRef(onDismiss);
  useEffect(() => {
    dismissNow.current = onDismiss;
  });

  // Alt+F10 from anywhere on the page moves focus here; Escape outside the
  // bar sends it away.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      const inBar =
        event.target instanceof Node &&
        bar.current?.contains(event.target) === true;
      if (event.altKey && event.key === "F10") {
        event.preventDefault();
        if (inBar) return;
        const selection = window.getSelection();
        origin.current = {
          focus: document.activeElement,
          range:
            selection !== null && selection.rangeCount > 0
              ? selection.getRangeAt(0).cloneRange()
              : null,
        };
        button.current?.focus();
      } else if (event.key === "Escape" && !inBar) {
        dismissNow.current();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("keydown", onKey);
    };
  }, []);

  // Above the selection, or below it with no room above (the sticky header
  // counts as no room); never over it, and never past its frame's edges.
  useLayoutEffect(() => {
    const el = bar.current;
    const parent = el?.offsetParent;
    if (!el || !(parent instanceof HTMLElement)) return;
    const frame = parent.getBoundingClientRect();
    const top = box.top - window.scrollY;
    const bottom = box.bottom - window.scrollY;
    const left = box.left - window.scrollX;
    const height = el.offsetHeight;
    const width = el.offsetWidth;
    const y =
      top - height - GAP >= ceiling() ? top - height - GAP : bottom + GAP;
    const x = Math.max(
      frame.left,
      Math.min(left, frame.left + frame.width - width),
    );
    el.style.top = `${String(y - frame.top)}px`;
    el.style.left = `${String(x - frame.left)}px`;
  });

  // Focus back where Alt+F10 found it (nowhere in particular: the page
  // itself), then the selection as it was, then the bar goes.
  const back = () => {
    const from = origin.current;
    origin.current = null;
    const to = from?.focus;
    if (
      to instanceof HTMLElement &&
      to !== document.body &&
      to.isConnected &&
      !bar.current?.contains(to)
    ) {
      to.focus();
    } else {
      button.current?.blur();
    }
    const selection = window.getSelection();
    if (
      from?.range &&
      selection !== null &&
      (selection.rangeCount === 0 || selection.isCollapsed)
    ) {
      selection.removeAllRanges();
      selection.addRange(from.range);
    }
    onDismiss();
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const stops = [...(bar.current?.querySelectorAll("button") ?? [])];
    const index = stops.indexOf(event.target as HTMLButtonElement);
    const go = (to: number) => {
      stops[(to + stops.length) % stops.length]?.focus();
    };
    switch (event.key) {
      case "ArrowRight":
        go(index + 1);
        break;
      case "ArrowLeft":
        go(index - 1);
        break;
      case "Home":
        go(0);
        break;
      case "End":
        go(stops.length - 1);
        break;
      case "Escape":
        back();
        break;
      default:
        return;
    }
    event.preventDefault();
    event.stopPropagation();
  };

  const keepSelection = (event: MouseEvent) => {
    event.preventDefault();
  };

  return (
    <div
      ref={bar}
      role="toolbar"
      aria-label="Selection actions"
      aria-orientation="horizontal"
      {...{ [SELECTION_BAR_ATTR]: "" }}
      onMouseDown={keepSelection}
      onKeyDown={onKeyDown}
      className="absolute z-20 flex items-center rounded-md border border-hair bg-raised p-0.5 shadow-pop print:hidden"
    >
      <button
        ref={button}
        type="button"
        tabIndex={-1}
        aria-label="Comment on selection"
        onClick={onComment}
        className={`inline-flex h-8 items-center gap-1.5 rounded px-2 text-[13px] font-medium text-ink hover:bg-sunken ${FOCUS_RING}`}
      >
        <MessageSquarePlus aria-hidden size={16} className="shrink-0" />
        Comment
      </button>
    </div>
  );
}
