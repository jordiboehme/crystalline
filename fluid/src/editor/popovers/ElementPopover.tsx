/**
 * The element popover host: one small `role="dialog"` named for what it
 * edits ("Link", "Wiki link", "Status label", "Formula"), or a
 * `role="menu"` for a choice ("Task state", "Callout kind", "Code
 * language", "Code block actions"), placed under
 * the element (above it when there is no room below), with the focus on its
 * first field or on the menu's checked item (a menu with a filter field
 * focuses the field). In a menu the arrow keys, Home and End move between
 * the items (in a filter field Home and End stay the field's), and picking
 * one commits.
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
  openElementEdit,
} from "../elementEdit";
import type { ElementPopoverProps } from "./ElementPopoverSlot";
import { CalloutKindMenu } from "./CalloutKindMenu";
import { CodeActionsMenu, LanguageMenu } from "./LanguageMenu";
import { LinkPopover } from "./LinkPopover";
import { MathPopover } from "./MathPopover";
import { placement } from "./placement";
import { StatusPopover } from "./StatusPopover";
import { TaskMenu } from "./TaskMenu";
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
  /** The request as the buffer follows it now (mapped), or null once gone. */
  live: () => ElementEdit | null;
}

/** Marks the popover, for the "is this press inside it" test. */
const POPOVER_MARK = "data-element-popover";

function nameOf(edit: ElementEdit): string {
  switch (edit.kind) {
    case "wikilink":
      return "Wiki link";
    case "status":
      return "Status label";
    case "math":
      return "Formula";
    case "task":
      return "Task state";
    case "callout-kind":
      return "Callout kind";
    case "language":
      return "Code language";
    case "code-actions":
      return "Code block actions";
    default:
      return "Link";
  }
}

/** Whether the popover for `edit` is a menu of choices rather than a form. */
function isMenu(edit: ElementEdit): boolean {
  return (
    edit.kind === "task" ||
    edit.kind === "callout-kind" ||
    edit.kind === "language" ||
    edit.kind === "code-actions"
  );
}

const MENU_ITEM = '[role="menuitemradio"], [role="menuitem"]';

/** Marks a menu's filter field (`LanguageMenu`), which takes the focus first. */
const MENU_FILTER = "data-menu-filter";

/**
 * The item the focus goes to in a menu: a filter field, else the checked
 * item, else the first.
 */
function firstFocus(root: HTMLElement): HTMLElement | null {
  return (
    root.querySelector<HTMLElement>(`[${MENU_FILTER}]`) ??
    root.querySelector<HTMLElement>(
      '[role="menuitemradio"][aria-checked="true"]',
    ) ??
    root.querySelector<HTMLElement>(MENU_ITEM) ??
    root.querySelector<HTMLElement>('input:not([type="radio"]), textarea')
  );
}

/** Moves the focus between a menu's items for an arrow key, Home or End. */
function moveInMenu(root: HTMLElement, key: string): boolean {
  const items = [...root.querySelectorAll<HTMLElement>(MENU_ITEM)];
  if (items.length === 0) return false;
  const now = items.findIndex((item) => item === document.activeElement);
  let next: number;
  switch (key) {
    case "ArrowDown":
      next = now < 0 ? 0 : (now + 1) % items.length;
      break;
    case "ArrowUp":
      next =
        now < 0 ? items.length - 1 : (now - 1 + items.length) % items.length;
      break;
    case "Home":
      next = 0;
      break;
    case "End":
      next = items.length - 1;
      break;
    default:
      return false;
  }
  items[next]?.focus();
  return true;
}

/** A press in a field keeps its own default (it places the caret there). */
function takesText(target: EventTarget): boolean {
  return (
    target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement
  );
}

function isGone(view: EditorView): boolean {
  return (view as unknown as { destroyed?: unknown }).destroyed === true;
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
  /**
   * `closed` is set by the first close of any kind, so a later one does
   * nothing; `inside` while the focus is in the popover, read by the
   * teardown below.
   */
  const flags = useRef({ closed: false, inside: false });
  const { edit, view } = request;

  const close = () => {
    if (flags.current.closed) return;
    flags.current.closed = true;
    closeElementEdit(main);
  };
  const discard = () => {
    if (flags.current.closed) return;
    flags.current.closed = true;
    discardElementEdit(main);
  };
  const commit = (write: ElementWrite) => {
    if (flags.current.closed) return;
    flags.current.closed = true;
    commitElementEdit(main, write);
  };

  // The first field takes the focus. When the buffer closes the popover on
  // its own (the element went, the setting or Raw flipped, the page stopped
  // taking writes) with the focus in it, the focus goes back to the view
  // that asked, so a keyboard user keeps their place.
  useEffect(() => {
    if (root.current) firstFocus(root.current)?.focus();
    // One object for the popover's life; its fields are read at teardown.
    const now = flags.current;
    return () => {
      const { closed, inside } = now;
      if (closed || !inside) return;
      if (!isGone(view) && view.dom.isConnected) view.focus();
    };
  }, [view]);

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

  const menu = isMenu(edit);
  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    if (event.defaultPrevented) return;
    // In a filter field Home and End move its own caret.
    const inField =
      takesText(event.target) && (event.key === "Home" || event.key === "End");
    if (
      menu &&
      !inField &&
      root.current &&
      moveInMenu(root.current, event.key)
    ) {
      event.preventDefault();
      event.stopPropagation();
      return;
    }
    if (event.key !== "Escape") return;
    event.preventDefault();
    event.stopPropagation();
    discard();
  };
  const onFocus = () => {
    flags.current.inside = true;
  };
  const onBlur = (event: FocusEvent<HTMLDivElement>) => {
    const next = event.relatedTarget;
    if (next instanceof Node && event.currentTarget.contains(next)) return;
    flags.current.inside = false;
    // Null is the window losing the focus: the person comes back to it.
    if (next instanceof Node) close();
  };
  /** The request as the buffer follows it now, or null once it is gone. */
  const live = (): ElementEdit | null => {
    const now = openElementEdit(main);
    return now && now.view === view ? now.edit : null;
  };
  const onMouseDown = (event: MouseEvent<HTMLDivElement>) => {
    if (!takesText(event.target)) event.preventDefault();
  };

  const body: PopoverBodyProps = {
    view,
    edit,
    commit,
    close,
    live,
    ...(titles !== undefined ? { titles } : {}),
  };

  return (
    <div
      ref={root}
      role={menu ? "menu" : "dialog"}
      aria-label={nameOf(edit)}
      {...{ [POPOVER_MARK]: "" }}
      onKeyDown={onKeyDown}
      onFocus={onFocus}
      onBlur={onBlur}
      onMouseDown={onMouseDown}
      className={`absolute z-30 max-w-[calc(100vw-2rem)] rounded-md border border-slate-200 bg-white text-sm shadow-md print:hidden dark:border-slate-700 dark:bg-slate-900 ${menu ? "w-52 py-1" : "w-80 p-3"}`}
    >
      {edit.kind === "wikilink" ? (
        <WikilinkPopover {...body} />
      ) : edit.kind === "link" || edit.kind === "new-link" ? (
        <LinkPopover {...body} />
      ) : edit.kind === "status" ? (
        <StatusPopover {...body} />
      ) : edit.kind === "math" ? (
        <MathPopover {...body} />
      ) : edit.kind === "task" ? (
        <TaskMenu {...body} />
      ) : edit.kind === "callout-kind" ? (
        <CalloutKindMenu {...body} />
      ) : edit.kind === "language" ? (
        <LanguageMenu {...body} />
      ) : edit.kind === "code-actions" ? (
        <CodeActionsMenu {...body} />
      ) : null}
    </div>
  );
}
