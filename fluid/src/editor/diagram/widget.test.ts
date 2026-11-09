/**
 * The drawn diagram on the real surface, with mermaid's render and the verdict
 * stubbed: what a box shows, when it changes, and the height it holds meanwhile.
 * `widget.marks-hidden.test.ts` and `widget.marks-show.test.ts` run this file
 * again in each marks mode. The rendering itself is `widget.real.test.ts` and
 * the browser's numbers are `fluid/e2e/diagram-widget.spec.ts`.
 */

import { EditorState, StateEffect } from "@codemirror/state";
import type { Decoration } from "@codemirror/view";
import { EditorView } from "@codemirror/view";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { cellMarksMode } from "../../test/cellMarksMode";
import type { MarkViewOptions } from "../../test/markKit";
import {
  cleanupMarkViews,
  keys,
  markPeers,
  markView,
  remoteInsert,
  setCaret,
  setRaw,
  undoOnce,
} from "../../test/markKit";
import { initializeMermaid, mermaidConfig } from "../../theme/mermaid";
import SKELETON from "../fixtures/diagrams/svg/f01-skeleton.svg?raw";
import { diagramField, drawUnderCaret } from "./field";
import { prefetchDiagrams } from "./loader";
import type { Verdict } from "./verdict";
import {
  DiagramWidget,
  forgetDrawings,
  hostOf,
  NOTICE_MS,
  onDiagramDrawn,
} from "./widget";

interface Asked {
  id: string;
  body: string;
  resolve: (svg: string) => void;
  reject: (cause: Error) => void;
}

const kit = vi.hoisted(() => ({
  settled: new Map<string, unknown>(),
  waiting: new Map<string, (verdict: unknown) => void>(),
  asked: [] as unknown[],
  /** Whether renders wait for the test to answer them. */
  hold: { value: false },
  initialize: vi.fn(),
}));

vi.mock("mermaid", () => ({
  default: {
    initialize: kit.initialize,
    render: (id: string, body: string) =>
      new Promise((resolve, reject) => {
        const answer = (svg: string) => {
          resolve({ svg });
        };
        kit.asked.push({ id, body, resolve: answer, reject });
        if (!kit.hold.value) {
          answer(recordedAs(id));
        }
      }),
  },
}));

vi.mock("./verdict", () => ({
  verdictFor: (body: string) => kit.settled.get(body),
  requestVerdict: (body: string) =>
    new Promise((resolve) => {
      kit.waiting.set(body, resolve);
    }),
}));

/** The recorded SVG as a render under `id` returns it. */
function recordedAs(id: string): string {
  return SKELETON.replaceAll("rec-f01-skeleton", id);
}

const OK: Verdict = { ok: true, kind: "flowchart", domIds: new Map() };
const BODY = "flowchart TD\n    A[First step] --> B[Next step]";
const fence = (body: string) => `\`\`\`mermaid\n${body}\n\`\`\``;
const DOC = `Top\n\n${fence(BODY)}\n\nEnd\n`;

/** Runs CodeMirror's pending measure now, as a frame would. */
function measured(view: EditorView): void {
  (view as unknown as { measure(): void }).measure();
}

const flush = async () => {
  await new Promise((resolve) => setTimeout(resolve, 0));
};

async function settle(body: string, verdict: Verdict = OK) {
  kit.settled.set(body, verdict);
  kit.waiting.get(body)?.(verdict);
  kit.waiting.delete(body);
  await flush();
}

function modeOptions(): MarkViewOptions {
  const mode = cellMarksMode();
  return mode === "off" ? {} : { mode };
}

/** Every render of the drawn layer, not of the read-only preview. */
function renders(): Asked[] {
  return (kit.asked as Asked[]).filter((r) => r.id.startsWith("cm-diagram-"));
}

const box = (view: EditorView) => {
  const found = view.dom.querySelector<HTMLElement>(".cm-diagram");
  if (found === null) throw new Error("no diagram box");
  return found;
};

/** A view of `doc` with its fences' verdicts ok and the chunk in. */
async function drawn(
  doc = DOC,
  options: MarkViewOptions = {},
  bodies = [BODY],
) {
  for (const body of bodies) {
    kit.settled.set(body, OK);
  }
  const view = markView(doc, { ...modeOptions(), ...options });
  await prefetchDiagrams();
  await vi.waitFor(() => {
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(
      bodies.length,
    );
  });
  await flush();
  return view;
}

beforeEach(() => {
  kit.settled.clear();
  kit.waiting.clear();
  kit.asked.length = 0;
  kit.hold.value = false;
  kit.initialize.mockClear();
  forgetDrawings();
});

afterEach(() => {
  cleanupMarkViews();
  vi.useRealTimers();
});

describe("what the box is", () => {
  it("draws the skeleton as an SVG, with no fence text left in the content", async () => {
    const view = await drawn();
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    const text = view.contentDOM.textContent;
    expect(text).not.toContain("```");
    expect(text).not.toContain("A[First step]");
    expect(view.state.doc.toString()).toBe(DOC);
  });

  it("is one named group with a drawing layer, an overlay layer and a status line", async () => {
    const view = await drawn();
    const dom = box(view);
    expect(dom.getAttribute("role")).toBe("group");
    expect(dom.getAttribute("aria-roledescription")).toBe("diagram");
    expect(dom.getAttribute("aria-label")).toBe("Flowchart");
    expect(dom.tabIndex).toBe(0);
    expect(dom.querySelector(".cm-diagram-overlay")).not.toBeNull();
    const status = dom.querySelector('[role="status"]');
    expect(status?.getAttribute("aria-live")).toBe("polite");
    expect(status?.classList.contains("cm-diagram-notice")).toBe(true);
  });

  it("decodes a label's entity form through drawnLabels", async () => {
    const body = 'flowchart TD\n    A["say #quot;hi#quot;"]';
    kit.hold.value = true;
    const view = await drawn(`Top\n\n${fence(body)}\n\nEnd`, {}, [body]);
    const render = renders().at(-1);
    render?.resolve(
      `<svg id="${String(render.id)}"><text>say &amp;quot;hi&amp;quot;</text></svg>`,
    );
    await flush();
    await vi.waitFor(() => {
      expect(box(view).querySelector("text")?.textContent).toBe('say "hi"');
    });
  });
});

describe("the host", () => {
  it("names the drawing on screen, and the body it was drawn from", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const host = hostOf(view, DOC.indexOf("```"));
    expect(host?.container).toBe(box(view));
    expect(host?.svg()).toBeNull();
    expect(host?.svgId()).toBe("");
    expect(host?.renderedBody()).toBe("");
    const render = renders().at(-1);
    render?.resolve(recordedAs(render.id));
    await flush();
    await vi.waitFor(() => {
      expect(host?.svgId()).toBe(render?.id);
    });
    expect(host?.svg()?.tagName.toLowerCase()).toBe("svg");
    expect(host?.renderedBody()).toBe(BODY);
  });

  it("is found by any position in the fence and by no other", async () => {
    const view = await drawn();
    const from = DOC.indexOf("```");
    const to = DOC.lastIndexOf("```") + 3;
    expect(hostOf(view, from)).not.toBeNull();
    expect(hostOf(view, to)).not.toBeNull();
    expect(hostOf(view, from + 10)).toBe(hostOf(view, from));
    expect(hostOf(view, 0)).toBeNull();
    expect(hostOf(view, DOC.length)).toBeNull();
  });

  it("says a notice for 4 s", async () => {
    const view = await drawn();
    vi.useFakeTimers();
    const host = hostOf(view, DOC.indexOf("```"));
    const status = box(view).querySelector('[role="status"]');
    host?.notice("stale");
    expect(status?.textContent).toBe("The diagram just changed. Try again.");
    vi.advanceTimersByTime(NOTICE_MS - 1);
    expect(status?.textContent).not.toBe("");
    vi.advanceTimersByTime(1);
    expect(status?.textContent).toBe("");
  });

  it("restarts the 4 s for a second notice", async () => {
    const view = await drawn();
    vi.useFakeTimers();
    const host = hostOf(view, DOC.indexOf("```"));
    const status = box(view).querySelector('[role="status"]');
    host?.notice("stale");
    vi.advanceTimersByTime(NOTICE_MS - 1);
    host?.notice("gone");
    vi.advanceTimersByTime(NOTICE_MS - 1);
    expect(status?.textContent).toBe(
      "That part of the diagram is no longer there.",
    );
    vi.advanceTimersByTime(1);
    expect(status?.textContent).toBe("");
  });

  it("stays usable with an empty drawing (R60)", async () => {
    kit.hold.value = true;
    const body = "flowchart TD";
    const view = await drawn(`Top\n\n${fence(body)}\n\nEnd`, {}, [body]);
    const render = renders().at(-1);
    render?.resolve(`<svg id="${String(render.id)}"></svg>`);
    await flush();
    await vi.waitFor(() => {
      expect(box(view).querySelector("svg")).not.toBeNull();
    });
    const host = hostOf(view, 5);
    expect(host?.renderedBody()).toBe(body);
    expect(host?.svgId()).toBe(render?.id);
    host?.notice("gone");
    expect(box(view).querySelector('[role="status"]')?.textContent).not.toBe(
      "",
    );
    box(view).focus();
    expect(document.activeElement).toBe(box(view));
  });

  it("tells a listener each new drawing", async () => {
    const view = await drawn();
    const drawings: string[] = [];
    onDiagramDrawn(view, (host) => {
      drawings.push(host.svgId());
    });
    const at = view.state.doc.toString().indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await flush();
    await vi.waitFor(() => {
      expect(drawings).toHaveLength(1);
    });
    expect(drawings[0]).toBe(renders().at(-1)?.id);
  });
});

describe("events on the box", () => {
  const pressOn = (view: EditorView, key: string) => {
    box(view).dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
    );
  };

  it("leaves the document alone for a key the canvas does not act on (Task 10's keys rest on this)", async () => {
    const doc = `Top line\n\n${fence(BODY)}\n\nEnd`;
    const view = await drawn(doc);
    setCaret(view, 8);
    box(view).focus();
    for (const key of ["ArrowLeft", "Enter", "x"]) {
      pressOn(view, key);
    }
    // No key of the diagram, and one main's keymap writes with: `ignoreEvent` is
    // all that keeps it from adding a line to the text.
    const mac = /Mac/.test(navigator.platform);
    box(view).dispatchEvent(
      new KeyboardEvent("keydown", {
        key: "Enter",
        keyCode: 13,
        metaKey: mac,
        ctrlKey: !mac,
        bubbles: true,
        cancelable: true,
      }),
    );
    expect(view.state.doc.toString()).toBe(doc);
    expect(view.state.selection.main.head).toBe(8);
  });

  it("takes exactly the fence for a Delete on the canvas, nothing of the caret's line", async () => {
    const doc = `Top line\n\n${fence(BODY)}\n\nEnd`;
    const view = await drawn(doc);
    setCaret(view, 8);
    box(view).focus();
    pressOn(view, "Delete");
    expect(view.state.doc.toString()).toBe("Top line\n\n\nEnd");
  });
});

describe("a change to the fence", () => {
  it("keeps the box and its overlay child, and swaps the SVG when the render lands", async () => {
    const view = await drawn(DOC, { room: true });
    const dom = box(view);
    const overlay = dom.querySelector(".cm-diagram-overlay");
    const child = document.createElement("textarea");
    overlay?.append(child);
    const old = dom.querySelector("svg");
    kit.hold.value = true;
    remoteInsert(view, view.state.doc.toString().indexOf("Next"), "X");
    await flush();
    expect(box(view)).toBe(dom);
    expect(dom.querySelector(".cm-diagram-overlay")).toBe(overlay);
    expect(overlay?.firstElementChild).toBe(child);
    // The old drawing stays until the new one lands.
    expect(dom.querySelector("svg")).toBe(old);
    const host = hostOf(view, DOC.indexOf("```"));
    expect(host?.renderedBody()).toBe(BODY);
    const render = renders().at(-1);
    expect(render?.body).toContain("B[XNext step]");
    render?.resolve(recordedAs(render.id));
    await flush();
    await vi.waitFor(() => {
      expect(dom.querySelector("svg")).not.toBe(old);
    });
    expect(dom.querySelector("svg")?.id).toBe(render?.id);
    expect(host?.renderedBody()).toBe(render?.body);
    expect(overlay?.firstElementChild).toBe(child);
  });

  it("lets a late render of an older body lose", async () => {
    const view = await drawn();
    kit.hold.value = true;
    const at = view.state.doc.toString().indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await flush();
    const older = renders().at(-1);
    view.dispatch({ changes: { from: at, insert: "Y" } });
    await flush();
    const newer = renders().at(-1);
    expect(newer).not.toBe(older);
    newer?.resolve(recordedAs(newer.id));
    await flush();
    await vi.waitFor(() => {
      expect(box(view).querySelector("svg")?.id).toBe(newer?.id);
    });
    older?.resolve(recordedAs(older.id));
    await flush();
    await flush();
    expect(box(view).querySelector("svg")?.id).toBe(newer?.id);
    expect(hostOf(view, DOC.indexOf("```"))?.renderedBody()).toBe(newer?.body);
  });

  it("lets a late render lose when an undo went back to a body at hand in the cache", async () => {
    const view = await drawn();
    const at = view.state.doc.toString().indexOf("Next");
    // Body 1 on screen, then body 2, so body 1 is in the cache and not on screen.
    view.dispatch({ changes: { from: at, insert: "X" } });
    await vi.waitFor(() => {
      expect(hostOf(view, 5)?.renderedBody()).toContain("XNext");
    });
    kit.hold.value = true;
    view.dispatch({ changes: { from: at, insert: "Y" } });
    await flush();
    const late = renders().at(-1);
    expect(late?.body).toContain("YXNext");
    view.dispatch({ changes: { from: at, to: at + 2 } });
    expect(hostOf(view, 5)?.renderedBody()).toBe(BODY);
    late?.resolve(recordedAs(late.id));
    await flush();
    await flush();
    expect(hostOf(view, 5)?.renderedBody()).toBe(BODY);
    expect(box(view).querySelector("svg")?.id).not.toBe(late?.id);
  });

  it("starts no retry and keeps the newer render when an older one fails", async () => {
    const view = await drawn();
    kit.hold.value = true;
    const at = view.state.doc.toString().indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await flush();
    const older = renders().at(-1);
    view.dispatch({ changes: { from: at, insert: "Y" } });
    await flush();
    const newer = renders().at(-1);
    expect(newer).not.toBe(older);
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    older?.reject(new Error("no"));
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    await Promise.resolve();
    const count = renders().length;
    vi.advanceTimersByTime(60_000);
    await Promise.resolve();
    expect(renders()).toHaveLength(count);
    // An edit that leaves the same body asks nothing new either: it is running.
    view.dispatch({ changes: { from: 0, insert: "a" } });
    await Promise.resolve();
    expect(renders()).toHaveLength(count);
    vi.useRealTimers();
    newer?.resolve(recordedAs(newer.id));
    await vi.waitFor(() => {
      expect(box(view).querySelector("svg")?.id).toBe(newer?.id);
    });
  });

  it("lets a late render lose when an undo went back to what is on screen", async () => {
    const view = await drawn();
    const old = box(view).querySelector("svg");
    kit.hold.value = true;
    const at = view.state.doc.toString().indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await flush();
    const late = renders().at(-1);
    view.dispatch({ changes: { from: at, to: at + 1 } });
    await flush();
    late?.resolve(recordedAs(late.id));
    await flush();
    await flush();
    expect(box(view).querySelector("svg")).toBe(old);
    expect(hostOf(view, DOC.indexOf("```"))?.renderedBody()).toBe(BODY);
  });

  it("does not render again for an insert above the fence", async () => {
    const view = await drawn();
    const dom = box(view);
    const count = renders().length;
    view.dispatch({ changes: { from: 0, insert: "More\n\n" } });
    await flush();
    expect(box(view)).toBe(dom);
    expect(renders()).toHaveLength(count);
    expect(hostOf(view, DOC.indexOf("```") + 6)?.container).toBe(dom);
  });

  it("renders again with the new theme when dark flips, the old SVG staying", async () => {
    const view = await drawn();
    const old = box(view).querySelector("svg");
    const dom = box(view);
    kit.hold.value = true;
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.darkTheme.of(true)),
    });
    await flush();
    await vi.waitFor(() => {
      expect(kit.initialize).toHaveBeenLastCalledWith(mermaidConfig(true));
    });
    expect(box(view)).toBe(dom);
    expect(dom.querySelector("svg")).toBe(old);
    const render = renders().at(-1);
    render?.resolve(recordedAs(render.id));
    await flush();
    await vi.waitFor(() => {
      expect(dom.querySelector("svg")?.id).toBe(render?.id);
    });
  });

  it("leaves each box with its own fence when a peer changes both in one go", async () => {
    const first = "flowchart TD\n    A --> B";
    const second = "flowchart TD\n    C --> D";
    const doc = `Top\n\n${fence(first)}\n\nMiddle\n\n${fence(second)}\n\nEnd`;
    const view = await drawn(doc, { room: true }, [first, second]);
    const [one, two] = view.dom.querySelectorAll<HTMLElement>(".cm-diagram");
    const text = view.state.doc.toString();
    view.dispatch({
      changes: [
        { from: text.indexOf(" B\n") + 1, insert: "X" },
        { from: text.indexOf(" D\n") + 1, insert: "Y" },
      ],
    });
    await flush();
    await vi.waitFor(() => {
      expect(hostOf(view, 5)?.renderedBody()).toBe(
        `flowchart TD\n    A --> XB`,
      );
    });
    expect(hostOf(view, 5)?.container).toBe(one);
    const later = view.state.doc.toString().lastIndexOf("```");
    expect(hostOf(view, later)?.container).toBe(two);
    expect(hostOf(view, later)?.renderedBody()).toBe(
      `flowchart TD\n    C --> YD`,
    );
  });

  // R76: CodeMirror reuses a box on an `eq` match without asking `updateDOM`.
  it("gives a far fence of the same body its own box when scrolling to it", async () => {
    const filler = "Line of text\n\n".repeat(8000);
    const doc = `Top\n\n${fence(BODY)}\n\n${filler}${fence(BODY)}\n\nEnd`;
    const view = await drawn(doc);
    const first = box(view);
    const id = first.querySelector("svg")?.id;
    first
      .querySelector(".cm-diagram-overlay")
      ?.append(document.createElement("textarea"));
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    measured(view);
    const second = box(view);
    expect(second).not.toBe(first);
    expect(second.querySelector(".cm-diagram-overlay")?.childElementCount).toBe(
      0,
    );
    expect(second.querySelector("svg")?.id).not.toBe(id);
  });

  it("keeps the second box when the first of two fences of one body goes", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\nMiddle\n\n${fence(BODY)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, BODY]);
    const second = view.dom.querySelectorAll<HTMLElement>(".cm-diagram")[1];
    view.dispatch({
      changes: { from: doc.indexOf("```"), to: doc.indexOf("Middle") },
    });
    const left = view.dom.querySelectorAll<HTMLElement>(".cm-diagram");
    expect(left).toHaveLength(1);
    expect(left[0]).toBe(second);
  });

  it("does not hand a removed fence's box, with its drawing and overlay, to the next fence", async () => {
    const first = "flowchart TD\n    A --> B";
    const second = "flowchart TD\n    C --> D";
    const doc = `Top\n\n${fence(first)}\n\n${fence(second)}\n\nEnd`;
    const view = await drawn(doc, {}, [first, second]);
    const [one, two] = view.dom.querySelectorAll<HTMLElement>(".cm-diagram");
    one
      ?.querySelector(".cm-diagram-overlay")
      ?.append(document.createElement("p"));
    // The first fence becomes a third body by a whole replace of it, so no
    // widget equals its box, and a third fence takes the second's place.
    const third = "flowchart TD\n    E --> F";
    kit.settled.set(third, OK);
    view.dispatch({
      changes: {
        from: 5,
        to: doc.indexOf("End"),
        insert: `${fence(third)}\n\n`,
      },
    });
    await flush();
    const boxes = view.dom.querySelectorAll<HTMLElement>(".cm-diagram");
    expect(boxes).toHaveLength(1);
    expect(boxes[0]).not.toBe(one);
    expect(boxes[0]).not.toBe(two);
    expect(
      boxes[0]?.querySelector(".cm-diagram-overlay")?.childElementCount,
    ).toBe(0);
  });
});

describe("the flip from the preview (R54, N6)", () => {
  it("moves the preview's drawing into the box, with no second render", async () => {
    const view = markView(DOC, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    const drawing = view.dom.querySelector(".cm-mermaid-preview svg");
    const id = drawing?.id;
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    expect(view.dom.querySelector(".cm-mermaid-preview")).toBeNull();
    expect(box(view).querySelector("svg")).toBe(drawing);
    expect(renders()).toHaveLength(0);
    const host = hostOf(view, DOC.indexOf("```"));
    expect(host?.svgId()).toBe(id);
    expect(host?.renderedBody()).toBe(BODY);
    expect(box(view).style.minHeight).toBe("");
  });

  // The second fence arrives after the first drew: two `import("mermaid")` in
  // one tick from `fencePreviews` get the real module beside vitest's mock.
  async function twoPreviews(second: string) {
    const view = markView(`Top\n\n${fence(BODY)}\n\nEnd`, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    view.dispatch({
      changes: {
        from: view.state.doc.toString().indexOf("End"),
        insert: `${fence(second)}\n\n`,
      },
    });
    await vi.waitFor(() => {
      expect(view.dom.querySelectorAll(".cm-mermaid-preview svg")).toHaveLength(
        2,
      );
    });
    return view;
  }

  it("leaves a quoted fence of the same body its own drawing", async () => {
    const quoted = `> \`\`\`mermaid\n${BODY.split("\n")
      .map((line) => `> ${line}`)
      .join("\n")}\n> \`\`\``;
    const view = markView(`Top\n\n${quoted}\n\nEnd`, modeOptions());
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    const theirs = view.dom.querySelector(".cm-mermaid-preview svg");
    view.dispatch({
      changes: {
        from: view.state.doc.toString().indexOf("End"),
        insert: `${fence(BODY)}\n\n`,
      },
    });
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelectorAll(".cm-mermaid-preview svg")).toHaveLength(
        2,
      );
    });
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    // The quoted preview keeps what it drew; the flip took the box's own.
    expect(view.dom.querySelector(".cm-mermaid-preview svg")).toBe(theirs);
    expect(box(view).querySelector("svg")).not.toBe(theirs);
    expect(box(view).querySelector("svg")).not.toBeNull();
  });

  it("leaves a held fence of the same body its own drawing", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\nEnd`;
    const view = markView(doc, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    view.dispatch({
      changes: { from: doc.indexOf("End"), insert: `${fence(BODY)}\n\n` },
    });
    await vi.waitFor(() => {
      expect(view.dom.querySelectorAll(".cm-mermaid-preview svg")).toHaveLength(
        2,
      );
    });
    const [first, second] = view.dom.querySelectorAll(
      ".cm-mermaid-preview svg",
    );
    // The caret holds the first fence as text; only the second flips.
    setCaret(view, doc.indexOf("flowchart"), "select");
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    expect(view.dom.querySelectorAll(".cm-diagram")).toHaveLength(1);
    expect(view.dom.querySelector(".cm-mermaid-preview svg")).toBe(first);
    expect(box(view).querySelector("svg")).toBe(second);
  });

  it("takes only the drawing of its own body", async () => {
    const other = "flowchart TD\n    C --> D";
    const view = await twoPreviews(other);
    const [mine, theirs] = view.dom.querySelectorAll(".cm-mermaid-preview svg");
    await vi.waitFor(() => {
      expect(kit.waiting.has(other)).toBe(true);
    });
    await settle(other);
    expect(box(view).querySelector("svg")).toBe(theirs);
    expect(view.dom.querySelector(".cm-mermaid-preview svg")).toBe(mine);
  });

  it("keeps the two previews of one body one each", async () => {
    const view = await twoPreviews(BODY);
    await settle(BODY);
    const boxes = view.dom.querySelectorAll(".cm-diagram");
    expect(boxes).toHaveLength(2);
    expect(boxes[0]?.querySelector("svg")).not.toBeNull();
    expect(boxes[1]?.querySelector("svg")).not.toBeNull();
    expect(boxes[0]?.querySelector("svg")?.id).not.toBe(
      boxes[1]?.querySelector("svg")?.id,
    );
    expect(renders()).toHaveLength(0);
  });

  it("holds a height until its SVG arrives when the preview has drawn nothing", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const dom = box(view);
    expect(dom.querySelector("svg")).toBeNull();
    expect(Number.parseFloat(dom.style.minHeight)).toBeGreaterThan(48);
    const render = renders().at(-1);
    render?.resolve(recordedAs(render.id));
    await flush();
    await vi.waitFor(() => {
      expect(dom.querySelector("svg")).not.toBeNull();
    });
    expect(dom.style.minHeight).toBe("");
  });
});

describe("off screen", () => {
  const widgetOf = (view: EditorView) => {
    let widget: DiagramWidget | null = null;
    view.state
      .field(diagramField)
      .decorations.between(
        0,
        view.state.doc.length,
        (_from, _to, deco: Decoration) => {
          widget = (deco.spec as { widget: DiagramWidget }).widget;
        },
      );
    if (widget === null) throw new Error("no widget");
    return widget as DiagramWidget;
  };

  it("gives the height map a height before anything was measured, taller for a longer body", async () => {
    const long = `flowchart TD\n${Array.from({ length: 5 }, (_, i) => `    N${String(i)} --> N${String(i + 1)}`).join("\n")}`;
    const view = await drawn();
    const short = widgetOf(view).estimatedHeight;
    expect(short).toBeGreaterThan(48);
    const other = await drawn(`Top\n\n${fence(long)}\n\nEnd`, {}, [long]);
    expect(widgetOf(other).estimatedHeight).toBeGreaterThan(short);
  });

  it("estimates a sideways diagram lower than a tall one, and caps a long source", () => {
    const edges = (n: number) =>
      Array.from(
        { length: n },
        (_, i) => `    N${String(i)} --> N${String(i + 1)}`,
      ).join("\n");
    const of = (body: string) =>
      new DiagramWidget(body, false, 1, 1).estimatedHeight;
    expect(of(`flowchart TD\n${edges(3)}`)).toBe(374);
    expect(of(`flowchart LR\n${edges(3)}`)).toBe(216);
    expect(of(`graph RL\n${edges(3)}`)).toBe(216);
    expect(of(`flowchart TD\n${edges(50)}`)).toBe(800);
    expect(of(`flowchart LR\n${edges(50)}`)).toBe(800);
  });

  it("gives the height map the measured height once one is known", async () => {
    const body = "flowchart TD\n    M --> N";
    const spy = vi
      .spyOn(HTMLElement.prototype, "getBoundingClientRect")
      .mockImplementation(function rect(this: HTMLElement) {
        const height = this.classList.contains("cm-diagram") ? 321 : 0;
        return {
          height,
          width: 0,
          top: 0,
          left: 0,
          right: 0,
          bottom: height,
          x: 0,
          y: 0,
          toJSON: () => ({}),
        };
      });
    try {
      const view = await drawn(`Top\n\n${fence(body)}\n\nEnd`, {}, [body]);
      measured(view);
      await vi.waitFor(() => {
        expect(widgetOf(view).estimatedHeight).toBe(321);
      });
    } finally {
      spy.mockRestore();
    }
  });

  it("pastes its drawing in the same step when the fence comes back, with no render", async () => {
    const long = `${DOC}${"Line of text\n\n".repeat(8000)}`;
    const view = await drawn(long);
    const id = box(view).querySelector("svg")?.id;
    const count = kit.asked.length;
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    measured(view);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    view.dispatch({ effects: EditorView.scrollIntoView(0) });
    measured(view);
    // In this very step, not after a render.
    expect(box(view).querySelector("svg")?.id).toBe(id);
    expect(box(view).style.minHeight).toBe("");
    await flush();
    expect(kit.asked).toHaveLength(count);
  });

  describe("a drawing landing above the window, the page scrolling outside the editor", () => {
    function rects(top: number, bodyTop = 0) {
      return vi
        .spyOn(HTMLElement.prototype, "getBoundingClientRect")
        .mockImplementation(function rect(this: HTMLElement) {
          const height = this.classList.contains("cm-diagram")
            ? this.querySelector("svg") === null
              ? 300
              : 374
            : 0;
          return {
            height,
            width: 0,
            top: this.classList.contains("cm-diagram")
              ? top
              : this === document.body
                ? bodyTop
                : 0,
            left: 0,
            right: 0,
            bottom: top + height,
            x: 0,
            y: top,
            toJSON: () => ({}),
          };
        });
    }

    async function land(top: number) {
      const scrollBy = vi.fn();
      // jsdom has neither.
      const scroller = document.documentElement;
      scroller.scrollBy = scrollBy;
      Object.defineProperty(document, "scrollingElement", {
        configurable: true,
        value: scroller,
      });
      kit.hold.value = true;
      const view = await drawn();
      const spy = rects(top);
      try {
        const render = renders().at(-1);
        render?.resolve(recordedAs(render.id));
        await vi.waitFor(() => {
          expect(box(view).querySelector("svg")).not.toBeNull();
        });
        measured(view);
      } finally {
        spy.mockRestore();
      }
      return scrollBy;
    }

    it("takes the difference on the scroller when the box starts above the window", async () => {
      const scrollBy = await land(-500);
      expect(scrollBy).toHaveBeenCalledTimes(1);
      expect(scrollBy).toHaveBeenCalledWith({
        top: 74,
        behavior: "instant",
      });
    });

    it("finds a scroller further out, and reads the window's top from it", async () => {
      const scrollBy = vi.fn();
      const scroller = document.body;
      scroller.scrollBy = scrollBy;
      // A rule, not the style attribute: jsdom's computed style may leave that out.
      const rule = document.createElement("style");
      rule.textContent = "body { overflow-y: auto; }";
      document.head.append(rule);
      const heights = vi
        .spyOn(scroller, "scrollHeight", "get")
        .mockReturnValue(5_000);
      const fallback = vi.fn();
      document.documentElement.scrollBy = fallback;
      Object.defineProperty(document, "scrollingElement", {
        configurable: true,
        value: document.documentElement,
      });
      kit.hold.value = true;
      const view = await drawn();
      // The box is below the page's top and above the top of its scroller.
      const spy = rects(50, 100);
      try {
        const render = renders().at(-1);
        render?.resolve(recordedAs(render.id));
        await vi.waitFor(() => {
          expect(box(view).querySelector("svg")).not.toBeNull();
        });
        measured(view);
        expect(scrollBy).toHaveBeenCalledWith({ top: 74, behavior: "instant" });
        expect(fallback).not.toHaveBeenCalled();
      } finally {
        spy.mockRestore();
        heights.mockRestore();
        rule.remove();
      }
    });

    it("leaves the scroller alone when the box is in the window", async () => {
      const scrollBy = await land(100);
      expect(scrollBy).not.toHaveBeenCalled();
    });
  });

  it("has a flipped fence's drawing at hand when it comes back, with no render", async () => {
    const long = `${DOC}${"Line of text\n\n".repeat(8000)}`;
    const view = markView(long, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    const id = box(view).querySelector("svg")?.id;
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    measured(view);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    const count = kit.asked.length;
    view.dispatch({ effects: EditorView.scrollIntoView(0) });
    measured(view);
    expect(box(view).querySelector("svg")?.id).toBe(id);
    await flush();
    expect(kit.asked).toHaveLength(count);
  });

  it("takes no preview drawn across a theme change (R88)", async () => {
    kit.hold.value = true;
    const view = markView(DOC, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(
        (kit.asked as Asked[]).some((r) => r.id.startsWith("cm-mermaid-")),
      ).toBe(true);
    });
    const preview = (kit.asked as Asked[]).find((r) =>
      r.id.startsWith("cm-mermaid-"),
    );
    // Another surface asks for the other theme and back while it waits.
    const other = { initialize: vi.fn() };
    initializeMermaid(other, true);
    initializeMermaid(other, false);
    preview?.resolve(recordedAs(preview.id));
    await vi.waitFor(() => {
      expect(view.dom.querySelector(".cm-mermaid-preview svg")).not.toBeNull();
    });
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    expect(box(view).querySelector("svg")?.id ?? "").not.toBe(preview?.id);
    expect(renders().length).toBeGreaterThan(0);
  });

  it("gives a box that left and a render still running no way back in", async () => {
    kit.hold.value = true;
    const long = `${DOC}${"Line of text\n\n".repeat(8000)}`;
    const view = await drawn(long);
    const render = renders().at(-1);
    view.dispatch({
      effects: EditorView.scrollIntoView(view.state.doc.length),
    });
    measured(view);
    render?.resolve(recordedAs(render.id));
    await flush();
    await flush();
    expect(view.dom.querySelector("svg")).toBeNull();
    // What it drew is at hand when the fence comes back.
    view.dispatch({ effects: EditorView.scrollIntoView(0) });
    measured(view);
    expect(box(view).querySelector("svg")?.id).toBe(render?.id);
  });

  it("gives a copy of a drawing on screen an id of its own, with no render", async () => {
    const view = await drawn();
    const id = box(view).querySelector("svg")?.id ?? "";
    const count = kit.asked.length;
    view.dispatch({
      changes: {
        from: view.state.doc.toString().indexOf("End"),
        insert: `${fence(BODY)}\n\n`,
      },
    });
    const svgs = view.dom.querySelectorAll(".cm-diagram svg");
    expect(svgs).toHaveLength(2);
    expect(svgs[0]?.id).toBe(id);
    expect(svgs[1]?.id).not.toBe(id);
    expect(svgs[1]?.innerHTML).not.toContain(id);
    expect(svgs[1]?.innerHTML).toContain(svgs[1]?.id);
    await flush();
    expect(kit.asked).toHaveLength(count);
  });

  it("does not paste a drawing whose id is on screen in another box", async () => {
    const doc = `Top\n\n${fence(BODY)}\n\nMiddle\n\n${fence(BODY)}\n\nEnd`;
    const view = await drawn(doc, {}, [BODY, BODY]);
    await vi.waitFor(() => {
      expect(view.dom.querySelectorAll(".cm-diagram svg")).toHaveLength(2);
    });
    const ids = [...view.dom.querySelectorAll(".cm-diagram svg")].map(
      (svg) => svg.id,
    );
    expect(new Set(ids).size).toBe(2);
  });

  it("stops a dropped box's render and retry", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const old = box(view);
    const widget = widgetOf(view);
    widget.destroy(old);
    const render = renders().at(-1);
    render?.reject(new Error("no"));
    await flush();
    vi.useFakeTimers();
    const count = renders().length;
    vi.advanceTimersByTime(60_000);
    expect(renders()).toHaveLength(count);
  });
});

describe("render ids and what mermaid leaves behind", () => {
  it("never repeats an id, across views either", async () => {
    kit.hold.value = true;
    const one = await drawn();
    const two = await drawn();
    expect(one.dom.querySelector(".cm-diagram")).not.toBeNull();
    expect(two.dom.querySelector(".cm-diagram")).not.toBeNull();
    const ids = renders().map((r) => r.id);
    expect(ids.length).toBeGreaterThanOrEqual(2);
    expect(new Set(ids).size).toBe(ids.length);
    expect(ids.every((id) => /^cm-diagram-\d+$/.test(id))).toBe(true);
  });

  it("removes the temporary d<id> element of a render that failed", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const render = renders().at(-1);
    const temp = document.createElement("div");
    temp.id = `d${String(render?.id)}`;
    document.body.append(temp);
    render?.reject(new Error("no"));
    await flush();
    await vi.waitFor(() => {
      expect(document.getElementById(temp.id)).toBeNull();
    });
    expect(box(view).querySelector("svg")).toBeNull();
  });
});

describe("a theme change while a render is queued (mermaid's queue)", () => {
  it("does not cache a drawing that ran under the other theme", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const light = renders().at(-1);
    expect(light?.body).toBe(BODY);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.darkTheme.of(true)),
    });
    await vi.waitFor(() => {
      expect(kit.initialize).toHaveBeenLastCalledWith(mermaidConfig(true));
    });
    // The queued light render finishes under the dark configuration.
    light?.resolve(recordedAs(light.id));
    await flush();
    await flush();
    // A light view of this body has nothing at hand: it asks for its own render.
    const before = renders().length;
    kit.hold.value = false;
    const other = await drawn();
    await vi.waitFor(() => {
      expect(box(other).querySelector("svg")).not.toBeNull();
    });
    expect(renders().length).toBeGreaterThan(before);
    expect(box(other).querySelector("svg")?.id).not.toBe(light?.id);
  });

  it("renders again when the box still wants what ran under the other theme", async () => {
    kit.hold.value = true;
    const one = await drawn();
    const first = renders().at(-1);
    // Another box's render of the dark theme comes in between.
    const body = "flowchart TD\n    C --> D";
    kit.settled.set(body, OK);
    const dark = markView(`Top\n\n${fence(body)}\n\nEnd`, modeOptions());
    dark.dispatch({
      effects: StateEffect.appendConfig.of(EditorView.darkTheme.of(true)),
    });
    await vi.waitFor(() => {
      expect(kit.initialize).toHaveBeenLastCalledWith(mermaidConfig(true));
    });
    first?.resolve(recordedAs(first.id));
    await vi.waitFor(() => {
      expect(renders().filter((r) => r.body === BODY).length).toBeGreaterThan(
        1,
      );
    });
    expect(box(one).querySelector("svg")).toBeNull();
  });
});

describe("a render that fails", () => {
  it("keeps the old drawing and tries again later", async () => {
    const view = await drawn();
    const old = box(view).querySelector("svg");
    kit.hold.value = true;
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    const at = view.state.doc.toString().indexOf("Next");
    view.dispatch({ changes: { from: at, insert: "X" } });
    await vi.waitFor(() => {
      expect(renders().at(-1)?.body).toContain("XNext");
    });
    const failed = renders().at(-1);
    failed?.reject(new Error("no"));
    await vi.waitFor(async () => {
      await Promise.resolve();
      vi.advanceTimersByTime(5_000);
      expect(renders().at(-1)).not.toBe(failed);
    });
    expect(box(view).querySelector("svg")).toBe(old);
    const next = renders().at(-1);
    expect(next?.id).not.toBe(failed?.id);
    expect(next?.body).toBe(failed?.body);
    next?.resolve(recordedAs(next.id));
    await vi.waitFor(() => {
      expect(box(view).querySelector("svg")?.id).toBe(next?.id);
    });
  });
});

describe("the caret and deleting", () => {
  it("makes the fence one atomic range", async () => {
    const view = await drawn();
    const from = DOC.indexOf("```");
    const to = DOC.lastIndexOf("```") + 3;
    const set = view.state
      .facet(EditorView.atomicRanges)
      .map((get) => get(view));
    const covers = set.some((ranges) => {
      let found = false;
      ranges.between(from, to, (a, b) => {
        found ||= a === from && b === to;
      });
      return found;
    });
    expect(covers).toBe(true);
  });

  // R73: the line right below never joins the closing mark.
  it("leaves the line right below on its own line on Backspace, and goes to the canvas", async () => {
    const doc = `Top\n\n${fence(BODY)}\nBelow\n`;
    const view = await drawn(doc);
    setCaret(view, doc.indexOf("Below"));
    keys(view, "Backspace");
    expect(view.state.doc.toString()).toBe(doc);
    expect(document.activeElement).toBe(box(view));
  });

  it("restores a fence a selection deleted with one undo", async () => {
    const view = await drawn();
    view.dispatch({
      selection: { anchor: 2, head: DOC.indexOf("End") },
      userEvent: "select",
    });
    keys(view, "Backspace");
    expect(view.state.doc.toString()).not.toContain("flowchart");
    undoOnce(view);
    expect(view.state.doc.toString()).toBe(DOC);
  });
});

describe("trying a render again (R79 N5)", () => {
  const edited = (n: number) => BODY.replace("Next", `Next${String(n)}`);

  function edit(view: EditorView, from: string, to: string) {
    const at = view.state.doc.toString().indexOf(from);
    view.dispatch({ changes: { from: at, to: at + from.length, insert: to } });
  }

  it("drops the retry of a failed render when the next edit draws, so the drawing stays (w18)", async () => {
    const view = await drawn();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    kit.hold.value = true;
    edit(view, "Next", "Next1");
    await vi.advanceTimersByTimeAsync(0);
    renders().at(-1)?.reject(new Error("no"));
    await vi.advanceTimersByTimeAsync(0);
    edit(view, "Next1", "Next2");
    await vi.advanceTimersByTimeAsync(0);
    const last = renders().at(-1);
    last?.resolve(recordedAs(last.id));
    await vi.advanceTimersByTimeAsync(0);
    const svg = box(view).querySelector("svg");
    expect(svg?.id).toBe(last?.id);
    const count = renders().length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(renders()).toHaveLength(count);
    expect(box(view).querySelector("svg")).toBe(svg);
  });

  it("starts its tries again for an edit after they ran out (w4)", async () => {
    const view = await drawn();
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    kit.hold.value = true;
    edit(view, "Next", "Next1");
    for (const wait of [5_000, 30_000, 0]) {
      await vi.advanceTimersByTimeAsync(0);
      renders().at(-1)?.reject(new Error("no"));
      await vi.advanceTimersByTimeAsync(wait);
    }
    const spent = renders().length;
    await vi.advanceTimersByTimeAsync(60_000);
    expect(renders()).toHaveLength(spent);
    expect(view.state.doc.toString()).toContain(edited(1));
    edit(view, "Next1", "Next2");
    await vi.advanceTimersByTimeAsync(0);
    expect(renders()).toHaveLength(spent + 1);
    renders().at(-1)?.reject(new Error("no"));
    await vi.advanceTimersByTimeAsync(5_000);
    expect(renders()).toHaveLength(spent + 2);
  });
});

describe("what a held fence does (R52)", () => {
  it("is not drawn while the caret is in it, and draws when the caret leaves", async () => {
    const view = markView(DOC, modeOptions());
    setCaret(view, DOC.indexOf("flowchart"), "select");
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(view.contentDOM.textContent).toContain("flowchart TD");
    setCaret(view, DOC.indexOf("End"), "select");
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
  });

  it("draws under the caret through the starter's hook", async () => {
    const view = markView(DOC, modeOptions());
    setCaret(view, DOC.indexOf("flowchart"), "select");
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(kit.waiting.has(BODY)).toBe(true);
    });
    await settle(BODY);
    view.dispatch({ effects: drawUnderCaret.of(DOC.indexOf("```")) });
    expect(view.dom.querySelector(".cm-diagram")).not.toBeNull();
  });
});

describe("on a page that takes no writes", () => {
  it("draws, and says it takes none", async () => {
    const view = await drawn();
    expect(hostOf(view, DOC.indexOf("```"))?.writable()).toBe(true);
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
    await flush();
    expect(box(view).querySelector("svg")).not.toBeNull();
    expect(hostOf(view, DOC.indexOf("```"))?.writable()).toBe(false);
  });
});

describe("Raw and two views", () => {
  it("takes the box away on Raw and pastes the drawing back when Raw goes off", async () => {
    const view = await drawn();
    const count = renders().length;
    setRaw(view, true);
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(view.contentDOM.textContent).toContain("flowchart TD");
    setRaw(view, false);
    expect(box(view).querySelector("svg")).not.toBeNull();
    await flush();
    expect(renders()).toHaveLength(count);
  });

  it("gives a render running at Raw on no box to land in", async () => {
    kit.hold.value = true;
    const view = await drawn();
    const render = renders().at(-1);
    setRaw(view, true);
    render?.resolve(recordedAs(render.id));
    await flush();
    expect(view.dom.querySelector(".cm-diagram")).toBeNull();
    expect(view.dom.querySelector("svg")).toBeNull();
  });

  it("draws one fence in two views of a room, each with its own drawing and id", async () => {
    kit.settled.set(BODY, OK);
    const { local, peer, sync } = markPeers(DOC, modeOptions());
    await prefetchDiagrams();
    await vi.waitFor(() => {
      expect(local.dom.querySelector(".cm-diagram svg")).not.toBeNull();
      expect(peer.dom.querySelector(".cm-diagram svg")).not.toBeNull();
    });
    const mine = box(local).querySelector("svg")?.id;
    const theirs = box(peer).querySelector("svg")?.id;
    expect(mine).not.toBe(theirs);
    expect(document.querySelectorAll(`[id="${String(mine)}"]`)).toHaveLength(1);
    // A change from the one reaches the other through the room.
    const at = local.state.doc.toString().indexOf("Next");
    local.dispatch({ changes: { from: at, insert: "X" } });
    sync();
    await flush();
    await vi.waitFor(() => {
      expect(hostOf(peer, DOC.indexOf("```"))?.renderedBody()).toContain(
        "XNext",
      );
    });
    expect(hostOf(local, DOC.indexOf("```"))?.renderedBody()).toContain(
      "XNext",
    );
    const ids = [...document.querySelectorAll(".cm-diagram svg")].map(
      (svg) => svg.id,
    );
    expect(new Set(ids).size).toBe(ids.length);
  });
});

describe("a peer's line at the fence's first mark", () => {
  it("keeps the box element, and what its overlay holds", async () => {
    const view = await drawn(DOC, { room: true });
    const before = view.dom.querySelector<HTMLElement>(".cm-diagram");
    const held = document.createElement("textarea");
    view.dom.querySelector(".cm-diagram-overlay")?.append(held);
    held.focus();
    remoteInsert(view, DOC.indexOf("```"), "peer line\n\n");
    await new Promise((resolve) => setTimeout(resolve, 0));
    expect(view.dom.querySelector(".cm-diagram")).toBe(before);
    expect(held.isConnected).toBe(true);
    expect(document.activeElement).toBe(held);
    const moved = view.state.doc.toString().indexOf("```");
    expect(hostOf(view, moved)?.container).toBe(before);
    expect(view.dom.querySelector(".cm-diagram svg")).not.toBeNull();
  });
});
