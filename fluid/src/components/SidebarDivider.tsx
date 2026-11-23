/**
 * The draggable edge between the domain sidebar and the page.
 *
 * Pointer only, by decision: it takes no keyboard input and is not a tab
 * stop. It still says what it is (a vertical separator with its width) so a
 * screen reader can tell a sidebar has a divider and how wide it is. Pointer
 * capture keeps the drag with the divider when the pointer leaves it, and the
 * same handlers serve a mouse, a pen and a finger.
 */

import { useRef } from "react";
import type { PointerEvent, ReactElement } from "react";

import { NAV_WIDTH_MAX, NAV_WIDTH_MIN, clampNavWidth } from "../navWidth";

export function SidebarDivider({
  width,
  onDrag,
  onSettle,
  onReset,
}: {
  /** The sidebar's width now. */
  width: number;
  /** While dragging: the width the pointer asks for, already in range. */
  onDrag: (width: number) => void;
  /** On release: the width the pointer asked for, not yet in range. */
  onSettle: (raw: number) => void;
  /** Double click: back to the default. */
  onReset: () => void;
}): ReactElement {
  const drag = useRef<{ x: number; width: number; raw: number } | null>(null);

  const move = (event: PointerEvent<HTMLDivElement>) => {
    const start = drag.current;
    if (start === null) {
      return;
    }
    start.raw = start.width + event.clientX - start.x;
    onDrag(clampNavWidth(start.raw));
  };
  const end = (event: PointerEvent<HTMLDivElement>, settle: boolean) => {
    const start = drag.current;
    drag.current = null;
    if (start === null) {
      return;
    }
    try {
      event.currentTarget.releasePointerCapture(event.pointerId);
    } catch {
      // Already released, or never captured.
    }
    if (settle) {
      onSettle(start.raw);
    } else {
      onDrag(start.width);
    }
  };

  return (
    <div
      role="separator"
      aria-orientation="vertical"
      aria-label="Sidebar width"
      aria-valuenow={width}
      aria-valuemin={NAV_WIDTH_MIN}
      aria-valuemax={NAV_WIDTH_MAX}
      className="group absolute inset-y-0 -right-1 z-10 flex w-2 cursor-col-resize touch-none justify-center print:hidden"
      onPointerDown={(event) => {
        if (event.button !== 0) {
          return;
        }
        event.preventDefault();
        drag.current = { x: event.clientX, width, raw: width };
        try {
          event.currentTarget.setPointerCapture(event.pointerId);
        } catch {
          // An environment without pointer capture drags while over it.
        }
      }}
      onPointerMove={move}
      onPointerUp={(event) => {
        end(event, true);
      }}
      onPointerCancel={(event) => {
        end(event, false);
      }}
      onDoubleClick={onReset}
    >
      <span
        aria-hidden="true"
        className="h-full w-px bg-transparent group-hover:bg-accent-500 group-active:bg-accent-500"
      />
    </div>
  );
}
