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
 * in the page's flow, it moves with the text as the page scrolls, and it is
 * placed again when the window or the page changes size. It stays inside
 * the page column (`frame`), so it never covers the threads beside it.
 *
 * A press on it tells the threads (`onPressStart`, `onPressEnd`): a touch
 * can collapse the selection before the click lands, and the threads keep
 * the bar and its passage until then.
 */

import { MessageSquarePlus } from "lucide-react";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
  ReactElement,
  RefObject,
} from "react";
import { useEffect, useLayoutEffect, useReducer, useRef } from "react";

import { FOCUS_RING } from "./primitives";
import { SELECTION_BAR_ATTR } from "./selectionBarMark";

/** The gap between the bar and the selection it stands by, in pixels. */
const GAP = 6;

/**
 * How long a press released on the button waits for its click, in
 * milliseconds. On a touch screen the click can come a task or two after
 * the pointer goes up.
 */
const CLICK_GRACE_MS = 250;

/** The bottom of the app's sticky header, which the bar must not go under. */
function ceiling(): number {
  const header = document.querySelector("[data-app-header]");
  return header === null
    ? 0
    : Math.max(0, header.getBoundingClientRect().bottom);
}

export function SelectionBar({
  range,
  frame,
  onPressStart,
  onPressEnd,
  onLost,
  onComment,
  onDismiss,
}: {
  /** The passage it stands by, measured whenever the bar is placed. */
  range: Range;
  /** The page column it stays inside. */
  frame: RefObject<HTMLElement | null>;
  /** A pointer went down on the bar. */
  onPressStart: () => void;
  /** That press is over: released anywhere, taken over, or the window left. */
  onPressEnd: () => void;
  /** A live edit took the passage away (the range collapsed). */
  onLost: () => void;
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
  const pressEndNow = useRef(onPressEnd);
  const lostNow = useRef(onLost);
  useEffect(() => {
    dismissNow.current = onDismiss;
    pressEndNow.current = onPressEnd;
    lostNow.current = onLost;
  });
  /** Bumped when the window or the page column changes size: place again. */
  const [, replace] = useReducer((n: number) => n + 1, 0);

  useEffect(() => {
    window.addEventListener("resize", replace);
    const column = frame.current;
    const observer =
      column !== null && typeof ResizeObserver === "function"
        ? new ResizeObserver(() => {
            replace();
          })
        : null;
    if (column !== null) {
      observer?.observe(column);
    }
    return () => {
      window.removeEventListener("resize", replace);
      observer?.disconnect();
    };
  }, [frame]);

  // Every press ends when the pointer comes up, wherever that is. Up on the
  // button, the click that follows still finds it, for a moment; up
  // anywhere else (the bar's own padding included), taken over by the
  // browser, or with the window left, it ends at once.
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | undefined;
    const end = () => {
      clearTimeout(timer);
      timer = undefined;
      pressEndNow.current();
    };
    const onUp = (event: PointerEvent) => {
      if (
        event.target instanceof Node &&
        button.current?.contains(event.target) === true
      ) {
        clearTimeout(timer);
        timer = setTimeout(end, CLICK_GRACE_MS);
      } else {
        end();
      }
    };
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", end);
    window.addEventListener("blur", end);
    return () => {
      clearTimeout(timer);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", end);
      window.removeEventListener("blur", end);
    };
  }, []);

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
  // counts as no room); with no room on either side, at the top of the
  // window, over the passage rather than off screen. Never past the page
  // column's edges. Measured in the window, written against the element the
  // bar is placed in.
  useLayoutEffect(() => {
    const el = bar.current;
    if (!el) return;
    // A live edit took the words away: the range is a caret now, with
    // nothing to stand by. The bar goes until the next selection rather
    // than standing at the top of the window.
    if (range.collapsed) {
      el.style.visibility = "hidden";
      lostNow.current();
      return;
    }
    el.style.visibility = "";
    const origin = el.offsetParent;
    if (!(origin instanceof HTMLElement)) return;
    const from = origin.getBoundingClientRect();
    const column = (frame.current ?? origin).getBoundingClientRect();
    const all = range.getBoundingClientRect();
    // Where the passage starts, not the left edge of every line it spans.
    const first = range.getClientRects()[0] ?? all;
    const height = el.offsetHeight;
    const width = el.offsetWidth;
    const top = ceiling();
    const above = all.top - height - GAP;
    const below = all.bottom + GAP;
    const y =
      above >= top
        ? above
        : below + height <= window.innerHeight
          ? below
          : top + GAP;
    const x = Math.max(column.left, Math.min(first.left, column.right - width));
    el.style.top = `${String(y - from.top)}px`;
    el.style.left = `${String(x - from.left)}px`;
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
      onPointerDown={(event) => {
        if (event.isPrimary && event.button === 0) {
          onPressStart();
        }
      }}
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
