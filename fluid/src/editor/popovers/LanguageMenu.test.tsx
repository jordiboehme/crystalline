/**
 * The code box's menus. "Code language" filters and lists the languages, the
 * current one checked, and a pick writes only the info word. "Code block
 * actions" adds a paragraph below the box, the way out of a box that ends the
 * document. Each pick is one undo step; Escape, a press outside and the
 * focus leaving write nothing.
 */

import { LanguageDescription } from "@codemirror/language";
import { languages } from "@codemirror/language-data";
import { EditorState, StateEffect } from "@codemirror/state";
import type { EditorView } from "@codemirror/view";
import { cleanup, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";

import {
  bytes,
  caret,
  cleanupMarkViews,
  keys,
  setCaret,
  setRaw,
  setShowMarks,
  typeText,
  undoOnce,
} from "../../test/markKit";
import { popoverPeers, popoverView } from "../../test/popoverKit";
import { LANGUAGE_CHOICES, languageName } from "../codeBlocks";

afterEach(() => {
  cleanup();
  cleanupMarkViews();
});

type Mode = "hidden" | "show";
const MODES: Mode[] = ["hidden", "show"];
const CASES = MODES.flatMap((mode) =>
  [false, true].map((room) => ({ mode, room })),
);

const DOC = "a\n\n```ts\nx\n```\n\nb";

function press(el: Element): MouseEvent {
  const event = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
  });
  el.dispatchEvent(event);
  return event;
}

function languageButton(view: EditorView, index = 0): HTMLElement {
  const el = view.contentDOM.querySelectorAll<HTMLElement>(
    'button[aria-label^="Code language"]',
  )[index];
  if (!el) throw new Error("no language button is drawn");
  return el;
}

function moreButton(view: EditorView): HTMLElement {
  const el = view.contentDOM.querySelector<HTMLElement>(
    'button[aria-label="More code block actions"]',
  );
  if (!el) throw new Error("no actions button is drawn");
  return el;
}

/**
 * The language menu once it is ready: drawn, and its first field given the
 * focus by the popover's effect, which runs after the menu is in the page.
 * On a loaded machine a read between the two found the focus still on the
 * buffer.
 */
async function languageMenu(): Promise<HTMLElement> {
  const menu = await screen.findByRole("menu", { name: "Code language" });
  await waitFor(() => {
    expect(menu.contains(document.activeElement)).toBe(true);
  });
  return menu;
}

async function actionsMenu(): Promise<HTMLElement> {
  return screen.findByRole("menu", { name: "Code block actions" });
}

describe("the language choices", () => {
  it("start with Plain text and leave out the words that hold a space", () => {
    expect(LANGUAGE_CHOICES[0]).toEqual({ name: "Plain text", word: null });
    const names = LANGUAGE_CHOICES.map((choice) => choice.name);
    expect(names).toContain("TypeScript");
    expect(names).not.toContain("MS SQL");
    for (const choice of LANGUAGE_CHOICES) {
      expect(choice.word ?? "").not.toMatch(/[\s`]/);
    }
  });

  it("write a word that names the same language again", () => {
    for (const choice of LANGUAGE_CHOICES) {
      expect(languageName(choice.word)).toBe(choice.name);
    }
  });

  it("write the first alias, or the name in lower case", () => {
    const python = LANGUAGE_CHOICES.find((c) => c.name === "Python");
    expect(python?.word).toBe("python");
    const ts = LanguageDescription.matchLanguageName(languages, "TypeScript");
    expect(LANGUAGE_CHOICES.find((c) => c.name === "TypeScript")?.word).toBe(
      ts?.alias[0],
    );
  });
});

describe.each(CASES)(
  "the language menu ($mode mode, room $room)",
  ({ mode, room }) => {
    it("opens from the header with the filter focused and the language checked", async () => {
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      expect(
        screen.getByRole("textbox", { name: "Filter languages" }),
      ).toHaveFocus();
      const items = screen.getAllByRole("menuitemradio");
      expect(items[0]?.textContent).toBe("Plain text");
      expect(
        screen.getByRole("menuitemradio", { name: "TypeScript" }),
      ).toHaveAttribute("aria-checked", "true");
      expect(
        screen.getByRole("menuitemradio", { name: "Plain text" }),
      ).toHaveAttribute("aria-checked", "false");
    });

    it("writes the picked language's word as one undo step", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.click(screen.getByRole("menuitemradio", { name: "Python" }));
      expect(bytes(view.state)).toBe("a\n\n```python\nx\n```\n\nb");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(document.activeElement).toBe(view.contentDOM);
      undoOnce(view);
      expect(bytes(view.state)).toBe(DOC);
    });

    it("takes the word away for Plain text", async () => {
      const user = userEvent.setup();
      const view = popoverView("a\n\n```ts\nx\n```", { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.click(
        screen.getByRole("menuitemradio", { name: "Plain text" }),
      );
      expect(bytes(view.state)).toBe("a\n\n```\nx\n```");
    });

    it("filters the list and picks the first match with Enter", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.keyboard("rus");
      const names = screen
        .getAllByRole("menuitemradio")
        .map((item) => item.textContent);
      expect(names).toContain("Rust");
      expect(names).not.toContain("TypeScript");
      // The field only filters: nothing is written while typing.
      expect(bytes(view.state)).toBe(DOC);
      await user.keyboard("{Enter}");
      expect(bytes(view.state)).toBe(
        `a\n\n\`\`\`${String(
          LANGUAGE_CHOICES.find((c) => c.name === names[0])?.word,
        )}\nx\n\`\`\`\n\nb`,
      );
    });

    it("picks the checked language on Enter in an empty filter", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.keyboard("{Enter}");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(bytes(view.state)).toBe(DOC);
    });

    it("does nothing on Enter when nothing matches", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.keyboard("zzzz{Enter}");
      expect(
        screen.getByRole("menu", { name: "Code language" }),
      ).not.toBeNull();
      expect(bytes(view.state)).toBe(DOC);
    });

    it("keeps the word and says why when Plain text would lose the info after it", async () => {
      const user = userEvent.setup();
      const text = "a\n\n```ts title=x\nx\n```";
      const view = popoverView(text, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.click(
        screen.getByRole("menuitemradio", { name: "Plain text" }),
      );
      expect(bytes(view.state)).toBe(text);
      expect(screen.getByRole("status").textContent).toBe(
        "Plain text cannot keep the info after the language.",
      );
      await user.click(screen.getByRole("menuitemradio", { name: "Rust" }));
      expect(bytes(view.state)).toBe("a\n\n```rust title=x\nx\n```");
    });

    it.each(["language", "actions"])(
      "writes nothing when the focus leaves the %s menu",
      async (which) => {
        const view = popoverView(DOC, { mode, room });
        setCaret(view, 0);
        if (which === "language") {
          press(languageButton(view));
          await languageMenu();
        } else {
          press(moreButton(view));
          await actionsMenu();
        }
        const outside = document.createElement("button");
        document.body.appendChild(outside);
        outside.focus();
        await waitFor(() => {
          expect(screen.queryByRole("menu")).toBeNull();
        });
        expect(bytes(view.state)).toBe(DOC);
        outside.remove();
      },
    );

    it("moves from the filter into the list with the arrow keys", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.keyboard("python");
      await user.keyboard("{Home}");
      expect(
        screen.getByRole("textbox", { name: "Filter languages" }),
      ).toHaveFocus();
      await user.keyboard("{ArrowDown}");
      expect(
        screen.getByRole("menuitemradio", { name: "Python" }),
      ).toHaveFocus();
      await user.keyboard("{Enter}");
      expect(bytes(view.state)).toBe("a\n\n```python\nx\n```\n\nb");
    });

    it("writes nothing on Escape and gives the focus back", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.keyboard("py{ArrowDown}{Escape}");
      expect(screen.queryByRole("menu")).toBeNull();
      expect(bytes(view.state)).toBe(DOC);
      expect(document.activeElement).toBe(view.contentDOM);
    });

    it("writes nothing on a press outside", async () => {
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      document.body.dispatchEvent(
        new PointerEvent("pointerdown", { bubbles: true }),
      );
      await waitFor(() => {
        expect(screen.queryByRole("menu")).toBeNull();
      });
      expect(bytes(view.state)).toBe(DOC);
    });

    it("writes nothing for Plain text on a word no language has", async () => {
      const user = userEvent.setup();
      const view = popoverView("a\n\n```foo\nx\n```", { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      expect(
        screen.getByRole("menuitemradio", { name: "Plain text" }),
      ).toHaveAttribute("aria-checked", "true");
      await user.click(
        screen.getByRole("menuitemradio", { name: "Plain text" }),
      );
      expect(bytes(view.state)).toBe("a\n\n```foo\nx\n```");
    });

    it("writes nothing for the language already checked", async () => {
      const user = userEvent.setup();
      const view = popoverView("a\n\n```typescript\nx\n```", { mode, room });
      setCaret(view, 0);
      press(languageButton(view));
      await languageMenu();
      await user.click(
        screen.getByRole("menuitemradio", { name: "TypeScript" }),
      );
      expect(screen.queryByRole("menu")).toBeNull();
      expect(bytes(view.state)).toBe("a\n\n```typescript\nx\n```");
    });
  },
);

describe.each(CASES)(
  "the actions menu ($mode mode, room $room)",
  ({ mode, room }) => {
    it("adds a paragraph below a box that ends the document", async () => {
      const user = userEvent.setup();
      const view = popoverView("```\nx\n```", { mode, room });
      setCaret(view, 4);
      if (mode === "hidden") {
        // ArrowDown stays: the box ends the document.
        keys(view, "ArrowDown");
        expect(caret(view)).toBe(4);
        press(moreButton(view));
      } else {
        // The caret on the only code line shows the fences, so the
        // keyboard opens the menu.
        keys(view, "Shift-F10");
      }
      await actionsMenu();
      expect(
        screen.getAllByRole("menuitem").map((item) => item.textContent),
      ).toEqual(["Code language", "Add a paragraph below"]);
      await user.click(
        screen.getByRole("menuitem", { name: "Add a paragraph below" }),
      );
      expect(bytes(view.state)).toBe("```\nx\n```\n");
      expect(caret(view)).toBe(10);
      expect(view.state.doc.lineAt(caret(view)).number).toBe(
        view.state.doc.lines,
      );
      expect(document.activeElement).toBe(view.contentDOM);
      undoOnce(view);
      expect(bytes(view.state)).toBe("```\nx\n```");
      // Wherever the undo left the caret, typing never writes into a fence.
      typeText(view, "z");
      expect(bytes(view.state)).toMatch(/^```\n[xz]*\n```(?:\nz)?$/);
      expect(bytes(view.state).length).toBe(10);
    });

    it("opens on Shift+F10 and goes on to the language menu", async () => {
      const user = userEvent.setup();
      const view = popoverView(DOC, { mode, room });
      setCaret(view, 9);
      keys(view, "Shift-F10");
      await actionsMenu();
      expect(
        screen.getByRole("menuitem", { name: "Code language" }),
      ).toHaveFocus();
      await user.keyboard("{Enter}");
      await languageMenu();
      expect(
        screen.getByRole("textbox", { name: "Filter languages" }),
      ).toHaveFocus();
      await user.keyboard("rust{Enter}");
      expect(bytes(view.state)).toBe("a\n\n```rust\nx\n```\n\nb");
    });
  },
);

describe("the menus and their peers", () => {
  it("write nothing to the next box when a peer deleted the one asked about", async () => {
    const user = userEvent.setup();
    const text = "a\n\n```ts\nx\n```\n\n```ts\ny\n```";
    const { local, peer, sync } = popoverPeers(text);
    setCaret(local, 0);
    press(languageButton(local));
    await languageMenu();
    const python = screen.getByRole("menuitemradio", { name: "Python" });
    peer.dispatch({ changes: { from: 3, to: 16 }, userEvent: "delete" });
    sync();
    expect(bytes(local.state)).toBe("a\n\n```ts\ny\n```");
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    // A late click on what was the menu writes nothing either.
    if (python.isConnected) await user.click(python);
    expect(bytes(local.state)).toBe("a\n\n```ts\ny\n```");
  });

  it("replace the word as it is now when a peer changed it", async () => {
    const user = userEvent.setup();
    const { local, peer, sync } = popoverPeers(DOC);
    setCaret(local, 0);
    press(languageButton(local));
    await languageMenu();
    peer.dispatch({
      changes: { from: 6, to: 8, insert: "rust" },
      userEvent: "input",
    });
    sync();
    expect(bytes(local.state)).toBe("a\n\n```rust\nx\n```\n\nb");
    await user.click(screen.getByRole("menuitemradio", { name: "TypeScript" }));
    expect(bytes(local.state)).toBe(DOC);
  });

  it("adds no paragraph below the next box when a peer deleted the one asked about", async () => {
    const user = userEvent.setup();
    const text = "a\n\n```ts\nx\n```\n\n```ts\ny\n```";
    const { local, peer, sync } = popoverPeers(text);
    setCaret(local, 0);
    press(moreButton(local));
    await actionsMenu();
    const add = screen.getByRole("menuitem", { name: "Add a paragraph below" });
    peer.dispatch({ changes: { from: 3, to: 16 }, userEvent: "delete" });
    sync();
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    if (add.isConnected) await user.click(add);
    expect(bytes(local.state)).toBe("a\n\n```ts\ny\n```");
  });
});

describe("the menus close without writing", () => {
  it.each([
    ["the setting flips", (view: EditorView) => setShowMarks(view, true)],
    ["Raw flips", (view: EditorView) => setRaw(view, true)],
  ])("when %s", async (_name, flip) => {
    const view = popoverView(DOC);
    press(languageButton(view));
    await languageMenu();
    flip(view);
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(view.state)).toBe(DOC);
  });
});

describe("a page that takes no writes (C12)", () => {
  function readOnly(view: EditorView): void {
    view.dispatch({
      effects: StateEffect.appendConfig.of(EditorState.readOnly.of(true)),
    });
  }

  it("closes the language menu without writing", async () => {
    const view = popoverView(DOC, { adapter: "reflection" });
    press(languageButton(view));
    await languageMenu();
    readOnly(view);
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(view.state)).toBe(DOC);
  });

  it("closes the actions menu without adding a paragraph", async () => {
    const view = popoverView("```\nx\n```", { adapter: "reflection" });
    press(moreButton(view));
    await actionsMenu();
    readOnly(view);
    await waitFor(() => {
      expect(screen.queryByRole("menu")).toBeNull();
    });
    expect(bytes(view.state)).toBe("```\nx\n```");
  });

  it("opens neither menu", async () => {
    const view = popoverView(DOC, { adapter: "reflection" });
    readOnly(view);
    press(languageButton(view));
    press(moreButton(view));
    await Promise.resolve();
    expect(screen.queryByRole("menu")).toBeNull();
    expect(bytes(view.state)).toBe(DOC);
  });
});

describe("two boxes", () => {
  it("each header asks about its own box", async () => {
    const user = userEvent.setup();
    const text = "```ts\nx\n```\n\n```rust\ny\n```";
    const view = popoverView(text);
    press(languageButton(view, 1));
    await languageMenu();
    expect(screen.getByRole("menuitemradio", { name: "Rust" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    await user.click(screen.getByRole("menuitemradio", { name: "Python" }));
    expect(bytes(view.state)).toBe("```ts\nx\n```\n\n```python\ny\n```");
  });
});
