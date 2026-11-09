/**
 * A menu of plain DOM, with no React and nothing of the editor: `role="menu"` with
 * one submenu level, one roving focus, and the keys WAI-ARIA gives a menu.
 *
 * The submenu is a second element beside the menu, not inside it, so a menu that
 * has to scroll in a short window does not cut its submenu off.
 *
 * Only a first press acts. A held arrow moves on, and every other held key does
 * nothing: a key held down over a menu must not run what it comes to.
 *
 * Where the focus goes. A menu that closes with the focus in it gives the focus
 * back, so it is never left on the body. `returnFocus` may be a function, asked at
 * that moment, for an owner whose element can be replaced while the menu is open; it
 * answers null when its owner places the focus itself. A menu whose focus went
 * elsewhere leaves it there.
 */

import { isMenuKey } from "../../platform";

export interface MenuItem {
  label: string;
  run?: () => void;
  disabled?: boolean;
  /** Set for a radio item: `role="menuitemradio"` with `aria-checked`. */
  checked?: boolean;
  submenu?: MenuItem[];
  /** Why it is disabled: `aria-description`, and shown when the item has the focus. */
  description?: string;
}

/** How a menu came to close. */
export type MenuClose = "item" | "escape" | "tab" | "outside" | "left" | "api";

export interface MenuHandle {
  close(): void;
  /** What the items say and do, for the same items in the same order. */
  refresh(items: MenuItem[]): void;
  /** Whether it still stands in the page: a box going away takes it along. */
  connected(): boolean;
}

interface Level {
  el: HTMLElement;
  rows: HTMLElement[];
  items: MenuItem[];
  hint: HTMLElement;
}

const MENU_CLASS = "cm-diagram-menu";
const ITEM_CLASS = "cm-diagram-menu-item";
const HINT_CLASS = "cm-diagram-menu-hint";
/** A pointer on its way to a submenu crosses other items, and that closes nothing. */
const HOVER_MS = 120;
/** The least height a menu keeps, however little room a short editor leaves. */
const MIN_ROOM = 96;
/** Keys that act only on the first press. */
const SPENT_WHEN_HELD = new Set([
  "Enter",
  " ",
  "ArrowRight",
  "ArrowLeft",
  "Escape",
]);

/** What `item` says on `row`, whichever items came before. */
function describeRow(row: HTMLElement, item: MenuItem): void {
  row.textContent = item.label;
  // The name alone: the look's check mark and arrow are drawn in CSS, which a screen
  // reader would read as part of it.
  row.setAttribute("aria-label", item.label);
  row.setAttribute(
    "role",
    item.checked === undefined ? "menuitem" : "menuitemradio",
  );
  if (item.checked === undefined) {
    row.removeAttribute("aria-checked");
  } else {
    row.setAttribute("aria-checked", String(item.checked));
  }
  if (item.disabled === true) {
    row.setAttribute("aria-disabled", "true");
  } else {
    row.removeAttribute("aria-disabled");
  }
  if (item.description === undefined) {
    row.removeAttribute("aria-description");
    row.removeAttribute("title");
  } else {
    row.setAttribute("aria-description", item.description);
    row.title = item.description;
  }
  if (item.submenu === undefined) {
    row.removeAttribute("aria-haspopup");
    row.removeAttribute("aria-expanded");
  } else if (!row.hasAttribute("aria-haspopup")) {
    row.setAttribute("aria-haspopup", "menu");
    row.setAttribute("aria-expanded", "false");
  }
}

function buildLevel(doc: Document, name: string, items: MenuItem[]): Level {
  const el = doc.createElement("div");
  el.className = MENU_CLASS;
  el.setAttribute("role", "menu");
  el.setAttribute("aria-label", name);
  const rows = items.map((item) => {
    const row = doc.createElement("div");
    row.className = ITEM_CLASS;
    row.tabIndex = -1;
    describeRow(row, item);
    el.append(row);
    return row;
  });
  // A sighted person's way to read why an item is off. A screen reader has it from
  // the item's description, so it is hidden from one.
  const hint = doc.createElement("div");
  hint.className = HINT_CLASS;
  hint.setAttribute("aria-hidden", "true");
  el.append(hint);
  return { el, rows, items, hint };
}

/** The part of the window `el` can be seen in. */
function visibleArea(el: HTMLElement) {
  const area = {
    left: 0,
    top: 0,
    right: window.innerWidth,
    bottom: window.innerHeight,
  };
  for (let up = el.parentElement; up !== null; up = up.parentElement) {
    if (/auto|scroll|hidden|clip/.test(getComputedStyle(up).overflowY)) {
      const clip = up.getBoundingClientRect();
      if (clip.height > 0 && clip.width > 0) {
        area.left = Math.max(area.left, clip.left);
        area.top = Math.max(area.top, clip.top);
        area.right = Math.min(area.right, clip.right);
        area.bottom = Math.min(area.bottom, clip.bottom);
      }
    }
  }
  return area;
}

/** Stands `el` at `x`, `y` of its host, and moves it back into what can be seen. */
function place(el: HTMLElement, x: number, y: number): void {
  el.style.left = `${String(x)}px`;
  el.style.top = `${String(y)}px`;
  const area = visibleArea(el);
  // Taller than the room it scrolls inside, so its last item can still be reached.
  el.style.maxHeight = `${String(Math.max(area.bottom - area.top, MIN_ROOM))}px`;
  const box = el.getBoundingClientRect();
  // The start edge wins when the menu is larger than the room.
  const moveX =
    box.left + Math.min(0, area.right - box.right) < area.left
      ? area.left - box.left
      : Math.min(0, area.right - box.right);
  const moveY =
    box.top + Math.min(0, area.bottom - box.bottom) < area.top
      ? area.top - box.top
      : Math.min(0, area.bottom - box.bottom);
  el.style.left = `${String(x + moveX)}px`;
  el.style.top = `${String(y + moveY)}px`;
}

export function openMenu(
  host: HTMLElement,
  at: { x: number; y: number },
  name: string,
  items: MenuItem[],
  returnFocus: Element | (() => Element | null),
  onClose?: (reason: MenuClose) => void,
): MenuHandle {
  const doc = host.ownerDocument;
  const top = buildLevel(doc, name, items);
  let sub: Level | null = null;
  let owner: HTMLElement | null = null;
  let closed = false;
  let hover: ReturnType<typeof setTimeout> | null = null;
  /** The row a hover waits on. */
  let hoverRow: HTMLElement | null = null;
  /** Where the pointer was last seen; it stands still until this changes. */
  let pointerAt: { x: number; y: number } | null = null;

  const levels = () => (sub === null ? [top] : [top, sub]);
  const inside = (node: EventTarget | null) =>
    node instanceof Node && levels().some((level) => level.el.contains(node));
  const levelOf = (row: Element): Level =>
    sub?.rows.includes(row as HTMLElement) === true ? sub : top;

  function show(row: HTMLElement, pointer = false): void {
    const level = levelOf(row);
    level.hint.textContent =
      level.items[level.rows.indexOf(row)]?.description ?? "";
    // Under a pointer a scroll would bring the next item there, and so on.
    row.focus({ preventScroll: pointer });
  }

  function stopHover(): void {
    if (hover !== null) {
      clearTimeout(hover);
      hover = null;
    }
    hoverRow = null;
  }

  /**
   * Whether the pointer really moved. A browser sends mouse events to an element that
   * appears under a pointer at rest, and a menu opened by a key must not take the
   * focus from them, or Enter runs what the pointer happens to rest on. The first
   * event only says where the pointer is.
   */
  function pointerMoved(event: MouseEvent): boolean {
    const was = pointerAt;
    pointerAt = { x: event.clientX, y: event.clientY };
    return was !== null && (was.x !== pointerAt.x || was.y !== pointerAt.y);
  }

  function closeSub(focusOwner: boolean): void {
    if (sub === null || owner === null) {
      return;
    }
    const was = owner;
    sub.el.remove();
    sub = null;
    owner = null;
    was.setAttribute("aria-expanded", "false");
    if (focusOwner) {
      show(was);
    }
  }

  function openSub(row: HTMLElement, focusIt: boolean): void {
    const item = top.items[top.rows.indexOf(row)];
    if (item?.submenu === undefined || item.disabled === true) {
      return;
    }
    if (owner !== row) {
      closeSub(false);
      const level = buildLevel(doc, item.label, item.submenu);
      wire(level);
      host.append(level.el);
      sub = level;
      owner = row;
      row.setAttribute("aria-expanded", "true");
      const frame = host.getBoundingClientRect();
      const box = row.getBoundingClientRect();
      place(level.el, box.right - frame.left, box.top - frame.top);
      // No room on the right: it goes on the menu's left.
      if (level.el.getBoundingClientRect().left < box.right - 1) {
        const width = level.el.getBoundingClientRect().width;
        level.el.style.left = `${String(Math.max(0, box.left - frame.left - width))}px`;
      }
    }
    const first = sub?.rows[0];
    if (focusIt && first !== undefined) {
      show(first);
    }
  }

  function run(row: HTMLElement): void {
    const level = levelOf(row);
    const item = level.items[level.rows.indexOf(row)];
    if (item === undefined) {
      return;
    }
    if (item.disabled === true) {
      show(row);
      return;
    }
    if (item.submenu !== undefined) {
      openSub(row, true);
      return;
    }
    close("item");
    item.run?.();
  }

  function move(row: HTMLElement, key: string): void {
    const { rows } = levelOf(row);
    // A submenu a hover opened goes when a key walks the menu beside it.
    if (levelOf(row) === top) {
      stopHover();
      closeSub(false);
    }
    const at = rows.indexOf(row);
    const next =
      key === "Home"
        ? 0
        : key === "End"
          ? rows.length - 1
          : (at + (key === "ArrowDown" ? 1 : -1) + rows.length) % rows.length;
    const target = rows[next];
    if (target !== undefined) {
      show(target);
    }
  }

  function onKey(event: KeyboardEvent): void {
    // An IME owns these keys, and only the key code tells in some browsers.
    if (event.isComposing || event.keyCode === 229) {
      return;
    }
    const { key } = event;
    // A menu key on an open menu opens no second one. Read before the modifier
    // test: a Mac's menu key is Ctrl+Return (R111).
    const opener =
      isMenuKey(event) ||
      (key === "F10" && !event.ctrlKey && !event.metaKey && !event.altKey);
    if (!opener && (event.ctrlKey || event.metaKey || event.altKey)) {
      return;
    }
    const row = doc.activeElement;
    if (!(row instanceof HTMLElement) || !inside(row)) {
      return;
    }
    if (
      !opener &&
      key !== "Tab" &&
      !SPENT_WHEN_HELD.has(key) &&
      !["ArrowDown", "ArrowUp", "Home", "End"].includes(key)
    ) {
      return;
    }
    event.preventDefault();
    event.stopPropagation();
    if (opener || (event.repeat && SPENT_WHEN_HELD.has(key))) {
      return;
    }
    switch (key) {
      case "Tab":
        close("tab");
        break;
      case "Escape":
        if (sub === null) {
          close("escape");
        } else {
          closeSub(true);
        }
        break;
      case "ArrowLeft":
        closeSub(true);
        break;
      case "ArrowRight":
        if (levelOf(row) === top) {
          openSub(row, true);
        }
        break;
      case "Enter":
      case " ":
        run(row);
        break;
      default:
        move(row, key);
    }
  }

  function wire(level: Level): void {
    level.el.addEventListener("keydown", onKey);
    // A press keeps the focus where it is, and the page's own menu stays away.
    level.el.addEventListener("mousedown", (event) => {
      event.preventDefault();
    });
    level.el.addEventListener("contextmenu", (event) => {
      event.preventDefault();
      event.stopPropagation();
    });
    level.el.addEventListener("focusout", (event) => {
      const next = event.relatedTarget;
      if (next !== null && !inside(next)) {
        close("left");
      }
    });
    for (const row of level.rows) {
      row.addEventListener("click", () => {
        run(row);
      });
      row.addEventListener("mousemove", (event) => {
        if (!pointerMoved(event)) {
          return;
        }
        if (doc.activeElement !== row) {
          show(row, true);
        }
        if (level !== top || owner === row) {
          stopHover();
        } else if (hoverRow !== row) {
          stopHover();
          hoverRow = row;
          hover = setTimeout(() => {
            hover = null;
            hoverRow = null;
            closeSub(false);
            openSub(row, false);
          }, HOVER_MS);
        }
      });
    }
  }

  const onPress = (event: MouseEvent) => {
    if (!inside(event.target)) {
      close("outside");
    }
  };

  function focusBack(): void {
    const el = typeof returnFocus === "function" ? returnFocus() : returnFocus;
    if (el === null) {
      return;
    }
    const to = el.isConnected ? el : host.closest("[tabindex]");
    if (to instanceof HTMLElement || to instanceof SVGElement) {
      to.focus({ preventScroll: true });
    }
  }

  function close(reason: MenuClose): void {
    if (closed) {
      return;
    }
    closed = true;
    stopHover();
    const hadFocus = reason !== "left" && inside(doc.activeElement);
    doc.removeEventListener("mousedown", onPress, true);
    closeSub(false);
    top.el.remove();
    onClose?.(reason);
    if (hadFocus) {
      focusBack();
    }
    if (reason === "outside") {
      // A press on nothing that can take the focus leaves it on the body, and it
      // comes after this handler. A press on something that can keeps it there.
      setTimeout(() => {
        if (doc.activeElement === null || doc.activeElement === doc.body) {
          focusBack();
        }
      }, 0);
    }
  }

  wire(top);
  host.append(top.el);
  place(top.el, at.x, at.y);
  doc.addEventListener("mousedown", onPress, true);
  const first = top.rows[0];
  if (first !== undefined) {
    show(first);
  }

  return {
    close: () => {
      close("api");
    },
    refresh: (next) => {
      if (closed || next.length !== top.items.length) {
        return;
      }
      top.items = next;
      next.forEach((item, i) => {
        const row = top.rows[i];
        if (row !== undefined) {
          describeRow(row, item);
        }
      });
      if (sub !== null && owner !== null) {
        const fresh = next[top.rows.indexOf(owner)]?.submenu;
        if (fresh?.length === sub.items.length) {
          const open = sub;
          open.items = fresh;
          fresh.forEach((item, i) => {
            const row = open.rows[i];
            if (row !== undefined) {
              describeRow(row, item);
            }
          });
        }
      }
      // An item that stopped or started being blocked says so where it stands.
      const active = doc.activeElement;
      if (active instanceof HTMLElement && inside(active)) {
        const level = levelOf(active);
        level.hint.textContent =
          level.items[level.rows.indexOf(active)]?.description ?? "";
      }
    },
    connected: () => top.el.isConnected,
  };
}
