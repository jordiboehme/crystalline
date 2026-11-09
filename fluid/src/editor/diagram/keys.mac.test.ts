/**
 * A drawn diagram's keys on a Mac (R111). CodeMirror and `platform.ts` read
 * `navigator.platform` once, when they load, so it is set before any import.
 *
 * A Mac has no menu key, and Home, End and F10 need fn on a MacBook. So Ctrl+Return
 * opens a stop's menu, Cmd+Up and Cmd+Down go to the first and last stop, and
 * Cmd+Backspace deletes a stop, next to the keys every platform has.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.hoisted(() => {
  Object.defineProperty(navigator, "platform", {
    value: "MacIntel",
    configurable: true,
  });
});

import { EditorView } from "@codemirror/view";

import {
  bytes,
  cleanupMarkViews,
  keys,
  markView,
  setCaret,
} from "../../test/markKit";
import { isSequenceBody, sequenceDrawing } from "../../test/sequenceDrawing";
import { isMac } from "../../platform";
import { focusTarget } from "./focus";
import type { TargetKey } from "./intent";
import { diagramIntent } from "./intent";
import { prefetchDiagrams } from "./loader";
import { requestVerdict } from "./verdict";
import { forgetDrawings } from "./widget";
import { parseFlowchart } from "./flowchart/parse";
import "./flowchart/actions";

vi.mock("mermaid", async (original) => {
  const real = (await original<{ default: object }>()).default;
  return {
    default: {
      ...real,
      render: async (id: string, body: string) => {
        if (isSequenceBody(body)) {
          return { svg: sequenceDrawing(id, body) };
        }
        const verdict = await requestVerdict(body, false);
        const nodes =
          verdict.ok === true
            ? [...verdict.domIds.keys()]
                .map((dom) => `<g class="node default" id="${id}-${dom}"></g>`)
                .join("")
            : "";
        const edges = parseFlowchart(body)
          .edges.map(
            ({ mermaidId }) =>
              `<path d="M0,0L1,1" data-et="edge" data-id="${mermaidId}"></path>`,
          )
          .join("");
        return {
          svg: `<svg id="${id}"><g class="edgePaths">${edges}</g><g class="nodes">${nodes}</g></svg>`,
        };
      },
    },
  };
});

const SKELETON = "flowchart TD\n    A[First step] --> B[Next step]";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const page = (body: string) => `Above\n${fence(body)}\nBelow\n`;
const NODE_A: TargetKey = { kind: "node", id: "A" };
const NODE_B: TargetKey = { kind: "node", id: "B" };

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

async function drawn(doc = page(SKELETON)) {
  const view = markView(doc);
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
  await flush();
  await flush();
  view.contentDOM.focus();
  return view;
}

const text = (view: EditorView) => bytes(view.state);
const active = () => document.activeElement as HTMLElement;
const fromOf = (view: EditorView) => text(view).indexOf("```");
const intentOf = (view: EditorView) => view.state.field(diagramIntent);
const menus = () => [...document.querySelectorAll('[role="menu"]')];
const box = (view: EditorView) =>
  view.dom.querySelector<HTMLElement>(".cm-diagram");
const promptOf = (view: EditorView) =>
  view.dom.querySelector('.cm-diagram-overlay [role="status"]');

/** A key on `el` (the focus by default), as a browser sends it. */
function press(
  key: string,
  options: KeyboardEventInit = {},
  el: Element = active(),
): KeyboardEvent {
  const event = new KeyboardEvent("keydown", {
    key,
    bubbles: true,
    cancelable: true,
    ...options,
  });
  el.dispatchEvent(event);
  return event;
}

/** Ctrl+Return, the Mac's menu key. */
const ctrlReturn = (el?: Element) => press("Enter", { ctrlKey: true }, el);

function row(label: string): HTMLElement {
  const found = [...document.querySelectorAll('[role^="menuitem"]')].find(
    (el) => el.textContent === label,
  );
  if (found === undefined) throw new Error(`no item ${label}`);
  return found as HTMLElement;
}

async function changed(view: EditorView, from: string) {
  await vi.waitFor(() => {
    expect(text(view)).not.toBe(from);
  });
}

beforeEach(() => {
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
  document.body.replaceChildren();
});

it("runs on a Mac", () => {
  expect(isMac()).toBe(true);
});

describe("the menu key, Ctrl+Return", () => {
  it("opens Box actions on a box, with the focus on Rename", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    const event = ctrlReturn();
    expect(event.defaultPrevented).toBe(true);
    expect(menus()).toHaveLength(1);
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
    expect(active().textContent).toBe("Rename");
    expect(intentOf(view)?.target).toEqual(NODE_B);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("opens Diagram actions on the canvas, and opens no line below", async () => {
    // The fence is last, where Enter on the canvas opens a line below (R104).
    const doc = `Above\n${fence(SKELETON)}`;
    const view = await drawn(doc);
    focusTarget(view, fromOf(view), { kind: "canvas" });
    ctrlReturn();
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Diagram actions");
    expect(text(view)).toBe(doc);
  });

  it("leaves Shift+F10 working on a Mac", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("F10", { shiftKey: true });
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Box actions");
  });

  it("a held Ctrl+Return opens nothing", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("Enter", { ctrlKey: true, repeat: true });
    expect(menus()).toHaveLength(0);
  });

  it("Cmd+Return and Ctrl+Shift+Return are no menu key", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("Enter", { metaKey: true });
    press("Enter", { ctrlKey: true, shiftKey: true });
    expect(menus()).toHaveLength(0);
  });

  it("an open menu takes it and opens no second one", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    ctrlReturn();
    const first = active();
    const event = ctrlReturn();
    expect(event.defaultPrevented).toBe(true);
    expect(menus()).toHaveLength(1);
    expect(active()).toBe(first);
    expect(text(view)).toBe(page(SKELETON));
  });

  it("opens Message actions on a sequence message", async () => {
    const seq =
      "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request";
    const view = await drawn(page(seq));
    focusTarget(view, fromOf(view), {
      kind: "message",
      from: "caller",
      to: "service",
      text: "Request",
      occurrence: 0,
    });
    ctrlReturn();
    expect(menus()[0]?.getAttribute("aria-label")).toBe("Message actions");
  });
});

describe("Ctrl+Return in connect mode", () => {
  async function connecting() {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_A);
    press("F10", { shiftKey: true });
    row("Connect to...").click();
    await flush();
    expect(intentOf(view)?.mode).toBe("connect");
    return view;
  }

  it("chooses nothing and opens nothing while it waits", async () => {
    const view = await connecting();
    press("ArrowRight");
    const event = ctrlReturn();
    await flush();
    expect(event.defaultPrevented).toBe(true);
    expect(text(view)).toBe(page(SKELETON));
    expect(menus()).toHaveLength(0);
    expect(intentOf(view)?.mode).toBe("connect");
  });

  it("owes no contextmenu: a later menu command (VoiceOver's) still ends it", async () => {
    const view = await connecting();
    ctrlReturn();
    // VoiceOver's menu command sends a contextmenu with no press before it.
    const event = new MouseEvent("contextmenu", {
      bubbles: true,
      cancelable: true,
    });
    active().dispatchEvent(event);
    await flush();
    expect(promptOf(view)).toBeNull();
    expect(intentOf(view)?.mode).not.toBe("connect");
  });
});

describe("Ctrl+Return with the box away (R105)", () => {
  const LONG = `${page(SKELETON)}${"Line of text\n\n".repeat(8000)}`;

  it("brings the stop back and opens nothing", async () => {
    const view = await drawn(LONG);
    focusTarget(view, fromOf(view), NODE_B);
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    (view as unknown as { measure(): void }).measure();
    expect(box(view)).toBeNull();
    active().blur();
    await flush();
    const event = ctrlReturn(document.body);
    expect(event.defaultPrevented).toBe(true);
    (view as unknown as { measure(): void }).measure();
    await vi.waitFor(() => {
      expect(box(view)).not.toBeNull();
    });
    await flush();
    expect(active().getAttribute("aria-label")).toBe("Box: Next step");
    expect(menus()).toHaveLength(0);
    expect(text(view)).toBe(LONG);
  });
});

describe("first and last stop", () => {
  it("Cmd+Down goes to the last stop and Cmd+Up to the first box", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), { kind: "canvas" });
    const down = press("ArrowDown", { metaKey: true });
    expect(down.defaultPrevented).toBe(true);
    expect(active().getAttribute("aria-label")).toBe(
      "Arrow from First step to Next step",
    );
    press("ArrowUp", { metaKey: true });
    expect(active().getAttribute("aria-label")).toBe("Box: First step");
  });

  it("Home and End still do", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), { kind: "canvas" });
    press("End");
    expect(active().getAttribute("aria-label")).toBe(
      "Arrow from First step to Next step",
    );
    press("Home");
    expect(active().getAttribute("aria-label")).toBe("Box: First step");
  });

  it("Cmd+Left and Cmd+Right are not the diagram's", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_A);
    expect(press("ArrowRight", { metaKey: true }).defaultPrevented).toBe(false);
    expect(press("ArrowLeft", { metaKey: true }).defaultPrevented).toBe(false);
    expect(active().getAttribute("aria-label")).toBe("Box: First step");
  });
});

describe("deleting a stop", () => {
  const WITHOUT_B = page("flowchart TD\n    A[First step]");

  it("Cmd+Backspace deletes box B", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    expect(press("Backspace", { metaKey: true }).defaultPrevented).toBe(true);
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(WITHOUT_B);
  });

  it("Backspace deletes box B too", async () => {
    const view = await drawn();
    focusTarget(view, fromOf(view), NODE_B);
    press("Backspace");
    await changed(view, page(SKELETON));
    expect(text(view)).toBe(WITHOUT_B);
  });
});

describe("going in from the text", () => {
  it("ArrowDown from the line above goes to the canvas", async () => {
    const view = await drawn();
    setCaret(view, 2);
    expect(keys(view, "ArrowDown")).toBe(true);
    expect(active()).toBe(box(view));
    expect(text(view)).toBe(page(SKELETON));
  });

  it.each(["Mod-Backspace", "Alt-Backspace"])(
    "%s at the start of the line below goes to the canvas and deletes nothing",
    async (name) => {
      const doc = page(SKELETON);
      const view = await drawn();
      setCaret(view, doc.indexOf("Below"));
      expect(keys(view, name)).toBe(true);
      expect(active()).toBe(box(view));
      expect(text(view)).toBe(doc);
    },
  );
});

describe("a sequence message's move, Option+Up and Option+Down", () => {
  const STARTER =
    "sequenceDiagram\n    participant caller as Caller\n    participant service as Service\n    caller->>service: Request\n    service-->>caller: Reply";
  const SWAPPED = STARTER.replace(
    "    caller->>service: Request\n    service-->>caller: Reply",
    "    service-->>caller: Reply\n    caller->>service: Request",
  );

  it("Option+Up moves the message up, and the stop goes with it", async () => {
    const view = await drawn(page(STARTER));
    focusTarget(view, fromOf(view), {
      kind: "message",
      from: "service",
      to: "caller",
      text: "Reply",
      occurrence: 0,
    });
    expect(press("ArrowUp", { altKey: true }).defaultPrevented).toBe(true);
    await changed(view, page(STARTER));
    expect(text(view)).toBe(page(SWAPPED));
    await vi.waitFor(() => {
      expect(active().getAttribute("aria-label")).toBe(
        "Message from Service to Caller: Reply",
      );
    });
  });
});
