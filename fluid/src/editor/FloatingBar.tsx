/**
 * The floating action bar: a small `role="toolbar"` named "Quick format"
 * that stands just above a text selection, or above the caret in the open
 * table cell, with what makes sense there.
 *
 * - On text: Bold, Italic, Strikethrough, Inline code, Link and Wiki link,
 *   the sticky bar's own commands under the sticky bar's own names (the
 *   toolbar's name tells the two apart).
 * - In a cell: the same text group, then Insert row below, Insert column
 *   right, Delete row, Delete column, Align (left, center, right) and "More
 *   table actions", which opens the grid's Cell actions menu. The table verbs
 *   run through the grid's own action path (`runCellAction`), so they keep
 *   its undo groups, its focus rule and the cell they land in.
 *
 * Every command runs on `activeCell(main)?.view ?? main`, read when it runs,
 * and the focus goes back to that view afterwards. The bar never takes focus
 * on its own: Alt+F10 moves it to the first button, the arrows move along
 * (one roving tab stop), Home and End jump, and Escape hands it back to the
 * view it came from with that view's selection as it was. Like the sticky bar
 * it cancels its own mousedown, so a click never takes the selection away.
 *
 * It hides while the pointer button is down in the editor (a drag selection),
 * on Escape, when the focus leaves the editor and the bar, and when the
 * context is null. Lazy: the screens load this chunk only once a first
 * context exists (`FloatingBarSlot`).
 */

import type { EditorView } from "@codemirror/view";
import {
  Bold,
  Brackets,
  Code,
  Columns3,
  Ellipsis,
  Italic,
  Link,
  Rows3,
  Strikethrough,
} from "lucide-react";
import { DropdownMenu } from "radix-ui";
import type {
  KeyboardEvent as ReactKeyboardEvent,
  MouseEvent,
  ReactElement,
} from "react";
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import type { IconComponent } from "../components/primitives";
import { IconButton } from "../components/primitives";
import type { FloatingContext } from "./floatingContext";
import { setFloatingFocus } from "./floatingContext";
import { activeCell } from "./grid/cellEditor";
import {
  cellActionApplies,
  openCellActions,
  runCellAction,
} from "./grid/gridChrome";
import {
  AlignColumnCenterIcon,
  AlignColumnIcon,
  AlignColumnLeftIcon,
  AlignColumnRightIcon,
  DeleteColumnIcon,
  DeleteRowIcon,
} from "./tableIcons";
import { insertMarkdownLink, insertWikilink, toggleInline } from "./toolbar";

export interface FloatingBarProps {
  /** The main view; the open cell's view is found from it when needed. */
  main: EditorView | null;
  context: FloatingContext;
  /** Whether the document links engrams: a Reflection page does not. */
  wikilinks: boolean;
}

/** Marks the bar and its own menu, for the "is the focus still here" test. */
const MARK = "data-floating-bar";

/** The gap between the bar and the selection it stands by, in pixels. */
const GAP = 6;

interface Item {
  label: string;
  icon: IconComponent;
  disabled?: boolean;
  run: (main: EditorView) => void;
}

const ALIGNMENTS: { label: string; icon: IconComponent }[] = [
  { label: "Align left", icon: AlignColumnLeftIcon },
  { label: "Align center", icon: AlignColumnCenterIcon },
  { label: "Align right", icon: AlignColumnRightIcon },
];

/** The view a command acts on: the open cell's, or main. */
function targetOf(main: EditorView): EditorView {
  return activeCell(main)?.view ?? main;
}

function textItems(wikilinks: boolean): Item[] {
  const inline = (marker: string) => (main: EditorView) => {
    toggleInline(targetOf(main), marker);
  };
  return [
    { label: "Bold", icon: Bold, run: inline("**") },
    { label: "Italic", icon: Italic, run: inline("*") },
    { label: "Strikethrough", icon: Strikethrough, run: inline("~~") },
    { label: "Inline code", icon: Code, run: inline("`") },
    {
      label: "Link",
      icon: Link,
      run: (main) => {
        insertMarkdownLink(targetOf(main));
      },
    },
    ...(wikilinks
      ? [
          {
            label: "Wiki link",
            icon: Brackets,
            run: (main: EditorView) => {
              insertWikilink(targetOf(main));
            },
          },
        ]
      : []),
  ];
}

function cellItems(main: EditorView): Item[] {
  const verb = (label: string, name: string, icon: IconComponent): Item => ({
    label,
    icon,
    disabled: !cellActionApplies(main, name),
    run: (m) => {
      runCellAction(m, name);
    },
  });
  return [
    verb("Insert row below", "Insert row below", Rows3),
    verb("Insert column right", "Insert column right", Columns3),
    verb("Delete row", "Delete row", DeleteRowIcon),
    verb("Delete column", "Delete column", DeleteColumnIcon),
  ];
}

export default function FloatingBar({
  main,
  context,
  wikilinks,
}: FloatingBarProps): ReactElement | null {
  const bar = useRef<HTMLDivElement>(null);
  /** The view Alt+F10 came from, where Escape returns. */
  const origin = useRef<EditorView | null>(null);
  /** The context as of the last render, for the Alt+F10 handler. */
  const contextNow = useRef(context);
  useEffect(() => {
    contextNow.current = context;
  });
  const [dismissed, setDismissed] = useState(false);
  const [pointerDown, setPointerDown] = useState(false);
  const [focused, setFocused] = useState(() => {
    const active = document.activeElement;
    return main !== null && active !== null && main.dom.contains(active);
  });
  /** Bumped by Alt+F10; the effect below then focuses the first button. */
  const [focusAsked, setFocusAsked] = useState(0);
  const focusDone = useRef(0);
  const [current, setCurrent] = useState<string | null>(null);

  // A new context is a new place to stand by: an Escape held for the last.
  // Set during render, React's pattern for state that follows a prop.
  const [shownFor, setShownFor] = useState(context);
  if (shownFor !== context) {
    setShownFor(context);
    setDismissed(false);
  }

  // The editor area: main's DOM (cell views and the Cell actions menu are
  // inside it) and the bar with its own menu.
  useEffect(() => {
    if (!main) return;
    const inArea = (node: EventTarget | null): boolean => {
      if (!(node instanceof Node)) return false;
      if (main.dom.contains(node)) return true;
      const el = node instanceof Element ? node : node.parentElement;
      return el?.closest(`[${MARK}]`) != null;
    };
    const onFocusIn = (event: FocusEvent) => {
      setFocused(inArea(event.target));
    };
    const onFocusOut = (event: FocusEvent) => {
      if (!inArea(event.relatedTarget)) setFocused(false);
    };
    let pressed = false;
    const onDown = (event: globalThis.MouseEvent) => {
      if (event.button !== 0) return;
      pressed = true;
      setPointerDown(true);
    };
    // A press in the editor ends in a new place (or the same one picked
    // again): an Escape held for the last one is over.
    const onUp = () => {
      if (!pressed) return;
      pressed = false;
      setPointerDown(false);
      setDismissed(false);
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") setDismissed(true);
    };
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    main.dom.addEventListener("mousedown", onDown);
    document.addEventListener("mouseup", onUp);
    main.dom.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      main.dom.removeEventListener("mousedown", onDown);
      document.removeEventListener("mouseup", onUp);
      main.dom.removeEventListener("keydown", onKey);
    };
  }, [main]);

  // Alt+F10, from the text or the cell.
  useEffect(() => {
    if (!main) return;
    return setFloatingFocus(main, (from) => {
      if (contextNow.current === null) return false;
      origin.current = from;
      setDismissed(false);
      setFocused(true);
      setPointerDown(false);
      setFocusAsked((n) => n + 1);
      return true;
    });
  }, [main]);

  const visible =
    main !== null && context !== null && !dismissed && !pointerDown && focused;

  const items: Item[] =
    visible && context.kind === "cell"
      ? [...textItems(wikilinks), ...cellItems(main)]
      : textItems(wikilinks);
  const inCell = visible && context.kind === "cell";
  const alignOff = inCell && !cellActionApplies(main, "Align left");
  /** The enabled buttons, in order: the roving tab stop is one of them. */
  const labels = [
    ...items.filter((item) => !item.disabled).map((item) => item.label),
    ...(inCell ? [...(alignOff ? [] : ["Align"]), "More table actions"] : []),
  ];
  const stop =
    current !== null && labels.includes(current) ? current : labels[0];

  useEffect(() => {
    if (focusDone.current === focusAsked || !visible) return;
    focusDone.current = focusAsked;
    bar.current
      ?.querySelector<HTMLElement>("[data-roving]:not(:disabled)")
      ?.focus();
  }, [focusAsked, visible]);

  // The rect in page coordinates, fixed when the context arrives (it was
  // measured just before), so a later render after the page scrolled still
  // places the bar by the selection rather than by where it used to be.
  const anchor = useMemo(
    () =>
      context && {
        top: context.rect.top + window.scrollY,
        bottom: context.rect.bottom + window.scrollY,
        left: context.rect.left + window.scrollX,
      },
    [context],
  );

  // Placed above the rect, or below it with no room above; never over it.
  useLayoutEffect(() => {
    const el = bar.current;
    const parent = el?.offsetParent;
    if (!el || !(parent instanceof HTMLElement) || !anchor) return;
    const rect = {
      top: anchor.top - window.scrollY,
      bottom: anchor.bottom - window.scrollY,
      left: anchor.left - window.scrollX,
    };
    const box = parent.getBoundingClientRect();
    const sticky = parent
      .querySelector('[role="toolbar"][aria-label="Formatting"]')
      ?.getBoundingClientRect().bottom;
    const ceiling = Math.max(0, sticky ?? 0);
    const height = el.offsetHeight;
    const width = el.offsetWidth;
    let top = rect.top - height - GAP;
    if (top < ceiling) top = rect.bottom + GAP;
    const left = Math.max(
      box.left,
      Math.min(rect.left, box.left + box.width - width),
    );
    el.style.top = `${String(top - box.top)}px`;
    el.style.left = `${String(left - box.left)}px`;
  });

  if (!visible) return null;

  const back = () => {
    const to = origin.current ?? targetOf(main);
    origin.current = null;
    setDismissed(true);
    to.focus();
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (!target.hasAttribute("data-roving")) return; // inside the Align menu
    const stops = [
      ...(bar.current?.querySelectorAll<HTMLElement>(
        "[data-roving]:not(:disabled)",
      ) ?? []),
    ];
    const index = stops.indexOf(target);
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

  const roving = (label: string) => ({
    "data-roving": "",
    tabIndex: label === stop ? 0 : -1,
    onFocus: () => {
      setCurrent(label);
    },
  });

  return (
    <div
      ref={bar}
      role="toolbar"
      aria-label="Quick format"
      aria-orientation="horizontal"
      {...{ [MARK]: "" }}
      onMouseDown={keepSelection}
      onKeyDown={onKeyDown}
      className="absolute z-20 flex items-center gap-0.5 rounded-md border border-slate-200 bg-white p-0.5 shadow-md print:hidden dark:border-slate-700 dark:bg-slate-900"
    >
      {items.map((item) => (
        <IconButton
          key={item.label}
          label={item.label}
          icon={item.icon}
          disabled={item.disabled ?? false}
          onClick={() => {
            item.run(main);
          }}
          {...roving(item.label)}
        />
      ))}
      {inCell && (
        <>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <IconButton
                label="Align"
                icon={AlignColumnIcon}
                disabled={alignOff}
                {...roving("Align")}
              />
            </DropdownMenu.Trigger>
            {/*
              No portal: the menu stays inside the bar, so the focus moving
              into it never reads as the focus leaving the editor.
            */}
            <DropdownMenu.Content
              align="start"
              sideOffset={6}
              className={MENU_CLASSES}
              {...{ [MARK]: "" }}
              onCloseAutoFocus={(event) => {
                event.preventDefault();
                targetOf(main).focus();
              }}
            >
              {ALIGNMENTS.map(({ label, icon: Icon }) => (
                <DropdownMenu.Item
                  key={label}
                  className={ITEM_CLASSES}
                  onSelect={() => {
                    runCellAction(main, label);
                  }}
                >
                  <Icon aria-hidden="true" size={16} strokeWidth={1.75} />
                  {label}
                </DropdownMenu.Item>
              ))}
            </DropdownMenu.Content>
          </DropdownMenu.Root>
          <IconButton
            label="More table actions"
            icon={Ellipsis}
            aria-haspopup="menu"
            onClick={(event) => {
              const rect = event.currentTarget.getBoundingClientRect();
              openCellActions(main, { x: rect.left, y: rect.bottom });
            }}
            {...roving("More table actions")}
          />
        </>
      )}
    </div>
  );
}
