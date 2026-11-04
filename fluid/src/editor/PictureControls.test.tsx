/**
 * The floating bar on a selected picture: its placement and size buttons,
 * its alt text field and its Remove picture, each run on the main view the
 * way the picture verbs run there. Every document is written out by hand
 * and every view reads a full parse (`markView`).
 */

import type { EditorView } from "@codemirror/view";
import { act, fireEvent, render, screen, within } from "@testing-library/react";
import type { ReactElement } from "react";
import type { MockInstance } from "vitest";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import { Tooltips } from "../components/primitives";
import {
  bytes,
  cleanupMarkViews,
  markPeers,
  markView,
  setRaw,
  undoOnce,
  undoSteps,
} from "../test/markKit";
import FloatingBar from "./FloatingBar";
import type { FloatingContext } from "./floatingContext";
import { imagePreviews } from "./imagePreviews";
import type { PictureAlign, PictureSize } from "./pictures/verbs";
import {
  removePicture,
  setPictureAlign,
  setPictureSize,
} from "./pictures/verbs";

let errors: MockInstance<typeof console.error>;

beforeEach(() => {
  errors = vi.spyOn(console, "error");
});

afterEach(() => {
  cleanupMarkViews();
  expect(errors).not.toHaveBeenCalled();
  errors.mockRestore();
});

const PICTURE = "![Chart](assets/c.png)";
const DOC = `Before\n\n${PICTURE}\n\nAfter\n`;

interface Harness {
  main: EditorView;
  context: () => FloatingContext;
  /** Draws the bar for the latest context, the way a screen does. */
  show: () => void;
  /** Flips Raw the way the engram screen does: the bar is told null. */
  raw: (on: boolean) => void;
}

function harness(
  doc = DOC,
  build: (options: {
    images: ReturnType<typeof imagePreviews>;
    onFloatingContext: (context: FloatingContext) => void;
  }) => EditorView = (options) => markView(doc, options),
): Harness {
  let latest: FloatingContext = null;
  let raw = false;
  let rerender: ((ui: ReactElement) => void) | null = null;
  const ui = () => (
    <FloatingBar main={main} context={raw ? null : latest} wikilinks />
  );
  const redraw = () => {
    const draw = rerender;
    if (draw) {
      act(() => {
        draw(ui());
      });
    }
  };
  const main = build({
    images: imagePreviews("eng"),
    onFloatingContext: (next) => {
      latest = next;
      redraw();
    },
  });
  return {
    main,
    context: () => latest,
    show: () => {
      rerender = render(ui(), { wrapper: Tooltips }).rerender;
    },
    raw: (on) => {
      raw = on;
      setRaw(main, on);
      redraw();
    },
  };
}

/** Selects the first `ref` in the text and waits for the bar's context. */
async function selectPicture(h: Harness, ref = PICTURE): Promise<number> {
  const from = bytes(h.main.state).indexOf(ref);
  h.main.focus();
  h.main.dispatch({ selection: { anchor: from, head: from + ref.length } });
  await vi.waitFor(() => {
    expect(h.context()).toMatchObject({ kind: "image", from });
  });
  return from;
}

function bar(): HTMLElement {
  return screen.getByRole("toolbar", { name: "Picture options" });
}

function control(name: string): HTMLElement {
  return within(bar()).getByRole("button", { name });
}

function altField(): HTMLInputElement {
  return within(bar()).getByRole<HTMLInputElement>("textbox", {
    name: "Alt text",
  });
}

/** The text a verb gives on a fresh view of `doc` with the same picture selected. */
function expected(doc: string, run: (view: EditorView) => boolean): string {
  const twin = markView(doc, { images: imagePreviews("eng") });
  run(twin);
  return bytes(twin.state);
}

/** Lets a commit the bar defers past React's cleanup run. */
async function settle(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
  });
}

const BUTTONS: [string, (view: EditorView, at: number) => boolean][] = [
  ...(
    [
      ["Left with text wrap", "left"],
      ["Centered", "center"],
      ["Right with text wrap", "right"],
    ] as [string, PictureAlign][]
  ).map(
    ([name, align]): [string, (view: EditorView, at: number) => boolean] => [
      name,
      (view, at) => setPictureAlign(view, align, at),
    ],
  ),
  ...(
    [
      ["Small", "small"],
      ["Medium", "medium"],
      ["Full width", "full"],
      ["Original size", "original"],
    ] as [string, PictureSize][]
  ).map(([name, size]): [string, (view: EditorView, at: number) => boolean] => [
    name,
    (view, at) => setPictureSize(view, size, at),
  ]),
];

describe("the picture buttons", () => {
  for (const [name, verb] of BUTTONS) {
    test(`${name} writes what its verb writes, as one undo step`, async () => {
      // A place and a width no button has, so every one changes something.
      const fragment = name.startsWith("Right")
        ? "#left,w=75%"
        : "#right,w=75%";
      const ref = `![Chart](assets/c.png${fragment})`;
      const START = `Before\n\n${ref}\n\nAfter\n`;
      const h = harness(START);
      const from = await selectPicture(h, ref);
      h.show();
      const want = expected(START, (twin) => verb(twin, from + 1));
      fireEvent.click(control(name));
      expect(bytes(h.main.state)).toBe(want);
      expect(want).not.toBe(START);
      // The picture stays selected, and the focus is back in the text.
      const sel = h.main.state.selection.main;
      expect(h.main.state.sliceDoc(sel.from, sel.to)).toMatch(/^!\[Chart\]/);
      expect(h.main.hasFocus).toBe(true);
      expect(undoSteps(h.main)).toBe(1);
      undoOnce(h.main);
      expect(bytes(h.main.state)).toBe(START);
    });
  }

  test("at a junction of two pictures the right one is changed (P22)", async () => {
    const doc = "![a](assets/a.png)![b](assets/b.png)\n";
    const h = harness(doc);
    await selectPicture(h, "![b](assets/b.png)");
    h.show();
    fireEvent.click(control("Small"));
    expect(bytes(h.main.state)).toBe(
      "![a](assets/a.png)![b](assets/b.png#w=25%)\n",
    );
  });

  test("a press on the size it already has writes nothing and says nothing", async () => {
    const doc = "![Chart](assets/c.png#w=25%)\n";
    const h = harness(doc);
    await selectPicture(h, "![Chart](assets/c.png#w=25%)");
    h.show();
    fireEvent.click(control("Small"));
    expect(bytes(h.main.state)).toBe(doc);
    expect(h.main.hasFocus).toBe(true);
  });

  test("Remove picture removes it and the focus goes to the text", async () => {
    const h = harness();
    const from = await selectPicture(h);
    h.show();
    const want = expected(DOC, (twin) => removePicture(twin, from + 1));
    fireEvent.click(control("Remove picture"));
    expect(bytes(h.main.state)).toBe(want);
    expect(bytes(h.main.state)).not.toContain("![Chart]");
    expect(h.main.hasFocus).toBe(true);
  });
});

describe("the pressed states", () => {
  const pressed = (name: string) =>
    control(name).getAttribute("aria-pressed") === "true";
  const ALL = [
    "Left with text wrap",
    "Centered",
    "Right with text wrap",
    "Small",
    "Medium",
    "Full width",
    "Original size",
  ];

  async function pressedOn(ref: string): Promise<string[]> {
    const h = harness(`${ref}\n`);
    await selectPicture(h, ref);
    h.show();
    for (const name of ALL) {
      expect(control(name)).toHaveAttribute("aria-pressed");
    }
    return ALL.filter(pressed);
  }

  test("#left,w=25% presses Left with text wrap and Small", async () => {
    expect(await pressedOn("![C](assets/c.png#left,w=25%)")).toEqual([
      "Left with text wrap",
      "Small",
    ]);
  });

  test("#full presses Full width and no placement", async () => {
    expect(await pressedOn("![C](assets/c.png#full)")).toEqual(["Full width"]);
  });

  test("no fragment presses Centered and Original size", async () => {
    expect(await pressedOn("![C](assets/c.png)")).toEqual([
      "Centered",
      "Original size",
    ]);
  });

  test("another width presses no size", async () => {
    expect(await pressedOn("![C](assets/c.png#right,w=75%)")).toEqual([
      "Right with text wrap",
    ]);
  });

  test("a change that keeps the reference's length is drawn too", async () => {
    const doc = "![C](assets/c.png#left)\n";
    const h = harness(doc);
    await selectPicture(h, "![C](assets/c.png#left)");
    h.show();
    fireEvent.click(control("Right with text wrap"));
    expect(bytes(h.main.state)).toBe("![C](assets/c.png#right)\n");
    await vi.waitFor(() => {
      expect(control("Right with text wrap")).toHaveAttribute(
        "aria-pressed",
        "true",
      );
    });
    expect(control("Left with text wrap")).toHaveAttribute(
      "aria-pressed",
      "false",
    );
  });
});

describe("the alt text field", () => {
  test("holds the alt text as it reads, escapes taken out", async () => {
    const ref = "![a \\[b\\] c](assets/c.png)";
    const h = harness(`${ref}\n`);
    await selectPicture(h, ref);
    h.show();
    expect(altField().value).toBe("a [b] c");
    expect(altField()).toHaveAttribute("placeholder", "Describe the picture");
  });

  test("Enter writes only what was typed and returns the focus", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    // Nothing is written while typing.
    expect(bytes(h.main.state)).toBe(DOC);
    fireEvent.keyDown(field, { key: "Enter" });
    expect(bytes(h.main.state)).toBe(DOC.replace("![Chart]", "![Chart 2]"));
    expect(undoSteps(h.main)).toBe(1);
    expect(h.main.hasFocus).toBe(true);
    const sel = h.main.state.selection.main;
    expect(h.main.state.sliceDoc(sel.from, sel.to)).toBe(
      "![Chart 2](assets/c.png)",
    );
  });

  test("Escape restores the field, writes nothing and returns the focus", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Something else" } });
    fireEvent.keyDown(field, { key: "Escape" });
    await settle();
    expect(bytes(h.main.state)).toBe(DOC);
    expect(undoSteps(h.main)).toBe(0);
    expect(h.main.hasFocus).toBe(true);
  });

  test("typing then clicking into the text commits once", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    // A click in the text: the focus goes back first, then the caret moves.
    act(() => {
      h.main.focus();
    });
    h.main.dispatch({ selection: { anchor: 1 }, userEvent: "select.pointer" });
    await vi.waitFor(() => {
      expect(h.context()).toBeNull();
    });
    await settle();
    expect(bytes(h.main.state)).toBe(DOC.replace("![Chart]", "![Chart 2]"));
    expect(undoSteps(h.main)).toBe(1);
  });

  test("typing then Raw on commits once (Review Focus 5)", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    h.raw(true);
    await settle();
    expect(screen.queryByRole("toolbar")).toBeNull();
    expect(bytes(h.main.state)).toBe(DOC.replace("![Chart]", "![Chart 2]"));
    expect(undoSteps(h.main)).toBe(1);
    await settle();
    expect(undoSteps(h.main)).toBe(1);
  });

  test("typing then a rebuilt state commits once", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    act(() => {
      h.main.setState(h.main.state.update({ selection: { anchor: 0 } }).state);
    });
    await vi.waitFor(() => {
      expect(screen.queryByRole("toolbar")).toBeNull();
    });
    await settle();
    expect(bytes(h.main.state)).toBe(DOC.replace("![Chart]", "![Chart 2]"));
    // The write leaves the selection where the close put it.
    expect(h.main.state.selection.main.head).toBe(0);
    await settle();
    expect(bytes(h.main.state)).toBe(DOC.replace("![Chart]", "![Chart 2]"));
    expect(screen.queryByRole("toolbar")).toBeNull();
  });

  test("a rebuilt state with another picture in the place writes nothing", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    // The same length at the same place, but another file.
    const other = DOC.replace("assets/c.png", "assets/d.png");
    act(() => {
      h.main.setState(
        h.main.state.update({
          changes: { from: 0, to: h.main.state.doc.length, insert: other },
          selection: { anchor: 0 },
        }).state,
      );
    });
    await vi.waitFor(() => {
      expect(screen.queryByRole("toolbar")).toBeNull();
    });
    await settle();
    expect(bytes(h.main.state)).toBe(other);
  });

  test("a peer deleting the picture closes the bar and nothing is written", async () => {
    // The same picture again right after it: what moves into the deleted
    // one's place must not take the change either.
    const twice = `Before\n\n${PICTURE} ${PICTURE}\n\nAfter\n`;
    let peers: ReturnType<typeof markPeers> | null = null;
    const h = harness(twice, (options) => {
      peers = markPeers(twice, options);
      return peers.local;
    });
    if (peers === null) throw new Error("no peers");
    const { peer, sync } = peers as ReturnType<typeof markPeers>;
    const from = await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    peer.dispatch({ changes: { from, to: from + PICTURE.length + 1 } });
    act(() => {
      sync();
    });
    await vi.waitFor(() => {
      expect(screen.queryByRole("toolbar")).toBeNull();
    });
    await settle();
    act(() => {
      sync();
    });
    const left = `Before\n\n${PICTURE}\n\nAfter\n`;
    expect(bytes(h.main.state)).toBe(left);
    expect(bytes(peer.state)).toBe(left);
    expect(undoSteps(h.main)).toBe(0);
  });

  test("a peer typing before the picture keeps the change on the same picture", async () => {
    let peers: ReturnType<typeof markPeers> | null = null;
    const h = harness(DOC, (options) => {
      peers = markPeers(DOC, options);
      return peers.local;
    });
    if (peers === null) throw new Error("no peers");
    const { peer, sync } = peers as ReturnType<typeof markPeers>;
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    peer.dispatch({ changes: { from: 0, insert: "Very " } });
    act(() => {
      sync();
    });
    fireEvent.keyDown(field, { key: "Enter" });
    act(() => {
      sync();
    });
    const want = `Very Before\n\n![Chart 2](assets/c.png)\n\nAfter\n`;
    expect(bytes(h.main.state)).toBe(want);
    expect(bytes(peer.state)).toBe(want);
  });

  test("Left at the start goes to Original size, Right at the end to Remove picture", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    // Inside the text the arrows are the field's own.
    field.setSelectionRange(2, 2);
    fireEvent.keyDown(field, { key: "ArrowLeft" });
    expect(field).toHaveFocus();
    fireEvent.keyDown(field, { key: "Home" });
    expect(field).toHaveFocus();
    fireEvent.keyDown(field, { key: "End" });
    expect(field).toHaveFocus();
    field.setSelectionRange(0, 0);
    fireEvent.keyDown(field, { key: "ArrowLeft" });
    expect(control("Original size")).toHaveFocus();
    fireEvent.keyDown(control("Original size"), { key: "ArrowRight" });
    expect(field).toHaveFocus();
    field.setSelectionRange(field.value.length, field.value.length);
    fireEvent.keyDown(field, { key: "ArrowRight" });
    expect(control("Remove picture")).toHaveFocus();
  });

  test("a press on the field is not cancelled, a press on a button is", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    expect(fireEvent.mouseDown(altField())).toBe(true);
    expect(fireEvent.mouseDown(control("Small"))).toBe(false);
  });

  test("moving on to a button commits the alt before the button runs", async () => {
    const h = harness();
    await selectPicture(h);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Chart 2" } });
    fireEvent.click(control("Small"));
    expect(bytes(h.main.state)).toBe(
      DOC.replace(PICTURE, "![Chart 2](assets/c.png#w=25%)"),
    );
    expect(undoSteps(h.main)).toBe(2);
  });
});

describe("a picture that is not attached", () => {
  const EXTERNAL = "![Map](https://example.com/m.png)";

  test("shows the one line and no placement or size buttons", async () => {
    const h = harness(`${EXTERNAL}\n`);
    await selectPicture(h, EXTERNAL);
    h.show();
    expect(
      within(bar()).getByText(
        "Size and place can be set only for attached pictures.",
      ),
    ).toBeInTheDocument();
    for (const name of [
      "Left with text wrap",
      "Centered",
      "Right with text wrap",
      "Small",
      "Medium",
      "Full width",
      "Original size",
    ]) {
      expect(within(bar()).queryByRole("button", { name })).toBeNull();
    }
    expect(altField().value).toBe("Map");
    expect(control("Remove picture")).toBeInTheDocument();
  });

  test("its alt text can still be changed", async () => {
    const doc = `${EXTERNAL}\n`;
    const h = harness(doc);
    await selectPicture(h, EXTERNAL);
    h.show();
    const field = altField();
    act(() => {
      field.focus();
    });
    fireEvent.change(field, { target: { value: "Old map" } });
    fireEvent.keyDown(field, { key: "Enter" });
    expect(bytes(h.main.state)).toBe(doc.replace("![Map]", "![Old map]"));
  });
});
