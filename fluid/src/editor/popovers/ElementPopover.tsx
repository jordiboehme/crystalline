/**
 * The element popover host: one small `role="dialog"` named for what it
 * edits ("Link", "Wiki link"), placed under the element (above it when there
 * is no room below), with the focus on its first field.
 *
 * Enter or Save commits: the popover's verb runs again on the view that
 * asked, and its changes go in as one gesture there (`commitElementEdit`),
 * after which that view has the focus and the caret sits after the element.
 * Escape writes nothing and hands the focus back with the selection as it
 * was. A press outside and the focus leaving close it without writing too. The
 * buffer closes it on its own when the element went, the setting or Raw
 * flipped, the buffer was rebuilt or the page stopped taking writes.
 *
 * Lazy: the screens load this chunk with the first request
 * (`ElementPopoverSlot`). It renders inside the editor column like the
 * floating bar, and keeps its own presses from moving the focus out of its
 * field.
 */

import type { ChangeSpec, EditorState } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import type {
  FocusEvent,
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
  ReactElement,
} from "react";
import { useEffect, useLayoutEffect, useRef } from "react";

import type { ElementEdit, ElementRequest } from "../elementEdit";
import {
  closeElementEdit,
  commitElementEdit,
  discardElementEdit,
} from "../elementEdit";
import type { ElementPopoverProps } from "./ElementPopoverSlot";
import { placement } from "./placement";
import { LinkPopover } from "./LinkPopover";
import { WikilinkPopover } from "./WikilinkPopover";

/** What a popover's verb makes of the request, re-read at commit. */
export type ElementWrite = (
  state: EditorState,
  edit: ElementEdit,
) => ChangeSpec[] | null;

/** What the host hands the popover inside it. */
export interface PopoverBodyProps {
  /** The view that asked: main, or the open table cell. */
  view: EditorView;
  edit: ElementEdit;
  /** The title matches for a wikilink target, where engrams are linked. */
  titles?: ElementPopoverProps["titles"];
  /** Writes `write`'s changes as one gesture and closes. */
  commit: (write: ElementWrite) => void;
  /** Closes without writing and leaves the focus where it goes. */
  close: () => void;
}

/** Marks the popover, for the "is this press inside it" test. */
const POPOVER_MARK = "data-element-popover";

function nameOf(edit: ElementEdit): string {
  switch (edit.kind) {
    case "wikilink":
      return "Wiki link";
    default:
      return "Link";
  }
}

/** A press in a field keeps its own default (it places the caret there). */
function takesText(target: EventTarget): boolean {
  return (
    target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement
  );
}

const serials = new WeakMap<ElementEdit, number>();
let nextSerial = 0;

/** One number per request, so a new request starts a fresh popover. */
function serialOf(edit: ElementEdit): number {
  let serial = serials.get(edit);
  if (serial === undefined) {
    nextSerial += 1;
    serial = nextSerial;
    serials.set(edit, serial);
  }
  return serial;
}

export default function ElementPopover({
  main,
  request,
  titles,
}: ElementPopoverProps): ReactElement | null {
  if (!main || !request) return null;
  return (
    <Open
      key={serialOf(request.edit)}
      main={main}
      request={request}
      {...(titles !== undefined ? { titles } : {})}
    />
  );
}

function Open({
  main,
  request,
  titles,
}: {
  main: EditorView;
  request: ElementRequest;
  titles?: ElementPopoverProps["titles"];
}): ReactElement {
  const root = useRef<HTMLDivElement>(null);
  /** Set by the first close of any kind, so a later one does nothing. */
  const closed = useRef(false);
  const { edit, view } = request;

  const close = () => {
    if (closed.current) return;
    closed.current = true;
    closeElementEdit(main);
  };
  const discard = () => {
    if (closed.current) return;
    closed.current = true;
    discardElementEdit(main);
  };
  const commit = (write: ElementWrite) => {
    if (closed.current) return;
    closed.current = true;
    commitElementEdit(main, write);
  };

  // The first field takes the focus.
  useEffect(() => {
    root.current?.querySelector<HTMLElement>("input, textarea")?.focus();
  }, []);

  // A press anywhere outside closes it without writing.
  const closeRef = useRef(close);
  useEffect(() => {
    closeRef.current = close;
  });
  useEffect(() => {
    const onDown = (event: PointerEvent) => {
      const el = root.current;
      if (el && event.target instanceof Node && !el.contains(event.target)) {
        closeRef.current();
      }
    };
    document.addEventListener("pointerdown", onDown, true);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
    };
  }, []);

  // Under the element, inside the column it renders in.
  useLayoutEffect(() => {
    const el = root.current;
    const parent = el?.offsetParent;
    if (!el || !(parent instanceof HTMLElement)) return;
    const box = parent.getBoundingClientRect();
    const sticky = parent
      .querySelector('[role="toolbar"][aria-label="Formatting"]')
      ?.getBoundingClientRect().bottom;
    const at = placement(
      edit.rect,
      { width: el.offsetWidth, height: el.offsetHeight },
      box,
      window.innerHeight,
      Math.max(0, sticky ?? 0),
    );
    el.style.top = `${String(at.top)}px`;
    el.style.left = `${String(at.left)}px`;
  });

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Escape" || event.defaultPrevented) return;
    event.preventDefault();
    event.stopPropagation();
    discard();
  };
  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget;
    // Null is the window losing the focus: the person comes back to it.
    if (next instanceof Node && !event.currentTarget.contains(next)) close();
  };
  const onMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (!takesText(event.target)) event.preventDefault();
  };

  const body: PopoverBodyProps = {
    view,
    edit,
    commit,
    close,
    ...(titles !== undefined ? { titles } : {}),
  };

  return (
    <div
      ref={root}
      role="dialog"
      aria-label={nameOf(edit)}
      {...{ [POPOVER_MARK]: "" }}
      onKeyDown={onKeyDown}
      onBlur={onBlur}
      onMouseDown={onMouseDown}
      className="absolute z-30 w-80 max-w-[calc(100vw-2rem)] rounded-md border border-slate-200 bg-white p-3 text-sm shadow-md print:hidden dark:border-slate-700 dark:bg-slate-900"
    >
      {edit.kind === "wikilink" ? (
        <WikilinkPopover {...body} />
      ) : edit.kind === "link" || edit.kind === "new-link" ? (
        <LinkPopover {...body} />
      ) : null}
    </div>
  );
}
