/**
 * The plain DOM menu: its keyboard model, where the focus goes when it closes, and
 * the ways a person leaves it. A menu is built here with nothing of the editor.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { MenuClose, MenuHandle, MenuItem } from "./menu";
import { openMenu } from "./menu";

let host: HTMLElement;
let opener: HTMLButtonElement;
let handle: MenuHandle;
let ran: string[];
let closes: MenuClose[];

const item = (label: string, extra: Partial<MenuItem> = {}): MenuItem => ({
  label,
  run: () => {
    ran.push(label);
  },
  ...extra,
});

const ITEMS = (): MenuItem[] => [
  item("One"),
  {
    label: "Shape",
    submenu: [
      item("Rectangle", { checked: true }),
      item("Circle", { checked: false }),
    ],
  },
  item("Blocked", { disabled: true, description: "It cannot be done." }),
  item("Last"),
];

function open(items = ITEMS()) {
  handle = openMenu(
    host,
    { x: 10, y: 20 },
    "Box actions",
    items,
    opener,
    (reason) => {
      closes.push(reason);
    },
  );
}

const active = () => document.activeElement as HTMLElement;
const menus = () => [...document.querySelectorAll('[role="menu"]')];

/** A pointer event over `el`, at a place in the window. */
function pointer(el: Element | null | undefined, x: number, y = 5): void {
  el?.dispatchEvent(
    new MouseEvent("mousemove", { bubbles: true, clientX: x, clientY: y }),
  );
}

function press(key: string, options: KeyboardEventInit = {}): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  active().dispatchEvent(event);
  return event;
}

beforeEach(() => {
  ran = [];
  closes = [];
  host = document.createElement("div");
  opener = document.createElement("button");
  document.body.append(opener, host);
  opener.focus();
});

afterEach(() => {
  handle.close();
  document.body.replaceChildren();
});

describe("what a menu is", () => {
  it("has the role, the name and the focus on its first item", () => {
    open();
    const [menu] = menus();
    expect(menu?.getAttribute("aria-label")).toBe("Box actions");
    expect(active().textContent).toBe("One");
    expect(active().getAttribute("role")).toBe("menuitem");
    expect(host.contains(menu ?? null)).toBe(true);
  });

  it("names an item by its label alone", () => {
    open();
    expect(active().getAttribute("aria-label")).toBe("One");
  });

  it("is no taller than the room, so it scrolls and nothing is cut off", () => {
    open();
    const menu = menus()[0] as HTMLElement;
    expect(menu.style.maxHeight).toBe(`${String(window.innerHeight)}px`);
  });

  it("stands where it was asked to", () => {
    open();
    const menu = menus()[0] as HTMLElement;
    expect(menu.style.left).toBe("10px");
    expect(menu.style.top).toBe("20px");
  });

  it("marks a submenu's parent and a disabled item", () => {
    open();
    const rows = [...document.querySelectorAll('[role^="menuitem"]')];
    expect(rows[1]?.getAttribute("aria-haspopup")).toBe("menu");
    expect(rows[1]?.getAttribute("aria-expanded")).toBe("false");
    expect(rows[2]?.getAttribute("aria-disabled")).toBe("true");
    expect(rows[2]?.getAttribute("aria-description")).toBe(
      "It cannot be done.",
    );
    expect(rows[0]?.hasAttribute("aria-disabled")).toBe(false);
    expect(rows[0]?.hasAttribute("aria-haspopup")).toBe(false);
  });

  it("gives an item with checked set the radio role", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    const rows = [...(menus()[1]?.querySelectorAll("[role]") ?? [])];
    expect(rows.map((row) => row.getAttribute("role"))).toEqual([
      "menuitemradio",
      "menuitemradio",
    ]);
    expect(rows.map((row) => row.getAttribute("aria-checked"))).toEqual([
      "true",
      "false",
    ]);
  });
});

describe("moving", () => {
  it("wraps with the arrows, and goes through a disabled item", () => {
    open();
    press("ArrowUp");
    expect(active().textContent).toBe("Last");
    press("ArrowDown");
    expect(active().textContent).toBe("One");
    press("ArrowDown");
    press("ArrowDown");
    expect(active().textContent).toBe("Blocked");
  });

  it("jumps with Home and End", () => {
    open();
    press("End");
    expect(active().textContent).toBe("Last");
    press("Home");
    expect(active().textContent).toBe("One");
  });

  it("moves on a held arrow", () => {
    open();
    press("ArrowDown", { repeat: true });
    expect(active().textContent).toBe("Shape");
  });

  it("takes the keys it answers, so none reaches the page", () => {
    open();
    for (const key of ["ArrowDown", "Home", "End", "ArrowUp"]) {
      expect(press(key).defaultPrevented).toBe(true);
    }
  });
});

describe("running an item", () => {
  it("closes, runs once and gives the focus back", () => {
    open();
    press("Enter");
    expect(ran).toEqual(["One"]);
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(opener);
    expect(closes).toEqual(["item"]);
  });

  it("runs on a click, and closes before it runs", () => {
    let during = -1;
    open([
      {
        label: "Now",
        run: () => {
          during = menus().length;
        },
      },
    ]);
    active().click();
    expect(during).toBe(0);
    expect(active()).toBe(opener);
  });

  it("runs nothing on a held Enter", () => {
    open();
    press("Enter", { repeat: true });
    expect(ran).toEqual([]);
    expect(menus()).toHaveLength(1);
  });

  it("does not run a disabled item, stays open and shows why", () => {
    open();
    press("End");
    press("ArrowUp");
    press("Enter");
    expect(ran).toEqual([]);
    expect(menus()).toHaveLength(1);
    expect(active().textContent).toBe("Blocked");
    expect(document.querySelector(".cm-diagram-menu-hint")?.textContent).toBe(
      "It cannot be done.",
    );
  });

  it("does not run a disabled item on a click", () => {
    open();
    const blocked = [...document.querySelectorAll("[role=menuitem]")].find(
      (row) => row.textContent === "Blocked",
    ) as HTMLElement;
    blocked.click();
    expect(ran).toEqual([]);
    expect(menus()).toHaveLength(1);
  });

  it("shows the reason on the way past, and takes it down again", () => {
    open();
    press("End");
    press("ArrowUp");
    const hint = () => document.querySelector(".cm-diagram-menu-hint");
    expect(hint()?.textContent).toBe("It cannot be done.");
    press("ArrowUp");
    expect(hint()?.textContent).toBe("");
  });
});

describe("the submenu", () => {
  it("opens with ArrowRight on its first item", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    expect(menus()).toHaveLength(2);
    expect(menus()[1]?.getAttribute("aria-label")).toBe("Shape");
    expect(active().textContent).toBe("Rectangle");
    expect(
      menus()[0]
        ?.querySelector('[aria-haspopup="menu"]')
        ?.getAttribute("aria-expanded"),
    ).toBe("true");
  });

  it("opens with Enter, and with a click", () => {
    open();
    press("ArrowDown");
    press("Enter");
    expect(menus()).toHaveLength(2);
    expect(ran).toEqual([]);
    press("Escape");
    active().click();
    expect(menus()).toHaveLength(2);
  });

  it("closes with ArrowLeft and with Escape, back on its parent", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    press("ArrowLeft");
    expect(menus()).toHaveLength(1);
    expect(active().textContent).toBe("Shape");
    press("ArrowRight");
    press("Escape");
    expect(menus()).toHaveLength(1);
    expect(active().textContent).toBe("Shape");
    expect(closes).toEqual([]);
  });

  it("wraps inside the submenu", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    press("ArrowUp");
    expect(active().textContent).toBe("Circle");
    press("ArrowDown");
    expect(active().textContent).toBe("Rectangle");
  });

  it("runs a submenu's item and closes both", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    press("ArrowDown");
    press("Enter");
    expect(ran).toEqual(["Circle"]);
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(opener);
  });

  it("does nothing on ArrowRight on a plain item, or ArrowLeft in the top menu", () => {
    open();
    press("ArrowRight");
    expect(menus()).toHaveLength(1);
    expect(active().textContent).toBe("One");
    press("ArrowLeft");
    expect(menus()).toHaveLength(1);
    expect(active().textContent).toBe("One");
  });

  it("does not open or close on a held key", () => {
    open();
    press("ArrowDown");
    press("ArrowRight", { repeat: true });
    expect(menus()).toHaveLength(1);
    press("ArrowRight");
    press("ArrowLeft", { repeat: true });
    press("Escape", { repeat: true });
    expect(menus()).toHaveLength(2);
  });

  it("closes when the pointer stays on another item of the top menu", () => {
    vi.useFakeTimers();
    open();
    press("ArrowDown");
    press("ArrowRight");
    const first = menus()[0]?.querySelector('[role="menuitem"]');
    pointer(first, 1);
    pointer(first, 2);
    expect(active().textContent).toBe("One");
    expect(menus()).toHaveLength(2);
    vi.runAllTimers();
    expect(menus()).toHaveLength(1);
    vi.useRealTimers();
  });

  it("stays when the pointer only crosses another item on its way in", () => {
    vi.useFakeTimers();
    open();
    press("ArrowDown");
    press("ArrowRight");
    const first = menus()[0]?.querySelector('[role="menuitem"]');
    pointer(first, 1);
    pointer(first, 2);
    pointer(menus()[1]?.querySelector('[role="menuitemradio"]'), 3);
    vi.runAllTimers();
    expect(menus()).toHaveLength(2);
    expect(active().textContent).toBe("Rectangle");
    vi.useRealTimers();
  });

  it("opens a submenu under a resting pointer, with the focus left on its parent", () => {
    vi.useFakeTimers();
    open();
    const parent = menus()[0]?.querySelector('[aria-haspopup="menu"]');
    pointer(parent, 1);
    pointer(parent, 2);
    vi.runAllTimers();
    expect(menus()).toHaveLength(2);
    expect(active()).toBe(parent);
    vi.useRealTimers();
  });

  it("does not scroll under a pointer, or the next item comes under it", () => {
    open();
    const spy = vi.spyOn(HTMLElement.prototype, "focus");
    const parent = menus()[0]?.querySelector('[aria-haspopup="menu"]');
    pointer(parent, 1);
    pointer(parent, 2);
    expect(spy).toHaveBeenCalledWith({ preventScroll: true });
    spy.mockRestore();
  });

  it("does not move the focus for a pointer that rests where the menu opened", () => {
    vi.useFakeTimers();
    open();
    const last = menus()[0]?.querySelector('[aria-haspopup="menu"]');
    // What a browser sends when an element appears under a still pointer.
    last?.dispatchEvent(
      new MouseEvent("mouseover", { bubbles: true, clientX: 9, clientY: 9 }),
    );
    pointer(last, 9, 9);
    pointer(last, 9, 9);
    vi.runAllTimers();
    expect(active().textContent).toBe("One");
    expect(menus()).toHaveLength(1);
    press("Enter");
    expect(ran).toEqual(["One"]);
    vi.useRealTimers();
  });

  it("does not move the focus for a pointer resting where a submenu opened", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    const circle = menus()[1]?.querySelectorAll('[role="menuitemradio"]')[1];
    pointer(circle, 9, 9);
    pointer(circle, 9, 9);
    expect(active().textContent).toBe("Rectangle");
    pointer(circle, 10, 9);
    expect(active().textContent).toBe("Circle");
  });

  it("takes a standing submenu down when a key walks the top menu", () => {
    vi.useFakeTimers();
    open();
    const parent = menus()[0]?.querySelector('[aria-haspopup="menu"]');
    pointer(parent, 1);
    pointer(parent, 2);
    vi.runAllTimers();
    expect(menus()).toHaveLength(2);
    press("ArrowDown");
    expect(menus()).toHaveLength(1);
    vi.useRealTimers();
  });
});

describe("leaving", () => {
  it("gives the focus back on Escape, and says how it closed", () => {
    open();
    press("Escape");
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(opener);
    expect(closes).toEqual(["escape"]);
  });

  it("does not close on a held Escape", () => {
    open();
    press("Escape", { repeat: true });
    expect(menus()).toHaveLength(1);
  });

  it("closes on Tab with the focus back, not on the next thing in the page", () => {
    open();
    const event = press("Tab");
    expect(event.defaultPrevented).toBe(true);
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(opener);
    expect(closes).toEqual(["tab"]);
  });

  it("closes on a press outside, and not on one inside", () => {
    open();
    menus()[0]?.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(menus()).toHaveLength(1);
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(menus()).toHaveLength(0);
    expect(closes).toEqual(["outside"]);
  });

  it("does not let a press inside take the focus away", () => {
    open();
    const event = new MouseEvent("mousedown", {
      bubbles: true,
      cancelable: true,
    });
    menus()[0]?.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("takes the page's menu away on itself", () => {
    open();
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
    });
    active().dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
  });

  it("does not take the focus back when the focus went elsewhere", () => {
    open();
    const other = document.createElement("input");
    document.body.append(other);
    other.focus();
    expect(menus()).toHaveLength(0);
    expect(active()).toBe(other);
    expect(closes).toEqual(["left"]);
  });

  it("closes once, whatever asks", () => {
    open();
    handle.close();
    handle.close();
    expect(closes).toEqual(["api"]);
    expect(menus()).toHaveLength(0);
  });

  it("gives the focus back when closed from outside with the focus in it", () => {
    open();
    handle.close();
    expect(active()).toBe(opener);
  });

  it("leaves the focus alone when closed with the focus elsewhere", () => {
    open();
    const other = document.createElement("input");
    document.body.append(other);
    closes = [];
    handle = openMenu(host, { x: 0, y: 0 }, "Second", ITEMS(), opener);
    other.focus();
    handle.close();
    expect(active()).toBe(other);
  });

  it("removes its listeners, so a later press does nothing", () => {
    open();
    press("Escape");
    closes = [];
    document.body.dispatchEvent(new MouseEvent("mousedown", { bubbles: true }));
    expect(closes).toEqual([]);
  });
});

describe("where the focus goes back to", () => {
  it("asks a function at the moment of closing", () => {
    const later = document.createElement("button");
    document.body.append(later);
    const asked = vi.fn(() => later);
    handle = openMenu(host, { x: 0, y: 0 }, "Box actions", ITEMS(), asked);
    expect(asked).not.toHaveBeenCalled();
    press("Escape");
    expect(asked).toHaveBeenCalledTimes(1);
    expect(active()).toBe(later);
  });

  it("does not leave the focus on the body when the element went", () => {
    const frame = document.createElement("div");
    frame.tabIndex = 0;
    frame.append(host);
    document.body.append(frame);
    open();
    opener.remove();
    press("Escape");
    expect(active()).toBe(frame);
  });

  it("answers the function's own no, as its owner places the focus", () => {
    handle = openMenu(host, { x: 0, y: 0 }, "Box actions", ITEMS(), () => null);
    press("Escape");
    expect(active()).toBe(document.body);
  });
});

describe("refresh", () => {
  it("changes what the items say, keeps the focus and the open submenu", () => {
    open();
    press("ArrowDown");
    press("ArrowRight");
    const focused = active();
    const next = ITEMS();
    next[1] = {
      label: "Shape",
      submenu: [
        item("Rectangle", { checked: false }),
        item("Circle", { checked: true }),
      ],
    };
    next[2] = item("Blocked");
    handle.refresh(next);
    expect(active()).toBe(focused);
    const rows = [...(menus()[1]?.querySelectorAll("[role]") ?? [])];
    expect(rows.map((row) => row.getAttribute("aria-checked"))).toEqual([
      "false",
      "true",
    ]);
    const blocked = [...(menus()[0]?.querySelectorAll("[role]") ?? [])].find(
      (row) => row.textContent === "Blocked",
    );
    expect(blocked?.hasAttribute("aria-disabled")).toBe(false);
    expect(blocked?.hasAttribute("aria-description")).toBe(false);
  });

  it("runs what the new items say", () => {
    open();
    const next = ITEMS();
    next[0] = {
      label: "One",
      run: () => {
        ran.push("new one");
      },
    };
    handle.refresh(next);
    press("Enter");
    expect(ran).toEqual(["new one"]);
  });
});

describe("a composition", () => {
  it("leaves the keys to the IME", () => {
    open();
    press("Enter", { isComposing: true });
    expect(ran).toEqual([]);
    press("Enter", { keyCode: 229 });
    expect(ran).toEqual([]);
    expect(menus()).toHaveLength(1);
  });
});
