/**
 * The floating action bar: a small `role="toolbar"` named "Quick format"
 * that stands just above a text selection, or above (or below) the table
 * whose cell is open, with what makes sense there. A table with no room on
 * either side, or taller than the viewport, has the bar by the caret. On a
 * selected picture it is named "Picture options" and stands by the image.
 *
 * - On text: Bold, Italic, Strikethrough, Inline code, Link and Wiki link,
 *   the sticky bar's own commands under the sticky bar's own names (the
 *   toolbar's name tells the two apart).
 * - In a cell: the same text group, then Insert row below, Insert column
 *   right, Delete row, Delete column, Align (left, center, right) and "More
 *   table actions", which opens the grid's Cell actions menu. The table verbs
 *   run through the grid's own action path (`runCellAction`), so they keep
 *   its undo groups, its focus rule and the cell they land in.
 * - On a picture: `PictureControls`, its placement, size, alt text and
 *   removal. Its alt field takes the focus like a field: a press on it is
 *   not cancelled, and Left, Right, Home and End edit its text, with Left
 *   at the start and Right at the end going on along the bar (P13).
 *
 * Every command runs on `verbTarget(main)`, the sticky bar's own rule, read
 * when it runs: the open cell's view, else main, and nothing while main's
 * selection is parked with no cell mounted. The focus goes back to that view
 * afterwards. Bold, Italic, Strikethrough and Inline code run `toggleFormat`
 * and show as pressed the formats the screen is told by the buffer. The bar
 * never takes focus on its own: Alt+F10 moves it to the first button, the
 * arrows move along (one roving tab stop), Home and End jump, and Escape
 * hands it back to the view it came from with that view's selection as it
 * was. Like the sticky bar it cancels its own mousedown, so a click never
 * takes the selection away.
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
import {
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";

import { ITEM_CLASSES, MENU_CLASSES } from "../components/menu";
import type { IconComponent } from "../components/primitives";
import { IconButton } from "../components/primitives";
import { linkVerb } from "./elementEdit";
import type { FloatingContext } from "./floatingContext";
import {
  selectionMoves,
  setFloatingFocus,
  verbTarget,
} from "./floatingContext";
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
import type { Format } from "./marks/formatting";
import { judgeBeforeCommand } from "./marks/commandJudge";
import { formatRefusal, toggleFormat } from "./marks/formatting";
import { focusBack } from "./focusBack";
import { PictureControls, pictureLabels } from "./PictureControls";
import { insertWikilink } from "./toolbar";

export interface FloatingBarProps {
  /** The main view; the open cell's view is found from it when needed. */
  main: EditorView | null;
  context: FloatingContext;
  /** Whether the document links engrams: a Reflection page does not. */
  wikilinks: boolean;
  /**
   * The formats to draw pressed, as the sticky bar draws them. Absent, the
   * format buttons are plain buttons.
   */
  formats?: ReadonlySet<Format>;
}

/** Marks the bar and its own menu, for the "is the focus still here" test. */
const MARK = "data-floating-bar";

/** The gap between the bar and the selection it stands by, in pixels. */
const GAP = 6;

interface Item {
  label: string;
  icon: IconComponent;
  disabled?: boolean;
  /** The format a format button toggles, for its pressed state. */
  format?: Format;
  /** The id of the sentence that says why the item is off, if it says one. */
  describedBy?: string;
  run: (main: EditorView) => void;
}

const ALIGNMENTS: { label: string; icon: IconComponent }[] = [
  { label: "Align left", icon: AlignColumnLeftIcon },
  { label: "Align center", icon: AlignColumnCenterIcon },
  { label: "Align right", icon: AlignColumnRightIcon },
];

/** A verb on the view `verbTarget` names, or nothing when it names none. */
function onTarget(run: (view: EditorView) => void) {
  return (main: EditorView) => {
    const target = verbTarget(main);
    if (target) run(target);
  };
}

/**
 * The text group. `why` is the id of the sentence that says why the format
 * buttons are off (a selected picture in a link), or null when they are on.
 */
function textItems(wikilinks: boolean, why: string | null = null): Item[] {
  const format = (label: string, icon: IconComponent, which: Format): Item => ({
    label,
    icon,
    format: which,
    ...(why === null ? {} : { disabled: true, describedBy: why }),
    run: onTarget((view) => {
      toggleFormat(view, which);
    }),
  });
  return [
    format("Bold", Bold, "strong"),
    format("Italic", Italic, "em"),
    format("Strikethrough", Strikethrough, "strike"),
    format("Inline code", Code, "code"),
    {
      label: "Link",
      icon: Link,
      run: onTarget((view) => {
        linkVerb(view);
      }),
    },
    ...(wikilinks
      ? [
          {
            label: "Wiki link",
            icon: Brackets,
            run: onTarget((view) => {
              insertWikilink(view);
            }),
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
  formats,
}: FloatingBarProps): ReactElement | null {
  const bar = useRef<HTMLDivElement>(null);
  /** The view Alt+F10 came from, where Escape returns. */
  const origin = useRef<EditorView | null>(null);
  /** The context as of the last render, for the Alt+F10 handler. */
  const contextNow = useRef(context);
  useEffect(() => {
    contextNow.current = context;
  });
  /**
   * Set by Escape: the kind it was pressed on and the selection moves count
   * then. A rect that only moved (a peer's edit, a reflow, typing) keeps the
   * bar away; a new kind, a selection the person moved, or a press brings it
   * back.
   */
  const [dismissed, setDismissed] = useState<{
    kind: NonNullable<FloatingContext>["kind"] | null;
    moves: number;
  } | null>(null);
  const [pointerDown, setPointerDown] = useState(false);
  const [focused, setFocused] = useState(() => {
    const active = document.activeElement;
    return main !== null && active !== null && main.dom.contains(active);
  });
  /** Bumped by Alt+F10; the effect below then focuses the first button. */
  const [focusAsked, setFocusAsked] = useState(0);
  const focusDone = useRef(0);
  const [current, setCurrent] = useState<string | null>(null);

  const dismiss = useCallback(() => {
    setDismissed({
      kind: contextNow.current?.kind ?? null,
      moves: main ? selectionMoves(main) : 0,
    });
  }, [main]);

  // A new place to stand by ends an Escape held for the last one. Set
  // during render, React's pattern for state that follows a prop.
  if (
    dismissed !== null &&
    (dismissed.kind !== (context?.kind ?? null) ||
      (main !== null && dismissed.moves !== selectionMoves(main)))
  ) {
    setDismissed(null);
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
    // Mouse, touch and pen alike: the primary pointer's press.
    let pressed = false;
    const onDown = (event: PointerEvent) => {
      if (!event.isPrimary || event.button !== 0) return;
      pressed = true;
      setPointerDown(true);
    };
    // A press in the editor ends in a new place (or the same one picked
    // again): an Escape held for the last one is over.
    const onUp = (event: PointerEvent) => {
      if (!pressed || !event.isPrimary) return;
      pressed = false;
      setPointerDown(false);
      setDismissed(null);
    };
    // A touch scroll or a drag the browser takes over ends in a cancel, not
    // an up: the press is over, but nothing was picked, so an Escape holds.
    const onCancel = (event: PointerEvent) => {
      if (!pressed || !event.isPrimary) return;
      pressed = false;
      setPointerDown(false);
    };
    const onKey = (event: globalThis.KeyboardEvent) => {
      if (event.key === "Escape") dismiss();
    };
    document.addEventListener("focusin", onFocusIn);
    document.addEventListener("focusout", onFocusOut);
    main.dom.addEventListener("pointerdown", onDown);
    document.addEventListener("pointerup", onUp);
    document.addEventListener("pointercancel", onCancel);
    main.dom.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("focusin", onFocusIn);
      document.removeEventListener("focusout", onFocusOut);
      main.dom.removeEventListener("pointerdown", onDown);
      document.removeEventListener("pointerup", onUp);
      document.removeEventListener("pointercancel", onCancel);
      main.dom.removeEventListener("keydown", onKey);
    };
  }, [main, dismiss]);

  // Alt+F10, from the text or the cell.
  useEffect(() => {
    if (!main) return;
    return setFloatingFocus(main, (from) => {
      if (contextNow.current === null) return false;
      origin.current = from;
      setDismissed(null);
      setFocused(true);
      setPointerDown(false);
      setFocusAsked((n) => n + 1);
      return true;
    });
  }, [main]);

  const visible =
    main !== null &&
    context !== null &&
    dismissed === null &&
    !pointerDown &&
    focused;

  const picture = visible && context.kind === "image" ? context : null;
  /**
   * Why the format buttons are off, as the sticky bar says it (P21): a
   * selected picture in a link stands here as text, and has none to format.
   */
  const target = main === null ? null : (verbTarget(main) ?? null);
  const formatWhy =
    visible && target !== null ? formatRefusal(target.state) : null;
  const whyId = useId();
  const why = formatWhy === null ? null : whyId;
  const items: Item[] =
    picture !== null
      ? []
      : visible && context.kind === "cell"
        ? [...textItems(wikilinks, why), ...cellItems(main)]
        : textItems(wikilinks, why);
  const inCell = visible && context.kind === "cell";
  const alignOff = inCell && !cellActionApplies(main, "Align left");
  /** The enabled controls, in order: the roving tab stop is one of them. */
  const labels =
    picture !== null
      ? pictureLabels(picture.formattable)
      : [
          ...items.filter((item) => !item.disabled).map((item) => item.label),
          ...(inCell
            ? [...(alignOff ? [] : ["Align"]), "More table actions"]
            : []),
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
  const anchor = useMemo(() => {
    if (!context) return null;
    const page = (rect: DOMRect) => ({
      top: rect.top + window.scrollY,
      bottom: rect.bottom + window.scrollY,
      left: rect.left + window.scrollX,
    });
    return {
      ...page(context.rect),
      caret: context.kind === "cell" ? page(context.caret) : null,
    };
  }, [context]);

  // Placed above the rect, or below it with no room above; never over it.
  // For a cell the rect is the table: with no room on either side of it, the
  // bar goes by the caret instead.
  useLayoutEffect(() => {
    const el = bar.current;
    const parent = el?.offsetParent;
    if (!el || !(parent instanceof HTMLElement) || !anchor) return;
    const view = (at: { top: number; bottom: number; left: number }) => ({
      top: at.top - window.scrollY,
      bottom: at.bottom - window.scrollY,
      left: at.left - window.scrollX,
    });
    const box = parent.getBoundingClientRect();
    const sticky = parent
      .querySelector('[role="toolbar"][aria-label="Formatting"]')
      ?.getBoundingClientRect().bottom;
    const ceiling = Math.max(0, sticky ?? 0);
    const height = el.offsetHeight;
    const width = el.offsetWidth;
    let rect = view(anchor);
    const above = rect.top - height - GAP >= ceiling;
    const below = rect.bottom + GAP + height <= window.innerHeight;
    if (anchor.caret && !above && !below) rect = view(anchor.caret);
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
    const to = origin.current ?? verbTarget(main) ?? main;
    origin.current = null;
    dismiss();
    focusBack(to);
  };

  const onKeyDown = (event: ReactKeyboardEvent<HTMLDivElement>) => {
    const target = event.target as HTMLElement;
    if (!target.hasAttribute("data-roving")) return; // inside the Align menu
    // The alt field's keys edit its text; only its edges go on (P13).
    if (target instanceof HTMLInputElement) {
      // A key an input method's composition takes is not the bar's.
      if (event.nativeEvent.isComposing || event.keyCode === 229) return;
      const start = target.selectionStart;
      const end = target.selectionEnd;
      const length = target.value.length;
      if (event.key === "Home" || event.key === "End") return;
      if (event.key === "ArrowLeft" && !(start === 0 && end === 0)) return;
      if (event.key === "ArrowRight" && !(start === length && end === length)) {
        return;
      }
    }
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
    // The alt field is a field: a press on it puts the caret there (P13).
    if (event.target instanceof HTMLInputElement) return;
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
      aria-label={picture !== null ? "Picture options" : "Quick format"}
      aria-orientation="horizontal"
      {...{ [MARK]: "" }}
      onMouseDown={keepSelection}
      onKeyDown={onKeyDown}
      className="absolute z-20 flex items-center gap-0.5 rounded-md border border-slate-200 bg-white p-0.5 shadow-md print:hidden dark:border-slate-700 dark:bg-slate-900"
    >
      {picture !== null && (
        <PictureControls main={main} context={picture} roving={roving} />
      )}
      {formatWhy !== null && picture === null && (
        <span id={whyId} className="sr-only">
          {formatWhy}
        </span>
      )}
      {items.map((item) => (
        <IconButton
          key={item.label}
          label={item.label}
          icon={item.icon}
          disabled={item.disabled ?? false}
          {...(item.describedBy === undefined
            ? {}
            : { "aria-describedby": item.describedBy })}
          {...(item.format && formats
            ? { "aria-pressed": formats.has(item.format) }
            : {})}
          onClick={() => {
            // From outside the editor's DOM: a waiting composition first (D60).
            judgeBeforeCommand(verbTarget(main) ?? main);
            item.run(main);
            // The view Alt+F10 came from may be gone now (a verb that
            // landed in another cell): Escape later returns to the target.
            origin.current = null;
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
                (verbTarget(main) ?? main).focus();
              }}
            >
              {ALIGNMENTS.map(({ label, icon: Icon }) => (
                <DropdownMenu.Item
                  key={label}
                  className={ITEM_CLASSES}
                  onSelect={() => {
                    runCellAction(main, label);
                    origin.current = null;
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
              openCellActions(main, {
                x: rect.left,
                y: rect.bottom,
                top: rect.top,
              });
              origin.current = null;
            }}
            {...roving("More table actions")}
          />
        </>
      )}
    </div>
  );
}
