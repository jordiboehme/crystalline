/**
 * One diagram, on its own, in the whole window, with pan and zoom.
 *
 * An in-window layer rather than the browser's Fullscreen API as the primary
 * path, because element fullscreen does not exist on iOS Safari at all: a
 * layer over the page is the one shape that works everywhere, and true
 * fullscreen is offered on top of it where the browser has it.
 *
 * Pan and zoom live here and nowhere else. Inline, a diagram that zoomed on a
 * plain wheel would eat the page's scrolling, which is the trap every survey
 * of this feature names; in here there is no page behind it to scroll, so the
 * wheel is free. The library arrives on first open through a dynamic import,
 * so a reader who never opens a diagram never downloads it.
 *
 * The dialog contract is hand-rolled rather than taken from the app's dialog
 * primitive: this layer needs the wheel, pointer and key events of its stage
 * to reach the pan-and-zoom instance untouched, and a primitive that manages
 * scrolling and pointer capture for the page is exactly what must not sit
 * between the two. What it owes in return is written out below - a modal role
 * with a name, focus moved in and handed back, Tab kept inside, the page's
 * scrolling locked while it is open.
 */

import {
  Maximize2,
  Minimize2,
  RotateCcw,
  X,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import type { PanzoomObject } from "@panzoom/panzoom";
import { useCallback, useEffect, useRef, useState } from "react";
import type { KeyboardEvent, ReactElement, RefObject } from "react";
import { createPortal } from "react-dom";

import { FOCUS_RING } from "./primitives";

/**
 * What Tab may land on inside this layer. Wider than the bar's own buttons,
 * because the diagram's markup can carry links of its own.
 */
const FOCUS_STOPS = 'button, a[href], [tabindex]:not([tabindex="-1"])';

/** How far an arrow key moves the drawing, in pixels of the stage. */
const ARROW_STEP_PX = 40;

/**
 * Whether the browser is showing something fullscreen right now.
 *
 * `Boolean` rather than a comparison against `null`: a browser without the
 * Fullscreen API has no `fullscreenElement` at all, and `undefined !== null`
 * would read as "yes, and please leave it" on the way out.
 */
function inFullscreen(): boolean {
  return Boolean(document.fullscreenElement);
}

/** How much of the stage a fitted diagram fills, so it never touches the edges. */
const FIT_MARGIN = 0.95;

/**
 * A click that lands this far from where the pointer went down is a click; any
 * further and it was a drag that happened to end on the backdrop, which must
 * not close the thing the reader was busy panning.
 */
const CLICK_SLOP_PX = 5;

const BAR_BUTTON = `inline-flex h-8 w-8 shrink-0 items-center justify-center rounded text-slate-600 hover:bg-slate-100 dark:text-slate-300 dark:hover:bg-slate-800 ${FOCUS_RING}`;

export interface DiagramOverlayProps {
  /** The diagram's markup, exactly as mermaid rendered it. */
  svg: string;
  onClose: () => void;
  /** The control that opened this, where the keyboard goes when it closes. */
  returnFocusTo: RefObject<HTMLElement | null>;
}

/** The drawing's own size, for the fit: the viewBox is what mermaid always writes. */
function naturalSize(
  svg: SVGElement,
): { width: number; height: number } | null {
  const viewBox = svg.getAttribute("viewBox");
  if (viewBox === null) {
    return null;
  }
  const parts = viewBox.trim().split(/[\s,]+/);
  if (parts.length !== 4) {
    return null;
  }
  const width = Number(parts[2]);
  const height = Number(parts[3]);
  if (!Number.isFinite(width) || !Number.isFinite(height)) {
    return null;
  }
  return width > 0 && height > 0 ? { width, height } : null;
}

export default function DiagramOverlay({
  svg,
  onClose,
  returnFocusTo,
}: DiagramOverlayProps): ReactElement {
  const overlayRef = useRef<HTMLDivElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const hostRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const instanceRef = useRef<PanzoomObject | null>(null);
  const pressedAt = useRef<{ x: number; y: number } | null>(null);
  const [fullscreen, setFullscreen] = useState(false);

  // Read at render rather than remembered: a browser that has no element
  // fullscreen must not be offered a button that would do nothing, and jsdom
  // (where `fullscreenEnabled` is simply absent) is that browser.
  const canFullscreen = document.fullscreenEnabled === true;

  /**
   * Fit and centre. The drawing's own size comes from its viewBox and the room
   * from the stage; where either is unknown - a layout-less test environment
   * says zero to everything - the diagram is left at its natural size rather
   * than scaled by a number derived from nothing.
   */
  const fit = useCallback((instance: PanzoomObject) => {
    const stage = stageRef.current;
    const drawing = hostRef.current?.querySelector("svg");
    if (stage === null || drawing === null || drawing === undefined) {
      return;
    }
    const natural = naturalSize(drawing);
    const room = stage.getBoundingClientRect();
    if (natural === null || room.width <= 0 || room.height <= 0) {
      return;
    }
    const scale =
      Math.min(room.width / natural.width, room.height / natural.height) *
      FIT_MARGIN;
    instance.zoom(scale, { animate: false });
    instance.pan(0, 0, { animate: false });
  }, []);

  // The library, the instance, and the wheel. All three are torn down together
  // on the way out: an instance that outlived this layer would keep listening
  // on a stage nobody can see.
  useEffect(() => {
    let live = true;
    let detachWheel: (() => void) | undefined;
    void import("@panzoom/panzoom").then(({ default: Panzoom }) => {
      const host = hostRef.current;
      const stage = stageRef.current;
      if (!live || host === null || stage === null) {
        return;
      }
      // The markup arrives sized for a reading column: mermaid's own
      // `width="100%"` and inline clamp would let the browser scale the
      // drawing to the host instead of letting the transform do it. Its own
      // pixel size goes on instead, so one scale factor means one thing.
      const drawing = host.querySelector("svg");
      const natural = drawing === null ? null : naturalSize(drawing);
      if (drawing !== null && natural !== null) {
        drawing.setAttribute("width", `${natural.width}px`);
        drawing.setAttribute("height", `${natural.height}px`);
        drawing.style.maxWidth = "none";
      }
      const instance = Panzoom(host, {
        maxScale: 10,
        minScale: 0.1,
        // The stage is the canvas: pointer events are bound to it rather than
        // to the drawing, so a drag anywhere in the empty space pans too.
        canvas: true,
        cursor: "grab",
        // Where the press is recorded, and it has to be here rather than on a
        // React `onPointerDown`. With `canvas: true` panzoom binds its own
        // `pointerdown` on the stage and its default start handler calls
        // `stopPropagation`, so the event never reaches the delegated listener
        // React keeps on the portal's container: a React handler would never
        // run, `pressedAt` would stay null, and the guard below would skip
        // itself. The two lines after the record are that default handler,
        // repeated because passing this option replaces it.
        handleStartEvent: (event: Event) => {
          const point = event as Partial<MouseEvent>;
          // Read structurally rather than through `instanceof`: panzoom hands
          // this whatever start event the browser gave it, and a press whose
          // position cannot be read records nothing rather than leaving the
          // previous one standing.
          pressedAt.current =
            typeof point.clientX === "number" &&
            typeof point.clientY === "number"
              ? { x: point.clientX, y: point.clientY }
              : null;
          event.preventDefault();
          event.stopPropagation();
        },
      });
      instanceRef.current = instance;
      fit(instance);
      // Not passive: zooming with the wheel means the page's own wheel
      // handling has to be prevented, and a passive listener may not.
      const onWheel = (event: WheelEvent) => {
        instance.zoomWithWheel(event);
      };
      stage.addEventListener("wheel", onWheel, { passive: false });
      detachWheel = () => {
        stage.removeEventListener("wheel", onWheel);
      };
    });
    return () => {
      live = false;
      detachWheel?.();
      instanceRef.current?.destroy();
      instanceRef.current = null;
    };
    // The markup is a dependency: a scheme change while this is open redraws
    // the diagram, and the new drawing needs its own sizing and its own fit
    // rather than the transform that belonged to the old one.
  }, [fit, svg]);

  // The keyboard comes in here and goes back where it came from, and the page
  // behind holds still while this is open. The previous overflow is restored
  // rather than cleared: something else may have been holding the page.
  useEffect(() => {
    const cameFrom = document.activeElement as HTMLElement | null;
    // Read now rather than in the cleanup: the control that opened this layer
    // is on screen the whole time this one is, so its node is the same node
    // either way, and reading a ref on the way out is a race in the general
    // case.
    const goesTo = returnFocusTo.current ?? cameFrom;
    closeRef.current?.focus();
    const held = document.body.style.overflow;
    document.body.style.overflow = "hidden";
    return () => {
      document.body.style.overflow = held;
      goesTo?.focus();
    };
  }, [returnFocusTo]);

  // True fullscreen, where the browser has it. The label follows the browser's
  // own event rather than the click, because Escape leaves fullscreen without
  // asking anybody, and leaving this layer leaves fullscreen with it.
  useEffect(() => {
    if (!canFullscreen) {
      return;
    }
    const onChange = () => {
      setFullscreen(inFullscreen());
    };
    document.addEventListener("fullscreenchange", onChange);
    return () => {
      document.removeEventListener("fullscreenchange", onChange);
      if (inFullscreen()) {
        void document.exitFullscreen().catch(() => undefined);
      }
    };
  }, [canFullscreen]);

  const toggleFullscreen = useCallback(() => {
    if (inFullscreen()) {
      void document.exitFullscreen().catch(() => undefined);
      return;
    }
    void overlayRef.current?.requestFullscreen().catch(() => undefined);
  }, []);

  const onKeyDown = useCallback(
    (event: KeyboardEvent<HTMLDivElement>) => {
      const instance = instanceRef.current;
      if (event.key === "Escape") {
        event.stopPropagation();
        onClose();
        return;
      }
      if (event.key === "Tab") {
        // The trap. The stops are not only the bar's buttons: the hosted
        // markup is mermaid's own output, and a diagram that uses a link or a
        // click directive puts an `<a href>` inside this dialog. A ring that
        // knew about buttons alone would let Tab walk out of an `aria-modal`
        // layer from one of those, which makes the modal promise false.
        const stops = Array.from(
          overlayRef.current?.querySelectorAll<HTMLElement>(FOCUS_STOPS) ?? [],
        );
        const first = stops.at(0);
        const last = stops.at(-1);
        if (first === undefined || last === undefined) {
          return;
        }
        const on = document.activeElement;
        if (on === null || !stops.includes(on as HTMLElement)) {
          // Focus is somewhere in here that is not a stop - the dialog itself
          // after a click on the bar's empty half, say - so Tab goes to an
          // end rather than to wherever the document would have taken it.
          event.preventDefault();
          (event.shiftKey ? last : first).focus();
          return;
        }
        if (event.shiftKey && on === first) {
          event.preventDefault();
          last.focus();
        } else if (!event.shiftKey && on === last) {
          event.preventDefault();
          first.focus();
        }
        return;
      }
      if (instance === null) {
        return;
      }
      switch (event.key) {
        case "+":
        case "=":
          event.preventDefault();
          instance.zoomIn();
          break;
        case "-":
          event.preventDefault();
          instance.zoomOut();
          break;
        case "0":
          event.preventDefault();
          // Back to the start, and then back to the fit: `reset` returns the
          // instance to its starting scale of 1, which is not where the
          // diagram was when it opened, so fitting it again is the second
          // half of the same act. The Reset button does exactly this too.
          instance.reset({ animate: false });
          fit(instance);
          break;
        case "ArrowLeft":
          event.preventDefault();
          instance.pan(ARROW_STEP_PX, 0, { relative: true });
          break;
        case "ArrowRight":
          event.preventDefault();
          instance.pan(-ARROW_STEP_PX, 0, { relative: true });
          break;
        case "ArrowUp":
          event.preventDefault();
          instance.pan(0, ARROW_STEP_PX, { relative: true });
          break;
        case "ArrowDown":
          event.preventDefault();
          instance.pan(0, -ARROW_STEP_PX, { relative: true });
          break;
        default:
          break;
      }
    },
    [fit, onClose],
  );

  /**
   * The empty space around the drawing is the way out, the way a lightbox's
   * backdrop is. Two conditions: the click landed on the stage itself rather
   * than on the drawing, and the pointer barely moved - a drag that ended out
   * here was panning, not dismissing.
   */
  const onStageClick = useCallback(
    (event: {
      target: EventTarget | null;
      currentTarget: EventTarget;
      clientX: number;
      clientY: number;
    }) => {
      if (event.target !== event.currentTarget) {
        return;
      }
      const from = pressedAt.current;
      if (
        from !== null &&
        (Math.abs(event.clientX - from.x) > CLICK_SLOP_PX ||
          Math.abs(event.clientY - from.y) > CLICK_SLOP_PX)
      ) {
        return;
      }
      onClose();
    },
    [onClose],
  );

  const onStageDoubleClick = useCallback(() => {
    instanceRef.current?.zoomIn();
  }, []);

  return createPortal(
    <div
      ref={overlayRef}
      role="dialog"
      aria-modal="true"
      // Focusable itself, so a click on the bar's empty half lands here rather
      // than on `document.body`: the key handler below is on this element, and
      // focus outside it would take Escape, the zoom keys and the arrows with
      // it. -1 keeps it out of the Tab order all the same.
      tabIndex={-1}
      aria-label="Diagram, full window"
      // Opaque rather than a translucent scrim: what is behind is a page of
      // prose, and a diagram's own thin lines read badly over it.
      className="fixed inset-0 z-50 flex flex-col bg-white dark:bg-slate-950"
      onKeyDown={onKeyDown}
    >
      <div className="flex shrink-0 items-center justify-end gap-1 border-b border-slate-200 px-2 py-1 dark:border-slate-800">
        <button
          type="button"
          aria-label="Zoom in"
          title="Zoom in"
          className={BAR_BUTTON}
          onClick={() => instanceRef.current?.zoomIn()}
        >
          <ZoomIn size={16} strokeWidth={1.75} />
        </button>
        <button
          type="button"
          aria-label="Zoom out"
          title="Zoom out"
          className={BAR_BUTTON}
          onClick={() => instanceRef.current?.zoomOut()}
        >
          <ZoomOut size={16} strokeWidth={1.75} />
        </button>
        <button
          type="button"
          aria-label="Reset"
          title="Reset"
          className={BAR_BUTTON}
          onClick={() => {
            const instance = instanceRef.current;
            if (instance !== null) {
              instance.reset({ animate: false });
              fit(instance);
            }
          }}
        >
          <RotateCcw size={16} strokeWidth={1.75} />
        </button>
        {canFullscreen && (
          <button
            type="button"
            aria-label={fullscreen ? "Exit fullscreen" : "Enter fullscreen"}
            title={fullscreen ? "Exit fullscreen" : "Enter fullscreen"}
            className={BAR_BUTTON}
            onClick={toggleFullscreen}
          >
            {fullscreen ? (
              <Minimize2 size={16} strokeWidth={1.75} />
            ) : (
              <Maximize2 size={16} strokeWidth={1.75} />
            )}
          </button>
        )}
        <button
          type="button"
          ref={closeRef}
          aria-label="Close"
          title="Close"
          className={BAR_BUTTON}
          onClick={onClose}
        >
          <X size={16} strokeWidth={1.75} />
        </button>
      </div>
      <div
        ref={stageRef}
        data-testid="diagram-overlay-stage"
        // `touch-none` hands every touch gesture to the pan-and-zoom instance,
        // pinch included; the browser's own panning here would fight it.
        className="relative flex flex-1 touch-none items-center justify-center overflow-hidden"
        onClick={onStageClick}
        onDoubleClick={onStageDoubleClick}
      >
        {/*
          The markup is mermaid's own output, produced by its sanitizing mode
          from the source in the document; the reading view hands it over
          unchanged and this layer only resizes the root.
        */}
        <div ref={hostRef} dangerouslySetInnerHTML={{ __html: svg }} />
      </div>
    </div>,
    document.body,
  );
}
